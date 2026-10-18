//! Reference skeletons (system setting): which model assets animation-only
//! versions can be previewed on. Everyone logged in can list them (the asset
//! page offers them for binding); only global admins manage them.

use crate::auth::CurrentUser;
use crate::entities::{assets, skeletons};
use crate::error::{Ctx, Result, bad_request, conflict, not_found, slugify};
use crate::jobs::{JobQueue, KIND_PROCESS_VERSION, ProcessVersionPayload};
use hangar_core::{
    CreateSkeletonRequest, ReprocessQueued, SkeletonDto, UpdateSkeletonRequest, VersionBrief, VersionStatus,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult,
    IntoActiveModel, QueryFilter, Set, Statement, TransactionTrait, Value,
};
use summer_web::axum::http::StatusCode;
use summer_web::extractor::{Component, Json, Path};
use summer_web::{delete, get, patch, post};
use uuid::Uuid;

#[derive(FromQueryResult)]
struct SkeletonRow {
    id: Uuid,
    slug: String,
    name: String,
    description: String,
    asset_id: Uuid,
    asset_name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    head_version_id: Option<Uuid>,
    head_number: Option<i32>,
    head_status: Option<String>,
    thumb: Option<String>,
    has_skin: Option<bool>,
    animation_count: i64,
    stale_count: i64,
}

/// Head versions of other assets whose manifest has an entry for the skeleton
/// `s` (aliased in the outer query, `a` being its asset).
const BOUND_HEADS: &str = "FROM assets aa
     JOIN asset_versions v ON v.id = aa.head_version_id
     CROSS JOIN LATERAL jsonb_array_elements(COALESCE(v.preview->'skeletons', '[]'::jsonb)) e
     WHERE e->>'skeleton' = s.slug";

async fn load(db: &DatabaseConnection, id: Option<Uuid>) -> Result<Vec<SkeletonDto>> {
    let (filter, values): (&str, Vec<Value>) = match id {
        Some(id) => ("WHERE s.id = $1", vec![id.into()]),
        None => ("", vec![]),
    };
    let rows = SkeletonRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT s.id, s.slug, s.name, s.description, s.asset_id, s.created_at,
                    a.name AS asset_name, a.head_version_id,
                    hv.number AS head_number, hv.status AS head_status,
                    hv.preview->>'thumbnail_hash' AS thumb,
                    (hv.preview->'stats'->>'has_skin')::boolean AS has_skin,
                    (SELECT count(DISTINCT v.id) {BOUND_HEADS}) AS animation_count,
                    (SELECT count(DISTINCT v.id) {BOUND_HEADS}
                        AND (e->>'version_id') IS DISTINCT FROM a.head_version_id::text) AS stale_count
             FROM skeletons s
             JOIN assets a ON a.id = s.asset_id
             LEFT JOIN asset_versions hv ON hv.id = a.head_version_id
             {filter}
             ORDER BY lower(s.name)"
        ),
        values,
    ))
    .all(db)
    .await
    .ctx("list skeletons")?;
    Ok(rows
        .into_iter()
        .map(|r| SkeletonDto {
            id: r.id,
            slug: r.slug,
            name: r.name,
            description: r.description,
            asset_id: r.asset_id,
            asset_name: r.asset_name,
            head: match (r.head_version_id, r.head_number) {
                (Some(id), Some(number)) => Some(VersionBrief {
                    id,
                    number,
                    status: r.head_status.and_then(|s| s.parse().ok()).unwrap_or(VersionStatus::Pending),
                }),
                _ => None,
            },
            thumbnail_url: r.thumb.map(|h| hangar_core::blob_url(&h)),
            has_skin: r.has_skin,
            animation_count: r.animation_count,
            stale_count: r.stale_count,
            created_at: r.created_at,
        })
        .collect())
}

async fn load_one(db: &DatabaseConnection, id: Uuid) -> Result<SkeletonDto> {
    load(db, Some(id)).await?.into_iter().next().ok_or_else(|| not_found("esqueleto não encontrado"))
}

