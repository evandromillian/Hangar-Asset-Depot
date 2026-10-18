//! Wavefront OBJ (+ MTL) import through tobj.

use crate::scene::{Material, MeshPrim, Scene, TexRef, TexSource};
use anyhow::{anyhow, Result};
use hangar_core::TextureSlot;
use std::path::Path;

pub fn load(path: &Path, base_dir: &str) -> Result<Scene> {
    let (models, materials) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS).map_err(|e| anyhow!("OBJ: {e}"))?;
    let mut out = Scene { importer: "obj", ..Default::default() };
    let materials = match materials {
        Ok(m) => m,
        Err(e) => {
            out.warnings.push(format!("MTL could not be loaded: {e}"));
            vec![]
        }
    };
    for m in &materials {
        out.materials.push(convert_material(m, base_dir));
    }
    let default_index = out.materials.len();
    let mut need_default = false;
    let mut any_normals = false;
    for model in models {
        let mesh = model.mesh;
        if mesh.indices.is_empty() {
            continue;
        }
        let positions: Vec<[f32; 3]> = mesh.positions.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let normals: Vec<[f32; 3]> = mesh.normals.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let uvs: Vec<[f32; 2]> = mesh.texcoords.chunks_exact(2).map(|c| [c[0], 1.0 - c[1]]).collect();
        any_normals |= !normals.is_empty();
        let material = match mesh.material_id.filter(|&i| i < default_index) {
            Some(i) => Some(i),
            None => {
                need_default = true;
                Some(default_index)
            }
        };
        out.meshes.push(MeshPrim {
            lod: crate::lod::parse_lod_suffix(&model.name),
            name: model.name,
            material,
            normals: if normals.len() == positions.len() { normals } else { vec![] },
            uvs: if uvs.len() == positions.len() { uvs } else { vec![] },
            positions,
            indices: mesh.indices,
            ..Default::default()
        });
    }
    if need_default {
        out.materials.push(Material::named("default"));
    }
    out.source_had_normals = any_normals;
    if out.meshes.is_empty() {
        return Err(anyhow!("OBJ contains no faces"));
    }
    Ok(out)
}

fn convert_material(m: &tobj::Material, base_dir: &str) -> Material {
    let mut out = Material::named(m.name.clone());
    if let Some(d) = m.diffuse {
        out.base_color = [d[0], d[1], d[2], m.dissolve.unwrap_or(1.0)];
    }
    if let Some(e) = m.emissive {
        out.emissive = e;
    }
    // Phong exponent → rough approximation of roughness.
    if let Some(ns) = m.shininess {
        out.roughness = (2.0 / (ns + 2.0)).sqrt().clamp(0.05, 1.0);
    }
    let param = |k: &str| m.unknown_param.get(k).or_else(|| m.unknown_param.get(&k.to_ascii_lowercase()));
    if let Some(v) = param("Pr").and_then(|v| v.trim().parse::<f32>().ok()) {
        out.roughness = v;
    }
    if let Some(v) = param("Pm").and_then(|v| v.trim().parse::<f32>().ok()) {
        out.metallic = v;
    }
    let mut push = |slot: TextureSlot, p: Option<&String>| {
        if let Some(p) = p.filter(|p| !p.trim().is_empty())
            && !out.textures.iter().any(|t| t.slot == slot)
        {
            out.textures.push(TexRef {
                slot,
                source: TexSource::Path { original: p.clone(), base_dir: base_dir.to_string() },
            });
        }
    };
    push(TextureSlot::BaseColor, m.diffuse_texture.as_ref());
    push(TextureSlot::Normal, param("norm"));
    // map_Bump / bump: usually a normal map in game assets, unless named like a height map.
    push(TextureSlot::Normal, m.normal_texture.as_ref().filter(|p| {
        crate::textures::detect_slot(p) != Some(TextureSlot::Height)
    }));
    push(TextureSlot::Roughness, param("map_Pr"));
    push(TextureSlot::Metallic, param("map_Pm"));
    push(TextureSlot::Emissive, param("map_Ke"));
    push(TextureSlot::Opacity, m.dissolve_texture.as_ref());
    push(TextureSlot::Specular, m.specular_texture.as_ref());
    push(TextureSlot::Occlusion, m.ambient_texture.as_ref().filter(|p| {
        crate::textures::detect_slot(p) == Some(TextureSlot::Occlusion)
    }));
    out
}
