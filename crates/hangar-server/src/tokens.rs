//! Long-lived access tokens (`hgr_…`) for editor plugins.
//!
//! Sessions stay sa-token's job: a layer outside `SaTokenLayer` looks the
//! token up in `api_tokens` and swaps the `Authorization` header for a session
//! token minted for its user, cached in memory. The cache is checked against
//! the database again every minute, so a revoked token stops working on every
//! server instance, and a restart (which drops in-memory sessions) only costs
//! one lookup.

use crate::entities::api_tokens;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use hangar_core::API_TOKEN_PREFIX;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, Set};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use summer_sa_token::sa_token_plugin_axum::{StpUtil, TokenValue};
use summer_web::axum::extract::{Request, State};
use summer_web::axum::http::{HeaderValue, StatusCode, header};
use summer_web::axum::middleware::Next;
use summer_web::axum::response::{IntoResponse, Response};
use uuid::Uuid;

const RECHECK: Duration = Duration::from_secs(60);

/// A new random token: `hgr_` + 64 hex digits.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{API_TOKEN_PREFIX}{}", hex(&bytes))
}

/// What the database stores for a token (or a pairing's device code).
pub fn hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Request extension: the access token the request came with.
#[derive(Clone, Copy, Debug)]
pub struct ApiTokenId(pub Uuid);

#[derive(Clone)]
struct Cached {
    token_id: Uuid,
    session: TokenValue,
    checked: Instant,
}

/// Token → session cache. Registered as a component; the layer and the token
/// routes share it.
#[derive(Clone)]
pub struct ApiTokens {
    db: DatabaseConnection,
    sessions: Arc<Mutex<HashMap<String, Cached>>>,
}

impl ApiTokens {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db, sessions: Arc::default() }
    }

    /// The session (and token id) for a raw `hgr_…` token, or None when it doesn't exist (any more).
    async fn session_for(&self, raw: &str) -> anyhow::Result<Option<(TokenValue, Uuid)>> {
        let key = hash(raw);
        let cached = self.sessions.lock().unwrap().get(&key).cloned();
        if let Some(c) = &cached
            && c.checked.elapsed() < RECHECK
            && StpUtil::is_login(&c.session).await
        {
            return Ok(Some((c.session.clone(), c.token_id)));
        }
        let row = api_tokens::Entity::find()
            .filter(api_tokens::Column::TokenHash.eq(&key))
            .one(&self.db)
            .await?;
        let Some(row) = row else {
            let gone = self.sessions.lock().unwrap().remove(&key);
            if let Some(c) = gone {
                let _ = StpUtil::logout(&c.session).await;
            }
            return Ok(None);
        };
        let session = match cached {
            Some(c) if c.token_id == row.id && StpUtil::is_login(&c.session).await => c.session,
            _ => StpUtil::login(row.user_id.to_string()).await.map_err(|e| anyhow::anyhow!("sa-token login: {e}"))?,
        };
        let mut active = row.into_active_model();
        active.last_used_at = Set(Some(chrono::Utc::now()));
        let token_id = active.id.clone().unwrap();
        active.update(&self.db).await?;
        self.sessions
            .lock()
            .unwrap()
            .insert(key, Cached { token_id, session: session.clone(), checked: Instant::now() });
        Ok(Some((session, token_id)))
    }

    /// Ends the cached session of a deleted token (other instances notice within a minute).
    pub async fn forget(&self, token_id: Uuid) {
        let gone: Vec<Cached> = {
            let mut map = self.sessions.lock().unwrap();
            let keys: Vec<String> = map.iter().filter(|(_, c)| c.token_id == token_id).map(|(k, _)| k.clone()).collect();
            keys.iter().filter_map(|k| map.remove(k)).collect()
        };
        for c in gone {
            let _ = StpUtil::logout(&c.session).await;
        }
    }
}

/// Layer (outside sa-token's): `Authorization: Bearer hgr_…` → the token's session.
pub async fn layer(State(tokens): State<ApiTokens>, mut req: Request, next: Next) -> Response {
    let raw = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim)
        .filter(|t| t.starts_with(API_TOKEN_PREFIX))
        .map(str::to_string);
    if let Some(raw) = raw {
        match tokens.session_for(&raw).await {
            Ok(Some((session, token_id))) => {
                let value = HeaderValue::from_str(&format!("Bearer {}", session.as_str())).expect("session token is ASCII");
                req.headers_mut().insert(header::AUTHORIZATION, value);
                req.extensions_mut().insert(ApiTokenId(token_id));
            }
            Ok(None) => return (StatusCode::UNAUTHORIZED, "token de acesso inválido ou revogado").into_response(),
            Err(e) => {
                tracing::error!("api token lookup: {e:#}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "falha ao verificar o token").into_response();
            }
        }
    }
    next.run(req).await
}
