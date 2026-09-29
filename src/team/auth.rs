use super::{
    control,
    server::{HttpError, OAuth, Shared, browser_origin, cookie, cookie_header, database, secret},
};
use crate::error::{AppError, Result};
use axum::{
    Json,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, time::Duration};

fn failed() -> AppError {
    AppError::new(
        "login_failed",
        "invalid, expired, or already used login challenge",
    )
}
fn provider<'a>(state: &'a Shared, name: &str) -> Result<&'a OAuth> {
    match name {
        "github" => state.config.github.as_ref(),
        "feishu" => state.config.feishu.as_ref(),
        _ => None,
    }
    .ok_or_else(|| AppError::new("provider_unavailable", "login provider is not configured"))
}

pub(super) async fn providers(State(state): State<Shared>) -> Json<Value> {
    Json(
        json!({"email":state.config.smtp.is_some(),"github":state.config.github.is_some(),"feishu":state.config.feishu.is_some()}),
    )
}

pub(super) fn rate(tx: &Transaction<'_>, key: &str, seconds: i64, maximum: i64) -> Result<()> {
    let now: i64 = tx.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
    tx.execute("INSERT INTO login_rate(key,window,count) VALUES(?1,?2,1) ON CONFLICT(key) DO UPDATE SET window=excluded.window,count=CASE WHEN login_rate.window=excluded.window THEN login_rate.count+1 ELSE 1 END",params![key,now/seconds])?;
    let count: i64 = tx.query_row("SELECT count FROM login_rate WHERE key=?1", [key], |r| {
        r.get(0)
    })?;
    if count > maximum {
        return Err(AppError::new(
            "rate_limited",
            "too many login attempts; retry later",
        ));
    }
    Ok(())
}

fn challenge(
    conn: &mut Connection,
    kind: &str,
    subject: &str,
    secret_hash: &str,
    binding: &str,
    peer: &str,
) -> Result<String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    rate(&tx, "global", 60, 100)?;
    rate(&tx, &format!("peer:{peer}"), 60, 20)?;
    rate(
        &tx,
        &format!("subject:{kind}:{}", control::digest(subject)),
        60,
        1,
    )?;
    rate(
        &tx,
        &format!("hour:{kind}:{}", control::digest(subject)),
        3600,
        5,
    )?;
    tx.execute(
        "DELETE FROM login_challenges WHERE expires_at<unixepoch()-86400",
        [],
    )?;
    let id = control::token()?;
    tx.execute(
        "UPDATE login_challenges SET consumed=1 WHERE kind=?1 AND subject=?2",
        params![kind, subject],
    )?;
    tx.execute("INSERT INTO login_challenges(id,kind,subject,secret_hash,binding_hash,expires_at) VALUES(?1,?2,?3,?4,?5,unixepoch()+300)",params![id,kind,subject,secret_hash,control::digest(binding)])?;
    tx.commit()?;
    Ok(id)
}

fn consume(
    conn: &mut Connection,
    id: &str,
    kind: &str,
    binding: &str,
    secret_hash: &str,
) -> Result<String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row=tx.query_row("SELECT subject,secret_hash,binding_hash FROM login_challenges WHERE id=?1 AND kind=?2 AND expires_at>unixepoch() AND consumed=0 AND attempts<5",params![id,kind],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).optional()?;
    let Some((subject, expected, bound)) = row else {
        return Err(failed());
    };
    tx.execute(
        "UPDATE login_challenges SET attempts=attempts+1 WHERE id=?1",
        [id],
    )?;
    let valid = expected == secret_hash && bound == control::digest(binding);
    if valid {
        tx.execute("UPDATE login_challenges SET consumed=1 WHERE id=?1", [id])?;
    }
    tx.commit()?;
    if valid { Ok(subject) } else { Err(failed()) }
}

