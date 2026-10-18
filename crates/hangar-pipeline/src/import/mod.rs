//! Importers: model file → [`Scene`](crate::scene::Scene).

pub mod blender;
pub mod fbx;
pub mod gltf_import;
pub mod obj;

use crate::scene::{ensure_normals, Scene};
use crate::PipelineConfig;
use anyhow::{anyhow, Result};
use hangar_core::paths;
use std::path::Path;

/// Import a version file (`rel` relative to `root`).
pub fn import(root: &Path, rel: &str, cfg: &PipelineConfig) -> Result<Scene> {
    let path = root.join(rel);
    let base_dir = paths::parent(rel).to_string();
    let ext = paths::extension(rel);
    let native = match ext.as_str() {
        "fbx" => Some(fbx::load(&path, &base_dir)),
        "glb" | "gltf" => Some(gltf_import::load(&path, &base_dir, "gltf")),
        "obj" => Some(obj::load(&path, &base_dir)),
        _ => None,
    };
    let mut scene = match native {
        Some(Ok(s)) => s,
        Some(Err(e)) => {
            if blender::available(cfg) {
                tracing::warn!("native import of {rel} failed ({e:#}); trying Blender");
                let mut s = blender::load(&path, &base_dir, cfg).map_err(|be| anyhow!("{e:#}; Blender fallback: {be:#}"))?;
                s.warnings.push(format!("native importer failed ({e}); converted with Blender"));
                s
            } else {
                return Err(e);
            }
        }
        None => {
            if blender::available(cfg) {
                blender::load(&path, &base_dir, cfg)?
            } else {
                return Err(anyhow!("no native importer for .{ext}; configure the Blender fallback to preview it"));
            }
        }
    };
    for m in &mut scene.meshes {
        ensure_normals(m);
    }
    scene.meshes.retain(|m| !m.indices.is_empty() && !m.positions.is_empty());
    Ok(scene)
}