/// The asset must exist and not be registered under another skeleton.
async fn check_asset(db: &DatabaseConnection, asset_id: Uuid, except: Option<Uuid>) -> Result<()> {
    assets::Entity::find_by_id(asset_id)
        .one(db)
        .await
        .ctx("load asset")?
        .ok_or_else(|| bad_request("asset não encontrado"))?;
    let taken = skeletons::Entity::find()
        .filter(skeletons::Column::AssetId.eq(asset_id))
        .one(db)
        .await
        .ctx("check asset")?
        .filter(|s| Some(s.id) != except);
    match taken {
        Some(s) => Err(conflict(format!("este asset já é o esqueleto \"{}\"", s.name))),
        None => Ok(()),
    }
}

#[get("/api/skeletons")]
async fn list_skeletons(_user: CurrentUser, Component(db): Component<DatabaseConnection>) -> Result<Json<Vec<SkeletonDto>>> {
    Ok(Json(load(&db, None).await?))
}

#[post("/api/skeletons")]
async fn create_skeleton(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Json(req): Json<CreateSkeletonRequest>,
) -> Result<Json<SkeletonDto>> {
    user.require_admin()?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("nome obrigatório"));
    }
    let slug = slugify(req.slug.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&name));
    let exists = skeletons::Entity::find()
        .filter(skeletons::Column::Slug.eq(&slug))
        .one(&db)
        .await
        .ctx("check slug")?;
    if exists.is_some() {
        return Err(conflict(format!("já existe um esqueleto com o identificador \"{slug}\"")));
    }
    check_asset(&db, req.asset_id, None).await?;
    let model = skeletons::ActiveModel {
        id: Set(Uuid::now_v7()),
        slug: Set(slug),
        name: Set(name),
        description: Set(req.description.trim().to_string()),
        asset_id: Set(req.asset_id),
        created_by: Set(Some(user.0.id)),
        created_at: Set(chrono::Utc::now()),
    }
    .insert(&db)
    .await
    .ctx("insert skeleton")?;
    Ok(Json(load_one(&db, model.id).await?))
}

#[patch("/api/skeletons/{id}")]
async fn update_skeleton(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateSkeletonRequest>,
) -> Result<Json<SkeletonDto>> {
    user.require_admin()?;
    let s = skeletons::Entity::find_by_id(id)
        .one(&db)
        .await
        .ctx("load skeleton")?
        .ok_or_else(|| not_found("esqueleto não encontrado"))?;
    if let Some(a) = req.asset_id {
        check_asset(&db, a, Some(s.id)).await?;
    }
    let mut m = s.into_active_model();
    if let Some(name) = req.name.map(|n| n.trim().to_string()) {
        if name.is_empty() {
            return Err(bad_request("nome obrigatório"));
        }
        m.name = Set(name);
    }
    if let Some(d) = req.description {
        m.description = Set(d.trim().to_string());
    }
    if let Some(a) = req.asset_id {
        m.asset_id = Set(a);
    }
    m.update(&db).await.ctx("update skeleton")?;
    Ok(Json(load_one(&db, id).await?))
}

/// Versions already built on the skeleton keep their previews until reprocessed.
#[delete("/api/skeletons/{id}")]
async fn delete_skeleton(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    user.require_admin()?;
    let res = skeletons::Entity::delete_by_id(id).exec(&db).await.ctx("delete skeleton")?;
    if res.rows_affected == 0 {
        return Err(not_found("esqueleto não encontrado"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

/// Re-runs the pipeline on the head versions previewed on this skeleton, so
/// they pick up its current head version (after a new skeleton upload).
#[post("/api/skeletons/{id}/reprocess")]
async fn reprocess_skeleton_animations(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(queue): Component<JobQueue>,
    Path(id): Path<Uuid>,
) -> Result<Json<ReprocessQueued>> {
    user.require_admin()?;
    let txn = db.begin().await.ctx("begin")?;
    let ids = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT DISTINCT b.id FROM skeletons s, LATERAL (SELECT v.id {BOUND_HEADS}) b WHERE s.id = $1"),
        [id.into()],
    ))
    .all(&txn)
    .await
    .ctx("find bound versions")?;
    for row in &ids {
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE asset_versions SET status = 'pending', error = NULL WHERE id = $1",
            [row.id.into()],
        ))
        .await
        .ctx("reset status")?;
        queue
            .enqueue(&txn, KIND_PROCESS_VERSION, ProcessVersionPayload { version_id: row.id })
            .await
            .map_err(|e| e.context("enqueue"))?;
    }
    txn.commit().await.ctx("commit")?;
    queue.wake();
    Ok(Json(ReprocessQueued { queued: ids.len() as u64 }))
}
