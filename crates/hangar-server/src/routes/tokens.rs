//! Access tokens and device pairing for editor plugins.
//!
//! Pairing works like OAuth's device flow: the plugin starts a pairing
//! (public), opens `/dispositivos?codigo=<user code>` in the browser, and polls
//! with its secret device code until the logged-in user approves; that poll
//! creates the token and returns it once.

use crate::auth::CurrentUser;
use crate::entities::{api_tokens, device_pairings, users};
use crate::error::{Ctx, Result, bad_request, not_found};
use crate::tokens::{self, ApiTokenId, ApiTokens};
use argon2::password_hash::rand_core::{OsRng, RngCore};
use chrono::{Duration, Utc};
use hangar_core::{
    ApiTokenDto, PairDecision, PairInfo, PairPollRequest, PairPollResponse, PairStartRequest, PairStartResponse,
    PairStatus,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set};
use summer_web::axum::http::StatusCode;
use summer_web::extractor::{Component, Extension, Json, Path};
use summer_web::{delete, get, post};
use uuid::Uuid;

const PAIRING_TTL_SECS: u32 = 600;
const POLL_INTERVAL_SECS: u32 = 3;
/// No vowels (no words), no 0/O/1/I.
const CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ23456789";

fn user_code() -> String {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    let c: String = bytes.iter().map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char).collect();
    format!("{}-{}", &c[..4], &c[4..])
}

/// "kxrt4qpm", "KXRT-4QPM" → "KXRT-4QPM".
fn normalize_code(code: &str) -> String {
    let c: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    if c.len() == 8 { format!("{}-{}", &c[..4], &c[4..]) } else { c }
}

fn client_label(client: &str) -> &str {
    match client {
        "unreal" => "Unreal",
        "blender" => "Blender",
        "cli" => "CLI",
        other => other,
    }
}

fn clip(s: &str, max: usize) -> String {
    s.trim().chars().filter(|c| !c.is_control()).take(max).collect()
}

#[get("/api/tokens")]
async fn list_tokens(user: CurrentUser, Component(db): Component<DatabaseConnection>) -> Result<Json<Vec<ApiTokenDto>>> {
    let rows = api_tokens::Entity::find()
        .filter(api_tokens::Column::UserId.eq(user.0.id))
        .order_by_desc(api_tokens::Column::CreatedAt)
        .all(&db)
        .await
        .ctx("list tokens")?;
    Ok(Json(rows.iter().map(|t| t.to_dto()).collect()))
}

