use super::*;
use crate::store::{Store, StoreIdentity, SyncTransferSummary};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpaceRecord {
    pub version: u32,
    #[serde(default)]
    pub server_key: Option<String>,
    pub server: String,
    pub space_id: String,
    pub user_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub device_id: String,
    pub replica_id: Option<String>,
    pub role: String,
    pub joined: bool,
    #[serde(default)]
    pub automatic: bool,
    pub interval_ms: u64,
    pub max_transfer_bytes: u64,
    pub remote_head: Option<Value>,
    pub baseline_generation: Option<String>,
    #[serde(default)]
    pub reconciling: bool,
    pub acknowledged_identity: Option<StoreIdentity>,
    #[serde(default)]
    pub acknowledged_auxiliary: Option<String>,
    #[serde(default)]
    pub projection_pending: bool,
}

pub(super) fn canonical_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| AppError::new("entropy_unavailable", "secure randomness unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub(super) fn sync_lock(directory: &Path) -> Result<fs::File> {
    let path = directory.join("sync.lock");
    if path.try_exists()? && fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_replica_path",
            "sync lock cannot be a symlink",
        ));
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock().map_err(|_| {
        AppError::new(
            "replica_busy",
            "another sync operation is active for this replica",
        )
    })?;
    Ok(file)
}

