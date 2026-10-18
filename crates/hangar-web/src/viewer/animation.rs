//! Animation playback: the clips baked into the GLB (`#AnimationN` labels) are
//! put in an [`AnimationGraph`] and driven through the [`AnimationPlayer`] that
//! the glTF loader adds to the model's single root node.

use super::model::{ModelState, mem_path};
use super::{ClipInfo, ViewerCommand, ViewerEvent};
use bevy::animation::{ActiveAnimation, AnimationTargetId, RepeatAnimation};
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

/// How often the playback position is reported to Leptos while playing.
const REPORT_INTERVAL: f32 = 0.1;

#[derive(Resource)]
pub struct AnimState {
    key: Option<String>,
    clips: Vec<ClipInfo>,
    graph: Option<Handle<AnimationGraph>>,
    nodes: Vec<AnimationNodeIndex>,
    /// Entity carrying the AnimationPlayer of the current model.
    player: Option<Entity>,
    /// Rest transforms of every animated node, restored before a clip starts.
    rest: Vec<(Entity, Transform)>,
    /// Model generation the player was attached for.
    attached_generation: Option<u64>,
    // Playback settings; they survive LOD / version switches.
    selected: Option<usize>,
    paused: bool,
    speed: f32,
    /// Position to resume at when the player is (re)created.
    time: f32,
    /// The player must be (re)configured from the settings above.
    dirty: bool,
    last_reported: Option<f32>,
    since_report: f32,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            key: None,
            clips: vec![],
            graph: None,
            nodes: vec![],
            player: None,
            rest: vec![],
            attached_generation: None,
            selected: None,
            paused: false,
            speed: 1.0,
            time: 0.0,
            dirty: false,
            last_reported: None,
            since_report: 0.0,
        }
    }
}

impl AnimState {
    fn clip(&self) -> Option<(usize, ClipInfo, AnimationNodeIndex)> {
        let i = self.selected?;
        Some((i, *self.clips.get(i)?, *self.nodes.get(i)?))
    }
}

pub fn handle_animation_commands(
    mut reader: MessageReader<ViewerCommand>,
    mut state: ResMut<AnimState>,
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for cmd in reader.read() {
        match cmd {
            ViewerCommand::LoadModel { key, animations, .. } => {
                if state.key.as_deref() == Some(key.as_str()) {
                    continue;
                }
                state.key = Some(key.clone());
                state.player = None;
                state.rest.clear();
                state.attached_generation = None;
                state.clips = animations.clone();
                // Labels of the same file: loaded together with the scene.
                let path = format!("mem://{}", mem_path(key));
                let handles = (0..animations.len())
                    .map(|i| asset_server.load(GltfAssetLabel::Animation(i).from_asset(path.clone())));
                let (graph, nodes) = AnimationGraph::from_clips(handles);
                state.graph = (!animations.is_empty()).then(|| graphs.add(graph));
                state.nodes = nodes;
                // `selected` is kept: Leptos owns it, and an index this model
                // doesn't have simply shows the rest pose.
                state.dirty = true;
            }
            ViewerCommand::Clear => {
                *state = AnimState { speed: state.speed, ..default() };
            }
            ViewerCommand::SelectAnimation(sel) => {
                state.selected = *sel;
                state.time = 0.0;
                state.dirty = true;
            }
            ViewerCommand::SetAnimationPaused(p) => {
                state.paused = *p;
                with_active(&state, &mut players, |a| {
                    if *p {
                        a.pause();
                    } else {
                        a.resume();
                    }
                });
                state.last_reported = None;
            }
            ViewerCommand::SetAnimationSpeed(s) => {
                state.speed = if s.is_finite() { *s } else { 1.0 };
                let speed = state.speed;
                with_active(&state, &mut players, |a| {
                    a.set_speed(speed);
                });
            }
            ViewerCommand::StepAnimation(frames) => {
                let Some((_, info, _)) = state.clip() else { continue };
                state.paused = true;
                let frames = *frames;
                with_active(&state, &mut players, |a| {
                    // Snap to the frame grid, then move; wraps like the looping playback.
                    let fps = info.fps.max(1.0);
                    let count = ((info.duration * fps).round() as i64).max(1);
                    let frame = ((a.seek_time() * fps).round() as i64 + frames as i64).rem_euclid(count);
                    a.pause();
                    a.seek_to(frame as f32 / fps);
                });
                state.last_reported = None;
            }
            _ => {}
        }
    }
}

