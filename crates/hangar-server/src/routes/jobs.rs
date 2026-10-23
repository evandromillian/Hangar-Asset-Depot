use crate::auth::CurrentUser;
use crate::error::{Ctx, Result};
use hangar_core::JobDto;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Deserialize;
use summer_web::extractor::{Component, Json, Query};
use crate::storage::BlobStore;
use summer_web::{get, post};

#[derive(Deserialize)]
struct JobsQuery {
    status: Option<String>,
}

#[derive(FromQueryResult)]
struct Row {
    id: i64,
    kind: String,
    status: String,
    attempts: i32,
    last_error: Option<String>,
    payload: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[get("/api/jobs")]
async fn list_jobs(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Query(q): Query<JobsQuery>,
) -> Result<Json<Vec<JobDto>>> {
    user.require_admin()?;
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, kind, status, attempts, last_error, payload, created_at, finished_at
         FROM jobs WHERE ($1::text IS NULL OR status = $1) ORDER BY id DESC LIMIT 200",
        [q.status.into()],
    ))
    .all(&db)
    .await
    .ctx("list jobs")?;
    Ok(Json(
        rows.into_iter()
            .map(|r| JobDto {
                id: r.id,
                kind: r.kind,
                status: r.status,
                attempts: r.attempts,
                last_error: r.last_error,
                payload: r.payload,
                created_at: r.created_at,
                finished_at: r.finished_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct GcQuery {
    /// Keep unreferenced blobs younger than this (default 24h).
    grace_hours: Option<i64>,
}

#[derive(serde::Serialize)]
struct GcResult {
    deleted: u64,
    freed_bytes: i64,
}

/// Garbage-collects blobs no longer referenced by any version or rendition
/// (e.g. after deleting assets).
#[post("/api/admin/gc")]
async fn gc_blobs(
    user: CurrentUser,
    Component(store): Component<BlobStore>,
    Query(q): Query<GcQuery>,
) -> Result<Json<GcResult>> {
    user.require_admin()?;
    // Bounded so the cast to SQL int can't wrap into a negative grace (= delete in-flight uploads).
    let (deleted, freed_bytes) = store.gc(q.grace_hours.unwrap_or(24).clamp(0, 24 * 365 * 10)).await?;
    tracing::info!(deleted, freed_bytes, "blob gc finished");
    Ok(Json(GcResult { deleted, freed_bytes }))
}
