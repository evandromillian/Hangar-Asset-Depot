# CLAUDE.md

Orientation for Claude Code working in this repository.

## What this is

**Hangar** — a self-hosted digital asset manager for 3D / game content (models, textures, audio),
written entirely in Rust. Core ideas: git-like **versioning per asset** over a **content-addressed
blob store**, a background **processing pipeline** (FBX/glTF/OBJ → web GLB per LOD, texture
variants, thumbnails) and a **browser 3D viewer** (Leptos UI + embedded Bevy).

Full design, data model and REST table: `docs/ARCHITECTURE.md` (Portuguese). Read it before
changing the data model, the API or the pipeline output.

## Workspace layout

| crate | target | role |
|---|---|---|
| `crates/hangar-core` | native + wasm32 | API DTOs (`api.rs`), enums (`enums.rs`), `PreviewManifest` (`preview.rs`), path rules (`paths.rs`). Serde-only — no native deps. |
| `crates/hangar-pipeline` | native | Sync, CPU-bound processing. Entry point `process(&ProcessInput, &PipelineConfig)`. Modules: `import/{fbx,gltf_import,obj,blender}`, `textures` (reference resolution, slot detection), `variants`, `lod`, `glb` (hand-written GLB writer), `raster` (CPU thumbnail), `images`, `audio`, orchestration in `process.rs`. |
| `crates/hangar-server` | native | summer-rs app: REST API, auth, storage, job workers. |
| `crates/hangar-web` | wasm32 | Leptos 0.8 CSR app (`pages/`, `components/`, `api.rs`, `upload.rs`) + Bevy 0.19 viewer (`viewer/`). Built with Trunk. **Not** in `default-members`. |
| `crates/hangar-cli` | native | `hangar` binary: login, push (folder → new version, uploads only changed blobs), pull, versions, diff, status. |

Other: `config/app.toml` (server config), `crates/hangar-server/migrations/*.sql`,
`tools/blender/export_glb.py` (headless Blender fallback), `docker-compose.yml` (Postgres, optional MinIO).

## Commands

Run everything from the repo root (summer-rs loads `./config/app.toml` relative to the CWD).

```bash
cargo build                                   # core, pipeline, server, cli (default-members)
cargo test                                    # unit + pipeline integration tests (no DB needed)
cargo clippy --all-targets                    # keep it warning-free
cargo fmt

docker compose up -d                          # local Postgres (hangar/hangar@localhost:5432/hangar)
cargo run -p hangar-server                    # API + static web on http://localhost:8100

# Frontend (needs: rustup target add wasm32-unknown-unknown; cargo install trunk --locked)
cd crates/hangar-web && trunk serve           # dev on :8080, proxies /api → :8100
cd crates/hangar-web && trunk build --release # writes crates/hangar-web/dist, served by the backend
cargo check -p hangar-web --target wasm32-unknown-unknown   # fast type-check of the web crate

# Server end-to-end test (needs a running server; ignored by default)
HANGAR_TEST_URL=http://127.0.0.1:8100 cargo test -p hangar-server --test e2e -- --ignored --nocapture
#   if the DB already has users, also: HANGAR_TEST_ADMIN=user:password

# Pipeline on a folder, dumping manifest + artifacts (inspect thumbnails visually)
cargo run -p hangar-pipeline --example process_dir -- <asset-dir> <out-dir>

# CLI
cargo run -p hangar-cli -- login --server http://localhost:8100 --user <name>
cargo run -p hangar-cli -- push <asset-id> ./folder -m "message" --mirror --wait
cargo run -p hangar-cli -- clone-asset <asset-id> --name "New"   # kind/description/tags/source, no versions
cargo run -p hangar-cli -- push <asset-id> ./folder -m "msg" --texture-set <set-asset-id>   # links a shared texture set

# Batch import: each subfolder of -Root → one asset cloned from -Template, v1 = folder; resumable
.\tools\import-folders.ps1 -Root <package-dir> -Template <asset-id> -AssetPrefix "Name "   # -TextureSet <id>; -Files "*.fbx" = one asset per file
```

