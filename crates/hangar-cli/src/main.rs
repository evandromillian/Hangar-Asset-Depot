//! `hangar` — command line client.
//!
//! Typical workflow after fixing a model in Blender:
//!
//! ```text
//! hangar login --server http://hangar.local:8100 --user evandro
//! hangar push 0192… ./M1A2_Abrams -m "Preenche a parte inferior do casco" --mirror --wait
//! ```
//!
//! Only files whose content changed are uploaded (BLAKE3 + `/api/blobs/missing`).

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use hangar_core::{
    AssetDetail, AssetKind, AssetSource, AssetSummary, CommitVersionRequest, CreateAssetRequest,
    CreateProjectRequest, FileEntry,
    LoginRequest, LoginResponse, MissingBlobsRequest, MissingBlobsResponse, Page, ProjectDto, UserDto, VersionDiff,
    MaterialConfig, VersionDto, VersionStatus, MATERIAL_CONFIG_PATH, paths,
};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "hangar", version, about = "Hangar DAM — cliente de linha de comando")]
struct Cli {
    /// Server URL (overrides the saved login).
    #[arg(long, env = "HANGAR_SERVER", global = true)]
    server: Option<String>,
    /// API token (overrides the saved login).
    #[arg(long, env = "HANGAR_TOKEN", global = true, hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Log in and save the session token.
    Login {
        #[arg(long)]
        user: String,
        /// Password (prompted when omitted).
        #[arg(long, env = "HANGAR_PASSWORD", hide_env_values = true)]
        password: Option<String>,
    },
    /// Forget the saved session.
    Logout,
    Whoami,
    /// List projects.
    Projects,
    /// Create a project (prints its id).
    NewProject {
        #[arg(long)]
        name: String,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// List assets of a project.
    Assets {
        project: Uuid,
        #[arg(short, long)]
        query: Option<String>,
        #[arg(long)]
        kind: Option<AssetKind>,
        #[arg(long)]
        tag: Option<String>,
    },
    /// Create an asset (prints its id).
    NewAsset {
        project: Uuid,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "model")]
        kind: AssetKind,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        vendor: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        license: Option<String>,
    },
    /// Create an asset with the kind, description, tags and source of SOURCE (prints its id).
    /// Versions and files are not copied.
    CloneAsset {
        source: Uuid,
        #[arg(long)]
        name: String,
        /// Target project (default: the source's).
        #[arg(long)]
        project: Option<Uuid>,
        /// Overrides the source's description.
        #[arg(long)]
        description: Option<String>,
    },
    /// Version history of an asset.
    Versions { asset: Uuid },
    /// Upload a folder as a new version (only changed files are sent).
    Push {
        asset: Uuid,
        dir: PathBuf,
        #[arg(short, long)]
        message: String,
        /// Parent version: a version id, "head" (default) or "none".
        #[arg(long, default_value = "head")]
        parent: String,
        /// Remove files from the inherited tree that are not in DIR.
        #[arg(long)]
        mirror: bool,
        /// Don't make the new version the asset head.
        #[arg(long)]
        no_head: bool,
        /// Wait for processing and print the result.
        #[arg(long)]
        wait: bool,
        /// Texture set (asset id) whose images the model may use; added to the
        /// version's .hangar/materials.json. Repeatable.
        #[arg(long = "texture-set")]
        texture_sets: Vec<Uuid>,
    },
    /// Download every file of a version into DIR.
    Pull { version: Uuid, dir: PathBuf },
    /// Processing status of a version.
    Status {
        version: Uuid,
        #[arg(long)]
        wait: bool,
    },
    /// Files added / removed / modified compared to the parent (or --against).
    Diff {
        version: Uuid,
        #[arg(long)]
        against: Option<Uuid>,
    },
}

#[derive(Serialize, Deserialize, Default)]
struct Credentials {
    server: String,
    token: String,
    username: String,
}

fn credentials_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("hangar").join("credentials.json")
}

fn load_credentials() -> Option<Credentials> {
    let s = std::fs::read_to_string(credentials_path()).ok()?;
    serde_json::from_str(&s).ok()
}

struct Api {
    base: String,
    token: Option<String>,
    http: Client,
}

