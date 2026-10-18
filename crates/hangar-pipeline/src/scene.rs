//! Intermediate representation shared by all importers: a flat list of
//! world-space triangle meshes plus PBR-ish materials.

use hangar_core::TextureSlot;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<MeshPrim>,
    pub materials: Vec<Material>,
    /// Which importer produced the scene ("ufbx", "gltf", "obj", "blender").
    pub importer: &'static str,
    pub has_skin: bool,
    pub animations: u32,
    /// Whether the source provided normals (before we generated missing ones).
    pub source_had_normals: bool,
    pub warnings: Vec<String>,
}

/// One triangle list with a single material, already in world space
/// (Y-up, metres, right-handed — the glTF convention).
#[derive(Debug, Clone, Default)]
pub struct MeshPrim {
    pub name: String,
    pub material: Option<usize>,
    pub positions: Vec<[f32; 3]>,
    /// Empty or same length as `positions`.
    pub normals: Vec<[f32; 3]>,
    /// Empty or same length as `positions`. glTF convention (v pointing down).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Authored LOD level parsed from the node/mesh name (`Body_LOD1` → 1).
    pub lod: Option<u32>,
}

impl MeshPrim {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub double_sided: bool,
    pub textures: Vec<TexRef>,
}

impl Material {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.8,
            emissive: [0.0; 3],
            double_sided: false,
            textures: vec![],
        }
    }
}

/// A texture reference as found in the source file.
#[derive(Debug, Clone)]
pub struct TexRef {
    pub slot: TextureSlot,
    pub source: TexSource,
}

#[derive(Debug, Clone)]
pub enum TexSource {
    /// A path as written in the source file, relative to `base_dir` (the
    /// version-relative directory of the model file) or absolute on the
    /// author's machine.
    Path { original: String, base_dir: String },
    /// Image bytes embedded in the model (GLB, FBX with embedded media).
    Embedded { name: String, bytes: Arc<Vec<u8>> },
}

impl TexSource {
    pub fn display(&self) -> &str {
        match self {
            TexSource::Path { original, .. } => original,
            TexSource::Embedded { name, .. } => name,
        }
    }
}

/// Bounds helper.
#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn empty() -> Self {
        Self { min: [f32::INFINITY; 3], max: [f32::NEG_INFINITY; 3] }
    }
    pub fn extend(&mut self, p: [f32; 3]) {
        for (i, v) in p.iter().enumerate() {
            self.min[i] = self.min[i].min(*v);
            self.max[i] = self.max[i].max(*v);
        }
    }
    pub fn is_valid(&self) -> bool {
        self.min[0] <= self.max[0]
    }
    pub fn of_meshes<'a>(meshes: impl IntoIterator<Item = &'a MeshPrim>) -> Self {
        let mut b = Aabb::empty();
        for m in meshes {
            for &i in &m.indices {
                if let Some(p) = m.positions.get(i as usize) {
                    b.extend(*p);
                }
            }
        }
        b
    }
}

/// Fill in missing normals with area-weighted smooth normals.
pub fn ensure_normals(mesh: &mut MeshPrim) {
    if mesh.normals.len() == mesh.positions.len() && !mesh.positions.is_empty() {
        return;
    }
    let mut normals = vec![[0.0f32; 3]; mesh.positions.len()];
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= normals.len() || b >= normals.len() || c >= normals.len() {
            continue;
        }
        let pa = glam::Vec3::from(mesh.positions[a]);
        let pb = glam::Vec3::from(mesh.positions[b]);
        let pc = glam::Vec3::from(mesh.positions[c]);
        let n = (pb - pa).cross(pc - pa);
        for &v in &[a, b, c] {
            let cur = glam::Vec3::from(normals[v]) + n;
            normals[v] = cur.into();
        }
    }
    for n in &mut normals {
        let v = glam::Vec3::from(*n);
        *n = if v.length_squared() > 0.0 { v.normalize().into() } else { [0.0, 1.0, 0.0] };
    }
    mesh.normals = normals;
}

/// Deduplicating vertex builder used by importers that expand per-corner data.
#[derive(Default)]
pub struct VertexDedup {
    map: std::collections::HashMap<[u32; 8], u32>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl VertexDedup {
    pub fn push(&mut self, p: [f32; 3], n: Option<[f32; 3]>, uv: Option<[f32; 2]>) {
        let nn = n.unwrap_or([0.0; 3]);
        let tt = uv.unwrap_or([0.0; 2]);
        let key = [
            p[0].to_bits(),
            p[1].to_bits(),
            p[2].to_bits(),
            nn[0].to_bits(),
            nn[1].to_bits(),
            nn[2].to_bits(),
            tt[0].to_bits(),
            tt[1].to_bits(),
        ];
        let idx = *self.map.entry(key).or_insert_with(|| {
            self.positions.push(p);
            if n.is_some() {
                self.normals.push(nn);
            }
            if uv.is_some() {
                self.uvs.push(tt);
            }
            (self.positions.len() - 1) as u32
        });
        self.indices.push(idx);
    }

    pub fn into_prim(self, name: String, material: Option<usize>) -> MeshPrim {
        let n = self.positions.len();
        MeshPrim {
            lod: crate::lod::parse_lod_suffix(&name),
            name,
            material,
            normals: if self.normals.len() == n { self.normals } else { vec![] },
            uvs: if self.uvs.len() == n { self.uvs } else { vec![] },
            positions: self.positions,
            indices: self.indices,
        }
    }
}
