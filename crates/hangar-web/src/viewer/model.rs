//! Loading / swapping the displayed GLB.

use super::camera::{MainCamera, OrbitCamera};
use super::{MemDir, ViewerCommand, ViewerEvent, ViewerSettings};
use bevy::asset::LoadState;
use bevy::asset::io::memory::Value;
use bevy::camera::primitives::Aabb;
use bevy::gltf::GltfAssetLabel;
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceReady};
use std::path::PathBuf;

/// Root entity of the displayed model.
#[derive(Component)]
pub struct ModelRoot {
    pub key: String,
}

/// Set by the observer once the glTF scene has been instantiated; consumed in
/// `Last`, after transforms have been propagated.
#[derive(Component)]
pub struct InstanceReady;

#[derive(Resource, Default)]
pub struct ModelState {
    pub root: Option<Entity>,
    pub key: Option<String>,
    pub handle: Option<Handle<WorldAsset>>,
    /// Logical asset currently framed by the camera.
    pub framed_asset: Option<String>,
    /// Asset of the model being loaded (decides whether to re-frame).
    pub loading_asset: Option<String>,
    pub bounds: Option<(Vec3, Vec3)>,
    pub failed_reported: bool,
    /// Mesh entities of the current model (filled once ready).
    pub meshes: Vec<Entity>,
    /// Bumped every time a model becomes ready, so other systems can react.
    pub generation: u64,
}

pub fn mem_path(key: &str) -> String {
    format!("{key}.glb")
}

pub fn handle_commands(
    mut reader: MessageReader<ViewerCommand>,
    mut commands: Commands,
    mut state: ResMut<ModelState>,
    mut settings: ResMut<ViewerSettings>,
    mut orbit: ResMut<OrbitCamera>,
    dir: Res<MemDir>,
    asset_server: Res<AssetServer>,
    projection: Query<&Projection, With<MainCamera>>,
) {
    for cmd in reader.read() {
        match cmd {
            ViewerCommand::LoadModel { key, asset, bytes, .. } => {
                if state.key.as_deref() == Some(key.as_str()) && state.root.is_some() {
                    continue;
                }
                if let Some(old) = state.root.take() {
                    commands.entity(old).despawn();
                }
                let path = mem_path(key);
                dir.0.insert_asset(&PathBuf::from(&path), Value::Vec(bytes.clone()));
                let handle: Handle<WorldAsset> =
                    asset_server.load(GltfAssetLabel::Scene(0).from_asset(format!("mem://{path}")));
                let root = commands
                    .spawn((
                        WorldAssetRoot(handle.clone()),
                        ModelRoot { key: key.clone() },
                        Transform::default(),
                        Name::new("model"),
                    ))
                    .id();
                state.root = Some(root);
                state.key = Some(key.clone());
                state.handle = Some(handle);
                state.loading_asset = Some(asset.clone());
                state.failed_reported = false;
                state.meshes.clear();
            }
            ViewerCommand::Clear => {
                if let Some(old) = state.root.take() {
                    commands.entity(old).despawn();
                }
                state.key = None;
                state.handle = None;
                state.bounds = None;
                state.meshes.clear();
                state.framed_asset = None;
            }
            ViewerCommand::SetWireframe(on) => settings.wireframe = *on,
            ViewerCommand::SetAutoRotate(on) => settings.auto_rotate = *on,
            ViewerCommand::SetGrid(on) => settings.grid = *on,
            ViewerCommand::ResetCamera => {
                if let Some((min, max)) = state.bounds {
                    let fov = match projection.single() {
                        Ok(Projection::Perspective(p)) => p.fov,
                        _ => std::f32::consts::FRAC_PI_4,
                    };
                    orbit.reset_angles();
                    orbit.frame(min, max, fov);
                }
            }
            ViewerCommand::ApplyVariant { .. }
            | ViewerCommand::ResetVariant
            | ViewerCommand::SelectAnimation(_)
            | ViewerCommand::SetAnimationPaused(_)
            | ViewerCommand::StepAnimation(_)
            | ViewerCommand::SetAnimationSpeed(_) => {}
        }
    }
}

