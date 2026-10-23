//! Authentication (sa-token sessions) and user management.

use crate::config::HangarConfig;
use crate::entities::users;
use crate::error::{Ctx, KnownWebError, Result, bad_request, conflict, forbidden, unauthorized};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng};
use argon2::Argon2;
use hangar_core::{CreateUserRequest, LoginRequest, LoginResponse, RegisterRequest, SetupStatus, UserDto};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, Set};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use summer_sa_token::sa_token_plugin_axum::{StpUtil, TokenValue};
use summer_web::axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use summer_web::axum::response::IntoResponse;
use summer_web::extractor::{Component, Config, Extension, FromRequestParts, Json, RequestPartsExt};
use summer_web::{get, post};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const COOKIE_NAME: &str = "hangar_token";

/// Failed logins of a user name allowed per window before it is throttled.
/// Keyed by name, not address: behind a reverse proxy every peer looks the same.
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const MAX_PASSWORD_BYTES: usize = 256;

/// user name → (window start, failures).
static FAILURES: LazyLock<Mutex<HashMap<String, (Instant, u32)>>> = LazyLock::new(Mutex::default);

/// Argon2 is memory- and CPU-hard on purpose; bounding how many run at once
/// keeps a flood of logins from exhausting the server.
static HASHING: Semaphore = Semaphore::const_new(4);

fn throttled(username: &str) -> bool {
    let map = FAILURES.lock().unwrap();
    map.get(username).is_some_and(|(start, n)| start.elapsed() < FAILURE_WINDOW && *n >= MAX_FAILURES)
}

fn record_failure(username: &str) {
    let mut map = FAILURES.lock().unwrap();
    if map.len() > 10_000 {
        map.retain(|_, (start, _)| start.elapsed() < FAILURE_WINDOW);
    }
    let entry = map.entry(username.to_string()).or_insert((Instant::now(), 0));
    if entry.0.elapsed() >= FAILURE_WINDOW {
        *entry = (Instant::now(), 0);
    }
    entry.1 += 1;
    if entry.1 == MAX_FAILURES {
        tracing::warn!(username, "login throttled after repeated failures");
    }
}

/// The logged-in user, loaded from the database. Rejects with 401.
#[derive(Clone, Debug)]
pub struct CurrentUser(pub users::Model);

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = summer_web::error::WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self> {
        // SaTokenLayer stores the login id (our user UUID) as a String extension.
        let login_id = parts
            .extensions
            .get::<String>()
            .cloned()
            .ok_or_else(|| unauthorized("login necessário"))?;
        let id = Uuid::parse_str(&login_id).map_err(|_| unauthorized("sessão inválida"))?;
        let db: DatabaseConnection = parts.get_component()?;
        let user = users::Entity::find_by_id(id)
            .one(&db)
            .await
            .ctx("load user")?
            .ok_or_else(|| unauthorized("usuário removido"))?;
        Ok(CurrentUser(user))
    }
}

impl CurrentUser {
    pub fn require_admin(&self) -> Result<()> {
        if self.0.is_admin { Ok(()) } else { Err(forbidden("somente administradores")) }
    }
}

pub async fn hash_password(password: String) -> Result<String> {
    let _permit = HASHING.acquire().await.ctx("hash permit")?;
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hash password: {e}"))
    })
    .await
    .ctx("join")?
    .map_err(Into::into)
}

pub async fn verify_password(password: String, hash: String) -> bool {
    let Ok(_permit) = HASHING.acquire().await else { return false };
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn validate_credentials(username: &str, password: &str) -> Result<()> {
    let ok_name = (3..=40).contains(&username.len())
        && username.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok_name {
        return Err(bad_request("usuário deve ter 3–40 caracteres (letras, números, _ - .)"));
    }
    if password.chars().count() < 8 {
        return Err(bad_request("a senha precisa ter pelo menos 8 caracteres"));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(bad_request(format!("a senha pode ter no máximo {MAX_PASSWORD_BYTES} caracteres")));
    }
    Ok(())
}

pub async fn create_user(
    db: &DatabaseConnection,
    username: &str,
    password: String,
    display_name: Option<String>,
    is_admin: bool,
) -> Result<users::Model> {
    let username = username.trim().to_lowercase();
    validate_credentials(&username, &password)?;
    let exists = users::Entity::find()
        .filter(users::Column::Username.eq(&username))
        .count(db)
        .await
        .ctx("check user")?;
    if exists > 0 {
        return Err(conflict("nome de usuário já existe"));
    }
    let model = users::ActiveModel {
        id: Set(Uuid::now_v7()),
        username: Set(username.clone()),
        display_name: Set(display_name
            .map(|d| d.trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>())
            .filter(|d| !d.is_empty())
            .unwrap_or(username)),
        password_hash: Set(hash_password(password).await?),
        is_admin: Set(is_admin),
        created_at: Set(chrono::Utc::now()),
    };
    model.insert(db).await.ctx("insert user")
}

/// The session cookie (`max_age` 0 clears it). SameSite=Lax keeps it off
/// cross-site subrequests; `security::layer` also refuses cross-site writes.
fn session_cookie(token: &str, max_age: i64, cfg: &HangarConfig) -> HeaderValue {
    let secure = if cfg.cookie_secure { "; Secure" } else { "" };
    let v = format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}");
    HeaderValue::from_str(&v).expect("valid cookie header")
}

