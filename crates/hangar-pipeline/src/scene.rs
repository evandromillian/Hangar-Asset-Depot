//! Intermediate representation shared by all importers: a flat list of
//! world-space triangle meshes plus PBR-ish materials, and — for animated
//! sources — the node hierarchy, skins and baked clips ([`Rig`]).

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
    /// Present only when the source has animation that actually moves something.
    pub rig: Option<Rig>,
    /// Separate animation files: their named nodes found in this model / all of them.
    pub bones_matched: u32,
    pub bones_total: u32,
}

/// Node hierarchy, skins and baked animation clips of an animated source.
/// Meshes stay in world space (rest pose); the GLB writer moves rigid meshes
/// into their node's space and skins the others.
#[derive(Debug, Clone, Default)]
pub struct Rig {
    pub nodes: Vec<SceneNode>,
    pub skins: Vec<Skin>,
    pub clips: Vec<AnimClip>,
}

#[derive(Debug, Clone)]
pub struct SceneNode {
    pub name: String,
    pub parent: Option<usize>,
    /// Rest local transform; rotation is a quaternion (x, y, z, w).
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Rest node-to-world matrix and its inverse.
    pub world: glam::Mat4,
    pub world_inv: glam::Mat4,
}

#[derive(Debug, Clone)]
pub struct Skin {
    pub name: String,
    /// Indices into [`Rig::nodes`]; `MeshPrim::joints` index this list.
    pub joints: Vec<usize>,
    /// Per joint: world-space rest vertex → joint space at bind time.
    pub inverse_bind: Vec<glam::Mat4>,
}

#[derive(Debug, Clone)]
pub struct AnimClip {
    pub name: String,
    pub duration: f32,
    pub fps: f32,
    pub channels: Vec<NodeChannel>,
    /// Version path of the separate animation file the clip came from.
    pub source: Option<String>,
}

/// Baked local-transform keys of one node; an empty list means "not animated".
/// Times are seconds from the start of the clip.
#[derive(Debug, Clone, Default)]
pub struct NodeChannel {
    pub node: usize,
    pub translation: Vec<(f32, [f32; 3])>,
    pub rotation: Vec<(f32, [f32; 4])>,
    pub scale: Vec<(f32, [f32; 3])>,
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
    /// Rigid animation: the [`Rig`] node this primitive moves with.
    pub node: Option<usize>,
    /// Skinning: index into [`Rig::skins`], with up to four influences per
    /// vertex (`joints` / `weights` are empty or the length of `positions`).
    pub skin: Option<usize>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
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
    pub alpha: AlphaMode,
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
            alpha: AlphaMode::Auto,
            textures: vec![],
        }
    }
}

/// How the source file says the material's alpha is used.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AlphaMode {
    /// The file doesn't say (OBJ, textures matched by name): an alpha channel in
    /// the base color texture is taken as a cutout.
    #[default]
    Auto,
    /// The base color texture's alpha channel is not opacity (e.g. a specular mask
    /// the material doesn't wire to it): only `base_color[3]` counts.
    IgnoreTexture,
    /// Cutout at this threshold.
    Mask(f32),
    Blend,
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
    map: std::collections::HashMap<[u32; 9], u32>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl VertexDedup {
    pub fn push(&mut self, p: [f32; 3], n: Option<[f32; 3]>, uv: Option<[f32; 2]>) {
        self.push_skinned(p, n, uv, None);
    }

    /// Like [`push`](Self::push); `skin` is (source vertex id, joints, weights).
    /// The vertex id keeps corners with different influences apart.
    pub fn push_skinned(
        &mut self,
        p: [f32; 3],
        n: Option<[f32; 3]>,
        uv: Option<[f32; 2]>,
        skin: Option<(u32, [u16; 4], [f32; 4])>,
    ) {
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
            skin.map(|s| s.0).unwrap_or(u32::MAX),
        ];
        let idx = *self.map.entry(key).or_insert_with(|| {
            self.positions.push(p);
            if n.is_some() {
                self.normals.push(nn);
            }
            if uv.is_some() {
                self.uvs.push(tt);
            }
            if let Some((_, j, w)) = skin {
                self.joints.push(j);
                self.weights.push(w);
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
            joints: if self.joints.len() == n { self.joints } else { vec![] },
            weights: if self.weights.len() == n { self.weights } else { vec![] },
            positions: self.positions,
            indices: self.indices,
            node: None,
            skin: None,
        }
    }
}
