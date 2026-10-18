//! Levels of detail: authored (by naming convention) or generated with
//! meshoptimizer.

use crate::scene::MeshPrim;
use hangar_core::LodSource;

/// Parses an LOD suffix: `Body_LOD1`, `Body.lod2`, `body-LOD0`, `LOD3`,
/// also tolerating Blender duplicate suffixes (`Body_LOD1.001`).
pub fn parse_lod_suffix(name: &str) -> Option<u32> {
    let mut s = name.trim();
    // strip Blender ".001" style suffix
    if let Some(i) = s.rfind('.') {
        let tail = &s[i + 1..];
        if tail.len() == 3 && tail.chars().all(|c| c.is_ascii_digit()) {
            s = &s[..i];
        }
    }
    let lower = s.to_ascii_lowercase();
    let digits_len = lower.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 || digits_len > 2 {
        return None;
    }
    let head = &lower[..lower.len() - digits_len];
    let head = head.strip_suffix("lod")?;
    if head.is_empty() || head.ends_with(['_', '.', '-', ' ']) {
        lower[lower.len() - digits_len..].parse().ok()
    } else {
        None
    }
}

/// One level of detail ready to be exported.
pub struct LodLevel {
    pub level: u32,
    pub source: LodSource,
    pub meshes: Vec<MeshPrim>,
    pub ratio: f32,
}

impl LodLevel {
    pub fn triangles(&self) -> u64 {
        self.meshes.iter().map(|m| m.triangle_count() as u64).sum()
    }
    pub fn vertices(&self) -> u64 {
        self.meshes.iter().map(|m| m.positions.len() as u64).sum()
    }
}

/// Split meshes carrying authored LOD suffixes into levels. Meshes without a
/// suffix appear in every level. Returns None when nothing is tagged.
pub fn split_authored(meshes: &[MeshPrim]) -> Option<Vec<LodLevel>> {
    let mut levels: Vec<u32> = meshes.iter().filter_map(|m| m.lod).collect();
    levels.sort();
    levels.dedup();
    if levels.len() < 2 {
        return None;
    }
    let mut out: Vec<LodLevel> = levels
        .iter()
        .map(|&l| LodLevel {
            level: l,
            source: LodSource::Authored,
            meshes: meshes.iter().filter(|m| m.lod.is_none() || m.lod == Some(l)).cloned().collect(),
            ratio: 1.0,
        })
        .collect();
    renumber_and_ratio(&mut out);
    Some(out)
}

/// Renumbers levels 0..n (authored files may start at LOD1) and fills ratios.
pub fn renumber_and_ratio(levels: &mut [LodLevel]) {
    let base = levels.first().map(|l| l.triangles()).unwrap_or(0).max(1) as f32;
    for (i, l) in levels.iter_mut().enumerate() {
        l.level = i as u32;
        l.ratio = l.triangles() as f32 / base;
    }
}

/// Generate simplified LODs from LOD0 (LOD0 itself is not included).
pub fn generate(lod0: &[MeshPrim], ratios: &[f32], min_triangles: u64) -> Vec<LodLevel> {
    let total: u64 = lod0.iter().map(|m| m.triangle_count() as u64).sum();
    if total < min_triangles.max(1) {
        return vec![];
    }
    let mut out: Vec<LodLevel> = Vec::new();
    let mut prev_tris = total;
    for (i, &ratio) in ratios.iter().enumerate() {
        if !(0.0..1.0).contains(&ratio) || ratio <= 0.0 {
            continue;
        }
        // Allow larger geometric error for coarser levels.
        let target_error = (0.02 * 2f32.powi(i as i32)).min(0.25);
        let meshes: Vec<MeshPrim> = lod0.iter().map(|m| simplify_prim(m, ratio, target_error)).filter(|m| !m.indices.is_empty()).collect();
        let tris: u64 = meshes.iter().map(|m| m.triangle_count() as u64).sum();
        // Stop when the reduction stalls (less than 10% fewer triangles than the previous level).
        if tris == 0 || tris as f32 > prev_tris as f32 * 0.9 {
            break;
        }
        prev_tris = tris;
        out.push(LodLevel {
            level: (i + 1) as u32,
            source: LodSource::Generated,
            meshes,
            ratio: tris as f32 / total as f32,
        });
    }
    out
}

