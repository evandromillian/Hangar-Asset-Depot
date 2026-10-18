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
    pub stats: Option<ModelStats>,
    pub audio: Option<AudioInfo>,
    /// PNG thumbnail (software rendered for models, downscaled for images).
    pub thumbnail_hash: Option<String>,
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

impl PreviewManifest {
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
