//! Texture naming heuristics: tokenisation, slot detection and resolution of
//! the (often broken) texture paths stored inside model files.

use crate::scene::{Material, TexSource};
use hangar_core::{paths, MaterialConfig, TextureSlot, MATERIAL_CONFIG_PATH};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Splits a string on `/ \ _ - . space` and camelCase boundaries are NOT split
/// (artists use `BaseColor`, which we handle as a single token).
pub fn tokenize(s: &str) -> Vec<String> {
    s.split(['/', '\\', '_', '-', '.', ' '])
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Map a single lower-case token to a texture slot.
pub fn slot_from_token(token: &str) -> Option<TextureSlot> {
    use TextureSlot::*;
    let t = token.to_ascii_lowercase();
    Some(match t.as_str() {
        "basecolor" | "basecolour" | "albedo" | "alb" | "diffuse" | "diff" | "dif" | "d" | "bc" | "col"
        | "color" | "colour" | "albedotransparency" | "basecolormap" | "diffusemap" => BaseColor,
        "normal" | "normals" | "nrm" | "nor" | "norm" | "n" | "nml" | "normalgl" | "normaldx" | "normalmap"
        | "nrml" => Normal,
        "orm" | "arm" | "occlusionroughnessmetallic" | "aorm" => Orm,
        "mr" | "metallicroughness" | "metalrough" | "metalroughness" | "roughnessmetallic" => MetallicRoughness,
        "r" | "rough" | "roughness" | "rgh" | "roughnessmap" => Roughness,
        "m" | "metal" | "metallic" | "metalness" | "met" | "metallicmap" => Metallic,
        "ao" | "occlusion" | "ambientocclusion" | "occ" | "ambient" => Occlusion,
        "e" | "emissive" | "emission" | "emit" | "glow" | "illum" | "selfillum" => Emissive,
        "h" | "height" | "disp" | "displacement" | "bump" | "heightmap" => Height,
        "opacity" | "alpha" | "transparency" | "transparent" | "o" => Opacity,
        // Glossiness maps are inverted roughness, not specular: no slot.
        "s" | "spec" | "specular" | "specularlevel" | "specularmap" => Specular,
        _ => return None,
    })
}

/// Tokens that carry no meaning for slot/variant detection.
pub fn is_noise_token(token: &str) -> bool {
    let t = token.to_ascii_lowercase();
    matches!(t.as_str(), "map" | "tex" | "texture" | "gl" | "dx" | "opengl" | "directx" | "srgb" | "linear")
        || is_resolution_token(&t)
}

/// `2k`, `4K`, `1024`, `2048x2048`, `512px`...
pub fn is_resolution_token(token: &str) -> bool {
    let t = token.to_ascii_lowercase();
    if let Some(n) = t.strip_suffix('k') {
        return !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && n.len() <= 2;
    }
    if let Some(n) = t.strip_suffix("px") {
        return !n.is_empty() && n.chars().all(|c| c.is_ascii_digit());
    }
    if let Some((a, b)) = t.split_once('x') {
        return is_pow2_size(a) && is_pow2_size(b);
    }
    is_pow2_size(&t)
}

fn is_pow2_size(s: &str) -> bool {
    matches!(s, "128" | "256" | "512" | "1024" | "2048" | "4096" | "8192" | "16384")
}

/// Guess the slot of an image from its file name: the last meaningful token
/// (`T_Abrams_Desert_D.png` → BaseColor, `tank_normal_map.tga` → Normal).
pub fn detect_slot(path: &str) -> Option<TextureSlot> {
    let stem = paths::stem(path);
    let tokens = tokenize(stem);
    for t in tokens.iter().rev() {
        if is_noise_token(t) || t.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // Only the last meaningful token counts: suffix convention.
        return slot_from_token(t).or_else(|| detect_compound_slot(t));
    }
    None
}

/// Handles glued suffixes such as `TankNormal`, `body_BaseColorMap`.
fn detect_compound_slot(token: &str) -> Option<TextureSlot> {
    compound_slot(token).map(|(_, slot)| slot)
}

/// The glued slot suffix of `token` and its slot.
fn compound_slot(token: &str) -> Option<(&'static str, TextureSlot)> {
    let t = token.to_ascii_lowercase();
    const SUFFIXES: &[(&str, TextureSlot)] = &[
        ("basecolor", TextureSlot::BaseColor),
        ("albedo", TextureSlot::BaseColor),
        ("diffuse", TextureSlot::BaseColor),
        ("normal", TextureSlot::Normal),
        ("roughness", TextureSlot::Roughness),
        ("metallic", TextureSlot::Metallic),
        ("metalness", TextureSlot::Metallic),
        ("occlusion", TextureSlot::Occlusion),
        ("emissive", TextureSlot::Emissive),
        ("height", TextureSlot::Height),
        ("opacity", TextureSlot::Opacity),
        ("specular", TextureSlot::Specular),
    ];
    SUFFIXES.iter().find(|(s, _)| t.len() > s.len() && t.ends_with(s)).copied()
}

/// Index of all image files of a version, for name based lookups.
pub struct ImageIndex {
    /// Version-relative paths of every image.
    pub images: Vec<String>,
    by_lower_path: HashMap<String, String>,
    by_lower_name: BTreeMap<String, Vec<String>>,
}

impl ImageIndex {
    pub fn new(mut images: Vec<String>) -> Self {
        images.sort();
        let mut by_lower_path = HashMap::new();
        let mut by_lower_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for p in &images {
            by_lower_path.insert(p.to_ascii_lowercase(), p.clone());
            by_lower_name.entry(paths::file_name(p).to_ascii_lowercase()).or_default().push(p.clone());
        }
        Self { images, by_lower_path, by_lower_name }
    }

    pub fn contains(&self, path: &str) -> bool {
        self.by_lower_path.contains_key(&path.to_ascii_lowercase())
    }

    /// The version path of an image given by its version-relative path (case-insensitive).
    pub fn find(&self, path: &str) -> Option<String> {
        let n = paths::normalize(path)?;
        self.by_lower_path.get(&n.to_ascii_lowercase()).cloned()
    }

    /// Fallback for a reference whose file was renamed: the image with the same
    /// slot and the same number tokens (UDIM tile, part number). Ties go to the
    /// candidate sharing most name tokens with the original, then alphabetical.
    pub fn resolve_tile(&self, original: &str, slot: TextureSlot) -> Option<String> {
        let cleaned = clean_original(original);
        let numbers = number_tokens(&cleaned);
        if numbers.is_empty() {
            return None;
        }
        let orig: HashSet<String> =
            tokenize(paths::stem(&cleaned)).into_iter().map(|t| t.to_ascii_lowercase()).collect();
        let shared = |p: &str| tokenize(paths::stem(p)).iter().filter(|t| orig.contains(&t.to_ascii_lowercase())).count();
        self.images
            .iter()
            .filter(|p| detect_slot(p) == Some(slot) && number_tokens(p) == numbers)
            .max_by(|a, b| shared(a).cmp(&shared(b)).then_with(|| b.cmp(a)))
            .cloned()
    }

    /// Resolve a path written inside a model file. Returns the version path
    /// and how it was found ("exact" / "filename").
    pub fn resolve(&self, original: &str, base_dir: &str) -> Option<(String, &'static str)> {
        let cleaned = clean_original(original);
        if cleaned.is_empty() {
            return None;
        }
        // 1. relative to the model directory
        let joined = if base_dir.is_empty() { cleaned.clone() } else { format!("{base_dir}/{cleaned}") };
        if let Some(n) = paths::normalize(&joined)
            && let Some(p) = self.by_lower_path.get(&n.to_ascii_lowercase())
        {
            return Some((p.clone(), "exact"));
        }
        // 2. relative to the version root
        if let Some(n) = paths::normalize(&cleaned)
            && let Some(p) = self.by_lower_path.get(&n.to_ascii_lowercase())
        {
            return Some((p.clone(), "exact"));
        }
        // 3. same file name anywhere; prefer the candidate sharing the longest
        //    directory suffix with the original, then the one closest to the model.
        let name = paths::file_name(&cleaned).to_ascii_lowercase();
        if let Some(cands) = self.by_lower_name.get(&name) {
            return Some((best_candidate(cands, &cleaned, base_dir), "filename"));
        }
        // 4. same stem, different extension (FBX says .tga, the pack ships .png)
        let stem = paths::stem(&cleaned).to_ascii_lowercase();
        let mut same_stem: Vec<&String> =
            self.images.iter().filter(|p| paths::stem(p).to_ascii_lowercase() == stem).collect();
        same_stem.sort();
        if let Some(p) = same_stem.first() {
            return Some(((*p).clone(), "filename"));
        }
        None
    }
}

fn clean_original(original: &str) -> String {
    let s = original.trim().trim_matches('"');
    // MTL options: "-bm 0.5 textures/n.png" → last token
    let s = if s.starts_with('-') { s.split_whitespace().last().unwrap_or("") } else { s };
    let s = s.strip_prefix("file:///").or_else(|| s.strip_prefix("file://")).unwrap_or(s);
    s.replace('\\', "/")
}

fn best_candidate(cands: &[String], original: &str, base_dir: &str) -> String {
    let orig_dirs: Vec<String> =
        paths::parent(original).split('/').filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()).collect();
    let score = |p: &String| -> (usize, usize, usize) {
        let dirs: Vec<String> =
            paths::parent(p).split('/').filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()).collect();
        // common directory suffix length
        let mut common = 0;
        for (a, b) in dirs.iter().rev().zip(orig_dirs.iter().rev()) {
            if a == b {
                common += 1;
            } else {
                break;
            }
        }
        let under_model = usize::from(base_dir.is_empty() || p.starts_with(base_dir));
        (common, under_model, usize::MAX - p.len())
    };
    // ties: alphabetical order wins (deterministic, "Desert" before "Winter")
    cands.iter().max_by(|a, b| score(a).cmp(&score(b)).then_with(|| b.cmp(a))).cloned().unwrap_or_default()
}

