//! CPU evaluation of a baked clip: moves the world-space rest meshes into the
//! pose of a given time, mirroring what a glTF viewer does with the GLB that
//! [`glb::write_glb`](crate::glb::write_glb) writes (node hierarchy under an
//! identity root, linear keys, skins with the same inverse bind matrices).
//! Used for thumbnails of animation assets.

use crate::scene::{AnimClip, MeshPrim, Rig};
use glam::{Mat3, Mat4, Quat, Vec3};

/// Linear interpolation between the keys around `t` (clamped at both ends).
fn sample<const N: usize>(keys: &[(f32, [f32; N])], t: f32, lerp: impl Fn(&[f32; N], &[f32; N], f32) -> [f32; N]) -> Option<[f32; N]> {
    let first = keys.first()?;
    if t <= first.0 {
        return Some(first.1);
    }
    let i = keys.partition_point(|k| k.0 <= t);
    let Some(next) = keys.get(i) else { return keys.last().map(|k| k.1) };
    let prev = &keys[i - 1];
    let span = next.0 - prev.0;
    let f = if span > 0.0 { (t - prev.0) / span } else { 1.0 };
    Some(lerp(&prev.1, &next.1, f))
}

fn lerp3(a: &[f32; 3], b: &[f32; 3], f: f32) -> [f32; 3] {
    Vec3::from(*a).lerp(Vec3::from(*b), f).into()
}

fn slerp(a: &[f32; 4], b: &[f32; 4], f: f32) -> [f32; 4] {
    Quat::from_array(*a).normalize().slerp(Quat::from_array(*b).normalize(), f).to_array()
}

/// World matrix of every rig node at time `t` of `clip`.
pub fn node_worlds(rig: &Rig, clip: &AnimClip, t: f32) -> Vec<Mat4> {
    let n = rig.nodes.len();
    let mut local: Vec<Mat4> = rig
        .nodes
        .iter()
        .map(|node| {
            Mat4::from_scale_rotation_translation(
                node.scale.into(),
                Quat::from_array(node.rotation).normalize(),
                node.translation.into(),
            )
        })
        .collect();
    for ch in clip.channels.iter().filter(|c| c.node < n) {
        let rest = &rig.nodes[ch.node];
        let translation = sample(&ch.translation, t, lerp3).unwrap_or(rest.translation);
        let rotation = sample(&ch.rotation, t, slerp).unwrap_or(rest.rotation);
        let scale = sample(&ch.scale, t, lerp3).unwrap_or(rest.scale);
        local[ch.node] =
            Mat4::from_scale_rotation_translation(scale.into(), Quat::from_array(rotation).normalize(), translation.into());
    }
    // Parents are not guaranteed to come first: resolve on demand.
    let mut world: Vec<Option<Mat4>> = vec![None; n];
    fn resolve(i: usize, rig: &Rig, local: &[Mat4], world: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let parent = rig.nodes[i].parent.filter(|&p| p < rig.nodes.len() && p != i && depth < rig.nodes.len());
        let m = match parent {
            Some(p) => resolve(p, rig, local, world, depth + 1) * local[i],
            None => local[i],
        };
        world[i] = Some(m);
        m
    }
    (0..n).map(|i| resolve(i, rig, &local, &mut world, 0)).collect()
}

