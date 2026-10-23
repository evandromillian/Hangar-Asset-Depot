use crate::auth::CurrentUser;
use crate::entities::project_members;
use crate::error::{Result, forbidden, not_found};
use crate::storage::BlobStore;
use hangar_core::{BlobUploaded, MissingBlobsRequest, MissingBlobsResponse, paths};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use summer_web::axum::body::Body;
use summer_web::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use summer_web::axum::response::{IntoResponse, Response};
use summer_web::extractor::{Component, Json, Path};
use summer_web::{get, post, put};

/// Uploading requires being at least editor somewhere (or admin).
async fn require_uploader(db: &DatabaseConnection, user: &CurrentUser) -> Result<()> {
    if user.0.is_admin {
        return Ok(());
    }
    let n = project_members::Entity::find()
        .filter(project_members::Column::UserId.eq(user.0.id))
        .filter(project_members::Column::Role.is_in(["owner", "editor"]))
        .count(db)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    if n == 0 { Err(forbidden("sem permissão de envio")) } else { Ok(()) }
}

#[post("/api/blobs/missing")]
async fn missing_blobs(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(store): Component<BlobStore>,
    Json(req): Json<MissingBlobsRequest>,
) -> Result<Json<MissingBlobsResponse>> {
    require_uploader(&db, &user).await?;
    Ok(Json(MissingBlobsResponse { missing: store.missing(&req.hashes).await? }))
}

/// Raw body upload. The server re-hashes the stream and rejects mismatches.
#[put("/api/blobs/{hash}")]
async fn put_blob(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(store): Component<BlobStore>,
    Path(hash): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<BlobUploaded>)> {
    require_uploader(&db, &user).await?;
    if let Some(len) = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse().ok()) {
        store.check_upload_size(len)?;
    }
    // Blobs are served with this type, from our origin: only harmless ones are kept.
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|m| paths::is_served_mime(m))
        .map(str::to_string);
    let res = store.put_stream(&hash, mime, body.into_data_stream()).await?;
    let status = if res.created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Json(res)))
}

#[get("/api/blobs/{hash}")]
async fn get_blob(
    _user: CurrentUser,
    Component(store): Component<BlobStore>,
    Path(hash): Path<String>,
    req_headers: HeaderMap,
) -> Result<Response> {
    let blob = store.get(&hash).await?.ok_or_else(|| not_found("blob não encontrado"))?;
    let etag = format!("\"{}\"", blob.hash);
    let cache = HeaderValue::from_static("private, max-age=31536000, immutable");
    if req_headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        let mut h = HeaderMap::new();
        h.insert(header::CACHE_CONTROL, cache);
        return Ok((StatusCode::NOT_MODIFIED, h).into_response());
    }
    let stream = store.stream(&blob.hash).await?;
    let mut headers = HeaderMap::new();
    // Rows stored before uploads were filtered may hold any client-sent type.
    match blob.mime.as_deref().filter(|m| paths::is_served_mime(m)).and_then(|m| HeaderValue::from_str(m).ok()) {
        Some(mime) => headers.insert(header::CONTENT_TYPE, mime),
        None => {
            headers.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"))
        }
    };
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(blob.size as u64));
    headers.insert(header::CACHE_CONTROL, cache);
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).expect("hex etag"));
    Ok((headers, Body::from_stream(stream)).into_response())
}
