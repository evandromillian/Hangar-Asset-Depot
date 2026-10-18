//! End-to-end test: a procedurally generated "tank" (OBJ + MTL + textures in
//! Desert / Woodland folders) goes through the whole pipeline.

use hangar_core::{FileEntry, LodSource, PreviewKind, TextureSlot, MATERIAL_CONFIG_PATH};
use hangar_pipeline::{process, ArtifactRole, PipelineConfig, ProcessInput};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

struct ObjWriter {
    out: String,
    v: usize,
    vt: usize,
}

impl ObjWriter {
    fn new() -> Self {
        Self { out: String::from("mtllib tank.mtl\n"), v: 0, vt: 0 }
    }

    /// A box made of `n`x`n` quads per face.
    fn grid_box(&mut self, name: &str, mat: &str, center: [f32; 3], half: [f32; 3], n: usize) {
        writeln!(self.out, "o {name}\nusemtl {mat}").unwrap();
        // faces: (axis, sign)
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let (u_ax, v_ax) = ((axis + 1) % 3, (axis + 2) % 3);
                let base_v = self.v;
                let base_t = self.vt;
                for j in 0..=n {
                    for i in 0..=n {
                        let fu = i as f32 / n as f32 * 2.0 - 1.0;
                        let fv = j as f32 / n as f32 * 2.0 - 1.0;
                        let mut p = center;
                        p[axis] += sign * half[axis];
                        p[u_ax] += fu * half[u_ax];
                        p[v_ax] += fv * half[v_ax];
                        writeln!(self.out, "v {} {} {}", p[0], p[1], p[2]).unwrap();
                        writeln!(self.out, "vt {} {}", i as f32 / n as f32, j as f32 / n as f32).unwrap();
                        self.v += 1;
                        self.vt += 1;
                    }
                }
                for j in 0..n {
                    for i in 0..n {
                        let a = j * (n + 1) + i;
                        let (a, b, c, d) = (a, a + 1, a + n + 2, a + n + 1);
                        let idx = |k: usize| format!("{}/{}", base_v + k + 1, base_t + k + 1);
                        if sign > 0.0 {
                            writeln!(self.out, "f {} {} {} {}", idx(a), idx(b), idx(c), idx(d)).unwrap();
                        } else {
                            writeln!(self.out, "f {} {} {} {}", idx(a), idx(d), idx(c), idx(b)).unwrap();
                        }
                    }
                }
            }
        }
    }

    /// An open cylinder along `axis` (0 = X, 1 = Y, 2 = Z).
    #[allow(clippy::too_many_arguments)]
    fn cylinder(&mut self, name: &str, mat: &str, center: [f32; 3], radius: f32, len: f32, axis: usize, seg: usize, rings: usize) {
        writeln!(self.out, "o {name}\nusemtl {mat}").unwrap();
        let base_v = self.v;
        let (a1, a2) = ((axis + 1) % 3, (axis + 2) % 3);
        for r in 0..=rings {
            let t = r as f32 / rings as f32 - 0.5;
            for s in 0..seg {
                let ang = s as f32 / seg as f32 * std::f32::consts::TAU;
                let mut p = center;
                p[axis] += t * len;
                p[a1] += ang.cos() * radius;
                p[a2] += ang.sin() * radius;
                writeln!(self.out, "v {} {} {}", p[0], p[1], p[2]).unwrap();
                self.v += 1;
            }
        }
        for r in 0..rings {
            for s in 0..seg {
                let a = base_v + r * seg + s + 1;
                let b = base_v + r * seg + (s + 1) % seg + 1;
                let c = b + seg;
                let d = a + seg;
                writeln!(self.out, "f {a} {b} {c} {d}").unwrap();
            }
        }
    }
}

fn write_png(path: &Path, color: [u8; 3]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = image::RgbaImage::from_fn(64, 64, |x, y| {
        let k = if (x / 8 + y / 8) % 2 == 0 { 1.0 } else { 0.8 };
        image::Rgba([(color[0] as f32 * k) as u8, (color[1] as f32 * k) as u8, (color[2] as f32 * k) as u8, 255])
    });
    img.save(path).unwrap();
}