/// Name tokens of a material without conventional prefixes (`M_`, `MI_`, `MAT_`)
/// and trailing material/instance noise, lower-cased.
pub fn material_core(name: &str) -> Vec<String> {
    let mut toks: Vec<String> = tokenize(name).into_iter().map(|t| t.to_ascii_lowercase()).collect();
    while let Some(first) = toks.first() {
        if matches!(first.as_str(), "m" | "mi" | "mat" | "material" | "mtl") && toks.len() > 1 {
            toks.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = toks.last() {
        if (matches!(last.as_str(), "mat" | "material" | "mtl" | "inst" | "mi")
            || last.chars().all(|c| c.is_ascii_digit()))
            && toks.len() > 1
        {
            toks.pop();
        } else {
            break;
        }
    }
    toks
}

/// Tokens of an image stem without `T_`/`TX_` prefix and slot/noise suffixes.
pub fn image_core(path: &str) -> Vec<String> {
    let mut toks: Vec<String> = tokenize(paths::stem(path)).into_iter().map(|t| t.to_ascii_lowercase()).collect();
    while let Some(first) = toks.first() {
        if matches!(first.as_str(), "t" | "tx" | "tex" | "texture") && toks.len() > 1 {
            toks.remove(0);
        } else {
            break;
        }
    }
    while let Some(last) = toks.last() {
        if (is_noise_token(last) || slot_from_token(last).is_some() || last.chars().all(|c| c.is_ascii_digit()))
            && toks.len() > 1
        {
            toks.pop();
        } else {
            break;
        }
    }
    toks
}

fn contains_subsequence(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Result of resolving one material texture.
#[derive(Debug, Clone)]
pub struct ResolvedTexture {
    pub slot: TextureSlot,
    /// Version-relative path when the texture is a file of the version.
    pub path: Option<String>,
    /// Embedded image bytes when the texture lives inside the model.
    pub embedded: Option<std::sync::Arc<Vec<u8>>>,
    pub original: Option<String>,
    pub resolved_by: &'static str,
}

/// Resolve every texture of every material, in order of precedence:
///
/// 1. references stored in the model (exact path, file name, same stem);
/// 2. for a missing reference, the only image with the same slot and the same
///    tile / number tokens (`low1_BaseColor.1001.png` → `Tank_BaseColor.1001.png`);
/// 3. manual assignments from [`MaterialConfig`] — they override 1 and 2, and a
///    `null` assignment keeps the slot empty;
/// 4. material-name heuristic for slots still empty;
/// 5. siblings of an already resolved texture: same folder and name except
///    for the slot token (`Tank_BaseColor.1001` → `Tank_Normal.1001`).
pub fn resolve_materials(
    materials: &[Material],
    index: &ImageIndex,
    manual: Option<&MaterialConfig>,
    warnings: &mut Vec<String>,
) -> Vec<Vec<ResolvedTexture>> {
    let mut out: Vec<Vec<ResolvedTexture>> = Vec::with_capacity(materials.len());
    for mat in materials {
        let mut list: Vec<ResolvedTexture> = Vec::new();
        for tex in &mat.textures {
            if list.iter().any(|r| r.slot == tex.slot && (r.path.is_some() || r.embedded.is_some())) {
                continue;
            }
            match &tex.source {
                TexSource::Path { original, base_dir } => {
                    let found = index
                        .resolve(original, base_dir)
                        .or_else(|| index.resolve_tile(original, tex.slot).map(|p| (p, "slot-tile")));
                    list.push(ResolvedTexture {
                        slot: tex.slot,
                        path: found.as_ref().map(|(p, _)| p.clone()),
                        embedded: None,
                        original: Some(original.clone()),
                        resolved_by: found.map(|(_, how)| how).unwrap_or("missing"),
                    });
                }
                TexSource::Embedded { name, bytes } => {
                    // Prefer the loose file with the same name when the pack ships it
                    // (lets variants work), otherwise use the embedded bytes.
                    if let Some((p, _)) = index.resolve(name, "").filter(|_| looks_like_file_name(name)) {
                        list.push(ResolvedTexture {
                            slot: tex.slot,
                            path: Some(p),
                            embedded: None,
                            original: Some(name.clone()),
                            resolved_by: "filename",
                        });
                    } else {
                        list.push(ResolvedTexture {
                            slot: tex.slot,
                            path: None,
                            embedded: Some(bytes.clone()),
                            original: Some(name.clone()),
                            resolved_by: "embedded",
                        });
                    }
                }
            }
        }
        out.push(list);
    }

    // Manual assignments.
    let mut cleared: Vec<HashSet<TextureSlot>> = vec![HashSet::new(); materials.len()];
    if let Some(cfg) = manual {
        for name in cfg.materials.keys() {
            if !materials.iter().any(|m| m.name.eq_ignore_ascii_case(name)) {
                warnings.push(format!("{MATERIAL_CONFIG_PATH}: material '{name}' does not exist in the model"));
            }
        }
        for (mi, mat) in materials.iter().enumerate() {
            let Some(assignments) = cfg.assignments(&mat.name) else { continue };
            for (&slot, value) in assignments {
                match value {
                    Some(path) => match index.find(path) {
                        Some(p) => {
                            // A manual choice also displaces automatic textures of an
                            // overlapping slot (manual roughness beats an ORM found by name).
                            out[mi].retain(|r| {
                                !(r.slot == slot || (r.resolved_by != "manual" && overlaps(slot, r.slot)))
                            });
                            out[mi].push(ResolvedTexture {
                                slot,
                                path: Some(p),
                                embedded: None,
                                original: None,
                                resolved_by: "manual",
                            });
                        }
                        None => warnings.push(format!(
                            "material '{}': manual {slot} texture '{path}' not found in the version",
                            mat.name
                        )),
                    },
                    None => {
                        out[mi].retain(|r| r.slot != slot);
                        cleared[mi].insert(slot);
                    }
                }
            }
        }
    }

    // Material-name heuristic for materials without a resolved base colour.
    let cores: Vec<Vec<String>> = materials.iter().map(|m| material_core(&m.name)).collect();
    // Assign each image to the material with the longest matching core.
    let mut by_material: Vec<Vec<&String>> = vec![vec![]; materials.len()];
    for img in &index.images {
        let ic = image_core(img);
        let best = cores
            .iter()
            .enumerate()
            .filter(|(_, c)| contains_subsequence(&ic, c))
            .max_by_key(|(_, c)| c.len());
        if let Some((mi, _)) = best {
            by_material[mi].push(img);
        }
    }
    let single_material = materials.len() == 1;
    for mi in 0..materials.len() {
        let mut candidates: Vec<&String> = by_material[mi].clone();
        let mut how = "material-name";
        if candidates.is_empty() && single_material && !covered(&out[mi], &cleared[mi], TextureSlot::BaseColor) {
            candidates = index.images.iter().collect();
            how = "single-material";
        }
        if candidates.is_empty() {
            continue;
        }
        let core_len = cores[mi].len();
        for &slot in HEURISTIC_SLOTS {
            if covered(&out[mi], &cleared[mi], slot) {
                continue;
            }
            // Only fill the slot if nothing else was found for it.
            let mut matching: Vec<&&String> = candidates.iter().filter(|p| detect_slot(p) == Some(slot)).collect();
            // Fewest extra tokens first, then alphabetical.
            matching.sort_by_key(|p| (image_core(p).len().saturating_sub(core_len), (**p).clone()));
            if let Some(p) = matching.first() {
                fill(&mut out[mi], slot, (**p).clone(), how);
            }
        }
    }

    // Texture-set siblings of what is already resolved (base colour first).
    for mi in 0..materials.len() {
        let mut anchors: Vec<(TextureSlot, String)> =
            out[mi].iter().filter_map(|r| r.path.clone().map(|p| (r.slot, p))).collect();
        anchors.sort_by_key(|(slot, _)| *slot != TextureSlot::BaseColor);
        let Some(key) = anchors.iter().find_map(|(_, p)| set_key(p)) else { continue };
        for &slot in HEURISTIC_SLOTS {
            if covered(&out[mi], &cleared[mi], slot) {
                continue;
            }
            let sibling = index.images.iter().find(|p| detect_slot(p) == Some(slot) && set_key(p).as_ref() == Some(&key));
            if let Some(p) = sibling {
                fill(&mut out[mi], slot, p.clone(), "sibling");
            }
        }
    }

    for (mat, list) in materials.iter().zip(&out) {
        for r in list {
            if r.resolved_by == "missing" {
                warnings.push(format!(
                    "material '{}': texture '{}' ({}) not found in the version",
                    mat.name,
                    r.original.as_deref().unwrap_or("?"),
                    r.slot
                ));
            }
        }
    }
    out
}

/// Slots the heuristics try to fill, in priority order.
const HEURISTIC_SLOTS: &[TextureSlot] = &[
    TextureSlot::BaseColor,
    TextureSlot::Normal,
    TextureSlot::Orm,
    TextureSlot::MetallicRoughness,
    TextureSlot::Roughness,
    TextureSlot::Metallic,
    TextureSlot::Occlusion,
    TextureSlot::Emissive,
    TextureSlot::Opacity,
    TextureSlot::Specular,
];

/// Whether two different slots feed the same glTF channels, so only one of
/// them can be used (an ORM map already provides roughness, metallic and AO).
fn overlaps(a: TextureSlot, b: TextureSlot) -> bool {
    use TextureSlot::*;
    let pair = |x, y| (a == x && b == y) || (a == y && b == x);
    pair(Orm, MetallicRoughness)
        || pair(Orm, Roughness)
        || pair(Orm, Metallic)
        || pair(Orm, Occlusion)
        || pair(MetallicRoughness, Roughness)
        || pair(MetallicRoughness, Metallic)
}

/// Whether `slot` already has a texture (directly or through an overlapping
/// slot) or was explicitly left empty.
fn covered(list: &[ResolvedTexture], cleared: &HashSet<TextureSlot>, slot: TextureSlot) -> bool {
    cleared.contains(&slot)
        || list
            .iter()
            .any(|r| (r.path.is_some() || r.embedded.is_some()) && (r.slot == slot || overlaps(slot, r.slot)))
}

/// Fill `slot` with a heuristic match, replacing its "missing" record if any.
fn fill(list: &mut Vec<ResolvedTexture>, slot: TextureSlot, path: String, how: &'static str) {
    list.retain(|r| !(r.slot == slot && r.path.is_none() && r.embedded.is_none()));
    list.push(ResolvedTexture { slot, path: Some(path), embedded: None, original: None, resolved_by: how });
}

/// Identity of the texture set an image belongs to: its folder plus the stem
/// tokens without the slot token and noise. `Tank_BaseColor.1001.png` and
/// `Tank_Normal.1001.png` share `"|tank 1001"`. None when no slot token is found.
pub fn set_key(path: &str) -> Option<String> {
    let mut toks: Vec<String> = tokenize(paths::stem(path)).into_iter().map(|t| t.to_ascii_lowercase()).collect();
    let i = toks.iter().rposition(|t| !is_noise_token(t) && !t.chars().all(|c| c.is_ascii_digit()))?;
    if slot_from_token(&toks[i]).is_some() {
        toks.remove(i);
    } else {
        let (suffix, _) = compound_slot(&toks[i])?;
        let keep = toks[i].len() - suffix.len();
        toks[i].truncate(keep);
    }
    toks.retain(|t| !is_noise_token(t));
    Some(format!("{}|{}", paths::parent(path).to_ascii_lowercase(), toks.join(" ")))
}

/// All-digit tokens of a file stem that are not texture sizes (UDIM tiles,
/// part numbers): `low1_BaseColor.1001` → `["1001"]`.
fn number_tokens(path: &str) -> Vec<String> {
    tokenize(paths::stem(path))
        .into_iter()
        .filter(|t| t.chars().all(|c| c.is_ascii_digit()) && !is_resolution_token(t))
        .collect()
}

fn looks_like_file_name(name: &str) -> bool {
    paths::is_image(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::TexRef;

    #[test]
    fn slots() {
        assert_eq!(detect_slot("T_Abrams_Desert_D.png"), Some(TextureSlot::BaseColor));
        assert_eq!(detect_slot("Textures/T_Abrams_N.tga"), Some(TextureSlot::Normal));
        assert_eq!(detect_slot("tank_normal_map.png"), Some(TextureSlot::Normal));
        assert_eq!(detect_slot("Tank_ORM_4k.png"), Some(TextureSlot::Orm));
        assert_eq!(detect_slot("body_BaseColor.png"), Some(TextureSlot::BaseColor));
        assert_eq!(detect_slot("body-roughness.jpg"), Some(TextureSlot::Roughness));
        assert_eq!(detect_slot("TankBaseColor.png"), Some(TextureSlot::BaseColor));
        assert_eq!(detect_slot("T_Body_S.png"), Some(TextureSlot::Specular));
        assert_eq!(detect_slot("body_spec_2k.tga"), Some(TextureSlot::Specular));
        assert_eq!(detect_slot("BodySpecular.png"), Some(TextureSlot::Specular));
        assert_eq!(detect_slot("body_gloss.png"), None, "glossiness is not specular");
        assert_eq!(detect_slot("logo.png"), None);
    }

    #[test]
    fn resolution_tokens() {
        assert!(is_resolution_token("4k"));
        assert!(is_resolution_token("2048"));
        assert!(is_resolution_token("1024x1024"));
        assert!(!is_resolution_token("desert"));
        assert!(!is_resolution_token("01"));
    }

    #[test]
    fn resolves_absolute_author_paths() {
        let idx = ImageIndex::new(vec![
            "Abrams/Textures/T_Body_D.png".into(),
            "Abrams/Textures/T_Body_N.png".into(),
            "Other/T_Body_D.png".into(),
        ]);
        let (p, how) = idx.resolve(r"C:\Users\artist\Desktop\Textures\T_Body_D.png", "Abrams").unwrap();
        assert_eq!(p, "Abrams/Textures/T_Body_D.png");
        assert_eq!(how, "filename");
        let (p, how) = idx.resolve("Textures/T_Body_N.png", "Abrams").unwrap();
        assert_eq!((p.as_str(), how), ("Abrams/Textures/T_Body_N.png", "exact"));
        // different extension
        let (p, _) = idx.resolve("Textures/T_Body_N.tga", "Abrams").unwrap();
        assert_eq!(p, "Abrams/Textures/T_Body_N.png");
        assert!(idx.resolve("nothing.png", "").is_none());
    }

    #[test]
    fn material_name_heuristic() {
        let idx = ImageIndex::new(vec![
            "T_Abrams_Desert_D.png".into(),
            "T_Abrams_N.png".into(),
            "T_Abrams_Tracks_D.png".into(),
            "T_Abrams_Tracks_N.png".into(),
        ]);
        let mats = vec![Material::named("M_Abrams"), Material::named("M_Abrams_Tracks")];
        let mut w = vec![];
        let r = resolve_materials(&mats, &idx, None, &mut w);
        let body_d = r[0].iter().find(|t| t.slot == TextureSlot::BaseColor).unwrap();
        assert_eq!(body_d.path.as_deref(), Some("T_Abrams_Desert_D.png"));
        let body_n = r[0].iter().find(|t| t.slot == TextureSlot::Normal).unwrap();
        assert_eq!(body_n.path.as_deref(), Some("T_Abrams_N.png"));
        let tr_d = r[1].iter().find(|t| t.slot == TextureSlot::BaseColor).unwrap();
        assert_eq!(tr_d.path.as_deref(), Some("T_Abrams_Tracks_D.png"));
    }

    #[test]
    fn missing_is_reported() {
        let idx = ImageIndex::new(vec![]);
        let mut m = Material::named("lambert1");
        m.textures.push(TexRef {
            slot: TextureSlot::BaseColor,
            source: TexSource::Path { original: "C:/x/foo.png".into(), base_dir: String::new() },
        });
        let mut w = vec![];
        let r = resolve_materials(&[m], &idx, None, &mut w);
        assert_eq!(r[0][0].resolved_by, "missing");
        assert_eq!(w.len(), 1);
    }

    fn with_ref(name: &str, slot: TextureSlot, original: &str) -> Material {
        let mut m = Material::named(name);
        m.textures.push(TexRef { slot, source: TexSource::Path { original: original.into(), base_dir: String::new() } });
        m
    }

    fn path_of(list: &[ResolvedTexture], slot: TextureSlot) -> Option<(&str, &str)> {
        list.iter().find(|r| r.slot == slot).and_then(|r| r.path.as_deref().map(|p| (p, r.resolved_by)))
    }

    /// Renamed UDIM-style tiles: references point at `low1_standardSurface2_*`,
    /// the pack ships `Tank_*`; material names (`a1`) carry no information.
    #[test]
    fn renamed_tiles_and_siblings() {
        let mut images = vec![];
        for tile in ["1001", "1002"] {
            for slot in ["BaseColor", "Normal", "Metallic", "Roughness", "Emissive"] {
                images.push(format!("Tank_{slot}.{tile}.png"));
            }
        }
        let idx = ImageIndex::new(images);
        let mats = vec![
            with_ref("a1", TextureSlot::BaseColor, r"..\..\Png\low1_standardSurface2_BaseColor.1001.png"),
            with_ref("a2", TextureSlot::BaseColor, r"..\..\Png\low1_standardSurface2_BaseColor.1002.png"),
        ];
        let mut w = vec![];
        let r = resolve_materials(&mats, &idx, None, &mut w);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(path_of(&r[0], TextureSlot::BaseColor), Some(("Tank_BaseColor.1001.png", "slot-tile")));
        assert_eq!(path_of(&r[1], TextureSlot::BaseColor), Some(("Tank_BaseColor.1002.png", "slot-tile")));
        assert_eq!(path_of(&r[0], TextureSlot::Normal), Some(("Tank_Normal.1001.png", "sibling")));
        assert_eq!(path_of(&r[0], TextureSlot::Roughness), Some(("Tank_Roughness.1001.png", "sibling")));
        assert_eq!(path_of(&r[1], TextureSlot::Metallic), Some(("Tank_Metallic.1002.png", "sibling")));
        assert_eq!(path_of(&r[1], TextureSlot::Emissive), Some(("Tank_Emissive.1002.png", "sibling")));
        // No number in the reference: the tile fallback must not guess.
        assert!(idx.resolve_tile("other_BaseColor.png", TextureSlot::BaseColor).is_none());
    }

    #[test]
    fn manual_assignments_win() {
        let idx = ImageIndex::new(vec![
            "T_Body_D.png".into(),
            "T_Body_ORM.png".into(),
            "T_Body_N.png".into(),
            "Alt/Body_Rough.png".into(),
            "Alt/Body_Color.png".into(),
        ]);
        let mats = vec![Material::named("M_Body")];
        let cfg = MaterialConfig::from_json(
            br#"{ "materials": {
                "m_body": { "base_color": "Alt/Body_Color.png", "roughness": "alt/body_rough.png", "normal": null },
                "Ghost": { "base_color": "T_Body_D.png" }
            } }"#,
        )
        .unwrap();
        let mut w = vec![];
        let r = resolve_materials(&mats, &idx, Some(&cfg), &mut w);
        assert_eq!(path_of(&r[0], TextureSlot::BaseColor), Some(("Alt/Body_Color.png", "manual")));
        assert_eq!(path_of(&r[0], TextureSlot::Roughness), Some(("Alt/Body_Rough.png", "manual")));
        // Cleared slot stays empty; the ORM found by name is displaced by the manual roughness.
        assert!(r[0].iter().all(|t| t.slot != TextureSlot::Normal && t.slot != TextureSlot::Orm), "{:?}", r[0]);
        assert!(w.iter().any(|m| m.contains("'Ghost'")), "{w:?}");

        let bad = MaterialConfig::from_json(br#"{ "materials": { "M_Body": { "base_color": "nope.png" } } }"#).unwrap();
        let mut w = vec![];
        let r = resolve_materials(&mats, &idx, Some(&bad), &mut w);
        assert!(w.iter().any(|m| m.contains("nope.png")), "{w:?}");
        // Falls back to the automatic match.
        assert_eq!(path_of(&r[0], TextureSlot::BaseColor).map(|(_, how)| how), Some("material-name"));
    }

    #[test]
    fn texture_set_keys() {
        assert_eq!(set_key("Tank_BaseColor.1001.png"), set_key("Tank_Normal.1001.png"));
        assert_ne!(set_key("Tank_BaseColor.1001.png"), set_key("Tank_Normal.1002.png"));
        assert_eq!(set_key("a/TankBaseColor.png"), set_key("a/Tank_Normal_4k.png"));
        assert_ne!(set_key("a/Tank_D.png"), set_key("b/Tank_N.png"));
        assert_eq!(set_key("logo.png"), None);
    }
}
