//! Run the pipeline on a folder and dump the results.
//!
//! ```text
//! cargo run -p hangar-pipeline --example process_dir -- <asset-dir> <out-dir>
//! ```
//! Set `HANGAR_BLENDER=/path/to/blender` to enable the Blender fallback.

use hangar_core::FileEntry;
use hangar_pipeline::{process, ArtifactRole, PipelineConfig, ProcessInput};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: process_dir <dir> <out>"));
    let out = PathBuf::from(args.next().expect("usage: process_dir <dir> <out>"));
    std::fs::create_dir_all(&out)?;

    let mut files = Vec::new();
    for e in walkdir::WalkDir::new(&dir).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()) {
        let rel = e.path().strip_prefix(&dir)?.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(e.path())?;
        files.push(FileEntry { path: rel, hash: blake3::hash(&bytes).to_hex().to_string(), size: bytes.len() as i64 });
    }

    let mut cfg = PipelineConfig::default();
    if let Ok(b) = std::env::var("HANGAR_BLENDER") {
        cfg.blender_path = Some(b.into());
        cfg.blender_script = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/blender/export_glb.py"));
    }
    let t = std::time::Instant::now();
    let res = process(&ProcessInput { root: &dir, files: &files }, &cfg)?;
    eprintln!("processed {} files in {:.2?}", files.len(), t.elapsed());

    std::fs::write(out.join("manifest.json"), serde_json::to_vec_pretty(&res.manifest)?)?;
    for a in &res.artifacts {
        let ext = match a.mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "model/gltf-binary" => "glb",
            _ => "bin",
        };
        let name = match &a.role {
            ArtifactRole::LodGlb(l) => format!("lod{l}.{ext}"),
            ArtifactRole::Thumbnail => format!("thumbnail.{ext}"),
            ArtifactRole::TexturePreview(p) => format!("tex_{}.{ext}", p.replace(['/', '\\', ' '], "_")),
        };
        std::fs::write(out.join(name), &a.bytes)?;
    }
    let m = &res.manifest;
    println!("kind={:?} importer={:?} primary={:?}", m.kind, m.importer, m.primary_file);
    for l in &m.lods {
        println!("  LOD{} {:?}: {} tris, {} verts, ratio {:.3}, glb {} KiB", l.level, l.source, l.triangles, l.vertices, l.ratio, l.glb_size / 1024);
    }
    for v in &m.variants {
        println!("  variant {} default={} by={} overrides={}", v.name, v.is_default, v.detected_by, v.overrides.len());
    }
    for mat in &m.materials {
        let t: Vec<String> = mat.textures.iter().map(|t| format!("{}={}({})", t.slot, t.path.as_deref().unwrap_or("-"), t.resolved_by)).collect();
        println!("  material {}: {}", mat.name, t.join(", "));
    }
    if let Some(s) = &m.stats {
        println!("  stats: {} meshes, {} materials, dims {:?}, uvs={} normals={} skin={} anims={}", s.meshes, s.materials, s.dimensions(), s.has_uvs, s.has_normals, s.has_skin, s.animations);
    }
    for w in &m.warnings {
        println!("  warning: {w}");
    }
    Ok(())
}
