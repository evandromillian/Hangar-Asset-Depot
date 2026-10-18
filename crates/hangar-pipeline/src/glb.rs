//! Minimal glTF 2.0 binary (GLB) writer.

use crate::scene::{Aabb, MeshPrim, Rig, SceneNode};
use serde_json::{json, Value};
use std::borrow::Cow;

pub struct EncodedImage {
    pub name: String,
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

#[derive(Debug, Clone)]
pub struct GlbMaterial {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub double_sided: bool,
    /// "OPAQUE" | "MASK" | "BLEND"
    pub alpha_mode: &'static str,
    pub base_color_tex: Option<usize>,
    pub normal_tex: Option<usize>,
    pub metallic_roughness_tex: Option<usize>,
    pub occlusion_tex: Option<usize>,
    pub emissive_tex: Option<usize>,
}

impl GlbMaterial {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.8,
            emissive: [0.0; 3],
            double_sided: false,
            alpha_mode: "OPAQUE",
            base_color_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            occlusion_tex: None,
            emissive_tex: None,
        }
    }
}

struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({ "buffer": 0, "byteOffset": offset, "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, ty: &str, minmax: Option<(Vec<f32>, Vec<f32>)>) -> usize {
        let mut a = json!({ "bufferView": view, "componentType": component, "count": count, "type": ty });
        if let Some((min, max)) = minmax {
            a["min"] = json!(min);
            a["max"] = json!(max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
}

fn f32_bytes<const N: usize>(data: &[[f32; N]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * N * 4);
    for v in data {
        for c in v {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    out
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const U16: u32 = 5123;
const U32: u32 = 5125;

/// Serialise meshes + materials + images into a GLB. With a `rig`, the node
/// hierarchy, skins and animation clips are written too (all under one root
/// node, so viewers find a single animation root).
pub fn write_glb(meshes: &[MeshPrim], materials: &[GlbMaterial], images: &[EncodedImage], rig: Option<&Rig>) -> Vec<u8> {
    let mut b = Builder { bin: Vec::new(), views: vec![], accessors: vec![] };
    let mut gl_meshes = Vec::new();
    let mut nodes = Vec::new();
    // Per written mesh: (node json, rig node it hangs from).
    let mut mesh_nodes: Vec<(Value, Option<usize>)> = Vec::new();

    for (i, m) in meshes.iter().enumerate() {
        if m.indices.is_empty() || m.positions.is_empty() {
            continue;
        }
        // Rigid parts of an animated model live in their node's space.
        let rig_node = rig.zip(m.node).and_then(|(r, n)| r.nodes.get(n).map(|node| (n, node)));
        let (positions, normals) = match rig_node {
            Some((_, node)) => {
                let (p, n) = localize(m, node);
                (Cow::Owned(p), Cow::Owned(n))
            }
            None => (Cow::Borrowed(&m.positions), Cow::Borrowed(&m.normals)),
        };
        let bounds = {
            let mut a = Aabb::empty();
            for p in positions.iter() {
                a.extend(*p);
            }
            a
        };
        let pv = b.view(&f32_bytes(&positions), Some(ARRAY_BUFFER));
        let pa = b.accessor(pv, FLOAT, positions.len(), "VEC3", Some((bounds.min.to_vec(), bounds.max.to_vec())));
        let mut attrs = json!({ "POSITION": pa });
        if normals.len() == positions.len() {
            let nv = b.view(&f32_bytes(&normals), Some(ARRAY_BUFFER));
            attrs["NORMAL"] = json!(b.accessor(nv, FLOAT, normals.len(), "VEC3", None));
        }
        if m.uvs.len() == m.positions.len() {
            let uv = b.view(&f32_bytes(&m.uvs), Some(ARRAY_BUFFER));
            attrs["TEXCOORD_0"] = json!(b.accessor(uv, FLOAT, m.uvs.len(), "VEC2", None));
        }
        let skin = m.skin.filter(|&s| {
            rig.is_some_and(|r| s < r.skins.len()) && m.joints.len() == m.positions.len() && m.weights.len() == m.positions.len()
        });
        if skin.is_some() {
            let joints: Vec<u8> = m.joints.iter().flatten().flat_map(|j| j.to_le_bytes()).collect();
            let jv = b.view(&joints, Some(ARRAY_BUFFER));
            attrs["JOINTS_0"] = json!(b.accessor(jv, U16, m.joints.len(), "VEC4", None));
            let wv = b.view(&f32_bytes(&m.weights), Some(ARRAY_BUFFER));
            attrs["WEIGHTS_0"] = json!(b.accessor(wv, FLOAT, m.weights.len(), "VEC4", None));
        }
        let (ibytes, ctype) = if m.positions.len() <= u16::MAX as usize {
            (m.indices.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect::<Vec<u8>>(), U16)
        } else {
            (m.indices.iter().flat_map(|&i| i.to_le_bytes()).collect::<Vec<u8>>(), U32)
        };
        let iv = b.view(&ibytes, Some(ELEMENT_ARRAY_BUFFER));
        let ia = b.accessor(iv, ctype, m.indices.len(), "SCALAR", None);
        let mut prim = json!({ "attributes": attrs, "indices": ia, "mode": 4 });
        if let Some(mat) = m.material.filter(|&mi| mi < materials.len()) {
            prim["material"] = json!(mat);
        }
        let name = if m.name.is_empty() { format!("mesh_{i}") } else { m.name.clone() };
        gl_meshes.push(json!({ "name": name, "primitives": [prim] }));
        let mut node = json!({ "name": name, "mesh": gl_meshes.len() - 1 });
        if let Some(s) = skin {
            node["skin"] = json!(s);
        }
        mesh_nodes.push((node, rig_node.map(|(n, _)| n)));
    }

    let mut skins = Vec::new();
    let mut animations = Vec::new();
    let root_nodes: Vec<usize> = match rig {
        None => {
            nodes.extend(mesh_nodes.into_iter().map(|(n, _)| n));
            (0..nodes.len()).collect()
        }
        Some(rig) => {
            // node 0: wrapper; 1..=N: rig nodes; then one node per mesh.
            let rig_index = |n: usize| n + 1;
            let mut children: Vec<Vec<usize>> = vec![vec![]; rig.nodes.len() + 1];
            for (i, n) in rig.nodes.iter().enumerate() {
                let parent = n.parent.filter(|&p| p < rig.nodes.len()).map(rig_index).unwrap_or(0);
                children[parent].push(rig_index(i));
            }
            let first_mesh = rig.nodes.len() + 1;
            for (k, (_, owner)) in mesh_nodes.iter().enumerate() {
                children[owner.map(rig_index).unwrap_or(0)].push(first_mesh + k);
            }
            nodes.push(json!({ "name": "hangar_root" }));
            for (i, n) in rig.nodes.iter().enumerate() {
                let name = if n.name.is_empty() { format!("node_{i}") } else { n.name.clone() };
                nodes.push(json!({ "name": name, "translation": n.translation, "rotation": n.rotation, "scale": n.scale }));
            }
            nodes.extend(mesh_nodes.into_iter().map(|(n, _)| n));
            for (i, c) in children.into_iter().enumerate() {
                if !c.is_empty() {
                    nodes[i]["children"] = json!(c);
                }
            }
            for s in &rig.skins {
                let ibm: Vec<[f32; 16]> = s.inverse_bind.iter().map(|m| m.to_cols_array()).collect();
                let v = b.view(&f32_bytes(&ibm), None);
                let a = b.accessor(v, FLOAT, ibm.len(), "MAT4", None);
                let joints: Vec<usize> = s.joints.iter().map(|&j| rig_index(j.min(rig.nodes.len() - 1))).collect();
                skins.push(json!({ "name": s.name, "joints": joints, "inverseBindMatrices": a }));
            }
            for clip in &rig.clips {
                let mut samplers = Vec::new();
                let mut channels = Vec::new();
                for ch in clip.channels.iter().filter(|c| c.node < rig.nodes.len()) {
                    let target = rig_index(ch.node);
                    let paths = [
                        ("translation", sampler(&mut b, &ch.translation)),
                        ("rotation", sampler(&mut b, &ch.rotation)),
                        ("scale", sampler(&mut b, &ch.scale)),
                    ];
                    for (path, s) in paths {
                        if let Some((input, output)) = s {
                            channels.push(json!({ "sampler": samplers.len(), "target": { "node": target, "path": path } }));
                            samplers.push(json!({ "input": input, "output": output, "interpolation": "LINEAR" }));
                        }
                    }
                }
                if !channels.is_empty() {
                    animations.push(json!({ "name": clip.name, "samplers": samplers, "channels": channels }));
                }
            }
            vec![0]
        }
    };

    let mut gl_images = Vec::new();
    let mut textures = Vec::new();
    for (i, img) in images.iter().enumerate() {
        let v = b.view(&img.bytes, None);
        gl_images.push(json!({ "name": img.name, "bufferView": v, "mimeType": img.mime }));
        textures.push(json!({ "sampler": 0, "source": i }));
    }

    let gl_materials: Vec<Value> = materials
        .iter()
        .map(|m| {
            let mut pbr = json!({
                "baseColorFactor": m.base_color,
                "metallicFactor": m.metallic,
                "roughnessFactor": m.roughness,
            });
            if let Some(t) = m.base_color_tex {
                pbr["baseColorTexture"] = json!({ "index": t });
            }
            if let Some(t) = m.metallic_roughness_tex {
                pbr["metallicRoughnessTexture"] = json!({ "index": t });
            }
            let mut mat = json!({
                "name": m.name,
                "pbrMetallicRoughness": pbr,
                "doubleSided": m.double_sided,
                "alphaMode": m.alpha_mode,
            });
            if m.alpha_mode == "MASK" {
                mat["alphaCutoff"] = json!(0.5);
            }
            if let Some(t) = m.normal_tex {
                mat["normalTexture"] = json!({ "index": t });
            }
            if let Some(t) = m.occlusion_tex {
                mat["occlusionTexture"] = json!({ "index": t });
            }
            if let Some(t) = m.emissive_tex {
                mat["emissiveTexture"] = json!({ "index": t });
                mat["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
            } else if m.emissive.iter().any(|&c| c > 0.0) {
                mat["emissiveFactor"] = json!(m.emissive);
            }
            mat
        })
        .collect();

    b.align();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "hangar-pipeline" },
        "scene": 0,
        "scenes": [{ "name": "Scene", "nodes": root_nodes }],
        "nodes": nodes,
        "meshes": gl_meshes,
        "accessors": b.accessors,
        "bufferViews": b.views,
        "buffers": [{ "byteLength": b.bin.len() }],
    });
    if !gl_materials.is_empty() {
        doc["materials"] = json!(gl_materials);
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    if !animations.is_empty() {
        doc["animations"] = json!(animations);
    }
    if !gl_images.is_empty() {
        doc["images"] = json!(gl_images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]);
    }

    let mut json_bytes = serde_json::to_vec(&doc).expect("gltf json");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b.bin);
    out
}

/// World-space rest geometry → the space of `node` (positions by the inverse
/// world matrix, normals by its transpose: the inverse of the normal matrix).
fn localize(m: &MeshPrim, node: &SceneNode) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let positions = m.positions.iter().map(|p| node.world_inv.transform_point3((*p).into()).into()).collect();
    let to_local = glam::Mat3::from_mat4(node.world).transpose();
    let normals = m.normals.iter().map(|n| (to_local * glam::Vec3::from(*n)).normalize_or_zero().into()).collect();
    (positions, normals)
}

/// Keys ready for a glTF sampler: repeated times keep the last value (glTF
/// needs strictly increasing input), and a channel that never changes becomes
/// its first and last key. Exporters such as Unreal key every bone on every
/// frame, so this keeps GLBs of big animation packages small. The channel is
/// kept (not dropped) so a clip still overrides what the previous one left.
fn clean_keys<const N: usize>(keys: &[(f32, [f32; N])]) -> Vec<(f32, [f32; N])> {
    let mut clean: Vec<(f32, [f32; N])> = Vec::with_capacity(keys.len());
    for &(t, v) in keys {
        match clean.last_mut() {
            Some(last) if t <= last.0 => last.1 = v,
            _ => clean.push((t, v)),
        }
    }
    if let (Some(&first), Some(&last)) = (clean.first(), clean.last())
        && clean.len() > 2
        && clean.iter().all(|k| k.1.iter().zip(&first.1).all(|(a, b)| (a - b).abs() <= 1e-6))
    {
        return vec![first, (last.0, first.1)];
    }
    clean
}

/// Writes one animation sampler (input times + output values); None for an
/// empty channel.
fn sampler<const N: usize>(b: &mut Builder, keys: &[(f32, [f32; N])]) -> Option<(usize, usize)> {
    let clean = clean_keys(keys);
    let (first, last) = (clean.first()?.0, clean.last()?.0);
    let times: Vec<[f32; 1]> = clean.iter().map(|k| [k.0]).collect();
    let values: Vec<[f32; N]> = clean.iter().map(|k| k.1).collect();
    let tv = b.view(&f32_bytes(&times), None);
    let input = b.accessor(tv, FLOAT, times.len(), "SCALAR", Some((vec![first], vec![last])));
    let vv = b.view(&f32_bytes(&values), None);
    let output = b.accessor(vv, FLOAT, values.len(), if N == 4 { "VEC4" } else { "VEC3" }, None);
    Some((input, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_channels_shrink_to_two_keys() {
        let still: Vec<(f32, [f32; 3])> = (0..30).map(|i| (i as f32 / 30.0, [0.0, 1.0, 0.0])).collect();
        assert_eq!(clean_keys(&still), vec![(0.0, [0.0, 1.0, 0.0]), (29.0 / 30.0, [0.0, 1.0, 0.0])]);
        let moving = [(0.0, [0.0; 3]), (0.5, [1.0, 0.0, 0.0]), (0.5, [2.0, 0.0, 0.0]), (1.0, [0.0; 3])];
        assert_eq!(clean_keys(&moving), vec![(0.0, [0.0; 3]), (0.5, [2.0, 0.0, 0.0]), (1.0, [0.0; 3])]);
    }

    #[test]
    fn writes_valid_glb() {
        let m = MeshPrim {
            name: "tri".into(),
            material: Some(0),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let png = crate::images::encode_png(&img);
        let mut mat = GlbMaterial::new("M_Body");
        mat.base_color_tex = Some(0);
        let glb = write_glb(&[m], &[mat], &[EncodedImage { name: "bc".into(), bytes: png, mime: "image/png" }], None);
        assert_eq!(glb.len() % 4, 0);
        let g = gltf::Gltf::from_slice(&glb).expect("valid glb");
        assert_eq!(g.materials().next().unwrap().name(), Some("M_Body"));
        let prim = g.meshes().next().unwrap().primitives().next().unwrap();
        assert_eq!(prim.get(&gltf::Semantic::Positions).unwrap().count(), 3);
        assert_eq!(g.images().count(), 1);
        let buffers = gltf::import_buffers(&g.document, None, g.blob.clone()).unwrap();
        let reader = prim.reader(|b| Some(&buffers[b.index()]));
        let idx: Vec<u32> = reader.read_indices().unwrap().into_u32().collect();
        assert_eq!(idx, vec![0, 1, 2]);
    }
}
