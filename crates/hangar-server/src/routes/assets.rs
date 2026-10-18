use crate::access::{self, Need};
use crate::auth::CurrentUser;
use crate::dto;
use crate::entities::{asset_versions, assets};
use crate::error::{Ctx, Result, bad_request, slugify};
use hangar_core::{
    AssetDetail, AssetKind, AssetQuery, AssetSummary, CreateAssetRequest, Page, UpdateAssetRequest, VersionBrief,
    VersionDto,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, IntoActiveModel,
    QueryFilter, QueryOrder, Set, Statement, Value,
};
use summer_web::axum::http::StatusCode;
use summer_web::extractor::{Component, Json, Path, Query};
use summer_web::{delete, get, patch, post};
use uuid::Uuid;

#[derive(FromQueryResult)]
struct AssetRow {
    id: Uuid,
    project_id: Uuid,
    slug: String,
    name: String,
    kind: String,
    tags: serde_json::Value,
    updated_at: chrono::DateTime<chrono::Utc>,
    head_version_id: Option<Uuid>,
    head_number: Option<i32>,
    head_status: Option<String>,
    thumb: Option<String>,
    version_count: i64,
}

#[derive(FromQueryResult)]
struct CountRow {
    total: i64,
}

fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && t.len() <= 40)
        .collect();
    out.sort();
    out.dedup();
    out.truncate(32);
    out
}

#[get("/api/projects/{pid}/assets")]
async fn list_assets(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(pid): Path<Uuid>,
    Query(q): Query<AssetQuery>,
) -> Result<Json<Page<AssetSummary>>> {
    access::project(&db, &user, pid, Need::Read).await?;

    let size = q.size.unwrap_or(48).clamp(1, 200);
    let page = q.page.unwrap_or(0);
    let mut where_sql = String::from("a.project_id = $1");
    let mut values: Vec<Value> = vec![pid.into()];
    if let Some(text) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        values.push(format!("%{}%", text.replace('%', "\\%").replace('_', "\\_")).into());
        let n = values.len();
        where_sql.push_str(&format!(
            " AND (a.name ILIKE ${n} OR a.slug ILIKE ${n} OR a.description ILIKE ${n} OR a.tags::text ILIKE ${n})"
        ));
    }
    if let Some(kind) = q.kind {
        values.push(kind.as_str().into());
        where_sql.push_str(&format!(" AND a.kind = ${}", values.len()));
    }
    if let Some(tag) = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        values.push(tag.to_lowercase().into());
        where_sql.push_str(&format!(" AND a.tags @> jsonb_build_array(${}::text)", values.len()));
    }

    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT count(*) AS total FROM assets a WHERE {where_sql}"),
        values.clone(),
    ))
    .one(&db)
    .await
    .ctx("count assets")?
    .map(|r| r.total)
    .unwrap_or(0);

    values.push((size as i64).into());
    let limit_n = values.len();
    values.push(((page * size) as i64).into());
    let offset_n = values.len();
    let rows = AssetRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT a.id, a.project_id, a.slug, a.name, a.kind, a.tags, a.updated_at, a.head_version_id,
                    hv.number AS head_number, hv.status AS head_status,
                    hv.preview->>'thumbnail_hash' AS thumb,
                    (SELECT count(*) FROM asset_versions v WHERE v.asset_id = a.id) AS version_count
             FROM assets a
             LEFT JOIN asset_versions hv ON hv.id = a.head_version_id
             WHERE {where_sql}
             ORDER BY a.updated_at DESC
             LIMIT ${limit_n} OFFSET ${offset_n}"
        ),
        values,
    ))
    .all(&db)
    .await
    .ctx("list assets")?;

    let items = rows
        .into_iter()
        .map(|r| AssetSummary {
            id: r.id,
            project_id: r.project_id,
            slug: r.slug,
            name: r.name,
            kind: r.kind.parse().unwrap_or(AssetKind::Other),
            tags: serde_json::from_value(r.tags).unwrap_or_default(),
            head: match (r.head_version_id, r.head_number) {
                (Some(id), Some(number)) => Some(VersionBrief {
                    id,
                    number,
                    status: r.head_status.and_then(|s| s.parse().ok()).unwrap_or(hangar_core::VersionStatus::Pending),
                }),
                _ => None,
            },
            version_count: r.version_count,
            thumbnail_url: r.thumb.map(|h| hangar_core::blob_url(&h)),
            updated_at: r.updated_at,
        })
        .collect();
    Ok(Json(Page { items, page, size, total: total as u64 }))
}

