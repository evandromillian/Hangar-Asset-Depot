use serde::Deserialize;
use std::path::PathBuf;
use summer::config::Configurable;

/// `[hangar]` section of `config/app.toml`.
#[derive(Debug, Clone, Configurable, Deserialize)]
#[config_prefix = "hangar"]
pub struct HangarConfig {
    /// Let anyone register (otherwise only the first user, then admins create accounts).
    #[serde(default)]
    pub allow_signup: bool,
    /// Mark the session cookie `Secure` (enable behind HTTPS).
    #[serde(default)]
    pub cookie_secure: bool,
    /// Session lifetime in seconds (must match `[sa-token] timeout`).
    #[serde(default = "default_session_ttl")]
    pub session_ttl: i64,
    /// Number of concurrent processing workers.
    #[serde(default = "default_workers")]
    pub workers: usize,
    /// Scratch directory for uploads and processing (defaults to the OS temp dir).
    #[serde(default)]
    pub temp_dir: Option<PathBuf>,
    /// Blender executable for the headless fallback. "auto" searches PATH, "" disables.
    #[serde(default = "default_blender")]
    pub blender_path: String,
    #[serde(default = "default_blender_script")]
    pub blender_script: PathBuf,
    #[serde(default = "default_texture_size")]
    pub max_texture_size: u32,
    #[serde(default = "default_lod_ratios")]
    pub lod_ratios: Vec<f32>,
    #[serde(default = "default_thumbnail_size")]
    pub thumbnail_size: u32,
}

fn default_session_ttl() -> i64 {
    7 * 24 * 3600
}
fn default_workers() -> usize {
    1
}
fn default_blender() -> String {
    "auto".into()
}
fn default_blender_script() -> PathBuf {
    "tools/blender/export_glb.py".into()
}
fn default_texture_size() -> u32 {
    2048
}
fn default_lod_ratios() -> Vec<f32> {
    vec![0.5, 0.25, 0.1]
}
fn default_thumbnail_size() -> u32 {
    512
}

impl HangarConfig {
    pub fn temp_root(&self) -> PathBuf {
        self.temp_dir.clone().unwrap_or_else(std::env::temp_dir)
    }

    /// Resolved Blender executable, if the fallback is enabled and found.
    pub fn blender(&self) -> Option<PathBuf> {
        match self.blender_path.trim() {
            "" | "off" | "none" => None,
            "auto" => which::which("blender").ok(),
            p => {
                let p = PathBuf::from(p);
                p.exists().then_some(p)
            }
        }
    }

    pub fn pipeline(&self) -> hangar_pipeline::PipelineConfig {
        let blender = self.blender();
        hangar_pipeline::PipelineConfig {
            max_texture_size: self.max_texture_size,
            lod_ratios: self.lod_ratios.clone(),
            thumbnail_size: self.thumbnail_size,
            blender_script: blender.as_ref().map(|_| self.blender_script.clone()),
            blender_path: blender,
            ..Default::default()
        }
    }
}
