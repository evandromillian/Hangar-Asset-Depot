use crate::access::{self, Need};
use crate::auth::CurrentUser;
use crate::dto;
use crate::entities::{asset_versions, assets, version_files};
use crate::error::{Ctx, Result, bad_request, not_found};
use crate::jobs::{JobQueue, KIND_PROCESS_VERSION, ProcessVersionPayload};
use crate::storage::BlobStore;
use hangar_core::{CommitVersionRequest, DiffQuery, FileEntry, VersionDiff, VersionDto, VersionStatus, paths};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use summer_web::axum::body::Body;
use summer_web::axum::http::{HeaderMap, HeaderValue, header};
use summer_web::axum::response::{IntoResponse, Response};
use summer_web::extractor::{Component, Json, Path, Query};
use summer_web::{get, post};
use uuid::Uuid;

async fn load_tree<C: ConnectionTrait>(db: &C, version_id: Uuid) -> Result<Vec<FileEntry>> {
    Ok(version_files::Entity::find()
        .filter(version_files::Column::VersionId.eq(version_id))
        .order_by_asc(version_files::Column::Path)
        .all(db)
        .await
        .ctx("load tree")?
        .iter()
        .map(|f| f.to_entry())
        .collect())
}

#[post("/api/assets/{id}/versions")]
async fn commit_version(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(store): Component<BlobStore>,
    Component(queue): Component<JobQueue>,
    Path(asset_id): Path<Uuid>,
    Json(req): Json<CommitVersionRequest>,
) -> Result<Json<VersionDto>> {
    access::asset(&db, &user, asset_id, Need::Write).await?;

    // Validate the request before touching the database.
    let mut upserts: BTreeMap<String, FileEntry> = BTreeMap::new();
    for f in req.upserts {
        let path = paths::normalize(&f.path).ok_or_else(|| bad_request(format!("caminho inválido: {}", f.path)))?;
        if !hangar_core::is_valid_hash(&f.hash) {
            return Err(bad_request(format!("hash inválido para {path}")));
        }
        upserts.insert(path.clone(), FileEntry { path, hash: f.hash, size: f.size });
    }
    let deletes: Vec<String> = req.deletes.iter().filter_map(|d| paths::normalize(d)).collect();
    let message = req.message.trim().to_string();
    if message.chars().count() > 2000 {
        return Err(bad_request("mensagem muito longa"));
    }

    // Every referenced blob must exist (uploaded through PUT /api/blobs/{hash}).
    let hashes: Vec<String> = upserts.values().map(|f| f.hash.clone()).collect();
    let missing = store.missing(&hashes).await?;
    if !missing.is_empty() {
        return Err(bad_request(format!("blobs ainda não enviados: {}", missing.join(", "))));
    }

    let txn = db.begin().await.ctx("begin")?;
    // Lock the asset row: serialises version numbering per asset.
    let asset = assets::Entity::find_by_id(asset_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .ctx("lock asset")?
        .ok_or_else(|| not_found("asset não encontrado"))?;

    let mut tree: BTreeMap<String, FileEntry> = BTreeMap::new();
    let mut parent_tree: Option<Vec<FileEntry>> = None;
    if let Some(parent_id) = req.parent_id {
        let parent = asset_versions::Entity::find_by_id(parent_id)
            .one(&txn)
            .await
            .ctx("load parent")?
            .filter(|p| p.asset_id == asset.id)
            .ok_or_else(|| bad_request("versão pai não pertence a este asset"))?;
        if req.inherit {
            let pt = load_tree(&txn, parent.id).await?;
            tree.extend(pt.iter().map(|f| (f.path.clone(), f.clone())));
            parent_tree = Some(pt);
        }
    }
    for d in &deletes {
        // "Textures/Old/" removes a whole folder; otherwise an exact path.
        if d.ends_with('/') || !tree.contains_key(d) {
            let prefix = format!("{}/", d.trim_end_matches('/'));
            tree.retain(|p, _| !p.starts_with(&prefix));
        }
        tree.remove(d);
    }
    tree.extend(upserts);

    if tree.is_empty() {
        return Err(bad_request("a versão não tem nenhum arquivo"));
    }
    if let Some(pt) = &parent_tree {
        let new: Vec<FileEntry> = tree.values().cloned().collect();
        if VersionDiff::compute(None, pt, Uuid::nil(), &new).is_empty() {
            return Err(bad_request("nenhuma alteração em relação à versão pai"));
        }
    }

    let next: i32 = asset_versions::Entity::find()
        .filter(asset_versions::Column::AssetId.eq(asset.id))
        .select_only()
        .column_as(asset_versions::Column::Number.max(), "max")
        .into_tuple::<Option<i32>>()
        .one(&txn)
        .await
        .ctx("next number")?
        .flatten()
        .unwrap_or(0)
        + 1;

    let version_id = Uuid::now_v7();
    let now = chrono::Utc::now();
    let total_size: i64 = tree.values().map(|f| f.size).sum();
    let version = asset_versions::ActiveModel {
        id: Set(version_id),
        asset_id: Set(asset.id),
        number: Set(next),
        parent_id: Set(req.parent_id),
        message: Set(if message.is_empty() { format!("v{next}") } else { message }),
        author_id: Set(Some(user.0.id)),
        status: Set(VersionStatus::Pending.as_str().into()),
        error: Set(None),
        preview: Set(None),
        file_count: Set(tree.len() as i32),
        total_size: Set(total_size),
        created_at: Set(now),
        processed_at: Set(None),
    }
    .insert(&txn)
    .await
    .ctx("insert version")?;

    let rows: Vec<version_files::ActiveModel> = tree
        .values()
        .map(|f| version_files::ActiveModel {
            version_id: Set(version_id),
            path: Set(f.path.clone()),
            blob_hash: Set(f.hash.clone()),
            size: Set(f.size),
        })
        .collect();
    for chunk in rows.chunks(500) {
        version_files::Entity::insert_many(chunk.to_vec())
            .exec(&txn)
            .await
            .ctx("insert files")?;
    }

    let head = if req.set_head || asset.head_version_id.is_none() { Some(version_id) } else { asset.head_version_id };
    txn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE assets SET head_version_id = $2, updated_at = now() WHERE id = $1",
        [asset.id.into(), head.into()],
    ))
    .await
    .ctx("update asset")?;

    queue
        .enqueue(&txn, KIND_PROCESS_VERSION, ProcessVersionPayload { version_id })
        .await
        .ctx_any("enqueue")?;
    txn.commit().await.ctx("commit")?;
    queue.wake();

    tracing::info!(asset = %asset.id, number = next, files = tree.len(), "version committed");
    Ok(Json(dto::version_dto_one(&db, version, head).await?))
}

