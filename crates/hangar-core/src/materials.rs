//! Manual material configuration: texture assignments and named skins that a
//! user sets explicitly, stored as a JSON file inside the version tree so it is
//! versioned like any other file. The pipeline applies it on top of automatic
//! texture resolution and variant detection.

use crate::enums::TextureSlot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Version-relative path of the configuration file.
pub const MATERIAL_CONFIG_PATH: &str = ".hangar/materials.json";

/// Slots that can be assigned by hand, in display order. Height is left out:
/// the GLB export doesn't use it.
pub const EDITABLE_SLOTS: &[TextureSlot] = &[
    TextureSlot::BaseColor,
    TextureSlot::Normal,
    TextureSlot::Orm,
    TextureSlot::MetallicRoughness,
    TextureSlot::Roughness,
    TextureSlot::Metallic,
    TextureSlot::Occlusion,
    TextureSlot::Specular,
    TextureSlot::Emissive,
    TextureSlot::Opacity,
];

/// Base assignments of one material. `Some(path)` forces that texture,
/// `None` (JSON `null`) forces the slot to stay empty; slots that are absent
/// keep the automatic resolution.
pub type SlotAssignments = BTreeMap<TextureSlot, Option<String>>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialConfig {
    /// Material name → slot assignments of the default look.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub materials: BTreeMap<String, SlotAssignments>,
    /// Named texture sets layered on top of the default look.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skins: Vec<SkinConfig>,
    /// Material name → alpha cutoff (0–1): the material becomes a cutout of its
    /// base color alpha (or opacity map) at that threshold. A model file can't
    /// carry the threshold of a shader (e.g. Blender's `alpha > 0.03`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub alpha_cutoff: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinConfig {
    pub name: String,
    /// Material name → slot → texture path.
    #[serde(default)]
    pub materials: BTreeMap<String, BTreeMap<TextureSlot, String>>,
}

impl MaterialConfig {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn to_json(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).expect("MaterialConfig serializes");
        out.push(b'\n');
        out
    }

    pub fn is_empty(&self) -> bool {
        self.materials.values().all(|m| m.is_empty()) && self.skins.is_empty() && self.alpha_cutoff.is_empty()
    }

    /// Base assignments for `material`, matched case-insensitively like the viewer does.
    pub fn assignments(&self, material: &str) -> Option<&SlotAssignments> {
        self.materials.iter().find(|(k, _)| k.eq_ignore_ascii_case(material)).map(|(_, v)| v)
    }

    /// Manual alpha cutoff of `material` (case-insensitive), clamped to 0–1.
    pub fn cutoff(&self, material: &str) -> Option<f32> {
        self.alpha_cutoff
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(material))
            .map(|(_, v)| v.clamp(0.0, 1.0))
            .filter(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let json = r#"{
            "materials": { "a1": { "base_color": "T_1001_D.png", "normal": null } },
            "skins": [ { "name": "Deserto", "materials": { "a1": { "base_color": "T_Desert_D.png" } } } ],
            "alpha_cutoff": { "Net": 0.03, "over": 4 }
        }"#;
        let cfg = MaterialConfig::from_json(json.as_bytes()).unwrap();
        assert_eq!(cfg.cutoff("net"), Some(0.03));
        assert_eq!(cfg.cutoff("OVER"), Some(1.0));
        assert_eq!(cfg.cutoff("a1"), None);
        let a1 = cfg.assignments("A1").unwrap();
        assert_eq!(a1[&TextureSlot::BaseColor].as_deref(), Some("T_1001_D.png"));
        assert_eq!(a1[&TextureSlot::Normal], None);
        assert_eq!(cfg.skins[0].materials["a1"][&TextureSlot::BaseColor], "T_Desert_D.png");
        assert_eq!(MaterialConfig::from_json(&cfg.to_json()).unwrap(), cfg);
    }

    #[test]
    fn empty_config() {
        let cfg = MaterialConfig::from_json(b"{}").unwrap();
        assert!(cfg.is_empty());
        assert_eq!(String::from_utf8(cfg.to_json()).unwrap(), "{}\n");
    }
}
