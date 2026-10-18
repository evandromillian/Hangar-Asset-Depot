//! Animated FBX → GLB with node hierarchy, skin and baked clip.
//!
//! The FBX is written as ASCII in centimetres (like most real exports), so the
//! unit conversion of geometry, transforms, bind matrices and keys is covered:
//! a rigid box spinning 90° around Y and a strip skinned to two bones, the
//! second bending 45° around Z, plus an empty "Take 001" that must be ignored.
//! A second test splits the same content into a model file and a separate
//! animation-only file (`Model@Clip.fbx`, bones under a namespace).

use glam::{Mat4, Quat, Vec3};
use hangar_core::FileEntry;
use hangar_pipeline::{process, ArtifactRole, PipelineConfig, ProcessInput, SkeletonSource};
use std::fmt::Write as _;

const KTIME_PER_SECOND: i64 = 46_186_158_000;

fn list<T: std::fmt::Display>(v: &[T]) -> String {
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

/// What the synthetic FBX contains.
struct Parts<'a> {
    /// Meshes, skin and clusters (false: an animation-only file with bare nodes).
    geometry: bool,
    /// The take that spins the box and bends Bone1.
    animated: bool,
    /// Namespace-like prefix of the bone names (`rig:`).
    bone_prefix: &'a str,
    take: &'a str,
}

fn animated_fbx() -> String {
    fbx(Parts { geometry: true, animated: true, bone_prefix: "", take: "Mexer" })
}

