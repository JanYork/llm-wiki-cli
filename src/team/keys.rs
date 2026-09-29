//! Personal keys authenticate people; space grants remain authoritative.
use super::{
    control,
    server::{HttpError, Shared, browser_origin, cookie_header, database},
};
use crate::error::{AppError, Result};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

pub(crate) fn valid_format(key: &str) -> bool {
    key.strip_prefix("lwc_user_")
        .is_some_and(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(super) fn user(conn: &Connection, key: &str) -> Result<String> {
    if !valid_format(key) {
        return Err(AppError::new(
            "invalid_personal_key",
            "invalid or expired personal key",
        ));
    }
    conn.query_row("SELECT k.user_id FROM personal_keys k JOIN users u ON u.id=k.user_id WHERE k.key_hash=?1 AND k.revoked=0 AND k.expires_at>unixepoch() AND u.disabled=0",[control::digest(key)],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("invalid_personal_key","invalid or expired personal key"))
}
pub(super) fn issue(
    tx: &Transaction<'_>,
    user: &str,
    actor: &str,
    name: &str,
    days: i64,
) -> Result<Value> {
    if ![7, 30, 90, 365].contains(&days) {
        return Err(AppError::new("invalid_request", "invalid key duration"));
    }
    let key = format!("lwc_user_{}", control::token()?);
    let hash = control::digest(&key);
    tx.execute("INSERT INTO personal_keys(key_hash,user_id,issued_by,name,expires_at) VALUES(?1,?2,?3,?4,unixepoch()+?5)",params![hash,user,actor,name,days*86400])?;
    control::audit(tx, actor, "key.create", &hash)?;
    Ok(json!({"personal_key":key,"expires_in":days*86400}))
}
pub(super) fn inherit(conn: &Connection, parent: &str, child: &str) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO key_credentials SELECT ?1,key_hash FROM key_credentials WHERE credential_hash=?2",params![child,parent])?;
    Ok(())
}
pub(super) fn check(conn: &Connection, credential_hash: &str) -> Result<()> {
    let denied:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM key_credentials c WHERE c.credential_hash=?1 AND NOT EXISTS(SELECT 1 FROM personal_keys k WHERE k.key_hash=c.key_hash AND k.revoked=0 AND k.expires_at>unixepoch()))",[credential_hash],|r|r.get(0))?;
    if denied {
        return Err(AppError::new(
            "unauthorized",
            "originating personal key is revoked or expired",
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Login {
    key: String,
    #[serde(default)]
    cli: bool,
}
pub(super) async fn login(
    State(state): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> std::result::Result<Response, HttpError> {
    if !input.cli {
        browser_origin(&state, &headers)?;
    }
    let cli = input.cli;
    let result = database(&state, move |conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        super::auth::rate(
            &tx,
            &format!("key-login:{}", control::digest(&peer.ip().to_string())),
            60,
            20,
        )?;
        // Persist failed attempts too; rollback must not erase the rate limit.
        let authenticated = user(&tx, &input.key);
        tx.commit()?;
        let user_id = authenticated?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        // Recheck inside the session transaction, including concurrent revocation.
        user(&tx, &input.key)?;
        let token = control::create_session(&tx, &user_id)?;
        tx.execute(
            "INSERT INTO key_credentials VALUES(?1,?2)",
            params![control::digest(&token), control::digest(&input.key)],
        )?;
        control::audit(&tx, &user_id, "login.key", &user_id)?;
        tx.commit()?;
        Ok(json!({"authenticated":true,"user_id":user_id,"access_token":token}))
    })
    .await?;
    if cli {
        return Ok(([(header::CACHE_CONTROL, "no-store")], Json(result)).into_response());
    }
    let token = result["access_token"].as_str().unwrap();
    Ok((
        axum::response::AppendHeaders([
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::SET_COOKIE,
                cookie_header(&state, "lwc_session", token, 2592000),
            ),
            (header::SET_COOKIE, super::access::admission_cookie(&state)?),
        ]),
        Json(json!({"authenticated":true})),
    )
        .into_response())
}
