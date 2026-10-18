//! glTF 2.0 / GLB import through the `gltf` crate.
//!
//! Images are NOT decoded here: external URIs become texture paths (so they
//! can be resolved against the version tree and take part in variant
//! detection) and embedded images keep their encoded bytes.

use crate::scene::{Material, MeshPrim, Scene, TexRef, TexSource};
use anyhow::{anyhow, Context, Result};
use glam::{Mat3, Mat4, Vec3};
use hangar_core::TextureSlot;
use std::path::Path;
use std::sync::Arc;

pub fn load(path: &Path, base_dir: &str, importer: &'static str) -> Result<Scene> {
    let gltf = gltf::Gltf::open(path).with_context(|| format!("open {}", path.display()))?;
    let parent = path.parent().unwrap_or(Path::new("."));
    let buffers = gltf::import_buffers(&gltf.document, Some(parent), gltf.blob.clone())
        .map_err(|e| anyhow!("glTF buffers: {e}"))?;
    let doc = &gltf.document;

    let mut out = Scene { importer, ..Default::default() };
    out.has_skin = doc.skins().count() > 0;
    out.animations = doc.animations().count() as u32;

    // Images → texture sources
    let image_sources: Vec<TexSource> = doc
        .images()
        .map(|img| {
            let name = img.name().map(|s| s.to_string());
            match img.source() {
                gltf::image::Source::Uri { uri, .. } => {
                    if let Some(data) = uri.strip_prefix("data:") {
                        let bytes = data.split_once(',').map(|(_, b)| base64_decode(b)).unwrap_or_default();
                        TexSource::Embedded {
                            name: name.unwrap_or_else(|| format!("image_{}", img.index())),
                            bytes: Arc::new(bytes),
                        }
                    } else {
                        TexSource::Path { original: percent_decode(uri), base_dir: base_dir.to_string() }
                    }
                }
                gltf::image::Source::View { view, .. } => {
                    let buf = &buffers[view.buffer().index()];
                    let start = view.offset();
                    let end = (start + view.length()).min(buf.len());
                    TexSource::Embedded {
                        name: name.unwrap_or_else(|| format!("image_{}", img.index())),
                        bytes: Arc::new(buf[start.min(end)..end].to_vec()),
                    }
                }
            }
        })
        .collect();

    for (i, m) in doc.materials().enumerate() {
        let mut mat = Material::named(m.name().map(|s| s.to_string()).unwrap_or_else(|| format!("material_{i}")));
        let pbr = m.pbr_metallic_roughness();
        mat.base_color = pbr.base_color_factor();
        mat.metallic = pbr.metallic_factor();
        mat.roughness = pbr.roughness_factor();
        mat.emissive = m.emissive_factor();
        mat.double_sided = m.double_sided();
        let mut push = |slot: TextureSlot, tex: Option<gltf::Texture>| {
            if let Some(t) = tex
                && let Some(src) = image_sources.get(t.source().index())
            {
                mat.textures.push(TexRef { slot, source: src.clone() });
            }
        };
        push(TextureSlot::BaseColor, pbr.base_color_texture().map(|t| t.texture()));
        let mr = pbr.metallic_roughness_texture().map(|t| t.texture());
        let occ = m.occlusion_texture().map(|t| t.texture());
        let packed = matches!((&mr, &occ), (Some(a), Some(b)) if a.source().index() == b.source().index());
        if packed {
            push(TextureSlot::Orm, mr);
        } else {
            push(TextureSlot::MetallicRoughness, mr);
            push(TextureSlot::Occlusion, occ);
        }
        push(TextureSlot::Normal, m.normal_texture().map(|t| t.texture()));
        push(TextureSlot::Emissive, m.emissive_texture().map(|t| t.texture()));
        out.materials.push(mat);
    }
    let default_material = out.materials.len();
    let mut need_default = false;

    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or_else(|| anyhow!("glTF has no scene"))?;
    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    let mut any_normals = false;
    while let Some((node, parent_m)) = stack.pop() {
        let local = Mat4::from_cols_array_2d(&node.transform().matrix());
        let world = parent_m * local;
        for child in node.children() {
            stack.push((child, world));
        }
        let Some(mesh) = node.mesh() else { continue };
        let name = node.name().or(mesh.name()).map(|s| s.to_string()).unwrap_or_else(|| format!("mesh_{}", mesh.index()));
        let normal_m = Mat3::from_mat4(world).inverse().transpose();
        let flip = Mat3::from_mat4(world).determinant() < 0.0;
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(pos) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = pos.map(|p| world.transform_point3(Vec3::from(p)).into()).collect();
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|it| it.map(|n| (normal_m * Vec3::from(n)).normalize_or_zero().into()).collect())
                .unwrap_or_default();
            any_normals |= !normals.is_empty();
            let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|t| t.into_f32().collect()).unwrap_or_default();
            let mut indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            if flip {
                for t in indices.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            let material = match prim.material().index() {
                Some(i) => Some(i),
                None => {
                    need_default = true;
                    Some(default_material)
                }
            };
            out.meshes.push(MeshPrim {
                lod: crate::lod::parse_lod_suffix(&name),
                name: name.clone(),
                material,
                normals: if normals.len() == positions.len() { normals } else { vec![] },
                uvs: if uvs.len() == positions.len() { uvs } else { vec![] },
                positions,
                indices,
            });
        }
    }
    if need_default {
        out.materials.push(Material::named("default"));
    }
    out.source_had_normals = any_normals;
    if out.meshes.is_empty() {
        return Err(anyhow!("glTF contains no triangle meshes"));
    }
    Ok(out)
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 3 <= bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn base64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoders() {
        assert_eq!(percent_decode("Tex%20Folder/a%2Bb.png"), "Tex Folder/a+b.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(base64_decode("aGVsbG8="), b"hello");
        assert_eq!(base64_decode("aGk"), b"hi");
    }
}