fn make_tank(dir: &Path) -> Vec<FileEntry> {
    let mut obj = ObjWriter::new();
    obj.grid_box("Hull", "M_Tank_Body", [0.0, 0.6, 0.0], [1.6, 0.5, 3.2], 16);
    obj.cylinder("Turret", "M_Tank_Body", [0.0, 1.4, -0.3], 1.0, 0.6, 1, 48, 6);
    obj.cylinder("Barrel", "M_Tank_Barrel", [0.0, 1.45, 2.0], 0.12, 3.0, 2, 24, 12);
    std::fs::write(dir.join("tank.obj"), &obj.out).unwrap();
    std::fs::write(
        dir.join("tank.mtl"),
        "newmtl M_Tank_Body\nKd 1 1 1\nmap_Kd Textures/Desert/T_Tank_Body_D.png\nnorm Textures/T_Tank_Body_N.png\n\n\
         newmtl M_Tank_Barrel\nKd 0.2 0.22 0.2\nNs 200\n",
    )
    .unwrap();
    write_png(&dir.join("Textures/Desert/T_Tank_Body_D.png"), [200, 170, 110]);
    write_png(&dir.join("Textures/Woodland/T_Tank_Body_D.png"), [70, 95, 50]);
    write_png(&dir.join("Textures/T_Tank_Body_N.png"), [128, 128, 255]);
    std::fs::write(dir.join("README.txt"), "bought on a store").unwrap();

    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let bytes = std::fs::read(e.path()).unwrap();
            FileEntry {
                path: e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"),
                hash: blake3::hash(&bytes).to_hex().to_string(),
                size: bytes.len() as i64,
            }
        })
        .collect()
}

#[test]
fn processes_tank_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let files = make_tank(dir.path());
    let cfg = PipelineConfig { thumbnail_size: 256, ..Default::default() };
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &cfg).expect("pipeline");
    let m = &res.manifest;

    assert_eq!(m.kind, Some(PreviewKind::Model));
    assert_eq!(m.importer.as_deref(), Some("obj"));
    assert_eq!(m.primary_file.as_deref(), Some("tank.obj"));

    // LODs
    assert!(m.lods.len() >= 3, "lods: {:?}", m.lods);
    assert_eq!(m.lods[0].source, LodSource::Authored);
    assert!(m.lods[0].triangles > 3000, "lod0 tris {}", m.lods[0].triangles);
    assert!(m.lods[1..].iter().all(|l| l.source == LodSource::Generated));
    for w in m.lods.windows(2) {
        assert!(w[1].triangles < w[0].triangles);
        assert!(w[1].ratio < w[0].ratio);
    }
    assert!((m.lods[1].ratio - 0.5).abs() < 0.2, "lod1 ratio {}", m.lods[1].ratio);

    // Textures resolved
    let body = m.materials.iter().find(|x| x.name == "M_Tank_Body").unwrap();
    let bc = body.textures.iter().find(|t| t.slot == TextureSlot::BaseColor).unwrap();
    assert_eq!(bc.path.as_deref(), Some("Textures/Desert/T_Tank_Body_D.png"));
    assert_eq!(bc.resolved_by, "exact");
    assert!(body.textures.iter().any(|t| t.slot == TextureSlot::Normal && t.path.is_some()));

    // Variants
    assert_eq!(m.variants.len(), 2, "{:?}", m.variants);
    assert_eq!(m.variants[0].name, "Desert");
    assert!(m.variants[0].is_default);
    let wood = &m.variants[1];
    assert_eq!(wood.name, "Woodland");
    assert_eq!(wood.detected_by, "folder");
    assert_eq!(wood.overrides.len(), 1);
    assert_eq!(wood.overrides[0].material, "M_Tank_Body");
    assert_eq!(wood.overrides[0].slot, TextureSlot::BaseColor);
    assert_eq!(wood.overrides[0].path, "Textures/Woodland/T_Tank_Body_D.png");

    // Every hash referenced by the manifest exists as an artifact; no duplicates.
    let hashes: HashSet<&str> = res.artifacts.iter().map(|a| a.hash.as_str()).collect();
    assert_eq!(hashes.len(), res.artifacts.len());
    for l in &m.lods {
        assert!(hashes.contains(l.glb_hash.as_str()));
    }
    assert!(hashes.contains(wood.overrides[0].preview_hash.as_str()));
    assert!(hashes.contains(m.thumbnail_hash.as_deref().unwrap()));
    assert_eq!(m.textures.len(), 3);
    assert!(m.textures.iter().all(|t| t.preview_hash.is_some() && t.width == 64));

    // GLBs are valid and keep material names.
    for a in &res.artifacts {
        if let ArtifactRole::LodGlb(level) = a.role {
            let g = gltf::Gltf::from_slice(&a.bytes).expect("valid glb");
            let names: Vec<&str> = g.materials().filter_map(|m| m.name()).collect();
            assert!(names.contains(&"M_Tank_Body"), "lod{level}: {names:?}");
            assert!(names.contains(&"M_Tank_Barrel"));
            assert_eq!(g.images().count(), 2, "base color + normal");
            let mat = g.materials().find(|m| m.name() == Some("M_Tank_Body")).unwrap();
            assert!(mat.pbr_metallic_roughness().base_color_texture().is_some());
            assert!(mat.normal_texture().is_some());
        }
    }

    // Thumbnail decodes and is not just background.
    let thumb = res.artifacts.iter().find(|a| a.role == ArtifactRole::Thumbnail).unwrap();
    let img = image::load_from_memory(&thumb.bytes).unwrap().to_rgba8();
    assert_eq!(img.dimensions(), (256, 256));
    let center = img.get_pixel(128, 128).0;
    let corner = img.get_pixel(2, 2).0;
    assert_ne!(center, corner, "model should cover the centre of the thumbnail");

    let s = m.stats.as_ref().unwrap();
    assert!(s.has_uvs);
    let dims = s.dimensions();
    assert!(dims[2] > 6.0 && dims[1] > 1.0, "dims {dims:?}");
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);
}

