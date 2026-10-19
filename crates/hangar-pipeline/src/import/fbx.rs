//! FBX import through ufbx.

use crate::scene::{AlphaMode, AnimClip, Material, NodeChannel, Rig, Scene, SceneNode, Skin, TexRef, TexSource, VertexDedup};
use anyhow::{anyhow, Result};
use hangar_core::{paths, TextureSlot};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

fn load_opts(geometry: bool) -> ufbx::LoadOpts<'static> {
    ufbx::LoadOpts {
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
        ignore_geometry: !geometry,
        ignore_embedded: !geometry,
        evaluate_skinning: false,
        // glTF can't express Maya's segment scale compensation: fold it into the
        // local transforms so the exported hierarchy reproduces the world matrices.
        inherit_mode_handling: ufbx::InheritModeHandling::Compensate,
        ..Default::default()
    }
}

fn open(path: &Path, geometry: bool) -> Result<ufbx::SceneRoot> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("non utf-8 path"))?;
    ufbx::load_file(path_str, load_opts(geometry)).map_err(|e| anyhow!("ufbx: {e:?}"))
}

/// What an FBX file holds, for picking the preview model and animation files.
#[derive(Debug, Clone, Copy)]
pub struct FbxProbe {
    pub has_mesh: bool,
    /// Some take actually moves a node.
    pub has_animation: bool,
}

/// Cheap look into an FBX (geometry is not loaded). Baking the takes is the
/// expensive part: with `check_animation` false it only happens for files
/// without a mesh (`has_animation` stays false for the others).
pub fn probe(path: &Path, check_animation: bool) -> Result<FbxProbe> {
    let scene = open(path, false)?;
    let has_mesh = scene.nodes.iter().any(|n| n.mesh.is_some());
    let has_animation = (check_animation || !has_mesh)
        && !bake_clips(&scene, None, &|n| Some(n.element.typed_id as usize), &mut vec![]).is_empty();
    Ok(FbxProbe { has_mesh, has_animation })
}

/// The moving takes of an animation file on its own (no model to retarget
/// to): for listing the clips of a version that has no mesh.
pub fn clips(path: &Path, rel: &str) -> Result<Vec<AnimClip>> {
    let scene = open(path, false)?;
    Ok(bake_clips(&scene, Some(rel), &|n| Some(n.element.typed_id as usize), &mut vec![]))
}

/// Loads the model at `path`. `anim_files` are (version path, file) of other FBX
/// files whose takes are added to the model's clips, matched by node name
/// (animations exported separately, e.g. `Shark@Swim.fbx`).
pub fn load(path: &Path, base_dir: &str, anim_files: &[(String, std::path::PathBuf)]) -> Result<Scene> {
    let scene = open(path, true)?;

    let mut out = Scene { importer: "ufbx", ..Default::default() };
    out.has_skin = !scene.skin_deformers.is_empty();
    let mut clips = bake_clips(&scene, None, &|n| Some(n.element.typed_id as usize), &mut out.warnings);
    for (rel, file) in anim_files {
        let (file_clips, matched, total) = external_clips(&scene, rel, file, &mut out.warnings);
        clips.extend(file_clips);
        out.bones_matched += matched;
        out.bones_total += total;
    }
    unique_names(&mut clips);
    let mut rig = (!clips.is_empty()).then(|| Rig { nodes: rig_nodes(&scene), skins: vec![], clips });
    out.animations = rig.as_ref().map_or(0, |r| r.clips.len() as u32);
    // (skin deformer, mesh node) -> index into rig.skins
    let mut skin_index: HashMap<(u32, u32), usize> = HashMap::new();

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
        // Animated scenes: rigid meshes follow their node, skinned ones their skin.
        let deformer = mesh.skin_deformers.iter().next().filter(|_| rig.is_some());
        let skin = match (deformer, rig.as_mut()) {
            (Some(d), Some(rig)) => Some(*skin_index.entry((d.element.typed_id, node.element.typed_id)).or_insert_with(|| {
                rig.skins.push(convert_skin(d, node));
                rig.skins.len() - 1
            })),
            _ => None,
        };
        let rigid_node = (rig.is_some() && skin.is_none()).then_some(node.element.typed_id as usize);

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
                        let influences = deformer.filter(|_| skin.is_some()).map(|d| {
                            let v = mesh.vertex_indices[ix];
                            let (j, w) = vertex_influences(d, v as usize);
                            (v, j, w)
                        });
                        dd.push_skinned([wp.x as f32, wp.y as f32, wp.z as f32], nn, uv, influences);
                    }
                }
            }
            if !dd.indices.is_empty() {
                let mut prim = dd.into_prim(name.clone(), mat);
                prim.node = rigid_node;
                prim.skin = skin;
                out.meshes.push(prim);
            }
        }
    }
    out.source_had_normals = any_normals;
    if out.meshes.is_empty() {
        return Err(anyhow!("FBX contains no mesh geometry"));
    }
    out.rig = rig;
    Ok(out)
}