fn otp() -> Result<String> {
    loop {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes)
            .map_err(|_| AppError::new("entropy_unavailable", "secure randomness unavailable"))?;
        let value = u32::from_le_bytes(bytes);
        if value < 4_294_000_000 {
            return Ok(format!("{:06}", value % 1_000_000));
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Email {
    email: String,
    #[serde(default)]
    link: bool,
}
pub(super) async fn email_challenge(
    State(state): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Email>,
) -> std::result::Result<Response, HttpError> {
    browser_origin(&state, &headers)?;
    let smtp =
        state.config.smtp.as_ref().ok_or_else(|| {
            AppError::new("provider_unavailable", "email login is not configured")
        })?;
    let email = control::normalize_email(&input.email)?;
    let target = link_target(&state, &headers, input.link).await?;
    let kind = target
        .map(|user| format!("email:link:{user}"))
        .unwrap_or_else(|| "email".into());
    let binding = control::token()?;
    let code = otp()?;
    let bound = binding.clone();
    let recipient = email.clone();
    let hash = control::digest(&format!("{binding}:{code}"));
    let peer = control::digest(&peer.ip().to_string());
    let id = database(&state, move |conn| {
        challenge(conn, &kind, &recipient, &hash, &bound, &peer)
    })
    .await?;
    let message = Message::builder()
        .from(
            smtp.from
                .parse()
                .map_err(|_| AppError::new("provider_unavailable", "invalid SMTP sender"))?,
        )
        .to(email
            .parse()
            .map_err(|_| AppError::new("invalid_email", "invalid email address"))?)
        .subject("LWC 登录验证码")
        .body(format!(
            "你的 LWC 登录验证码是 {code}，5 分钟内有效。如非本人操作，请忽略。"
        ))
        .map_err(|_| AppError::new("provider_unavailable", "cannot prepare login email"))?;
    let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
        .map_err(|_| AppError::new("provider_unavailable", "invalid SMTP configuration"))?
        .port(smtp.port)
        .timeout(Some(Duration::from_secs(15)))
        .credentials(Credentials::new(
            smtp.username.clone(),
            secret(&smtp.password_env)?,
        ))
        .build();
    if transport.send(message).await.is_err() {
        let failed_id = id.clone();
        database(&state, move |conn| {
            conn.execute(
                "UPDATE login_challenges SET consumed=1 WHERE id=?1",
                [failed_id],
            )?;
            Ok(())
        })
        .await?;
        return Err(AppError::new(
            "provider_unavailable",
            "login email delivery failed; retry later",
        )
        .into());
    }
    Ok((
        [(
            header::SET_COOKIE,
            cookie_header(&state, "lwc_email", &binding, 300),
        )],
        Json(json!({"challenge_id":id,"expires_in":300})),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Verify {
    challenge_id: String,
    code: String,
}
pub(super) async fn email_verify(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Verify>,
) -> std::result::Result<Response, HttpError> {
    browser_origin(&state, &headers)?;
    let binding = cookie(&headers, "lwc_email").ok_or_else(failed)?;
    let (kind, target) = challenge_scope(&state, &headers, "email", &input.challenge_id).await?;
    if input.code.len() != 6 || !input.code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(failed().into());
    }
    let hash = control::digest(&format!("{binding}:{}", input.code));
    let session = database(&state, move |conn| {
        let email = consume(conn, &input.challenge_id, &kind, &binding, &hash)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user = if let Some(user) = target {
            control::link_identity(&tx, &user, "email", "", &email)?;
            user
        } else {
            control::identity_user(&tx, "email", "", &email)?
        };
        let session = control::create_session(&tx, &user)?;
        control::audit(&tx, &user, "login.email", &user)?;
        tx.commit()?;
        Ok(session)
    })
    .await?;
    Ok((
        axum::response::AppendHeaders([
            (
                header::SET_COOKIE,
                cookie_header(&state, "lwc_session", &session, 2592000),
            ),
            (
                header::SET_COOKIE,
                cookie_header(&state, "lwc_email", "", 0),
            ),
        ]),
        Json(json!({"authenticated":true})),
    )
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct LinkQuery {
    #[serde(default)]
    link: bool,
}
pub(super) async fn oauth_start(
    State(state): State<Shared>,
    Path(name): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(input): Query<LinkQuery>,
) -> std::result::Result<Response, HttpError> {
    let config = provider(&state, &name)?;
    let target = link_target(&state, &headers, input.link).await?;
    let kind = target
        .map(|user| format!("{name}:link:{user}"))
        .unwrap_or_else(|| name.clone());
    let verifier = control::token()?;
    let bound = verifier.clone();
    let peer = control::digest(&peer.ip().to_string());
    // The PKCE verifier lives only in a short-lived HttpOnly cookie, not the control database.
    let challenge_id = database(&state, move |conn| {
        challenge(
            conn,
            &kind,
            &control::digest(&bound),
            &control::digest(&bound),
            &bound,
            &peer,
        )
    })
    .await?;
    let callback = format!("{}/api/auth/{name}/callback", state.config.public_url);
    let endpoint = if name == "github" {
        "https://github.com/login/oauth/authorize"
    } else {
        "https://accounts.feishu.cn/open-apis/authen/v1/authorize"
    };
    let mut url = reqwest::Url::parse(endpoint)
        .map_err(|_| AppError::new("internal_error", "invalid provider endpoint"))?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", callback.as_str()),
        ("state", challenge_id.as_str()),
        ("response_type", "code"),
    ]);
    if name == "github" {
        url.query_pairs_mut()
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256");
    }
    Ok((
        [(
            header::SET_COOKIE,
            cookie_header(
                &state,
                "lwc_oauth",
                &format!("{challenge_id}.{verifier}"),
                300,
            ),
        )],
        Redirect::temporary(url.as_str()),
    )
        .into_response())
}

#[derive(Deserialize)]
pub(super) struct Callback {
    state: String,
    code: String,
}
pub(super) async fn oauth_callback(
    State(state): State<Shared>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Query(input): Query<Callback>,
) -> std::result::Result<Response, HttpError> {
    let config = provider(&state, &name)?;
    if input.code.len() > 4096 || input.state.len() != 64 {
        return Err(failed().into());
    }
    let raw = cookie(&headers, "lwc_oauth").ok_or_else(failed)?;
    let (bound, verifier) = raw.split_once('.').ok_or_else(failed)?;
    if bound != input.state || verifier.len() != 64 {
        return Err(failed().into());
    }
    let (kind, target) = challenge_scope(&state, &headers, &name, &input.state).await?;
    let verifier = verifier.to_owned();
    let binding = verifier.clone();
    let id = input.state;
    database(&state, move |conn| {
        consume(conn, &id, &kind, &binding, &control::digest(&binding))
    })
    .await?;
    let callback = format!("{}/api/auth/{name}/callback", state.config.public_url);
    let client_secret = secret(&config.client_secret_env)?;
    let mut fields = json!({"grant_type":"authorization_code","client_id":config.client_id,"client_secret":client_secret,"code":input.code,"redirect_uri":callback});
    if name == "github" {
        fields["code_verifier"] = json!(verifier);
    }
    let request = if name == "github" {
        state
            .client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&fields)
    } else {
        state
            .client
            .post("https://open.feishu.cn/open-apis/authen/v2/oauth/token")
            .json(&fields)
    };
    let tokens = provider_json(request).await?;
    if name == "feishu" && tokens["code"].as_i64().is_some_and(|code| code != 0) {
        return Err(failed().into());
    }
    let access = tokens["access_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(failed)?;
    let info_url = if name == "github" {
        "https://api.github.com/user"
    } else {
        "https://open.feishu.cn/open-apis/authen/v1/user_info"
    };
    let identity = provider_json(state.client.get(info_url).bearer_auth(access)).await?;
    let (namespace, subject) = if name == "github" {
        (
            config.client_id.clone(),
            identity["id"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or_else(failed)?
                .to_string(),
        )
    } else {
        if identity["code"].as_i64() != Some(0) {
            return Err(failed().into());
        }
        let tenant = identity["data"]["tenant_key"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(failed)?;
        let subject = identity["data"]["open_id"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(failed)?;
        (format!("{}:{tenant}", config.client_id), subject.to_owned())
    };
    let session = database(&state, move |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user = if let Some(user) = target {
            control::link_identity(&tx, &user, &name, &namespace, &subject)?;
            user
        } else {
            control::identity_user(&tx, &name, &namespace, &subject)?
        };
        let session = control::create_session(&tx, &user)?;
        control::audit(&tx, &user, &format!("login.{name}"), &user)?;
        tx.commit()?;
        Ok(session)
    })
    .await?;
    Ok((
        axum::response::AppendHeaders([
            (
                header::SET_COOKIE,
                cookie_header(&state, "lwc_session", &session, 2592000),
            ),
            (
                header::SET_COOKIE,
                cookie_header(&state, "lwc_oauth", "", 0),
            ),
        ]),
        Redirect::to("/"),
    )
        .into_response())
}

async fn provider_json(request: reqwest::RequestBuilder) -> Result<Value> {
    let mut response = request
        .send()
        .await
        .map_err(|_| AppError::new("provider_unavailable", "identity provider request failed"))?;
    if !response.status().is_success() {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::new("provider_unavailable", "identity provider response failed"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| failed())
}

async fn link_target(state: &Shared, headers: &HeaderMap, link: bool) -> Result<Option<String>> {
    if !link {
        return Ok(None);
    }
    browser_origin(state, headers)?;
    let session = super::server::session(headers)?;
    database(state, move |conn| {
        control::session_user(conn, &session).map(Some)
    })
    .await
}

async fn challenge_scope(
    state: &Shared,
    headers: &HeaderMap,
    provider: &str,
    id: &str,
) -> Result<(String, Option<String>)> {
    let id = id.to_owned();
    let provider = provider.to_owned();
    let session = super::server::session(headers).ok();
    database(state,move|conn|{
        let kind:String=conn.query_row("SELECT kind FROM login_challenges WHERE id=?1 AND consumed=0 AND expires_at>unixepoch()",[id],|r|r.get(0)).optional()?.ok_or_else(failed)?;
        if kind==provider {return Ok((kind,None));}
        let target=kind.strip_prefix(&format!("{provider}:link:")).ok_or_else(failed)?.to_owned();
        let user=control::session_user(conn,session.as_deref().ok_or_else(failed)?)?;
        if user!=target {return Err(failed());}Ok((kind,Some(user)))
    }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceName {
    name: String,
}
pub(super) async fn device_start(
    State(state): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<DeviceName>,
) -> std::result::Result<Json<Value>, HttpError> {
    if input.name.trim().is_empty()
        || input.name.len() > 160
        || input.name.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "invalid_request",
            "device name must contain 1–160 printable bytes",
        )
        .into());
    }
    let result=database(&state,move|conn|{
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        rate(&tx,"device-global",60,100)?;rate(&tx,&format!("device-peer:{}",control::digest(&peer.ip().to_string())),60,10)?;
        tx.execute("DELETE FROM device_authorizations WHERE expires_at<unixepoch()",[])?;
        let device=control::token()?;let user_code=control::token()?[..8].to_ascii_uppercase();
        tx.execute("INSERT INTO device_authorizations(device_hash,user_code_hash,name,expires_at) VALUES(?1,?2,?3,unixepoch()+600)",params![control::digest(&device),control::digest(&user_code),input.name])?;
        tx.commit()?;Ok(json!({"device_code":device,"user_code":user_code,"expires_in":600,"interval":5}))
    }).await?;
    let mut result = result;
    result["verification_uri"] = json!(format!("{}/devices/authorize", state.config.public_url));
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceCode {
    device_code: String,
}
fn poll_device(conn: &mut Connection, device: &str) -> Result<Value> {
    if device.len() != 64 {
        return Err(failed());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let hash = control::digest(device);
    let row=tx.query_row("SELECT user_id,last_poll,unixepoch() FROM device_authorizations WHERE device_hash=?1 AND expires_at>unixepoch() AND consumed=0",[&hash],|r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).optional()?.ok_or_else(failed)?;
    if row.2 - row.1 < 5 {
        return Ok(json!({"status":"slow_down","interval":5}));
    }
    tx.execute(
        "UPDATE device_authorizations SET last_poll=unixepoch() WHERE device_hash=?1",
        [&hash],
    )?;
    let result = if let Some(user) = row.0 {
        super::keys::check(&tx, &hash)?;
        let session = control::create_session(&tx, &user)?;
        super::keys::inherit(&tx, &hash, &control::digest(&session))?;
        tx.execute(
            "UPDATE device_authorizations SET consumed=1 WHERE device_hash=?1",
            [&hash],
        )?;
        control::audit(&tx, &user, "device.authorized", &hash)?;
        json!({"status":"authorized","access_token":session,"token_type":"Bearer","expires_in":2592000,"user_id":user})
    } else {
        json!({"status":"authorization_pending","interval":5})
    };
    tx.commit()?;
    Ok(result)
}
pub(super) async fn device_poll(
    State(state): State<Shared>,
    Json(input): Json<DeviceCode>,
) -> std::result::Result<Json<Value>, HttpError> {
    Ok(Json(
        database(&state, move |conn| poll_device(conn, &input.device_code)).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeviceApproval {
    user_code: String,
}
pub(super) async fn device_preview(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<DeviceApproval>,
) -> std::result::Result<Json<Value>, HttpError> {
    browser_origin(&state, &headers)?;
    let session = super::server::session(&headers)?;
    if input.user_code.len() != 8 || !input.user_code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failed().into());
    }
    Ok(Json(database(&state, move |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user = control::session_user(&tx, &session)?;
        rate(&tx, &format!("device-preview:{user}"), 60, 20)?;
        let result = tx.query_row("SELECT name,expires_at FROM device_authorizations WHERE user_code_hash=?1 AND user_id IS NULL AND consumed=0 AND expires_at>unixepoch()", [control::digest(&input.user_code.to_ascii_uppercase())], |row| Ok(json!({"name":row.get::<_,String>(0)?,"expires_at":row.get::<_,i64>(1)?}))).optional()?;
        tx.commit()?;
        result.ok_or_else(failed)
    }).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InvitationPreview {
    invitation_token: String,
}
pub(super) async fn invitation_preview(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<InvitationPreview>,
) -> std::result::Result<Json<Value>, HttpError> {
    browser_origin(&state, &headers)?;
    if input.invitation_token.len() != 64
        || !input
            .invitation_token
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::new("invalid_invitation", "invitation is invalid or expired").into());
    }
    Ok(Json(database(&state, move |conn| {
        conn.query_row("SELECT t.name,i.expires_at FROM invitations i JOIN teams t ON t.id=i.team_id WHERE i.token_hash=?1 AND i.expires_at>unixepoch() AND i.consumed_by IS NULL", [control::digest(&input.invitation_token)], |row|Ok(json!({"team_name":row.get::<_,String>(0)?,"expires_at":row.get::<_,i64>(1)?}))).optional()?.ok_or_else(||AppError::new("invalid_invitation","invitation is invalid or expired"))
    }).await?))
}

pub(super) async fn device_approve(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<DeviceApproval>,
) -> std::result::Result<Json<Value>, HttpError> {
    browser_origin(&state, &headers)?;
    let session = super::server::session(&headers)?;
    if input.user_code.len() != 8 || !input.user_code.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failed().into());
    }
    Ok(Json(database(&state,move|conn|{
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user=control::session_user(&tx,&session)?;
        rate(&tx,&format!("device-approve:{user}"),60,10)?;
        let changed=tx.execute("UPDATE device_authorizations SET user_id=?1 WHERE user_code_hash=?2 AND user_id IS NULL AND consumed=0 AND expires_at>unixepoch()",params![user,control::digest(&input.user_code.to_ascii_uppercase())])?;
        if changed==1 {
            let device:String=tx.query_row("SELECT device_hash FROM device_authorizations WHERE user_code_hash=?1",[control::digest(&input.user_code.to_ascii_uppercase())],|r|r.get(0))?;
            super::keys::inherit(&tx,&control::digest(&session),&device)?;
        }
        tx.commit()?;
        if changed!=1 {return Err(failed());}Ok(json!({"approved":true}))
    }).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn team_login_challenge_is_bound_single_use_and_locks_after_five_failures() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("team");
        control::initialize(&data, "owner@example.com", "Test").unwrap();
        let mut conn = control::open(&data).unwrap();
        let id = challenge(
            &mut conn,
            "email",
            "one@example.com",
            "correct",
            "browser",
            "peer",
        )
        .unwrap();
        for _ in 0..5 {
            assert!(consume(&mut conn, &id, "email", "browser", "wrong").is_err());
        }
        assert!(consume(&mut conn, &id, "email", "browser", "correct").is_err());
        let id = challenge(
            &mut conn,
            "email",
            "two@example.com",
            "correct",
            "browser",
            "peer",
        )
        .unwrap();
        assert!(consume(&mut conn, &id, "email", "other-browser", "correct").is_err());
        assert_eq!(
            consume(&mut conn, &id, "email", "browser", "correct").unwrap(),
            "two@example.com"
        );
        assert!(consume(&mut conn, &id, "email", "browser", "correct").is_err());
        assert!(
            challenge(
                &mut conn,
                "email",
                "two@example.com",
                "another",
                "browser",
                "peer"
            )
            .is_err()
        );
    }

    #[test]
    fn team_device_requires_approval_and_cannot_replay() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("team");
        let initialized = control::initialize(&data, "owner@example.com", "Test").unwrap();
        let mut conn = control::open(&data).unwrap();
        let device = control::token().unwrap();
        let hash = control::digest(&device);
        conn.execute("INSERT INTO device_authorizations VALUES(?1,'user-code','test',NULL,unixepoch()+600,0,0)",[&hash]).unwrap();
        assert_eq!(
            poll_device(&mut conn, &device).unwrap()["status"],
            "authorization_pending"
        );
        assert_eq!(
            poll_device(&mut conn, &device).unwrap()["status"],
            "slow_down"
        );
        conn.execute(
            "UPDATE device_authorizations SET user_id=?1,last_poll=0 WHERE device_hash=?2",
            params![initialized["user_id"].as_str().unwrap(), hash],
        )
        .unwrap();
        let authorized = poll_device(&mut conn, &device).unwrap();
        assert_eq!(authorized["status"], "authorized");
        assert!(control::session_user(&conn, authorized["access_token"].as_str().unwrap()).is_ok());
        assert!(poll_device(&mut conn, &device).is_err());
    }
}
