//! Minimal glTF 2.0 binary (GLB) writer.

use crate::scene::{Aabb, MeshPrim};
use serde_json::{json, Value};

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

/// Serialise meshes + materials + images into a GLB.
pub fn write_glb(meshes: &[MeshPrim], materials: &[GlbMaterial], images: &[EncodedImage]) -> Vec<u8> {
    const ARRAY_BUFFER: u32 = 34962;
    const ELEMENT_ARRAY_BUFFER: u32 = 34963;
    const FLOAT: u32 = 5126;
    const U16: u32 = 5123;
    const U32: u32 = 5125;

    let mut b = Builder { bin: Vec::new(), views: vec![], accessors: vec![] };
    let mut gl_meshes = Vec::new();
    let mut nodes = Vec::new();

    for (i, m) in meshes.iter().enumerate() {
        if m.indices.is_empty() || m.positions.is_empty() {
            continue;
        }
        let bounds = {
            let mut a = Aabb::empty();
            for p in &m.positions {
                a.extend(*p);
            }
            a
        };
        let pv = b.view(&f32_bytes(&m.positions), Some(ARRAY_BUFFER));
        let pa = b.accessor(pv, FLOAT, m.positions.len(), "VEC3", Some((bounds.min.to_vec(), bounds.max.to_vec())));
        let mut attrs = json!({ "POSITION": pa });
        if m.normals.len() == m.positions.len() {
            let nv = b.view(&f32_bytes(&m.normals), Some(ARRAY_BUFFER));
            attrs["NORMAL"] = json!(b.accessor(nv, FLOAT, m.normals.len(), "VEC3", None));
        }
        if m.uvs.len() == m.positions.len() {
            let uv = b.view(&f32_bytes(&m.uvs), Some(ARRAY_BUFFER));
            attrs["TEXCOORD_0"] = json!(b.accessor(uv, FLOAT, m.uvs.len(), "VEC2", None));
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
        nodes.push(json!({ "name": name, "mesh": gl_meshes.len() - 1 }));
    }

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
    let node_ids: Vec<usize> = (0..nodes.len()).collect();
    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "hangar-pipeline" },
        "scene": 0,
        "scenes": [{ "name": "Scene", "nodes": node_ids }],
        "nodes": nodes,
        "meshes": gl_meshes,
        "accessors": b.accessors,
        "bufferViews": b.views,
        "buffers": [{ "byteLength": b.bin.len() }],
    });
    if !gl_materials.is_empty() {
        doc["materials"] = json!(gl_materials);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_glb() {
        let m = MeshPrim {
            name: "tri".into(),
            material: Some(0),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            lod: None,
        };
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let png = crate::images::encode_png(&img);
        let mut mat = GlbMaterial::new("M_Body");
        mat.base_color_tex = Some(0);
        let glb = write_glb(&[m], &[mat], &[EncodedImage { name: "bc".into(), bytes: png, mime: "image/png" }]);
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