fn fbx(p: Parts) -> String {
    // Box: 50 cm cube; vertex i has bits (x, y, z).
    let cube: Vec<i32> = (0..8).flat_map(|i| [if i & 1 != 0 { 25 } else { -25 }, if i & 2 != 0 { 25 } else { -25 }, if i & 4 != 0 { 25 } else { -25 }]).collect();
    let cube_faces = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    // Strip: two quads, 20 cm wide, 2 m tall.
    let strip = [-10, 0, 0, 10, 0, 0, -10, 100, 0, 10, 100, 0, -10, 200, 0, 10, 200, 0];
    let strip_faces = [[0, 1, 3, 2], [2, 3, 5, 4]];
    let poly = |faces: &[[i32; 4]]| -> Vec<i32> { faces.iter().flat_map(|f| [f[0], f[1], f[2], -f[3] - 1]).collect() };
    let cube_idx = poly(&cube_faces);
    let strip_idx = poly(&strip_faces);
    // Cluster matrices (column-major): TransformLink is the bone's bind world
    // matrix; Transform is the mesh→bone offset, inverse(link) × mesh bind.
    let identity = "1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1";
    let bone1_link = "1,0,0,0,0,1,0,0,0,0,1,0,0,100,0,1";
    let bone1_offset = "1,0,0,0,0,1,0,0,0,0,1,0,0,-100,0,1";

    let model = |id: u32, name: &str, kind: &str, t: [f64; 3]| {
        format!(
            "\tModel: {id}, \"Model::{name}\", \"{kind}\" {{\n\t\tVersion: 232\n\t\tProperties70:  {{\n\t\t\tP: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",{},{},{}\n\t\t}}\n\t}}\n",
            t[0], t[1], t[2]
        )
    };
    let geometry = |id: u32, name: &str, verts: &[i32], idx: &[i32]| {
        format!(
            "\tGeometry: {id}, \"Geometry::{name}\", \"Mesh\" {{\n\t\tVertices: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tPolygonVertexIndex: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tGeometryVersion: 124\n\t}}\n",
            verts.len(),
            list(verts),
            idx.len(),
            list(idx)
        )
    };
    let cluster = |id: u32, name: &str, idx: &[i32], w: &[f64], offset: &str, link: &str| {
        format!(
            "\tDeformer: {id}, \"SubDeformer::{name}\", \"Cluster\" {{\n\t\tVersion: 100\n\t\tIndexes: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tWeights: *{} {{\n\t\t\ta: {}\n\t\t}}\n\t\tTransform: *16 {{\n\t\t\ta: {offset}\n\t\t}}\n\t\tTransformLink: *16 {{\n\t\t\ta: {link}\n\t\t}}\n\t}}\n",
            idx.len(),
            list(idx),
            w.len(),
            list(w)
        )
    };
    let stack = |id: u32, name: &str, stop: i64| {
        format!(
            "\tAnimationStack: {id}, \"AnimStack::{name}\", \"\" {{\n\t\tProperties70:  {{\n\t\t\tP: \"LocalStop\", \"KTime\", \"Time\", \"\",{stop}\n\t\t\tP: \"ReferenceStop\", \"KTime\", \"Time\", \"\",{stop}\n\t\t}}\n\t}}\n"
        )
    };
    let curve_node = |id: u32| {
        format!(
            "\tAnimationCurveNode: {id}, \"AnimCurveNode::R\", \"\" {{\n\t\tProperties70:  {{\n\t\t\tP: \"d|X\", \"Number\", \"\", \"A\",0\n\t\t\tP: \"d|Y\", \"Number\", \"\", \"A\",0\n\t\t\tP: \"d|Z\", \"Number\", \"\", \"A\",0\n\t\t}}\n\t}}\n"
        )
    };
    let curve = |id: u32, to: f64| {
        format!(
            "\tAnimationCurve: {id}, \"AnimCurve::\", \"\" {{\n\t\tDefault: 0\n\t\tKeyVer: 4008\n\t\tKeyTime: *2 {{\n\t\t\ta: 0,{KTIME_PER_SECOND}\n\t\t}}\n\t\tKeyValueFloat: *2 {{\n\t\t\ta: 0,{to}\n\t\t}}\n\t\tKeyAttrFlags: *1 {{\n\t\t\ta: 4\n\t\t}}\n\t\tKeyAttrDataFloat: *4 {{\n\t\t\ta: 0,0,0,0\n\t\t}}\n\t\tKeyAttrRefCount: *1 {{\n\t\t\ta: 2\n\t\t}}\n\t}}\n"
        )
    };

    let mut s = String::new();
    s.push_str("; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXHeaderVersion: 1003\n\tFBXVersion: 7400\n}\n");
    s.push_str("GlobalSettings:  {\n\tVersion: 1000\n\tProperties70:  {\n");
    for (k, v) in [("UpAxis", 1), ("UpAxisSign", 1), ("FrontAxis", 2), ("FrontAxisSign", 1), ("CoordAxis", 0), ("CoordAxisSign", 1)] {
        writeln!(s, "\t\tP: \"{k}\", \"int\", \"Integer\", \"\",{v}").unwrap();
    }
    s.push_str("\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",1\n\t\tP: \"TimeMode\", \"enum\", \"\", \"\",6\n\t}\n}\n");
    s.push_str("Objects:  {\n");
    let mut ids = vec![0, 2001, 3001, 3002];
    if p.geometry {
        s += &geometry(1001, "Box", &cube, &cube_idx);
        s += &geometry(1002, "Strip", &strip, &strip_idx);
        s += &model(2002, "Tentacle", "Mesh", [0.0, 0.0, 0.0]);
        s.push_str("\tDeformer: 4001, \"Deformer::Skin\", \"Skin\" {\n\t\tVersion: 101\n\t}\n");
        s += &cluster(4002, "Cluster0", &[0, 1, 2, 3], &[1.0, 1.0, 0.5, 0.5], identity, identity);
        s += &cluster(4003, "Cluster1", &[2, 3, 4, 5], &[0.5, 0.5, 1.0, 1.0], bone1_offset, bone1_link);
        ids.extend([1001, 1002, 2002, 4001, 4002, 4003]);
    }
    s += &model(2001, "Spinner", if p.geometry { "Mesh" } else { "Null" }, [200.0, 0.0, 0.0]);
    s += &model(3001, &format!("{}Bone0", p.bone_prefix), "LimbNode", [0.0, 0.0, 0.0]);
    s += &model(3002, &format!("{}Bone1", p.bone_prefix), "LimbNode", [0.0, 100.0, 0.0]);
    if p.animated {
        s += &stack(5000, p.take, KTIME_PER_SECOND);
        s.push_str("\tAnimationLayer: 5001, \"AnimLayer::BaseLayer\", \"\" {\n\t}\n");
        s += &curve_node(5002);
        s += &curve(5003, 90.0);
        s += &curve_node(5004);
        s += &curve(5005, 45.0);
        ids.extend([5000, 5001, 5002, 5003, 5004, 5005]);
    }
    if !(p.animated && p.take == "Take 001") {
        s += &stack(6000, "Take 001", KTIME_PER_SECOND);
        s.push_str("\tAnimationLayer: 6001, \"AnimLayer::BaseLayer\", \"\" {\n\t}\n");
        ids.extend([6000, 6001]);
    }
    s.push_str("}\nConnections:  {\n");
    for (a, b) in [
        (2001, 0),
        (1001, 2001),
        (2002, 0),
        (1002, 2002),
        (3001, 0),
        (3002, 3001),
        (4001, 1002),
        (4002, 4001),
        (4003, 4001),
        (3001, 4002),
        (3002, 4003),
        (5001, 5000),
        (5002, 5001),
        (5004, 5001),
        (6001, 6000),
    ]
    .into_iter()
    .filter(|(a, b)| ids.contains(a) && ids.contains(b))
    {
        writeln!(s, "\tC: \"OO\",{a},{b}").unwrap();
    }
    for (a, b, prop) in [(5002, 2001, "Lcl Rotation"), (5003, 5002, "d|Y"), (5004, 3002, "Lcl Rotation"), (5005, 5004, "d|Z")]
        .into_iter()
        .filter(|(a, b, _)| ids.contains(a) && ids.contains(b))
    {
        writeln!(s, "\tC: \"OP\",{a},{b}, \"{prop}\"").unwrap();
    }
    s.push_str("}\n");
    s
}