/// The meshes in the pose of `clip` at time `t`. Rigid parts follow their
/// node, skinned parts blend their joints; anything else stays put.
pub fn pose_meshes(meshes: &[MeshPrim], rig: &Rig, clip: &AnimClip, t: f32) -> Vec<MeshPrim> {
    let world = node_worlds(rig, clip, t);
    // Per skin: joint matrix = joint world now × inverse bind.
    let skin_mats: Vec<Vec<Mat4>> = rig
        .skins
        .iter()
        .map(|s| {
            s.joints
                .iter()
                .zip(&s.inverse_bind)
                .map(|(&j, ibm)| world.get(j).copied().unwrap_or(Mat4::IDENTITY) * *ibm)
                .collect()
        })
        .collect();
    meshes
        .iter()
        .map(|m| {
            let mut out = m.clone();
            let skin = m.skin.and_then(|s| skin_mats.get(s)).filter(|_| m.joints.len() == m.positions.len() && m.weights.len() == m.positions.len());
            if let Some(mats) = skin {
                for (v, (j, w)) in m.joints.iter().zip(&m.weights).enumerate() {
                    let mut blended = Mat4::ZERO;
                    for k in 0..4 {
                        if w[k] > 0.0 {
                            blended += *mats.get(j[k] as usize).unwrap_or(&Mat4::IDENTITY) * w[k];
                        }
                    }
                    out.positions[v] = blended.transform_point3(m.positions[v].into()).into();
                    if let Some(n) = out.normals.get_mut(v) {
                        *n = (Mat3::from_mat4(blended) * Vec3::from(*n)).normalize_or_zero().into();
                    }
                }
            } else if let Some((node, now)) = m.node.and_then(|n| Some((rig.nodes.get(n)?, world.get(n)?))) {
                let mat = *now * node.world_inv;
                let normal = Mat3::from_mat4(mat).inverse().transpose();
                for p in &mut out.positions {
                    *p = mat.transform_point3((*p).into()).into();
                }
                for n in &mut out.normals {
                    *n = (normal * Vec3::from(*n)).normalize_or_zero().into();
                }
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{NodeChannel, SceneNode, Skin};

    fn node(name: &str, parent: Option<usize>, y: f32) -> SceneNode {
        let world = Mat4::from_translation(Vec3::new(0.0, y, 0.0));
        SceneNode {
            name: name.into(),
            parent,
            translation: [0.0, if parent.is_some() { 1.0 } else { 0.0 }, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            world,
            world_inv: world.inverse(),
        }
    }

    /// Two bones one metre apart; the clip bends the second 90° around Z over a
    /// second. A vertex on top of the second bone swings with it.
    #[test]
    fn poses_skinned_and_rigid_parts() {
        let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
        let rig = Rig {
            nodes: vec![node("Bone0", None, 0.0), node("Bone1", Some(0), 1.0)],
            skins: vec![Skin {
                name: "s".into(),
                joints: vec![0, 1],
                inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0))],
            }],
            clips: vec![],
        };
        let clip = AnimClip {
            name: "Bend".into(),
            duration: 1.0,
            fps: 30.0,
            channels: vec![NodeChannel { node: 1, rotation: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (1.0, quarter)], ..Default::default() }],
            source: None,
        };
        let skinned = MeshPrim {
            positions: vec![[0.0, 2.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            indices: vec![0, 0, 0],
            skin: Some(0),
            joints: vec![[1, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0]],
            ..Default::default()
        };
        let rigid = MeshPrim { positions: vec![[0.0, 2.0, 0.0]], indices: vec![0, 0, 0], node: Some(1), ..Default::default() };
        let near = |a: [f32; 3], b: [f32; 3]| Vec3::from(a).distance(Vec3::from(b)) < 1e-4;

        let rest = pose_meshes(&[skinned.clone(), rigid.clone()], &rig, &clip, 0.0);
        assert!(near(rest[0].positions[0], [0.0, 2.0, 0.0]) && near(rest[1].positions[0], [0.0, 2.0, 0.0]));
        // Past the last key: clamped to the final pose.
        let end = pose_meshes(&[skinned.clone(), rigid], &rig, &clip, 5.0);
        for m in &end {
            assert!(near(m.positions[0], [-1.0, 1.0, 0.0]), "{:?}", m.positions[0]);
        }
        assert!(near(end[0].normals[0], [-1.0, 0.0, 0.0]), "{:?}", end[0].normals[0]);
        let half = pose_meshes(&[skinned], &rig, &clip, 0.5);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(half[0].positions[0], [-s, 1.0 + s, 0.0]), "{:?}", half[0].positions[0]);
    }
}