fn with_active(state: &AnimState, players: &mut Query<&mut AnimationPlayer>, f: impl FnOnce(&mut ActiveAnimation)) {
    let (Some(entity), Some((_, _, node))) = (state.player, state.clip()) else { return };
    if let Ok(mut player) = players.get_mut(entity)
        && let Some(active) = player.animation_mut(node)
    {
        f(active);
    }
}

/// Once a model is ready, hooks the graph to its AnimationPlayer and records
/// the rest pose of the animated nodes.
pub fn attach_player(
    mut commands: Commands,
    mut state: ResMut<AnimState>,
    model: Res<ModelState>,
    children: Query<&Children>,
    players: Query<(), With<AnimationPlayer>>,
    targets: Query<&Transform, With<AnimationTargetId>>,
) {
    let (Some(root), Some(graph)) = (model.root, state.graph.clone()) else { return };
    if model.meshes.is_empty() || state.attached_generation == Some(model.generation) {
        return;
    }
    state.attached_generation = Some(model.generation);
    let descendants: Vec<Entity> = children.iter_descendants(root).collect();
    let Some(player) = descendants.iter().copied().find(|e| players.contains(*e)) else {
        warn!("animated model without an AnimationPlayer");
        return;
    };
    commands.entity(player).insert(AnimationGraphHandle(graph));
    state.rest = descendants.iter().filter_map(|&e| targets.get(e).ok().map(|t| (e, *t))).collect();
    state.player = Some(player);
    state.dirty = true;
}

/// Applies the playback settings to a fresh player or after a clip change.
pub fn apply_animation(
    mut state: ResMut<AnimState>,
    mut players: Query<&mut AnimationPlayer>,
    mut transforms: Query<&mut Transform>,
) {
    if !state.dirty {
        return;
    }
    let Some(entity) = state.player else { return };
    let Ok(mut player) = players.get_mut(entity) else { return };
    state.dirty = false;
    state.last_reported = None;
    player.stop_all();
    // Stopping keeps the last pose; start every clip (or the rest pose) clean.
    for (e, rest) in &state.rest {
        if let Ok(mut t) = transforms.get_mut(*e) {
            *t = *rest;
        }
    }
    let Some((_, info, node)) = state.clip() else { return };
    let active = player.play(node);
    active.set_repeat(RepeatAnimation::Forever).set_speed(state.speed);
    active.seek_to(state.time.clamp(0.0, info.duration));
    if state.paused {
        active.pause();
    }
}

/// Reports the playback position (throttled while playing, at once when paused / stepped).
pub fn report_time(
    time: Res<Time>,
    mut state: ResMut<AnimState>,
    model: Res<ModelState>,
    players: Query<&AnimationPlayer>,
    mut events: MessageWriter<ViewerEvent>,
) {
    // Leptos sees Bevy events through one signal: never share a frame with `Loaded`.
    if model.is_changed() {
        return;
    }
    let (Some(entity), Some((clip, _, node))) = (state.player, state.clip()) else { return };
    let Some(seconds) = players.get(entity).ok().and_then(|p| p.animation(node)).map(|a| a.seek_time()) else {
        return;
    };
    state.time = seconds;
    state.since_report += time.delta_secs();
    let changed = state.last_reported.is_none_or(|t| (t - seconds).abs() > 1e-4);
    if changed && (state.since_report >= REPORT_INTERVAL || state.paused || state.last_reported.is_none()) {
        state.since_report = 0.0;
        state.last_reported = Some(seconds);
        events.write(ViewerEvent::AnimationTime { clip, seconds });
    }
}