pub(super) fn credentials_for(server: &str, user: Option<&str>) -> Result<Credentials> {
    let path = if let Some(path) = std::env::var_os("LWC_TEAM_CREDENTIALS_FILE") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(AppError::new(
                "credential_error",
                "delegated credential path must be absolute",
            ));
        }
        path
    } else {
        account_file(server)?
    };
    let credentials: Credentials = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| AppError::new("credential_error", "invalid saved credentials"))?;
    if credentials.server != server || user.is_some_and(|user| user != credentials.user_id) {
        return Err(AppError::new(
            "replica_account_changed",
            "saved session belongs to another server or user; local work is preserved",
        ));
    }
    Ok(credentials)
}
fn selected_principal() -> Result<Option<Credentials>> {
    let Some(path) = std::env::var_os("LWC_TEAM_CREDENTIALS_FILE") else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::new(
            "credential_error",
            "credential override must be absolute",
        ));
    }
    let credentials: Credentials = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| AppError::new("credential_error", "invalid delegated credentials"))?;
    if credentials
        .agent_id
        .as_deref()
        .is_some_and(|id| !canonical_id(id))
    {
        return Err(AppError::new("credential_error", "invalid Agent identity"));
    }
    Ok(Some(credentials))
}
fn storage_root(server: &str, agent: Option<&str>) -> Result<PathBuf> {
    let account = account_file(server)?.parent().unwrap().to_owned();
    match agent {
        Some(id) if canonical_id(id) => Ok(account.join("agents").join(id)),
        Some(_) => Err(AppError::new("credential_error", "invalid Agent identity")),
        None => Ok(account),
    }
}
pub(super) fn read_record(path: &Path) -> Result<SpaceRecord> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_replica_path",
            "replica configuration cannot be a symlink",
        ));
    }
    let record: SpaceRecord = serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| AppError::new("invalid_replica", "invalid replica configuration"))?;
    if record.version != 1
        || !canonical_id(&record.space_id)
        || !canonical_id(&record.device_id)
        || !canonical_id(&record.user_id)
        || record
            .replica_id
            .as_deref()
            .is_some_and(|v| !canonical_id(v))
        || record
            .baseline_generation
            .as_deref()
            .is_some_and(|v| !canonical_id(v))
        || !matches!(record.role.as_str(), "viewer" | "editor" | "manager")
        || record.interval_ms < 250
        || record.max_transfer_bytes == 0
        || origin(&record.server)? != record.server
    {
        return Err(AppError::new(
            "invalid_replica",
            "unsupported replica version or invalid binding fields",
        ));
    }
    let principal = selected_principal()?;
    if principal.as_ref().and_then(|p| p.agent_id.as_ref()) != record.agent_id.as_ref() {
        return Err(AppError::new(
            "replica_principal_changed",
            "replica belongs to a different local principal",
        ));
    }
    let expected = storage_root(&record.server, record.agent_id.as_deref())?
        .join("spaces")
        .join(&record.space_id)
        .join("replica.json");
    if path != expected {
        return Err(AppError::new(
            "invalid_replica",
            "replica configuration is outside its bound server/space directory",
        ));
    }
    Ok(record)
}
pub(super) fn records() -> Result<Vec<(PathBuf, SpaceRecord)>> {
    let accounts = crate::scope::global_lwc_root()?.join("team/accounts");
    let mut output = vec![];
    if !accounts.exists() {
        return Ok(output);
    }
    let principal = selected_principal()?;
    let selected_account = principal
        .as_ref()
        .map(|p| account_file(&p.server))
        .transpose()?
        .map(|p| p.parent().unwrap().to_owned());
    for account in fs::read_dir(accounts)? {
        let account = account?;
        if !account.file_type()?.is_dir() || !canonical_id(&account.file_name().to_string_lossy()) {
            continue;
        }
        if selected_account
            .as_ref()
            .is_some_and(|selected| selected != &account.path())
        {
            continue;
        }
        let root = match principal.as_ref().and_then(|p| p.agent_id.as_ref()) {
            Some(agent) => account.path().join("agents").join(agent),
            None => account.path(),
        };
        let spaces = root.join("spaces");
        if !spaces.is_dir() {
            continue;
        }
        for space in fs::read_dir(spaces)? {
            let space = space?;
            if !space.file_type()?.is_dir() || !canonical_id(&space.file_name().to_string_lossy()) {
                continue;
            }
            let path = space.path().join("replica.json");
            if path.is_file() {
                output.push((space.path(), read_record(&path)?));
            }
        }
    }
    output.sort_by(|(_, a), (_, b)| (&a.server, &a.space_id).cmp(&(&b.server, &b.space_id)));
    Ok(output)
}
pub(super) fn reference(_directory: &Path, record: &SpaceRecord) -> String {
    let account: String = Sha256::digest(record.server.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    match record.agent_id.as_deref() {
        Some(agent) => format!("{account}/agents/{agent}/{}", record.space_id),
        None => format!("{account}/{}", record.space_id),
    }
}
pub(super) fn resolve(reference_id: &str) -> Result<(PathBuf, SpaceRecord)> {
    let matches = records()?
        .into_iter()
        .filter(|(directory, record)| {
            record.space_id == reference_id || reference(directory, record) == reference_id
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => Err(AppError::new(
            "space_not_joined",
            "join the space before selecting it",
        )),
        _ => Err(AppError::new(
            "space_ambiguous",
            "use the complete space reference returned by space list",
        )),
    }
}
pub(crate) fn selected_database(space: &str) -> Result<PathBuf> {
    let (directory, record) = resolve(space)?;
    if !record.joined {
        return Err(AppError::new(
            "space_not_joined",
            "space join is incomplete; resume join first",
        ));
    }
    Ok(directory.join("wiki.db"))
}
pub(crate) fn list_spaces() -> Result<Value> {
    Ok(
        json!({"spaces":records()?.into_iter().map(|(directory,record)|json!({"reference":reference(&directory,&record),"server":record.server,"space_id":record.space_id,"role":record.role,"joined":record.joined,"database":directory.join("wiki.db"),"head":record.remote_head})).collect::<Vec<_>>()}),
    )
}
pub(crate) fn show_space(space: &str) -> Result<Value> {
    let (directory, record) = resolve(space)?;
    let report = directory.join("worker.json");
    let report = if report.is_file() {
        serde_json::from_slice::<Value>(&fs::read(report)?)
            .map_err(|_| AppError::new("invalid_replica", "invalid worker report"))?
    } else {
        Value::Null
    };
    let signal = super::engine::conflict_signal(&reference(&directory, &record))?;
    Ok(
        json!({"reference":reference(&directory,&record),"database":directory.join("wiki.db"),"replica":record,"last_worker_report":report,"pending_signal":signal}),
    )
}

pub(super) async fn download_snapshot(
    client: &reqwest::Client,
    record: &SpaceRecord,
    credentials: &Credentials,
    directory: &Path,
    baseline: Option<(&str, &Path)>,
) -> Result<(PathBuf, Value)> {
    let mut request = client
        .get(format!(
            "{}/api/spaces/{}/pull",
            record.server, record.space_id
        ))
        .bearer_auth(&credentials.access_token)
        .timeout(Duration::from_secs(600));
    if let Some((artifact, _)) = baseline {
        request = request.query(&[("baseline_artifact", artifact)]);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| AppError::new("server_unavailable", "snapshot download failed"))?;
    if !response.status().is_success() {
        return Err(AppError::new(
            if response.status() == 403 {
                "forbidden"
            } else if response.status() == 401 {
                "unauthorized"
            } else {
                "server_unavailable"
            },
            "snapshot download was refused; local work is preserved",
        ));
    }
    let transfer: SyncTransferSummary = response
        .headers()
        .get("x-lwc-transfer")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| AppError::new("invalid_transfer", "missing transfer metadata"))?;
    let head: Value = response
        .headers()
        .get("x-lwc-head")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str(value).ok())
        .ok_or_else(|| AppError::new("invalid_transfer", "missing head metadata"))?;
    super::engine::verify_head(record, &head)?;
    if head["protocol"] != "lwc-team-sync/1"
        || head["share_schema"] != 1
        || head["space_id"] != record.space_id
        || head["digest"] != transfer.state_digest
        || head["head"].as_u64().is_none()
        || !head["server_epoch"].as_str().is_some_and(canonical_id)
        || !head["artifact_id"].as_str().is_some_and(canonical_id)
        || transfer.size > record.max_transfer_bytes
    {
        return Err(AppError::new(
            "invalid_transfer",
            "snapshot identity, version, or size is invalid",
        ));
    }
    let download = directory.join("download.bin");
    let normalized = directory.join("remote.db");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&download)
        .await?;
    let mut bytes = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AppError::new("download_interrupted", "snapshot download was interrupted"))?
    {
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| AppError::new("artifact_too_large", "download exceeded size limit"))?;
        if bytes > transfer.size {
            return Err(AppError::new(
                "artifact_too_large",
                "download exceeded declared size",
            ));
        }
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    if bytes != transfer.size {
        return Err(AppError::new(
            "download_incomplete",
            "snapshot size differs from transfer metadata",
        ));
    }
    crate::store::apply_sync_transfer_artifact(
        baseline.map(|(_, path)| path),
        &download,
        &transfer,
        &normalized,
    )?;
    Ok((normalized, head))
}

