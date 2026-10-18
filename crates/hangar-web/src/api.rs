//! Typed REST client for the Hangar API.
//!
//! Requests are same-origin, so the `hangar_token` HttpOnly cookie set at login
//! is sent automatically. A 401 anywhere (except on the auth endpoints
//! themselves) bounces the browser to `/login`.

use gloo_net::http::{Request, RequestBuilder, Response};
use hangar_core::*;
use serde::{Serialize, de::DeserializeOwned};
use std::fmt;
use uuid::Uuid;

/// Error surfaced to the UI. `status == 0` means a network failure.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            0 => write!(f, "Falha de rede: {}", self.message),
            401 => write!(f, "Sessão expirada — faça login novamente"),
            403 => write!(f, "Sem permissão: {}", self.message),
            404 => write!(f, "Não encontrado: {}", self.message),
            s => write!(f, "Erro {s}: {}", self.message),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

fn net_err(e: impl fmt::Display) -> ApiError {
    ApiError { status: 0, message: e.to_string() }
}

fn redirect_to_login() {
    if let Some(w) = web_sys::window() {
        let loc = w.location();
        let path = loc.pathname().unwrap_or_default();
        if path != "/login" {
            let _ = loc.set_href("/login");
        }
    }
}

async fn check(resp: Response, url: &str) -> ApiResult<Response> {
    if resp.ok() {
        return Ok(resp);
    }
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if status == 401 && !url.starts_with("/api/auth/") && !url.starts_with("/api/setup") {
        redirect_to_login();
    }
    let message = if body.trim().is_empty() { resp.status_text() } else { body };
    Err(ApiError { status, message })
}

async fn send(builder: RequestBuilder, url: &str) -> ApiResult<Response> {
    let resp = builder.send().await.map_err(net_err)?;
    check(resp, url).await
}

async fn send_req(req: Request, url: &str) -> ApiResult<Response> {
    let resp = req.send().await.map_err(net_err)?;
    check(resp, url).await
}

pub async fn get_json<T: DeserializeOwned>(url: &str) -> ApiResult<T> {
    let resp = send(Request::get(url), url).await?;
    resp.json::<T>().await.map_err(net_err)
}

pub async fn get_bytes(url: &str) -> ApiResult<Vec<u8>> {
    let resp = send(Request::get(url), url).await?;
    resp.binary().await.map_err(net_err)
}

async fn body_json<B: Serialize, T: DeserializeOwned>(builder: RequestBuilder, url: &str, body: &B) -> ApiResult<T> {
    let req = builder.json(body).map_err(net_err)?;
    let resp = send_req(req, url).await?;
    resp.json::<T>().await.map_err(net_err)
}

async fn no_content(builder: RequestBuilder, url: &str) -> ApiResult<()> {
    send(builder, url).await.map(|_| ())
}

fn qs(pairs: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.as_ref().filter(|v| !v.is_empty()).map(|v| format!("{k}={}", encode(v))))
        .collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

/// Percent-encodes a query/path component.
pub fn encode(s: &str) -> String {
    js_sys::encode_uri_component(s).as_string().unwrap_or_default()
}

/// Encodes a version-relative path segment by segment (keeps the slashes).
pub fn encode_path(p: &str) -> String {
    p.split('/').map(encode).collect::<Vec<_>>().join("/")
}

// ------------------------------------------------------------------- setup/auth

pub async fn setup_status() -> ApiResult<SetupStatus> {
    get_json("/api/setup").await
}

pub async fn login(username: String, password: String) -> ApiResult<LoginResponse> {
    let url = "/api/auth/login";
    body_json(Request::post(url), url, &LoginRequest { username, password }).await
}

pub async fn register(req: RegisterRequest) -> ApiResult<LoginResponse> {
    let url = "/api/auth/register";
    body_json(Request::post(url), url, &req).await
}

pub async fn logout() -> ApiResult<()> {
    no_content(Request::post("/api/auth/logout"), "/api/auth/logout").await
}

