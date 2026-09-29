use crate::error::{AppError, Result};
mod daemon;
mod engine;
mod identity;
mod spaces;
pub(crate) use daemon::{configure_space, start_worker, supervise, watch_space};
pub(crate) use engine::{
    claim_conflict, conflict_candidate, conflict_packet, conflict_signal, resolve_space,
    resolve_space_json, signal_for_database, sync_space,
};
pub(crate) use identity::{configure_identity, delegate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(crate) use spaces::{
    bind_project, join_space, list_spaces, project_binding, selected_database, show_space,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    server: String,
    user_id: String,
    access_token: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
}

fn origin(input: &str) -> Result<String> {
    let url = reqwest::Url::parse(input)
        .map_err(|_| AppError::new("invalid_server", "server must be an HTTPS origin"))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !(url.scheme() == "https" || (url.scheme() == "http" && local))
    {
        return Err(AppError::new(
            "invalid_server",
            "server must be an HTTPS origin; HTTP is limited to loopback development",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn account_file(server: &str) -> Result<PathBuf> {
    let key: String = Sha256::digest(server.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(crate::scope::global_lwc_root()?
        .join("team")
        .join("accounts")
        .join(key)
        .join("credentials.json"))
}

fn save_credentials(path: &Path, credentials: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        AppError::new("unsafe_credentials_path", "credential directory is missing")
    })?;
    crate::team::private_directory(parent)?;
    if path.try_exists()? && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_credentials_path",
            "credential file cannot be a symlink",
        ));
    }
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| AppError::new("entropy_unavailable", "secure randomness unavailable"))?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = parent.join(format!(".credentials-{id}"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        let bytes = serde_json::to_vec(credentials)
            .map_err(|_| AppError::new("credential_error", "cannot encode credentials"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn configure_server(server: &str) -> Result<Value> {
    use std::io::Read;
    let server = origin(server)?;
    let mut value = String::new();
    std::io::stdin().take(4097).read_to_string(&mut value)?;
    let token = value.trim();
    if !(crate::team::valid_personal_key(token)
        || token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(AppError::new(
            "invalid_server_token",
            "expected the server-generated token on stdin",
        ));
    }
    save_credentials(
        &account_file(&server)?.with_file_name("server-access.json"),
        &json!({"server":server,"token":token}),
    )?;
    Ok(json!({"configured":true,"server":server}))
}
fn client(server: &str) -> Result<reqwest::Client> {
    let path = account_file(server)?.with_file_name("server-access.json");
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(|_| {
        AppError::new(
            "server_token_required",
            "configure the instance token with lwc config server --server ORIGIN --token-stdin",
        )
    })?)
    .map_err(|_| AppError::new("credential_error", "invalid server token configuration"))?;
    let token = value["token"]
        .as_str()
        .filter(|v| {
            (v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                || crate::team::valid_personal_key(v)
        })
        .filter(|_| value["server"] == server)
        .ok_or_else(|| AppError::new("credential_error", "invalid server token configuration"))?;
    let mut header = reqwest::header::HeaderValue::from_str(token)
        .map_err(|_| AppError::new("credential_error", "invalid server token"))?;
    header.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-lwc-server-token", header);
    reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("lwc-replica")
        .build()
        .map_err(|_| AppError::new("http_client_error", "could not initialize HTTPS client"))
}
async fn request_json(request: reqwest::RequestBuilder) -> Result<Value> {
    let mut response = request
        .send()
        .await
        .map_err(|_| AppError::new("server_unavailable", "team server request failed"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::new("server_unavailable", "team server response failed"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(AppError::new(
                "invalid_response",
                "team server response exceeds limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::new("invalid_response", "team server returned invalid JSON"))?;
    if !status.is_success() {
        return Err(AppError::new(
            if status == 401 {
                "unauthorized"
            } else {
                "team_request_failed"
            },
            "team server rejected the request",
        )
        .with_details(json!({"http_status":status.as_u16(),"code":value["error"]["code"],"remote_details":value["error"]["details"]})));
    }
    Ok(value)
}

pub(crate) fn login(server: &str, name: &str) -> Result<Value> {
    let server = origin(server)?;
    let path = account_file(&server)?;
    // Prepare storage before approving a remote device so a permission failure
    // cannot silently leave a session that was never saved locally.
    crate::team::private_directory(path.parent().unwrap())?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async{
        let client=client(&server)?;
        let device=request_json(client.post(format!("{server}/api/auth/device/start")).json(&json!({"name":name}))).await?;
        let code=device["device_code"].as_str().filter(|v|v.len()==64).ok_or_else(||AppError::new("invalid_response","device code is missing"))?;
        let user_code=device["user_code"].as_str().filter(|v|v.len()==8 && v.bytes().all(|b|b.is_ascii_hexdigit())).ok_or_else(||AppError::new("invalid_response","user code is missing"))?;
        // Derive the destination from the explicitly selected origin, not server-supplied HTML/URLs.
        eprintln!("Open {server}/devices/authorize#device={user_code} and confirm this device.");
        let deadline=Instant::now()+Duration::from_secs(600);
        loop {
            if Instant::now()>=deadline {return Err(AppError::new("login_expired","device authorization expired; run login again"));}
            tokio::time::sleep(Duration::from_secs(5)).await;
            let result=request_json(client.post(format!("{server}/api/auth/device/poll")).json(&json!({"device_code":code}))).await?;
            match result["status"].as_str() {
                Some("authorization_pending"|"slow_down")=>continue,
                Some("authorized")=>{
                    let credentials:Credentials=serde_json::from_value(json!({"server":server,"user_id":result["user_id"],"access_token":result["access_token"]})).map_err(|_|AppError::new("invalid_response","authorization response is incomplete"))?;
                    if credentials.access_token.len()!=64 || !credentials.access_token.bytes().all(|b|b.is_ascii_hexdigit()) {return Err(AppError::new("invalid_response","invalid access token"));}
                    if let Err(error)=save_credentials(&path,&credentials) {
                        let _=request_json(client.post(format!("{server}/api/logout")).bearer_auth(&credentials.access_token)).await;
                        return Err(error);
                    }
                    return Ok(json!({"authenticated":true,"server":server,"user_id":credentials.user_id}));
                }
                _=>return Err(AppError::new("invalid_response","unknown device authorization status")),
            }
        }
    })
}

pub(crate) fn logout(server: &str) -> Result<Value> {
    let server = origin(server)?;
    let path = account_file(&server)?;
    let credentials: Credentials = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|_| AppError::new("credential_error", "invalid saved credentials"))?;
    if credentials.server != server {
        return Err(AppError::new(
            "credential_error",
            "saved server identity mismatch",
        ));
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            request_json(
                client(&server)?
                    .post(format!("{server}/api/logout"))
                    .bearer_auth(&credentials.access_token),
            )
            .await?;
            fs::remove_file(path)?;
            Ok(json!({"logged_out":true,"server":server}))
        })
}

/// Read cloud memory with existing credentials; no Store, replica, or worker is opened locally.
pub(crate) fn cloud_query(
    server: &str,
    space: &str,
    query: &crate::team::CloudQuery,
) -> Result<Value> {
    query.validate()?;
    if space.len() != 64
        || !space
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AppError::new("invalid_id", "expected a canonical space ID"));
    }
    let server = origin(server)?;
    let credentials = spaces::credentials_for(&server, None)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            request_json(
                client(&server)?
                    .post(format!("{server}/api/spaces/{space}/query"))
                    .bearer_auth(&credentials.access_token)
                    .json(query),
            )
            .await
        })
}

pub(crate) fn active_credential_hash(server: &str) -> Result<String> {
    let credentials = spaces::credentials_for(server, None)?;
    Ok(Sha256::digest(credentials.access_token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub(crate) fn recovery_json(server: &str, space: &str, input: &str) -> Result<Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if input == "-" {
        std::io::stdin().take(65537).read_to_end(&mut bytes)?;
    } else if let Some(path) = input.strip_prefix('@') {
        fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    } else {
        bytes = input.as_bytes().to_vec();
    }
    if bytes.len() > 65536 {
        return Err(AppError::new(
            "invalid_arguments",
            "recovery input exceeds 64 KiB",
        ));
    }
    let query: crate::team::RecoveryQuery = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::new("invalid_arguments", "invalid recovery request"))?;
    recovery_query(server, space, &query)
}
pub(crate) fn recovery_query(
    server: &str,
    space: &str,
    query: &crate::team::RecoveryQuery,
) -> Result<Value> {
    if !spaces::canonical_id(space) {
        return Err(AppError::new(
            "invalid_space",
            "expected a canonical space ID",
        ));
    }
    let server = origin(server)?;
    let credentials = spaces::credentials_for(&server, None)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            request_json(
                client(&server)?
                    .post(format!("{server}/api/spaces/{space}/recovery"))
                    .bearer_auth(&credentials.access_token)
                    .json(query),
            )
            .await
        })
}

pub(crate) fn login_key(server: &str) -> Result<Value> {
    use std::io::Read;
    let server = origin(server)?;
    let mut key = String::new();
    std::io::stdin().take(4097).read_to_string(&mut key)?;
    let key = key.trim();
    if !crate::team::valid_personal_key(key) {
        return Err(AppError::new(
            "invalid_personal_key",
            "expected a personal key on private stdin",
        ));
    }
    let response = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| {
                    AppError::new("http_client_error", "could not initialize HTTPS client")
                })?;
            request_json(
                client
                    .post(format!("{server}/api/auth/key"))
                    .json(&json!({"key":key,"cli":true})),
            )
            .await
        })?;
    let credentials:Credentials=serde_json::from_value(json!({"server":server,"user_id":response["user_id"],"access_token":response["access_token"]})).map_err(|_|AppError::new("invalid_response","invalid key login response"))?;
    if credentials.access_token.len() != 64
        || !credentials
            .access_token
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::new(
            "invalid_response",
            "invalid session credential",
        ));
    }
    let path = account_file(&server)?;
    save_credentials(
        &path.with_file_name("server-access.json"),
        &json!({"server":server,"token":key}),
    )?;
    save_credentials(&path, &credentials)?;
    Ok(json!({"authenticated":true,"server":server,"user_id":credentials.user_id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replica_credentials_are_private_and_origin_is_exact() {
        assert_eq!(
            origin("https://example.com/").unwrap(),
            "https://example.com"
        );
        for input in [
            "http://example.com",
            "https://user:secret@example.com/",
            "https://example.com/path",
            "https://example.com/?q=1",
        ] {
            assert!(origin(input).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private/credentials.json");
        let credentials = Credentials {
            agent_id: None,
            device_id: None,
            server: "https://example.com".into(),
            user_id: "user".into(),
            access_token: "a".repeat(64),
        };
        save_credentials(&path, &credentials).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        save_credentials(&path, &credentials).unwrap();
        assert_eq!(
            serde_json::from_slice::<Credentials>(&fs::read(path).unwrap())
                .unwrap()
                .access_token,
            credentials.access_token
        );
    }
}
