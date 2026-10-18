//! Bevy 3D viewer embedded in the Leptos UI through `leptos-bevy-canvas`.
//!
//! The viewer never talks to the network: Leptos downloads the GLB / texture
//! bytes and sends them in [`ViewerCommand`]s. GLBs are written into an
//! in-memory asset source (`mem://`) and loaded with the regular glTF loader.

mod camera;
mod model;
mod variants;
mod wireframe;

use bevy::asset::AssetMetaCheck;
use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::memory::{Dir, MemoryAssetReader};
use bevy::prelude::*;
use hangar_core::TextureSlot;
use leptos_bevy_canvas::prelude::*;
use std::sync::Arc;

/// Leptos → Bevy.
#[derive(Message, Clone)]
pub enum ViewerCommand {
    /// Replace the displayed model. `key` must be unique per content (the GLB
    /// hash); `asset` identifies the logical asset: the camera is only
    /// re-framed when it changes, so switching LOD/version keeps the view.
    LoadModel { key: String, asset: String, bytes: Arc<Vec<u8>> },
    /// Remove the model.
    Clear,
    /// Override textures by material name (a skin / texture variant).
    ApplyVariant { textures: Vec<VariantTexture> },
    /// Restore the textures baked into the GLB.
    ResetVariant,
    SetWireframe(bool),
    SetAutoRotate(bool),
    SetGrid(bool),
    /// Frame the model again.
    ResetCamera,
}

#[derive(Clone)]
pub struct VariantTexture {
    pub material: String,
    pub slot: TextureSlot,
    /// File extension of `bytes` ("png", "jpg").
    pub ext: String,
    pub bytes: Arc<Vec<u8>>,
}

/// Bevy → Leptos.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum ViewerEvent {
    Loaded { key: String, meshes: usize, triangles: usize },
    Error(String),
}

/// In-memory directory backing the `mem://` asset source.
#[derive(Resource, Clone)]
pub struct MemDir(pub Dir);

/// Display toggles, shared by the systems.
#[derive(Resource)]
pub struct ViewerSettings {
    pub wireframe: bool,
    pub grid: bool,
    pub auto_rotate: bool,
}

impl Default for ViewerSettings {
    fn default() -> Self {
        Self { wireframe: false, grid: true, auto_rotate: false }
    }
}

pub const CANVAS_ID: &str = "hangar_viewer";

/// Builds the Bevy app rendering into `#hangar_viewer`.
pub fn build_app(
    commands_rx: BevyMessageReceiver<ViewerCommand>,
    events_tx: BevyMessageSender<ViewerEvent>,
) -> App {
    let dir = Dir::default();
    let reader_dir = dir.clone();

    let mut app = App::new();
    // Must happen before AssetPlugin (part of DefaultPlugins).
    app.register_asset_source(
        "mem",
        AssetSourceBuilder::new(move || Box::new(MemoryAssetReader { root: reader_dir.clone() })),
    );
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    canvas: Some(format!("#{CANVAS_ID}")),
                    fit_canvas_to_parent: true,
                    prevent_default_event_handling: true,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                // Nothing on the default source is used; never probe for .meta files.
                meta_check: AssetMetaCheck::Never,
                ..default()
            }),
    )
    .insert_resource(MemDir(dir))
    .insert_resource(ClearColor(Color::srgb(0.075, 0.08, 0.09)))
    .insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.85, 0.88, 0.95),
        brightness: 450.0,
        ..default()
    })
    .init_resource::<ViewerSettings>()
    .init_resource::<camera::OrbitCamera>()
    .init_resource::<model::ModelState>()
    .init_resource::<variants::VariantState>()
    .init_resource::<wireframe::WireCache>()
    .import_message_from_leptos(commands_rx)
    .export_message_to_leptos(events_tx)
    .add_systems(Startup, setup_scene)
    .add_observer(model::on_instance_ready)
    .add_systems(
        Update,
        (
            model::handle_commands,
            variants::handle_variant_commands,
            model::watch_load_failures,
            camera::orbit_input,
            camera::apply_orbit,
            draw_grid,
        )
            .chain(),
    )
    .add_systems(
        Last,
        (model::finish_model_setup, wireframe::sync_wireframe, variants::apply_pending_variant).chain(),
    );
    app
}

fn setup_scene(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(4.0, 3.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
        camera::MainCamera,
    ));
    // Key light.
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(4.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Cool rim / fill light from the opposite side.
    commands.spawn((
        DirectionalLight {
            illuminance: 2_500.0,
            color: Color::srgb(0.75, 0.82, 1.0),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-6.0, 3.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Ground grid sized to the model, drawn at its lowest point.
fn draw_grid(mut gizmos: Gizmos, settings: Res<ViewerSettings>, state: Res<model::ModelState>) {
    if !settings.grid {
        return;
    }
    let (center, size, floor) = match state.bounds {
        Some((min, max)) => {
            let c = (min + max) * 0.5;
            let s = (max - min).max_element().max(0.01);
            (Vec3::new(c.x, min.y, c.z), s, min.y)
        }
        None => (Vec3::ZERO, 4.0, 0.0),
    };
    // Spacing: power of ten that gives ~10-20 cells across the model.
    let spacing = 10f32.powf((size / 2.0).log10().floor()).max(0.001);
    let cells = ((size * 3.0) / spacing).ceil().clamp(4.0, 60.0) as u32;
    let iso = Isometry3d::new(
        Vec3::new(center.x, floor, center.z),
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
    );
    gizmos.grid(iso, UVec2::splat(cells), Vec2::splat(spacing), Color::srgba(0.6, 0.65, 0.7, 0.22));
}