fn mat4(m: &ufbx::Matrix) -> glam::Mat4 {
    glam::Mat4::from_cols_array(&[
        m.m00 as f32, m.m10 as f32, m.m20 as f32, 0.0,
        m.m01 as f32, m.m11 as f32, m.m21 as f32, 0.0,
        m.m02 as f32, m.m12 as f32, m.m22 as f32, 0.0,
        m.m03 as f32, m.m13 as f32, m.m23 as f32, 1.0,
    ])
}

/// Bakes every take of `scene` that actually moves something (exporters write
/// an empty "Take 001" into almost every FBX). `target` maps a node of `scene`
/// to the rig node it drives; unmapped nodes are dropped. `source` is the
/// version path of a separate animation file.
fn bake_clips(
    scene: &ufbx::Scene,
    source: Option<&str>,
    target: &dyn Fn(&ufbx::Node) -> Option<usize>,
    warnings: &mut Vec<String>,
) -> Vec<AnimClip> {
    let fps = match scene.settings.frames_per_second {
        f if f > 0.0 => f.min(120.0),
        _ => 30.0,
    };
    let mut clips = Vec::new();
    for (i, stack) in scene.anim_stacks.iter().enumerate() {
        let opts = ufbx::BakeOpts { resample_rate: fps, minimum_sample_rate: fps, ..Default::default() };
        let baked = match ufbx::bake_anim(scene, &stack.anim, opts) {
            Ok(b) => b,
            Err(e) => {
                warnings.push(format!("animation '{}' skipped: {e:?}", stack.element.name));
                continue;
            }
        };
        let start = baked.playback_time_begin;
        let rel = |t: f64| (t - start).max(0.0) as f32;
        let v3 = |v: &ufbx::Vec3| [v.x as f32, v.y as f32, v.z as f32];
        let channels: Vec<NodeChannel> = baked
            .nodes
            .iter()
            .filter_map(|b| Some((b, target(scene.nodes.get(b.typed_id as usize)?)?)))
            .map(|(b, node)| NodeChannel {
                node,
                translation: b.translation_keys.iter().map(|k| (rel(k.time), v3(&k.value))).collect(),
                rotation: b
                    .rotation_keys
                    .iter()
                    .map(|k| (rel(k.time), [k.value.x as f32, k.value.y as f32, k.value.z as f32, k.value.w as f32]))
                    .collect(),
                scale: b.scale_keys.iter().map(|k| (rel(k.time), v3(&k.value))).collect(),
            })
            .collect();
        if !channels.iter().any(channel_moves) {
            continue;
        }
        let last_key = channels
            .iter()
            .flat_map(|c| {
                let t = c.translation.iter().map(|k| k.0);
                t.chain(c.rotation.iter().map(|k| k.0)).chain(c.scale.iter().map(|k| k.0))
            })
            .fold(0.0f32, f32::max);
        let duration = (baked.playback_duration as f32).max(last_key);
        if duration <= 0.0 {
            continue;
        }
        let name = clip_name(&stack.element.name, source, i);
        clips.push(AnimClip { name, duration, fps: fps as f32, channels, source: source.map(str::to_string) });
    }
    clips
}

/// Takes of a separate animation file, retargeted by node name onto `model`,
/// plus (named nodes of the file that exist in the model, named nodes).
fn external_clips(model: &ufbx::Scene, rel: &str, file: &Path, warnings: &mut Vec<String>) -> (Vec<AnimClip>, u32, u32) {
    let scene = match open(file, false) {
        Ok(s) => s,
        Err(e) => {
            warnings.push(format!("animation file {rel} skipped: {e:#}"));
            return (vec![], 0, 0);
        }
    };
    let by_name = nodes_by_name(model);
    let (matched, total) = name_matches(&by_name, &scene);
    let clips = bake_clips(&scene, Some(rel), &|n| by_name.get(&node_key(&n.element.name)).copied(), warnings);
    if clips.is_empty() {
        warnings.push(format!("animation file {rel}: no animated node matches the model's nodes"));
    }
    (clips, matched, total)
}

