//! Orbit camera: LMB orbit, RMB/MMB pan, wheel zoom, F to frame.

use super::model::ModelState;
use super::{ViewAngles, ViewerSettings};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};

/// Highest |pitch|: just short of the poles, where `look_at` with +Y up flips.
const PITCH_LIMIT: f32 = 1.54;

#[derive(Component)]
pub struct MainCamera;

#[derive(Resource)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub radius: f32,
    /// Rotation around Y (radians).
    pub yaw: f32,
    /// Elevation (radians), clamped to avoid flipping over the poles.
    pub pitch: f32,
    /// Size of the framed model, used to scale pan speed and clip planes.
    pub scene_radius: f32,
    /// (yaw, pitch) the camera is turning to (`view_from`), animated by `orbit_input`.
    pub target: Option<(f32, f32)>,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self { focus: Vec3::ZERO, radius: 8.0, yaw: 0.6, pitch: 0.35, scene_radius: 2.0, target: None }
    }
}

impl OrbitCamera {
    /// Frames a bounding box (keeps the current viewing angle).
    pub fn frame(&mut self, min: Vec3, max: Vec3, fov: f32) {
        let center = (min + max) * 0.5;
        let r = ((max - min).length() * 0.5).max(0.01);
        self.focus = center;
        self.scene_radius = r;
        self.radius = r / (fov * 0.5).sin() * 1.08;
    }

    pub fn reset_angles(&mut self) {
        self.yaw = 0.6;
        self.pitch = 0.35;
        self.target = None;
    }

    /// Starts turning the camera to look at the focus from `dir` (world space: `Vec3::X` looks
    /// from +X towards -X).
    pub fn view_from(&mut self, dir: Vec3) {
        let d = dir.normalize_or_zero();
        // Straight above or below: yaw 0 keeps +X to the right, like Blender's top and bottom views.
        let yaw = if Vec2::new(d.x, d.z).length() < 1e-4 { 0.0 } else { d.x.atan2(d.z) };
        let pitch = d.y.clamp(-1.0, 1.0).asin().clamp(-PITCH_LIMIT, PITCH_LIMIT);
        // The shortest way round from the current yaw (auto-rotate lets it grow without bound).
        let yaw = self.yaw + (yaw - self.yaw + PI).rem_euclid(TAU) - PI;
        self.target = Some((yaw, pitch));
    }
}

pub fn orbit_input(
    mut orbit: ResMut<OrbitCamera>,
    settings: Res<ViewerSettings>,
    state: Res<ModelState>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    projection: Query<&Projection, With<MainCamera>>,
) {
    let delta = motion.delta;
    if let Some((yaw, pitch)) = orbit.target {
        if buttons.pressed(MouseButton::Left) {
            orbit.target = None;
        } else {
            // Exponential ease (frame-rate independent), snapping once within ~0.05°.
            let k = 1.0 - (-time.delta_secs() * 12.0).exp();
            orbit.yaw += (yaw - orbit.yaw) * k;
            orbit.pitch += (pitch - orbit.pitch) * k;
            if (yaw - orbit.yaw).abs() < 1e-3 && (pitch - orbit.pitch).abs() < 1e-3 {
                orbit.yaw = yaw;
                orbit.pitch = pitch;
                orbit.target = None;
            }
        }
    }
    if buttons.pressed(MouseButton::Left) && delta != Vec2::ZERO {
        orbit.yaw -= delta.x * 0.008;
        orbit.pitch = (orbit.pitch + delta.y * 0.008).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    } else if (buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle)) && delta != Vec2::ZERO {
        let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
        let right = rot * Vec3::X;
        let up = rot * Vec3::Y;
        let speed = orbit.radius * 0.0018;
        orbit.focus += (-right * delta.x + up * delta.y) * speed;
    }

    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 60.0,
    };
    if lines != 0.0 {
        let min = orbit.scene_radius * 0.05;
        let max = orbit.scene_radius * 40.0;
        orbit.radius = (orbit.radius * (1.0 - lines * 0.12)).clamp(min, max.max(min * 2.0));
    }

    if keys.just_pressed(KeyCode::KeyF) {
        if let Some((min, max)) = state.bounds {
            let fov = match projection.single() {
                Ok(Projection::Perspective(p)) => p.fov,
                _ => std::f32::consts::FRAC_PI_4,
            };
            orbit.frame(min, max, fov);
        }
    }

    if settings.auto_rotate && orbit.target.is_none() && !buttons.pressed(MouseButton::Left) {
        orbit.yaw += time.delta_secs() * 0.35;
    }
}

pub fn apply_orbit(orbit: Res<OrbitCamera>, mut cams: Query<(&mut Transform, &mut Projection), With<MainCamera>>) {
    if !orbit.is_changed() {
        return;
    }
    for (mut tf, mut proj) in &mut cams {
        let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
        tf.translation = orbit.focus + rot * Vec3::new(0.0, 0.0, orbit.radius);
        tf.look_at(orbit.focus, Vec3::Y);
        if let Projection::Perspective(p) = proj.as_mut() {
            // Clip planes follow the model scale (1 cm screws to 50 m ships).
            p.near = (orbit.scene_radius * 0.002).max(0.0005);
            p.far = (orbit.radius + orbit.scene_radius) * 50.0;
        }
    }
}

/// Sends the orbit angles to Leptos (the axis gizmo) whenever they change.
pub fn report_view(orbit: Res<OrbitCamera>, mut out: MessageWriter<ViewAngles>) {
    if orbit.is_changed() {
        out.write(ViewAngles { yaw: orbit.yaw, pitch: orbit.pitch });
    }
}