/// Rest-pose world matrix of every node of the GLB.
fn world_matrices(doc: &gltf::Document) -> Vec<Mat4> {
    let mut world = vec![Mat4::IDENTITY; doc.nodes().count()];
    fn visit(n: gltf::Node, parent: Mat4, world: &mut [Mat4]) {
        let m = parent * Mat4::from_cols_array_2d(&n.transform().matrix());
        world[n.index()] = m;
        for c in n.children() {
            visit(c, m, world);
        }
    }
    for root in doc.default_scene().unwrap().nodes() {
        visit(root, Mat4::IDENTITY, &mut world);
    }
    world
}

#[test]
fn animated_fbx_exports_rig_and_clip() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("anim.fbx"), animated_fbx()).unwrap();
    let files = vec![FileEntry { path: "anim.fbx".into(), hash: String::new(), size: 1 }];
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).expect("pipeline");
    let m = &res.manifest;
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);

    // Only the take that moves is exported.
    assert_eq!(m.animations.len(), 1, "{:?}", m.animations);
    let clip = &m.animations[0];
    assert_eq!(clip.name, "Mexer");
    assert!((clip.duration_secs - 1.0).abs() < 0.05, "duration {}", clip.duration_secs);
    assert_eq!(clip.fps, 30.0);
    assert_eq!(clip.frames, 31);
    let stats = m.stats.as_ref().unwrap();
    assert_eq!(stats.animations, 1);
    assert!(stats.has_skin);

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).expect("valid glb");
    let buffers = gltf::import_buffers(&g.document, None, g.blob.clone()).unwrap();
    let doc = &g.document;
    assert_eq!(doc.default_scene().unwrap().nodes().count(), 1, "single animation root");
    assert_eq!(doc.animations().count(), 1);
    let skin = doc.skins().next().expect("skin");
    let joint_names: Vec<&str> = skin.joints().map(|j| j.name().unwrap_or("")).collect();
    assert_eq!(joint_names, ["Bone0", "Bone1"]);

    // Rest pose rebuilt from the GLB (hierarchy + skinning) must match the
    // world-space geometry the rest of the pipeline used (stats bounds, metres).
    let world = world_matrices(doc);
    let ibm: Vec<Mat4> = skin
        .reader(|b| Some(&buffers[b.index()]))
        .read_inverse_bind_matrices()
        .unwrap()
        .map(|m| Mat4::from_cols_array_2d(&m))
        .collect();
    let joints_world: Vec<Mat4> = skin.joints().map(|j| world[j.index()]).collect();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        for prim in mesh.primitives() {
            let r = prim.reader(|b| Some(&buffers[b.index()]));
            let positions: Vec<Vec3> = r.read_positions().unwrap().map(Vec3::from).collect();
            let rest: Vec<Vec3> = if node.skin().is_some() {
                let joints: Vec<[u16; 4]> = r.read_joints(0).unwrap().into_u16().collect();
                let weights: Vec<[f32; 4]> = r.read_weights(0).unwrap().into_f32().collect();
                positions
                    .iter()
                    .zip(joints.iter().zip(&weights))
                    .map(|(p, (j, w))| {
                        let skinned: Vec3 = (0..4).map(|k| (joints_world[j[k] as usize] * ibm[j[k] as usize]).transform_point3(*p) * w[k]).sum();
                        assert!(skinned.distance(*p) < 1e-3, "rest pose must equal bind pose: {skinned} vs {p}");
                        skinned
                    })
                    .collect()
            } else {
                positions.iter().map(|p| world[node.index()].transform_point3(*p)).collect()
            };
            for p in rest {
                min = min.min(p);
                max = max.max(p);
            }
        }
    }
    let (smin, smax) = (Vec3::from(stats.bounds_min), Vec3::from(stats.bounds_max));
    assert!(min.distance(smin) < 1e-3 && max.distance(smax) < 1e-3, "glb {min}..{max} vs stats {smin}..{smax}");
    // Centimetres converted: the box sits 2 m away, the strip is 2 m tall.
    assert!((smax.x - 2.25).abs() < 1e-3 && (smax.y - 2.0).abs() < 1e-3, "{smax}");

    // Final keys: spinner turned 90° around Y, Bone1 bent 45° around Z.
    let anim = doc.animations().next().unwrap();
    let last_rotation = |name: &str| -> Quat {
        let ch = anim
            .channels()
            .find(|c| c.target().node().name() == Some(name) && c.target().property() == gltf::animation::Property::Rotation)
            .unwrap_or_else(|| panic!("no rotation channel for {name}"));
        let out = match ch.reader(|b| Some(&buffers[b.index()])).read_outputs().unwrap() {
            gltf::animation::util::ReadOutputs::Rotations(r) => r.into_f32().last().unwrap(),
            _ => unreachable!(),
        };
        Quat::from_array(out)
    };
    assert!(last_rotation("Spinner").angle_between(Quat::from_rotation_y(90f32.to_radians())) < 1e-3);
    assert!(last_rotation("Bone1").angle_between(Quat::from_rotation_z(45f32.to_radians())) < 1e-3);
}

