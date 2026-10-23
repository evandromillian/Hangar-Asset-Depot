//! Headless Blender fallback: Blender opens/imports the file and exports a GLB
//! which we then read with the glTF importer.

use crate::scene::Scene;
use crate::PipelineConfig;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn available(cfg: &PipelineConfig) -> bool {
    cfg.blender_path.as_ref().is_some_and(|p| p.exists() || p.components().count() == 1)
        && cfg.blender_script.as_ref().is_some_and(|p| p.exists())
}

pub fn load(path: &Path, base_dir: &str, cfg: &PipelineConfig) -> Result<Scene> {
    let blender = cfg.blender_path.as_ref().ok_or_else(|| anyhow!("Blender fallback not configured"))?;
    let script = cfg.blender_script.as_ref().ok_or_else(|| anyhow!("Blender script not configured"))?;
    let tmp = std::env::temp_dir().join(format!("hangar-blender-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let out_glb = tmp.join(format!(
        "{}.glb",
        blake3::hash(path.to_string_lossy().as_bytes()).to_hex().chars().take(16).collect::<String>()
    ));
    let _ = std::fs::remove_file(&out_glb);

    let mut cmd = Command::new(blender);
    // Uploaded .blend files are untrusted: never run their embedded scripts / drivers.
    cmd.arg("--background").arg("--factory-startup").arg("--disable-autoexec").arg("--python-exit-code").arg("3");
    cmd.arg("--python").arg(script).arg("--").arg(path).arg(&out_glb);
    let log_path = out_glb.with_extension("log");
    let log = std::fs::File::create(&log_path)?;
    cmd.stdout(Stdio::from(log.try_clone()?)).stderr(Stdio::from(log));
    let mut child = cmd.spawn().with_context(|| format!("spawn {}", blender.display()))?;
    let deadline = Instant::now() + Duration::from_secs(cfg.blender_timeout_secs.max(5));
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Blender timed out after {}s", cfg.blender_timeout_secs);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() || !out_glb.exists() {
        let err = std::fs::read_to_string(&log_path).unwrap_or_default();
        let _ = std::fs::remove_file(&log_path);
        let tail: String = err.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        bail!("Blender export failed ({status}): {tail}");
    }
    let res = super::gltf_import::load(&out_glb, base_dir, "blender");
    let _ = std::fs::remove_file(&out_glb);
    let _ = std::fs::remove_file(&log_path);
    res
}