#[post("/api/projects/{pid}/assets")]
async fn create_asset(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(pid): Path<Uuid>,
    Json(req): Json<CreateAssetRequest>,
) -> Result<Json<AssetDetail>> {
    let (_, role) = access::project(&db, &user, pid, Need::Write).await?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("nome obrigatório"));
    }
    let base = slugify(req.slug.as_deref().unwrap_or(&name));
    let mut slug = base.clone();
    let mut n = 2;
    while assets::Entity::find()
        .filter(assets::Column::ProjectId.eq(pid))
        .filter(assets::Column::Slug.eq(&slug))
        .one(&db)
        .await
        .ctx("check slug")?
        .is_some()
    {
        slug = format!("{base}-{n}");
        n += 1;
    }
    let now = chrono::Utc::now();
    let model = assets::ActiveModel {
        id: Set(Uuid::now_v7()),
        project_id: Set(pid),
        slug: Set(slug),
        name: Set(name),
        kind: Set(req.kind.as_str().into()),
        description: Set(req.description.trim().to_string()),
        tags: Set(serde_json::to_value(clean_tags(req.tags)).ctx("tags")?),
        source: Set(serde_json::to_value(&req.source).ctx("source")?),
        head_version_id: Set(None),
        created_by: Set(Some(user.0.id)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .ctx("insert asset")?;
    Ok(Json(dto::asset_detail(&db, model, Some(role)).await?))
}

#[get("/api/assets/{id}")]
async fn get_asset(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<AssetDetail>> {
    let (a, role) = access::asset(&db, &user, id, Need::Read).await?;
    Ok(Json(dto::asset_detail(&db, a, Some(role)).await?))
}

#[patch("/api/assets/{id}")]
async fn update_asset(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAssetRequest>,
) -> Result<Json<AssetDetail>> {
    let (a, role) = access::asset(&db, &user, id, Need::Write).await?;
    if let Some(head) = req.head_version_id {
        let v = asset_versions::Entity::find_by_id(head).one(&db).await.ctx("load version")?;
        if v.map(|v| v.asset_id) != Some(a.id) {
            return Err(bad_request("a versão não pertence a este asset"));
        }
    }
    let mut m = a.into_active_model();
    if let Some(name) = req.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        m.name = Set(name);
    }
    if let Some(d) = req.description {
        m.description = Set(d.trim().to_string());
    }
    if let Some(k) = req.kind {
        m.kind = Set(k.as_str().into());
    }
    if let Some(t) = req.tags {
        m.tags = Set(serde_json::to_value(clean_tags(t)).ctx("tags")?);
    }
    if let Some(s) = req.source {
        m.source = Set(serde_json::to_value(&s).ctx("source")?);
    }
    if let Some(h) = req.head_version_id {
        m.head_version_id = Set(Some(h));
    }
    m.updated_at = Set(chrono::Utc::now());
    let a = m.update(&db).await.ctx("update asset")?;
    Ok(Json(dto::asset_detail(&db, a, Some(role)).await?))
}

#[delete("/api/assets/{id}")]
async fn delete_asset(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let (a, _) = access::asset(&db, &user, id, Need::Admin).await?;
    // Versions, file trees and renditions cascade; blobs stay until a GC pass.
    assets::Entity::delete_by_id(a.id).exec(&db).await.ctx("delete asset")?;
    Ok(StatusCode::NO_CONTENT)
}

#[get("/api/assets/{id}/versions")]
async fn list_versions(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<VersionDto>>> {
    let (a, _) = access::asset(&db, &user, id, Need::Read).await?;
    let versions = asset_versions::Entity::find()
        .filter(asset_versions::Column::AssetId.eq(a.id))
        .order_by_desc(asset_versions::Column::Number)
        .all(&db)
        .await
        .ctx("list versions")?;
    let names = dto::usernames(&db, versions.iter().filter_map(|v| v.author_id)).await?;
    Ok(Json(
        versions
            .into_iter()
            .map(|v| dto::version_dto(v, a.head_version_id, &names))
            .collect(),
    ))
}