/// `.hangar/materials.json` assigns a texture to the untextured barrel and
/// defines a manual skin; both must reach the manifest and the GLB.
#[test]
fn manual_material_config() {
    let dir = tempfile::tempdir().unwrap();
    make_tank(dir.path());
    write_png(&dir.path().join("Textures/Snow/Body_Snow.png"), [235, 235, 240]);
    write_png(&dir.path().join("Textures/Barrel_Steel.png"), [90, 92, 95]);
    std::fs::create_dir_all(dir.path().join(".hangar")).unwrap();
    std::fs::write(
        dir.path().join(MATERIAL_CONFIG_PATH),
        r#"{
            "materials": { "M_Tank_Barrel": { "base_color": "Textures/Barrel_Steel.png" } },
            "skins": [ { "name": "Neve", "materials": { "M_Tank_Body": { "base_color": "Textures/Snow/Body_Snow.png" } } } ]
        }"#,
    )
    .unwrap();
    let files = entries(dir.path());
    assert!(files.iter().any(|f| f.path == MATERIAL_CONFIG_PATH && hangar_pipeline::is_relevant(&f.path)));
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);

    let barrel = m.materials.iter().find(|x| x.name == "M_Tank_Barrel").unwrap();
    let bc = barrel.textures.iter().find(|t| t.slot == TextureSlot::BaseColor).unwrap();
    assert_eq!(bc.path.as_deref(), Some("Textures/Barrel_Steel.png"));
    assert_eq!(bc.resolved_by, "manual");

    let snow = m.variants.iter().find(|v| v.name == "Neve").expect("manual skin");
    assert_eq!(snow.detected_by, "manual");
    assert!(!snow.is_default);
    assert_eq!(snow.overrides.len(), 1);
    assert_eq!(snow.overrides[0].material, "M_Tank_Body");
    let hashes: HashSet<&str> = res.artifacts.iter().map(|a| a.hash.as_str()).collect();
    assert!(hashes.contains(snow.overrides[0].preview_hash.as_str()));
    // Automatic variants are kept next to the manual skin.
    assert!(m.variants.iter().any(|v| v.name == "Woodland"));

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    let mat = g.materials().find(|x| x.name() == Some("M_Tank_Barrel")).unwrap();
    assert!(mat.pbr_metallic_roughness().base_color_texture().is_some(), "barrel texture baked into the GLB");
}

