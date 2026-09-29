//! Instance admission is independent of user sessions and space authorization.
use super::server::{HttpError, Shared};
use crate::error::{AppError, Result};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

pub(super) fn initialize(directory: &Path) -> Result<std::path::PathBuf> {
    let path = directory.join("server-access.token");
    if !path.try_exists()? {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(super::control::token()?.as_bytes())?;
        file.sync_all()?;
    }
    read(directory)?;
    Ok(path)
}
fn read(directory: &Path) -> Result<String> {
    let path = directory.join("server-access.token");
    let meta = std::fs::symlink_metadata(&path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_token_file",
            "server token must be a private regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(AppError::new(
                "unsafe_token_file",
                "server token must have mode 0600",
            ));
        }
    }
    let value = std::fs::read_to_string(path)?;
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::new(
            "invalid_server_token",
            "invalid server access token file",
        ));
    }
    Ok(value)
}
pub(super) async fn gate(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let cookie = super::server::cookie(request.headers(), "lwc_server_access");
    let provided = request
        .headers()
        .get("x-lwc-server-token")
        .and_then(|v| v.to_str().ok());
    // Hash before comparison so mismatch timing does not reveal token prefixes.
    let mut allowed = read(&state.config.data).ok().is_some_and(|token| {
        let expected = if provided.is_some() {
            token.clone()
        } else {
            browser_token(&token)
        };
        let actual = provided.or(cookie.as_deref()).unwrap_or("");
        let expected = Sha256::digest(expected.as_bytes());
        let actual = Sha256::digest(actual.as_bytes());
        expected
            .iter()
            .zip(actual.iter())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    });
    if !allowed && let Some(key) = provided.filter(|key| super::keys::valid_format(key)) {
        let key = key.to_owned();
        allowed = super::server::database(&state, move |conn| super::keys::user(conn, &key))
            .await
            .is_ok();
    }
    if !allowed {
        return HttpError(AppError::new(
            "unauthorized",
            "server access token required",
        ))
        .into_response();
    }
    next.run(request).await
}

fn browser_token(token: &str) -> String {
    Sha256::digest(format!("lwc-browser-access-v1:{token}").as_bytes())
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Admission {
    token: String,
}
pub(super) async fn activate(
    State(state): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(input): axum::Json<Admission>,
) -> std::result::Result<Response, HttpError> {
    if headers.get("origin").and_then(|v| v.to_str().ok()) != Some(state.config.public_url.as_str())
    {
        return Err(AppError::new("forbidden", "same-origin activation required").into());
    }
    let expected = read(&state.config.data)?;
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(input.token.as_bytes());
    if a.iter()
        .zip(b.iter())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        != 0
    {
        return Err(AppError::new("unauthorized", "invalid server access token").into());
    }
    let secure = if state.config.public_url.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    Ok((
        [(
            axum::http::header::SET_COOKIE,
            format!(
                "lwc_server_access={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400{}",
                browser_token(&expected),
                secure
            ),
        )],
        axum::Json(serde_json::json!({"admitted":true})),
    )
        .into_response())
}
pub(super) async fn page() -> axum::response::Html<&'static str> {
    axum::response::Html(
        r#"<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>LWC · 连接团队</title><style>body{margin:0;background:#f7f8fc;color:#242424;font:16px system-ui;display:grid;min-height:100vh;place-items:center}form{width:min(360px,80vw);display:grid;gap:16px;padding:32px;border:1px solid #ddd;border-radius:12px;background:white}h1,p{margin:0}input,button{box-sizing:border-box;width:100%;padding:12px;border:1px solid #ccc;border-radius:6px;font:inherit}button{background:#005cff;color:white;cursor:pointer}#status{min-height:24px}</style><form><h1>连接 LWC 团队</h1><p>输入部署管理员提供的接入密钥。</p><label for="token">服务端密钥</label><input id="token" type="password" autocomplete="off" required minlength="64" maxlength="64"><button>连接</button><p id="status" role="status"></p></form><script>document.querySelector('form').onsubmit=async e=>{e.preventDefault();const status=document.querySelector('#status');try{const r=await fetch('/api/access',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token:document.querySelector('#token').value})});document.querySelector('#token').value='';if(!r.ok)throw Error();location.assign('/');}catch{status.textContent='连接失败，请检查接入密钥。';}};</script></html>"#,
    )
}

pub(crate) fn rotate(directory: &Path) -> Result<serde_json::Value> {
    super::private_directory(directory)?;
    super::control::open(directory)?;
    let path = directory.join("server-access.token");
    read(directory)?;
    let temporary = directory.join(format!(".server-access-{}", super::control::token()?));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(super::control::token()?.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)?;
        #[cfg(unix)]
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(serde_json::json!({"rotated":true,"server_token_file":path,"clients_must_reconfigure":true}))
}

pub(super) fn admission_cookie(state: &Shared) -> Result<String> {
    Ok(super::server::cookie_header(
        state,
        "lwc_server_access",
        &browser_token(&read(&state.config.data)?),
        86400,
    ))
}
