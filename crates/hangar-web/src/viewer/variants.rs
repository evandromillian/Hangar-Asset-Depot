//! Texture variants ("skins") and live material previews: swap textures of
//! StandardMaterials by material name, mirroring what the pipeline bakes.

use super::model::ModelState;
use super::{HiddenTexture, ViewerCommand, ViewerEvent};
use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfMaterialName;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use hangar_core::TextureSlot;
use std::collections::HashMap;

/// Texture handles and factors of a material as baked into the GLB.
#[derive(Clone)]
struct OriginalMaterial {
    base_color: Color,
    metallic: f32,
    roughness: f32,
    emissive: LinearRgba,
    base_color_texture: Option<Handle<Image>>,
    normal: Option<Handle<Image>>,
    metallic_roughness: Option<Handle<Image>>,
    occlusion: Option<Handle<Image>>,
    emissive_texture: Option<Handle<Image>>,
}

impl OriginalMaterial {
    fn of(m: &StandardMaterial) -> Self {
        Self {
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.perceptual_roughness,
            emissive: m.emissive,
            base_color_texture: m.base_color_texture.clone(),
            normal: m.normal_map_texture.clone(),
            metallic_roughness: m.metallic_roughness_texture.clone(),
            occlusion: m.occlusion_texture.clone(),
            emissive_texture: m.emissive_texture.clone(),
        }
    }

    fn restore(&self, m: &mut StandardMaterial) {
        m.base_color = self.base_color;
        m.metallic = self.metallic;
        m.perceptual_roughness = self.roughness;
        m.emissive = self.emissive;
        m.base_color_texture = self.base_color_texture.clone();
        m.normal_map_texture = self.normal.clone();
        m.metallic_roughness_texture = self.metallic_roughness.clone();
        m.occlusion_texture = self.occlusion.clone();
        m.emissive_texture = self.emissive_texture.clone();
    }
}

#[derive(Resource, Default)]
pub struct VariantState {
    /// (material name, slot, decoded image) of the active variant; empty = default.
    active: Vec<(String, TextureSlot, Handle<Image>)>,
    /// Textures switched off by the materials panel.
    hidden: Vec<HiddenTexture>,
    originals: HashMap<AssetId<StandardMaterial>, OriginalMaterial>,
    /// Metal/rough maps packed from separate roughness / metallic overrides,
    /// kept alive while applied.
    packed: Vec<Handle<Image>>,
    /// Generation of `ModelState` the active variant was last applied to.
    applied_generation: Option<u64>,
    dirty: bool,
}

pub fn handle_variant_commands(
    mut reader: MessageReader<ViewerCommand>,
    mut state: ResMut<VariantState>,
    mut images: ResMut<Assets<Image>>,
    mut events: MessageWriter<ViewerEvent>,
) {
    for cmd in reader.read() {
        match cmd {
            ViewerCommand::ApplyVariant { textures, hidden } => {
                let mut active = Vec::new();
                for t in textures {
                    let ext = if t.ext.is_empty() { "png" } else { t.ext.as_str() };
                    match Image::from_buffer(
                        &t.bytes,
                        ImageType::Extension(ext),
                        CompressedImageFormats::NONE,
                        t.slot.is_srgb(),
                        ImageSampler::Default,
                        RenderAssetUsages::default(),
                    ) {
                        Ok(img) => active.push((t.material.clone(), t.slot, images.add(img))),
                        Err(e) => {
                            events.write(ViewerEvent::Error(format!(
                                "Textura inválida para {}: {e}",
                                t.material
                            )));
                        }
                    }
                }
                state.active = active;
                state.hidden = hidden.clone();
                state.dirty = true;
            }
            ViewerCommand::ResetVariant => {
                state.active.clear();
                state.hidden.clear();
                state.dirty = true;
            }
            _ => {}
        }
    }
}

