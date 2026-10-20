//! The preview manifest is what the processing pipeline produces for every
//! version: web-ready GLBs per level of detail, texture previews, detected
//! skin variants (desert / woodland / ...) and statistics.
//!
//! All `*_hash` fields reference blobs in the content-addressed store and are
//! served from `/api/blobs/{hash}`.

use crate::enums::{LodSource, PreviewKind, TextureSlot};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Bumped whenever the pipeline output changes in an incompatible way, so the
/// server can re-process old versions on demand.
pub const PIPELINE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewManifest {
    pub pipeline_version: u32,
    pub kind: Option<PreviewKind>,
    /// Version-relative path of the file the preview was built from.
    pub primary_file: Option<String>,
    /// Which importer produced the scene ("ufbx", "gltf", "obj", "blender").
    pub importer: Option<String>,
    /// Levels of detail, LOD0 first.
    #[serde(default)]
    pub lods: Vec<LodInfo>,
    /// Texture sets ("skins"). The first one marked `is_default` is baked into the GLBs.
    #[serde(default)]
    pub variants: Vec<VariantInfo>,
    #[serde(default)]
    pub materials: Vec<MaterialInfo>,
    #[serde(default)]
    pub textures: Vec<TextureInfo>,
    /// Animation clips baked into every LOD GLB, in GLB order. For an
    /// animation-only version (`kind: animation`): every clip of its files.
    #[serde(default)]
    pub animations: Vec<AnimationInfo>,
    /// Animation-only versions: one entry per reference skeleton of
    /// `.hangar/animation.json`, in that order.
    #[serde(default)]
    pub skeletons: Vec<SkeletonPreview>,
    /// Texture sets of `.hangar/materials.json` the model drew textures from, in
    /// that order. Their images are listed in `textures` as `<prefix>/<path>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texture_sets: Vec<TextureSetUse>,
    pub stats: Option<ModelStats>,
    pub audio: Option<AudioInfo>,
    /// PNG thumbnail (software rendered for models, downscaled for images).
    pub thumbnail_hash: Option<String>,
    /// Why the version's model could not be previewed when the pipeline fell
    /// back to its images / audio (shown on the stage, not just in warnings).
    #[serde(default)]
    pub model_error: Option<String>,
    /// Non fatal issues found while processing (missing textures, fallbacks, ...).
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LodInfo {
    pub level: u32,
    pub source: LodSource,
    pub triangles: u64,
    pub vertices: u64,
    /// Self-contained GLB (embedded, downscaled textures of the default variant).
    pub glb_hash: String,
    pub glb_size: u64,
    /// Fraction of LOD0 triangles kept (1.0 for LOD0).
    pub ratio: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantInfo {
    pub name: String,
    pub is_default: bool,
    /// Texture assignments that differ from what is baked into the GLB.
    /// Empty for the default variant.
    pub overrides: Vec<TextureOverride>,
    /// How the variant was found: "folder", "token", "manual", "default".
    pub detected_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureOverride {
    pub material: String,
    pub slot: TextureSlot,
    /// Version-relative path of the source texture.
    pub path: String,
    /// Web preview (PNG/JPEG) of that texture.
    pub preview_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialInfo {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub textures: Vec<MaterialTexture>,
    /// glTF alpha mode baked into the GLBs: "OPAQUE", "MASK" or "BLEND"
    /// (absent in manifests written before it was recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_mode: Option<String>,
    /// Cutoff of a "MASK" material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_cutoff: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialTexture {
    pub slot: TextureSlot,
    /// Version-relative path of the texture after resolution, if found.
    pub path: Option<String>,
    /// The path as written inside the source file (often an absolute path on
    /// the original artist's machine).
    pub original: Option<String>,
    /// How the texture was matched: "exact", "filename", "material-name", "missing".
    pub resolved_by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub size: u64,
    /// Guessed slot from the file name (`_D`, `_N`, `_ORM`...).
    pub slot: Option<TextureSlot>,
    pub preview_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationInfo {
    /// Take / clip name from the source file.
    pub name: String,
    pub duration_secs: f32,
    /// Sampling rate of the baked keys (the source frame rate).
    pub fps: f32,
    /// Number of frames, the last one included.
    pub frames: u32,
    /// Version path of the separate animation file it came from (None: the model file).
    #[serde(default)]
    pub source: Option<String>,
}

/// The clips of an animation-only version retargeted (by bone name) onto a
/// reference skeleton's mesh.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkeletonPreview {
    /// Slug of the reference skeleton.
    pub skeleton: String,
    /// Its display name when the version was processed.
    pub name: String,
    /// Version of the skeleton asset the preview was built from.
    pub version_id: Option<Uuid>,
    pub version_number: Option<i32>,
    /// The skeleton's mesh plus the retargeted clips; None when nothing could be built.
    pub glb_hash: Option<String>,
    #[serde(default)]
    pub glb_size: u64,
    /// Clips that move at least one bone of the skeleton, in GLB order.
    #[serde(default)]
    pub animations: Vec<AnimationInfo>,
    /// Nodes of the animation files that have a same-named bone in the skeleton.
    #[serde(default)]
    pub bones_matched: u32,
    #[serde(default)]
    pub bones_total: u32,
    pub stats: Option<ModelStats>,
    /// Why there is no preview (skeleton unknown, no bone matches, ...).
    pub error: Option<String>,
}

/// A texture set (an asset whose images several models share) linked by a
/// model version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextureSetUse {
    pub asset_id: Uuid,
    /// The set asset's name when the version was processed.
    pub name: String,
    /// `@<set slug>`: the set's images appear in the manifest as `<prefix>/<path>`.
    pub prefix: String,
    /// Version of the set the images came from (its head when processed).
    pub version_id: Option<Uuid>,
    pub version_number: Option<i32>,
    /// Why the set could not be used (unknown asset, no access, no version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where a texture path of a manifest is stored: (version, path in it). Images
/// of a texture set (`@slug/…`) live in the set's version.
pub fn texture_location(sets: &[TextureSetUse], own_version: Uuid, path: &str) -> (Uuid, String) {
    for s in sets {
        if let Some(v) = s.version_id
            && let Some(rest) = path.strip_prefix(s.prefix.as_str()).and_then(|r| r.strip_prefix('/'))
        {
            return (v, rest.to_string());
        }
    }
    (own_version, path.to_string())
}

impl PreviewManifest {
    /// [`texture_location`] with this manifest's texture sets.
    pub fn file_location(&self, own_version: Uuid, path: &str) -> (Uuid, String) {
        texture_location(&self.texture_sets, own_version, path)
    }

    /// The texture-set images (`@slug/…` paths) that the materials and variants
    /// use, sorted: the files the version needs besides its own.
    pub fn texture_set_files(&self) -> Vec<String> {
        let in_set = |p: &str| {
            self.texture_sets.iter().any(|s| {
                s.version_id.is_some() && p.strip_prefix(s.prefix.as_str()).is_some_and(|r| r.starts_with('/'))
            })
        };
        let used = self.materials.iter().flat_map(|m| m.textures.iter().filter_map(|t| t.path.as_deref()));
        let overrides = self.variants.iter().flat_map(|v| v.overrides.iter().map(|o| o.path.as_str()));
        let mut out: Vec<String> = used.chain(overrides).filter(|p| in_set(p)).map(str::to_string).collect();
        out.sort();
        out.dedup();
        out
    }

    /// An animation version seen on one of its skeletons, shaped like a model
    /// manifest (a single LOD, the skeleton's clips) so the 3D viewer can show it.
    pub fn on_skeleton(&self, slug: &str) -> Option<PreviewManifest> {
        let s = self.skeletons.iter().find(|s| s.skeleton == slug)?;
        let glb_hash = s.glb_hash.clone()?;
        let stats = s.stats.clone();
        Some(PreviewManifest {
            kind: Some(PreviewKind::Model),
            lods: vec![LodInfo {
                level: 0,
                source: LodSource::Authored,
                triangles: stats.as_ref().map_or(0, |s| s.triangles),
                vertices: stats.as_ref().map_or(0, |s| s.vertices),
                glb_hash,
                glb_size: s.glb_size,
                ratio: 1.0,
            }],
            animations: s.animations.clone(),
            stats,
            skeletons: vec![],
            variants: vec![],
            materials: vec![],
            ..self.clone()
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelStats {
    pub meshes: u32,
    pub materials: u32,
    pub triangles: u64,
    pub vertices: u64,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub has_uvs: bool,
    pub has_normals: bool,
    pub has_skin: bool,
    pub animations: u32,
}

impl ModelStats {
    pub fn dimensions(&self) -> [f32; 3] {
        [
            self.bounds_max[0] - self.bounds_min[0],
            self.bounds_max[1] - self.bounds_min[1],
            self.bounds_max[2] - self.bounds_min[2],
        ]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioInfo {
    pub path: String,
    pub format: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub duration_secs: Option<f32>,
    /// Normalised peak envelope (0..1), ~200 buckets — enough to draw a waveform.
    #[serde(default)]
    pub peaks: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_set_file_location() {
        let (own, set_version) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let m = PreviewManifest {
            texture_sets: vec![TextureSetUse { prefix: "@city".into(), version_id: Some(set_version), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(m.file_location(own, "@city/walls/bh2b.jpg"), (set_version, "walls/bh2b.jpg".to_string()));
        assert_eq!(m.file_location(own, "@cityscape/a.png"), (own, "@cityscape/a.png".to_string()));
        assert_eq!(m.file_location(own, "Textures/a.png"), (own, "Textures/a.png".to_string()));
    }

    #[test]
    fn texture_set_files_are_the_used_ones() {
        let tex = |path: &str| MaterialTexture {
            slot: TextureSlot::BaseColor,
            path: Some(path.into()),
            original: None,
            resolved_by: "texture-set".into(),
        };
        let m = PreviewManifest {
            texture_sets: vec![
                TextureSetUse { prefix: "@city".into(), version_id: Some(Uuid::from_u128(2)), ..Default::default() },
                TextureSetUse { prefix: "@broken".into(), error: Some("x".into()), ..Default::default() },
            ],
            materials: vec![MaterialInfo {
                name: "wall".into(),
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                textures: vec![tex("@city/b.jpg"), tex("own/a.png"), tex("@broken/c.jpg"), tex("@city/a.jpg")],
                alpha_mode: None,
                alpha_cutoff: None,
            }],
            variants: vec![VariantInfo {
                name: "night".into(),
                is_default: false,
                overrides: vec![TextureOverride {
                    material: "wall".into(),
                    slot: TextureSlot::BaseColor,
                    path: "@city/b.jpg".into(),
                    preview_hash: String::new(),
                }],
                detected_by: "manual".into(),
            }],
            ..Default::default()
        };
        assert_eq!(m.texture_set_files(), ["@city/a.jpg", "@city/b.jpg"]);
    }
}
