use super::policy::{self, Denial};
use crate::error::{AppError, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyLease {
    pub version: u32,
    pub credential_hash: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub device_id: Option<String>,
    pub space_id: String,
    pub epoch: String,
    pub revision: i64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub role: String,
    pub denials: Vec<Denial>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignedPolicy {
    pub payload: String,
    pub signature: String,
    pub public_key: String,
}
fn invalid() -> AppError {
    AppError::new("invalid_policy_signature", "invalid signed space policy")
}
pub(crate) fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| invalid())
}
fn key(directory: &Path) -> Result<SigningKey> {
    let path = directory.join("policy-signing.key");
    if !path.try_exists()? {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| AppError::new("entropy_unavailable", "secure randomness unavailable"))?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(&bytes)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(invalid());
    }
    let bytes: [u8; 32] = fs::read(path)?.try_into().map_err(|_| invalid())?;
    Ok(SigningKey::from_bytes(&bytes))
}
pub(super) fn issue(
    conn: &Connection,
    directory: &Path,
    user: &str,
    space: &str,
    role: &str,
) -> Result<SignedPolicy> {
    let (epoch, revision): (String, i64) = conn.query_row(
        "SELECT epoch,revision FROM spaces WHERE id=?1",
        [space],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let issued_at = now()?;
    let principal = super::delegation::current(conn)?;
    let lease = PolicyLease {
        version: 1,
        credential_hash: conn
            .query_row("SELECT token_hash FROM request_principal", [], |r| r.get(0))?,
        user_id: user.into(),
        agent_id: principal.as_ref().map(|p| p.agent_id.clone()),
        device_id: principal.map(|p| p.device_id),
        space_id: space.into(),
        epoch,
        revision,
        issued_at,
        expires_at: issued_at + 900,
        role: role.into(),
        denials: policy::denials(conn, user, space)?,
    };
    let payload = serde_json::to_string(&lease).map_err(|_| invalid())?;
    let key = key(directory)?;
    Ok(SignedPolicy {
        signature: STANDARD.encode(key.sign(payload.as_bytes()).to_bytes()),
        public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        payload,
    })
}
impl SignedPolicy {
    pub(crate) fn verify_payload(&self) -> Result<serde_json::Value> {
        if self.payload.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let key: [u8; 32] = STANDARD
            .decode(&self.public_key)
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let signature =
            Signature::from_slice(&STANDARD.decode(&self.signature).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        VerifyingKey::from_bytes(&key)
            .map_err(|_| invalid())?
            .verify_strict(self.payload.as_bytes(), &signature)
            .map_err(|_| invalid())?;
        serde_json::from_str(&self.payload).map_err(|_| invalid())
    }
    pub(crate) fn verify(&self) -> Result<PolicyLease> {
        let policy: PolicyLease =
            serde_json::from_value(self.verify_payload()?).map_err(|_| invalid())?;
        if policy.version != 1
            || !matches!(policy.role.as_str(), "viewer" | "editor" | "manager")
            || policy.revision < 1
            || policy.expires_at <= policy.issued_at
            || policy.expires_at - policy.issued_at > 86400
            || policy.denials.len() > 256
        {
            return Err(invalid());
        }
        for rule in &policy.denials {
            rule.validate()?;
        }
        Ok(policy)
    }
}

pub(crate) fn sign(directory: &Path, value: &serde_json::Value) -> Result<SignedPolicy> {
    let payload = serde_json::to_string(value).map_err(|_| invalid())?;
    let key = key(directory)?;
    Ok(SignedPolicy {
        signature: STANDARD.encode(key.sign(payload.as_bytes()).to_bytes()),
        public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        payload,
    })
}
