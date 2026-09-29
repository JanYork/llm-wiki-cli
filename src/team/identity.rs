use super::{
    control,
    server::{HttpError, Shared, database, session},
};
use crate::error::{AppError, Result};
use axum::{Json, extract::State, http::HeaderMap};
use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceProfile {
    pub device_id: String,
    pub os: String,
    pub arch: String,
    pub lwc_version: String,
    pub mac_fingerprint: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IdentityRegistration {
    pub email: String,
    pub nickname: String,
    pub device: DeviceProfile,
    pub agent_id: String,
    pub agent_name: String,
}
fn canonical_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl IdentityRegistration {
    pub(crate) fn validate(&self) -> Result<()> {
        control::normalize_email(&self.email)?;
        for label in [
            &self.nickname,
            &self.agent_name,
            &self.device.os,
            &self.device.arch,
            &self.device.lwc_version,
        ] {
            if label.trim().is_empty() || label.len() > 160 || label.chars().any(char::is_control) {
                return Err(AppError::new(
                    "invalid_identity",
                    "identity labels must contain 1–160 printable bytes",
                ));
            }
        }
        if !canonical_id(&self.device.device_id)
            || !canonical_id(&self.agent_id)
            || self
                .device
                .mac_fingerprint
                .as_deref()
                .is_some_and(|v| !canonical_id(v))
        {
            return Err(AppError::new(
                "invalid_identity",
                "invalid device or Agent identifier",
            ));
        }
        Ok(())
    }
}
pub(super) async fn register(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<IdentityRegistration>,
) -> std::result::Result<Json<Value>, HttpError> {
    if !headers.contains_key(axum::http::header::AUTHORIZATION) {
        return Err(AppError::new(
            "unauthorized",
            "device registration requires bearer authentication",
        )
        .into());
    }
    input.validate()?;
    let secret = session(&headers)?;
    Ok(Json(database(&state,move|conn| {
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let user=control::session_user(&tx,&secret)?;
        let email=control::normalize_email(&input.email)?;
        let verified:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM identities WHERE user_id=?1 AND provider='email' AND namespace='' AND subject=?2)",params![user,email],|r|r.get(0))?;
        // An email hint never binds an identity or changes the authenticated owner.
        tx.execute("INSERT INTO user_profiles(user_id,email_hint,nickname) VALUES(?1,?2,?3) ON CONFLICT(user_id) DO UPDATE SET email_hint=excluded.email_hint,nickname=excluded.nickname,updated_at=unixepoch()",params![user,email,input.nickname])?;
        let metadata=serde_json::to_string(&input.device).map_err(|_|AppError::new("invalid_identity","invalid device metadata"))?;
        tx.execute("INSERT INTO devices(id,user_id,metadata_json) VALUES(?1,?2,?3) ON CONFLICT(user_id,id) DO NOTHING",params![input.device.device_id,user,metadata])?;
        let current:Option<String>= {
            use rusqlite::OptionalExtension;
            tx.query_row("SELECT device_id FROM agents WHERE user_id=?1 AND id=?2",params![user,input.agent_id],|r|r.get(0)).optional()?
        };
        if current.as_deref().is_some_and(|v|v!=input.device.device_id) {return Err(AppError::new("invalid_identity","Agent already belongs to another device"));}
        tx.execute("INSERT INTO agents(id,user_id,device_id,name) VALUES(?1,?2,?3,?4) ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name",params![input.agent_id,user,input.device.device_id,input.agent_name])?;
        tx.execute("INSERT INTO control_audit(actor,action,target) VALUES(?1,'identity.register',?2)",params![user,input.agent_id])?;
        tx.commit()?;
        Ok(json!({"registered":true,"user_id":user,"email":email,"email_verified":verified,"nickname":input.nickname,"device_id":input.device.device_id,"agent_id":input.agent_id,"agent_name":input.agent_name,"identity_assurance":"authenticated-user; declared-agent"}))
    }).await?))
}
