//! Request / response bodies of the REST API (`/api/...`).

use crate::enums::{AssetKind, Role, VersionStatus};
use crate::preview::PreviewManifest;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------- auth / users

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    /// Also set as an HttpOnly cookie; returned for CLI / API clients.
    pub token: String,
    pub user: UserDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatus {
    /// True when no user exists yet: the first registered user becomes admin.
    pub needs_setup: bool,
    pub allow_signup: bool,
    pub blender_available: bool,
    pub version: String,
}

// ------------------------------------------------------------------- projects

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDto {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// Role of the calling user (admins see every project).
    pub my_role: Option<Role>,
    pub asset_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemberDto {
    pub user: UserDto,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMemberRequest {
    pub username: String,
    pub role: Role,
}

// --------------------------------------------------------------------- assets

/// Where an asset came from — useful for purchased content (license, store, order).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetSource {
    /// Store / marketplace: "Fab", "CGTrader", "TurboSquid", "Sketchfab", "in-house"...
    pub vendor: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub order_ref: Option<String>,
    pub purchased_at: Option<NaiveDate>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionBrief {
    pub id: Uuid,
    pub number: i32,
    pub status: VersionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetSummary {
    pub id: Uuid,
    pub project_id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: AssetKind,
    pub tags: Vec<String>,
    pub head: Option<VersionBrief>,
    pub version_count: i64,
    pub thumbnail_url: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetDetail {
    #[serde(flatten)]
    pub summary: AssetSummary,
    pub description: String,
    pub source: AssetSource,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub my_role: Option<Role>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAssetRequest {
    pub name: String,
    pub slug: Option<String>,
    pub kind: AssetKind,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: AssetSource,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateAssetRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<AssetKind>,
    pub tags: Option<Vec<String>>,
    pub source: Option<AssetSource>,
    /// Promote a version to be the one shown by default ("head").
    pub head_version_id: Option<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetQuery {
    pub q: Option<String>,
    pub kind: Option<AssetKind>,
    pub tag: Option<String>,
    pub page: Option<u64>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub size: u64,
    pub total: u64,
}

// ------------------------------------------------------------------- versions

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionDto {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub number: i32,
    pub parent_id: Option<Uuid>,
    pub message: String,
    pub author: Option<String>,
    pub status: VersionStatus,
    pub error: Option<String>,
    pub file_count: i32,
    pub total_size: i64,
    pub is_head: bool,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub preview: Option<PreviewManifest>,
}

impl VersionDto {
    pub fn thumbnail_url(&self) -> Option<String> {
        self.preview
            .as_ref()
            .and_then(|p| p.thumbnail_hash.as_deref())
            .map(crate::blob_url)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub size: i64,
}

/// Create a new version.
///
/// With `parent_id` set and `inherit` true (the default) the new version
/// starts as a copy of the parent's file tree, then `upserts` are applied and
/// `deletes` removed — so fixing one mesh only uploads that mesh. All hashes
/// in `upserts` must have been uploaded with `PUT /api/blobs/{hash}` first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitVersionRequest {
    pub parent_id: Option<Uuid>,
    pub message: String,
    #[serde(default = "default_true")]
    pub inherit: bool,
    #[serde(default)]
    pub upserts: Vec<FileEntry>,
    #[serde(default)]
    pub deletes: Vec<String>,
    /// Make the new version the asset head (default true).
    #[serde(default = "default_true")]
    pub set_head: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingBlobsRequest {
    pub hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingBlobsResponse {
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobUploaded {
    pub hash: String,
    pub size: i64,
    /// False when the blob already existed (deduplicated).
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModifiedEntry {
    pub path: String,
    pub old_hash: String,
    pub new_hash: String,
    pub old_size: i64,
    pub new_size: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionDiff {
    pub base_id: Option<Uuid>,
    pub target_id: Uuid,
    pub added: Vec<FileEntry>,
    pub removed: Vec<FileEntry>,
    pub modified: Vec<ModifiedEntry>,
    pub unchanged: usize,
}

impl VersionDiff {
    /// Compares two file trees (both sorted or not).
    pub fn compute(base_id: Option<Uuid>, base: &[FileEntry], target_id: Uuid, target: &[FileEntry]) -> Self {
        use std::collections::BTreeMap;
        let b: BTreeMap<&str, &FileEntry> = base.iter().map(|f| (f.path.as_str(), f)).collect();
        let t: BTreeMap<&str, &FileEntry> = target.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut diff = VersionDiff {
            base_id,
            target_id,
            added: vec![],
            removed: vec![],
            modified: vec![],
            unchanged: 0,
        };
        for (path, tf) in &t {
            match b.get(path) {
                None => diff.added.push((*tf).clone()),
                Some(bf) if bf.hash != tf.hash => diff.modified.push(ModifiedEntry {
                    path: path.to_string(),
                    old_hash: bf.hash.clone(),
                    new_hash: tf.hash.clone(),
                    old_size: bf.size,
                    new_size: tf.size,
                }),
                Some(_) => diff.unchanged += 1,
            }
        }
        for (path, bf) in &b {
            if !t.contains_key(path) {
                diff.removed.push((*bf).clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffQuery {
    /// Version to compare against (defaults to the parent).
    pub against: Option<Uuid>,
}

// ------------------------------------------------------- reference skeletons

/// A system-wide reference skeleton: animation-only versions bound to it (see
/// [`AnimationConfig`](crate::AnimationConfig)) are previewed on the mesh of
/// `asset_id`'s head version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkeletonDto {
    pub id: Uuid,
    /// Immutable: it is what `.hangar/animation.json` refers to.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub asset_id: Uuid,
    pub asset_name: String,
    pub head: Option<VersionBrief>,
    pub thumbnail_url: Option<String>,
    /// Whether the head version's mesh is skinned (None: not processed yet).
    pub has_skin: Option<bool>,
    /// Head versions of animation assets previewed on this skeleton...
    pub animation_count: i64,
    /// ...and how many of them were built from an older skeleton version.
    pub stale_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSkeletonRequest {
    pub name: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub description: String,
    pub asset_id: Uuid,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateSkeletonRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub asset_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReprocessQueued {
    pub queued: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDto {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Generic error body returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fe(p: &str, h: &str) -> FileEntry {
        FileEntry { path: p.into(), hash: h.into(), size: 1 }
    }

    #[test]
    fn diff_trees() {
        let base = vec![fe("tank.fbx", "a"), fe("tex/d.png", "b"), fe("old.txt", "c")];
        let target = vec![fe("tank.fbx", "a2"), fe("tex/d.png", "b"), fe("tex/n.png", "d")];
        let d = VersionDiff::compute(None, &base, Uuid::nil(), &target);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].path, "tex/n.png");
        assert_eq!(d.removed[0].path, "old.txt");
        assert_eq!(d.modified[0].path, "tank.fbx");
        assert_eq!(d.unchanged, 1);
    }

    #[test]
    fn commit_defaults() {
        let r: CommitVersionRequest = serde_json::from_str(r#"{"parent_id":null,"message":"x"}"#).unwrap();
        assert!(r.inherit && r.set_head);
        assert!(r.upserts.is_empty());
    }
}
