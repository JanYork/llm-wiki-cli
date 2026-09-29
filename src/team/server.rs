use super::{auth, control};
use crate::error::{AppError, Result};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    pub data: PathBuf,
    pub admin_assets: Option<PathBuf>,
    pub public_url: String,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub smtp: Option<Smtp>,
    pub github: Option<OAuth>,
    pub feishu: Option<OAuth>,
    #[serde(default = "default_artifact_limit")]
    pub max_artifact_bytes: u64,
    #[serde(default = "default_space_limit")]
    pub max_space_bytes: u64,
}
fn default_artifact_limit() -> u64 {
    256 * 1024 * 1024
}
fn default_space_limit() -> u64 {
    10 * 1024 * 1024 * 1024
}
fn default_listen() -> SocketAddr {
    ([127, 0, 0, 1], 8787).into()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Smtp {
    pub host: String,
    pub from: String,
    pub username: String,
    pub password_env: String,
    #[serde(default = "smtp_port")]
    pub port: u16,
}
fn smtp_port() -> u16 {
    587
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OAuth {
    pub client_id: String,
    pub client_secret_env: String,
}
pub(super) struct Server {
    pub config: Config,
    pub client: reqwest::Client,
}
pub(super) type Shared = Arc<Server>;
pub(super) struct HttpError(pub AppError);
impl From<AppError> for HttpError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            "unauthorized" | "login_failed" => StatusCode::UNAUTHORIZED,
            "forbidden" => StatusCode::FORBIDDEN,
            "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
            "revision_conflict"
            | "head_changed"
            | "server_epoch_changed"
            | "batch_conflict"
            | "batch_already_committed" => StatusCode::CONFLICT,
            "artifact_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "space_quota_exceeded" => StatusCode::INSUFFICIENT_STORAGE,
            "provider_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
            "database_error" | "io_error" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            "internal service error".into()
        } else {
            self.0.message
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"error":{"code":self.0.code,"message":message,"details":if matches!(self.0.code,"forbidden"|"revoked_memory_version"){self.0.details}else{None}}})),
        )
            .into_response()
    }
}

pub(super) fn secret(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            AppError::new(
                "provider_unavailable",
                format!("missing configured environment variable {name}"),
            )
        })
}

pub(crate) fn run(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path)?;
    let value = jsonc_parser::cst::CstRootNode::parse(&raw, &Default::default())
        .map_err(|_| AppError::new("invalid_server_config", "invalid JSONC configuration"))?
        .to_serde_value()
        .ok_or_else(|| AppError::new("invalid_server_config", "empty configuration"))?;
    let mut config: Config = serde_json::from_value(value)
        .map_err(|_| AppError::new("invalid_server_config", "invalid team configuration fields"))?;
    if config.max_artifact_bytes == 0
        || config.max_space_bytes < config.max_artifact_bytes
        || config.max_space_bytes > i64::MAX as u64
    {
        return Err(AppError::new(
            "invalid_server_config",
            "invalid space or artifact capacity limits",
        ));
    }
    if config.data.is_relative() {
        config.data = path.parent().unwrap_or(Path::new(".")).join(&config.data);
    }
    let url = reqwest::Url::parse(&config.public_url).map_err(|_| {
        AppError::new(
            "invalid_server_config",
            "public_url must be an HTTPS origin",
        )
    })?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !(url.scheme() == "https"
            || (url.scheme() == "http" && loopback && config.listen.ip().is_loopback()))
    {
        return Err(AppError::new(
            "invalid_server_config",
            "public_url must be an HTTPS origin; HTTP is limited to loopback development",
        ));
    }
    config.public_url = url.origin().ascii_serialization();
    if let Some(assets) = config.admin_assets.as_mut()
        && assets.is_relative()
    {
        *assets = path.parent().unwrap_or(Path::new(".")).join(&*assets);
    }
    super::private_directory(&config.data)?;
    let _lifecycle_lock = super::backup::offline_lock(&config.data)?;
    control::open(&config.data)?;
    super::access::initialize(&config.data)?;
    for provider in [&config.github, &config.feishu].into_iter().flatten() {
        if provider.client_id.trim().is_empty() {
            return Err(AppError::new(
                "invalid_server_config",
                "OAuth client_id is empty",
            ));
        }
        secret(&provider.client_secret_env)?;
    }
    if let Some(smtp) = &config.smtp {
        secret(&smtp.password_env)?;
        smtp.from
            .parse::<lettre::message::Mailbox>()
            .map_err(|_| AppError::new("invalid_server_config", "invalid SMTP from address"))?;
        if smtp.host.trim().is_empty() || smtp.username.trim().is_empty() {
            return Err(AppError::new(
                "invalid_server_config",
                "SMTP host and username are required",
            ));
        }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .user_agent("lwc-team")
            .build()
            .map_err(|_| AppError::new("http_client_error", "failed to initialize HTTPS client"))?;
        let address = config.listen;
        let state = Arc::new(Server { config, client });
        let app = Router::new()
            .merge(super::hub::routes())
            .route(
                "/health",
                get(|| async { Json(json!({"status":"ok","protocol":"lwc-team-sync/1"})) }),
            )
            .route("/api/auth/providers", get(auth::providers))
            .route("/api/auth/device/start", post(auth::device_start))
            .route("/api/auth/device/poll", post(auth::device_poll))
            .route("/api/auth/device/approve", post(auth::device_approve))
            .route("/api/auth/device/preview", post(auth::device_preview))
            .route("/api/invitations/preview", post(auth::invitation_preview))
            .route("/api/auth/email/challenge", post(auth::email_challenge))
            .route("/api/auth/email/verify", post(auth::email_verify))
            .route(
                "/api/auth/{provider}/start",
                get(auth::oauth_start).post(auth::oauth_start),
            )
            .route("/api/auth/{provider}/callback", get(auth::oauth_callback))
            .route("/api/me", get(me))
            .route("/api/admin", get(super::admin::browse))
            .route("/api/identity/register", post(super::identity::register))
            .route("/api/agents/delegate", post(super::delegation::grant))
            .route("/api/logout", post(logout))
            .route("/api/manage", post(manage))
            .layer(DefaultBodyLimit::max(64 * 1024))
            .layer(axum::middleware::from_fn(
                |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    let mut response = next.run(request).await;
                    for (key, value) in [
                        ("cache-control", "no-store"),
                        ("x-content-type-options", "nosniff"),
                        ("referrer-policy", "no-referrer"),
                    ] {
                        response
                            .headers_mut()
                            .insert(key, axum::http::HeaderValue::from_static(value));
                    }
                    response
                },
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                super::access::gate,
            ))
            .route("/", get(super::admin::index))
            .route("/devices/authorize", get(super::admin::index))
            .route("/assets/{*path}", get(super::admin::asset))
            .route("/access", get(super::access::page))
            .route(
                "/api/access",
                post(super::access::activate).layer(DefaultBodyLimit::max(4096)),
            )
            .route(
                "/api/auth/key",
                post(super::keys::login).layer(DefaultBodyLimit::max(4096)),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind(address).await?;
        eprintln!("LWC team service listening on {address}");
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
        Ok(json!({"stopped":true}))
    })
}

