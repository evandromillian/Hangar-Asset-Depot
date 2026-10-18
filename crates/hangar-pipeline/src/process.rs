//! Orchestration of the pipeline steps.

use crate::glb::{self, EncodedImage, GlbMaterial};
use crate::images::{self, Decoded, ImageCache};
use crate::lod::{self, LodLevel};
use crate::raster::{self, ThumbMaterial};
use crate::scene::{Aabb, Material, MeshPrim, Scene};
use crate::textures::{self, ImageIndex, ResolvedTexture};
use crate::variants::{self, UsedTexture};
use crate::{import, Artifact, ArtifactRole, PipelineConfig, ProcessInput, ProcessOutput};
use anyhow::{anyhow, Result};
use hangar_core::{
    paths, AnimationInfo, LodInfo, MaterialInfo, MaterialTexture, ModelStats, PreviewKind, PreviewManifest, TextureInfo,
    MaterialConfig, TextureOverride, TextureSlot, VariantInfo, MATERIAL_CONFIG_PATH, PIPELINE_VERSION,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

struct Out {
    artifacts: Vec<Artifact>,
    seen: HashSet<String>,
}

impl Out {
    fn push(&mut self, a: Artifact) -> String {
        let h = a.hash.clone();
        if self.seen.insert(h.clone()) {
            self.artifacts.push(a);
        }
        h
    }
}

pub fn run(input: &ProcessInput, cfg: &PipelineConfig) -> Result<ProcessOutput> {
    let mut files: Vec<&hangar_core::FileEntry> = input.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let images: Vec<String> = files.iter().filter(|f| paths::is_image(&f.path)).map(|f| f.path.clone()).collect();
    let models: Vec<String> = files.iter().filter(|f| paths::is_model(&f.path)).map(|f| f.path.clone()).collect();
    let audio: Vec<String> = files.iter().filter(|f| paths::is_audio(&f.path)).map(|f| f.path.clone()).collect();
    let sizes: HashMap<&str, i64> = files.iter().map(|f| (f.path.as_str(), f.size)).collect();

    let mut out = Out { artifacts: vec![], seen: HashSet::new() };
    let mut manifest = PreviewManifest { pipeline_version: PIPELINE_VERSION, ..Default::default() };
    let mut warnings: Vec<String> = Vec::new();
    let mut cache = ImageCache::new(cfg.max_texture_size);
    let manual = load_material_config(input, &files, &mut warnings);

    // ---- texture previews -------------------------------------------------
    let mut preview_by_path: HashMap<String, String> = HashMap::new();
    for img in &images {
        let ext = paths::extension(img);
        let mut info = TextureInfo {
            path: img.clone(),
            width: 0,
            height: 0,
            format: ext.clone(),
            size: sizes.get(img.as_str()).copied().unwrap_or(0).max(0) as u64,
            slot: textures::detect_slot(img),
            preview_hash: None,
        };
        if images::is_decodable_ext(&ext) {
            match cache.get_file(input.root, img) {
                Some(d) => {
                    info.width = d.orig_width;
                    info.height = d.orig_height;
                    info.format = d.format.clone();
                    let enc = cache.encode(&d, cfg.max_texture_size);
                    let h = out.push(Artifact::new(ArtifactRole::TexturePreview(img.clone()), enc.1, enc.0.clone()));
                    preview_by_path.insert(img.clone(), h.clone());
                    info.preview_hash = Some(h);
                }
                None => warnings.push(format!("could not decode image {img}")),
            }
        } else {
            warnings.push(format!("no web preview for .{ext} images ({img})"));
        }
        manifest.textures.push(info);
    }

    if let Some(a) = audio.first() {
        manifest.audio = Some(crate::audio::analyze(input.root, a));
    }

    // ---- model ------------------------------------------------------------
    // With several FBX files, look inside them: animation-only files can't be the
    // preview model, and files whose takes move feed the model's animations.
    let fbx_count = models.iter().filter(|m| paths::extension(m) == "fbx").count();
    let probes: HashMap<&String, import::fbx::FbxProbe> = if fbx_count > 1 {
        models
            .iter()
            .filter(|m| paths::extension(m) == "fbx")
            .filter_map(|m| import::fbx::probe(&input.root.join(m)).ok().map(|p| (m, p)))
            .collect()
    } else {
        HashMap::new()
    };
    let candidates: Vec<String> = models.iter().filter(|m| probes.get(m).is_none_or(|p| p.has_mesh)).cloned().collect();
    let mut model_done = false;
    if let Some(mut sel) = select_model(&candidates) {
        let anim_files: Vec<String> = if paths::extension(&sel.primary) == "fbx" {
            models
                .iter()
                .filter(|m| **m != sel.primary && !sel.lod_files.iter().any(|(_, p)| p == *m))
                .filter(|m| probes.get(m).is_some_and(|p| p.has_animation))
                .cloned()
                .collect()
        } else {
            vec![]
        };
        // Models that are neither previewed nor animation sources.
        sel.ignored = models
            .iter()
            .filter(|m| **m != sel.primary && !sel.lod_files.iter().any(|(_, p)| p == *m) && !anim_files.contains(m))
            .cloned()
            .collect();
        if !sel.ignored.is_empty() {
            warnings.push(format!(
                "{} other model file(s) not previewed: {}",
                sel.ignored.len(),
                sel.ignored.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        let ctx = ModelInputs {
            sel: &sel,
            images: &images,
            preview_by_path: &preview_by_path,
            manual: manual.as_ref(),
            anim_files: &anim_files,
        };
        match build_model(input, cfg, &ctx, &mut cache, &mut out, &mut manifest, &mut warnings) {
            Ok(()) => model_done = true,
            Err(e) => {
                if images.is_empty() && audio.is_empty() {
                    return Err(e.context(format!("model {}", sel.primary)));
                }
                warnings.insert(0, format!("3D preview unavailable for {}: {e:#}", sel.primary));
            }
        }
    }

    if !model_done {
        if let Some(primary) = pick_primary_image(&images, &preview_by_path) {
            manifest.kind = Some(PreviewKind::Image);
            manifest.primary_file = Some(primary.clone());
            if let Some(d) = cache.get_file(input.root, &primary) {
                let thumb = raster::image_thumbnail(&d.image, cfg.thumbnail_size);
                let h = out.push(Artifact::new(ArtifactRole::Thumbnail, "image/png", images::encode_png(&thumb)));
                manifest.thumbnail_hash = Some(h);
            }
        } else if let Some(a) = audio.first() {
            manifest.kind = Some(PreviewKind::Audio);
            manifest.primary_file = Some(a.clone());
        } else {
            manifest.kind = Some(PreviewKind::None);
        }
    }

    manifest.warnings = warnings;
    Ok(ProcessOutput { manifest, artifacts: out.artifacts })
}

/// Which model file to preview, plus authored LOD files.
#[derive(Debug, PartialEq)]
pub struct ModelSelection {
    pub primary: String,
    /// (level, path) sorted by level, including the primary; empty when the
    /// model is not split into LOD files.
    pub lod_files: Vec<(u32, String)>,
    pub ignored: Vec<String>,
}

fn ext_priority(ext: &str) -> usize {
    ["glb", "gltf", "fbx", "obj", "blend", "dae", "3ds", "stl", "ply"]
        .iter()
        .position(|e| *e == ext)
        .unwrap_or(99)
}

pub fn select_model(models: &[String]) -> Option<ModelSelection> {
    if models.is_empty() {
        return None;
    }
    // group LOD files: (dir, stem without lod suffix, ext)
    let mut groups: BTreeMap<(String, String, String), Vec<(u32, String)>> = BTreeMap::new();
    for m in models {
        let stem = paths::stem(m);
        if let Some(l) = lod::parse_lod_suffix(stem) {
            let base = strip_lod(stem).to_ascii_lowercase();
            groups
                .entry((paths::parent(m).to_string(), base, paths::extension(m)))
                .or_default()
                .push((l, m.clone()));
        }
    }
    let mut best: Vec<&String> = models.iter().collect();
    // `Model@Clip.fbx` is the usual name of a separate animation file: never the model if avoidable.
    best.sort_by_key(|m| {
        let anim_name = paths::stem(m).contains('@');
        (ext_priority(&paths::extension(m)), anim_name, m.matches('/').count(), m.to_ascii_lowercase())
    });
    let primary_candidate = best[0].clone();
    // If the best candidate belongs to a LOD group of >= 2 files, use the group.
    let group = groups.values().find(|g| g.len() >= 2 && g.iter().any(|(_, p)| *p == primary_candidate)).cloned();
    let (primary, lod_files) = match group {
        Some(mut g) => {
            g.sort();
            (g[0].1.clone(), g)
        }
        None => (primary_candidate, vec![]),
    };
    let used: HashSet<&String> = lod_files.iter().map(|(_, p)| p).chain(std::iter::once(&primary)).collect();
    let ignored = models.iter().filter(|m| !used.contains(m)).cloned().collect();
    Some(ModelSelection { primary, lod_files, ignored })
}

fn strip_lod(stem: &str) -> &str {
    let lower = stem.to_ascii_lowercase();
    match lower.rfind("lod") {
        Some(i) => stem[..i].trim_end_matches(['_', '-', '.', ' ']),
        None => stem,
    }
}

fn pick_primary_image(images: &[String], previews: &HashMap<String, String>) -> Option<String> {
    let previewable: Vec<&String> = images.iter().filter(|i| previews.contains_key(*i)).collect();
    previewable
        .iter()
        .find(|i| textures::detect_slot(i) == Some(TextureSlot::BaseColor))
        .or(previewable.first())
        .map(|s| (*s).clone())
}

/// Merge `src` materials into `dst` by name, returning the index remap.
fn merge_materials(dst: &mut Vec<Material>, src: &[Material]) -> Vec<usize> {
    src.iter()
        .map(|m| match dst.iter().position(|d| d.name == m.name) {
            Some(i) => i,
            None => {
                dst.push(m.clone());
                dst.len() - 1
            }
        })
        .collect()
}

/// Reads `.hangar/materials.json` when the version has one. A broken file is a
/// warning: the model is still previewed with automatic resolution.
fn load_material_config(
    input: &ProcessInput,
    files: &[&hangar_core::FileEntry],
    warnings: &mut Vec<String>,
) -> Option<MaterialConfig> {
    files.iter().find(|f| f.path == MATERIAL_CONFIG_PATH)?;
    let parsed = std::fs::read(input.root.join(MATERIAL_CONFIG_PATH))
        .map_err(anyhow::Error::from)
        .and_then(|b| MaterialConfig::from_json(&b).map_err(anyhow::Error::from));
    match parsed {
        Ok(c) => Some(c),
        Err(e) => {
            warnings.push(format!("{MATERIAL_CONFIG_PATH} ignored: {e:#}"));
            None
        }
    }
}

struct ModelInputs<'a> {
    sel: &'a ModelSelection,
    images: &'a [String],
    preview_by_path: &'a HashMap<String, String>,
    manual: Option<&'a MaterialConfig>,
    /// Other FBX files whose takes animate the primary model.
    anim_files: &'a [String],
}

fn build_model(
    input: &ProcessInput,
    cfg: &PipelineConfig,
    ctx: &ModelInputs,
    cache: &mut ImageCache,
    out: &mut Out,
    manifest: &mut PreviewManifest,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let ModelInputs { sel, images, preview_by_path, manual, anim_files } = *ctx;
    let scene: Scene = import::import(input.root, &sel.primary, cfg, anim_files)?;
    warnings.extend(scene.warnings.iter().cloned());
    let mut materials = scene.materials.clone();

    // ---- levels of detail
    let mut levels: Vec<LodLevel> = if sel.lod_files.len() >= 2 {
        let mut lv = vec![LodLevel { level: 0, source: hangar_core::LodSource::Authored, meshes: scene.meshes.clone(), ratio: 1.0 }];
        for (_, path) in sel.lod_files.iter().skip(1) {
            match import::import(input.root, path, cfg, &[]) {
                Ok(s) => {
                    let remap = merge_materials(&mut materials, &s.materials);
                    let meshes = s
                        .meshes
                        .into_iter()
                        .map(|mut m| {
                            m.material = m.material.and_then(|i| remap.get(i).copied());
                            // Only the primary file's rig is exported; other LOD files stay static.
                            (m.node, m.skin) = (None, None);
                            m
                        })
                        .collect();
                    lv.push(LodLevel { level: 0, source: hangar_core::LodSource::Authored, meshes, ratio: 1.0 });
                }
                Err(e) => warnings.push(format!("LOD file {path} skipped: {e:#}")),
            }
        }
        lod::renumber_and_ratio(&mut lv);
        lv
    } else if let Some(lv) = lod::split_authored(&scene.meshes) {
        lv
    } else {
        vec![LodLevel { level: 0, source: hangar_core::LodSource::Authored, meshes: scene.meshes.clone(), ratio: 1.0 }]
    };
    if levels.len() == 1 {
        let generated = lod::generate(&levels[0].meshes, &cfg.lod_ratios, cfg.min_triangles_for_lod);
        levels.extend(generated);
    }
    if levels[0].meshes.is_empty() {
        return Err(anyhow!("model has no renderable geometry"));
    }

    // ---- textures
    let index = ImageIndex::new(images.to_vec());
    let resolved = textures::resolve_materials(&materials, &index, manual, warnings);
    let used: Vec<UsedTexture> = materials
        .iter()
        .zip(&resolved)
        .flat_map(|(m, rs)| {
            rs.iter().filter_map(move |r| {
                r.path.as_ref().map(|p| UsedTexture { material: m.name.clone(), slot: r.slot, path: p.clone() })
            })
        })
        .collect();
    let detected = variants::detect(images, &used);
    manifest.variants = detected
        .into_iter()
        .map(|v| VariantInfo {
            name: v.name,
            is_default: v.is_default,
            detected_by: v.detected_by.to_string(),
            overrides: v
                .overrides
                .into_iter()
                .filter_map(|o| match preview_by_path.get(&o.path) {
                    Some(h) => Some(TextureOverride { material: o.material, slot: o.slot, path: o.path, preview_hash: h.clone() }),
                    None => {
                        warnings.push(format!("variant texture {} has no web preview", o.path));
                        None
                    }
                })
                .collect(),
        })
        .collect();
    if let Some(cfg) = manual {
        add_manual_skins(cfg, &materials, &index, preview_by_path, manifest, warnings);
    }

    manifest.materials = materials
        .iter()
        .zip(&resolved)
        .map(|(m, rs)| MaterialInfo {
            name: m.name.clone(),
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.roughness,
            textures: rs
                .iter()
                .map(|r| MaterialTexture {
                    slot: r.slot,
                    path: r.path.clone(),
                    original: r.original.clone(),
                    resolved_by: r.resolved_by.to_string(),
                })
                .collect(),
        })
        .collect();

    // Decode every texture once.
    let decoded: Vec<Vec<(TextureSlot, Arc<Decoded>)>> = materials
        .iter()
        .zip(&resolved)
        .map(|(m, rs)| {
            rs.iter()
                .filter_map(|r| {
                    let d = decode_resolved(input, cache, &m.name, r)?;
                    Some((r.slot, d))
                })
                .collect()
        })
        .collect();

    // ---- GLB per level
    for level in &levels {
        let tex_size = (cfg.max_texture_size >> level.level).max(256).min(cfg.max_texture_size);
        let (glb_mats, glb_images) = build_glb_materials(&materials, &decoded, tex_size, cache);
        let bytes = glb::write_glb(&level.meshes, &glb_mats, &glb_images, scene.rig.as_ref());
        let size = bytes.len() as u64;
        let h = out.push(Artifact::new(ArtifactRole::LodGlb(level.level), "model/gltf-binary", bytes));
        manifest.lods.push(LodInfo {
            level: level.level,
            source: level.source,
            triangles: level.triangles(),
            vertices: level.vertices(),
            glb_hash: h,
            glb_size: size,
            ratio: level.ratio,
        });
    }

    // ---- thumbnail
    let thumb_textures: Vec<Option<image::RgbaImage>> = decoded
        .iter()
        .map(|list| {
            list.iter().find(|(s, _)| *s == TextureSlot::BaseColor).map(|(_, d)| {
                let img = &d.image;
                if img.width().max(img.height()) > 512 {
                    image::imageops::thumbnail(&**img, 512, 512)
                } else {
                    (**img).clone()
                }
            })
        })
        .collect();
    let thumb_mats: Vec<ThumbMaterial> = materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let tex = thumb_textures.get(i).and_then(|t| t.as_ref());
            let mut base = m.base_color;
            if tex.is_some() {
                base = [1.0, 1.0, 1.0, base[3]];
            }
            ThumbMaterial {
                base_color: base,
                texture: tex,
                alpha_test: decoded[i].iter().any(|(s, d)| *s == TextureSlot::BaseColor && d.has_alpha),
            }
        })
        .collect();
    let thumb = raster::render_thumbnail(&levels[0].meshes, &thumb_mats, cfg.thumbnail_size);
    manifest.thumbnail_hash = Some(out.push(Artifact::new(ArtifactRole::Thumbnail, "image/png", images::encode_png(&thumb))));

    // ---- stats
    let lod0: &[MeshPrim] = &levels[0].meshes;
    let bounds = Aabb::of_meshes(lod0);
    let used_materials: HashSet<usize> = lod0.iter().filter_map(|m| m.material).collect();
    manifest.stats = Some(ModelStats {
        meshes: lod0.len() as u32,
        materials: used_materials.len() as u32,
        triangles: levels[0].triangles(),
        vertices: levels[0].vertices(),
        bounds_min: if bounds.is_valid() { bounds.min } else { [0.0; 3] },
        bounds_max: if bounds.is_valid() { bounds.max } else { [0.0; 3] },
        has_uvs: lod0.iter().any(|m| !m.uvs.is_empty()),
        has_normals: scene.source_had_normals,
        has_skin: scene.has_skin,
        animations: scene.animations,
    });
    manifest.kind = Some(PreviewKind::Model);
    manifest.primary_file = Some(sel.primary.clone());
    manifest.importer = Some(scene.importer.to_string());
    manifest.animations = scene
        .rig
        .iter()
        .flat_map(|r| &r.clips)
        .map(|c| AnimationInfo {
            name: c.name.clone(),
            duration_secs: c.duration,
            fps: c.fps,
            frames: (c.duration * c.fps).round() as u32 + 1,
            source: c.source.clone(),
        })
        .collect();
    Ok(())
}

/// Appends the skins of `.hangar/materials.json`; a manual skin replaces an
/// automatically detected variant of the same name.
fn add_manual_skins(
    cfg: &MaterialConfig,
    materials: &[Material],
    index: &ImageIndex,
    preview_by_path: &HashMap<String, String>,
    manifest: &mut PreviewManifest,
    warnings: &mut Vec<String>,
) {
    for skin in &cfg.skins {
        let name = skin.name.trim();
        if name.is_empty() || manifest.variants.iter().any(|v| v.is_default && v.name.eq_ignore_ascii_case(name)) {
            warnings.push(format!("{MATERIAL_CONFIG_PATH}: skin name '{name}' is empty or taken by the default look"));
            continue;
        }
        let mut overrides = Vec::new();
        for (mat_name, slots) in &skin.materials {
            let Some(mat) = materials.iter().find(|m| m.name.eq_ignore_ascii_case(mat_name)) else {
                warnings.push(format!("skin '{name}': material '{mat_name}' does not exist in the model"));
                continue;
            };
            for (&slot, path) in slots {
                let found = index.find(path).and_then(|p| preview_by_path.get(&p).map(|h| (p, h.clone())));
                match found {
                    Some((path, preview_hash)) => {
                        overrides.push(TextureOverride { material: mat.name.clone(), slot, path, preview_hash })
                    }
                    None => warnings.push(format!("skin '{name}': {slot} texture '{path}' not found or not previewable")),
                }
            }
        }
        manifest.variants.retain(|v| v.is_default || !v.name.eq_ignore_ascii_case(name));
        manifest.variants.push(VariantInfo {
            name: name.to_string(),
            is_default: false,
            overrides,
            detected_by: "manual".to_string(),
        });
    }
}

fn decode_resolved(input: &ProcessInput, cache: &mut ImageCache, mat: &str, r: &ResolvedTexture) -> Option<Arc<Decoded>> {
    if let Some(p) = &r.path {
        if images::is_decodable_ext(&paths::extension(p)) {
            return cache.get_file(input.root, p);
        }
        return None;
    }
    let bytes = r.embedded.as_ref()?;
    cache.get_bytes(&format!("embedded:{mat}:{}", r.slot), bytes)
}

/// Build glTF materials and the (encoded, downscaled) images they reference.
fn build_glb_materials(
    materials: &[Material],
    decoded: &[Vec<(TextureSlot, Arc<Decoded>)>],
    tex_size: u32,
    cache: &mut ImageCache,
) -> (Vec<GlbMaterial>, Vec<EncodedImage>) {
    let mut imgs: Vec<EncodedImage> = Vec::new();
    // pointer identity of the decoded image -> image index in this GLB
    let mut indices: HashMap<usize, usize> = HashMap::new();
    // Plain decoded texture: encoded once per size across previews and LODs.
    let mut add_decoded = |dst: &mut Vec<EncodedImage>, name: String, d: &Arc<Decoded>| -> usize {
        let k = Arc::as_ptr(d) as usize;
        if let Some(&i) = indices.get(&k) {
            return i;
        }
        let enc = cache.encode(d, tex_size);
        dst.push(EncodedImage { name, bytes: enc.0.clone(), mime: enc.1 });
        indices.insert(k, dst.len() - 1);
        dst.len() - 1
    };
    // Generated (packed / merged) texture.
    let add_generated = |dst: &mut Vec<EncodedImage>, name: String, img: &image::RgbaImage, alpha: bool| -> usize {
        let (bytes, mime) = images::encode_web(img, alpha, tex_size);
        dst.push(EncodedImage { name, bytes, mime });
        dst.len() - 1
    };

    let mut out = Vec::with_capacity(materials.len());
    for (mi, m) in materials.iter().enumerate() {
        let mut g = GlbMaterial::new(&m.name);
        g.base_color = m.base_color;
        g.metallic = m.metallic;
        g.roughness = m.roughness;
        g.emissive = m.emissive;
        g.double_sided = m.double_sided;
        let list = &decoded[mi];
        let get = |slot: TextureSlot| list.iter().find(|(s, _)| *s == slot).map(|(_, d)| d.clone());

        let base = get(TextureSlot::BaseColor);
        let opacity = get(TextureSlot::Opacity);
        if let Some(b) = &base {
            g.base_color = [1.0, 1.0, 1.0, m.base_color[3]];
            if let Some(o) = &opacity {
                let merged = images::merge_alpha(&b.image, &o.image);
                g.base_color_tex = Some(add_generated(&mut imgs, format!("{}_basecolor", m.name), &merged, true));
                g.alpha_mode = "MASK";
            } else {
                g.base_color_tex = Some(add_decoded(&mut imgs, format!("{}_basecolor", m.name), b));
                if b.has_alpha {
                    g.alpha_mode = "MASK";
                }
            }
        }
        if g.alpha_mode == "OPAQUE" && m.base_color[3] < 0.99 {
            g.alpha_mode = "BLEND";
        }
        if let Some(n) = get(TextureSlot::Normal) {
            g.normal_tex = Some(add_decoded(&mut imgs, format!("{}_normal", m.name), &n));
        }
        if let Some(orm) = get(TextureSlot::Orm) {
            let i = add_decoded(&mut imgs, format!("{}_orm", m.name), &orm);
            g.metallic_roughness_tex = Some(i);
            g.occlusion_tex = Some(i);
            g.metallic = 1.0;
            g.roughness = 1.0;
        } else {
            if let Some(mr) = get(TextureSlot::MetallicRoughness) {
                g.metallic_roughness_tex = Some(add_decoded(&mut imgs, format!("{}_mr", m.name), &mr));
                g.metallic = 1.0;
                g.roughness = 1.0;
            } else {
                let r = get(TextureSlot::Roughness);
                let mt = get(TextureSlot::Metallic);
                if r.is_some() || mt.is_some() {
                    let packed = images::pack_metal_rough(
                        r.as_ref().map(|d| &*d.image),
                        mt.as_ref().map(|d| &*d.image),
                        m.roughness,
                        m.metallic,
                    );
                    g.metallic_roughness_tex = Some(add_generated(&mut imgs, format!("{}_mr", m.name), &packed, false));
                    g.metallic = 1.0;
                    g.roughness = 1.0;
                }
            }
            if let Some(ao) = get(TextureSlot::Occlusion) {
                g.occlusion_tex = Some(add_decoded(&mut imgs, format!("{}_ao", m.name), &ao));
            }
        }
        if let Some(e) = get(TextureSlot::Emissive) {
            g.emissive_tex = Some(add_decoded(&mut imgs, format!("{}_emissive", m.name), &e));
        }
        out.push(g);
    }
    (out, imgs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn selects_primary() {
        let sel = select_model(&s(&["src/Tank.blend", "Tank.fbx", "Tank.obj"])).unwrap();
        assert_eq!(sel.primary, "Tank.fbx");
        assert!(sel.lod_files.is_empty());
        assert_eq!(sel.ignored.len(), 2);

        let sel = select_model(&s(&["Tank_LOD1.fbx", "Tank_LOD0.fbx", "Tank_LOD2.fbx", "Tank.blend"])).unwrap();
        assert_eq!(sel.primary, "Tank_LOD0.fbx");
        assert_eq!(sel.lod_files.iter().map(|(l, _)| *l).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(sel.ignored, vec!["Tank.blend".to_string()]);
        assert!(select_model(&[]).is_none());

        // Separate animation files (`Model@Clip`) lose to the model itself.
        let sel = select_model(&s(&["Shark@Attack.fbx", "Shark.fbx", "Shark@Swim.fbx"])).unwrap();
        assert_eq!(sel.primary, "Shark.fbx");
    }
}