fn nodes_by_name(model: &ufbx::Scene) -> HashMap<String, usize> {
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for n in &model.nodes {
        by_name.entry(node_key(&n.element.name)).or_insert(n.element.typed_id as usize);
    }
    by_name
}

/// (named nodes of `anim` that exist in the model, named nodes of `anim`).
fn name_matches(by_name: &HashMap<String, usize>, anim: &ufbx::Scene) -> (u32, u32) {
    let named: Vec<String> =
        anim.nodes.iter().filter(|n| !n.is_root).map(|n| node_key(&n.element.name)).filter(|k| !k.is_empty()).collect();
    (named.iter().filter(|k| by_name.contains_key(*k)).count() as u32, named.len() as u32)
}

/// Whether the animation file `anim` was made for the model at `model`: at
/// least half of its named nodes exist in the model. A prop shipped with an
/// animation pack (the weapon of "Rocket Launcher") shares none.
pub fn animates(model: &Path, anim: &Path) -> Result<bool> {
    let (model, anim) = (open(model, false)?, open(anim, false)?);
    let (matched, total) = name_matches(&nodes_by_name(&model), &anim);
    Ok(total > 0 && matched * 2 >= total)
}

/// Node name for matching across files: case-insensitive, without namespaces
/// (`mixamorig:Hips` ↔ `Hips`).
fn node_key(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).trim().to_lowercase()
}

/// Display name of a take. Generic exporter names are replaced by the file
/// name (`Shark@Swim.fbx` → "Swim"); Blender's `Armature|Swim` keeps "Swim".
fn clip_name(take: &str, source: Option<&str>, index: usize) -> String {
    let take = take.rsplit('|').next().unwrap_or(take).trim();
    let generic = take.is_empty()
        || matches!(
            take.to_lowercase().as_str(),
            "take 001" | "take 01" | "take001" | "take 1" | "mixamo.com" | "unreal take" | "default take" | "animation" | "anim" | "scene"
        );
    match (generic, source) {
        (false, _) => take.to_string(),
        (true, Some(src)) => {
            let stem = paths::stem(src);
            stem.rsplit('@').next().unwrap_or(stem).trim().to_string()
        }
        (true, None) if take.is_empty() => format!("Animação {}", index + 1),
        (true, None) => take.to_string(),
    }
}

/// Clip names must be unique for the dropdown: repeated ones get " (2)", " (3)"...
pub(crate) fn unique_names(clips: &mut [AnimClip]) {
    for i in 1..clips.len() {
        let base = clips[i].name.clone();
        let mut n = 1;
        while clips[..i].iter().any(|c| c.name.eq_ignore_ascii_case(&clips[i].name)) {
            n += 1;
            clips[i].name = format!("{base} ({n})");
        }
    }
}

fn rig_nodes(scene: &ufbx::Scene) -> Vec<SceneNode> {
    scene
        .nodes
        .iter()
        .map(|n| {
            let t = &n.local_transform;
            SceneNode {
                name: n.element.name.to_string(),
                parent: n.parent.as_ref().map(|p| p.element.typed_id as usize),
                translation: [t.translation.x as f32, t.translation.y as f32, t.translation.z as f32],
                rotation: [t.rotation.x as f32, t.rotation.y as f32, t.rotation.z as f32, t.rotation.w as f32],
                scale: [t.scale.x as f32, t.scale.y as f32, t.scale.z as f32],
                world: mat4(&n.node_to_world),
                world_inv: mat4(&ufbx::matrix_invert(&n.node_to_world)),
            }
        })
        .collect()
}

/// Whether any key differs from the previous one (a constant pose is not an animation).
fn channel_moves(c: &NodeChannel) -> bool {
    fn varies<const N: usize>(keys: &[(f32, [f32; N])]) -> bool {
        keys.windows(2).any(|w| w[0].1.iter().zip(&w[1].1).any(|(a, b)| (a - b).abs() > 1e-5))
    }
    varies(&c.translation) || varies(&c.rotation) || varies(&c.scale)
}

