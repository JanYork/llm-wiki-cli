use super::{
    control,
    server::{HttpError, Shared, database, session},
};
use crate::error::{AppError, Result};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, header},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Delegation {
    pub user_id: String,
    pub device_id: String,
    pub agent_id: String,
    pub space_id: String,
    pub can_write: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Grant {
    agent_id: String,
    space_id: String,
    #[serde(default)]
    can_write: bool,
}
pub(super) async fn grant(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Grant>,
) -> std::result::Result<Json<Value>, HttpError> {
    if !headers.contains_key(header::AUTHORIZATION) {
        return Err(
            AppError::new("unauthorized", "delegation requires a user bearer session").into(),
        );
    }
    let secret = session(&headers)?;
    Ok(Json(database(&state,move|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Deliberately owner sessions only: an Agent cannot mint another credential.
        let user=control::session_user(&tx,&secret)?;
        control::authorize(&tx,&user,&input.space_id,if input.can_write{"editor"}else{"viewer"})?;
        let device:String=tx.query_row("SELECT a.device_id FROM agents a JOIN devices d ON d.id=a.device_id AND d.user_id=a.user_id WHERE a.id=?1 AND a.user_id=?2 AND a.revoked=0 AND d.revoked=0",params![input.agent_id,user],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("forbidden","active registered Agent required"))?;
        let token=control::token()?;
        tx.execute("DELETE FROM agent_sessions WHERE user_id=?1 AND agent_id=?2 AND space_id=?3",params![user,input.agent_id,input.space_id])?;
        tx.execute("INSERT INTO agent_sessions VALUES(?1,?2,?3,?4,?5,?6,unixepoch()+86400)",params![control::digest(&token),user,device,input.agent_id,input.space_id,input.can_write])?;
        tx.execute("INSERT INTO control_audit(actor,action,target) VALUES(?1,'agent.delegate',?2)",params![user,input.agent_id])?;
        super::keys::inherit(&tx,&control::digest(&secret),&control::digest(&token))?;
        tx.commit()?;
        Ok(json!({"user_id":user,"device_id":device,"agent_id":input.agent_id,"space_id":input.space_id,"can_write":input.can_write,"access_token":token,"expires_in":86400}))
    }).await?))
}
pub(super) fn principal(
    conn: &Connection,
    secret: &str,
    space: &str,
    role: &str,
) -> Result<(String, Option<Delegation>)> {
    if let Ok(user) = control::session_user(conn, secret) {
        return Ok((user, None));
    }
    super::keys::check(conn, &control::digest(secret))?;
    let delegated=conn.query_row("SELECT s.user_id,s.device_id,s.agent_id,s.space_id,s.can_write FROM agent_sessions s JOIN users u ON u.id=s.user_id JOIN agents a ON a.id=s.agent_id AND a.user_id=s.user_id JOIN devices d ON d.id=s.device_id AND d.user_id=s.user_id WHERE s.token_hash=?1 AND s.expires_at>unixepoch() AND u.disabled=0 AND a.revoked=0 AND d.revoked=0",[control::digest(secret)],|r|Ok(Delegation{user_id:r.get(0)?,device_id:r.get(1)?,agent_id:r.get(2)?,space_id:r.get(3)?,can_write:r.get(4)?})).optional()?.ok_or_else(||AppError::new("unauthorized","invalid or expired Agent credential"))?;
    if delegated.space_id != space
        || role == "manager"
        || (role == "editor" && !delegated.can_write)
    {
        return Err(AppError::new(
            "forbidden",
            "Agent credential does not grant this space/action",
        ));
    }
    Ok((delegated.user_id.clone(), Some(delegated)))
}
// Each space operation owns a fresh control connection. This principal cannot leak into another request.
pub(super) fn bind(conn: &Connection, delegated: Option<&Delegation>, secret: &str) -> Result<()> {
    conn.execute_batch("CREATE TEMP TABLE request_principal(body TEXT,token_hash TEXT NOT NULL);")?;
    let body = delegated
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| AppError::new("invalid_identity", "invalid principal"))?;
    conn.execute(
        "INSERT INTO request_principal VALUES(?1,?2)",
        params![body, control::digest(secret)],
    )?;
    Ok(())
}
pub(super) fn current(conn: &Connection) -> Result<Option<Delegation>> {
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='request_principal')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    let raw: Option<String> = conn
        .query_row("SELECT body FROM request_principal LIMIT 1", [], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    raw.map(|raw| {
        serde_json::from_str(&raw)
            .map_err(|_| AppError::new("invalid_identity", "invalid principal"))
    })
    .transpose()
}
