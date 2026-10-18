//! End-to-end API test against a running server.
//!
//! ```text
//! cargo run -p hangar-server &            # empty database
//! HANGAR_TEST_URL=http://127.0.0.1:8100 cargo test -p hangar-server --test e2e -- --ignored --nocapture
//! ```
//!
//! Covers: first-user setup, projects, assets, deduplicated upload, incremental
//! versions (inherit + delete), processing (LODs, variants, thumbnail), diff,
//! downloads and project permissions.

use hangar_core::*;
use reqwest::blocking::{Client, ClientBuilder};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::fmt::Write as _;
use std::time::{Duration, Instant};
use uuid::Uuid;

struct S {
    base: String,
    http: Client,
}

impl S {
    fn new(base: &str) -> Self {
        Self { base: base.into(), http: ClientBuilder::new().cookie_store(true).build().unwrap() }
    }
    fn url(&self, p: &str) -> String {
        format!("{}{}", self.base, p)
    }
    fn get<T: DeserializeOwned>(&self, p: &str) -> T {
        let r = self.http.get(self.url(p)).send().unwrap();
        assert!(r.status().is_success(), "GET {p}: {}", r.status());
        r.json().unwrap()
    }
    fn post<T: DeserializeOwned>(&self, p: &str, body: serde_json::Value) -> T {
        let r = self.http.post(self.url(p)).json(&body).send().unwrap();
        let status = r.status();
        assert!(status.is_success(), "POST {p}: {status} {}", r.text().unwrap_or_default());
        r.json().unwrap()
    }
    fn status_of(&self, method: reqwest::Method, p: &str, body: Option<serde_json::Value>) -> u16 {
        let mut rb = self.http.request(method, self.url(p));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        rb.send().unwrap().status().as_u16()
    }
}

/// A crude tank: hull, tracks, a UV-sphere turret and a long barrel.
fn tank_obj(with_under_plate: bool) -> String {
    let mut out = String::from("mtllib tank.mtl\nusemtl M_Abrams_Body\n");
    let mut base = 1;
    let mut add = |out: &mut String, verts: Vec<[f32; 3]>, quads: Vec<[usize; 4]>| {
        for v in &verts {
            writeln!(out, "v {} {} {}\nvt {} {}", v[0], v[1], v[2], (v[0] * 0.2).rem_euclid(1.0), (v[2] * 0.2).rem_euclid(1.0))
                .unwrap();
        }
        for q in &quads {
            let i = |k: usize| q[k] + base;
            writeln!(out, "f {0}/{0} {1}/{1} {2}/{2} {3}/{3}", i(0), i(1), i(2), i(3)).unwrap();
        }
        base += verts.len();
    };
    let cuboid = |c: [f32; 3], s: [f32; 3]| {
        let mut v = vec![];
        for dx in [-0.5, 0.5] {
            for dy in [-0.5, 0.5] {
                for dz in [-0.5, 0.5] {
                    v.push([c[0] + dx * s[0], c[1] + dy * s[1], c[2] + dz * s[2]]);
                }
            }
        }
        (v, vec![[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]])
    };
    let (v, f) = cuboid([0.0, 0.6, 0.0], [7.9, 1.0, 3.6]);
    add(&mut out, v, f);
    let (seg, rings) = (48usize, 32usize);
    let mut v = vec![];
    let mut f = vec![];
    for i in 0..=rings {
        let th = std::f32::consts::PI * i as f32 / rings as f32;
        for s in 0..seg {
            let ph = std::f32::consts::TAU * s as f32 / seg as f32;
            v.push([-0.3 + 1.4 * th.sin() * ph.cos(), 1.45 + 1.4 * th.cos(), 1.4 * th.sin() * ph.sin()]);
        }
    }
    for i in 0..rings {
        for s in 0..seg {
            let (a, b) = (i * seg + s, i * seg + (s + 1) % seg);
            f.push([a, b, b + seg, a + seg]);
        }
    }
    add(&mut out, v, f);
    if with_under_plate {
        let (v, f) = cuboid([0.0, 0.08, 0.0], [7.0, 0.1, 3.2]);
        add(&mut out, v, f);
    }
    out
}