/// Animations exported separately: the model file has an empty take, the clip
/// lives in `anims/Shark@Bend.fbx` (no geometry, bones as `rig:BoneN`).
#[test]
fn separate_animation_files_animate_the_model() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("anims")).unwrap();
    let model = fbx(Parts { geometry: true, animated: false, bone_prefix: "", take: "" });
    let clip = fbx(Parts { geometry: false, animated: true, bone_prefix: "rig:", take: "Take 001" });
    std::fs::write(dir.path().join("Shark.fbx"), model).unwrap();
    std::fs::write(dir.path().join("anims/Shark@Bend.fbx"), clip).unwrap();
    let files: Vec<FileEntry> = ["Shark.fbx", "anims/Shark@Bend.fbx"]
        .iter()
        .map(|p| FileEntry { path: p.to_string(), hash: String::new(), size: 1 })
        .collect();
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).expect("pipeline");
    let m = &res.manifest;
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);
    assert_eq!(m.primary_file.as_deref(), Some("Shark.fbx"));
    assert_eq!(m.animations.len(), 1, "{:?}", m.animations);
    // Generic take name → named after the file (`Model@Clip`).
    assert_eq!(m.animations[0].name, "Bend");
    assert_eq!(m.animations[0].source.as_deref(), Some("anims/Shark@Bend.fbx"));
    assert_eq!(m.stats.as_ref().unwrap().animations, 1);

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    let anim = g.document.animations().next().expect("clip in the GLB");
    assert_eq!(anim.name(), Some("Bend"));
    // Channels drive the model's own nodes (matched across the `rig:` namespace).
    let mut targets: Vec<&str> = anim.channels().filter_map(|c| c.target().node().name()).collect();
    targets.sort();
    targets.dedup();
    assert!(targets.contains(&"Spinner") && targets.contains(&"Bone1"), "{targets:?}");
}

fn write_files(root: &std::path::Path, files: &[(&str, String)]) -> Vec<FileEntry> {
    files
        .iter()
        .map(|(path, content)| {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, content).unwrap();
            FileEntry { path: path.to_string(), hash: String::new(), size: content.len() as i64 }
        })
        .collect()
}

