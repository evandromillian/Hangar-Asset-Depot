//! Animation binding: which reference skeletons an animation-only version is
//! previewed on. Stored as a JSON file inside the version tree (like
//! [`MaterialConfig`](crate::MaterialConfig)), so changing it is a new version.
//!
//! Reference skeletons themselves are a system setting (`/api/skeletons`): a
//! name, an immutable slug and the model asset whose mesh plays the clips.
//! Versions that bring their own mesh ignore the binding — their animations
//! play on that mesh only.

use serde::{Deserialize, Serialize};

/// Version-relative path of the binding file.
pub const ANIMATION_CONFIG_PATH: &str = ".hangar/animation.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimationConfig {
    /// Slugs of the reference skeletons, in display order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skeletons: Vec<String>,
}

impl AnimationConfig {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let mut cfg: Self = serde_json::from_slice(bytes)?;
        let mut seen = std::collections::HashSet::new();
        cfg.skeletons.retain(|s| !s.trim().is_empty() && seen.insert(s.trim().to_string()));
        Ok(cfg)
    }

    pub fn to_json(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).expect("AnimationConfig serializes");
        out.push(b'\n');
        out
    }

    pub fn is_empty(&self) -> bool {
        self.skeletons.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let cfg = AnimationConfig::from_json(br#"{ "skeletons": ["ue5-manny", "", "ue4-mannequin", "ue5-manny"] }"#).unwrap();
        assert_eq!(cfg.skeletons, ["ue5-manny", "ue4-mannequin"]);
        assert_eq!(AnimationConfig::from_json(&cfg.to_json()).unwrap(), cfg);
        assert!(AnimationConfig::from_json(b"{}").unwrap().is_empty());
    }
}
