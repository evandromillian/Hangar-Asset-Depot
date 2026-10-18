//! Wireframe overlay. WebGL2 has no `PolygonMode::Line`, so we build a
//! `LineList` mesh of the unique triangle edges and parent it to each mesh.

use super::ViewerSettings;
use super::model::ModelState;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Component)]
pub struct WireOverlay;

#[derive(Resource, Default)]
pub struct WireCache {
    lines: HashMap<AssetId<Mesh>, Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    /// Model generation for which overlays were built.
    built_for: Option<u64>,
}

fn edge_mesh(mesh: &Mesh) -> Option<Mesh> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return None;
    };
    let tri: Vec<u32> = match mesh.indices() {
        Some(Indices::U16(i)) => i.iter().map(|&v| v as u32).collect(),
        Some(Indices::U32(i)) => i.clone(),
        None => (0..pos.len() as u32).collect(),
    };
    let mut edges: HashSet<(u32, u32)> = HashSet::with_capacity(tri.len());
    for t in tri.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            edges.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    let mut indices = Vec::with_capacity(edges.len() * 2);
    for (a, b) in edges {
        indices.push(a);
        indices.push(b);
    }
    // Lift the lines off their own triangles along the vertex normals so they win the depth test.
    // A material `depth_bias` can't be used: wgpu rejects it for non-triangle topologies, and
    // Bevy quits on that validation error.
    let lifted = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(nrm)) if nrm.len() == pos.len() => {
            let (min, max) = pos.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
                let p = Vec3::from(*p);
                (lo.min(p), hi.max(p))
            });
            let eps = (max - min).length() * 1e-3;
            pos.iter()
                .zip(nrm)
                .map(|(p, n)| (Vec3::from(*p) + Vec3::from(*n).normalize_or_zero() * eps).to_array())
                .collect()
        }
        _ => pos.clone(),
    };
    let mut lines = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD);
    lines.insert_attribute(Mesh::ATTRIBUTE_POSITION, lifted);
    lines.insert_indices(Indices::U32(indices));
    Some(lines)
}

pub fn sync_wireframe(
    mut commands: Commands,
    settings: Res<ViewerSettings>,
    model: Res<ModelState>,
    mut cache: ResMut<WireCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mesh_q: Query<&Mesh3d, Without<WireOverlay>>,
    mut overlays: Query<&mut Visibility, With<WireOverlay>>,
) {
    if settings.wireframe && cache.built_for != Some(model.generation) && !model.meshes.is_empty() {
        let material = cache
            .material
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.69, 0.0),
                    unlit: true,
                    ..default()
                })
            })
            .clone();
        for &e in &model.meshes {
            let Ok(mesh3d) = mesh_q.get(e) else { continue };
            let id = mesh3d.0.id();
            let handle = match cache.lines.get(&id) {
                Some(h) => h.clone(),
                None => {
                    let Some(src) = meshes.get(id) else { continue };
                    let Some(lines) = edge_mesh(src) else { continue };
                    let h = meshes.add(lines);
                    cache.lines.insert(id, h.clone());
                    h
                }
            };
            commands.entity(e).with_child((
                Mesh3d(handle),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
                WireOverlay,
            ));
        }
        cache.built_for = Some(model.generation);
    }
    if settings.is_changed() || model.is_changed() {
        let vis = if settings.wireframe { Visibility::Inherited } else { Visibility::Hidden };
        for mut v in &mut overlays {
            if *v != vis {
                *v = vis;
            }
        }
    }
}