pub(super) async fn database<T: Send + 'static>(
    state: &Shared,
    operation: impl FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    let directory = state.config.data.clone();
    tokio::task::spawn_blocking(move || operation(&mut control::open(&directory)?))
        .await
        .map_err(|_| AppError::new("internal_error", "database worker stopped"))?
}

pub(super) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|s| s.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned())
}

pub(super) fn session(headers: &HeaderMap) -> Result<String> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        return Ok(value.to_owned());
    }
    cookie(headers, "lwc_session").ok_or_else(|| AppError::new("unauthorized", "login required"))
}

pub(super) fn browser_origin(state: &Shared, headers: &HeaderMap) -> Result<()> {
    if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok())
        != Some(state.config.public_url.as_str())
    {
        return Err(AppError::new(
            "forbidden",
            "same-origin browser request required",
        ));
    }
    Ok(())
}

pub(super) fn cookie_header(state: &Shared, name: &str, value: &str, max_age: u32) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if state.config.public_url.starts_with("https:") {
            "; Secure"
        } else {
            ""
        }
    )
}

async fn me(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, HttpError> {
    let secret = session(&headers)?;
    Ok(Json(database(&state,move|conn|{
        let user=control::session_user(conn,&secret)?;
        let mut stmt=conn.prepare("SELECT s.id,s.name,g.role,s.revision,s.team_id FROM spaces s JOIN space_grants g ON g.space_id=s.id WHERE g.user_id=?1 AND s.archived=0 AND (s.user_owner=?1 OR EXISTS(SELECT 1 FROM memberships m WHERE m.team_id=s.team_id AND m.user_id=?1)) ORDER BY s.name,s.id")?;
        let spaces=stmt.query_map([&user],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?,"revision":r.get::<_,i64>(3)?,"team_id":r.get::<_,Option<String>>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"user_id":user,"spaces":spaces}))
    }).await?))
}

async fn manage(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> std::result::Result<Json<Value>, HttpError> {
    if !headers.contains_key(header::AUTHORIZATION) {
        browser_origin(&state, &headers)?;
    }
    let secret = session(&headers)?;
    Ok(Json(
        database(&state, move |conn| {
            let actor = control::session_user(conn, &secret)?;
            control::manage(conn, &actor, &input)
        })
        .await?,
    ))
}

async fn logout(
    State(state): State<Shared>,
    headers: HeaderMap,
) -> std::result::Result<Response, HttpError> {
    if !headers.contains_key(header::AUTHORIZATION) {
        browser_origin(&state, &headers)?;
    }
    let secret = session(&headers)?;
    database(&state, move |conn| {
        conn.execute(
            "DELETE FROM sessions WHERE token_hash=?1",
            [control::digest(&secret)],
        )?;
        Ok(())
    })
    .await?;
    Ok((
        [(
            header::SET_COOKIE,
            cookie_header(&state, "lwc_session", "", 0),
        )],
        Json(json!({"logged_out":true})),
    )
        .into_response())
}
