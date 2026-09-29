use super::*;
use crate::team::{DeviceProfile, IdentityRegistration};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    server: String,
    user_id: String,
    email: String,
    nickname: String,
    device: DeviceProfile,
    agents: std::collections::BTreeMap<String, String>,
}
fn collect_device() -> Result<DeviceProfile> {
    let device_id = spaces::random_id()?;
    #[cfg(windows)]
    let output = std::process::Command::new("getmac.exe")
        .args(["/fo", "csv", "/nh"])
        .output();
    #[cfg(not(windows))]
    let output = std::process::Command::new("ifconfig").output();
    let mut addresses = output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter_map(|value| {
                    let value = value
                        .trim_matches(['"', ','])
                        .replace('-', ":")
                        .to_ascii_lowercase();
                    (value.len() == 17
                        && value.split(':').count() == 6
                        && value
                            .split(':')
                            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
                        && value != "00:00:00:00:00:00")
                        .then_some(value)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    addresses.sort();
    addresses.dedup();
    let mac_fingerprint = if addresses.is_empty() {
        None
    } else {
        Some(
            Sha256::digest(format!("{device_id}:{}", addresses.join(",")))
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        )
    };
    Ok(DeviceProfile {
        device_id,
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        lwc_version: env!("CARGO_PKG_VERSION").into(),
        mac_fingerprint,
    })
}

/// User-level configuration and authenticated registration; metadata cannot grant permissions.
pub(crate) fn configure_identity(
    server: &str,
    email: &str,
    nickname: &str,
    agent: &str,
) -> Result<Value> {
    let server = origin(server)?;
    let credentials = spaces::credentials_for(&server, None)?;
    let account = account_file(&server)?.parent().unwrap().to_owned();
    crate::team::private_directory(&account)?;
    let _lock = spaces::sync_lock(&account)?;
    let path = account.join("identity.json");
    let mut profile: Profile = if path.try_exists()? {
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(AppError::new(
                "unsafe_credentials_path",
                "identity file cannot be a symlink",
            ));
        }
        serde_json::from_slice(&fs::read(&path)?)
            .map_err(|_| AppError::new("invalid_identity", "invalid saved identity"))?
    } else {
        Profile {
            server: server.clone(),
            user_id: credentials.user_id.clone(),
            email: email.into(),
            nickname: nickname.into(),
            device: collect_device()?,
            agents: Default::default(),
        }
    };
    if profile.server != server || profile.user_id != credentials.user_id {
        return Err(AppError::new(
            "replica_account_changed",
            "identity belongs to another account; explicitly use a separate account directory",
        ));
    }
    let agent_id = if let Some(id) = profile.agents.get(agent) {
        id.clone()
    } else {
        spaces::random_id()?
    };
    let registration = IdentityRegistration {
        email: email.into(),
        nickname: nickname.into(),
        device: profile.device.clone(),
        agent_id: agent_id.clone(),
        agent_name: agent.into(),
    };
    registration.validate()?;
    profile.email = email.into();
    profile.nickname = nickname.into();
    profile.agents.insert(agent.into(), agent_id);
    // Preserve one-time device sampling and retry identity even if the network request fails.
    save_credentials(&path, &profile)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            request_json(
                client(&server)?
                    .post(format!("{server}/api/identity/register"))
                    .bearer_auth(&credentials.access_token)
                    .json(&registration),
            )
            .await
        })
}

pub(crate) fn delegate(
    server: &str,
    agent: &str,
    space: &str,
    write: bool,
    output: &Path,
) -> Result<Value> {
    let server = origin(server)?;
    // Granting always uses the owner's login, never an injected Agent credential.
    let owner: Credentials = serde_json::from_slice(&fs::read(account_file(&server)?)?)
        .map_err(|_| AppError::new("credential_error", "invalid owner credentials"))?;
    if owner.server != server {
        return Err(AppError::new("credential_error", "saved server mismatch"));
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let mut result=request_json(client(&server)?.post(format!("{server}/api/agents/delegate")).bearer_auth(&owner.access_token).json(&json!({"agent_id":agent,"space_id":space,"can_write":write}))).await?;
        let token=result["access_token"].as_str().ok_or_else(||AppError::new("invalid_response","missing delegated credential"))?.to_owned();
        let credentials=Credentials{server:server.clone(),user_id:owner.user_id,access_token:token,agent_id:Some(agent.into()),device_id:result["device_id"].as_str().map(str::to_owned)};
        save_credentials(output,&credentials)?;
        result.as_object_mut().unwrap().remove("access_token");
        result["credentials_file"]=json!(output);result["usage"]=json!("Set LWC_TEAM_CREDENTIALS_FILE to this private file in the Agent environment; do not give the Agent the owner login file.");
        Ok(result)
    })
}