pub async fn me() -> ApiResult<UserDto> {
    get_json("/api/auth/me").await
}

// --------------------------------------------------------------------- projects

pub async fn projects() -> ApiResult<Vec<ProjectDto>> {
    get_json("/api/projects").await
}

pub async fn project(pid: Uuid) -> ApiResult<ProjectDto> {
    get_json(&format!("/api/projects/{pid}")).await
}

pub async fn create_project(req: CreateProjectRequest) -> ApiResult<ProjectDto> {
    body_json(Request::post("/api/projects"), "/api/projects", &req).await
}

pub async fn members(pid: Uuid) -> ApiResult<Vec<MemberDto>> {
    get_json(&format!("/api/projects/{pid}/members")).await
}

pub async fn add_member(pid: Uuid, req: AddMemberRequest) -> ApiResult<MemberDto> {
    let url = format!("/api/projects/{pid}/members");
    body_json(Request::post(&url), &url, &req).await
}

pub async fn remove_member(pid: Uuid, uid: Uuid) -> ApiResult<()> {
    let url = format!("/api/projects/{pid}/members/{uid}");
    no_content(Request::delete(&url), &url).await
}

// ----------------------------------------------------------------------- assets

pub async fn assets(pid: Uuid, q: &AssetQuery) -> ApiResult<Page<AssetSummary>> {
    let url = format!(
        "/api/projects/{pid}/assets{}",
        qs(&[
            ("q", q.q.clone()),
            ("kind", q.kind.map(|k| k.as_str().to_string())),
            ("tag", q.tag.clone()),
            ("page", q.page.map(|p| p.to_string())),
            ("size", q.size.map(|s| s.to_string())),
        ])
    );
    get_json(&url).await
}

pub async fn create_asset(pid: Uuid, req: CreateAssetRequest) -> ApiResult<AssetDetail> {
    let url = format!("/api/projects/{pid}/assets");
    body_json(Request::post(&url), &url, &req).await
}

pub async fn asset(aid: Uuid) -> ApiResult<AssetDetail> {
    get_json(&format!("/api/assets/{aid}")).await
}

pub async fn update_asset(aid: Uuid, req: UpdateAssetRequest) -> ApiResult<AssetDetail> {
    let url = format!("/api/assets/{aid}");
    body_json(Request::patch(&url), &url, &req).await
}

pub async fn delete_asset(aid: Uuid) -> ApiResult<()> {
    let url = format!("/api/assets/{aid}");
    no_content(Request::delete(&url), &url).await
}

// --------------------------------------------------------------------- versions

pub async fn versions(aid: Uuid) -> ApiResult<Vec<VersionDto>> {
    get_json(&format!("/api/assets/{aid}/versions")).await
}

pub async fn commit_version(aid: Uuid, req: &CommitVersionRequest) -> ApiResult<VersionDto> {
    let url = format!("/api/assets/{aid}/versions");
    body_json(Request::post(&url), &url, req).await
}

#[allow(dead_code)]
pub async fn version(vid: Uuid) -> ApiResult<VersionDto> {
    get_json(&format!("/api/versions/{vid}")).await
}

pub async fn version_files(vid: Uuid) -> ApiResult<Vec<FileEntry>> {
    get_json(&format!("/api/versions/{vid}/files")).await
}

pub async fn version_diff(vid: Uuid, against: Option<Uuid>) -> ApiResult<VersionDiff> {
    let url = format!(
        "/api/versions/{vid}/diff{}",
        qs(&[("against", against.map(|a| a.to_string()))])
    );
    get_json(&url).await
}

pub async fn reprocess(vid: Uuid) -> ApiResult<VersionDto> {
    let url = format!("/api/versions/{vid}/reprocess");
    let resp = send(Request::post(&url), &url).await?;
    resp.json().await.map_err(net_err)
}

pub fn raw_file_url(vid: Uuid, path: &str, download: bool) -> String {
    format!(
        "/api/versions/{vid}/raw/{}{}",
        encode_path(path),
        if download { "?download=1" } else { "" }
    )
}