/// Applies (or reverts) the active variant whenever it changes or a new model appears.
pub fn apply_pending_variant(
    mut state: ResMut<VariantState>,
    model: Res<ModelState>,
    names: Query<(&GltfMaterialName, &MeshMaterial3d<StandardMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if model.meshes.is_empty() {
        return;
    }
    if !state.dirty && state.applied_generation == Some(model.generation) {
        return;
    }
    let state = &mut *state;
    state.packed.clear();
    let mut seen = std::collections::HashSet::new();
    for &e in &model.meshes {
        let Ok((name, mat)) = names.get(e) else { continue };
        let id = mat.0.id();
        if !seen.insert(id) {
            continue;
        }
        let Some(mut material) = materials.get_mut(&mat.0) else { continue };
        let original = state.originals.entry(id).or_insert_with(|| OriginalMaterial::of(&material)).clone();
        // Start from the originals, then layer the variant on top.
        original.restore(&mut material);
        let hidden = |slot: TextureSlot| {
            state.hidden.iter().find(|h| h.slot == slot && h.material.eq_ignore_ascii_case(&name.0))
        };
        let ours = |slot: TextureSlot| {
            if hidden(slot).is_some() {
                return None;
            }
            state
                .active
                .iter()
                .find(|(m, s, _)| *s == slot && m.eq_ignore_ascii_case(&name.0))
                .map(|(_, _, h)| h.clone())
        };
        // Same factor rules as the pipeline's GLB export: a texture replaces the factor.
        if let Some(h) = ours(TextureSlot::BaseColor) {
            material.base_color = Color::WHITE.with_alpha(original.base_color.alpha());
            material.base_color_texture = Some(h);
        }
        if let Some(h) = ours(TextureSlot::Normal) {
            material.normal_map_texture = Some(h);
        }
        // Hiding any map of the metal/rough family invalidates the baked metal/rough
        // texture: drop it and rebuild from the visible maps (the materials panel
        // sends all of them), over the material's source factors.
        let family = [TextureSlot::Orm, TextureSlot::MetallicRoughness, TextureSlot::Roughness, TextureSlot::Metallic];
        let family_hidden = family.iter().find_map(|s| hidden(*s)).cloned();
        let (rough_factor, metal_factor) = match &family_hidden {
            Some(h) => (h.roughness, h.metallic),
            None => (original.roughness, original.metallic),
        };
        if family_hidden.is_some() {
            material.metallic_roughness_texture = None;
            material.metallic = metal_factor;
            material.perceptual_roughness = rough_factor;
            if hidden(TextureSlot::Orm).is_some() {
                material.occlusion_texture = None;
            }
        }
        if let Some(h) = ours(TextureSlot::Orm) {
            material.metallic_roughness_texture = Some(h.clone());
            material.occlusion_texture = Some(h);
            material.metallic = 1.0;
            material.perceptual_roughness = 1.0;
        } else {
            let mr = ours(TextureSlot::MetallicRoughness).or_else(|| {
                let (r, m) = (ours(TextureSlot::Roughness), ours(TextureSlot::Metallic));
                if r.is_none() && m.is_none() {
                    return None;
                }
                let packed = pack_metal_rough(
                    r.and_then(|h| images.get(&h)),
                    m.and_then(|h| images.get(&h)),
                    rough_factor,
                    metal_factor,
                )?;
                let h = images.add(packed);
                state.packed.push(h.clone());
                Some(h)
            });
            if let Some(h) = mr {
                material.metallic_roughness_texture = Some(h);
                material.metallic = 1.0;
                material.perceptual_roughness = 1.0;
            }
            if let Some(h) = ours(TextureSlot::Occlusion) {
                material.occlusion_texture = Some(h);
            }
        }
        if let Some(h) = ours(TextureSlot::Emissive) {
            material.emissive_texture = Some(h);
            material.emissive = LinearRgba::WHITE;
        }
        hide_textures(&mut material, |slot| hidden(slot).cloned());
    }
    state.dirty = false;
    state.applied_generation = Some(model.generation);
}

/// Removes switched-off textures outside the metal/rough family (handled
/// above). A hidden base colour shows neutral grey rather than the source
/// factor, which in FBX files is often black once a map is connected.
fn hide_textures(material: &mut StandardMaterial, hidden: impl Fn(TextureSlot) -> Option<HiddenTexture>) {
    if let Some(h) = hidden(TextureSlot::BaseColor) {
        material.base_color_texture = None;
        material.base_color = Color::srgba(0.72, 0.72, 0.72, h.alpha);
    }
    if hidden(TextureSlot::Normal).is_some() {
        material.normal_map_texture = None;
    }
    if hidden(TextureSlot::Occlusion).is_some() {
        material.occlusion_texture = None;
    }
    if hidden(TextureSlot::Emissive).is_some() {
        material.emissive_texture = None;
        material.emissive = LinearRgba::BLACK;
    }
}

/// Packs grey-scale roughness / metallic maps into a glTF metal/rough texture
/// (G = roughness, B = metallic). A missing map is filled with its factor; the
/// smaller map is sampled nearest-neighbour at the size of the larger one.
fn pack_metal_rough(rough: Option<&Image>, metal: Option<&Image>, rough_factor: f32, metal_factor: f32) -> Option<Image> {
    let r = rough.and_then(Rgba8::of);
    let m = metal.and_then(Rgba8::of);
    let (w, h) = [&r, &m].iter().filter_map(|i| i.as_ref().map(|i| (i.w, i.h))).max_by_key(|(w, h)| w * h)?;
    let fill = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let g = r.as_ref().map(|i| i.red_at(x, y, w, h)).unwrap_or(fill(rough_factor));
            let b = m.as_ref().map(|i| i.red_at(x, y, w, h)).unwrap_or(fill(metal_factor));
            data.extend_from_slice(&[0, g, b, 255]);
        }
    }
    Some(Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

/// RGBA8 pixels of a decoded image (the pipeline's previews are always 8-bit).
struct Rgba8<'a> {
    data: &'a [u8],
    w: u32,
    h: u32,
}

impl<'a> Rgba8<'a> {
    fn of(img: &'a Image) -> Option<Self> {
        let format = img.texture_descriptor.format;
        if !matches!(format, TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb) {
            return None;
        }
        let (w, h) = (img.width(), img.height());
        let data = img.data.as_deref().filter(|d| d.len() >= (w * h * 4) as usize)?;
        Some(Self { data, w, h })
    }

    /// Red channel at (x, y) of a `tw`×`th` target grid.
    fn red_at(&self, x: u32, y: u32, tw: u32, th: u32) -> u8 {
        let sx = (x as u64 * self.w as u64 / tw as u64) as u32;
        let sy = (y as u64 * self.h as u64 / th as u64) as u32;
        self.data[((sy * self.w + sx) * 4) as usize]
    }
}