/// Reports glTF loading failures to Leptos.
pub fn watch_load_failures(
    mut state: ResMut<ModelState>,
    asset_server: Res<AssetServer>,
    mut events: MessageWriter<ViewerEvent>,
) {
    if state.failed_reported {
        return;
    }
    let Some(handle) = state.handle.as_ref() else { return };
    if let Some(LoadState::Failed(err)) = asset_server.get_load_state(handle.id()) {
        state.failed_reported = true;
        events.write(ViewerEvent::Error(format!("Falha ao carregar o GLB: {err}")));
    }
}

pub fn on_instance_ready(ev: On<WorldInstanceReady>, mut commands: Commands, roots: Query<(), With<ModelRoot>>) {
    let entity = ev.event().entity;
    if roots.contains(entity) {
        commands.entity(entity).insert(InstanceReady);
    }
}

/// Runs in `Last`: computes bounds, counts triangles, frames the camera.
#[allow(clippy::too_many_arguments)]
pub fn finish_model_setup(
    mut commands: Commands,
    ready: Query<(Entity, &ModelRoot), With<InstanceReady>>,
    children: Query<&Children>,
    mesh_q: Query<(&GlobalTransform, Option<&Aabb>, &Mesh3d)>,
    meshes: Res<Assets<Mesh>>,
    mut state: ResMut<ModelState>,
    mut orbit: ResMut<OrbitCamera>,
    dir: Res<MemDir>,
    projection: Query<&Projection, With<MainCamera>>,
    mut events: MessageWriter<ViewerEvent>,
) {
    for (entity, root) in &ready {
        commands.entity(entity).remove::<InstanceReady>();
        if state.root != Some(entity) {
            continue;
        }
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut triangles = 0usize;
        let mut mesh_entities = Vec::new();
        for e in children.iter_descendants(entity) {
            let Ok((gt, aabb, mesh3d)) = mesh_q.get(e) else { continue };
            mesh_entities.push(e);
            if let Some(mesh) = meshes.get(&mesh3d.0) {
                triangles += match mesh.indices() {
                    Some(Indices::U16(i)) => i.len() / 3,
                    Some(Indices::U32(i)) => i.len() / 3,
                    None => mesh.count_vertices() / 3,
                };
                if aabb.is_none() {
                    if let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
                        for v in p {
                            let w = gt.transform_point(Vec3::from(*v));
                            min = min.min(w);
                            max = max.max(w);
                        }
                    }
                }
            }
            if let Some(aabb) = aabb {
                let c = Vec3::from(aabb.center);
                let h = Vec3::from(aabb.half_extents);
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        for sz in [-1.0, 1.0] {
                            let w = gt.transform_point(c + h * Vec3::new(sx, sy, sz));
                            min = min.min(w);
                            max = max.max(w);
                        }
                    }
                }
            }
        }
        if min.x > max.x {
            min = Vec3::splat(-0.5);
            max = Vec3::splat(0.5);
        }
        state.bounds = Some((min, max));
        let mesh_count = mesh_entities.len();
        state.meshes = mesh_entities;
        state.generation += 1;

        if state.framed_asset != state.loading_asset {
            let fov = match projection.single() {
                Ok(Projection::Perspective(p)) => p.fov,
                _ => std::f32::consts::FRAC_PI_4,
            };
            orbit.reset_angles();
            orbit.frame(min, max, fov);
            state.framed_asset = state.loading_asset.clone();
        }

        // The asset server keeps the parsed scene; drop the raw bytes.
        dir.0.remove_asset(&PathBuf::from(mem_path(&root.key)));

        events.write(ViewerEvent::Loaded { key: root.key.clone(), meshes: mesh_count, triangles });
    }
}
