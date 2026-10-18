//! Skin / texture-set variant detection (desert, woodland, winter...).
//!
//! Two image files that differ in exactly one token — that is neither a
//! texture-slot token (`_D` vs `_N`) nor a resolution token (`2k` vs `4k`) —
//! are considered variants of each other. The token of the texture the model
//! actually uses names the default variant.

use crate::textures::{is_resolution_token, slot_from_token, tokenize};
use hangar_core::{paths, TextureSlot};
use std::collections::{BTreeMap, HashSet};

/// A texture actually used by the model (default variant).
#[derive(Debug, Clone)]
pub struct UsedTexture {
    pub material: String,
    pub slot: TextureSlot,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedOverride {
    pub material: String,
    pub slot: TextureSlot,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetectedVariant {
    pub name: String,
    pub is_default: bool,
    pub overrides: Vec<DetectedOverride>,
    pub detected_by: &'static str,
}

struct Tokens {
    /// Original-case tokens.
    toks: Vec<String>,
    lower: Vec<String>,
    /// Number of leading tokens that come from directories.
    dir_count: usize,
}

fn tokens_of(path: &str) -> Tokens {
    let dir = paths::parent(path);
    let dir_toks = tokenize(dir);
    let stem_toks = tokenize(paths::stem(path));
    let dir_count = dir_toks.len();
    let toks: Vec<String> = dir_toks.into_iter().chain(stem_toks).collect();
    let lower = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
    Tokens { toks, lower, dir_count }
}

/// If `a` and `b` differ in exactly one token, return its index.
fn single_difference(a: &Tokens, b: &Tokens) -> Option<usize> {
    if a.lower.len() != b.lower.len() {
        return None;
    }
    let mut diff = None;
    for i in 0..a.lower.len() {
        if a.lower[i] != b.lower[i] {
            if diff.is_some() {
                return None;
            }
            diff = Some(i);
        }
    }
    diff
}

fn is_variant_token(t: &str) -> bool {
    slot_from_token(t).is_none() && !is_resolution_token(t)
}

/// Detect variants. `images` is every image path of the version; `used`
/// the textures referenced by the model after resolution.
pub fn detect(images: &[String], used: &[UsedTexture]) -> Vec<DetectedVariant> {
    let used_paths: HashSet<String> = used.iter().map(|u| u.path.to_ascii_lowercase()).collect();
    let image_tokens: Vec<(String, Tokens)> = images
        .iter()
        .filter(|p| !used_paths.contains(&p.to_ascii_lowercase()))
        .map(|p| (p.clone(), tokens_of(p)))
        .collect();

    // variant name (lower) -> (display name, overrides, folder?)
    let mut variants: BTreeMap<String, (String, Vec<DetectedOverride>, bool)> = BTreeMap::new();
    // default token votes
    let mut default_votes: BTreeMap<String, (String, usize)> = BTreeMap::new();

    for u in used {
        let ut = tokens_of(&u.path);
        for (path, it) in &image_tokens {
            let Some(i) = single_difference(&ut, it) else { continue };
            let (ua, ia) = (&ut.lower[i], &it.lower[i]);
            if !is_variant_token(ua) || !is_variant_token(ia) {
                continue;
            }
            // Different extensions are fine (desert .png, winter .tga).
            let entry = variants.entry(ia.clone()).or_insert_with(|| (it.toks[i].clone(), vec![], false));
            if !entry.1.iter().any(|o| o.material == u.material && o.slot == u.slot) {
                entry.1.push(DetectedOverride { material: u.material.clone(), slot: u.slot, path: path.clone() });
            }
            entry.2 |= i < ut.dir_count;
            let v = default_votes.entry(ua.clone()).or_insert_with(|| (ut.toks[i].clone(), 0));
            v.1 += 1;
        }
    }

    let default_name = default_votes
        .iter()
        .max_by_key(|(_, (_, n))| *n)
        .map(|(_, (name, _))| name.clone());

    let mut out = Vec::new();
    match default_name {
        Some(name) => {
            let lower = name.to_ascii_lowercase();
            let any_folder = variants.values().any(|v| v.2);
            out.push(DetectedVariant {
                name,
                is_default: true,
                overrides: vec![],
                detected_by: if any_folder { "folder" } else { "token" },
            });
            for (key, (display, mut overrides, folder)) in variants {
                if key == lower {
                    continue;
                }
                overrides.sort_by(|a, b| (&a.material, a.slot.as_str()).cmp(&(&b.material, b.slot.as_str())));
                out.push(DetectedVariant {
                    name: display,
                    is_default: false,
                    overrides,
                    detected_by: if folder { "folder" } else { "token" },
                });
            }
        }
        None => out.push(DetectedVariant {
            name: "Default".into(),
            is_default: true,
            overrides: vec![],
            detected_by: "default",
        }),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(m: &str, slot: TextureSlot, p: &str) -> UsedTexture {
        UsedTexture { material: m.into(), slot, path: p.into() }
    }

    #[test]
    fn token_variants() {
        let images: Vec<String> = [
            "T_Abrams_Desert_D.png",
            "T_Abrams_Woodland_D.png",
            "T_Abrams_Winter_D.tga",
            "T_Abrams_N.png",
            "T_Abrams_Desert_N.png",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let u = vec![
            used("M_Abrams", TextureSlot::BaseColor, "T_Abrams_Desert_D.png"),
            used("M_Abrams", TextureSlot::Normal, "T_Abrams_N.png"),
        ];
        let v = detect(&images, &u);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].name, "Desert");
        assert!(v[0].is_default);
        let names: Vec<&str> = v.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"Woodland") && names.contains(&"Winter"));
        let wood = v.iter().find(|x| x.name == "Woodland").unwrap();
        assert_eq!(wood.detected_by, "token");
        assert_eq!(wood.overrides, vec![DetectedOverride {
            material: "M_Abrams".into(),
            slot: TextureSlot::BaseColor,
            path: "T_Abrams_Woodland_D.png".into()
        }]);
    }