/// A version with only an animation file and no skeleton bound: the clips are
/// listed, nothing is rendered.
#[test]
fn animation_only_version_lists_its_clips() {
    let dir = tempfile::tempdir().unwrap();
    let clip = fbx(Parts { geometry: false, animated: true, bone_prefix: "", take: "Run" });
    let files = write_files(dir.path(), &[("Run.fbx", clip)]);
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).expect("pipeline");
    let m = &res.manifest;
    assert_eq!(m.kind, Some(hangar_core::PreviewKind::Animation));
    assert_eq!(m.primary_file.as_deref(), Some("Run.fbx"));
    assert_eq!(m.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Run"]);
    assert!(m.lods.is_empty() && m.skeletons.is_empty() && m.thumbnail_hash.is_none());
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);
}

/// Animation-only version bound to two reference skeletons: one whose bones
/// match (clips retargeted across the `rig:` namespace, the skeleton's own
/// take dropped) and one that shares no bone with the clip.
#[test]
fn animations_play_on_reference_skeletons() {
    let skel = tempfile::tempdir().unwrap();
    // The skeleton's model has a take of its own ("Mexer") that must not leak in.
    let manny = fbx(Parts { geometry: true, animated: true, bone_prefix: "", take: "Mexer" });
    let skel_files = write_files(skel.path(), &[("SKM_Manny.fbx", manny)]);
    let other = tempfile::tempdir().unwrap();
    let mut alien = fbx(Parts { geometry: true, animated: false, bone_prefix: "", take: "" });
    for name in ["Spinner", "Bone0", "Bone1", "Tentacle"] {
        alien = alien.replace(&format!("Model::{name}\""), &format!("Model::Alien{name}\""));
    }
    let other_files = write_files(other.path(), &[("Alien.fbx", alien)]);
    let skeletons = vec![
        SkeletonSource { slug: "ue5-manny".into(), name: "UE5 Manny".into(), root: skel.path().to_path_buf(), files: skel_files },
        SkeletonSource { slug: "alien".into(), name: "Alien".into(), root: other.path().to_path_buf(), files: other_files },
    ];

    let dir = tempfile::tempdir().unwrap();
    let clip = fbx(Parts { geometry: false, animated: true, bone_prefix: "rig:", take: "Take 001" });
    let config = r#"{"skeletons":["ue5-manny","alien"]}"#.to_string();
    let files = write_files(dir.path(), &[("anims/Manny@Bend.fbx", clip.clone()), (".hangar/animation.json", config)]);
    let input = ProcessInput { root: dir.path(), files: &files, skeletons: &skeletons };
    let res = process(&input, &PipelineConfig::default()).expect("pipeline");
    let m = &res.manifest;
    assert_eq!(m.kind, Some(hangar_core::PreviewKind::Animation));
    assert_eq!(m.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Bend"]);
    assert_eq!(m.skeletons.len(), 2);

    let manny = &m.skeletons[0];
    assert_eq!(manny.skeleton, "ue5-manny");
    assert_eq!(manny.error, None);
    assert_eq!(manny.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Bend"]);
    assert_eq!((manny.bones_matched, manny.bones_total), (3, 3));
    assert!(manny.stats.as_ref().unwrap().has_skin);
    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::SkeletonGlb("ue5-manny".into())).expect("skeleton glb");
    assert_eq!(manny.glb_hash.as_deref(), Some(glb.hash.as_str()));
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    assert_eq!(g.document.animations().count(), 1, "only the bound clip, not the skeleton's own take");
    assert!(g.document.skins().next().is_some());
    assert!(g.document.meshes().count() >= 2, "the skeleton's mesh is in the GLB");
    let anim = g.document.animations().next().unwrap();
    assert_eq!(anim.name(), Some("Bend"));
    let targets: Vec<&str> = anim.channels().filter_map(|c| c.target().node().name()).collect();
    assert!(targets.contains(&"Bone1") && targets.contains(&"Spinner"), "{targets:?}");

    let alien = &m.skeletons[1];
    assert!(alien.glb_hash.is_none() && alien.animations.is_empty());
    assert!(alien.error.as_deref().is_some_and(|e| e.contains("0 of 3")), "{:?}", alien.error);
    assert!(m.warnings.iter().any(|w| w.starts_with("skeleton Alien:")), "{:?}", m.warnings);

    // Thumbnail of the posed skeleton.
    let thumb = m.thumbnail_hash.as_deref().expect("thumbnail");
    assert!(res.artifacts.iter().any(|a| a.hash == thumb && a.role == ArtifactRole::Thumbnail));

    // A version with its own mesh keeps its animations on it.
    let model = fbx(Parts { geometry: true, animated: false, bone_prefix: "", take: "" });
    let files = write_files(dir.path(), &[("Manny.fbx", model), ("anims/Manny@Bend.fbx", clip)]);
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &skeletons }, &PipelineConfig::default()).unwrap();
    assert_eq!(res.manifest.kind, Some(hangar_core::PreviewKind::Model));
    assert!(res.manifest.skeletons.is_empty());
    assert_eq!(res.manifest.animations.len(), 1);
    assert!(res.manifest.warnings.iter().any(|w| w.contains("ignored")), "{:?}", res.manifest.warnings);
}