/// Specular maps: a `_S` sibling of the body maps is picked up automatically,
/// one assigned by hand in `.hangar/materials.json` reaches the barrel, and
/// both are exported as `KHR_materials_specular` with the reflectance factor.
#[test]
fn specular_maps() {
    let dir = tempfile::tempdir().unwrap();
    make_tank(dir.path());
    write_png(&dir.path().join("Textures/T_Tank_Body_S.png"), [128, 128, 128]);
    write_png(&dir.path().join("Textures/Barrel_Spec.png"), [200, 200, 200]);
    std::fs::create_dir_all(dir.path().join(".hangar")).unwrap();
    std::fs::write(
        dir.path().join(MATERIAL_CONFIG_PATH),
        r#"{ "materials": { "M_Tank_Barrel": { "specular": "Textures/Barrel_Spec.png" } } }"#,
    )
    .unwrap();
    let files = entries(dir.path());
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert!(m.warnings.is_empty(), "warnings: {:?}", m.warnings);
    let spec = |name: &str| {
        let mat = m.materials.iter().find(|x| x.name == name).unwrap();
        mat.textures.iter().find(|t| t.slot == TextureSlot::Specular).map(|t| (t.path.clone().unwrap(), t.resolved_by.clone()))
    };
    assert_eq!(spec("M_Tank_Body").map(|(p, _)| p).as_deref(), Some("Textures/T_Tank_Body_S.png"));
    assert_eq!(spec("M_Tank_Barrel"), Some(("Textures/Barrel_Spec.png".to_string(), "manual".to_string())));

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    assert!(g.extensions_used().any(|e| e == "KHR_materials_specular"));
    for name in ["M_Tank_Body", "M_Tank_Barrel"] {
        let mat = g.materials().find(|x| x.name() == Some(name)).unwrap();
        let s = mat.specular().unwrap_or_else(|| panic!("{name}: no KHR_materials_specular"));
        assert!(s.specular_color_texture().is_some(), "{name}");
        assert_eq!(s.specular_color_factor(), [2.0, 2.0, 2.0]);
        assert_eq!(s.specular_factor(), 1.0);
    }
}

#[test]
fn image_only_version() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("T_Rock_D.png"), [120, 120, 120]);
    write_png(&dir.path().join("T_Rock_N.png"), [128, 128, 255]);
    let files: Vec<FileEntry> = ["T_Rock_D.png", "T_Rock_N.png"]
        .iter()
        .map(|p| FileEntry { path: p.to_string(), hash: String::new(), size: 10 })
        .collect();
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    assert_eq!(res.manifest.kind, Some(PreviewKind::Image));
    assert_eq!(res.manifest.primary_file.as_deref(), Some("T_Rock_D.png"));
    assert!(res.manifest.thumbnail_hash.is_some());
    assert!(res.manifest.lods.is_empty());
}

#[test]
fn missing_importer_degrades_to_images() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tank.blend"), b"BLENDER-v300").unwrap();
    write_png(&dir.path().join("T_Tank_D.png"), [1, 2, 3]);
    let files = vec![
        FileEntry { path: "tank.blend".into(), hash: String::new(), size: 12 },
        FileEntry { path: "T_Tank_D.png".into(), hash: String::new(), size: 10 },
    ];
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    assert_eq!(res.manifest.kind, Some(PreviewKind::Image));
    assert!(res.manifest.warnings[0].contains("3D preview unavailable"));

    // A lone unsupported model is an error.
    let files = vec![FileEntry { path: "tank.blend".into(), hash: String::new(), size: 12 }];
    assert!(process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).is_err());
}

