use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath},
    store::Store,
};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, Read, Write},
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

const CURRENT_SCHEMA_VERSION: u32 = 12;
const RESUME_STALE_AFTER_MS: u128 = 30_000;
const STATE_REPLACE_RETRIES: usize = 200;
const STATE_REPLACE_RETRY_DELAY: Duration = Duration::from_millis(5);
static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
const GRAPH_PENDING_FILE: &str = "graph-pending.json";
const GRAPH_PENDING_LOCK: &str = "graph-pending.lock";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    items_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    eta_seconds: Option<u64>,
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
            started_at_unix_ms: None,
            items_per_second: None,
            eta_seconds: None,
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
    Ok(matches!(version, 10 | 11))
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

pub fn start_graph_projection(scope: &str, database: &Path) -> Result<Value> {
    let documents = crate::external_graph::projection_keys(scope, database)?;
    start_graph_documents(scope, database, &documents)
}

pub fn start_graph_documents(
    scope: &str,
    database: &Path,
    documents: &[(String, String)],
) -> Result<Value> {
    let root = work_root(database)?;
    ensure_root(&root)?;
    append_graph_pending(&root, documents)?;
    if let Some(state) = active_state(&root)?
        && state.kind == "graph-project"
        && !terminal(&state.state)
    {
        return Ok(json!({"work": state}));
    }
    if documents.is_empty() {
        return Ok(json!({"work": null}));
    }
    match start_at_with_options(scope, database, "graph-project") {
        Ok(work) => Ok(work),
        Err(error) if error.code == "work_busy" => {
            for _ in 0..=STATE_REPLACE_RETRIES {
                if let Some(state) = active_state(&root)?
                    && state.kind == "graph-project"
                    && !terminal(&state.state)
                {
                    return Ok(json!({"work": state}));
                }
                thread::sleep(STATE_REPLACE_RETRY_DELAY);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

struct GraphPendingLock(PathBuf);

impl Drop for GraphPendingLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn lock_graph_pending(root: &Path) -> Result<GraphPendingLock> {
    let path = root.join(GRAPH_PENDING_LOCK);
    for attempt in 0..=STATE_REPLACE_RETRIES {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(_) => return Ok(GraphPendingLock(path)),
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    && attempt < STATE_REPLACE_RETRIES =>
            {
                thread::sleep(STATE_REPLACE_RETRY_DELAY);
            }
            Err(error) => {
                return Err(AppError::new(
                    "work_busy",
                    format!("graph document queue is busy: {error}"),
                ));
            }
        }
    }
    unreachable!("graph queue lock loop always returns")
}

fn append_graph_pending(root: &Path, documents: &[(String, String)]) -> Result<()> {
    let _lock = lock_graph_pending(root)?;
    let path = root.join(GRAPH_PENDING_FILE);
    let mut pending = match fs::metadata(&path) {
        Ok(_) => read_json::<BTreeSet<(String, String)>>(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
        Err(error) => return Err(error.into()),
    };
    pending.extend(documents.iter().cloned());
    write_json(&path, &pending)
}

fn drain_graph_pending(root: &Path) -> Result<Vec<(String, String)>> {
    let _lock = lock_graph_pending(root)?;
    let path = root.join(GRAPH_PENDING_FILE);
    let pending = match fs::metadata(&path) {
        Ok(_) => read_json::<BTreeSet<(String, String)>>(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
        Err(error) => return Err(error.into()),
    };
    write_json(&path, &BTreeSet::<(String, String)>::new())?;
    Ok(pending.into_iter().collect())
}

fn has_graph_pending(root: &Path) -> Result<bool> {
    let _lock = lock_graph_pending(root)?;
    let path = root.join(GRAPH_PENDING_FILE);
    match fs::metadata(&path) {
        Ok(_) => Ok(!read_json::<BTreeSet<(String, String)>>(&path)?.is_empty()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn run_graph_projection(
    request: &WorkRequest,
    root: &Path,
    progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
) -> Result<Value> {
    let mut completed = 0;
    let mut result = json!({"status": "ready", "documents": 0});
    loop {
        let documents = drain_graph_pending(root)?;
        if documents.is_empty() {
            break;
        }
        let base = completed;
        let mut batch_completed = 0;
        let batch = crate::external_graph::project_documents(
            &request.scope,
            &request.database,
            Some(&documents),
            &mut |done, total, phase| {
                batch_completed = done;
                progress(base + done, base + total, phase)?;
                if done > 0
                    && env::var("LWC_TEST_GRAPH_FAIL_AFTER_DOCUMENTS")
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok())
                        == Some(done)
                    && fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(root.join("graph-test-failure-injected"))
                        .is_ok()
                {
                    return Err(AppError::new(
                        "graph_test_failure",
                        "injected graph projection failure",
                    ));
                }
                Ok(())
            },
        );
        result = match batch {
            Ok(result) => result,
            Err(error) => {
                append_graph_pending(root, &documents[batch_completed..])?;
                return Err(error);
            }
        };
        completed += documents.len();
    }
    Ok(result)
}

fn start(store: &StorePath, kind: &str) -> Result<Value> {
    start_at(scope_name(store.scope), &store.path, kind)
}

fn start_at(scope: &str, database: &Path, kind: &str) -> Result<Value> {
    start_at_with_options(scope, database, kind)
}

fn start_at_with_options(scope: &str, database: &Path, kind: &str) -> Result<Value> {
    let root = work_root(database)?;
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
        id: work_id(database),
        kind: kind.into(),
        scope: scope.into(),
        database: database.to_path_buf(),
    };
    let directory = root.join(&request.id);
    fs::create_dir(&directory)?;
    set_directory_mode(&directory)?;
    let state = WorkState::queued(&request);
    write_json(&directory.join("request.json"), &request)?;
    write_json(&directory.join("state.json"), &state)?;
    if let Err(error) = claim_active(&root, &request.id) {
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }

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
    initial.started_at_unix_ms = Some(now_ms());
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
        let elapsed_ms = state
            .started_at_unix_ms
            .map(|started| now_ms().saturating_sub(started))
            .unwrap_or_default();
        state.items_per_second = (completed > 0 && elapsed_ms > 0)
            .then(|| completed as f64 * 1_000.0 / elapsed_ms as f64);
        state.eta_seconds = state.items_per_second.and_then(|rate| {
            (rate > 0.0).then(|| ((total.saturating_sub(completed)) as f64 / rate).ceil() as u64)
        });
        state.update(
            "running",
            phase,
            format!("{} {completed}/{total}", request.kind),
        );
        write_json(&directory.join("state.json"), &*state)
    };
    let result: Result<Value> = (|| match request.kind.as_str() {
        "schema-migrate" => migrate_schema_shadow(&request, &directory, &mut progress),
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
        "graph-project" => run_graph_projection(&request, root, &mut progress),
        other => Err(AppError::new(
            "work_invalid",
            format!("unsupported work kind: {other}"),
        )),
    })();
    let continue_graph_projection = result.is_ok()
        && crate::config::resolve(&request.scope, &request.database)
            .is_ok_and(|config| config.setting != crate::config::GraphSetting::Disabled);
    let _ = stop_heartbeat.send(());
    let _ = heartbeat.join();
    let mut state = state
        .lock()
        .map_err(|_| AppError::new("work_invalid", "work state lock poisoned"))?;
    match result {
        Ok(result) => {
            if state.total.is_none() {
                state.completed = 1;
                state.total = Some(1);
            }
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
    if request.kind == "graph-project"
        && continue_graph_projection
        && has_graph_pending(root).unwrap_or(false)
    {
        let _ = start_at_with_options(&request.scope, &request.database, "graph-project");
    }
    if request.kind != "graph-project"
        && let Ok(mut store) = Store::open(&request.scope, &request.database)
    {
        let _ = store.reconcile_graph_projection();
    }
    Ok(json!({"work": &*state}))
}

fn migrate_schema_shadow(
    request: &WorkRequest,
    directory: &Path,
    progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
) -> Result<Value> {
    let pending = directory.join("migrated.db");
    remove_sqlite_family(&pending)?;
    progress(0, 1, "snapshotting-live")?;
    let source = Connection::open_with_flags(&request.database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(2))?;
    let base_revision: String = source.query_row(
        "SELECT value FROM meta WHERE key = 'store_revision'",
        [],
        |row| row.get(0),
    )?;
    let mut destination = Connection::open(&pending)?;
    {
        let backup = Backup::new(&source, &mut destination)?;
        backup.run_to_completion(256, Duration::from_millis(2), None)?;
    }
    drop(destination);
    drop(source);
    progress(1, 1, "snapshotting-live")?;

    let signal_indexing = env::var_os("LWC_TEST_MIGRATION_READY").is_some();
    let indexing_ready = directory.join("migration-indexing-ready");
    let mut migration_progress = |completed: usize, total: usize, phase: &str| {
        progress(completed, total, phase)?;
        if signal_indexing && phase == "indexing" && !indexing_ready.exists() {
            write_bytes(&indexing_ready, b"ready\n")?;
        }
        Ok(())
    };
    let migrated =
        Store::open_with_migration_progress(&request.scope, &pending, &mut migration_progress)?;
    drop(migrated);
    let pending_connection = Connection::open(&pending)?;
    pending_connection.busy_timeout(Duration::from_secs(2))?;
    let pending_version: i32 =
        pending_connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if pending_version != CURRENT_SCHEMA_VERSION as i32 {
        return Err(AppError::new(
            "store_migration_failed",
            "shadow Wiki did not reach the target schema version",
        ));
    }
    let (busy, _, _): (i64, i64, i64) =
        pending_connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(AppError::new(
            "database_busy",
            "shadow Wiki checkpoint is busy; resume this work",
        ));
    }
    drop(pending_connection);
    remove_empty_sqlite_sidecars(&pending)?;

    progress(0, 1, "swapping-shadow")?;
    let live = Connection::open(&request.database)?;
    live.busy_timeout(Duration::from_secs(2))?;
    live.execute_batch("BEGIN IMMEDIATE")?;
    let observed_revision: String = live.query_row(
        "SELECT value FROM meta WHERE key = 'store_revision'",
        [],
        |row| row.get(0),
    )?;
    if observed_revision != base_revision {
        let _ = live.execute_batch("ROLLBACK");
        return Err(AppError::new(
            "migration_conflict",
            "live Wiki changed while its shadow migration was being built; resume to rebuild",
        ));
    }
    live.execute_batch("COMMIT")?;
    let (busy, _, _): (i64, i64, i64) =
        live.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(AppError::new(
            "database_busy",
            "live Wiki has an active reader; resume migration after it closes",
        ));
    }
    drop(live);

    let checkpoint_directory = request
        .database
        .parent()
        .ok_or_else(|| AppError::new("work_invalid", "Wiki database has no parent directory"))?
        .join("checkpoints");
    fs::create_dir_all(&checkpoint_directory)?;
    set_directory_mode(&checkpoint_directory)?;
    let backup_name = format!("pre-migration-{}-{}.db", &request.id[..12], now_ms());
    let backup = checkpoint_directory.join(backup_name);
    let live_wal = sqlite_sidecar(&request.database, "wal");
    let live_shm = sqlite_sidecar(&request.database, "shm");
    let backup_wal = sqlite_sidecar(&backup, "wal");
    let backup_shm = sqlite_sidecar(&backup, "shm");
    fs::rename(&request.database, &backup)?;
    move_if_exists(&live_wal, &backup_wal)?;
    move_if_exists(&live_shm, &backup_shm)?;
    if let Err(error) = fs::rename(&pending, &request.database) {
        let _ = move_if_exists(&backup_wal, &live_wal);
        let _ = move_if_exists(&backup_shm, &live_shm);
        let _ = fs::rename(&backup, &request.database);
        return Err(error.into());
    }
    progress(1, 1, "swapping-shadow")?;

    progress(0, 1, "materializing")?;
    let mut store = Store::open(&request.scope, &request.database)?;
    store.materialize()?;
    progress(1, 1, "materializing")?;
    Ok(json!({
        "database": request.database,
        "schema_version": CURRENT_SCHEMA_VERSION,
        "safety_checkpoint": backup,
    }))
}

fn sqlite_sidecar(database: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}-{suffix}", database.display()))
}

fn move_if_exists(from: &Path, to: &Path) -> Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_empty_sqlite_sidecars(database: &Path) -> Result<()> {
    for (path, must_be_empty) in [
        (sqlite_sidecar(database, "wal"), true),
        (sqlite_sidecar(database, "shm"), false),
    ] {
        match fs::metadata(&path) {
            Ok(metadata) if !must_be_empty || metadata.len() == 0 => {
                fs::remove_file(&path)?;
            }
            Ok(_) => {
                return Err(AppError::new(
                    "store_migration_failed",
                    "shadow Wiki still has uncheckpointed WAL data",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_sqlite_family(database: &Path) -> Result<()> {
    for path in [
        database.to_path_buf(),
        sqlite_sidecar(database, "wal"),
        sqlite_sidecar(database, "shm"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
    states.sort_by_key(|state| std::cmp::Reverse(state.updated_at_unix_ms));
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
    #[cfg(windows)]
    disable_standard_handle_inheritance()?;
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

#[cfg(windows)]
fn disable_standard_handle_inheritance() -> Result<()> {
    use std::ffi::c_void;

    const STD_INPUT_HANDLE: u32 = -10_i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12_i32 as u32;
    const HANDLE_FLAG_INHERIT: u32 = 1;
    const INVALID_HANDLE_VALUE: *mut c_void = -1_isize as *mut c_void;

    unsafe extern "system" {
        #[link_name = "GetStdHandle"]
        fn get_std_handle(n_std_handle: u32) -> *mut c_void;
        #[link_name = "SetHandleInformation"]
        fn set_handle_information(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }

    for standard_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { get_std_handle(standard_handle) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { set_handle_information(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(AppError::new(
                "work_spawn_failed",
                format!(
                    "failed to detach Work standard handles: {}",
                    io::Error::last_os_error()
                ),
            ));
        }
    }
    Ok(())
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
    if let Err(error) = replace_with_retry(|| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn replace_with_retry(mut replace: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    for attempt in 0..=STATE_REPLACE_RETRIES {
        match replace() {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < STATE_REPLACE_RETRIES
                    && matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                    ) =>
            {
                thread::sleep(STATE_REPLACE_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("state replacement loop always returns")
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

    #[test]
    fn transient_state_replace_conflicts_are_retried() {
        let mut attempts = 0;
        replace_with_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(attempts, 3);
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
