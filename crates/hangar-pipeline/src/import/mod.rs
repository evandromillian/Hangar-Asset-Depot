//! Importers: model file → [`Scene`](crate::scene::Scene).

pub mod blender;
pub mod fbx;
pub mod gltf_import;
pub mod obj;

use crate::scene::{ensure_normals, Scene};
use crate::PipelineConfig;
use anyhow::{anyhow, Result};
use hangar_core::paths;
use std::path::{Path, PathBuf};

/// The version root of a file at `path` whose version directory is `base_dir`
/// (`root/Models/x.gltf` with "Models" → `root`). References inside model files
/// are resolved against it with [`paths::join_within`], never joined raw.
fn version_root<'a>(path: &'a Path, base_dir: &str) -> &'a Path {
    let parent = path.parent().unwrap_or(Path::new("."));
    base_dir.split('/').filter(|s| !s.is_empty()).fold(parent, |p, _| p.parent().unwrap_or(p))
}

/// Import a version file (`rel` relative to `root`). `anim_files` are
/// (version path, file on disk) of FBX files holding animations for this model
/// (FBX primaries only); they may live outside `root` (reference skeletons).
pub fn import(root: &Path, rel: &str, cfg: &PipelineConfig, anim_files: &[(String, PathBuf)]) -> Result<Scene> {
    let path = root.join(rel);
    let base_dir = paths::parent(rel).to_string();
    let ext = paths::extension(rel);
    let native = match ext.as_str() {
        "fbx" => Some(fbx::load(&path, &base_dir, anim_files)),
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
    if ext != "fbx" && !anim_files.is_empty() {
        scene.warnings.push(format!(
            "separate animation files need an FBX model; not applied: {}",
            anim_files.iter().map(|(rel, _)| rel.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    for m in &mut scene.meshes {
        ensure_normals(m);
    }
    scene.meshes.retain(|m| !m.indices.is_empty() && !m.positions.is_empty());
    Ok(scene)
}
