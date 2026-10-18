//! Hangar processing pipeline.
//!
//! Turns the file tree of a version (already materialised on disk) into
//! web-ready artifacts plus a [`PreviewManifest`]:
//!
//! * import FBX (ufbx), glTF/GLB (gltf), OBJ (tobj) — or anything Blender can open
//!   through the headless fallback;
//! * resolve broken texture references and detect skin variants;
//! * build LODs (authored by name, or generated with meshoptimizer);
//! * write self-contained GLBs, texture previews and a software-rendered thumbnail.
//!
//! The pipeline is synchronous and CPU bound: callers should run it on a
//! blocking thread.

pub mod audio;
pub mod glb;
pub mod images;
pub mod import;
pub mod lod;
pub mod pose;
mod process;
pub mod raster;
pub mod scene;
pub mod textures;
pub mod variants;

use hangar_core::{FileEntry, PreviewManifest};
use std::path::{Path, PathBuf};

/// Tunables of the pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Max width/height of textures embedded in LOD0 GLBs and texture previews.
    pub max_texture_size: u32,
    /// Triangle ratios for generated LODs (LOD1, LOD2, ...).
    pub lod_ratios: Vec<f32>,
    /// Do not generate LODs below this triangle count.
    pub min_triangles_for_lod: u64,
    /// Thumbnail edge in pixels.
    pub thumbnail_size: u32,
    /// Path to a Blender executable for the headless fallback (None = disabled).
    pub blender_path: Option<PathBuf>,
    /// Script used by the Blender fallback (tools/blender/export_glb.py).
    pub blender_script: Option<PathBuf>,
    /// Abort the Blender fallback after this many seconds.
    pub blender_timeout_secs: u64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_texture_size: 2048,
            lod_ratios: vec![0.5, 0.25, 0.1],
            min_triangles_for_lod: 500,
            thumbnail_size: 512,
            blender_path: None,
            blender_script: None,
            blender_timeout_secs: 300,
        }
    }
}

/// Input of [`process`]: every entry of `files` exists at `root.join(entry.path)`.
/// Only files for which [`is_relevant`] is true need to be present.
pub struct ProcessInput<'a> {
    pub root: &'a Path,
    pub files: &'a [FileEntry],
    /// The reference skeletons named by the version's `.hangar/animation.json`
    /// (resolved by the caller), in that order. Used only when the version has
    /// animation files and no mesh of its own.
    pub skeletons: &'a [SkeletonSource],
}

/// The head version of a reference skeleton's asset, materialised on disk like
/// a [`ProcessInput`] (relevant files only).
pub struct SkeletonSource {
    pub slug: String,
    pub name: String,
    pub root: PathBuf,
    pub files: Vec<FileEntry>,
}

/// What a generated artifact is used for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactRole {
    /// GLB for a level of detail.
    LodGlb(u32),
    /// PNG thumbnail of the version.
    Thumbnail,
    /// Web preview (PNG/JPEG) of the texture at this version-relative path.
    TexturePreview(String),
    /// GLB of a reference skeleton (slug) playing the version's animations.
    SkeletonGlb(String),
}

/// A blob produced by the pipeline; `hash` is the BLAKE3 hex of `bytes` and is
/// what the manifest references.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub hash: String,
    pub role: ArtifactRole,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

impl Artifact {
    pub fn new(role: ArtifactRole, mime: &'static str, bytes: Vec<u8>) -> Self {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Self { hash, role, mime, bytes }
    }
}

pub struct ProcessOutput {
    pub manifest: PreviewManifest,
    pub artifacts: Vec<Artifact>,
}

/// Whether a file is needed by the pipeline (models, images, audio, sidecar
/// files such as `.mtl` / `.bin`, and the `.hangar/` configuration files).
pub fn is_relevant(path: &str) -> bool {
    let ext = hangar_core::paths::extension(path);
    path == hangar_core::MATERIAL_CONFIG_PATH
        || path == hangar_core::ANIMATION_CONFIG_PATH
        || hangar_core::paths::is_model(path)
        || hangar_core::paths::is_image(path)
        || hangar_core::paths::is_audio(path)
        || matches!(ext.as_str(), "mtl" | "bin")
}

/// Run the whole pipeline.
///
/// Fails only when the version contains a model that cannot be imported and
/// nothing else to preview; recoverable problems end up in
/// `manifest.warnings`.
pub fn process(input: &ProcessInput, cfg: &PipelineConfig) -> anyhow::Result<ProcessOutput> {
    process::run(input, cfg)
}

/// Whether the Blender fallback is usable with this configuration.
pub fn blender_available(cfg: &PipelineConfig) -> bool {
    import::blender::available(cfg)
}
