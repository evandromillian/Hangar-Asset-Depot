//! Texture variants ("skins"): swap textures of StandardMaterials by material name.

use super::model::ModelState;
use super::{ViewerCommand, ViewerEvent};
use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfMaterialName;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use hangar_core::TextureSlot;
use std::collections::HashMap;

/// Texture handles of a material as baked into the GLB.
#[derive(Clone, Default)]
struct OriginalTextures {
    base_color: Option<Handle<Image>>,
    normal: Option<Handle<Image>>,
    metallic_roughness: Option<Handle<Image>>,
    occlusion: Option<Handle<Image>>,
    emissive: Option<Handle<Image>>,
}

#[derive(Resource, Default)]
pub struct VariantState {
    /// (material name, slot, decoded image) of the active variant; empty = default.
    active: Vec<(String, TextureSlot, Handle<Image>)>,
    originals: HashMap<AssetId<StandardMaterial>, OriginalTextures>,
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
            ViewerCommand::ApplyVariant { textures } => {
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
                state.dirty = true;
            }
            ViewerCommand::ResetVariant => {
                state.active.clear();
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
) {
    if model.meshes.is_empty() {
        return;
    }
    if !state.dirty && state.applied_generation == Some(model.generation) {
        return;
    }
    let state = &mut *state;
    let mut seen = std::collections::HashSet::new();
    for &e in &model.meshes {
        let Ok((name, mat)) = names.get(e) else { continue };
        let id = mat.0.id();
        if !seen.insert(id) {
            continue;
        }
        let Some(mut material) = materials.get_mut(&mat.0) else { continue };
        let original = state
            .originals
            .entry(id)
            .or_insert_with(|| OriginalTextures {
                base_color: material.base_color_texture.clone(),
                normal: material.normal_map_texture.clone(),
                metallic_roughness: material.metallic_roughness_texture.clone(),
                occlusion: material.occlusion_texture.clone(),
                emissive: material.emissive_texture.clone(),
            })
            .clone();
        // Start from the originals, then layer the variant on top.
        material.base_color_texture = original.base_color.clone();
        material.normal_map_texture = original.normal.clone();
        material.metallic_roughness_texture = original.metallic_roughness.clone();
        material.occlusion_texture = original.occlusion.clone();
        material.emissive_texture = original.emissive.clone();
        for (mname, slot, handle) in &state.active {
            if !mname.eq_ignore_ascii_case(&name.0) {
                continue;
            }
            match slot {
                TextureSlot::BaseColor => {
                    material.base_color_texture = Some(handle.clone());
                }
                TextureSlot::Normal => material.normal_map_texture = Some(handle.clone()),
                TextureSlot::MetallicRoughness => material.metallic_roughness_texture = Some(handle.clone()),
                TextureSlot::Orm => {
                    material.metallic_roughness_texture = Some(handle.clone());
                    material.occlusion_texture = Some(handle.clone());
                }
                TextureSlot::Occlusion => material.occlusion_texture = Some(handle.clone()),
                TextureSlot::Emissive => material.emissive_texture = Some(handle.clone()),
                _ => {}
            }
        }
    }
    state.dirty = false;
    state.applied_generation = Some(model.generation);
}