    #[test]
    fn folder_variants() {
        let images: Vec<String> = [
            "Textures/Desert/T_Body_D.png",
            "Textures/Forest/T_Body_D.png",
            "Textures/Desert/T_Body_N.png",
            "Textures/Forest/T_Body_N.png",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let u = vec![
            used("Body", TextureSlot::BaseColor, "Textures/Desert/T_Body_D.png"),
            used("Body", TextureSlot::Normal, "Textures/Desert/T_Body_N.png"),
        ];
        let v = detect(&images, &u);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Desert");
        assert_eq!(v[1].name, "Forest");
        assert_eq!(v[1].detected_by, "folder");
        assert_eq!(v[1].overrides.len(), 2);
    }

    #[test]
    fn slot_difference_is_not_a_variant() {
        let images: Vec<String> =
            ["T_Body_D.png", "T_Body_N.png", "T_Body_ORM.png", "T_Body_D_2k.png"].iter().map(|s| s.to_string()).collect();
        let u = vec![used("Body", TextureSlot::BaseColor, "T_Body_D.png")];
        let v = detect(&images, &u);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Default");
        assert_eq!(v[0].detected_by, "default");
    }

    #[test]
    fn used_parts_are_not_variants() {
        // Wheel_01 and Wheel_02 are both used by the model: different parts, not skins.
        let images: Vec<String> = ["T_Wheel_01_D.png", "T_Wheel_02_D.png"].iter().map(|s| s.to_string()).collect();
        let u = vec![
            used("Wheel1", TextureSlot::BaseColor, "T_Wheel_01_D.png"),
            used("Wheel2", TextureSlot::BaseColor, "T_Wheel_02_D.png"),
        ];
        assert_eq!(detect(&images, &u)[0].name, "Default");
    }
}
