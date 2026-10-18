//! FBX import through ufbx.

use crate::scene::{Material, Scene, TexRef, TexSource, VertexDedup};
use anyhow::{anyhow, Result};
use hangar_core::TextureSlot;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub fn load(path: &Path, base_dir: &str) -> Result<Scene> {
    let opts = ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes {
            right: ufbx::CoordinateAxis::PositiveX,
            up: ufbx::CoordinateAxis::PositiveY,
            front: ufbx::CoordinateAxis::PositiveZ,
        },
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::ModifyGeometry,
        generate_missing_normals: true,
        load_external_files: false,
        ignore_missing_external_files: true,
        ignore_animation: false,
        evaluate_skinning: false,
        ..Default::default()
    };
    let path_str = path.to_str().ok_or_else(|| anyhow!("non utf-8 path"))?;
    let scene = ufbx::load_file(path_str, opts).map_err(|e| anyhow!("ufbx: {e:?}"))?;

    let mut out = Scene { importer: "ufbx", ..Default::default() };
    out.animations = scene.anim_stacks.len() as u32;
    out.has_skin = !scene.skin_deformers.is_empty();

    // Materials: map ufbx element id -> our index.
    let mut mat_index: HashMap<u32, usize> = HashMap::new();
    for m in &scene.materials {
        mat_index.insert(m.element.element_id, out.materials.len());
        out.materials.push(convert_material(m, base_dir));
    }

    let mut any_normals = false;
    let mut tri_buf: Vec<u32> = Vec::new();
    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else { continue };
        let mesh: &ufbx::Mesh = mesh;
        let lname = node.element.name.to_ascii_lowercase();
        if lname.starts_with("ucx_") || lname.starts_with("ubx_") || lname.starts_with("usp_") || lname.starts_with("ucp_") {
            continue; // Unreal collision primitives
        }
        let world = node.geometry_to_world;
        let normal_m = ufbx::matrix_for_normals(&world);
        let det = det3(&world);
        let flip = det < 0.0;
        let has_n = mesh.vertex_normal.exists;
        let has_uv = mesh.vertex_uv.exists;
        any_normals |= has_n && !mesh.generated_normals;
        let materials: Vec<Option<usize>> = if !node.materials.is_empty() {
            node.materials.iter().map(|m| mat_index.get(&m.element.element_id).copied()).collect()
        } else {
            mesh.materials.iter().map(|m| mat_index.get(&m.element.element_id).copied()).collect()
        };
        let name = if node.element.name.is_empty() { mesh.element.name.to_string() } else { node.element.name.to_string() };

        let parts: Vec<(Option<usize>, Vec<u32>)> = if mesh.material_parts.is_empty() {
            vec![(materials.first().copied().flatten(), (0..mesh.faces.len() as u32).collect())]
        } else {
            mesh.material_parts
                .iter()
                .map(|p| (materials.get(p.index as usize).copied().flatten(), p.face_indices.to_vec()))
                .collect()
        };

        for (mat, faces) in parts {
            let mut dd = VertexDedup::default();
            for fi in faces {
                let Some(face) = mesh.faces.get(fi as usize).copied() else { continue };
                if face.num_indices < 3 {
                    continue;
                }
                tri_buf.resize(((face.num_indices - 2) * 3) as usize, 0);
                let n = ufbx::triangulate_face(&mut tri_buf, mesh, face) as usize;
                for t in 0..n {
                    let tri = [tri_buf[t * 3], tri_buf[t * 3 + 1], tri_buf[t * 3 + 2]];
                    let order: [usize; 3] = if flip { [0, 2, 1] } else { [0, 1, 2] };
                    for &k in &order {
                        let ix = tri[k] as usize;
                        let p = mesh.vertex_position[ix];
                        let wp = ufbx::transform_position(&world, p);
                        let nn = has_n.then(|| {
                            let nv = mesh.vertex_normal[ix];
                            let wn = ufbx::transform_direction(&normal_m, nv);
                            normalize([wn.x as f32, wn.y as f32, wn.z as f32])
                        });
                        let uv = has_uv.then(|| {
                            let u = mesh.vertex_uv[ix];
                            [u.x as f32, 1.0 - u.y as f32]
                        });
                        dd.push([wp.x as f32, wp.y as f32, wp.z as f32], nn, uv);
                    }
                }
            }
            if !dd.indices.is_empty() {
                out.meshes.push(dd.into_prim(name.clone(), mat));
            }
        }
    }
    out.source_had_normals = any_normals;
    if out.meshes.is_empty() {
        return Err(anyhow!("FBX contains no mesh geometry"));
    }
    Ok(out)
}

fn det3(m: &ufbx::Matrix) -> f64 {
    m.m00 * (m.m11 * m.m22 - m.m21 * m.m12) - m.m01 * (m.m10 * m.m22 - m.m20 * m.m12)
        + m.m02 * (m.m10 * m.m21 - m.m20 * m.m11)
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 1.0, 0.0] }
}