pub fn archive_url(vid: Uuid) -> String {
    format!("/api/versions/{vid}/archive")
}

/// The manual material configuration of a version; `None` when it has none.
pub async fn material_config(vid: Uuid) -> ApiResult<Option<MaterialConfig>> {
    match get_bytes(&raw_file_url(vid, MATERIAL_CONFIG_PATH, false)).await {
        Ok(bytes) => MaterialConfig::from_json(&bytes)
            .map(Some)
            .map_err(|e| ApiError { status: 422, message: format!("{MATERIAL_CONFIG_PATH} inválido: {e}") }),
        Err(e) if e.status == 404 => Ok(None),
        Err(e) => Err(e),
    }
}

/// The animation binding (reference skeletons) of a version; `None` when it has none.
pub async fn animation_config(vid: Uuid) -> ApiResult<Option<AnimationConfig>> {
    match get_bytes(&raw_file_url(vid, ANIMATION_CONFIG_PATH, false)).await {
        Ok(bytes) => AnimationConfig::from_json(&bytes)
            .map(Some)
            .map_err(|e| ApiError { status: 422, message: format!("{ANIMATION_CONFIG_PATH} inválido: {e}") }),
        Err(e) if e.status == 404 => Ok(None),
        Err(e) => Err(e),
    }
}

// ----------------------------------------------------------- reference skeletons

pub async fn skeletons() -> ApiResult<Vec<SkeletonDto>> {
    get_json("/api/skeletons").await
}

pub async fn create_skeleton(req: CreateSkeletonRequest) -> ApiResult<SkeletonDto> {
    let url = "/api/skeletons";
    body_json(Request::post(url), url, &req).await
}

pub async fn update_skeleton(id: Uuid, req: UpdateSkeletonRequest) -> ApiResult<SkeletonDto> {
    let url = format!("/api/skeletons/{id}");
    body_json(Request::patch(&url), &url, &req).await
}

pub async fn delete_skeleton(id: Uuid) -> ApiResult<()> {
    let url = format!("/api/skeletons/{id}");
    no_content(Request::delete(&url), &url).await
}

pub async fn reprocess_skeleton(id: Uuid) -> ApiResult<ReprocessQueued> {
    let url = format!("/api/skeletons/{id}/reprocess");
    let resp = send(Request::post(&url), &url).await?;
    resp.json().await.map_err(net_err)
}

// ------------------------------------------------------------------------ blobs

pub async fn missing_blobs(hashes: Vec<String>) -> ApiResult<Vec<String>> {
    let url = "/api/blobs/missing";
    let r: MissingBlobsResponse = body_json(Request::post(url), url, &MissingBlobsRequest { hashes }).await?;
    Ok(r.missing)
}

/// Stores in-memory bytes as a blob (skipped when the server already has it)
/// and returns the file entry to commit at `path`.
pub async fn upload_bytes(path: &str, bytes: &[u8]) -> ApiResult<FileEntry> {
    let hash = blake3::hash(bytes).to_hex().to_string();
    if !missing_blobs(vec![hash.clone()]).await?.is_empty() {
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
        let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(|e| net_err(format!("{e:?}")))?;
        put_blob(&hash, &blob).await?;
    }
    Ok(FileEntry { path: path.to_string(), hash, size: bytes.len() as i64 })
}

/// Uploads a blob; the browser streams the `File`/`Blob` directly.
pub async fn put_blob(hash: &str, body: &web_sys::Blob) -> ApiResult<BlobUploaded> {
    let url = format!("/api/blobs/{hash}");
    let req = Request::put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(wasm_bindgen::JsValue::from(body.clone()))
        .map_err(net_err)?;
    let resp = send_req(req, &url).await?;
    resp.json().await.map_err(net_err)
}

pub async fn blob_bytes(hash: &str) -> ApiResult<Vec<u8>> {
    get_bytes(&blob_url(hash)).await
}