pub(super) fn save_baseline(
    directory: &Path,
    record: &mut SpaceRecord,
    local: &Path,
    remote: &Path,
    head: Value,
    identity: StoreIdentity,
) -> Result<()> {
    let generation = random_id()?;
    let root = directory.join("generations").join(&generation);
    fs::create_dir_all(&root)?;
    for (source, name) in [(local, "local.db"), (remote, "remote.db")] {
        let path = root.join(name);
        fs::copy(source, &path)?;
        fs::File::open(path)?.sync_all()?;
    }
    #[cfg(unix)]
    fs::File::open(&root)?.sync_all()?;
    record.baseline_generation = Some(generation);
    record.remote_head = Some(head);
    record.reconciling = false;
    record.acknowledged_identity = Some(identity);
    record.projection_pending = true;
    record.joined = true;
    save_credentials(&directory.join("replica.json"), record)
}

pub(crate) fn join_space(
    server: &str,
    space: &str,
    device: &str,
    automatic: bool,
) -> Result<Value> {
    let server = origin(server)?;
    if !canonical_id(space) {
        return Err(AppError::new("invalid_space", "space ID must be canonical"));
    }
    let credentials = credentials_for(&server, None)?;
    let directory = storage_root(&server, credentials.agent_id.as_deref())?
        .join("spaces")
        .join(space);
    crate::team::private_directory(&directory)?;
    let _lock = sync_lock(&directory)?;
    let path = directory.join("replica.json");
    let mut record = if path.exists() {
        read_record(&path)?
    } else {
        SpaceRecord {
            version: 1,
            server_key: None,
            server: server.clone(),
            space_id: space.into(),
            user_id: credentials.user_id.clone(),
            agent_id: credentials.agent_id.clone(),
            device_id: credentials.device_id.clone().unwrap_or(random_id()?),
            replica_id: None,
            role: "viewer".into(),
            joined: false,
            automatic,
            interval_ms: 2000,
            max_transfer_bytes: 2 * 1024 * 1024 * 1024,
            remote_head: None,
            baseline_generation: None,
            reconciling: false,
            acknowledged_identity: None,
            acknowledged_auxiliary: None,
            projection_pending: true,
        }
    };
    if record.user_id != credentials.user_id {
        return Err(AppError::new(
            "replica_account_changed",
            "space belongs to another local account; local work is preserved",
        ));
    }
    record.automatic = automatic;
    save_credentials(&path, &record)?;
    if record.joined {
        return show_space(&reference(&directory, &record));
    }
    save_credentials(&path, &record)?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
        let client=client(&server)?;
        let registered=request_json(client.post(format!("{server}/api/spaces/{space}/replicas")).bearer_auth(&credentials.access_token).json(&json!({"device":device,"device_id":record.device_id}))).await?;
        let replica=registered["replica_id"].as_str().filter(|id|canonical_id(id)).ok_or_else(||AppError::new("invalid_response","invalid replica registration"))?;
        let role=registered["role"].as_str().filter(|role|matches!(*role,"viewer"|"editor"|"manager")).ok_or_else(||AppError::new("invalid_response","invalid replica role"))?;
        let key=registered["head"]["manifest"]["public_key"].as_str().ok_or_else(||AppError::new("invalid_manifest","missing server signing key"))?;
        if record.server_key.as_deref().is_some_and(|pinned|pinned!=key){return Err(AppError::new("policy_key_changed","server signing key changed"));}
        record.server_key=Some(key.into());
        super::engine::verify_head(&record,&registered["head"])?;
        record.replica_id=Some(replica.into());record.role=role.into();save_credentials(&path,&record)?;
        let staging=directory.join("staging").join(random_id()?);fs::create_dir_all(&staging)?;
        let (remote,head)=download_snapshot(&client,&record,&credentials,&staging,None).await?;
        let (mut store,_)=Store::initialize("project",directory.join("wiki.db"))?;
        store.bind_team_space(space,head["server_epoch"].as_str().unwrap())?;
        store.publish_sync_state(&remote,&store.identity()?,&format!("join:{}",record.device_id))?;
        let identity=store.identity()?;
        record.role=head["role"].as_str().filter(|role|matches!(*role,"viewer"|"editor"|"manager")).ok_or_else(||AppError::new("invalid_response","invalid current space role"))?.to_owned();
        store.set_replica_policy(&registered["head"]["policy"],&record.user_id,&record.server)?;
        save_baseline(&directory,&mut record,&remote,&remote,head.clone(),identity)?;
        let projection=match super::engine::project(&directory,&mut record,&mut store) {Ok(_)=>json!({"status":"ready"}),Err(error)=>json!({"status":"pending","error":error.code})};
        let acknowledged=request_json(client.post(format!("{server}/api/spaces/{space}/ack")).bearer_auth(&credentials.access_token).json(&json!({"replica_id":record.replica_id,"server_epoch":head["server_epoch"],"head":head["head"]}))).await.is_ok();
        Ok(json!({"joined":true,"reference":reference(&directory,&record),"database":directory.join("wiki.db"),"role":record.role,"acknowledged":acknowledged,"head":head,"projection":projection}))
    })
}