/// An animation package ("Rocket Launcher"): several animation files plus the
/// weapon prop. The prop shares no bone with the clips, so it is not their
/// mesh: the package is previewed on the reference skeleton with every clip.
#[test]
fn animation_package_with_a_prop_plays_on_the_skeleton() {
    let skel = tempfile::tempdir().unwrap();
    let skel_files = write_files(skel.path(), &[("SKM_Manny.fbx", fbx(Parts { geometry: true, animated: false, bone_prefix: "", take: "" }))]);
    let skeletons = vec![SkeletonSource { slug: "ue5-manny".into(), name: "UE5 Manny".into(), root: skel.path().to_path_buf(), files: skel_files }];

    let mut prop = fbx(Parts { geometry: true, animated: false, bone_prefix: "", take: "" });
    for name in ["Spinner", "Bone0", "Bone1", "Tentacle"] {
        prop = prop.replace(&format!("Model::{name}\""), &format!("Model::SM_Launcher_{name}\""));
    }
    let dir = tempfile::tempdir().unwrap();
    let files = write_files(
        dir.path(),
        &[
            ("RocketLauncher/Anims/A_Fire.fbx", fbx(Parts { geometry: false, animated: true, bone_prefix: "", take: "A_Fire" })),
            ("RocketLauncher/Anims/A_Reload.fbx", fbx(Parts { geometry: false, animated: true, bone_prefix: "", take: "Unreal Take" })),
            ("RocketLauncher/Props/SM_RocketLauncher.fbx", prop),
            (".hangar/animation.json", r#"{"skeletons":["ue5-manny"]}"#.into()),
        ],
    );
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &skeletons }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert_eq!(m.kind, Some(hangar_core::PreviewKind::Animation), "{:?}", m.warnings);
    assert_eq!(m.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["A_Fire", "A_Reload"]);
    assert!(m.warnings.iter().any(|w| w.contains("SM_RocketLauncher.fbx")), "{:?}", m.warnings);
    let manny = &m.skeletons[0];
    assert_eq!(manny.error, None);
    assert_eq!(manny.animations.len(), 2);
    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::SkeletonGlb("ue5-manny".into())).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    assert_eq!(g.document.animations().count(), 2);
}