/// Runs on a real FBX: `HANGAR_SAMPLE_FBX=/path/model.fbx cargo test -p hangar-pipeline -- --ignored`.
#[test]
#[ignore]
fn sample_fbx_from_env() {
    let Ok(p) = std::env::var("HANGAR_SAMPLE_FBX") else { return };
    let path = Path::new(&p);
    let dir = path.parent().unwrap();
    let files: Vec<FileEntry> = walkdir::WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| FileEntry {
            path: e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"),
            hash: String::new(),
            size: e.metadata().map(|m| m.len() as i64).unwrap_or(0),
        })
        .filter(|f| hangar_pipeline::is_relevant(&f.path))
        .collect();
    let res = process(&ProcessInput { root: dir, files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    println!("{}", serde_json::to_string_pretty(m).unwrap());
    assert_eq!(m.kind, Some(PreviewKind::Model));
    assert!(!m.lods.is_empty());
}

fn entries(dir: &Path) -> Vec<FileEntry> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| FileEntry {
            path: e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"),
            hash: String::new(),
            size: e.metadata().map(|m| m.len() as i64).unwrap_or(0),
        })
        .collect()
}

#[test]
fn authored_lod_files() {
    let dir = tempfile::tempdir().unwrap();
    for (lod, n) in [(0, 12), (1, 4)] {
        let mut obj = ObjWriter::new();
        obj.grid_box("Hull", "M_Tank_Body", [0.0, 0.5, 0.0], [1.5, 0.5, 3.0], n);
        std::fs::write(dir.path().join(format!("Tank_LOD{lod}.obj")), &obj.out).unwrap();
    }
    std::fs::write(dir.path().join("tank.mtl"), "newmtl M_Tank_Body\nKd 0.5 0.5 0.4\n").unwrap();
    let files = entries(dir.path());
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert_eq!(m.primary_file.as_deref(), Some("Tank_LOD0.obj"));
    assert_eq!(m.lods.len(), 2, "{:?}", m.lods);
    assert!(m.lods.iter().all(|l| l.source == LodSource::Authored));
    assert_eq!(m.lods[0].triangles, 6 * 12 * 12 * 2);
    assert_eq!(m.lods[1].triangles, 6 * 4 * 4 * 2);
    assert_eq!(m.materials.len(), 1, "materials merged by name");
}

#[test]
fn authored_lod_meshes_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut obj = ObjWriter::new();
    obj.grid_box("Hull_LOD0", "M_Tank_Body", [0.0, 0.5, 0.0], [1.5, 0.5, 3.0], 10);
    obj.grid_box("Hull_LOD1", "M_Tank_Body", [0.0, 0.5, 0.0], [1.5, 0.5, 3.0], 3);
    obj.grid_box("Antenna", "M_Tank_Body", [1.0, 1.5, -2.0], [0.02, 0.5, 0.02], 1);
    std::fs::write(dir.path().join("tank.obj"), &obj.out).unwrap();
    std::fs::write(dir.path().join("tank.mtl"), "newmtl M_Tank_Body\nKd 0.5 0.5 0.4\n").unwrap();
    let files = entries(dir.path());
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert_eq!(m.lods.len(), 2);
    // the un-suffixed antenna (12 tris) is part of every level
    assert_eq!(m.lods[0].triangles, 6 * 10 * 10 * 2 + 12);
    assert_eq!(m.lods[1].triangles, 6 * 3 * 3 * 2 + 12);
}