/// glTF skin for `deformer` as used by the mesh on `node`. Vertices are exported
/// in world space (rest pose), so each inverse bind matrix maps world space to
/// the bone at bind time.
fn convert_skin(deformer: &ufbx::SkinDeformer, node: &ufbx::Node) -> Skin {
    let world_to_geometry = mat4(&ufbx::matrix_invert(&node.geometry_to_world));
    let mut joints = Vec::new();
    let mut inverse_bind = Vec::new();
    for cluster in deformer.clusters.iter() {
        // A cluster without a bone still needs a joint slot; the root is static.
        joints.push(cluster.bone_node.as_ref().map_or(0, |b| b.element.typed_id as usize));
        inverse_bind.push(mat4(&cluster.geometry_to_bone) * world_to_geometry);
    }
    Skin { name: deformer.element.name.to_string(), joints, inverse_bind }
}

/// The four strongest influences of a control point, normalised.
fn vertex_influences(deformer: &ufbx::SkinDeformer, vertex: usize) -> ([u16; 4], [f32; 4]) {
    let mut all: Vec<(u16, f32)> = deformer
        .vertices
        .get(vertex)
        .map(|v| {
            let range = v.weight_begin as usize..(v.weight_begin + v.num_weights) as usize;
            range.filter_map(|i| deformer.weights.get(i)).map(|w| (w.cluster_index as u16, w.weight as f32)).collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b.1.total_cmp(&a.1));
    all.truncate(4);
    let sum: f32 = all.iter().map(|w| w.1).sum();
    if sum <= 0.0 {
        return ([0; 4], [1.0, 0.0, 0.0, 0.0]);
    }
    let mut joints = [0u16; 4];
    let mut weights = [0f32; 4];
    for (i, (j, w)) in all.into_iter().enumerate() {
        joints[i] = j;
        weights[i] = w / sum;
    }
    (joints, weights)
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
    let opacity = constant_opacity(m);
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
    // Blender writes an alpha wired to opacity as a TransparencyFactor texture; other
    // exporters use TransparentColor or the PBR opacity. The same image as the base
    // color means "use its alpha channel"; a separate one is an opacity map.
    let base_tex = [&pbr.base_color, &m.fbx.diffuse_color].into_iter().find_map(|map| map.texture.as_ref());
    let opacity_map = [&pbr.opacity, &m.fbx.transparency_factor, &m.fbx.transparency_color]
        .into_iter()
        .find(|map| map.texture.is_some());
    if let Some(map) = opacity_map
        && !base_tex.zip(map.texture.as_ref()).is_some_and(|(b, t)| same_image(b, t))
    {
        push(TextureSlot::Opacity, map);
    }
    push(TextureSlot::Specular, &pbr.specular_color);
    push(TextureSlot::Specular, &m.fbx.specular_color);
    push(TextureSlot::Specular, &pbr.specular_factor);
    push(TextureSlot::Specular, &m.fbx.specular_factor);

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

    // A linked base color without a linked opacity: whatever its alpha channel holds
    // (Halo specular masks, Unreal roughness...) is not transparency.
    out.alpha = match (base_tex.is_some(), opacity_map.is_some()) {
        (_, true) => AlphaMode::Mask(0.5),
        (true, false) => AlphaMode::IgnoreTexture,
        (false, false) => AlphaMode::Auto,
    };
    out
}

/// The material's constant opacity: the PBR value, else 1 − TransparentColor ×
/// TransparencyFactor (Maya writes factor 1 with a black color on opaque materials,
/// so the factor alone is not enough).
fn constant_opacity(m: &ufbx::Material) -> f64 {
    if m.pbr.opacity.has_value {
        return m.pbr.opacity.value_vec4.x;
    }
    let (f, c) = (&m.fbx.transparency_factor, &m.fbx.transparency_color);
    let factor = if f.has_value { f.value_vec4.x } else { 1.0 };
    let color = if c.has_value { (c.value_vec4.x + c.value_vec4.y + c.value_vec4.z) / 3.0 } else { 1.0 };
    if f.has_value || c.has_value { 1.0 - factor * color } else { 1.0 }
}

/// Exporters may write one texture object per slot for the same image file.
fn same_image(a: &ufbx::Texture, b: &ufbx::Texture) -> bool {
    let key = |t: &ufbx::Texture| {
        let candidates: [&str; 3] = [&t.relative_filename, &t.filename, &t.absolute_filename];
        candidates.into_iter().find(|s| !s.is_empty()).map(|s| s.replace('\\', "/").to_ascii_lowercase())
    };
    a.element.element_id == b.element.element_id || key(a).is_some_and(|k| key(b) == Some(k))
}
