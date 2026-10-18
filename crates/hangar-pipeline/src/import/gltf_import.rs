//! glTF 2.0 / GLB import through the `gltf` crate.
//!
//! Images are NOT decoded here: external URIs become texture paths (so they
//! can be resolved against the version tree and take part in variant
//! detection) and embedded images keep their encoded bytes.
//!
//! Files with animations keep their node hierarchy, skins and clips as a
//! [`Rig`] (same model as the FBX importer); meshes stay in world space.

use crate::scene::{AnimClip, Material, MeshPrim, NodeChannel, Rig, Scene, SceneNode, Skin, TexRef, TexSource};
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
        let occ = m.occlusion_texture().map(|t| t.texture());
        let mut roughness = None;
        // KHR_materials_pbrSpecularGlossiness (older exporters, e.g. Microsoft's)
        // supersedes the metallic-roughness block when present.
        if let Some(sg) = m.pbr_specular_glossiness() {
            mat.base_color = sg.diffuse_factor();
            mat.metallic = 0.0;
            mat.roughness = (1.0 - sg.glossiness_factor()).clamp(0.04, 1.0);
            push(TextureSlot::BaseColor, sg.diffuse_texture().map(|t| t.texture()));
            push(TextureSlot::Occlusion, occ);
            let gloss = sg.specular_glossiness_texture().and_then(|t| image_sources.get(t.texture().source().index()));
            if let Some(src) = gloss {
                match roughness_from_gloss(src, parent, sg.glossiness_factor()) {
                    Ok(bytes) => {
                        let name = format!("{}_roughness", src.display());
                        roughness = Some(TexRef { slot: TextureSlot::Roughness, source: TexSource::Embedded { name, bytes: Arc::new(bytes) } });
                    }
                    Err(e) => out.warnings.push(format!("material {}: glossiness map skipped: {e:#}", mat.name)),
                }
            }
        } else {
            push(TextureSlot::BaseColor, pbr.base_color_texture().map(|t| t.texture()));
            let mr = pbr.metallic_roughness_texture().map(|t| t.texture());
            let packed = matches!((&mr, &occ), (Some(a), Some(b)) if a.source().index() == b.source().index());
            if packed {
                push(TextureSlot::Orm, mr);
            } else {
                push(TextureSlot::MetallicRoughness, mr);
                push(TextureSlot::Occlusion, occ);
            }
        }
        push(TextureSlot::Normal, m.normal_texture().map(|t| t.texture()));
        push(TextureSlot::Emissive, m.emissive_texture().map(|t| t.texture()));
        mat.textures.extend(roughness);
        out.materials.push(mat);
    }
    let default_material = out.materials.len();
    let mut need_default = false;

    // Node hierarchy: every node's rest local and world matrix.
    let node_count = doc.nodes().count();
    let mut parent: Vec<Option<usize>> = vec![None; node_count];
    for n in doc.nodes() {
        for c in n.children() {
            parent[c.index()] = Some(n.index());
        }
    }
    let local: Vec<Mat4> = doc.nodes().map(|n| Mat4::from_cols_array_2d(&n.transform().matrix())).collect();
    let mut world: Vec<Option<Mat4>> = vec![None; node_count];
    fn resolve(i: usize, parent: &[Option<usize>], local: &[Mat4], world: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let m = match parent[i].filter(|&p| p != i && depth < parent.len()) {
            Some(p) => resolve(p, parent, local, world, depth + 1) * local[i],
            None => local[i],
        };
        world[i] = Some(m);
        m
    }
    let world: Vec<Mat4> = (0..node_count).map(|i| resolve(i, &parent, &local, &mut world, 0)).collect();

    // Animated files keep the hierarchy (like FBX): rigid parts follow their
    // node, skinned parts their joints.
    let clips = read_clips(doc, &buffers, &mut out.warnings);
    let mut rig = (!clips.is_empty()).then(|| Rig {
        nodes: doc
            .nodes()
            .map(|n| {
                let (t, r, s) = n.transform().decomposed();
                SceneNode {
                    name: n.name().map(str::to_string).unwrap_or_else(|| format!("node_{}", n.index())),
                    parent: parent[n.index()],
                    translation: t,
                    rotation: r,
                    scale: s,
                    world: world[n.index()],
                    world_inv: world[n.index()].inverse(),
                }
            })
            .collect(),
        skins: vec![],
        clips,
    });
    out.animations = rig.as_ref().map_or(0, |r| r.clips.len() as u32);
    // Inverse bind matrices per glTF skin.
    let ibms: Vec<Vec<Mat4>> = doc
        .skins()
        .map(|s| {
            let r = s.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            match r.read_inverse_bind_matrices() {
                Some(it) => it.map(|m| Mat4::from_cols_array_2d(&m)).collect(),
                None => vec![Mat4::IDENTITY; s.joints().count()],
            }
        })
        .collect();

    let scene = doc.default_scene().or_else(|| doc.scenes().next()).ok_or_else(|| anyhow!("glTF has no scene"))?;
    let mut stack: Vec<gltf::Node> = scene.nodes().collect();
    let mut any_normals = false;
    while let Some(node) = stack.pop() {
        stack.extend(node.children());
        let Some(mesh) = node.mesh() else { continue };
        let name = node.name().or(mesh.name()).map(|s| s.to_string()).unwrap_or_else(|| format!("mesh_{}", mesh.index()));
        let prims: Vec<gltf::Primitive> = mesh.primitives().filter(|p| p.mode() == gltf::mesh::Mode::Triangles).collect();
        // Skinned meshes ignore their node: vertices are in bind space, which
        // joint world × inverse bind maps to the world. That bind matrix is the
        // same for every joint when the rest pose is the bind pose; the joint
        // weighing most on the mesh decides otherwise.
        let gl_skin = node.skin().filter(|s| s.joints().count() > 0);
        let bind = gl_skin.as_ref().map(|s| {
            let joints: Vec<usize> = s.joints().map(|j| j.index()).collect();
            let mut weight = vec![0f32; joints.len()];
            for p in &prims {
                let r = p.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                if let (Some(j), Some(w)) = (r.read_joints(0), r.read_weights(0)) {
                    for (j, w) in j.into_u16().zip(w.into_f32()) {
                        for k in 0..4 {
                            if let Some(slot) = weight.get_mut(j[k] as usize) {
                                *slot += w[k];
                            }
                        }
                    }
                }
            }
            let main = (0..joints.len()).max_by(|a, b| weight[*a].total_cmp(&weight[*b])).unwrap_or(0);
            let ibm = ibms[s.index()].get(main).copied().unwrap_or(Mat4::IDENTITY);
            let bind = world[joints[main]] * ibm;
            (joints, bind)
        });
        let to_world = bind.as_ref().map_or(world[node.index()], |(_, b)| *b);
        let skin_index = match (&bind, gl_skin.as_ref(), rig.as_mut()) {
            (Some((joints, b)), Some(s), Some(rig)) => {
                let from_world = b.inverse();
                let inverse_bind =
                    (0..joints.len()).map(|k| ibms[s.index()].get(k).copied().unwrap_or(Mat4::IDENTITY) * from_world).collect();
                let name = s.name().map(str::to_string).unwrap_or_else(|| format!("skin_{}", s.index()));
                rig.skins.push(Skin { name, joints: joints.clone(), inverse_bind });
                Some(rig.skins.len() - 1)
            }
            _ => None,
        };
        let normal_m = Mat3::from_mat4(to_world).inverse().transpose();
        let flip = Mat3::from_mat4(to_world).determinant() < 0.0;
        for prim in &prims {
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(pos) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = pos.map(|p| to_world.transform_point3(Vec3::from(p)).into()).collect();
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
            let (mut joints, mut weights) = (vec![], vec![]);
            if skin_index.is_some()
                && let (Some(j), Some(w)) = (reader.read_joints(0), reader.read_weights(0))
            {
                joints = j.into_u16().collect();
                weights = w.into_f32().map(normalize_weights).collect();
            }
            let skinned = skin_index.is_some() && joints.len() == positions.len() && weights.len() == positions.len();
            out.meshes.push(MeshPrim {
                lod: crate::lod::parse_lod_suffix(&name),
                name: name.clone(),
                material,
                normals: if normals.len() == positions.len() { normals } else { vec![] },
                uvs: if uvs.len() == positions.len() { uvs } else { vec![] },
                positions,
                indices,
                node: (rig.is_some() && !skinned).then_some(node.index()),
                skin: if skinned { skin_index } else { None },
                joints: if skinned { joints } else { vec![] },
                weights: if skinned { weights } else { vec![] },
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
    out.rig = rig;
    Ok(out)
}

fn normalize_weights(w: [f32; 4]) -> [f32; 4] {
    let sum: f32 = w.iter().sum();
    if sum > 0.0 { w.map(|x| x / sum) } else { [1.0, 0.0, 0.0, 0.0] }
}

/// The file's animations as baked clips. LINEAR keys are taken as they are,
/// STEP keys get a hold key just before each change, CUBICSPLINE is sampled at
/// 60 Hz (the GLB writer only writes LINEAR). Times start at the clip's first
/// key. Morph target weights are not exported.
fn read_clips(doc: &gltf::Document, buffers: &[gltf::buffer::Data], warnings: &mut Vec<String>) -> Vec<AnimClip> {
    use gltf::animation::util::ReadOutputs;
    let mut clips = Vec::new();
    for (i, anim) in doc.animations().enumerate() {
        let mut channels: Vec<NodeChannel> = Vec::new();
        let mut deltas: Vec<f32> = Vec::new();
        let mut morph = false;
        for ch in anim.channels() {
            let node = ch.target().node().index();
            let reader = ch.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(times) = reader.read_inputs().map(|t| t.collect::<Vec<f32>>()) else { continue };
            deltas.extend(times.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0));
            let interp = ch.sampler().interpolation();
            let idx = match channels.iter().position(|c| c.node == node) {
                Some(p) => p,
                None => {
                    channels.push(NodeChannel { node, ..Default::default() });
                    channels.len() - 1
                }
            };
            let c = &mut channels[idx];
            match reader.read_outputs() {
                Some(ReadOutputs::Translations(v)) => c.translation = to_linear(&times, &v.collect::<Vec<_>>(), interp, false),
                Some(ReadOutputs::Rotations(v)) => c.rotation = to_linear(&times, &v.into_f32().collect::<Vec<_>>(), interp, true),
                Some(ReadOutputs::Scales(v)) => c.scale = to_linear(&times, &v.collect::<Vec<_>>(), interp, false),
                Some(ReadOutputs::MorphTargetWeights(_)) => morph = true,
                None => {}
            }
        }
        channels.retain(|c| !(c.translation.is_empty() && c.rotation.is_empty() && c.scale.is_empty()));
        let name = anim.name().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or(format!("Animação {}", i + 1));
        if morph {
            warnings.push(format!("animation '{name}': morph target weights are not exported"));
        }
        if channels.is_empty() {
            continue;
        }
        let all_times = || {
            channels.iter().flat_map(|c| {
                c.translation.iter().map(|k| k.0).chain(c.rotation.iter().map(|k| k.0)).chain(c.scale.iter().map(|k| k.0))
            })
        };
        let start = all_times().fold(f32::INFINITY, f32::min);
        let end = all_times().fold(f32::NEG_INFINITY, f32::max);
        for c in &mut channels {
            c.translation.iter_mut().for_each(|k| k.0 -= start);
            c.rotation.iter_mut().for_each(|k| k.0 -= start);
            c.scale.iter_mut().for_each(|k| k.0 -= start);
        }
        // glTF has no frame rate: dense (baked) keys tell it, sparse ones don't.
        deltas.sort_by(f32::total_cmp);
        let fps = match deltas.get(deltas.len() / 2).map(|d| 1.0 / d) {
            Some(f) if f >= 20.0 => f.round().min(120.0),
            _ => 30.0,
        };
        clips.push(AnimClip { name, duration: (end - start).max(0.0), fps, channels, source: None });
    }
    crate::import::fbx::unique_names(&mut clips);
    clips
}

fn to_linear<const N: usize>(
    times: &[f32],
    values: &[[f32; N]],
    interp: gltf::animation::Interpolation,
    rotation: bool,
) -> Vec<(f32, [f32; N])> {
    use gltf::animation::Interpolation;
    let n = times.len();
    match interp {
        Interpolation::Linear => times.iter().copied().zip(values.iter().copied()).collect(),
        Interpolation::Step => {
            let mut out = Vec::with_capacity(n * 2);
            for i in 0..n.min(values.len()) {
                if i > 0 {
                    let hold = (times[i] - times[i - 1]) * 0.5;
                    out.push((times[i] - hold.min(1e-3), values[i - 1]));
                }
                out.push((times[i], values[i]));
            }
            out
        }
        Interpolation::CubicSpline => {
            // Outputs are (in-tangent, value, out-tangent) per key.
            if values.len() < n * 3 {
                return vec![];
            }
            let key = |i: usize| (values[i * 3], values[i * 3 + 1], values[i * 3 + 2]);
            let mut out = Vec::new();
            for i in 0..n {
                let (_, v0, b0) = key(i);
                out.push((times[i], v0));
                let Some(&t1) = times.get(i + 1) else { break };
                let (a1, v1, _) = key(i + 1);
                let d = t1 - times[i];
                let steps = (d * 60.0).ceil().max(1.0) as usize;
                for s in 1..steps {
                    let u = s as f32 / steps as f32;
                    let (u2, u3) = (u * u, u * u * u);
                    let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
                    let mut v = [0f32; N];
                    for c in 0..N {
                        v[c] = h00 * v0[c] + h10 * d * b0[c] + h01 * v1[c] + h11 * d * a1[c];
                    }
                    if rotation {
                        let l = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                        if l > 0.0 {
                            v.iter_mut().for_each(|x| *x /= l);
                        }
                    }
                    out.push((times[i] + d * u, v));
                }
            }
            out
        }
    }
}

/// Roughness map (PNG) from a specular-glossiness texture: roughness = 1 −
/// glossiness, glossiness being its alpha channel times `factor`.
fn roughness_from_gloss(src: &TexSource, dir: &Path, factor: f32) -> Result<Vec<u8>> {
    let img = match src {
        TexSource::Embedded { bytes, .. } => image::load_from_memory(bytes)?,
        TexSource::Path { original, .. } => image::open(dir.join(original))?,
    };
    let mut rgba = img.to_rgba8();
    for p in rgba.pixels_mut() {
        let rough = (1.0 - p[3] as f32 / 255.0 * factor).clamp(0.04, 1.0);
        let v = (rough * 255.0).round() as u8;
        *p = image::Rgba([v, v, v, 255]);
    }
    Ok(crate::images::encode_png(&rgba))
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