/// glTF using the (archived) KHR_materials_pbrSpecularGlossiness extension as
/// *required*, as Microsoft's exporter writes it: must import (it used to fail
/// validation, so the version fell back to showing a texture), with the diffuse
/// map as base color and the glossiness (alpha) turned into a roughness map.
#[test]
fn gltf_with_specular_glossiness_material() {
    let dir = tempfile::tempdir().unwrap();
    let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let uvs: [f32; 6] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
    let bin: Vec<u8> = positions.iter().chain(&uvs).flat_map(|f| f.to_le_bytes()).collect();
    std::fs::write(dir.path().join("Wyvern.bin"), &bin).unwrap();
    let diffuse = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 40, 255]));
    diffuse.save(dir.path().join("Wyvern_diffuse.png")).unwrap();
    // Glossiness 0.75 everywhere (alpha 191) → roughness ≈ 0.25.
    let specgloss = image::RgbaImage::from_pixel(4, 4, image::Rgba([60, 60, 60, 191]));
    specgloss.save(dir.path().join("Wyvern_specgloss.png")).unwrap();
    let gltf = format!(
        r#"{{
  "asset": {{ "version": "2.0", "generator": "Microsoft GLTF Exporter 2.8.3.66" }},
  "extensionsUsed": ["KHR_materials_pbrSpecularGlossiness"],
  "extensionsRequired": ["KHR_materials_pbrSpecularGlossiness"],
  "scene": 0, "scenes": [{{ "nodes": [0] }}], "nodes": [{{ "mesh": 0, "name": "Wyvern" }}],
  "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0, "TEXCOORD_0": 1 }}, "material": 0 }}] }}],
  "materials": [{{ "name": "M_Wyvern", "extensions": {{ "KHR_materials_pbrSpecularGlossiness": {{
      "diffuseTexture": {{ "index": 0 }}, "specularGlossinessTexture": {{ "index": 1 }} }} }} }}],
  "textures": [{{ "source": 0 }}, {{ "source": 1 }}],
  "images": [{{ "uri": "Wyvern_diffuse.png" }}, {{ "uri": "Wyvern_specgloss.png" }}],
  "buffers": [{{ "uri": "Wyvern.bin", "byteLength": {len} }}],
  "bufferViews": [{{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }}, {{ "buffer": 0, "byteOffset": 36, "byteLength": 24 }}],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2" }}
  ]
}}"#,
        len = bin.len()
    );
    std::fs::write(dir.path().join("Wyvern.gltf"), gltf).unwrap();
    let files: Vec<FileEntry> = ["Wyvern.gltf", "Wyvern.bin", "Wyvern_diffuse.png", "Wyvern_specgloss.png"]
        .iter()
        .map(|p| FileEntry { path: p.to_string(), hash: String::new(), size: 1 })
        .collect();
    let res = process(&ProcessInput { root: dir.path(), files: &files, skeletons: &[] }, &PipelineConfig::default()).unwrap();
    let m = &res.manifest;
    assert_eq!(m.kind, Some(hangar_core::PreviewKind::Model), "warnings: {:?}", m.warnings);
    assert_eq!(m.model_error, None);
    let mat = &m.materials[0];
    let slot = |s: TextureSlot| mat.textures.iter().find(|t| t.slot == s);
    assert_eq!(slot(TextureSlot::BaseColor).and_then(|t| t.path.as_deref()), Some("Wyvern_diffuse.png"));
    assert!(slot(TextureSlot::Roughness).is_some(), "{:?}", mat.textures);

    let glb = res.artifacts.iter().find(|a| a.role == ArtifactRole::LodGlb(0)).unwrap();
    let g = gltf::Gltf::from_slice(&glb.bytes).unwrap();
    let buffers = gltf::import_buffers(&g.document, None, g.blob.clone()).unwrap();
    let pbr = g.document.materials().next().unwrap().pbr_metallic_roughness();
    assert_eq!(pbr.metallic_factor(), 1.0, "factors moved into the packed texture");
    let tex = pbr.metallic_roughness_texture().expect("metal/rough map");
    let view = match tex.texture().source().source() {
        gltf::image::Source::View { view, .. } => view,
        _ => panic!("embedded image expected"),
    };
    let data = &buffers[view.buffer().index()][view.offset()..view.offset() + view.length()];
    let px = image::load_from_memory(data).unwrap().to_rgba8();
    let (rough, metal) = (px.get_pixel(0, 0)[1], px.get_pixel(0, 0)[2]);
    assert!((rough as i32 - 64).abs() <= 2, "roughness channel {rough}");
    assert!(metal <= 2, "spec/gloss materials are dielectric (lossy encoding aside): {metal}");
}