## Configuration (summer-rs)

TOML, Spring-Boot style: `config/app.toml` is the base; `config/app-{dev,test,prod}.toml` is merged
on top according to `SUMMER_ENV` (default `dev`). `${VAR:default}` placeholders read the process
environment or a `.env` file in the CWD. Sections: `[web]`, `[sea-orm]`, `[opendal]` (storage:
`fs` or `s3`), `[sa-token]`, `[hangar]` (→ `HangarConfig` in `crates/hangar-server/src/config.rs`).
Never commit real credentials — put them in a local profile file or `.env`.

Supabase: use the **session pooler (port 5432)**, not the transaction pooler (6543) — sqlx needs
prepared statements. Add `?sslmode=require`.

## Server architecture & invariants

- **Plugins** (`main.rs`): `SeaOrmPlugin`, `OpenDALPlugin`, `SaTokenPlugin`, `HangarPlugin`
  (runs migrations, registers `BlobStore` + `JobQueue` components, schedules workers), `WebPlugin`.
- **Routes**: summer-web attribute macros (`#[get("/api/...")]`), auto-registered by
  `#[auto_config(WebConfigurator)]`. A new route module must be declared in `routes/mod.rs`.
  axum 0.8 path syntax: `{id}`, `{*path}`.
- **Auth**: sa-token path rules in `main.rs` (`/api/**` protected, a few public exclusions).
  Token comes from cookie `hangar_token` (HttpOnly, set on login), `Authorization: Bearer`, or query.
  Handlers take `CurrentUser` (`auth.rs`); the layer stores the login id (user UUID) as a `String`
  request extension and the `TokenValue` as another extension.
- **Authorization**: always go through `access::{project, asset, version}(db, &user, id, Need::…)`.
  Non-members get **404** (don't leak existence); insufficient role → 403. Global admins act as owner.
- **Errors**: return `crate::error::Result<T>` (summer-web `WebError`). Use the helpers
  `bad_request/forbidden/not_found/conflict/unauthorized` and `.ctx("what")` on DB/storage errors.
  Error bodies are plain text. **User-facing messages are Brazilian Portuguese**; code, comments and
  logs are English.
- **Blob store** (`storage.rs`): content-addressed by BLAKE3 hex, object key `blobs/ab/cd/<hash>`.
  The `blobs` row is inserted only after the object is fully written; uploads are re-hashed and
  rejected on mismatch. Blobs are immutable — never overwrite or mutate one. Unreferenced blobs are
  removed only by `POST /api/admin/gc` (24h grace for in-flight uploads).
- **Versions** (`routes/versions.rs`): immutable snapshots = file tree in `version_files` +
  `parent_id` (a DAG; branching allowed). Commit = parent tree (if `inherit`) − `deletes` + `upserts`.
  Numbering happens under an exclusive row lock on the asset. The processing job is enqueued **in the
  same transaction**, then `queue.wake()` after commit. Empty / no-op commits are rejected.
- **Paths**: every client-supplied path goes through `hangar_core::paths::normalize` (rejects `..`,
  drive letters, control chars; `/` separators). Never join an unnormalised path onto the filesystem.
- **Jobs** (`jobs.rs`): Postgres queue claimed with `FOR UPDATE SKIP LOCKED`, quadratic back-off,
  stale `running` jobs requeued after 30 min. `process_version` downloads only
  `hangar_pipeline::is_relevant` files to a temp dir, runs the pipeline in `spawn_blocking`, stores
  artifacts with `BlobStore::put_bytes`, records `renditions` (used for GC references) and writes the
  `PreviewManifest` JSON into `asset_versions.preview`. The manifest is what the UI consumes.
- **Migrations** (`db.rs` + `migrations/`): append-only. Add `NNNN_name.sql` and register it in the
  `MIGRATIONS` array. Never edit a migration that may have been applied. Each runs in a transaction.

## Pipeline invariants

- `PreviewManifest` (in `hangar-core`) is the contract between pipeline, server and web. If you change
  it incompatibly, bump `hangar_core::PIPELINE_VERSION`; keep new fields `#[serde(default)]` /
  `Option` so old manifests still deserialize.
- Every manifest `*_hash` must reference an `Artifact` returned in `ProcessOutput`
  (`Artifact::new` computes the BLAKE3 hash).
- Material names must survive into the GLB — the viewer applies texture variants by material name.
- Manual material settings live in the version tree as `.hangar/materials.json`
  (`hangar_core::MaterialConfig`): base slot assignments override automatic resolution, skins become
  `detected_by: "manual"` variants. The web "Materiais" tab edits it and commits a new version; the
  viewer's live preview must mirror the GLB export's factor rules (`viewer/variants.rs`).
  The Specular slot (reflectance convention, 0.5 = default 4 % F0) is exported as
  `KHR_materials_specular` (`specularColorTexture`, `specularColorFactor` 2); the viewer needs
  Bevy's `pbr_specular_textures` feature (two more fragment-shader textures — mind WebGL2's 16).