fn png(rgb: [u8; 3], size: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(size, size, |x, y| {
        let k = if (x / 32 + y / 32) % 2 == 0 { 0 } else { 30 };
        image::Rgb([rgb[0].saturating_sub(k), rgb[1].saturating_sub(k), rgb[2].saturating_sub(k)])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

struct Upload {
    path: &'static str,
    bytes: Vec<u8>,
}

impl Upload {
    fn entry(&self) -> FileEntry {
        FileEntry { path: self.path.into(), hash: blake3::hash(&self.bytes).to_hex().to_string(), size: self.bytes.len() as i64 }
    }
}

/// Uploads the blobs the server is missing; returns how many were sent.
fn upload(s: &S, files: &[Upload]) -> usize {
    let entries: Vec<FileEntry> = files.iter().map(Upload::entry).collect();
    let missing: MissingBlobsResponse =
        s.post("/api/blobs/missing", json!({ "hashes": entries.iter().map(|e| &e.hash).collect::<Vec<_>>() }));
    let mut sent = std::collections::HashSet::new();
    for (f, e) in files.iter().zip(&entries) {
        if missing.missing.contains(&e.hash) && sent.insert(e.hash.clone()) {
            let r = s.http.put(s.url(&format!("/api/blobs/{}", e.hash))).body(f.bytes.clone()).send().unwrap();
            assert_eq!(r.status().as_u16(), 201, "upload {}", f.path);
        }
    }
    sent.len()
}

fn wait_ready(s: &S, id: Uuid) -> VersionDto {
    let t = Instant::now();
    loop {
        let v: VersionDto = s.get(&format!("/api/versions/{id}"));
        if matches!(v.status, VersionStatus::Ready | VersionStatus::Failed) || t.elapsed() > Duration::from_secs(300) {
            return v;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs a running server: set HANGAR_TEST_URL"]
fn full_workflow() {
    let Ok(base) = std::env::var("HANGAR_TEST_URL") else {
        eprintln!("HANGAR_TEST_URL not set, skipping");
        return;
    };
    let s = S::new(&base);
    let tag = &Uuid::new_v4().simple().to_string()[..6];

    let setup: SetupStatus = s.get("/api/setup");
    let admin = format!("admin{tag}");
    if setup.needs_setup {
        let _: LoginResponse = s.post("/api/auth/register", json!({"username": admin, "password": "senha-forte-1"}));
    } else {
        let user = std::env::var("HANGAR_TEST_ADMIN").expect("server already set up: set HANGAR_TEST_ADMIN=user:pass");
        let (u, p) = user.split_once(':').unwrap();
        let _: LoginResponse = s.post("/api/auth/login", json!({"username": u, "password": p}));
    }
    let me: UserDto = s.get("/api/auth/me");
    assert!(me.is_admin);

    let project: ProjectDto = s.post("/api/projects", json!({"name": format!("Blindados {tag}")}));
    assert_eq!(project.my_role, Some(Role::Owner));
    let asset: AssetDetail = s.post(
        &format!("/api/projects/{}/assets", project.id),
        json!({"name": "M1A2 Abrams", "kind": "model", "tags": ["Tank", "tank", "US"], "source": {"vendor": "Fab"}}),
    );
    assert_eq!(asset.summary.tags, vec!["tank", "us"]);

    let mtl = "newmtl M_Abrams_Body\nKd 1 1 1\nmap_Kd Textures/Desert/T_Abrams_D.png\n";
    let v1_files = vec![
        Upload { path: "tank.obj", bytes: tank_obj(false).into_bytes() },
        Upload { path: "tank.mtl", bytes: mtl.as_bytes().to_vec() },
        Upload { path: "Textures/Desert/T_Abrams_D.png", bytes: png([196, 170, 120], 256) },
        Upload { path: "Textures/Woodland/T_Abrams_D.png", bytes: png([70, 90, 50], 256) },
        Upload { path: "LICENSE.txt", bytes: b"license".to_vec() },
    ];
    assert_eq!(upload(&s, &v1_files), 5);
    assert_eq!(upload(&s, &v1_files), 0, "second upload is fully deduplicated");

    let v1: VersionDto = s.post(
        &format!("/api/assets/{}/versions", asset.summary.id),
        json!({"parent_id": null, "message": "Original", "upserts": v1_files.iter().map(Upload::entry).collect::<Vec<_>>()}),
    );
    let v1 = wait_ready(&s, v1.id);
    assert_eq!(v1.status, VersionStatus::Ready, "{:?}", v1.error);
    let preview = v1.preview.clone().unwrap();
    assert!(preview.lods.len() >= 2, "generated LODs");
    let names: Vec<_> = preview.variants.iter().map(|v| v.name.to_lowercase()).collect();
    assert!(names.contains(&"desert".into()) && names.contains(&"woodland".into()), "variants {names:?}");
    assert!(preview.thumbnail_hash.is_some());
    let glb = s.http.get(s.url(&blob_url(&preview.lods[0].glb_hash))).send().unwrap().bytes().unwrap();
    assert_eq!(&glb[..4], b"glTF");

    // v2 replaces only the mesh and drops the license.
    let v2_files = vec![Upload { path: "tank.obj", bytes: tank_obj(true).into_bytes() }];
    assert_eq!(upload(&s, &v2_files), 1);
    let v2: VersionDto = s.post(
        &format!("/api/assets/{}/versions", asset.summary.id),
        json!({"parent_id": v1.id, "message": "Fecha o fundo", "upserts": v2_files.iter().map(Upload::entry).collect::<Vec<_>>(), "deletes": ["LICENSE.txt"]}),
    );
    assert_eq!((v2.number, v2.file_count, v2.parent_id), (2, 4, Some(v1.id)));
    let diff: VersionDiff = s.get(&format!("/api/versions/{}/diff", v2.id));
    assert_eq!(diff.modified.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), ["tank.obj"]);
    assert_eq!(diff.removed.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), ["LICENSE.txt"]);
    let v2 = wait_ready(&s, v2.id);
    assert!(v2.preview.unwrap().stats.unwrap().triangles > preview.stats.unwrap().triangles);

    // Permissions: a viewer can read but not write.
    let bob = format!("bob{tag}");
    let _: UserDto = s.post("/api/users", json!({"username": bob, "password": "senha-do-bob"}));
    let b = S::new(&base);
    let _: LoginResponse = b.post("/api/auth/login", json!({"username": bob, "password": "senha-do-bob"}));
    let asset_path = format!("/api/assets/{}", asset.summary.id);
    assert_eq!(b.status_of(reqwest::Method::GET, &asset_path, None), 404);
    let _: MemberDto = s.post(&format!("/api/projects/{}/members", project.id), json!({"username": bob, "role": "viewer"}));
    assert_eq!(b.status_of(reqwest::Method::GET, &asset_path, None), 200);
    let commit = json!({"parent_id": v1.id, "message": "x", "deletes": ["tank.mtl"]});
    assert_eq!(b.status_of(reqwest::Method::POST, &format!("{asset_path}/versions"), Some(commit)), 403);
}