async fn start_session(user: &users::Model, cfg: &HangarConfig) -> Result<(HeaderMap, Json<LoginResponse>)> {
    let token = StpUtil::login(user.id.to_string())
        .await
        .map_err(|e| anyhow::anyhow!("sa-token login: {e}"))?;
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, session_cookie(token.as_str(), cfg.session_ttl, cfg));
    Ok((headers, Json(LoginResponse { token: token.as_str().to_string(), user: user.to_dto() })))
}

#[get("/api/health")]
async fn health() -> &'static str {
    "ok"
}

#[get("/api/setup")]
async fn setup_status(
    Component(db): Component<DatabaseConnection>,
    Config(cfg): Config<HangarConfig>,
) -> Result<Json<SetupStatus>> {
    let users = users::Entity::find().count(&db).await.ctx("count users")?;
    Ok(Json(SetupStatus {
        needs_setup: users == 0,
        allow_signup: cfg.allow_signup,
        blender_available: hangar_pipeline::blender_available(&cfg.pipeline()),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

#[post("/api/auth/register")]
async fn register(
    Component(db): Component<DatabaseConnection>,
    Config(cfg): Config<HangarConfig>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse> {
    let first = users::Entity::find().count(&db).await.ctx("count users")? == 0;
    if !first && !cfg.allow_signup {
        return Err(forbidden("cadastro fechado: peça a um administrador para criar sua conta"));
    }
    let user = create_user(&db, &req.username, req.password, req.display_name, first).await?;
    tracing::info!(username = %user.username, admin = user.is_admin, "user registered");
    start_session(&user, &cfg).await
}

#[post("/api/auth/login")]
async fn login(
    Component(db): Component<DatabaseConnection>,
    Config(cfg): Config<HangarConfig>,
    Json(req): Json<LoginRequest>,
) -> Result<impl IntoResponse> {
    let username = req.username.trim().to_lowercase();
    if throttled(&username) {
        return Err(KnownWebError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "muitas tentativas de login: aguarde alguns minutos e tente de novo",
        )
        .into());
    }
    if req.password.len() > MAX_PASSWORD_BYTES {
        record_failure(&username);
        return Err(unauthorized("usuário ou senha inválidos"));
    }
    let user = users::Entity::find()
        .filter(users::Column::Username.eq(&username))
        .one(&db)
        .await
        .ctx("load user")?;
    // Verify against a dummy hash when the user doesn't exist to keep timing similar.
    let (hash, user) = match user {
        Some(u) => (u.password_hash.clone(), Some(u)),
        None => ("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$7gWl9pKn6c3d1PZ1xBpq4w".to_string(), None),
    };
    let ok = verify_password(req.password, hash).await;
    match (ok, user) {
        (true, Some(user)) => {
            FAILURES.lock().unwrap().remove(&username);
            start_session(&user, &cfg).await
        }
        _ => {
            record_failure(&username);
            Err(unauthorized("usuário ou senha inválidos"))
        }
    }
}

#[post("/api/auth/logout")]
async fn logout(Config(cfg): Config<HangarConfig>, token: Option<Extension<TokenValue>>) -> Result<impl IntoResponse> {
    if let Some(Extension(token)) = token {
        let _ = StpUtil::logout(&token).await;
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, session_cookie("", 0, &cfg));
    Ok((StatusCode::NO_CONTENT, headers))
}

#[get("/api/auth/me")]
async fn me(user: CurrentUser) -> Json<UserDto> {
    Json(user.0.to_dto())
}

#[get("/api/users")]
async fn list_users(user: CurrentUser, Component(db): Component<DatabaseConnection>) -> Result<Json<Vec<UserDto>>> {
    user.require_admin()?;
    let users = users::Entity::find()
        .order_by_asc(users::Column::Username)
        .all(&db)
        .await
        .ctx("list users")?;
    Ok(Json(users.iter().map(|u| u.to_dto()).collect()))
}

#[post("/api/users")]
async fn admin_create_user(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserDto>> {
    user.require_admin()?;
    let created = create_user(&db, &req.username, req.password, req.display_name, req.is_admin).await?;
    Ok(Json(created.to_dto()))
}