fn tex_source(t: &ufbx::Texture, base_dir: &str) -> Option<TexSource> {
    // Prefer embedded content, then the relative filename, then the absolute one.
    let content: &[u8] = &t.content;
    let candidates: [&str; 3] = [&t.relative_filename, &t.filename, &t.absolute_filename];
    let name = candidates.into_iter().find(|s| !s.is_empty()).map(|s| s.to_string());
    if !content.is_empty() {
        return Some(TexSource::Embedded {
            name: name.unwrap_or_else(|| t.element.name.to_string()),
            bytes: Arc::new(content.to_vec()),
        });
    }
    if let Some(video) = t.video.as_ref() {
        let vc: &[u8] = &video.content;
        if !vc.is_empty() {
            return Some(TexSource::Embedded {
                name: name.clone().unwrap_or_else(|| video.element.name.to_string()),
                bytes: Arc::new(vc.to_vec()),
            });
        }
    }
    // Layered textures: use the first file texture.
    if name.is_none()
        && let Some(ft) = t.file_textures.iter().next()
        && !std::ptr::eq(&**ft, t)
    {
        return tex_source(ft, base_dir);
    }
    name.map(|original| TexSource::Path { original, base_dir: base_dir.to_string() })
}

fn convert_material(m: &ufbx::Material, base_dir: &str) -> Material {
    let mut out = Material::named(m.element.name.to_string());
    let pbr = &m.pbr;
    let bc = pbr.base_color.value_vec4;
    let bf = if pbr.base_factor.has_value { pbr.base_factor.value_vec4.x } else { 1.0 };
    let opacity = if pbr.opacity.has_value { pbr.opacity.value_vec4.x } else { 1.0 };
    if pbr.base_color.has_value {
        out.base_color = [
            (bc.x * bf).clamp(0.0, 1.0) as f32,
            (bc.y * bf).clamp(0.0, 1.0) as f32,
            (bc.z * bf).clamp(0.0, 1.0) as f32,
            opacity.clamp(0.0, 1.0) as f32,
        ];
    }
    if pbr.roughness.has_value {
        out.roughness = pbr.roughness.value_vec4.x.clamp(0.04, 1.0) as f32;
    }
    if pbr.metalness.has_value {
        out.metallic = pbr.metalness.value_vec4.x.clamp(0.0, 1.0) as f32;
    }
    if pbr.emission_color.has_value {
        let ef = if pbr.emission_factor.has_value { pbr.emission_factor.value_vec4.x } else { 1.0 };
        let e = pbr.emission_color.value_vec4;
        out.emissive = [(e.x * ef) as f32, (e.y * ef) as f32, (e.z * ef) as f32];
    }
    out.double_sided = m.features.double_sided.enabled;

    let mut push = |slot: TextureSlot, map: &ufbx::MaterialMap| {
        if let Some(t) = map.texture.as_ref()
            && let Some(src) = tex_source(t, base_dir)
            && !out.textures.iter().any(|x: &TexRef| x.slot == slot)
        {
            out.textures.push(TexRef { slot, source: src });
        }
    };
    push(TextureSlot::BaseColor, &pbr.base_color);
    push(TextureSlot::BaseColor, &m.fbx.diffuse_color);
    push(TextureSlot::Normal, &pbr.normal_map);
    push(TextureSlot::Normal, &m.fbx.normal_map);
    push(TextureSlot::Roughness, &pbr.roughness);
    push(TextureSlot::Metallic, &pbr.metalness);
    push(TextureSlot::Occlusion, &pbr.ambient_occlusion);
    push(TextureSlot::Emissive, &pbr.emission_color);
    push(TextureSlot::Emissive, &m.fbx.emission_color);
    push(TextureSlot::Opacity, &pbr.opacity);

    // The "Bump" slot very often holds a normal map in exported FBX files.
    if let Some(t) = m.fbx.bump.texture.as_ref()
        && let Some(src) = tex_source(t, base_dir)
    {
        let slot = crate::textures::detect_slot(src.display()).unwrap_or(TextureSlot::Height);
        let slot = if slot == TextureSlot::Normal || slot == TextureSlot::Height { slot } else { TextureSlot::Height };
        if slot == TextureSlot::Normal && !out.textures.iter().any(|x| x.slot == TextureSlot::Normal) {
            out.textures.push(TexRef { slot, source: src });
        }
    }

    // A texture whose file name says otherwise wins over the FBX slot:
    // e.g. an ORM map plugged into "roughness".
    for t in &mut out.textures {
        if let Some(named) = crate::textures::detect_slot(t.source.display())
            && named == TextureSlot::Orm
            && matches!(t.slot, TextureSlot::Roughness | TextureSlot::Metallic | TextureSlot::Occlusion)
        {
            t.slot = TextureSlot::Orm;
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.textures.retain(|t| seen.insert(t.slot));
    out
}