- Meshes (`MeshPrim`) are always world-space rest pose. An animated FBX or glTF/GLB additionally
  yields a `scene::Rig` (hierarchy, skins, baked clips; glTF skinned vertices are mapped from bind
  space by joint world × inverse bind, and the rig's inverse binds compensate, so playback is exact;
  STEP / CUBICSPLINE samplers are converted to linear keys); only `glb::write_glb` uses it, moving rigid parts into
  their node's space and writing everything under a single root node (Bevy's animation root).
  The manifest's `animations` must stay in GLB clip order — the viewer loads `#AnimationN` by index.
  Other FBX files of the version whose takes move are animation sources for an FBX primary
  (`fbx::probe` classifies them; clips are retargeted by node name in `fbx::external_clips`).
- A version whose FBX files have animation but no mesh is `PreviewKind::Animation`. It is
  previewed on the **reference skeletons** named in its `.hangar/animation.json`
  (`hangar_core::AnimationConfig`, slugs of the `skeletons` table = system setting, admin
  only). The job worker materialises each skeleton asset's head version and passes it as
  `ProcessInput::skeletons`; the pipeline writes one `ArtifactRole::SkeletonGlb(slug)` per
  skeleton (rendition `skeleton_glb`) and a `SkeletonPreview` per slug in
  `manifest.skeletons` (config order, `error` when it can't be built). Versions with their
  own mesh ignore the binding — their animations only play on that mesh. "Own" means the
  animation files were made for it (`fbx::animates`: half of a file's named nodes exist in
  the mesh); meshes sharing no bones (props of an animation package) are ignored instead.
  An animation asset is a package: any number of animation FBX files, added over versions.
- Alpha: importers record what the file says in `scene::Material::alpha` (`AlphaMode`); the
  export and the thumbnail decide through `process::gltf_alpha`. FBX: a linked base color
  without a linked transparency (`TransparencyFactor`/`TransparentColor`/opacity) ignores the
  texture's alpha — game textures pack masks there (Halo specular); glTF keeps its
  `alphaMode`; OBJ and name-matched textures stay `Auto` (alpha in the base color = cutout).
  An Opacity map is merged into the base color alpha (GLB and thumbnail) from its own alpha
  channel when it has one, else from its red channel (`images::merge_alpha`).
  `MaterialConfig::alpha_cutoff` (per material, "Recorte alfa" in the Materiais tab) forces
  `Mask(cutoff)` — model files can't carry a shader's threshold. The manifest's
  `MaterialInfo::alpha_mode` / `alpha_cutoff` record what was baked; the viewer previews
  unsaved cutoffs as Bevy `AlphaMode::Mask`.
- Texture sets: `MaterialConfig::texture_sets` (asset ids) link assets of shared images. The
  worker materialises each set's **head** images under `@<slug>/` in the work dir
  (`ProcessInput::texture_sets`; the version's author must be able to read the set, else the
  entry gets an `error` and no name). `ImageIndex::with_fallback`: the set's images are
  searched after the version's own, by path / file name only (`resolved_by: "texture-set"`,
  siblings included) — material-name heuristics stay on own files. Set images are listed in
  `manifest.textures` (with previews) only when a model is built; `manifest.texture_sets`
  records them in config order. A new head of an asset re-queues the head versions whose
  manifest lists it (`jobs::requeue_texture_set_users`, on commit and on head change);
  `hangar_core::texture_location` maps `@slug/…` paths to the set's version for downloads.
