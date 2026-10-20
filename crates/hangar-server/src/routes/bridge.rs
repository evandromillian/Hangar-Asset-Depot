//! "Enviar para Unreal": editors hold `GET /api/bridge/events` open (server-sent
//! events); the web sends a version to one of them and follows the delivery.

use crate::access::{self, Need};
use crate::auth::CurrentUser;
use crate::bridge::BridgeHub;
use crate::entities::projects;
use crate::error::{Ctx, Result, bad_request, not_found};
use chrono::Utc;
use futures::StreamExt;
use futures::stream::{self, Stream};
use hangar_core::{BridgeClientDto, BridgeHello, BridgeImport, BridgeSendRequest, DeliveryDto, DeliveryUpdate};
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use summer_web::axum::http::{HeaderValue, header};
use summer_web::axum::response::sse::{Event, KeepAlive, Sse};
use summer_web::axum::response::{IntoResponse, Response};
use summer_web::extractor::{Component, Json, Path, Query};
use summer_web::{get, post};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Deserialize)]
struct EventsQuery {
    app: String,
    name: String,
    detail: Option<String>,
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

/// The events stream of one editor: `hello` first, then `import` events. A
/// comment every 15 s keeps proxies and load balancers from closing it.
#[get("/api/bridge/events")]
async fn events(user: CurrentUser, Component(hub): Component<BridgeHub>, Query(q): Query<EventsQuery>) -> Result<Response> {
    let (app, name) = (clip(&q.app, 20).to_lowercase(), clip(&q.name, 80));
    if app.is_empty() || name.is_empty() {
        return Err(bad_request("informe app e name"));
    }
    let client = BridgeClientDto {
        id: Uuid::now_v7(),
        app,
        name,
        detail: q.detail.as_deref().map(|d| clip(d, 120)).filter(|d| !d.is_empty()),
        connected_at: Utc::now(),
    };
    let hello = Event::default()
        .event("hello")
        .json_data(BridgeHello { client_id: client.id, user: user.0.to_dto() })
        .ctx("hello event")?;
    let (tx, rx) = mpsc::channel(16);
    let registration = hub.connect(user.0.id, client, tx);
    let stream = stream::once(async move { Ok(hello) }).chain(stream::unfold((rx, registration), |(mut rx, reg)| async move {
        rx.recv().await.map(|e| (Ok::<_, Infallible>(e), (rx, reg)))
    }));
    let mut res = sse(stream).into_response();
    // nginx and similar proxies buffer responses unless told otherwise.
    res.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(res)
}

fn sse(stream: impl Stream<Item = std::result::Result<Event, Infallible>> + Send + 'static) -> impl IntoResponse {
    Sse::new(stream.boxed()).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("ping"))
}


#[get("/api/bridge/clients")]
async fn clients(user: CurrentUser, Component(hub): Component<BridgeHub>) -> Json<Vec<BridgeClientDto>> {
    Json(hub.clients(user.0.id))
}

#[post("/api/bridge/send")]
async fn send(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(hub): Component<BridgeHub>,
    Json(req): Json<BridgeSendRequest>,
) -> Result<Json<DeliveryDto>> {
    let (v, a, _) = access::version(&db, &user, req.version_id, Need::Read).await?;
    if hub.client(user.0.id, req.client_id).is_none() {
        return Err(not_found("o editor não está mais conectado"));
    }
    let project = projects::Entity::find_by_id(a.project_id)
        .one(&db)
        .await
        .ctx("load project")?
        .ok_or_else(|| not_found("projeto não encontrado"))?;
    let import = BridgeImport {
        delivery_id: Uuid::now_v7(),
        project_id: project.id,
        project_slug: project.slug,
        project_name: project.name,
        asset_id: a.id,
        asset_slug: a.slug.clone(),
        asset_name: a.name.clone(),
        kind: a.kind_enum(),
        version_id: v.id,
        version_number: v.number,
        sent_by: user.0.display_name.clone(),
    };
    let delivery = hub
        .push(user.0.id, req.client_id, &import)
        .ok_or_else(|| not_found("o editor não está mais conectado"))?;
    tracing::info!(asset = %a.slug, version = v.number, client = %delivery.client_name, "sent to editor");
    Ok(Json(delivery))
}

#[get("/api/bridge/deliveries/{id}")]
async fn get_delivery(user: CurrentUser, Component(hub): Component<BridgeHub>, Path(id): Path<Uuid>) -> Result<Json<DeliveryDto>> {
    hub.delivery(user.0.id, id).map(Json).ok_or_else(|| not_found("envio não encontrado"))
}

/// The editor reports progress on a delivery.
#[post("/api/bridge/deliveries/{id}")]
async fn update_delivery(
    user: CurrentUser,
    Component(hub): Component<BridgeHub>,
    Path(id): Path<Uuid>,
    Json(update): Json<DeliveryUpdate>,
) -> Result<Json<DeliveryDto>> {
    hub.update(user.0.id, id, update).map(Json).ok_or_else(|| not_found("envio não encontrado"))
}