/// anyhow errors don't implement std::error::Error; give them the same `ctx` sugar.
trait CtxAny<T> {
    fn ctx_any(self, what: &'static str) -> Result<T>;
}
impl<T> CtxAny<T> for anyhow::Result<T> {
    fn ctx_any(self, what: &'static str) -> Result<T> {
        self.map_err(|e| e.context(what).into())
    }
}

#[get("/api/versions/{id}")]
async fn get_version(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<VersionDto>> {
    let (v, a, _) = access::version(&db, &user, id, Need::Read).await?;
    Ok(Json(dto::version_dto_one(&db, v, a.head_version_id).await?))
}

#[get("/api/versions/{id}/files")]
async fn list_files(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<FileEntry>>> {
    access::version(&db, &user, id, Need::Read).await?;
    Ok(Json(load_tree(&db, id).await?))
}

#[get("/api/versions/{id}/diff")]
async fn diff_version(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<VersionDiff>> {
    let (v, _, _) = access::version(&db, &user, id, Need::Read).await?;
    let base_id = q.against.or(v.parent_id);
    let base = match base_id {
        Some(b) => {
            let (bv, _, _) = access::version(&db, &user, b, Need::Read).await?;
            if bv.asset_id != v.asset_id {
                return Err(bad_request("só é possível comparar versões do mesmo asset"));
            }
            load_tree(&db, b).await?
        }
        None => vec![],
    };
    let target = load_tree(&db, v.id).await?;
    Ok(Json(VersionDiff::compute(base_id, &base, v.id, &target)))
}

#[post("/api/versions/{id}/reprocess")]
async fn reprocess_version(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(queue): Component<JobQueue>,
    Path(id): Path<Uuid>,
) -> Result<Json<VersionDto>> {
    let (v, a, _) = access::version(&db, &user, id, Need::Write).await?;
    let txn = db.begin().await.ctx("begin")?;
    txn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE asset_versions SET status = 'pending', error = NULL WHERE id = $1",
        [v.id.into()],
    ))
    .await
    .ctx("reset status")?;
    queue
        .enqueue(&txn, KIND_PROCESS_VERSION, ProcessVersionPayload { version_id: v.id })
        .await
        .ctx_any("enqueue")?;
    txn.commit().await.ctx("commit")?;
    queue.wake();
    let v = asset_versions::Entity::find_by_id(v.id)
        .one(&db)
        .await
        .ctx("reload")?
        .ok_or_else(|| not_found("versão"))?;
    Ok(Json(dto::version_dto_one(&db, v, a.head_version_id).await?))
}

#[derive(Deserialize)]
struct RawQuery {
    download: Option<u8>,
}

fn content_disposition(kind: &str, file_name: &str) -> HeaderValue {
    // RFC 6266 / 5987: ASCII fallback + UTF-8 encoded name.
    let ascii: String = file_name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' { c } else { '_' })
        .collect();
    let encoded: String = file_name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }
        })
        .collect();
    HeaderValue::from_str(&format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

#[get("/api/versions/{id}/raw/{*path}")]
async fn raw_file(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(store): Component<BlobStore>,
    Path((id, path)): Path<(Uuid, String)>,
    Query(q): Query<RawQuery>,
) -> Result<Response> {
    access::version(&db, &user, id, Need::Read).await?;
    let path = paths::normalize(&path).ok_or_else(|| not_found("arquivo"))?;
    let file = version_files::Entity::find_by_id((id, path.clone()))
        .one(&db)
        .await
        .ctx("load file")?
        .ok_or_else(|| not_found("arquivo não encontrado nesta versão"))?;
    let stream = store.stream(&file.blob_hash).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(paths::mime_for(&path)));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(file.size as u64));
    let disposition = if q.download == Some(1) { "attachment" } else { "inline" };
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(disposition, paths::file_name(&path)));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=60"));
    Ok((headers, Body::from_stream(stream)).into_response())
}

#[get("/api/versions/{id}/archive")]
async fn archive(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(store): Component<BlobStore>,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let (v, a, _) = access::version(&db, &user, id, Need::Read).await?;
    let tree = load_tree(&db, v.id).await?;
    let file = crate::archive::build_zip(&store, &tree).await?;
    let len = file.metadata().await.ctx("zip size")?.len();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(
        header::CONTENT_DISPOSITION,
        content_disposition("attachment", &format!("{}-v{}.zip", a.slug, v.number)),
    );
    let stream = tokio_util::io::ReaderStream::with_capacity(file, 1 << 20);
    Ok((headers, Body::from_stream(stream)).into_response())
}