/// Simplify a single primitive to `ratio` of its triangles and compact the
/// vertex buffer.
pub fn simplify_prim(m: &MeshPrim, ratio: f32, target_error: f32) -> MeshPrim {
    let tri_count = m.triangle_count();
    if tri_count < 8 || m.positions.is_empty() {
        return m.clone();
    }
    let target = ((m.indices.len() as f32 * ratio) as usize / 3 * 3).max(3);
    let mut err = 0.0f32;
    let mut idx = meshopt::simplify_decoder(
        &m.indices,
        &m.positions,
        target,
        target_error,
        meshopt::SimplifyOptions::None,
        Some(&mut err),
    );
    // If the error bound stopped us far from the target, retry sloppier.
    if idx.len() > target * 2 {
        idx = meshopt::simplify_decoder(
            &m.indices,
            &m.positions,
            target,
            target_error * 4.0,
            meshopt::SimplifyOptions::None,
            Some(&mut err),
        );
    }
    compact(m, &idx)
}

/// Build a new primitive that only contains the vertices referenced by `indices`.
pub fn compact(m: &MeshPrim, indices: &[u32]) -> MeshPrim {
    let mut remap = vec![u32::MAX; m.positions.len()];
    let mut out = MeshPrim {
        name: m.name.clone(),
        material: m.material,
        lod: m.lod,
        node: m.node,
        skin: m.skin,
        ..Default::default()
    };
    let has_n = m.normals.len() == m.positions.len();
    let has_uv = m.uvs.len() == m.positions.len();
    let has_skin = m.joints.len() == m.positions.len() && m.weights.len() == m.positions.len();
    for &i in indices {
        let i = i as usize;
        if remap[i] == u32::MAX {
            remap[i] = out.positions.len() as u32;
            out.positions.push(m.positions[i]);
            if has_n {
                out.normals.push(m.normals[i]);
            }
            if has_uv {
                out.uvs.push(m.uvs[i]);
            }
            if has_skin {
                out.joints.push(m.joints[i]);
                out.weights.push(m.weights[i]);
            }
        }
        out.indices.push(remap[i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes() {
        assert_eq!(parse_lod_suffix("Body_LOD1"), Some(1));
        assert_eq!(parse_lod_suffix("body.lod2"), Some(2));
        assert_eq!(parse_lod_suffix("Tank-LOD0"), Some(0));
        assert_eq!(parse_lod_suffix("LOD3"), Some(3));
        assert_eq!(parse_lod_suffix("Body_LOD1.001"), Some(1));
        assert_eq!(parse_lod_suffix("Tank_LOD1 "), Some(1));
        assert_eq!(parse_lod_suffix("Floodlight1"), None);
        assert_eq!(parse_lod_suffix("Body"), None);
        assert_eq!(parse_lod_suffix("Body_1"), None);
    }

    fn grid(n: usize, lod: Option<u32>) -> MeshPrim {
        let mut m = MeshPrim { name: "grid".into(), lod, ..Default::default() };
        for y in 0..=n {
            for x in 0..=n {
                let fx = x as f32 / n as f32;
                let fy = y as f32 / n as f32;
                m.positions.push([fx, (fx * 6.0).sin() * 0.1 + (fy * 4.0).cos() * 0.1, fy]);
            }
        }
        for y in 0..n {
            for x in 0..n {
                let i = (y * (n + 1) + x) as u32;
                let r = (n + 1) as u32;
                m.indices.extend_from_slice(&[i, i + r, i + 1, i + 1, i + r, i + r + 1]);
            }
        }
        m
    }

    #[test]
    fn generates_lods() {
        let lod0 = vec![grid(40, None)];
        let lods = generate(&lod0, &[0.5, 0.25, 0.1], 100);
        assert!(lods.len() >= 2, "only {} lods", lods.len());
        assert!(lods[0].triangles() < 3200);
        assert!(lods.windows(2).all(|w| w[1].triangles() < w[0].triangles()));
        // vertex buffers are compacted
        assert!(lods[0].vertices() < 41 * 41);
    }

    #[test]
    fn authored_split() {
        let meshes = vec![grid(2, Some(0)), grid(1, Some(1)), grid(1, None)];
        let lods = split_authored(&meshes).unwrap();
        assert_eq!(lods.len(), 2);
        assert_eq!(lods[0].meshes.len(), 2);
        assert_eq!(lods[1].meshes.len(), 2);
        assert!(lods[1].ratio < 1.0);
        assert!(split_authored(&[grid(1, None)]).is_none());
    }
}