impl Api {
    fn new(server: String, token: Option<String>) -> Result<Self> {
        let http = Client::builder().timeout(None).build()?;
        Ok(Self { base: server.trim_end_matches('/').to_string(), token, http })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        let rb = self.http.request(method, format!("{}{}", self.base, path));
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    fn check(resp: Response) -> Result<Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("não autenticado (401) — rode `hangar login`");
        }
        bail!("{status}: {}", body.trim())
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(Self::check(self.req(reqwest::Method::GET, path).send()?)?.json()?)
    }

    fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        Ok(Self::check(self.req(reqwest::Method::POST, path).json(body).send()?)?.json()?)
    }
}

fn main() {
    // Behave like other Unix tools when piped into `head` (no panic on EPIPE).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    if let Err(e) = run() {
        eprintln!("erro: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let saved = load_credentials();
    let server = cli
        .server
        .clone()
        .or_else(|| saved.as_ref().map(|c| c.server.clone()))
        .unwrap_or_else(|| "http://localhost:8100".into());
    let token = cli.token.clone().or_else(|| saved.as_ref().map(|c| c.token.clone()));

    match cli.cmd {
        Cmd::Login { user, password } => {
            let password = match password {
                Some(p) => p,
                None => rpassword::prompt_password(format!("senha de {user}: "))?,
            };
            let api = Api::new(server.clone(), None)?;
            let res: LoginResponse = api.post("/api/auth/login", &LoginRequest { username: user, password })?;
            let creds = Credentials { server, token: res.token, username: res.user.username.clone() };
            let path = credentials_path();
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, serde_json::to_vec_pretty(&creds)?)?;
            restrict_permissions(&path);
            println!("logado como {} ({})", res.user.username, path.display());
        }
        Cmd::Logout => {
            if let Some(t) = token {
                let api = Api::new(server, Some(t))?;
                let _ = api.req(reqwest::Method::POST, "/api/auth/logout").send();
            }
            let _ = std::fs::remove_file(credentials_path());
            println!("sessão removida");
        }
        Cmd::Whoami => {
            let me: UserDto = Api::new(server, token)?.get("/api/auth/me")?;
            println!("{} ({}){}", me.username, me.display_name, if me.is_admin { " [admin]" } else { "" });
        }
        Cmd::Projects => {
            let projects: Vec<ProjectDto> = Api::new(server, token)?.get("/api/projects")?;
            for p in projects {
                let role = p.my_role.map(|r| r.to_string()).unwrap_or_default();
                println!("{}  {:<28} {:>4} assets  {}", p.id, p.name, p.asset_count, role);
            }
        }
        Cmd::NewProject { name, slug, description } => {
            let req = CreateProjectRequest { name, slug, description: description.unwrap_or_default() };
            let p: ProjectDto = Api::new(server, token)?.post("/api/projects", &req)?;
            println!("{}", p.id);
        }
        Cmd::Assets { project, query, kind, tag } => {
            let api = Api::new(server, token)?;
            let mut url = format!("/api/projects/{project}/assets?size=200");
            if let Some(q) = query {
                url.push_str(&format!("&q={}", urlencode(&q)));
            }
            if let Some(k) = kind {
                url.push_str(&format!("&kind={k}"));
            }
            if let Some(t) = tag {
                url.push_str(&format!("&tag={}", urlencode(&t)));
            }
            let page: Page<AssetSummary> = api.get(&url)?;
            for a in page.items {
                let head = a.head.map(|h| format!("v{} {}", h.number, h.status)).unwrap_or_else(|| "—".into());
                println!("{}  {:<30} {:<8} {:<14} [{}]", a.id, a.name, a.kind, head, a.tags.join(", "));
            }
            println!("{} asset(s)", page.total);
        }
        Cmd::NewAsset { project, name, kind, tags, description, vendor, url, author, license } => {
            let api = Api::new(server, token)?;
            let req = CreateAssetRequest {
                name,
                slug: None,
                kind,
                description: description.unwrap_or_default(),
                tags,
                source: AssetSource { vendor, url, author, license, ..Default::default() },
            };
            let a: AssetDetail = api.post(&format!("/api/projects/{project}/assets"), &req)?;
            println!("{}", a.summary.id);
        }
        Cmd::CloneAsset { source, name, project, description } => {
            let api = Api::new(server, token)?;
            let src: AssetDetail = api.get(&format!("/api/assets/{source}"))?;
            let project = project.unwrap_or(src.summary.project_id);
            let req = CreateAssetRequest {
                name,
                slug: None,
                kind: src.summary.kind,
                description: description.unwrap_or(src.description),
                tags: src.summary.tags,
                source: src.source,
            };
            let a: AssetDetail = api.post(&format!("/api/projects/{project}/assets"), &req)?;
            println!("{}", a.summary.id);
        }
        Cmd::Versions { asset } => {
            let versions: Vec<VersionDto> = Api::new(server, token)?.get(&format!("/api/assets/{asset}/versions"))?;
            for v in versions {
                let parent = v.parent_id.map(|p| p.to_string()[..8].to_string()).unwrap_or_else(|| "—".into());
                println!(
                    "{}{:<4} {}  {:<10} parent {:<8}  {:<12} {}",
                    if v.is_head { "*" } else { " " },
                    format!("v{}", v.number),
                    v.id,
                    v.status,
                    parent,
                    v.author.unwrap_or_default(),
                    v.message
                );
            }
        }
        Cmd::Push { asset, dir, message, parent, mirror, no_head, wait, texture_sets } => {
            let api = Api::new(server, token)?;
            push(&api, asset, &dir, message, &parent, mirror, !no_head, wait, &texture_sets)?;
        }
        Cmd::Pull { version, dir } => {
            let api = Api::new(server, token)?;
            pull(&api, version, &dir)?;
        }
        Cmd::Status { version, wait } => {
            let api = Api::new(server, token)?;
            let v = if wait { wait_processed(&api, version)? } else { api.get(&format!("/api/versions/{version}"))? };
            print_version(&v);
        }
        Cmd::Diff { version, against } => {
            let api = Api::new(server, token)?;
            let url = match against {
                Some(a) => format!("/api/versions/{version}/diff?against={a}"),
                None => format!("/api/versions/{version}/diff"),
            };
            let d: VersionDiff = api.get(&url)?;
            for f in &d.added {
                println!("+ {}  ({})", f.path, human(f.size));
            }
            for f in &d.modified {
                println!("~ {}  ({} → {})", f.path, human(f.old_size), human(f.new_size));
            }
            for f in &d.removed {
                println!("- {}", f.path);
            }
            println!("{} inalterado(s)", d.unchanged);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_permissions(_: &Path) {}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn human(bytes: i64) -> String {
    let b = bytes as f64;
    match bytes {
        n if n >= 1 << 30 => format!("{:.2} GiB", b / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MiB", b / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KiB", b / 1024.0),
        n => format!("{n} B"),
    }
}

/// Junk that tools leave around and nobody wants versioned.
fn ignored(name: &str) -> bool {
    // `.hangar/` holds versioned settings (materials.json); other dotfiles are local noise.
    (name.starts_with('.') && name != ".hangar")
        || matches!(name, "Thumbs.db" | "desktop.ini") || name.ends_with(".blend1") || name.ends_with('~')
}

struct LocalFile {
    entry: FileEntry,
    abs: PathBuf,
    /// Content generated by the CLI instead of read from `abs`.
    data: Option<Vec<u8>>,
}

fn scan(dir: &Path) -> Result<Vec<LocalFile>> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| e.depth() == 0 || !ignored(&e.file_name().to_string_lossy())) {
        let e = e?;
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e.path().strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
        let path = paths::normalize(&rel).ok_or_else(|| anyhow!("caminho inválido: {rel}"))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(std::fs::File::open(e.path())?)?;
        let size = e.metadata()?.len() as i64;
        let entry = FileEntry { path, hash: hasher.finalize().to_hex().to_string(), size };
        out.push(LocalFile { entry, abs: e.into_path(), data: None });
    }
    out.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn push(
    api: &Api,
    asset: Uuid,
    dir: &Path,
    message: String,
    parent: &str,
    mirror: bool,
    set_head: bool,
    wait: bool,
    texture_sets: &[Uuid],
) -> Result<()> {
    if !dir.is_dir() {
        bail!("{} não é uma pasta", dir.display());
    }
    print!("calculando hashes… ");
    let started = Instant::now();
    let mut local = scan(dir)?;
    let local_bytes: i64 = local.iter().map(|f| f.entry.size).sum();
    println!("{} arquivo(s), {} em {:.1}s", local.len(), human(local_bytes), started.elapsed().as_secs_f32());

    let parent_id: Option<Uuid> = match parent {
        "none" => None,
        "head" => {
            let a: AssetDetail = api.get(&format!("/api/assets/{asset}"))?;
            a.summary.head.map(|h| h.id)
        }
        id => Some(id.parse().context("--parent deve ser um id, \"head\" ou \"none\"")?),
    };

    let remote: BTreeMap<String, FileEntry> = match parent_id {
        Some(p) => api
            .get::<Vec<FileEntry>>(&format!("/api/versions/{p}/files"))?
            .into_iter()
            .map(|f| (f.path.clone(), f))
            .collect(),
        None => BTreeMap::new(),
    };
    if !texture_sets.is_empty() {
        let parent_config = parent_id.filter(|_| remote.contains_key(MATERIAL_CONFIG_PATH));
        link_texture_sets(api, &mut local, parent_config, texture_sets)?;
    }
    let upserts: Vec<&LocalFile> =
        local.iter().filter(|f| remote.get(&f.entry.path).map(|r| r.hash != f.entry.hash).unwrap_or(true)).collect();
    let deletes: Vec<String> = if mirror {
        // Material settings made in the web UI survive a mirror push from a folder
        // that doesn't have them (they are edited there, or pushed when present locally).
        let here: std::collections::HashSet<&str> = local.iter().map(|f| f.entry.path.as_str()).collect();
        remote.keys().filter(|p| !here.contains(p.as_str()) && p.as_str() != MATERIAL_CONFIG_PATH).cloned().collect()
    } else {
        vec![]
    };
    if upserts.is_empty() && deletes.is_empty() {
        println!("nada mudou em relação à versão pai");
        return Ok(());
    }
    for f in &upserts {
        println!("  {} {}", if remote.contains_key(&f.entry.path) { "~" } else { "+" }, f.entry.path);
    }
    for d in &deletes {
        println!("  - {d}");
    }

    let missing: MissingBlobsResponse = api.post(
        "/api/blobs/missing",
        &MissingBlobsRequest { hashes: upserts.iter().map(|f| f.entry.hash.clone()).collect() },
    )?;
    let mut missing: std::collections::HashSet<String> = missing.missing.into_iter().collect();
    let to_send: Vec<&&LocalFile> = upserts.iter().filter(|f| missing.remove(&f.entry.hash)).collect();
    let send_bytes: i64 = to_send.iter().map(|f| f.entry.size).sum();
    let skipped: i64 = upserts.iter().map(|f| f.entry.size).sum::<i64>() - send_bytes;
    println!("enviando {} blob(s), {} ({} já estavam no servidor)", to_send.len(), human(send_bytes), human(skipped));
    for (i, f) in to_send.iter().enumerate() {
        let t = Instant::now();
        let body = match &f.data {
            Some(d) => reqwest::blocking::Body::from(d.clone()),
            None => reqwest::blocking::Body::sized(std::fs::File::open(&f.abs)?, f.entry.size as u64),
        };
        let resp = api
            .req(reqwest::Method::PUT, &format!("/api/blobs/{}", f.entry.hash))
            .header(reqwest::header::CONTENT_TYPE, paths::mime_for(&f.entry.path))
            .body(body)
            .send()?;
        Api::check(resp)?;
        let secs = t.elapsed().as_secs_f64().max(0.001);
        println!(
            "  [{}/{}] {}  {}  {}/s",
            i + 1,
            to_send.len(),
            f.entry.path,
            human(f.entry.size),
            human((f.entry.size as f64 / secs) as i64)
        );
    }

    let req = CommitVersionRequest {
        parent_id,
        message,
        inherit: true,
        upserts: upserts.iter().map(|f| f.entry.clone()).collect(),
        deletes,
        set_head,
    };
    let v: VersionDto = api.post(&format!("/api/assets/{asset}/versions"), &req)?;
    println!("criada v{} ({}) com {} arquivo(s), {}", v.number, v.id, v.file_count, human(v.total_size));
    if wait {
        let v = wait_processed(api, v.id)?;
        print_version(&v);
    }
    Ok(())
}

/// Adds `sets` to the `.hangar/materials.json` of the push: DIR's own file when
/// it has one, else the parent version's (`parent`), else a new file.
fn link_texture_sets(api: &Api, local: &mut Vec<LocalFile>, parent: Option<Uuid>, sets: &[Uuid]) -> Result<()> {
    for id in sets {
        let a: AssetDetail = api.get(&format!("/api/assets/{id}")).with_context(|| format!("conjunto de texturas {id}"))?;
        println!("conjunto de texturas: {} ({id})", a.summary.name);
    }
    let pos = local.iter().position(|f| f.entry.path == MATERIAL_CONFIG_PATH);
    let mut cfg = match (pos, parent) {
        (Some(i), _) => MaterialConfig::from_json(&std::fs::read(&local[i].abs)?)
            .with_context(|| format!("{MATERIAL_CONFIG_PATH} local inválido"))?,
        (None, Some(p)) => {
            let path = MATERIAL_CONFIG_PATH.split('/').map(urlencode).collect::<Vec<_>>().join("/");
            let url = format!("/api/versions/{p}/raw/{path}");
            let bytes = Api::check(api.req(reqwest::Method::GET, &url).send()?)?.bytes()?;
            MaterialConfig::from_json(&bytes).with_context(|| format!("{MATERIAL_CONFIG_PATH} da versão pai inválido"))?
        }
        (None, None) => MaterialConfig::default(),
    };
    let before = cfg.texture_sets.len();
    for id in sets {
        if !cfg.texture_sets.contains(id) {
            cfg.texture_sets.push(*id);
        }
    }
    if cfg.texture_sets.len() == before {
        return Ok(());
    }
    let data = cfg.to_json();
    let entry = FileEntry {
        path: MATERIAL_CONFIG_PATH.to_string(),
        hash: blake3::hash(&data).to_hex().to_string(),
        size: data.len() as i64,
    };
    let generated = LocalFile { entry, abs: PathBuf::new(), data: Some(data) };
    match pos {
        Some(i) => local[i] = generated,
        None => {
            local.push(generated);
            local.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
        }
    }
    Ok(())
}

fn wait_processed(api: &Api, version: Uuid) -> Result<VersionDto> {
    let started = Instant::now();
    loop {
        let v: VersionDto = api.get(&format!("/api/versions/{version}"))?;
        if matches!(v.status, VersionStatus::Ready | VersionStatus::Failed) {
            println!("processada em {:.0}s", started.elapsed().as_secs_f32());
            return Ok(v);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn print_version(v: &VersionDto) {
    println!("v{} {} — {}", v.number, v.status, v.message);
    if let Some(e) = &v.error {
        println!("  erro: {e}");
    }
    let Some(p) = &v.preview else { return };
    if let Some(s) = &p.stats {
        let d = s.dimensions();
        println!(
            "  {} triângulos, {} vértices, {} materiais, {:.2} × {:.2} × {:.2} m",
            s.triangles, s.vertices, s.materials, d[0], d[1], d[2]
        );
    }
    for l in &p.lods {
        println!("  LOD{} {:<9} {:>8} tris  ({:.0}%)", l.level, l.source.to_string(), l.triangles, l.ratio * 100.0);
    }
    for var in &p.variants {
        println!(
            "  variante {}{} ({} textura(s), {})",
            var.name,
            if var.is_default { " [padrão]" } else { "" },
            var.overrides.len(),
            var.detected_by
        );
    }
    for w in &p.warnings {
        println!("  aviso: {w}");
    }
}

fn pull(api: &Api, version: Uuid, dir: &Path) -> Result<()> {
    let files: Vec<FileEntry> = api.get(&format!("/api/versions/{version}/files"))?;
    std::fs::create_dir_all(dir)?;
    let mut fetched = 0;
    for f in &files {
        let rel = paths::normalize(&f.path).ok_or_else(|| anyhow!("caminho inválido do servidor: {}", f.path))?;
        let dest = dir.join(&rel);
        if dest.exists() {
            let mut h = blake3::Hasher::new();
            h.update_reader(std::fs::File::open(&dest)?)?;
            if h.finalize().to_hex().as_str() == f.hash {
                continue;
            }
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let url = format!("/api/versions/{version}/raw/{}", rel.split('/').map(urlencode).collect::<Vec<_>>().join("/"));
        let mut resp = Api::check(api.req(reqwest::Method::GET, &url).send()?)?;
        let mut out = std::fs::File::create(&dest)?;
        resp.copy_to(&mut out)?;
        fetched += 1;
        println!("  ↓ {}  {}", rel, human(f.size));
    }
    println!("{} arquivo(s) baixado(s), {} já estavam atualizados", fetched, files.len() - fetched);
    Ok(())
}