fn bindings_file() -> Result<PathBuf> {
    let root = crate::scope::global_lwc_root()?.join("team/bindings");
    Ok(match selected_principal()?.and_then(|p| p.agent_id) {
        Some(agent) => root.join(agent).join("projects.json"),
        None => root.join("projects.json"),
    })
}
fn bindings() -> Result<std::collections::BTreeMap<String, String>> {
    let path = bindings_file()?;
    if !path.try_exists()? {
        return Ok(Default::default());
    }
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "invalid_binding",
            "project bindings cannot be a symlink",
        ));
    }
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| AppError::new("invalid_binding", "invalid project-space bindings"))
}
pub(crate) fn project_binding(project: &Path) -> Result<Option<String>> {
    let project = project.canonicalize()?;
    let values = match bindings() {
        Ok(values) => values,
        Err(error) if error.code == "home_not_set" => return Ok(None),
        Err(error) => return Err(error),
    };
    for parent in project.ancestors() {
        if let Some(reference) = values.get(&parent.to_string_lossy().to_string()) {
            return Ok(Some(reference.clone()));
        }
    }
    Ok(None)
}
pub(crate) fn bind_project(project: &Path, space: Option<&str>, import: bool) -> Result<Value> {
    let project = project.canonicalize()?;
    let path = bindings_file()?;
    crate::team::private_directory(path.parent().unwrap())?;
    let _binding_lock = sync_lock(path.parent().unwrap())?;
    let mut values = bindings()?;
    let key = project.to_string_lossy().to_string();
    let Some(space) = space else {
        values.remove(&key);
        save_credentials(&bindings_file()?, &values)?;
        return Ok(json!({"bound":false,"project":project}));
    };
    let (directory, record) = resolve(space)?;
    if !record.joined {
        return Err(AppError::new(
            "invalid_replica",
            "join the space before binding a project",
        ));
    }
    let reference = reference(&directory, &record);
    if import {
        // Reuse the full-core archive merge and its durable conflict/CAS recovery protocol.
        // Original project memory stays intact; only an explicit import makes it shared.
        let _unselected = crate::scope::select_space(None);
        let archive = directory.join(format!("project-import-{}.lwc.zst", random_id()?));
        crate::archive::compress(&project, crate::scope::Scope::Project, Some(&archive))?;
        let _selected = crate::scope::select_space(Some(directory.join("wiki.db")));
        let result = crate::archive::merge(
            &project,
            crate::scope::Scope::Project,
            Some(&archive),
            None,
            None,
        )?;
        if result["committed"] != true && result["action"] != "unchanged" {
            return Ok(
                json!({"bound":false,"space":reference,"import":result,"instruction":"Resolve the preserved archive import with lwc --space SPACE merge --resume SESSION --resolve FILE; then bind without --import-project. Do not discard original project memory."}),
            );
        }
    }
    values.insert(key, reference.clone());
    save_credentials(&bindings_file()?, &values)?;
    start_worker(&reference)?;
    Ok(
        json!({"bound":true,"project":project,"space":reference,"imported":import,"original_project_memory_preserved":true}),
    )
}