#[delete("/api/tokens/{id}")]
async fn delete_token(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(tokens): Component<ApiTokens>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let res = api_tokens::Entity::delete_many()
        .filter(api_tokens::Column::Id.eq(id))
        .filter(api_tokens::Column::UserId.eq(user.0.id))
        .exec(&db)
        .await
        .ctx("delete token")?;
    if res.rows_affected == 0 {
        return Err(not_found("token não encontrado"));
    }
    tokens.forget(id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// A plugin disconnecting revokes the token it is using.
#[delete("/api/tokens/current")]
async fn delete_current_token(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Component(tokens): Component<ApiTokens>,
    token: Option<Extension<ApiTokenId>>,
) -> Result<StatusCode> {
    let Some(Extension(ApiTokenId(id))) = token else {
        return Err(bad_request("esta requisição não usa um token de acesso"));
    };
    api_tokens::Entity::delete_many()
        .filter(api_tokens::Column::Id.eq(id))
        .filter(api_tokens::Column::UserId.eq(user.0.id))
        .exec(&db)
        .await
        .ctx("delete token")?;
    tokens.forget(id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[post("/api/devices/pair")]
async fn start_pairing(
    Component(db): Component<DatabaseConnection>,
    Json(req): Json<PairStartRequest>,
) -> Result<Json<PairStartResponse>> {
    let client = clip(&req.client, 20).to_lowercase();
    let device_name = clip(&req.device_name, 80);
    if client.is_empty() || device_name.is_empty() {
        return Err(bad_request("informe o programa e o nome da máquina"));
    }
    device_pairings::Entity::delete_many()
        .filter(device_pairings::Column::ExpiresAt.lt(Utc::now()))
        .exec(&db)
        .await
        .ctx("prune pairings")?;
    let device_code = tokens::generate();
    let now = Utc::now();
    let pairing = device_pairings::ActiveModel {
        id: Set(Uuid::now_v7()),
        device_code_hash: Set(tokens::hash(&device_code)),
        user_code: Set(user_code()),
        client: Set(client),
        device_name: Set(device_name),
        detail: Set(req.detail.as_deref().map(|d| clip(d, 120)).filter(|d| !d.is_empty())),
        created_at: Set(now),
        expires_at: Set(now + Duration::seconds(PAIRING_TTL_SECS as i64)),
        status: Set("pending".into()),
        approved_by: Set(None),
    }
    .insert(&db)
    .await
    .ctx("insert pairing")?;
    Ok(Json(PairStartResponse {
        device_code,
        verify_path: format!("/dispositivos?codigo={}", pairing.user_code),
        user_code: pairing.user_code,
        expires_in: PAIRING_TTL_SECS,
        interval: POLL_INTERVAL_SECS,
    }))
}

#[post("/api/devices/pair/poll")]
async fn poll_pairing(
    Component(db): Component<DatabaseConnection>,
    Json(req): Json<PairPollRequest>,
) -> Result<Json<PairPollResponse>> {
    let reply = |status| Ok(Json(PairPollResponse { status, token: None, user: None }));
    let pairing = device_pairings::Entity::find()
        .filter(device_pairings::Column::DeviceCodeHash.eq(tokens::hash(req.device_code.trim())))
        .one(&db)
        .await
        .ctx("load pairing")?;
    let Some(p) = pairing else { return reply(PairStatus::Expired) };
    let finished = p.expires_at < Utc::now() || p.status != "pending";
    // Only the poll that removes the row issues the token (concurrent polls get Expired).
    let claimed = finished
        && device_pairings::Entity::delete_by_id(p.id).exec(&db).await.ctx("delete pairing")?.rows_affected == 1;
    match (p.status.as_str(), p.approved_by) {
        ("approved", Some(user_id)) if claimed && p.expires_at >= Utc::now() => {
            let user = users::Entity::find_by_id(user_id).one(&db).await.ctx("load user")?;
            let Some(user) = user else { return reply(PairStatus::Expired) };
            let token = tokens::generate();
            let mut name = format!("{} · {}", client_label(&p.client), p.device_name);
            if let Some(d) = &p.detail {
                name = format!("{name} · {d}");
            }
            api_tokens::ActiveModel {
                id: Set(Uuid::now_v7()),
                user_id: Set(user.id),
                name: Set(name),
                client: Set(p.client.clone()),
                token_hash: Set(tokens::hash(&token)),
                created_at: Set(Utc::now()),
                last_used_at: Set(None),
            }
            .insert(&db)
            .await
            .ctx("insert token")?;
            tracing::info!(user = %user.username, device = %p.device_name, client = %p.client, "device paired");
            Ok(Json(PairPollResponse { status: PairStatus::Approved, token: Some(token), user: Some(user.to_dto()) }))
        }
        ("denied", _) => reply(PairStatus::Denied),
        _ if finished => reply(PairStatus::Expired),
        _ => reply(PairStatus::Pending),
    }
}

async fn pending_pairing(db: &DatabaseConnection, code: &str) -> Result<device_pairings::Model> {
    device_pairings::Entity::find()
        .filter(device_pairings::Column::UserCode.eq(normalize_code(code)))
        .filter(device_pairings::Column::Status.eq("pending"))
        .filter(device_pairings::Column::ExpiresAt.gte(Utc::now()))
        .one(db)
        .await
        .ctx("load pairing")?
        .ok_or_else(|| not_found("código não encontrado ou expirado: inicie a conexão de novo no programa"))
}

#[get("/api/devices/pair/{code}")]
async fn pairing_info(
    _user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(code): Path<String>,
) -> Result<Json<PairInfo>> {
    Ok(Json(pending_pairing(&db, &code).await?.to_info()))
}

#[post("/api/devices/pair/{code}")]
async fn decide_pairing(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(code): Path<String>,
    Json(req): Json<PairDecision>,
) -> Result<StatusCode> {
    let p = pending_pairing(&db, &code).await?;
    let mut active: device_pairings::ActiveModel = p.into();
    active.status = Set(if req.approve { "approved" } else { "denied" }.into());
    active.approved_by = Set(req.approve.then_some(user.0.id));
    active.update(&db).await.ctx("update pairing")?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        let c = user_code();
        assert_eq!(c.len(), 9);
        assert_eq!(normalize_code(&c.to_lowercase().replace('-', " ")), c);
        assert!(tokens::generate().starts_with("hgr_"));
        assert_eq!(tokens::generate().len(), 4 + 64);
    }
}