- Recoverable problems go to `manifest.warnings`, not errors. Fail only when nothing can be previewed.
- Heuristics (texture resolution, slot tokens, variant detection, LOD suffixes) are unit-tested in
  their modules; extend those tests when changing them.
- `hangar-pipeline` is built with `opt-level = 2` even in dev (pixel/vertex loops).

## Web client

- All HTTP goes through `src/api.rs` (gloo-net, same-origin cookies). On 401, redirect to `/login`.
- The Bevy viewer never touches the network. Leptos downloads GLB / texture bytes and sends
  `viewer::ViewerCommand` messages (`LoadModel { key, asset, bytes }`, `ApplyVariant`, …); Bevy
  answers with `ViewerEvent`. GLBs are loaded from an in-memory asset source (`mem://`).
  The camera is re-framed only when `asset` changes, so switching LOD/version gives an A/B compare.
- Uploads (`upload.rs`): hash in wasm with BLAKE3 in chunks → `POST /api/blobs/missing` → `PUT` only
  missing blobs (browser streams the `File`) → `POST /api/assets/{id}/versions`.
- UI text is Brazilian Portuguese.

## Version-locked dependencies (check before upgrading)

- `leptos-bevy-canvas 0.6` ⇔ **Leptos 0.8** + **Bevy 0.19** (compatibility table on docs.rs).
  Bump all three together.
- **SeaORM 2.0**: raw SQL uses `execute_raw(Statement)` / `query_one_raw(Statement)`;
  `execute(&S)` / `query_one(&S)` are for query-builder statements. Entities use the compact
  `DeriveEntityModel` format (`entities.rs`); complex reads use `FromQueryResult` + raw SQL.
- **summer-rs 0.7**: scheduler futures must return `summer::error::Result<String>`;
  plugin dependency names are `std::any::type_name::<Plugin>()`.
- **OpenDAL 0.55**, **ufbx 0.10**, **meshopt 0.7**, **gltf 1.4**, `wasm-bindgen` CLI version must
  match the crate version in `Cargo.lock` (pinned in `crates/hangar-web/Trunk.toml`).
- Rust 2024 edition: an `async fn … -> impl IntoResponse` that borrows its arguments captures their
  lifetimes — return a concrete type instead.

## Known gaps / next steps

- Skeleton GLBs are baked per animation version (copy of the mesh each); a new skeleton
  version only applies after `POST /api/skeletons/{id}/reprocess`.
- No DDS/PSD/EXR texture previews. Animation is exported for FBX and glTF/GLB (node + skeletal, no
  morph targets); OBJ is static. Separate animation files and reference skeletons are FBX only.
- `glb::clean_keys` drops keys that interpolation reproduces (0.1 mm / ~0.01°); clips are still
  repeated in every LOD GLB.
- Blender fallback is implemented but was never run against a real Blender in CI.
- The S3 storage backend is compiled in but only the `fs` backend has been exercised end-to-end.
- Sessions are in memory by default (`--features redis-sessions` for Redis); blob downloads by hash
  only require login (the hash acts as a capability).