/// Binary buffer + accessors for a hand-written glTF.
#[derive(Default)]
struct GltfBin {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl GltfBin {
    /// Appends little-endian data and returns the accessor index.
    fn add(&mut self, bytes: Vec<u8>, component: u32, count: usize, ty: &str, minmax: Option<(&str, &str)>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        self.views.push(format!(r#"{{"buffer":0,"byteOffset":{},"byteLength":{}}}"#, self.bin.len(), bytes.len()));
        self.bin.extend(bytes);
        let mm = minmax.map(|(a, b)| format!(r#","min":{a},"max":{b}"#)).unwrap_or_default();
        self.accessors.push(format!(
            r#"{{"bufferView":{},"componentType":{component},"count":{count},"type":"{ty}"{mm}}}"#,
            self.views.len() - 1
        ));
        self.accessors.len() - 1
    }
    fn f32s(&mut self, v: &[f32], count: usize, ty: &str, minmax: Option<(&str, &str)>) -> usize {
        self.add(v.iter().flat_map(|f| f.to_le_bytes()).collect(), 5126, count, ty, minmax)
    }
    fn u16s(&mut self, v: &[u16], count: usize, ty: &str) -> usize {
        self.add(v.iter().flat_map(|f| f.to_le_bytes()).collect(), 5123, count, ty, None)
    }
}

/// Skinned glTF exported in centimetres under a scaled armature (like Blender
/// / Unity exports), plus a rigid box: a CUBICSPLINE clip bends Bone1 45°
/// around Z and a STEP clip makes the box hop. The GLB must keep the rig,
/// skin and both clips, with the rest pose in metres.
#[test]
fn gltf_animations_are_exported() {
    let mut b = GltfBin::default();
    let strip_pos = b.f32s(
        &[-10.0, 0.0, 0.0, 10.0, 0.0, 0.0, -10.0, 100.0, 0.0, 10.0, 100.0, 0.0, -10.0, 200.0, 0.0, 10.0, 200.0, 0.0],
        6,
        "VEC3",
        Some(("[-10,0,0]", "[10,200,0]")),
    );
    let strip_idx = b.u16s(&[0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4], 12, "SCALAR");
    let joints = b.u16s(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0], 6, "VEC4");
    let weights = b.f32s(
        &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        6,
        "VEC4",
        None,
    );
    // inverse(joint world) × armature world: identity and a 1 m (100 cm) drop.
    let ibm = b.f32s(
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, //
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -100.0, 0.0, 1.0,
        ],
        2,
        "MAT4",
        None,
    );
    let box_pos = b.f32s(&[0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.0], 3, "VEC3", Some(("[0,0,0]", "[0.5,0.5,0]")));
    let bend_t = b.f32s(&[0.0, 1.0], 2, "SCALAR", Some(("[0]", "[1]")));
    let q = Quat::from_rotation_z(45f32.to_radians());
    #[rustfmt::skip]
    let bend_v = b.f32s(
        &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
          0.0, 0.0, 0.0, 0.0, q.x, q.y, q.z, q.w, 0.0, 0.0, 0.0, 0.0],
        6,
        "VEC4",
        None,
    );
    let hop_t = b.f32s(&[0.0, 0.5, 1.0], 3, "SCALAR", Some(("[0]", "[1]")));
    let hop_v = b.f32s(&[2.0, 0.0, 0.0, 2.0, 1.0, 0.0, 2.0, 0.0, 0.0], 3, "VEC3", None);

    let gltf = format!(
        r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0, "scenes": [{{ "nodes": [0, 4] }}],
  "nodes": [
    {{ "name": "Armature", "scale": [0.01, 0.01, 0.01], "children": [1, 3] }},
    {{ "name": "Bone0", "children": [2] }},
    {{ "name": "Bone1", "translation": [0, 100, 0] }},
    {{ "name": "Strip", "mesh": 0, "skin": 0 }},
    {{ "name": "Box", "mesh": 1, "translation": [2, 0, 0] }}
  ],
  "meshes": [
    {{ "primitives": [{{ "attributes": {{ "POSITION": {strip_pos}, "JOINTS_0": {joints}, "WEIGHTS_0": {weights} }}, "indices": {strip_idx} }}] }},
    {{ "primitives": [{{ "attributes": {{ "POSITION": {box_pos} }} }}] }}
  ],
  "skins": [{{ "joints": [1, 2], "inverseBindMatrices": {ibm} }}],
  "animations": [
    {{ "name": "Bend", "samplers": [{{ "input": {bend_t}, "output": {bend_v}, "interpolation": "CUBICSPLINE" }}],
       "channels": [{{ "sampler": 0, "target": {{ "node": 2, "path": "rotation" }} }}] }},
    {{ "name": "Hop", "samplers": [{{ "input": {hop_t}, "output": {hop_v}, "interpolation": "STEP" }}],
       "channels": [{{ "sampler": 0, "target": {{ "node": 4, "path": "translation" }} }}] }}
  ],
  "buffers": [{{ "uri": "Rig.bin", "byteLength": {len} }}],
  "bufferViews": [{views}],
  "accessors": [{accessors}]
}}"#,
        len = b.bin.len(),
        views = b.views.join(","),
        accessors = b.accessors.join(","),
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Rig.bin"), &b.bin).unwrap();
    std::fs::write(dir.path().join("Rig.gltf"), gltf).unwrap();
    let files: Vec<FileEntry> =
        ["Rig.gltf", "Rig.bin"].iter().map(|p| FileEntry { path: p.to_string(), hash: String::new(), size: 1 }).collect();
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).expect("pipeline");
    let m = &res.manifest;
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);
    assert_eq!(m.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Bend", "Hop"]);
    assert!((m.animations[0].duration_secs - 1.0).abs() < 1e-4 && m.animations[0].fps == 30.0);
    let stats = m.stats.as_ref().unwrap();
    assert!(stats.has_skin && stats.animations == 2);
    // Centimetres under the 0.01 armature: the strip is 2 m tall, the box ends at x = 2.5.
    assert!((stats.bounds_max[1] - 2.0).abs() < 1e-4 && (stats.bounds_max[0] - 2.5).abs() < 1e-4, "{:?}", stats.bounds_max);

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    let buffers = gltf::import_buffers(&g.document, None, g.blob.clone()).unwrap();
    let doc = &g.document;
    assert_eq!(doc.default_scene().unwrap().nodes().count(), 1, "single animation root");
    assert_eq!(doc.animations().count(), 2);
    let skin = doc.skins().next().expect("skin");
    assert_eq!(skin.joints().map(|j| j.name().unwrap_or("")).collect::<Vec<_>>(), ["Bone0", "Bone1"]);

    // Rest pose rebuilt from the GLB equals the world-space geometry (metres).
    let world = world_matrices(doc);
    let ibm: Vec<Mat4> = skin
        .reader(|b| Some(&buffers[b.index()]))
        .read_inverse_bind_matrices()
        .unwrap()
        .map(|m| Mat4::from_cols_array_2d(&m))
        .collect();
    let joints_world: Vec<Mat4> = skin.joints().map(|j| world[j.index()]).collect();
    let mut top = f32::MIN;
    for node in doc.nodes().filter(|n| n.skin().is_some()) {
        for prim in node.mesh().unwrap().primitives() {
            let r = prim.reader(|b| Some(&buffers[b.index()]));
            let joints: Vec<[u16; 4]> = r.read_joints(0).unwrap().into_u16().collect();
            let weights: Vec<[f32; 4]> = r.read_weights(0).unwrap().into_f32().collect();
            for (p, (j, w)) in r.read_positions().unwrap().map(Vec3::from).zip(joints.iter().zip(&weights)) {
                let skinned: Vec3 = (0..4).map(|k| (joints_world[j[k] as usize] * ibm[j[k] as usize]).transform_point3(p) * w[k]).sum();
                assert!(skinned.distance(p) < 1e-4, "rest pose must equal bind pose: {skinned} vs {p}");
                top = top.max(skinned.y);
            }
        }
    }
    assert!((top - 2.0).abs() < 1e-4, "strip top {top}");

    let anim = |name: &str| doc.animations().find(|a| a.name() == Some(name)).unwrap();
    // CUBICSPLINE: sampled, ends at 45° around Z.
    let bend = anim("Bend").channels().next().unwrap();
    assert_eq!(bend.target().node().name(), Some("Bone1"));
    let rotations: Vec<Quat> = match bend.reader(|b| Some(&buffers[b.index()])).read_outputs().unwrap() {
        gltf::animation::util::ReadOutputs::Rotations(r) => r.into_f32().map(Quat::from_array).collect(),
        _ => unreachable!(),
    };
    assert!(rotations.len() > 2, "cubic spline sampled into linear keys");
    assert!(rotations.last().unwrap().angle_between(q) < 1e-3);
    // STEP: holds until 0.5 s, then jumps.
    let hop = anim("Hop").channels().next().unwrap();
    let r = hop.reader(|b| Some(&buffers[b.index()]));
    let times: Vec<f32> = r.read_inputs().unwrap().collect();
    let values: Vec<[f32; 3]> = match r.read_outputs().unwrap() {
        gltf::animation::util::ReadOutputs::Translations(t) => t.collect(),
        _ => unreachable!(),
    };
    let at = |t: f32| values[times.iter().position(|x| (x - t).abs() < 1e-5).unwrap_or_else(|| panic!("no key at {t}: {times:?}"))];
    assert_eq!(at(0.499), [2.0, 0.0, 0.0]);
    assert_eq!(at(0.5), [2.0, 1.0, 0.0]);
}
