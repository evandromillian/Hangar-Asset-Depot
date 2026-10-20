//! Editor plugins: access tokens, device pairing and the "send to editor"
//! bridge (`/api/tokens`, `/api/devices/…`, `/api/bridge/…`).
//!
//! A plugin pairs once (device flow: it shows a code, the user approves it on
//! the web) and keeps the `hgr_…` token it gets. It then holds
//! `GET /api/bridge/events` open (server-sent events); the web sends versions to
//! it through the server, so the plugin only makes outbound HTTPS requests.

use crate::api::{FileEntry, UserDto, VersionDto};
use crate::enums::AssetKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Every access token starts with this, so the server tells them from session tokens.
pub const API_TOKEN_PREFIX: &str = "hgr_";

// ------------------------------------------------------------- access tokens

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiTokenDto {
    pub id: Uuid,
    /// "Unreal · DESKTOP-01 · HumanityFading"
    pub name: String,
    /// Which program uses it: "unreal", "blender", "cli".
    pub client: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

// ------------------------------------------------------------ device pairing

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStartRequest {
    pub client: String,
    /// Machine name.
    pub device_name: String,
    /// Shown on the approval page, e.g. "HumanityFading · UE 5.8.1".
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairStartResponse {
    /// Secret the plugin polls with. Never shown to the user.
    pub device_code: String,
    /// Short code the user checks on the approval page ("KXRT-4QPM").
    pub user_code: String,
    /// Web page that approves this pairing, relative to the server root.
    pub verify_path: String,
    pub expires_in: u32,
    /// Seconds between polls.
    pub interval: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairPollRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairPollResponse {
    pub status: PairStatus,
    /// The access token, returned once when the pairing is approved.
    pub token: Option<String>,
    pub user: Option<UserDto>,
}

/// What the approval page shows about a pending pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairInfo {
    pub user_code: String,
    pub client: String,
    pub device_name: String,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairDecision {
    pub approve: bool,
}

// -------------------------------------------------------------------- bridge

/// An editor holding the events stream open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeClientDto {
    pub id: Uuid,
    /// "unreal"
    pub app: String,
    /// Machine name.
    pub name: String,
    /// Project / engine, e.g. "HumanityFading · UE 5.8.1".
    pub detail: Option<String>,
    pub connected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSendRequest {
    pub client_id: Uuid,
    pub version_id: Uuid,
}

/// First event of the stream (`event: hello`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeHello {
    pub client_id: Uuid,
    pub user: UserDto,
}

/// `event: import`: bring this version into the editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeImport {
    pub delivery_id: Uuid,
    pub project_id: Uuid,
    pub project_slug: String,
    pub project_name: String,
    pub asset_id: Uuid,
    pub asset_slug: String,
    pub asset_name: String,
    pub kind: AssetKind,
    pub version_id: Uuid,
    pub version_number: i32,
    /// Display name of who clicked "Enviar".
    pub sent_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Pushed to the stream; the editor hasn't answered yet.
    Sent,
    Downloading,
    Importing,
    Done,
    Failed,
}

impl DeliveryState {
    pub fn is_final(self) -> bool {
        matches!(self, DeliveryState::Done | DeliveryState::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryDto {
    pub id: Uuid,
    pub client_id: Uuid,
    pub client_name: String,
    pub asset_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub state: DeliveryState,
    /// Progress of the current state, 0..1.
    pub progress: Option<f32>,
    /// Error or summary from the editor.
    pub message: Option<String>,
    /// Editor object paths of what was imported.
    #[serde(default)]
    pub imported: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// The editor reports progress on a delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryUpdate {
    pub state: DeliveryState,
    #[serde(default)]
    pub progress: Option<f32>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub imported: Vec<String>,
}

/// Everything an editor downloads for a version: its own files plus the
/// texture-set images its materials use (under `@slug/`, as in the zip).
/// Files are fetched by hash from `/api/blobs/{hash}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionBundle {
    pub version: VersionDto,
    pub files: Vec<FileEntry>,
}
