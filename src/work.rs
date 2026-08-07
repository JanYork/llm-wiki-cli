use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CURRENT_SCHEMA_VERSION: u32 = 11;
const RESUME_STALE_AFTER_MS: u128 = 30_000;
static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Deserialize, Serialize)]
struct WorkRequest {
    id: String,
    kind: String,
    scope: String,
    database: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct WorkState {
    id: String,
    kind: String,
    scope: String,
    database: PathBuf,
    state: String,
    phase: String,
    completed: u64,
    total: Option<u64>,
    percent: Option<f64>,
    sequence: u64,
    updated_at_unix_ms: u128,
    cancel_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl WorkState {
    fn queued(request: &WorkRequest) -> Self {
        Self {
            id: request.id.clone(),
            kind: request.kind.clone(),
            scope: request.scope.clone(),
            database: request.database.clone(),
            state: "queued".into(),
            phase: "queued".into(),
            completed: 0,
            total: None,
            percent: None,
            sequence: 1,
            updated_at_unix_ms: now_ms(),
            cancel_requested: false,
            pid: None,
            message: format!("{} queued", request.kind),
            result: None,
            error: None,
        }
    }

    fn update(&mut self, state: &str, phase: &str, message: impl Into<String>) {
        self.state = state.into();
        self.phase = phase.into();
        self.message = message.into();
        self.sequence += 1;
        self.updated_at_unix_ms = now_ms();
    }
}

pub fn schema_migration_needed(database: &Path) -> Result<bool> {
    let mut header = [0_u8; 100];
    let mut file = fs::File::open(database)?;
    if file.read_exact(&mut header).is_err() || &header[..16] != b"SQLite format 3\0" {
        return Ok(false);
    }
    let version = u32::from_be_bytes(header[60..64].try_into().unwrap());
    Ok(version == CURRENT_SCHEMA_VERSION - 1)
}

pub fn start_schema_migration(store: &StorePath) -> Result<Value> {
    start(store, "schema-migrate")
}

pub fn start_compact(store: &StorePath) -> Result<Value> {
    start(store, "maintenance-compact")
}

pub fn start_reindex(store: &StorePath) -> Result<Value> {
    start(store, "maintenance-reindex")
}

pub fn start_materialize(store: &StorePath) -> Result<Value> {
    start(store, "maintenance-materialize")
}

fn start(store: &StorePath, kind: &str) -> Result<Value> {
    let root = work_root(&store.path)?;
    ensure_root(&root)?;
    if let Some(state) = active_state(&root)? {
        if state.kind == kind && !terminal(&state.state) {
            return Ok(json!({"work": state}));
        }
        return Err(AppError::new(
            "work_busy",
            format!("work {} ({}) is already active", state.id, state.kind),
        ));
    }

    let request = WorkRequest {
        id: work_id(&store.path),
        kind: kind.into(),
        scope: scope_name(store.scope).into(),
        database: store.path.clone(),
    };
    let directory = root.join(&request.id);
    fs::create_dir(&directory)?;
    set_directory_mode(&directory)?;
    let state = WorkState::queued(&request);
    write_json(&directory.join("request.json"), &request)?;
    write_json(&directory.join("state.json"), &state)?;
    claim_active(&root, &request.id)?;

    if let Err(error) = spawn(&root, &request.id) {
        let mut failed = state.clone();
        failed.update("failed", "spawn", error.message.clone());
        failed.error = Some(json!({"code": error.code, "message": error.message}));
        write_json(&directory.join("state.json"), &failed)?;
        release_active(&root, &request.id)?;
        return Err(error);
    }
    Ok(json!({"work": state}))
}

pub fn run(root: &Path, id: &str) -> Result<Value> {
    validate_id(id)?;
    ensure_root(root)?;
    let directory = root.join(id);
    ensure_work_directory(&directory)?;
    let request: WorkRequest = read_json(&directory.join("request.json"))?;
    if request.id != id || work_root(&request.database)? != root {
        return Err(AppError::new(
            "work_invalid",
            "work request does not match its database scope",
        ));
    }
    let mut initial: WorkState = read_json(&directory.join("state.json"))?;
    initial.pid = Some(std::process::id());
    initial.update("running", "starting", format!("{} started", request.kind));
    write_json(&directory.join("state.json"), &initial)?;
    let state = Arc::new(Mutex::new(initial));
    let (stop_heartbeat, heartbeat_stop) = mpsc::channel();
    let heartbeat_state = Arc::clone(&state);
    let heartbeat_path = directory.join("state.json");
    let heartbeat_cancel = directory.join("cancel");
    let heartbeat = thread::spawn(move || {
        while let Err(mpsc::RecvTimeoutError::Timeout) =
            heartbeat_stop.recv_timeout(Duration::from_secs(5))
        {
            let Ok(mut state) = heartbeat_state.lock() else {
                break;
            };
            state.cancel_requested = heartbeat_cancel.exists();
            state.sequence += 1;
            state.updated_at_unix_ms = now_ms();
            let _ = write_json(&heartbeat_path, &*state);
        }
    });

    let cancel = directory.join("cancel");
    let mut progress = |completed: usize, total: usize, phase: &str| -> Result<()> {
        if cancel.exists() {
            return Err(AppError::new(
                "work_cancelled",
                format!("{} cancelled", request.kind),
            ));
        }
        let mut state = state
            .lock()
            .map_err(|_| AppError::new("work_invalid", "work state lock poisoned"))?;
        state.completed = completed as u64;
        state.total = Some(total as u64);
        state.percent = (total > 0).then(|| completed as f64 * 100.0 / total as f64);
        state.update(
            "running",
            phase,
            format!("{} {completed}/{total}", request.kind),
        );
        write_json(&directory.join("state.json"), &*state)
    };
    let result: Result<Value> = (|| match request.kind.as_str() {
        "schema-migrate" => {
            Store::open_with_migration_progress(&request.scope, &request.database, &mut progress)
                .map(|_| {
                    json!({
                        "database": request.database,
                        "schema_version": CURRENT_SCHEMA_VERSION
                    })
                })
        }
        "maintenance-compact" => {
            progress(0, 1, "opening")?;
            let mut store = Store::open(&request.scope, &request.database)?;
            progress(0, 1, "compacting")?;
            Ok(serde_json::to_value(store.compact()?).map_err(|error| {
                AppError::new(
                    "work_invalid",
                    format!("cannot encode compact result: {error}"),
                )
            })?)
        }
        "maintenance-reindex" => {
            progress(0, 2, "opening")?;
            let mut store = Store::open(&request.scope, &request.database)?;
            progress(0, 2, "reindexing")?;
            let response = store.reindex()?;
            progress(1, 2, "materializing")?;
            store.materialize_wiki()?;
            Ok(serde_json::to_value(response).map_err(|error| {
                AppError::new(
                    "work_invalid",
                    format!("cannot encode reindex result: {error}"),
                )
            })?)
        }
        "maintenance-materialize" => {
            progress(0, 1, "opening")?;
            let mut store = Store::open(&request.scope, &request.database)?;
            progress(0, 1, "materializing")?;
            Ok(serde_json::to_value(store.materialize()?).map_err(|error| {
                AppError::new(
                    "work_invalid",
                    format!("cannot encode materialize result: {error}"),
                )
            })?)
        }
        other => Err(AppError::new(
            "work_invalid",
            format!("unsupported work kind: {other}"),
        )),
    })();
    let _ = stop_heartbeat.send(());
    let _ = heartbeat.join();
    let mut state = state
        .lock()
        .map_err(|_| AppError::new("work_invalid", "work state lock poisoned"))?;
    match result {
        Ok(result) => {
            state.completed = 1;
            state.total = Some(1);
            state.percent = Some(100.0);
            state.result = Some(result);
            state.update(
                "succeeded",
                "complete",
                format!("{} complete", request.kind),
            );
        }
        Err(error) if error.code == "work_cancelled" => {
            state.cancel_requested = true;
            state.error = Some(json!({"code": error.code, "message": error.message}));
            state.update(
                "cancelled",
                "cancelled",
                format!("{} cancelled", request.kind),
            );
        }
        Err(error) => {
            state.error = Some(json!({"code": error.code, "message": error.message}));
            state.update("failed", "failed", format!("{} failed", request.kind));
        }
    }
    state.pid = None;
    write_json(&directory.join("state.json"), &*state)?;
    release_active(root, id)?;
    Ok(json!({"work": &*state}))
}

pub fn list(store: &StorePath) -> Result<Value> {
    let root = work_root(&store.path)?;
    if !root.exists() {
        return Ok(json!({"works": []}));
    }
    ensure_root(&root)?;
    let mut states = fs::read_dir(&root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| read_json::<WorkState>(&entry.path().join("state.json")).ok())
        .collect::<Vec<_>>();
    states.sort_by(|left, right| right.updated_at_unix_ms.cmp(&left.updated_at_unix_ms));
    Ok(json!({"works": states}))
}

pub fn status(store: &StorePath, id: &str) -> Result<Value> {
    let state = load_state(store, id)?;
    Ok(json!({"work": state}))
}

pub fn watch(store: &StorePath, id: &str) -> Result<Value> {
    loop {
        let state = load_state(store, id)?;
        if terminal(&state.state) {
            return Ok(json!({"work": state}));
        }
        thread::sleep(Duration::from_millis(250));
    }
}

pub fn cancel(store: &StorePath, id: &str) -> Result<Value> {
    validate_id(id)?;
    let root = work_root(&store.path)?;
    ensure_root(&root)?;
    let directory = root.join(id);
    ensure_work_directory(&directory)?;
    let mut state: WorkState = read_json(&directory.join("state.json"))?;
    if !terminal(&state.state) {
        let cancel = directory.join("cancel");
        if !cancel.exists() {
            write_bytes(&cancel, b"cancel\n")?;
        }
        state.cancel_requested = true;
        state.update(
            &state.state.clone(),
            &state.phase.clone(),
            "cancellation requested",
        );
        write_json(&directory.join("state.json"), &state)?;
    }
    Ok(json!({"work": state}))
}

pub fn resume(store: &StorePath, id: &str) -> Result<Value> {
    validate_id(id)?;
    let root = work_root(&store.path)?;
    ensure_root(&root)?;
    let directory = root.join(id);
    ensure_work_directory(&directory)?;
    let mut state: WorkState = read_json(&directory.join("state.json"))?;
    if state.state == "succeeded"
        || (!terminal(&state.state)
            && now_ms().saturating_sub(state.updated_at_unix_ms) < RESUME_STALE_AFTER_MS)
    {
        return Err(AppError::new(
            "work_not_resumable",
            format!("work {id} is {} and cannot be resumed", state.state),
        ));
    }
    release_active(&root, id)?;
    match fs::remove_file(directory.join("cancel")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    state.cancel_requested = false;
    state.pid = None;
    state.error = None;
    let message = format!("{} queued for resume", state.kind);
    state.update("queued", "queued", message);
    write_json(&directory.join("state.json"), &state)?;
    claim_active(&root, id)?;
    if let Err(error) = spawn(&root, id) {
        release_active(&root, id)?;
        return Err(error);
    }
    Ok(json!({"work": state}))
}

fn load_state(store: &StorePath, id: &str) -> Result<WorkState> {
    validate_id(id)?;
    let root = work_root(&store.path)?;
    ensure_root(&root)?;
    let directory = root.join(id);
    ensure_work_directory(&directory)?;
    read_json(&directory.join("state.json"))
}

fn active_state(root: &Path) -> Result<Option<WorkState>> {
    let active = root.join("active");
    let id = match fs::read_to_string(&active) {
        Ok(value) => value.trim().to_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_id(&id)?;
    let directory = root.join(&id);
    ensure_work_directory(&directory)?;
    let state: WorkState = read_json(&directory.join("state.json"))?;
    if terminal(&state.state) {
        release_active(root, &id)?;
        return Ok(None);
    }
    Ok(Some(state))
}

fn claim_active(root: &Path, id: &str) -> Result<()> {
    let path = root.join("active");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&path).map_err(|error| {
        AppError::new(
            "work_busy",
            format!("another state-changing work is active: {error}"),
        )
    })?;
    file.write_all(id.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn spawn(root: &Path, id: &str) -> Result<()> {
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("__work-run")
        .arg("--root")
        .arg(root)
        .arg("--id")
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0800_0200);
    command.spawn().map(|_| ()).map_err(|error| {
        AppError::new(
            "work_spawn_failed",
            format!("failed to start work: {error}"),
        )
    })
}

fn release_active(root: &Path, id: &str) -> Result<()> {
    let path = root.join("active");
    match fs::read_to_string(&path) {
        Ok(value) if value.trim() == id => fs::remove_file(path)?,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn work_root(database: &Path) -> Result<PathBuf> {
    let parent = database
        .parent()
        .ok_or_else(|| AppError::new("work_invalid", "wiki database has no parent directory"))?;
    Ok(parent.join("work"))
}

fn ensure_root(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AppError::new(
                "work_invalid",
                format!("work root is not a real directory: {}", root.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(root)?;
            set_directory_mode(root)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn ensure_work_directory(directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| {
        AppError::new(
            "work_not_found",
            format!(
                "cannot inspect work directory {}: {error}",
                directory.display()
            ),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::new(
            "work_invalid",
            format!("work path is not a real directory: {}", directory.display()),
        ));
    }
    Ok(())
}

fn set_directory_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(|error| {
            AppError::new("work_invalid", format!("cannot encode work state: {error}"))
        })?,
    )
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::new(
            "work_invalid",
            format!("work state is a symlink: {}", path.display()),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        AppError::new(
            "work_not_found",
            format!("cannot read work state {}: {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        AppError::new(
            "work_invalid",
            format!("invalid work state {}: {error}", path.display()),
        )
    })
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            "work_invalid",
            "work ID must be 64 hex characters",
        ));
    }
    Ok(())
}

fn work_id(database: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(database.as_os_str().as_encoded_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(now_ms().to_be_bytes());
    hasher.update(WORK_COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn terminal(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "cancelled")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

    #[test]
    fn schema_preflight_does_not_scan_a_416_mib_wal() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("wiki.db");
        let mut header = [0_u8; 100];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        header[60..64].copy_from_slice(&10_u32.to_be_bytes());
        fs::write(&database, header).unwrap();
        fs::File::create(temp.path().join("wiki.db-wal"))
            .unwrap()
            .set_len(416 * 1024 * 1024)
            .unwrap();

        let started = Instant::now();
        assert!(schema_migration_needed(&database).unwrap());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "schema preflight must read only the SQLite header"
        );
    }

    #[cfg(unix)]
    #[test]
    fn work_commands_reject_a_symlinked_work_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let store_dir = temp.path().join(".lwc");
        let outside = temp.path().join("outside");
        fs::create_dir(&store_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        let database = store_dir.join("wiki.db");
        fs::write(&database, b"placeholder").unwrap();
        symlink(&outside, store_dir.join("work")).unwrap();

        let store = StorePath::new(Scope::Project, database);
        let error = list(&store).unwrap_err();
        assert_eq!(error.code, "work_invalid");
    }
}
