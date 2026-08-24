use crate::{
    error::{AppError, Result},
    learning_schema,
    scope::{
        Scope, StorePath, init_store_path, require_database_runtime_root,
        resolve_explicit_read_store_paths, resolve_read_store_paths,
    },
    store::{
        DetachedChangesetIntent, Store, StoreIdentity, SyncExportSummary, SyncTransferKind,
        SyncTransferSummary, apply_sync_transfer_artifact, cleanup_sync_conflict_candidates,
        create_empty_sync_state, merge_sync_states_directional, next_sync_conflict_batch,
        prepare_sync_transfer, resolve_sync_conflicts, sync_publication_receipt, sync_state_digest,
    },
};
use clap::ValueEnum;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const PROTOCOL_VERSION: u32 = 2;
const MAX_PROTOCOL_BYTES: u64 = 1024 * 1024;
const SSH_TIMEOUT: Duration = Duration::from_secs(30);
const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TRANSFER_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const FIXED_PLUGINS: [&str; 3] = ["tutor", "book", "practice"];
static STATE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lower")]
pub(crate) enum SyncMode {
    Merge,
    Pull,
    Push,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PeerRequest {
    protocol: u32,
    action: String,
    session_id: String,
    scope: Scope,
    directory: Option<PathBuf>,
    #[serde(default)]
    store_scope: Option<Scope>,
    #[serde(default)]
    payload_size: Option<u64>,
    #[serde(default)]
    state_digest: Option<String>,
    #[serde(default)]
    expected: Option<StoreIdentity>,
    #[serde(default)]
    baseline_digest: Option<String>,
    #[serde(default)]
    requester_store_id: Option<String>,
    #[serde(default)]
    plugin: Option<PluginRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PluginRequest {
    plugin_id: String,
    store_id: String,
    revision: u64,
    records_sha256: String,
    logical_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PluginExportInventory {
    store_id: String,
    revision: u64,
    records_sha256: String,
    logical_hash: String,
    artifact_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PeerPlugin {
    plugin_id: String,
    runtime_ready: bool,
    export: Option<PluginExportInventory>,
    history: Vec<PluginExportInventory>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PeerStore {
    scope: Scope,
    identity: StoreIdentity,
    #[serde(default)]
    missing: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PeerResponse {
    protocol: u32,
    action: String,
    session_id: String,
    version: String,
    stores: Vec<PeerStore>,
    plugins: Vec<PeerPlugin>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PeerTransfer {
    protocol: u32,
    action: String,
    session_id: String,
    scope: Scope,
    kind: SyncTransferKind,
    size: u64,
    state_digest: String,
    baseline_digest: Option<String>,
    artifact_digest: String,
    identity: StoreIdentity,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PeerPluginTransfer {
    protocol: u32,
    action: String,
    session_id: String,
    plugin: PluginRequest,
    size: u64,
    artifact_digest: String,
}

#[derive(Debug, Deserialize)]
struct PluginManifest {
    format: u32,
    plugin: String,
    store_id: String,
    revision: u64,
    records_sha256: String,
    logical_hash: String,
    blobs: Vec<PluginBlobManifest>,
}

#[derive(Debug, Deserialize)]
struct PluginBlobManifest {
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct NormalizedPluginRecord {
    table: String,
    key: Vec<Value>,
    values: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct SessionState {
    protocol: u32,
    session_id: String,
    mode: SyncMode,
    scope: Scope,
    host: String,
    remote_directory: Option<PathBuf>,
    phase: String,
    peer_digest: Option<String>,
    peer_stores: Vec<PeerStore>,
    #[serde(default)]
    peer_plugins: Vec<PeerPlugin>,
    #[serde(default)]
    plugin_results: Vec<Value>,
    #[serde(default)]
    conflict_count: u64,
    #[serde(default)]
    conflict_kinds: Vec<String>,
    #[serde(default)]
    next_action: Option<String>,
    #[serde(default)]
    units: Vec<SessionUnit>,
    created_at_unix_ms: u128,
    updated_at_unix_ms: u128,
    #[serde(default)]
    state_revision: u64,
    #[serde(default)]
    previous_state_digest: Option<String>,
    #[serde(default)]
    git_derived: Option<Value>,
    #[serde(default)]
    git_applied_local: bool,
    #[serde(default)]
    git_published_remote: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct SessionUnit {
    scope: Scope,
    local_identity: StoreIdentity,
    #[serde(default)]
    staged_digest: Option<String>,
    #[serde(default)]
    remote_digest: Option<String>,
    #[serde(default)]
    transfer_kind: Option<SyncTransferKind>,
    #[serde(default)]
    transferred_bytes: u64,
    #[serde(default)]
    full_bytes: u64,
    #[serde(default)]
    artifact_digest: Option<String>,
    #[serde(default)]
    transfer_baseline_digest: Option<String>,
    #[serde(default)]
    published_local: bool,
    #[serde(default)]
    published_remote: bool,
    #[serde(default)]
    derived_local: bool,
    #[serde(default)]
    derived_remote: bool,
    #[serde(default)]
    continuity_local: bool,
    #[serde(default)]
    continuity_remote: bool,
    #[serde(default)]
    acknowledged_remote: bool,
    #[serde(default)]
    ending_local: Option<StoreIdentity>,
    #[serde(default)]
    ending_remote: Option<StoreIdentity>,
    #[serde(default)]
    publication_local: Option<Value>,
    #[serde(default)]
    publication_remote: Option<Value>,
    #[serde(default)]
    derived_local_result: Option<Value>,
    #[serde(default)]
    derived_remote_result: Option<Value>,
    #[serde(default)]
    continuity_local_result: Option<Value>,
    #[serde(default)]
    continuity_remote_result: Option<Value>,
    #[serde(default)]
    local_missing: bool,
    #[serde(default)]
    resolved_conflict_ids: Vec<String>,
}

struct StagedStore {
    store: StorePath,
    peer: PeerStore,
    local_state: PathBuf,
    remote_state: PathBuf,
    merged_state: PathBuf,
    digest: String,
    transfer_kind: SyncTransferKind,
    transferred_bytes: u64,
    full_bytes: u64,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    cwd: &Path,
    scope: Scope,
    host: &str,
    directory: Option<&Path>,
    mode: SyncMode,
    resume: Option<&str>,
    resolve: Option<&Path>,
    abort: Option<&str>,
) -> Result<Value> {
    validate_host(host)?;
    let resolution = resolve
        .map(|path| {
            let bytes = read_bounded(fs::File::open(path)?, MAX_PROTOCOL_BYTES)?;
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| AppError::new("sync_resolution_invalid", error.to_string()))
        })
        .transpose()?;
    let remote_directory = validate_remote_directory(scope, directory)?;
    let mut stores = match resolve_read_store_paths(scope, cwd, true) {
        Ok(stores) => stores,
        Err(error)
            if error.code == "store_not_found" && scope != Scope::All && mode != SyncMode::Push =>
        {
            let store = init_store_path(scope, cwd)?;
            let runtime = store.path.parent().ok_or_else(|| {
                AppError::new("invalid_store_path", "Wiki has no runtime directory")
            })?;
            ensure_real_directory(runtime)?;
            vec![store]
        }
        Err(error)
            if error.code == "store_not_found" && scope != Scope::All && mode == SyncMode::Push =>
        {
            return Ok(json!({
                "action": "completed",
                "scope": scope,
                "mode": mode,
                "stores": [],
                "skipped": "local_store_missing",
            }));
        }
        Err(error) => return Err(error),
    };
    if scope == Scope::All {
        for missing_scope in [Scope::Project, Scope::Global] {
            if !stores.iter().any(|store| store.scope == missing_scope) {
                let store = init_store_path(missing_scope, cwd)?;
                let runtime = store.path.parent().ok_or_else(|| {
                    AppError::new("invalid_store_path", "Wiki has no runtime directory")
                })?;
                ensure_real_directory(runtime)?;
                stores.push(store);
            }
        }
        stores.sort_by_key(|store| store.scope);
    }

    if let Some(session_id) = abort {
        validate_session_id(session_id)?;
        let mut state = read_state_consistent(&stores, session_id)?;
        if state.scope != scope
            || state.mode != mode
            || state.host != host
            || state.remote_directory != remote_directory
        {
            return Err(AppError::new(
                "sync_session_mismatch",
                "abort arguments do not match the saved Sync session",
            ));
        }
        if !matches!(
            state.phase.as_str(),
            "handshake_complete" | "staging" | "conflicts" | "aborted"
        ) || state
            .units
            .iter()
            .any(|unit| unit.published_local || unit.published_remote)
        {
            return Err(AppError::new(
                "sync_partially_applied",
                "a Sync session cannot be aborted after canonical publication",
            ));
        }
        state.phase = "aborted".to_string();
        state.updated_at_unix_ms = now_unix_ms()?;
        write_state_all(&stores, &mut state)?;
        return Ok(json!({
            "action": "aborted",
            "session_id": session_id,
            "scope": scope,
            "mode": state.mode,
        }));
    }

    let (session_id, created_at, saved_state, git_only_state) = if let Some(session_id) = resume {
        validate_session_id(session_id)?;
        let state = read_state_consistent(&stores, session_id)?;
        if state.phase == "aborted" {
            return Err(AppError::new(
                "sync_session_aborted",
                "an aborted Sync session cannot be resumed",
            ));
        }
        if state.scope != scope
            || state.mode != mode
            || state.host != host
            || state.remote_directory != remote_directory
        {
            return Err(AppError::new(
                "sync_session_mismatch",
                "resume arguments do not match the saved Sync session",
            ));
        }
        if state.phase == "completed" {
            let receipts = session_receipts(&state);
            return Ok(json!({
                "action": "completed",
                "session_id": state.session_id,
                "scope": state.scope,
                "mode": state.mode,
                "stores": receipts,
                "plugins": state.plugin_results,
                "git_derived": state.git_derived,
                "idempotent": true,
            }));
        }
        let git_only = ((state.phase == "conflicts"
            && state.conflict_kinds == ["git".to_string()])
            || state.phase == "git_pending")
            .then_some(state.clone());
        (
            session_id.to_string(),
            state.created_at_unix_ms,
            Some(state),
            git_only,
        )
    } else {
        (new_session_id()?, now_unix_ms()?, None, None)
    };
    if let Some(resolution) = resolution.as_ref() {
        validate_resolution_envelope(scope, resolution, saved_state.as_ref())?;
    }

    if let Some(mut state) = git_only_state {
        if resolution.is_some() {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "Git conflicts are resolved in Git, then resumed without --resolve",
            ));
        }
        let remote_project = remote_directory.as_deref().ok_or_else(|| {
            AppError::new(
                "invalid_sync_directory",
                "Git Sync requires a remote project directory",
            )
        })?;
        let git = serde_json::to_value(crate::sync_git::run(
            cwd,
            host,
            remote_project,
            mode,
            &session_id,
        )?)
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?;
        state.git_applied_local |= git["applied_local"] == true;
        state.git_published_remote |= git["published_remote"] == true;
        rebuild_codegraph_after_git(
            &mut state,
            &stores,
            host,
            remote_directory.as_deref(),
            &session_id,
        )?;
        if git["status"] == "conflicts" {
            state.updated_at_unix_ms = now_unix_ms()?;
        } else if matches!(
            git["status"].as_str(),
            Some(
                "pending_local_wip"
                    | "pending_worktree_collision"
                    | "pending_remote_push"
                    | "remote_git_unavailable"
            )
        ) {
            state.phase = "git_pending".to_string();
            state.conflict_count = 0;
            state.conflict_kinds = vec!["git".to_string()];
            state.next_action = Some("resolve_or_commit_git_state_then_resume".to_string());
            state.updated_at_unix_ms = now_unix_ms()?;
        } else if git_derived_failed(&state) {
            state.phase = "partially_applied".to_string();
            state.conflict_count = 0;
            state.conflict_kinds.clear();
            state.next_action = Some("resume_derived_rebuild".to_string());
            state.updated_at_unix_ms = now_unix_ms()?;
        } else {
            cleanup_plugin_session(&session_id)?;
            cleanup_plugin_session_on_peer(host, scope, remote_directory.as_deref(), &session_id)?;
            state.phase = "completed".to_string();
            state.conflict_count = 0;
            state.conflict_kinds.clear();
            state.next_action = None;
            state.updated_at_unix_ms = now_unix_ms()?;
        }
        write_state_all(&stores, &mut state)?;
        return Ok(json!({
            "action": state.phase,
            "session_id": session_id,
            "scope": scope,
            "mode": mode,
            "git": git,
            "git_derived": state.git_derived,
            "stores": session_receipts(&state),
            "plugins": state.plugin_results,
        }));
    }

    let peer = if let Some(saved) = saved_state.as_ref() {
        PeerResponse {
            protocol: saved.protocol,
            action: "handshake".to_string(),
            session_id: saved.session_id.clone(),
            version: "bound-session".to_string(),
            stores: saved.peer_stores.clone(),
            plugins: saved.peer_plugins.clone(),
        }
    } else {
        let request = PeerRequest {
            protocol: PROTOCOL_VERSION,
            action: "handshake".to_string(),
            session_id: session_id.clone(),
            scope,
            directory: remote_directory.clone(),
            store_scope: None,
            payload_size: None,
            state_digest: None,
            expected: None,
            baseline_digest: None,
            requester_store_id: None,
            plugin: None,
        };
        call_peer(host, &request)?
    };
    if peer.protocol != PROTOCOL_VERSION {
        return Err(AppError::new(
            "sync_protocol_mismatch",
            format!(
                "remote protocol {} is incompatible with local protocol {PROTOCOL_VERSION}",
                peer.protocol
            ),
        ));
    }
    if peer.action != "handshake" || peer.session_id != session_id {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote handshake does not match the requested Sync session",
        ));
    }
    let local_plugins = fixed_plugin_inventory(&session_id)?;
    let plugin_results = validate_fixed_plugin_preflight(&local_plugins, &peer.plugins)?;
    let peer_bytes = serde_json::to_vec(&peer)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
    let state_created_at = now_unix_ms()?;
    let mut state = saved_state.unwrap_or_else(|| SessionState {
        protocol: PROTOCOL_VERSION,
        session_id: session_id.clone(),
        mode,
        scope,
        host: host.to_string(),
        remote_directory: remote_directory.clone(),
        phase: "handshake_complete".to_string(),
        peer_digest: Some(hex_digest(&peer_bytes)),
        peer_stores: peer.stores,
        peer_plugins: peer.plugins,
        plugin_results,
        conflict_count: 0,
        conflict_kinds: Vec::new(),
        next_action: None,
        units: Vec::new(),
        created_at_unix_ms: created_at,
        updated_at_unix_ms: state_created_at,
        state_revision: 0,
        previous_state_digest: None,
        git_derived: None,
        git_applied_local: false,
        git_published_remote: false,
    });
    state.phase = "staging".to_string();
    write_state_all(&stores, &mut state)?;
    let mut results = Vec::new();
    let mut all_conflicts = Vec::new();
    let mut remaining_conflict_count = 0_u64;
    let mut remaining_conflict_kinds = std::collections::BTreeSet::new();
    let mut staged = Vec::new();
    for store_path in &stores {
        let peer_store = state
            .peer_stores
            .iter()
            .find(|peer| peer.scope == store_path.scope)
            .cloned()
            .ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "remote omitted a requested store")
            })?;
        let local_missing = !canonical_store_exists(&store_path.path)?;
        if local_missing && mode == SyncMode::Push {
            continue;
        }
        if local_missing && peer_store.missing {
            continue;
        }
        let directory = session_scope_directory(store_path, &session_id)?;
        let local_state = directory.join("local.db");
        let remote_state = directory.join("remote.db");
        let base_local_state = directory.join("base-local.db");
        let base_remote_state = directory.join("base-remote.db");
        let merged_state = directory.join("merged.db");
        for artifact in [
            &local_state,
            &remote_state,
            &base_local_state,
            &base_remote_state,
            &merged_state,
        ] {
            reject_non_regular_artifact(artifact)?;
        }
        let local_identity = if !local_state.exists() {
            if !local_missing {
                let local = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                local.begin_read_snapshot()?;
                let identity = local.identity()?;
                export_sync_state_with_continuity(store_path, &local, &local_state)?;
                identity
            } else {
                create_empty_sync_state(&local_state)?;
                StoreIdentity {
                    store_id: "0".repeat(64),
                    revision: "0".repeat(64),
                    operation_id: -1,
                }
            }
        } else {
            sync_state_digest(&local_state)?;
            state
                .units
                .iter()
                .find(|unit| unit.scope == store_path.scope)
                .map(|unit| unit.local_identity.clone())
                .ok_or_else(|| {
                    AppError::new(
                        "sync_state_invalid",
                        "staged local state has no bound Store identity",
                    )
                })?
        };
        match state
            .units
            .iter()
            .find(|unit| unit.scope == store_path.scope)
        {
            Some(unit) if unit.local_identity != local_identity => {
                return Err(AppError::new(
                    "sync_state_invalid",
                    "staged local state no longer matches its bound Store identity",
                ));
            }
            Some(_) => {}
            None => {
                state.units.push(SessionUnit {
                    scope: store_path.scope,
                    local_identity: local_identity.clone(),
                    staged_digest: None,
                    remote_digest: None,
                    transfer_kind: None,
                    transferred_bytes: 0,
                    full_bytes: 0,
                    artifact_digest: None,
                    transfer_baseline_digest: None,
                    published_local: false,
                    published_remote: false,
                    derived_local: false,
                    derived_remote: false,
                    continuity_local: false,
                    continuity_remote: false,
                    acknowledged_remote: false,
                    ending_local: None,
                    ending_remote: None,
                    publication_local: None,
                    publication_remote: None,
                    derived_local_result: None,
                    derived_remote_result: None,
                    continuity_local_result: None,
                    continuity_remote_result: None,
                    local_missing,
                    resolved_conflict_ids: Vec::new(),
                });
                if peer_store.missing {
                    state
                        .units
                        .last_mut()
                        .expect("new Sync unit")
                        .acknowledged_remote = true;
                }
                state.updated_at_unix_ms = now_unix_ms()?;
                write_state_all(&stores, &mut state)?;
            }
        }
        let acknowledged_local = baseline_path(store_path, &peer_store.identity.store_id, "local")?;
        let acknowledged_remote =
            baseline_path(store_path, &peer_store.identity.store_id, "remote")?;
        let has_acknowledged_remote = acknowledged_remote.is_file();
        if !base_local_state.exists() {
            if acknowledged_local.is_file() {
                fs::copy(&acknowledged_local, &base_local_state)?;
                sync_state_digest(&base_local_state)?;
            } else {
                create_empty_sync_state(&base_local_state)?;
            }
        }
        if !base_remote_state.exists() {
            if has_acknowledged_remote {
                fs::copy(&acknowledged_remote, &base_remote_state)?;
                sync_state_digest(&base_remote_state)?;
            } else {
                create_empty_sync_state(&base_remote_state)?;
            }
        }
        let existing_remote_digest = remote_state
            .is_file()
            .then(|| sync_state_digest(&remote_state))
            .transpose()?;
        let recorded_remote_digest = state
            .units
            .iter()
            .find(|unit| unit.scope == store_path.scope)
            .and_then(|unit| unit.remote_digest.as_ref());
        if existing_remote_digest.is_none()
            || existing_remote_digest.as_ref() != recorded_remote_digest
        {
            if remote_state.is_file() {
                fs::remove_file(&remote_state)?;
            }
            let export = PeerRequest {
                protocol: PROTOCOL_VERSION,
                action: "export".to_string(),
                session_id: session_id.clone(),
                scope: store_path.scope,
                directory: remote_directory_for_store(
                    store_path.scope,
                    remote_directory.as_deref(),
                ),
                store_scope: Some(store_path.scope),
                payload_size: None,
                state_digest: None,
                expected: Some(peer_store.identity.clone()),
                baseline_digest: has_acknowledged_remote
                    .then(|| sync_state_digest(&base_remote_state))
                    .transpose()?,
                requester_store_id: Some(local_identity.store_id.clone()),
                plugin: None,
            };
            let transfer = call_peer_export(
                host,
                &export,
                has_acknowledged_remote.then_some(base_remote_state.as_path()),
                &remote_state,
            )?;
            let unit = state
                .units
                .iter_mut()
                .find(|unit| unit.scope == store_path.scope)
                .expect("local stage receipt");
            unit.remote_digest = Some(transfer.state_digest);
            unit.transfer_kind = Some(transfer.kind);
            unit.transferred_bytes = transfer.size;
            unit.full_bytes = fs::metadata(&remote_state)?.len();
            unit.artifact_digest = Some(transfer.artifact_digest.clone());
            unit.transfer_baseline_digest = transfer.baseline_digest.clone();
            state.updated_at_unix_ms = now_unix_ms()?;
            write_state_all(&stores, &mut state)?;
        }
        let conflicts_path = directory.join("conflicts.json");
        let mut summary = if merged_state.exists() {
            if conflicts_path.is_file() {
                let conflicts: Vec<Value> = serde_json::from_slice(&fs::read(&conflicts_path)?)
                    .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
                crate::store::SyncMergeSummary {
                    state_digest: sync_state_digest(&merged_state)?,
                    conflict_count: conflicts.len(),
                    conflict_kinds: conflicts
                        .iter()
                        .filter_map(|value| value["kind"].as_str().map(str::to_string))
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    conflicts,
                }
            } else {
                crate::store::SyncMergeSummary {
                    state_digest: sync_state_digest(&merged_state)?,
                    conflict_count: 0,
                    conflict_kinds: Vec::new(),
                    conflicts: Vec::new(),
                }
            }
        } else {
            merge_sync_states_directional(
                &base_local_state,
                &local_state,
                &base_remote_state,
                &remote_state,
                &merged_state,
            )?
        };
        let mut resolved_batch = false;
        if summary.conflict_count > 0 {
            let scoped_resolution = resolution.as_ref().and_then(|resolution| {
                if let Some(scopes) = resolution["scopes"].as_object() {
                    scopes
                        .get(scope_name(store_path.scope))
                        .map(|scope| json!({"version": 1, "decisions": scope["decisions"].clone()}))
                } else {
                    Some(resolution.clone())
                }
            });
            if let Some(scoped_resolution) = scoped_resolution {
                resolved_batch = true;
                let batch = next_sync_conflict_batch(&summary.conflicts);
                summary.state_digest =
                    resolve_sync_conflicts(&merged_state, &batch, &scoped_resolution)?;
                let resolved_ids = batch
                    .iter()
                    .filter_map(|conflict| conflict["conflict_id"].as_str().map(str::to_string))
                    .collect::<std::collections::BTreeSet<_>>();
                summary.conflicts.retain(|conflict| {
                    !conflict["conflict_id"]
                        .as_str()
                        .is_some_and(|id| resolved_ids.contains(id))
                });
                let unit = state
                    .units
                    .iter_mut()
                    .find(|unit| unit.scope == store_path.scope)
                    .expect("staged conflict unit");
                unit.resolved_conflict_ids.extend(resolved_ids);
                unit.resolved_conflict_ids.sort();
                unit.resolved_conflict_ids.dedup();
                write_atomic(
                    &directory.join("resolution.json"),
                    &serde_json::to_vec_pretty(&scoped_resolution).map_err(|error| {
                        AppError::new("sync_resolution_invalid", error.to_string())
                    })?,
                )?;
                summary.conflict_count = summary.conflicts.len();
                summary.conflict_kinds = summary
                    .conflicts
                    .iter()
                    .filter_map(|conflict| conflict["kind"].as_str().map(str::to_string))
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
            }
        }
        if summary.conflict_count > 0 {
            write_atomic(
                &directory.join("conflicts.json"),
                &serde_json::to_vec_pretty(&summary.conflicts)
                    .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
            )?;
            remaining_conflict_count += summary.conflict_count as u64;
            remaining_conflict_kinds.extend(summary.conflict_kinds.iter().cloned());
            all_conflicts.extend(next_sync_conflict_batch(&summary.conflicts));
            results.push(json!({
                "scope": store_path.scope,
                "state_digest": summary.state_digest,
                "conflict_count": summary.conflict_count,
                "conflict_kinds": summary.conflict_kinds,
            }));
            continue;
        } else if conflicts_path.is_file() {
            if resolved_batch {
                summary.state_digest = cleanup_sync_conflict_candidates(&merged_state)?;
            }
            fs::remove_file(&conflicts_path)?;
        }

        match state
            .units
            .iter_mut()
            .find(|unit| unit.scope == store_path.scope)
        {
            Some(unit) => {
                if unit.local_identity != local_identity
                    || unit
                        .staged_digest
                        .as_ref()
                        .is_some_and(|digest| digest != &summary.state_digest)
                {
                    return Err(AppError::new(
                        "sync_state_invalid",
                        "staged Sync unit no longer matches its durable receipt",
                    ));
                }
                unit.staged_digest = Some(summary.state_digest.clone());
            }
            None => state.units.push(SessionUnit {
                scope: store_path.scope,
                local_identity,
                staged_digest: Some(summary.state_digest.clone()),
                remote_digest: Some(sync_state_digest(&remote_state)?),
                transfer_kind: None,
                transferred_bytes: 0,
                full_bytes: 0,
                artifact_digest: None,
                transfer_baseline_digest: None,
                published_local: false,
                published_remote: false,
                derived_local: false,
                derived_remote: false,
                continuity_local: false,
                continuity_remote: false,
                acknowledged_remote: false,
                ending_local: None,
                ending_remote: None,
                publication_local: None,
                publication_remote: None,
                derived_local_result: None,
                derived_remote_result: None,
                continuity_local_result: None,
                continuity_remote_result: None,
                local_missing,
                resolved_conflict_ids: Vec::new(),
            }),
        }
        state.updated_at_unix_ms = now_unix_ms()?;
        write_state_all(&stores, &mut state)?;
        staged.push(StagedStore {
            store: store_path.clone(),
            peer: peer_store,
            local_state: local_state.clone(),
            remote_state: remote_state.clone(),
            merged_state,
            digest: summary.state_digest,
            transfer_kind: state
                .units
                .iter()
                .find(|unit| unit.scope == store_path.scope)
                .and_then(|unit| unit.transfer_kind)
                .unwrap_or(SyncTransferKind::Full),
            transferred_bytes: state
                .units
                .iter()
                .find(|unit| unit.scope == store_path.scope)
                .map_or(0, |unit| unit.transferred_bytes),
            full_bytes: fs::metadata(&remote_state)?.len(),
        });
    }
    if all_conflicts.is_empty() {
        state.phase = "publishing".to_string();
        state.updated_at_unix_ms = now_unix_ms()?;
        write_state_all(&stores, &mut state)?;
        let local_plugin_inventory = fixed_plugin_inventory(&session_id)?;
        let peer_plugins = state.peer_plugins.clone();
        let mut plugin_progress = state.plugin_results.clone();
        state.plugin_results = reconcile_fixed_plugins(
            host,
            scope,
            remote_directory.as_deref(),
            &session_id,
            mode,
            &local_plugin_inventory,
            &peer_plugins,
            |result| {
                if let Some(index) = plugin_progress
                    .iter()
                    .position(|entry| entry["plugin"] == result["plugin"])
                {
                    plugin_progress[index] = result.clone();
                } else {
                    plugin_progress.push(result.clone());
                }
                state.plugin_results = plugin_progress.clone();
                state.updated_at_unix_ms = now_unix_ms()?;
                write_state_all(&stores, &mut state)
            },
        )?;
        state.updated_at_unix_ms = now_unix_ms()?;
        write_state_all(&stores, &mut state)?;
        for staged_store in &staged {
            let unit_index = state
                .units
                .iter()
                .position(|unit| unit.scope == staged_store.store.scope)
                .expect("staged unit receipt");
            if mode != SyncMode::Pull && !state.units[unit_index].published_remote {
                let publish = PeerRequest {
                    protocol: PROTOCOL_VERSION,
                    action: "publish".to_string(),
                    session_id: session_id.clone(),
                    scope: staged_store.store.scope,
                    directory: remote_directory_for_store(
                        staged_store.store.scope,
                        remote_directory.as_deref(),
                    ),
                    store_scope: Some(staged_store.store.scope),
                    payload_size: Some(fs::metadata(&staged_store.merged_state)?.len()),
                    state_digest: Some(staged_store.digest.clone()),
                    expected: Some(staged_store.peer.identity.clone()),
                    baseline_digest: None,
                    requester_store_id: Some(
                        state.units[unit_index]
                            .ending_local
                            .as_ref()
                            .unwrap_or(&state.units[unit_index].local_identity)
                            .store_id
                            .clone(),
                    ),
                    plugin: None,
                };
                let response = call_peer_publish(host, &publish, &staged_store.merged_state)?;
                state.units[unit_index].published_remote = true;
                let remote_continuity = response["continuity"].clone();
                let remote_derived = response["derived"].clone();
                state.units[unit_index].continuity_remote =
                    continuity_succeeded(&remote_continuity);
                state.units[unit_index].derived_remote = derived_succeeded(&remote_derived);
                state.units[unit_index].ending_remote =
                    serde_json::from_value(response["ending_identity"].clone()).ok();
                state.units[unit_index].publication_remote = Some(response["publication"].clone());
                state.units[unit_index].continuity_remote_result = Some(remote_continuity);
                state.units[unit_index].derived_remote_result = Some(remote_derived.clone());
                state.updated_at_unix_ms = now_unix_ms()?;
                write_state_all(&stores, &mut state)?;
            }
            if mode != SyncMode::Pull
                && state.units[unit_index].published_remote
                && (!state.units[unit_index].continuity_remote
                    || !state.units[unit_index].derived_remote)
            {
                let rebuild = PeerRequest {
                    protocol: PROTOCOL_VERSION,
                    action: "rebuild".to_string(),
                    session_id: session_id.clone(),
                    scope: staged_store.store.scope,
                    directory: remote_directory_for_store(
                        staged_store.store.scope,
                        remote_directory.as_deref(),
                    ),
                    store_scope: Some(staged_store.store.scope),
                    payload_size: None,
                    state_digest: state.units[unit_index].staged_digest.clone(),
                    expected: None,
                    baseline_digest: None,
                    requester_store_id: Some(
                        state.units[unit_index]
                            .ending_local
                            .as_ref()
                            .unwrap_or(&state.units[unit_index].local_identity)
                            .store_id
                            .clone(),
                    ),
                    plugin: None,
                };
                let response = call_peer_value(host, &rebuild)?;
                if response["protocol"] != PROTOCOL_VERSION
                    || response["action"] != "rebuilt"
                    || response["session_id"] != session_id
                {
                    return Err(AppError::new(
                        "sync_protocol_invalid",
                        "remote rebuild response does not match the session",
                    ));
                }
                let remote_derived = response["derived"].clone();
                let remote_continuity = response["continuity"].clone();
                state.units[unit_index].continuity_remote =
                    continuity_succeeded(&remote_continuity);
                state.units[unit_index].continuity_remote_result = Some(remote_continuity);
                state.units[unit_index].derived_remote = derived_succeeded(&remote_derived);
                state.units[unit_index].derived_remote_result = Some(remote_derived.clone());
                state.updated_at_unix_ms = now_unix_ms()?;
                write_state_all(&stores, &mut state)?;
            }
            if mode != SyncMode::Push {
                if !state.units[unit_index].published_local {
                    let recovered = if canonical_store_exists(&staged_store.store.path)? {
                        state.units[unit_index]
                            .staged_digest
                            .as_deref()
                            .map(|digest| {
                                sync_publication_receipt(
                                    &staged_store.store.path,
                                    &session_id,
                                    digest,
                                )
                            })
                            .transpose()?
                            .flatten()
                    } else {
                        None
                    };
                    let publication = if let Some(mut recovered) = recovered {
                        recovered["recovered"] = Value::Bool(true);
                        recovered
                    } else {
                        let summary = if state.units[unit_index].local_missing {
                            Store::publish_sync_state_to_missing(
                                scope_name(staged_store.store.scope),
                                &staged_store.store.path,
                                &staged_store.merged_state,
                                &session_id,
                            )?
                        } else {
                            let mut local_store = Store::open(
                                scope_name(staged_store.store.scope),
                                &staged_store.store.path,
                            )?;
                            local_store.publish_sync_state(
                                &staged_store.merged_state,
                                &state.units[unit_index].local_identity,
                                &session_id,
                            )?
                        };
                        serde_json::to_value(summary).map_err(|error| {
                            AppError::new("sync_state_invalid", error.to_string())
                        })?
                    };
                    let local_store = Store::open_for_read(
                        scope_name(staged_store.store.scope),
                        &staged_store.store.path,
                    )?;
                    let ending_identity = local_store.identity()?;
                    drop(local_store);
                    state.units[unit_index].published_local = true;
                    state.units[unit_index].ending_local = Some(ending_identity);
                    state.units[unit_index].publication_local = Some(publication);
                    state.updated_at_unix_ms = now_unix_ms()?;
                    write_state_all(&stores, &mut state)?;
                }
                if !state.units[unit_index].continuity_local {
                    let continuity =
                        replay_sync_continuity(&staged_store.store, &staged_store.merged_state);
                    state.units[unit_index].continuity_local = continuity_succeeded(&continuity);
                    state.units[unit_index].continuity_local_result = Some(continuity);
                    state.updated_at_unix_ms = now_unix_ms()?;
                    write_state_all(&stores, &mut state)?;
                }
                if state.units[unit_index].continuity_local
                    && !state.units[unit_index].derived_local
                {
                    let publication = state.units[unit_index]
                        .publication_local
                        .as_ref()
                        .expect("published local store has a publication receipt");
                    let local_derived = rebuild_derived(&staged_store.store, publication);
                    state.units[unit_index].derived_local = derived_succeeded(&local_derived);
                    state.units[unit_index].derived_local_result = Some(local_derived.clone());
                    state.updated_at_unix_ms = now_unix_ms()?;
                    write_state_all(&stores, &mut state)?;
                }
            }
            if mode == SyncMode::Pull
                && state.units[unit_index].published_local
                && !state.units[unit_index].acknowledged_remote
            {
                let acknowledge = PeerRequest {
                    protocol: PROTOCOL_VERSION,
                    action: "ack".to_string(),
                    session_id: session_id.clone(),
                    scope: staged_store.store.scope,
                    directory: remote_directory_for_store(
                        staged_store.store.scope,
                        remote_directory.as_deref(),
                    ),
                    store_scope: Some(staged_store.store.scope),
                    payload_size: None,
                    state_digest: Some(sync_state_digest(&staged_store.remote_state)?),
                    expected: Some(staged_store.peer.identity.clone()),
                    baseline_digest: None,
                    requester_store_id: Some(
                        state.units[unit_index]
                            .ending_local
                            .as_ref()
                            .unwrap_or(&state.units[unit_index].local_identity)
                            .store_id
                            .clone(),
                    ),
                    plugin: None,
                };
                let response = call_peer_value(host, &acknowledge)?;
                if response["protocol"] != PROTOCOL_VERSION
                    || response["action"] != "acknowledged"
                    || response["session_id"] != session_id
                {
                    return Err(AppError::new(
                        "sync_protocol_invalid",
                        "remote acknowledgement does not match the session",
                    ));
                }
                state.units[unit_index].acknowledged_remote = true;
                state.updated_at_unix_ms = now_unix_ms()?;
                write_state_all(&stores, &mut state)?;
            }
            let baseline_local_source = match mode {
                SyncMode::Merge => &staged_store.merged_state,
                SyncMode::Pull => &staged_store.merged_state,
                SyncMode::Push => &staged_store.local_state,
            };
            let baseline_remote_source = match mode {
                SyncMode::Merge => &staged_store.merged_state,
                SyncMode::Pull => &staged_store.remote_state,
                SyncMode::Push => &staged_store.merged_state,
            };
            let peer_baseline_id = state.units[unit_index]
                .ending_remote
                .as_ref()
                .map_or(staged_store.peer.identity.store_id.as_str(), |identity| {
                    identity.store_id.as_str()
                });
            update_baseline(
                &staged_store.store,
                peer_baseline_id,
                "local",
                baseline_local_source,
            )?;
            update_baseline(
                &staged_store.store,
                peer_baseline_id,
                "remote",
                baseline_remote_source,
            )?;
            results.push(json!({
                "scope": staged_store.store.scope,
                "state_digest": staged_store.digest,
                "transfer_kind": staged_store.transfer_kind,
                "transferred_bytes": staged_store.transferred_bytes,
                "full_bytes": staged_store.full_bytes,
                "artifact_digest": state.units[unit_index].artifact_digest,
                "baseline_digest": state.units[unit_index].transfer_baseline_digest,
                "starting_local": state.units[unit_index].local_identity,
                "starting_remote": staged_store.peer.identity,
                "ending_local": state.units[unit_index].ending_local,
                "ending_remote": state.units[unit_index].ending_remote,
                "publication_local": state.units[unit_index].publication_local,
                "publication_remote": state.units[unit_index].publication_remote,
                "conflict_count": 0,
                "published_local": state.units[unit_index].published_local,
                "published_remote": state.units[unit_index].published_remote,
                "derived_local": state.units[unit_index].derived_local_result,
                "derived_remote": state.units[unit_index].derived_remote_result,
                "continuity_local": state.units[unit_index].continuity_local_result,
                "continuity_remote": state.units[unit_index].continuity_remote_result,
            }));
        }
    }
    let git = if all_conflicts.is_empty() && scope != Scope::Global {
        let remote_project = remote_directory.as_deref().ok_or_else(|| {
            AppError::new(
                "invalid_sync_directory",
                "project Git Sync requires a remote directory",
            )
        })?;
        serde_json::to_value(crate::sync_git::run(
            cwd,
            host,
            remote_project,
            mode,
            &session_id,
        )?)
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?
    } else {
        json!({"status": if scope == Scope::Global { "skipped_global_scope" } else { "skipped_semantic_conflicts" }})
    };
    if git["status"] == "conflicts"
        && let Some(conflicts) = git["conflicts"].as_array()
    {
        all_conflicts.extend(conflicts.iter().cloned());
    }
    state.git_applied_local |= git["applied_local"] == true;
    state.git_published_remote |= git["published_remote"] == true;
    if all_conflicts.is_empty() {
        rebuild_codegraph_after_git(
            &mut state,
            &stores,
            host,
            remote_directory.as_deref(),
            &session_id,
        )?;
    }
    if !all_conflicts.is_empty() {
        state.phase = "conflicts".to_string();
        state.conflict_count = remaining_conflict_count;
        state.conflict_kinds = remaining_conflict_kinds.into_iter().collect();
        state.next_action = Some("resolve".to_string());
    } else if matches!(
        git["status"].as_str(),
        Some(
            "pending_local_wip"
                | "pending_worktree_collision"
                | "pending_remote_push"
                | "remote_git_unavailable"
        )
    ) {
        state.phase = "git_pending".to_string();
        state.conflict_count = 0;
        state.conflict_kinds = vec!["git".to_string()];
        state.next_action = Some("resolve_or_commit_git_state_then_resume".to_string());
    } else if state.units.iter().any(|unit| {
        (mode != SyncMode::Push && (!unit.continuity_local || !unit.derived_local))
            || (mode != SyncMode::Pull && (!unit.continuity_remote || !unit.derived_remote))
    }) || git_derived_failed(&state)
    {
        state.phase = "partially_applied".to_string();
        state.conflict_count = 0;
        state.conflict_kinds.clear();
        state.next_action = Some(
            if state.units.iter().any(|unit| {
                (mode != SyncMode::Push && !unit.continuity_local)
                    || (mode != SyncMode::Pull && !unit.continuity_remote)
            }) {
                "resume_continuity"
            } else {
                "resume_derived_rebuild"
            }
            .to_string(),
        );
    } else {
        cleanup_plugin_session(&session_id)?;
        cleanup_plugin_session_on_peer(host, scope, remote_directory.as_deref(), &session_id)?;
        state.phase = "completed".to_string();
        state.next_action = None;
    }
    state.updated_at_unix_ms = now_unix_ms()?;
    write_state_all(&stores, &mut state)?;
    let conflict_batch = next_sync_conflict_batch(&all_conflicts);
    Ok(json!({
        "action": state.phase,
        "session_id": session_id,
        "scope": scope,
        "mode": mode,
        "remote_version": peer.version,
        "stores": results,
        "plugins": state.plugin_results,
        "git": git,
        "git_derived": state.git_derived,
        "conflicts": conflict_batch,
        "conflict_count": state.conflict_count,
        "next_action": state.next_action,
        "resume": format!("lwc --scope {} sync {}{} --mode {} --resume {}", scope_name(scope), host, remote_cli_directory(state.remote_directory.as_deref()), mode_name(mode), state.session_id),
    }))
}

fn session_receipts(state: &SessionState) -> Vec<Value> {
    state
        .units
        .iter()
        .map(|unit| {
            json!({
                "scope": unit.scope,
                "state_digest": unit.staged_digest,
                "baseline_digest": unit.transfer_baseline_digest,
                "artifact_digest": unit.artifact_digest,
                "transfer_kind": unit.transfer_kind,
                "transferred_bytes": unit.transferred_bytes,
                "full_bytes": unit.full_bytes,
                "starting_local": unit.local_identity,
                "starting_remote": state.peer_stores.iter().find(|peer| peer.scope == unit.scope).map(|peer| &peer.identity),
                "ending_local": unit.ending_local,
                "ending_remote": unit.ending_remote,
                "publication_local": unit.publication_local,
                "publication_remote": unit.publication_remote,
                "published_local": unit.published_local,
                "published_remote": unit.published_remote,
                "derived_local": unit.derived_local_result,
                "derived_remote": unit.derived_remote_result,
                "continuity_local": unit.continuity_local_result,
                "continuity_remote": unit.continuity_remote_result,
                "resolved_conflict_ids": unit.resolved_conflict_ids,
            })
        })
        .collect()
}

fn fixed_plugin_inventory(session_id: &str) -> Result<Vec<PeerPlugin>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    FIXED_PLUGINS
        .iter()
        .map(|plugin| plugin_inventory(&home, session_id, plugin))
        .collect()
}

fn plugin_inventory(home: &Path, session_id: &str, plugin: &str) -> Result<PeerPlugin> {
    let root = home.join(".lwc/plugins").join(plugin);
    let data_present = root.join("data.sqlite3").is_file();
    let runtime_ready = plugin_runtime_ready(plugin)?;
    let mut exports: Vec<PluginExportInventory> = Vec::new();
    for archive in [root.join("preserved"), root.join("sync-history")] {
        if !archive.is_dir() {
            continue;
        }
        for store in fs::read_dir(&archive)? {
            let store = store?;
            if !store.file_type()?.is_dir() {
                return Err(unsafe_plugin_export(plugin));
            }
            let store_id = store.file_name().to_string_lossy().into_owned();
            if !valid_plugin_store_id(&store_id) {
                return Err(unsafe_plugin_export(plugin));
            }
            for revision in fs::read_dir(store.path())? {
                let revision = revision?;
                if !revision.file_type()?.is_dir() {
                    return Err(unsafe_plugin_export(plugin));
                }
                let revision_name = revision.file_name().to_string_lossy().into_owned();
                if revision_name
                    .parse::<u64>()
                    .ok()
                    .map(|value| value.to_string())
                    != Some(revision_name)
                {
                    return Err(AppError::new(
                        "sync_plugin_invalid",
                        "plugin revision path is not canonical decimal",
                    ));
                }
                let export = validate_plugin_export(plugin, &revision.path())?;
                if export.revision.to_string() != revision.file_name().to_string_lossy() {
                    return Err(AppError::new(
                        "sync_plugin_invalid",
                        "plugin revision path does not match its manifest",
                    ));
                }
                if export.store_id != store_id {
                    return Err(AppError::new(
                        "sync_plugin_invalid",
                        "preserved plugin StoreIdentity does not match its path",
                    ));
                }
                if let Some(existing) = exports.iter().find(|existing| {
                    existing.store_id == export.store_id && existing.revision == export.revision
                }) {
                    if !same_plugin_state(existing, &export) {
                        return Err(AppError::new(
                            "sync_plugin_diverged",
                            format!("{plugin} archive revision has different canonical content"),
                        ));
                    }
                } else {
                    exports.push(export);
                }
            }
        }
    }
    if data_present {
        let live = ensure_live_plugin_export(home, session_id, plugin)?;
        if let Some(existing) = exports
            .iter()
            .find(|export| export.store_id == live.store_id && export.revision == live.revision)
        {
            if !same_plugin_state(existing, &live) {
                return Err(AppError::new(
                    "sync_plugin_changed",
                    format!("{plugin} canonical content changed without a revision bump"),
                ));
            }
        } else {
            exports.push(live);
        }
    }
    exports.sort_by_key(|export| export.revision);
    if exports.len() > 64 {
        return Err(AppError::new(
            "sync_plugin_history_too_large",
            format!("{plugin} preserved history exceeds the bounded inventory"),
        ));
    }
    if exports
        .iter()
        .map(|export| &export.store_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        > 1
    {
        return Err(AppError::new(
            "sync_plugin_diverged",
            format!("{plugin} has multiple preserved StoreIdentity values"),
        ));
    }
    let export = exports.last().cloned();
    Ok(PeerPlugin {
        plugin_id: plugin.to_owned(),
        runtime_ready,
        export,
        history: exports,
    })
}

fn validate_plugin_export(plugin: &str, root: &Path) -> Result<PluginExportInventory> {
    reject_plugin_path(root)?;
    let manifest_path = root.join("manifest.json");
    let records_path = root.join("records.ndjson");
    reject_plugin_file(&manifest_path)?;
    reject_plugin_file(&records_path)?;
    if fs::metadata(&manifest_path)?.len() > MAX_PROTOCOL_BYTES {
        return Err(AppError::new(
            "sync_plugin_invalid",
            "plugin manifest exceeds the bounded format",
        ));
    }
    let manifest: PluginManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
        .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
    if manifest.format != 1
        || manifest.plugin != plugin
        || !valid_plugin_store_id(&manifest.store_id)
        || !valid_sha256(&manifest.records_sha256)
        || !valid_sha256(&manifest.logical_hash)
        || file_digest(&records_path)? != manifest.records_sha256
    {
        return Err(AppError::new(
            "sync_plugin_invalid",
            format!("{plugin} normalized export manifest is invalid"),
        ));
    }
    let mut hasher = Sha256::new();
    plugin_digest_field(&mut hasher, b"manifest.json");
    plugin_digest_field(&mut hasher, file_digest(&manifest_path)?.as_bytes());
    plugin_digest_field(&mut hasher, b"records.ndjson");
    plugin_digest_field(&mut hasher, manifest.records_sha256.as_bytes());
    let mut expected = std::collections::BTreeSet::new();
    for blob in &manifest.blobs {
        if !valid_sha256(&blob.sha256) || !expected.insert(blob.sha256.clone()) {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "plugin blob inventory is invalid",
            ));
        }
        let relative = format!("blobs/sha256/{}/{}", &blob.sha256[..2], blob.sha256);
        let path = root.join(&relative);
        reject_plugin_file(&path)?;
        let metadata = fs::metadata(&path)?;
        if metadata.len() != blob.bytes || file_digest(&path)? != blob.sha256 {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "plugin blob does not match its canonical inventory",
            ));
        }
        plugin_digest_field(&mut hasher, relative.as_bytes());
        plugin_digest_field(&mut hasher, blob.sha256.as_bytes());
    }
    let artifact_sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(PluginExportInventory {
        store_id: manifest.store_id,
        revision: manifest.revision,
        records_sha256: manifest.records_sha256,
        logical_hash: manifest.logical_hash,
        artifact_sha256,
    })
}

fn plugin_digest_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_plugin_store_id(value: &str) -> bool {
    valid_sha256(value)
}

fn same_plugin_state(left: &PluginExportInventory, right: &PluginExportInventory) -> bool {
    left.store_id == right.store_id
        && left.revision == right.revision
        && left.logical_hash == right.logical_hash
}

fn plugin_runtime_ready(plugin: &str) -> Result<bool> {
    use crate::config::CapabilitySetting;
    let enabled = crate::config::resolve_learning(plugin)?.setting == CapabilitySetting::Enabled;
    let runtime = match plugin {
        "tutor" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Tutor)?,
        "book" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Book)?,
        "practice" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Practice)?,
        _ => return Err(AppError::new("sync_plugin_invalid", "unknown fixed plugin")),
    };
    Ok(enabled && runtime["installed"] == true)
}

fn ensure_live_plugin_export(
    home: &Path,
    session_id: &str,
    plugin: &str,
) -> Result<PluginExportInventory> {
    let root = home.join(".lwc/plugins").join(plugin);
    let database = root.join("data.sqlite3");
    reject_plugin_file(&database)?;
    let connection =
        Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute_batch("BEGIN;")?;
    let store_id: String = connection
        .query_row(
            "SELECT value FROM plugin_meta WHERE key='store_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| AppError::new("sync_plugin_invalid", "plugin StoreIdentity is missing"))?;
    if !valid_plugin_store_id(&store_id) {
        return Err(AppError::new(
            "sync_plugin_invalid",
            "plugin StoreIdentity is invalid",
        ));
    }
    let revision: u64 = connection
        .query_row(
            "SELECT value FROM plugin_meta WHERE key='revision'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse()
        .map_err(|_| AppError::new("sync_plugin_invalid", "plugin revision is invalid"))?;
    require_live_plugin_schema(plugin, &connection)?;
    let logical_hash = learning_schema::canonical_logical_hash(plugin, &connection)
        .map_err(|message| AppError::new("sync_plugin_invalid", message))?;
    let lwc = home.join(".lwc");
    let sync = lwc.join("sync");
    let plugins = sync.join("plugins");
    let session = plugins.join(session_id);
    for directory in [&lwc, &sync, &plugins, &session] {
        ensure_real_directory(directory)?;
    }
    let destination = plugin_live_export_path(home, session_id, plugin);
    if destination.is_dir() {
        let export = validate_plugin_export(plugin, &destination)?;
        if export.store_id != store_id
            || export.revision != revision
            || export.logical_hash != logical_hash
        {
            let materialized_by_session = connection
                .query_row(
                    "SELECT 1 FROM sync_receipts
                     WHERE session_id=?1 AND store_id=?2 AND resolved_revision=?3
                       AND logical_hash=?4 AND runtime_state='ready' AND state='completed'",
                    params![
                        session_id,
                        store_id,
                        i64::try_from(revision).map_err(|_| AppError::new(
                            "sync_plugin_invalid",
                            "plugin revision exceeds SQLite limits",
                        ))?,
                        logical_hash
                    ],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !materialized_by_session {
                return Err(AppError::new(
                    "sync_plugin_changed",
                    format!("{plugin} canonical content changed during the Sync session"),
                ));
            }
            fs::remove_dir_all(&destination)?;
        } else {
            return Ok(export);
        }
    }
    let staging = destination
        .parent()
        .expect("live export parent")
        .join(format!(
            ".{plugin}-export-{}-{}",
            std::process::id(),
            STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir(&staging)?;
    let outcome = (|| -> Result<PluginExportInventory> {
        let records_path = staging.join("records.ndjson");
        let mut records = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&records_path)?;
        for table in learning_schema::canonical_tables(plugin).expect("fixed plugin") {
            write_normalized_table(&connection, table, &mut records)?;
        }
        records.sync_all()?;
        let records_sha256 = file_digest(&records_path)?;
        let blobs = plugin_blob_inventory(plugin, &root, &connection, &staging)?;
        let manifest = json!({
            "format": 1,
            "plugin": plugin,
            "store_id": store_id,
            "revision": revision,
            "logical_hash": logical_hash,
            "records_sha256": records_sha256,
            "blobs": blobs,
        });
        let manifest_path = staging.join("manifest.json");
        let mut manifest_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)?;
        serde_json::to_writer_pretty(&mut manifest_file, &manifest)
            .map_err(|error| AppError::new("json_error", error.to_string()))?;
        manifest_file.sync_all()?;
        let export = validate_plugin_export(plugin, &staging)?;
        fs::rename(&staging, &destination)?;
        Ok(export)
    })();
    if outcome.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    outcome
}

fn plugin_live_export_path(home: &Path, session_id: &str, plugin: &str) -> PathBuf {
    home.join(".lwc/sync/plugins")
        .join(session_id)
        .join(format!("{plugin}-live"))
}

fn retain_plugin_history(
    session_id: &str,
    plugin: &str,
    export: &PluginExportInventory,
) -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let source = plugin_export_path(session_id, plugin, export)?;
    let history = home.join(".lwc/plugins").join(plugin).join("sync-history");
    ensure_real_directory(&history)?;
    let store = history.join(&export.store_id);
    ensure_real_directory(&store)?;
    let destination = store.join(export.revision.to_string());
    if destination.is_dir() {
        if validate_plugin_export(plugin, &destination)? == *export {
            return Ok(());
        }
        return Err(AppError::new(
            "sync_plugin_diverged",
            "plugin history revision differs from the resolved export",
        ));
    }
    let staging = history.join(format!(
        ".retain-{plugin}-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&staging)?;
    let outcome = (|| -> Result<()> {
        fs::copy(source.join("manifest.json"), staging.join("manifest.json"))?;
        fs::copy(
            source.join("records.ndjson"),
            staging.join("records.ndjson"),
        )?;
        let manifest: PluginManifest =
            serde_json::from_slice(&fs::read(source.join("manifest.json"))?)
                .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
        for blob in manifest.blobs {
            let relative = format!("blobs/sha256/{}/{}", &blob.sha256[..2], blob.sha256);
            let target = staging.join(&relative);
            fs::create_dir_all(target.parent().expect("blob parent"))?;
            fs::hard_link(source.join(&relative), target)?;
        }
        if validate_plugin_export(plugin, &staging)? != *export {
            return Err(AppError::new(
                "sync_plugin_changed",
                "plugin export changed while retaining its Sync baseline",
            ));
        }
        fs::rename(&staging, destination)?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    outcome
}

fn require_live_plugin_schema(plugin: &str, connection: &Connection) -> Result<()> {
    let canonical = learning_schema::canonical_tables(plugin).expect("fixed plugin");
    let derived = learning_schema::derived_tables(plugin);
    let tables = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for table in canonical {
        if !tables.iter().any(|actual| actual == table) {
            return Err(AppError::new(
                "sync_plugin_invalid",
                format!("canonical plugin table {table} is missing"),
            ));
        }
    }
    if let Some(table) = tables.iter().find(|table| {
        !canonical.contains(&table.as_str())
            && !["plugin_meta", "sync_receipts"].contains(&table.as_str())
            && !derived.contains(&table.as_str())
    }) {
        return Err(AppError::new(
            "sync_plugin_invalid",
            format!("plugin table {table} is outside the fixed normalized schema"),
        ));
    }
    Ok(())
}

fn write_normalized_table(
    connection: &Connection,
    table: &str,
    output: &mut impl Write,
) -> Result<()> {
    let mut schema = connection.prepare(&format!("PRAGMA table_info({})", quote_sql(table)))?;
    let columns = schema
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns.is_empty() || !columns.iter().any(|(_, primary)| *primary > 0) {
        return Err(AppError::new(
            "sync_plugin_invalid",
            format!("canonical table {table} has no stable primary key"),
        ));
    }
    let selected = columns
        .iter()
        .map(|(column, _)| quote_sql(column))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {selected} FROM {} ORDER BY {selected}",
        quote_sql(table)
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let mut values = serde_json::Map::new();
        let mut key = Vec::new();
        for (index, (column, primary)) in columns.iter().enumerate() {
            let value = normalized_sql_value(row.get_ref(index)?)?;
            if *primary > 0 {
                key.push((*primary, value.clone()));
            }
            values.insert(column.clone(), value);
        }
        key.sort_by_key(|(ordinal, _)| *ordinal);
        serde_json::to_writer(
            &mut *output,
            &json!({
                "table": table,
                "key": key.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
                "values": values,
            }),
        )
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
        output.write_all(b"\n")?;
    }
    Ok(())
}

fn normalized_sql_value(value: rusqlite::types::ValueRef<'_>) -> Result<Value> {
    Ok(match value {
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Integer(value) => json!(value),
        rusqlite::types::ValueRef::Real(value) => json!(value),
        rusqlite::types::ValueRef::Text(value) => Value::String(
            std::str::from_utf8(value)
                .map_err(|_| {
                    AppError::new("sync_plugin_invalid", "canonical TEXT is not valid UTF-8")
                })?
                .to_owned(),
        ),
        rusqlite::types::ValueRef::Blob(value) => json!({
            "$blob_hex": value.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        }),
    })
}

fn quote_sql(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn plugin_blob_inventory(
    plugin: &str,
    root: &Path,
    connection: &Connection,
    staging: &Path,
) -> Result<Vec<Value>> {
    if plugin != "book" {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare(
        "SELECT original_sha256 FROM books
         UNION SELECT normalized_sha256 FROM books WHERE normalized_sha256 IS NOT NULL
         ORDER BY 1",
    )?;
    let hashes = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut inventory = Vec::new();
    for hash in hashes {
        if !valid_sha256(&hash) {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "Book blob hash is invalid",
            ));
        }
        let source = root.join(format!("blobs/sha256/{}/{}", &hash[..2], hash));
        reject_plugin_file(&source)?;
        let relative = format!("blobs/sha256/{}/{}", &hash[..2], hash);
        let destination = staging.join(&relative);
        let blobs = staging.join("blobs");
        let sha = blobs.join("sha256");
        let prefix = sha.join(&hash[..2]);
        for directory in [&blobs, &sha, &prefix] {
            ensure_real_directory(directory)?;
        }
        fs::hard_link(&source, &destination).map_err(|error| {
            AppError::new(
                "sync_plugin_stage_failed",
                format!("failed to stage Book blob without copying it: {error}"),
            )
        })?;
        let bytes = fs::metadata(&destination)?.len();
        if file_digest(&destination)? != hash {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "Book blob hash changed",
            ));
        }
        inventory.push(json!({"sha256": hash, "bytes": bytes}));
    }
    Ok(inventory)
}

fn reject_plugin_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_plugin_export("unknown"));
    }
    Ok(())
}

fn reject_plugin_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(unsafe_plugin_export("unknown"));
    }
    Ok(())
}

fn unsafe_plugin_export(plugin: &str) -> AppError {
    AppError::new(
        "unsafe_sync_plugin_path",
        format!("{plugin} normalized export contains an unsafe path"),
    )
}

fn plugin_export_path(
    session_id: &str,
    plugin: &str,
    export: &PluginExportInventory,
) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let live = plugin_live_export_path(&home, session_id, plugin);
    if live.is_dir() && validate_plugin_export(plugin, &live)? == *export {
        return Ok(live);
    }
    let resolved = home
        .join(".lwc/sync/plugins")
        .join(session_id)
        .join(format!("{plugin}-resolved"));
    if resolved.is_dir() && validate_plugin_export(plugin, &resolved)? == *export {
        return Ok(resolved);
    }
    let history = home
        .join(".lwc/plugins")
        .join(plugin)
        .join("sync-history")
        .join(&export.store_id)
        .join(export.revision.to_string());
    if history.is_dir() && validate_plugin_export(plugin, &history)? == *export {
        return Ok(history);
    }
    Ok(home
        .join(".lwc/plugins")
        .join(plugin)
        .join("preserved")
        .join(&export.store_id)
        .join(export.revision.to_string()))
}

fn plugin_artifact_path(session_id: &str, plugin: &str, suffix: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let lwc = home.join(".lwc");
    let sync = lwc.join("sync");
    let plugins = sync.join("plugins");
    let root = plugins.join(session_id);
    for directory in [&lwc, &sync, &plugins, &root] {
        ensure_real_directory(directory)?;
    }
    Ok(root.join(format!("{plugin}-{suffix}.db")))
}

fn cleanup_plugin_session(session_id: &str) -> Result<()> {
    validate_session_id(session_id)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let root = home.join(".lwc/sync/plugins").join(session_id);
    if root.is_dir() {
        fs::remove_dir_all(root)?;
    } else if fs::symlink_metadata(&root).is_ok() {
        return Err(AppError::new(
            "sync_state_invalid",
            "plugin session cleanup path is not a real directory",
        ));
    }
    Ok(())
}

fn cleanup_plugin_session_on_peer(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
) -> Result<()> {
    let request = PeerRequest {
        protocol: PROTOCOL_VERSION,
        action: "plugin-cleanup".to_owned(),
        session_id: session_id.to_owned(),
        scope,
        directory: remote_directory.map(Path::to_path_buf),
        store_scope: None,
        payload_size: None,
        state_digest: None,
        expected: None,
        baseline_digest: None,
        requester_store_id: None,
        plugin: None,
    };
    let response = call_peer_value(host, &request)?;
    if response["protocol"] != PROTOCOL_VERSION
        || response["action"] != "plugin-cleaned"
        || response["session_id"] != session_id
    {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote plugin cleanup response does not match the request",
        ));
    }
    Ok(())
}

fn build_plugin_artifact(
    session_id: &str,
    plugin: &str,
    export: &PluginExportInventory,
    destination: &Path,
) -> Result<()> {
    reject_non_regular_artifact(destination)?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    let root = plugin_export_path(session_id, plugin, export)?;
    if validate_plugin_export(plugin, &root)? != *export {
        return Err(AppError::new(
            "sync_plugin_changed",
            "plugin export changed after the Sync handshake",
        ));
    }
    let connection = Connection::open(destination)?;
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         CREATE TABLE files(
           id INTEGER PRIMARY KEY,
           path TEXT NOT NULL UNIQUE,
           size INTEGER NOT NULL,
           sha256 TEXT NOT NULL,
           content BLOB NOT NULL
         );",
    )?;
    add_plugin_artifact_file(&connection, &root, "manifest.json")?;
    add_plugin_artifact_file(&connection, &root, "records.ndjson")?;
    let manifest: PluginManifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json"))?)
            .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
    for blob in manifest.blobs {
        add_plugin_artifact_file(
            &connection,
            &root,
            &format!("blobs/sha256/{}/{}", &blob.sha256[..2], blob.sha256),
        )?;
    }
    connection.execute_batch("VACUUM;")?;
    Ok(())
}

fn add_plugin_artifact_file(connection: &Connection, root: &Path, relative: &str) -> Result<()> {
    let source = root.join(relative);
    reject_plugin_file(&source)?;
    let size = fs::metadata(&source)?.len();
    let size_i64 = i64::try_from(size)
        .map_err(|_| AppError::new("sync_plugin_too_large", "plugin file exceeds SQLite limits"))?;
    let digest = file_digest(&source)?;
    connection.execute(
        "INSERT INTO files(path,size,sha256,content) VALUES(?1,?2,?3,zeroblob(?2))",
        params![relative, size_i64, digest],
    )?;
    let rowid = connection.query_row("SELECT id FROM files WHERE path=?1", [relative], |row| {
        row.get(0)
    })?;
    let mut blob = connection.blob_open("main", "files", "content", rowid, false)?;
    std::io::copy(&mut fs::File::open(source)?, &mut blob)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)] // One fixed Sync phase with explicit transport/session invariants.
fn reconcile_fixed_plugins(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    mode: SyncMode,
    local: &[PeerPlugin],
    remote: &[PeerPlugin],
    mut checkpoint: impl FnMut(&Value) -> Result<()>,
) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for plugin_id in FIXED_PLUGINS {
        #[cfg(debug_assertions)]
        if std::env::var("LWC_TEST_SYNC_FAIL_PLUGIN").as_deref() == Ok(plugin_id) {
            return Err(AppError::new(
                "sync_plugin_test_failure",
                format!("test failure before {plugin_id} publication"),
            ));
        }
        let local = local
            .iter()
            .find(|plugin| plugin.plugin_id == plugin_id)
            .ok_or_else(|| AppError::new("sync_protocol_invalid", "local plugin unit missing"))?;
        let remote = remote
            .iter()
            .find(|plugin| plugin.plugin_id == plugin_id)
            .ok_or_else(|| AppError::new("sync_protocol_invalid", "remote plugin unit missing"))?;
        let mut resolved_export = None;
        let mut local_published = false;
        let mut remote_published = false;
        let mut local_rebuilt = false;
        let mut remote_rebuilt = false;
        let mut local_receipt = false;
        let mut remote_receipt = false;
        let status = match (&local.export, &remote.export) {
            (Some(left), Some(right)) if !same_plugin_state(left, right) => {
                let baseline = common_plugin_baseline(local, remote);
                let resolved = if baseline
                    .as_ref()
                    .is_some_and(|baseline| same_plugin_state(baseline, left))
                {
                    right.clone()
                } else if baseline
                    .as_ref()
                    .is_some_and(|baseline| same_plugin_state(baseline, right))
                {
                    left.clone()
                } else {
                    merge_plugin_exports(
                        host,
                        scope,
                        remote_directory,
                        session_id,
                        plugin_id,
                        baseline.as_ref(),
                        left,
                        right,
                    )?
                };
                if mode != SyncMode::Push && !same_plugin_state(left, &resolved) {
                    if same_plugin_state(&resolved, right) {
                        fetch_plugin_from_peer(
                            host,
                            scope,
                            remote_directory,
                            session_id,
                            plugin_id,
                            &resolved,
                        )?;
                        local_published = true;
                        local_rebuilt = local.runtime_ready;
                        local_receipt = local.runtime_ready;
                    } else if local.runtime_ready {
                        local_rebuilt =
                            materialize_existing_plugin(session_id, plugin_id, &resolved)?;
                        local_published = local_rebuilt;
                        local_receipt = true;
                    }
                } else if mode != SyncMode::Push && local.runtime_ready {
                    local_rebuilt = materialize_existing_plugin(session_id, plugin_id, &resolved)?;
                    local_published = local_rebuilt;
                    local_receipt = true;
                }
                if mode != SyncMode::Pull && !same_plugin_state(right, &resolved) {
                    publish_plugin_to_peer(
                        host,
                        scope,
                        remote_directory,
                        session_id,
                        plugin_id,
                        &resolved,
                    )?;
                    remote_published = true;
                    remote_rebuilt = remote.runtime_ready;
                    remote_receipt = remote.runtime_ready;
                } else if mode != SyncMode::Pull && remote.runtime_ready {
                    remote_rebuilt = materialize_plugin_on_peer(
                        host,
                        scope,
                        remote_directory,
                        session_id,
                        plugin_id,
                        &resolved,
                    )?;
                    remote_published = remote_rebuilt;
                    remote_receipt = true;
                }
                retain_plugin_history(session_id, plugin_id, &resolved)?;
                resolved_export = Some(resolved);
                if local.runtime_ready && remote.runtime_ready {
                    "ready"
                } else {
                    "preserved_not_ready"
                }
            }
            (Some(export), Some(_)) => {
                resolved_export = Some(export.clone());
                if local.runtime_ready && mode != SyncMode::Push {
                    local_rebuilt = materialize_existing_plugin(session_id, plugin_id, export)?;
                    local_published = local_rebuilt;
                    local_receipt = true;
                }
                if remote.runtime_ready && mode != SyncMode::Pull {
                    remote_rebuilt = materialize_plugin_on_peer(
                        host,
                        scope,
                        remote_directory,
                        session_id,
                        plugin_id,
                        export,
                    )?;
                    remote_published = remote_rebuilt;
                    remote_receipt = true;
                }
                if local.runtime_ready && remote.runtime_ready {
                    "ready"
                } else {
                    "preserved_not_ready"
                }
            }
            (Some(export), None) if mode != SyncMode::Pull => {
                resolved_export = Some(export.clone());
                publish_plugin_to_peer(
                    host,
                    scope,
                    remote_directory,
                    session_id,
                    plugin_id,
                    export,
                )?;
                remote_published = true;
                remote_rebuilt = remote.runtime_ready;
                remote_receipt = remote.runtime_ready;
                "preserved_not_ready"
            }
            (None, Some(export)) if mode != SyncMode::Push => {
                resolved_export = Some(export.clone());
                fetch_plugin_from_peer(
                    host,
                    scope,
                    remote_directory,
                    session_id,
                    plugin_id,
                    export,
                )?;
                local_published = true;
                local_rebuilt = local.runtime_ready;
                local_receipt = local.runtime_ready;
                "preserved_not_ready"
            }
            (None, None) => "absent",
            _ => "retained_source_only",
        };
        if let Some(export) = resolved_export.as_ref() {
            retain_plugin_history(session_id, plugin_id, export)?;
        }
        let readback = resolved_export
            .as_ref()
            .map(|export| plugin_readback(session_id, plugin_id, export))
            .transpose()?
            .unwrap_or_else(|| json!({"record_counts": {}, "blob_hashes": []}));
        let result = json!({
            "plugin": plugin_id,
            "status": status,
            "local_runtime_ready": local.runtime_ready,
            "remote_runtime_ready": remote.runtime_ready,
            "local": local.export,
            "remote": remote.export,
            "record_counts": readback["record_counts"],
            "blob_hashes": readback["blob_hashes"],
            "publication": {
                "local": local_published,
                "remote": remote_published,
            },
            "rebuild": {
                "local": local_rebuilt,
                "remote": remote_rebuilt,
            },
            "receipt": {
                "local": local_receipt,
                "remote": remote_receipt,
            },
        });
        checkpoint(&result)?;
        results.push(result);
    }
    Ok(results)
}

fn plugin_readback(
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
) -> Result<Value> {
    let root = plugin_export_path(session_id, plugin_id, export)?;
    let records = read_normalized_plugin_records(plugin_id, &root)?;
    let mut counts = serde_json::Map::new();
    for table in learning_schema::canonical_tables(plugin_id).expect("fixed plugin") {
        counts.insert(
            (*table).to_owned(),
            json!(
                records
                    .iter()
                    .filter(|record| record.table == *table)
                    .count()
            ),
        );
    }
    let manifest: PluginManifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json"))?)
            .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
    Ok(json!({
        "record_counts": counts,
        "blob_hashes": manifest.blobs.into_iter().map(|blob| blob.sha256).collect::<Vec<_>>(),
    }))
}

fn validate_fixed_plugin_preflight(
    local: &[PeerPlugin],
    remote: &[PeerPlugin],
) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for plugin_id in FIXED_PLUGINS {
        let local = local
            .iter()
            .find(|plugin| plugin.plugin_id == plugin_id)
            .ok_or_else(|| AppError::new("sync_protocol_invalid", "local plugin unit missing"))?;
        let remote = remote
            .iter()
            .find(|plugin| plugin.plugin_id == plugin_id)
            .ok_or_else(|| AppError::new("sync_protocol_invalid", "remote plugin unit missing"))?;
        if let (Some(left), Some(right)) = (&local.export, &remote.export)
            && !same_plugin_state(left, right)
        {
            let baseline = common_plugin_baseline(local, remote);
            let valid_descendant = baseline.as_ref().map_or_else(
                || left.store_id == right.store_id && left.revision == right.revision,
                |baseline| {
                    left.store_id == baseline.store_id
                        && right.store_id == baseline.store_id
                        && (same_plugin_state(left, baseline) || left.revision > baseline.revision)
                        && (same_plugin_state(right, baseline)
                            || right.revision > baseline.revision)
                },
            );
            if !valid_descendant {
                let code = if left.store_id == right.store_id && left.revision == right.revision {
                    "sync_plugin_changed"
                } else {
                    "sync_plugin_diverged"
                };
                return Err(AppError::new(
                    code,
                    format!("{plugin_id} normalized exports diverged before Wiki publication"),
                )
                .with_details(json!({"local": left, "remote": right, "baseline": baseline})));
            }
        }
        results.push(json!({
            "plugin": plugin_id,
            "status": "preflight_complete",
            "local_runtime_ready": local.runtime_ready,
            "remote_runtime_ready": remote.runtime_ready,
            "local": local.export,
            "remote": remote.export,
        }));
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
fn merge_plugin_exports(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    plugin_id: &str,
    baseline: Option<&PluginExportInventory>,
    local: &PluginExportInventory,
    remote: &PluginExportInventory,
) -> Result<PluginExportInventory> {
    if local.store_id != remote.store_id
        || baseline.is_some_and(|baseline| baseline.store_id != local.store_id)
    {
        return Err(AppError::new(
            "sync_plugin_diverged",
            format!("{plugin_id} StoreIdentity changed after the common baseline"),
        ));
    }
    let baseline_root = baseline
        .map(|baseline| plugin_export_path(session_id, plugin_id, baseline))
        .transpose()?;
    let local_root = plugin_export_path(session_id, plugin_id, local)?;
    let remote_artifact = plugin_artifact_path(session_id, plugin_id, "merge-remote")?;
    download_plugin_export(
        host,
        scope,
        remote_directory,
        session_id,
        plugin_id,
        remote,
        &remote_artifact,
    )?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let remote_root = home
        .join(".lwc/sync/plugins")
        .join(session_id)
        .join(format!("{plugin_id}-merge-remote"));
    extract_plugin_artifact(plugin_id, remote, &remote_artifact, &remote_root)?;

    let base = baseline_root.as_ref().map_or_else(
        || Ok(std::collections::BTreeMap::new()),
        |root| normalized_record_map(read_normalized_plugin_records(plugin_id, root)?),
    )?;
    let left = normalized_record_map(read_normalized_plugin_records(plugin_id, &local_root)?)?;
    let right = normalized_record_map(read_normalized_plugin_records(plugin_id, &remote_root)?)?;
    let keys = base
        .keys()
        .chain(left.keys())
        .chain(right.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut merged = std::collections::BTreeMap::new();
    let mut conflicts = Vec::new();
    for key in keys.iter() {
        let base_value = base.get(key);
        let left_value = left.get(key);
        let right_value = right.get(key);
        let resolved = if left_value == right_value {
            left_value
        } else if left_value == base_value {
            right_value
        } else if right_value == base_value {
            left_value
        } else {
            conflicts.push(key.clone());
            continue;
        };
        if let Some(record) = resolved {
            merged.insert(key.clone(), record.clone());
        }
    }
    if !conflicts.is_empty() {
        return Err(AppError::new(
            "sync_plugin_conflict",
            format!("{plugin_id} has concurrent changes to the same canonical records"),
        )
        .with_details(json!({"conflicts": conflicts})));
    }

    let destination = home
        .join(".lwc/sync/plugins")
        .join(session_id)
        .join(format!("{plugin_id}-resolved"));
    if destination.exists() {
        fs::remove_dir_all(&destination)?;
    }
    fs::create_dir(&destination)?;
    let outcome = (|| -> Result<PluginExportInventory> {
        let mut records = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination.join("records.ndjson"))?;
        for table in learning_schema::canonical_tables(plugin_id).expect("fixed plugin") {
            for record in merged.values().filter(|record| record.table == *table) {
                serde_json::to_writer(&mut records, record)
                    .map_err(|error| AppError::new("json_error", error.to_string()))?;
                records.write_all(b"\n")?;
            }
        }
        records.sync_all()?;
        let records_sha256 = file_digest(&destination.join("records.ndjson"))?;
        let mut blob_roots = vec![local_root.as_path(), remote_root.as_path()];
        if let Some(root) = baseline_root.as_deref() {
            blob_roots.push(root);
        }
        let blobs = merge_plugin_blobs(plugin_id, &blob_roots, &destination)?;
        let revision = local
            .revision
            .max(remote.revision)
            .checked_add(1)
            .ok_or_else(|| AppError::new("sync_plugin_invalid", "plugin revision overflow"))?;
        let logical_hash = preview_plugin_logical_hash(plugin_id, merged.values())?;
        let manifest = json!({
            "format": 1,
            "plugin": plugin_id,
            "store_id": local.store_id,
            "revision": revision,
            "records_sha256": records_sha256,
            "logical_hash": logical_hash,
            "blobs": blobs,
        });
        fs::write(
            destination.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)
                .map_err(|error| AppError::new("json_error", error.to_string()))?,
        )?;
        validate_plugin_export(plugin_id, &destination)
    })();
    if outcome.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    outcome
}

fn normalized_record_map(
    records: Vec<NormalizedPluginRecord>,
) -> Result<std::collections::BTreeMap<String, NormalizedPluginRecord>> {
    let mut mapped = std::collections::BTreeMap::new();
    for record in records {
        let key = serde_json::to_string(&json!([&record.table, &record.key]))
            .map_err(|error| AppError::new("json_error", error.to_string()))?;
        if mapped.insert(key, record).is_some() {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "normalized plugin export contains a duplicate canonical key",
            ));
        }
    }
    Ok(mapped)
}

fn preview_plugin_logical_hash<'a>(
    plugin_id: &str,
    records: impl Iterator<Item = &'a NormalizedPluginRecord>,
) -> Result<String> {
    learning_schema::canonical_logical_hash_from_normalized(
        plugin_id,
        records.map(|record| (record.table.as_str(), &record.values)),
    )
    .map_err(|message| AppError::new("sync_plugin_invalid", message))
}

fn merge_plugin_blobs(plugin_id: &str, roots: &[&Path], destination: &Path) -> Result<Vec<Value>> {
    if plugin_id != "book" {
        return Ok(Vec::new());
    }
    let mut blobs = std::collections::BTreeMap::new();
    for root in roots {
        let manifest: PluginManifest =
            serde_json::from_slice(&fs::read(root.join("manifest.json"))?)
                .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
        for blob in manifest.blobs {
            if let Some(bytes) = blobs.insert(blob.sha256.clone(), blob.bytes)
                && bytes != blob.bytes
            {
                return Err(AppError::new(
                    "sync_plugin_diverged",
                    "Book blob inventory differs for the same content hash",
                ));
            }
        }
    }
    let mut inventory = Vec::new();
    for (hash, bytes) in blobs {
        let relative = format!("blobs/sha256/{}/{}", &hash[..2], hash);
        let source = roots
            .iter()
            .map(|root| root.join(&relative))
            .find(|path| path.is_file())
            .ok_or_else(|| AppError::new("sync_plugin_invalid", "Book blob is missing"))?;
        let target = destination.join(&relative);
        fs::create_dir_all(target.parent().expect("blob parent"))?;
        fs::hard_link(&source, &target).or_else(|_| fs::copy(&source, &target).map(|_| ()))?;
        inventory.push(json!({"sha256": hash, "bytes": bytes}));
    }
    Ok(inventory)
}

fn common_plugin_baseline(
    local: &PeerPlugin,
    remote: &PeerPlugin,
) -> Option<PluginExportInventory> {
    local
        .history
        .iter()
        .filter(|left| {
            remote
                .history
                .iter()
                .any(|right| same_plugin_state(right, left))
        })
        .max_by_key(|export| export.revision)
        .cloned()
}

fn publish_plugin_to_peer(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
) -> Result<()> {
    let artifact = plugin_artifact_path(session_id, plugin_id, "publish")?;
    build_plugin_artifact(session_id, plugin_id, export, &artifact)?;
    let request = PeerRequest {
        protocol: PROTOCOL_VERSION,
        action: "plugin-publish".to_owned(),
        session_id: session_id.to_owned(),
        scope,
        directory: remote_directory.map(Path::to_path_buf),
        store_scope: None,
        payload_size: Some(fs::metadata(&artifact)?.len()),
        state_digest: Some(file_digest(&artifact)?),
        expected: None,
        baseline_digest: None,
        requester_store_id: None,
        plugin: Some(PluginRequest {
            plugin_id: plugin_id.to_owned(),
            store_id: export.store_id.clone(),
            revision: export.revision,
            records_sha256: export.records_sha256.clone(),
            logical_hash: export.logical_hash.clone(),
        }),
    };
    call_peer_publish(host, &request, &artifact)?;
    Ok(())
}

fn materialize_plugin_on_peer(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
) -> Result<bool> {
    let request = PeerRequest {
        protocol: PROTOCOL_VERSION,
        action: "plugin-materialize".to_owned(),
        session_id: session_id.to_owned(),
        scope,
        directory: remote_directory.map(Path::to_path_buf),
        store_scope: None,
        payload_size: None,
        state_digest: None,
        expected: None,
        baseline_digest: None,
        requester_store_id: None,
        plugin: Some(PluginRequest {
            plugin_id: plugin_id.to_owned(),
            store_id: export.store_id.clone(),
            revision: export.revision,
            records_sha256: export.records_sha256.clone(),
            logical_hash: export.logical_hash.clone(),
        }),
    };
    let response = call_peer_value(host, &request)?;
    if response["protocol"] != PROTOCOL_VERSION
        || response["action"] != "plugin-materialized"
        || response["session_id"] != session_id
        || response["plugin"] != plugin_id
        || !response["rebuilt"].is_boolean()
    {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote plugin materialization response does not match the request",
        ));
    }
    Ok(response["rebuilt"] == true)
}

fn materialize_existing_plugin(
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
) -> Result<bool> {
    let root = plugin_export_path(session_id, plugin_id, export)?;
    materialize_plugin_export(session_id, plugin_id, export, &root)
}

fn fetch_plugin_from_peer(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
) -> Result<()> {
    let request = PeerRequest {
        protocol: PROTOCOL_VERSION,
        action: "plugin-export".to_owned(),
        session_id: session_id.to_owned(),
        scope,
        directory: remote_directory.map(Path::to_path_buf),
        store_scope: None,
        payload_size: None,
        state_digest: None,
        expected: None,
        baseline_digest: None,
        requester_store_id: None,
        plugin: Some(PluginRequest {
            plugin_id: plugin_id.to_owned(),
            store_id: export.store_id.clone(),
            revision: export.revision,
            records_sha256: export.records_sha256.clone(),
            logical_hash: export.logical_hash.clone(),
        }),
    };
    let artifact = plugin_artifact_path(session_id, plugin_id, "export")?;
    call_peer_plugin_export(host, &request, &artifact)?;
    publish_plugin_artifact(
        session_id,
        plugin_id,
        request.plugin.as_ref().expect("plugin request"),
        &artifact,
    )?;
    Ok(())
}

fn download_plugin_export(
    host: &str,
    scope: Scope,
    remote_directory: Option<&Path>,
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
    artifact: &Path,
) -> Result<()> {
    let request = PeerRequest {
        protocol: PROTOCOL_VERSION,
        action: "plugin-export".to_owned(),
        session_id: session_id.to_owned(),
        scope,
        directory: remote_directory.map(Path::to_path_buf),
        store_scope: None,
        payload_size: None,
        state_digest: None,
        expected: None,
        baseline_digest: None,
        requester_store_id: None,
        plugin: Some(PluginRequest {
            plugin_id: plugin_id.to_owned(),
            store_id: export.store_id.clone(),
            revision: export.revision,
            records_sha256: export.records_sha256.clone(),
            logical_hash: export.logical_hash.clone(),
        }),
    };
    call_peer_plugin_export(host, &request, artifact)?;
    Ok(())
}

fn extract_plugin_artifact(
    plugin_id: &str,
    expected: &PluginExportInventory,
    artifact: &Path,
    destination: &Path,
) -> Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::create_dir(destination)?;
    let connection =
        Connection::open_with_flags(artifact, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let files = connection
        .prepare("SELECT id,path,size,sha256 FROM files ORDER BY path")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let outcome = (|| -> Result<()> {
        for (id, relative, size, digest) in files {
            if size < 0 || !valid_plugin_artifact_relative(&relative) || !valid_sha256(&digest) {
                return Err(AppError::new(
                    "sync_plugin_invalid",
                    "plugin merge artifact inventory is invalid",
                ));
            }
            let target = destination.join(&relative);
            fs::create_dir_all(target.parent().expect("artifact parent"))?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            let mut blob = connection.blob_open("main", "files", "content", id, true)?;
            let copied = std::io::copy(&mut blob, &mut output)?;
            output.sync_all()?;
            if copied != size as u64 || file_digest(&target)? != digest {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    "plugin merge artifact checksum differs",
                ));
            }
        }
        if validate_plugin_export(plugin_id, destination)? != *expected {
            return Err(AppError::new(
                "sync_plugin_changed",
                "plugin merge artifact changed after the handshake",
            ));
        }
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    outcome
}

fn publish_plugin_artifact(
    session_id: &str,
    plugin_id: &str,
    expected: &PluginRequest,
    artifact: &Path,
) -> Result<bool> {
    let connection =
        Connection::open_with_flags(artifact, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let files = connection
        .prepare("SELECT id,path,size,sha256 FROM files ORDER BY path")?
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !files.iter().any(|(_, path, _, _)| path == "manifest.json")
        || !files.iter().any(|(_, path, _, _)| path == "records.ndjson")
        || files
            .iter()
            .any(|(_, path, _, _)| !valid_plugin_artifact_relative(path))
    {
        return Err(AppError::new(
            "sync_plugin_invalid",
            "normalized plugin artifact has an invalid file inventory",
        ));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let plugin_root = home.join(".lwc/plugins").join(plugin_id);
    let preserved = plugin_root.join("preserved");
    for directory in [
        home.join(".lwc"),
        home.join(".lwc/plugins"),
        plugin_root,
        preserved.clone(),
    ] {
        ensure_real_directory(&directory)?;
    }
    let staging = preserved.join(format!(
        ".receive-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&staging)?;
    let outcome = (|| -> Result<bool> {
        for (id, relative, size, digest) in &files {
            if !valid_sha256(digest) {
                return Err(AppError::new(
                    "sync_plugin_invalid",
                    "artifact hash is invalid",
                ));
            }
            let destination = staging.join(relative);
            if let Some(parent) = destination.parent() {
                let relative_parent = parent.strip_prefix(&staging).unwrap();
                let mut current = staging.clone();
                for component in relative_parent.components() {
                    current.push(component);
                    ensure_real_directory(&current)?;
                }
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            let mut blob = connection.blob_open("main", "files", "content", *id, true)?;
            let copied = std::io::copy(&mut blob, &mut output)?;
            output.sync_all()?;
            if *size < 0 || copied != *size as u64 || file_digest(&destination)? != *digest {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    "normalized plugin artifact file checksum differs",
                ));
            }
        }
        let export = validate_plugin_export(plugin_id, &staging)?;
        if export.store_id != expected.store_id
            || export.revision != expected.revision
            || export.records_sha256 != expected.records_sha256
            || export.logical_hash != expected.logical_hash
        {
            return Err(AppError::new(
                "sync_plugin_changed",
                "plugin artifact does not match the handshake inventory",
            ));
        }
        let store = preserved.join(&export.store_id);
        ensure_real_directory(&store)?;
        let destination = store.join(export.revision.to_string());
        if destination.is_dir() {
            if validate_plugin_export(plugin_id, &destination)? == export {
                if plugin_runtime_ready(plugin_id)? {
                    materialize_plugin_export(session_id, plugin_id, &export, &destination)?;
                }
                return Ok(false);
            }
            return Err(AppError::new(
                "sync_plugin_diverged",
                "plugin revision already exists with different canonical content",
            ));
        }
        fs::rename(&staging, &destination)?;
        if plugin_runtime_ready(plugin_id)? {
            materialize_plugin_export(session_id, plugin_id, &export, &destination)?;
        }
        Ok(true)
    })();
    if outcome.is_err() || matches!(outcome, Ok(false)) {
        let _ = fs::remove_dir_all(&staging);
    }
    outcome
}

fn materialize_plugin_export(
    session_id: &str,
    plugin_id: &str,
    export: &PluginExportInventory,
    source: &Path,
) -> Result<bool> {
    if validate_plugin_export(plugin_id, source)? != *export {
        return Err(AppError::new(
            "sync_plugin_changed",
            "plugin export changed before canonical materialization",
        ));
    }
    run_plugin_status(plugin_id)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::new("home_unavailable", "HOME is not configured"))?;
    let root = home.join(".lwc/plugins").join(plugin_id);
    materialize_plugin_blobs(plugin_id, source, &root)?;
    let database = root.join("data.sqlite3");
    reject_plugin_file(&database)?;
    let mut connection = Connection::open(&database)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys=ON;")?;
    require_live_plugin_schema(plugin_id, &connection)?;
    let current_store_id = connection.query_row(
        "SELECT value FROM plugin_meta WHERE key='store_id'",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let current_revision = connection
        .query_row(
            "SELECT value FROM plugin_meta WHERE key='revision'",
            [],
            |row| row.get::<_, String>(0),
        )?
        .parse::<u64>()
        .map_err(|_| AppError::new("sync_plugin_invalid", "plugin revision is invalid"))?;
    let canonical_count = canonical_record_count(plugin_id, &connection)?;
    let mutation_count = connection.query_row("SELECT COUNT(*) FROM requests", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let receipt_count = connection.query_row("SELECT COUNT(*) FROM sync_receipts", [], |row| {
        row.get::<_, i64>(0)
    })?;
    if current_store_id != export.store_id
        && (current_revision != 0 || mutation_count != 0 || receipt_count != 0)
    {
        return Err(AppError::new(
            "sync_plugin_diverged",
            "runtime StoreIdentity differs from the normalized export",
        ));
    }
    if current_store_id == export.store_id && current_revision > export.revision {
        return Err(AppError::new(
            "sync_plugin_changed",
            "runtime revision advanced beyond the normalized export",
        ));
    }
    if current_store_id == export.store_id && current_revision == export.revision {
        let current_hash = learning_schema::canonical_logical_hash(plugin_id, &connection)
            .map_err(|message| AppError::new("sync_plugin_invalid", message))?;
        if current_hash == export.logical_hash {
            record_ready_plugin_receipt(&mut connection, plugin_id, session_id, export)?;
            discard_stale_live_export(&home, session_id, plugin_id, source)?;
            return Ok(false);
        }
        if canonical_count != 0 {
            return Err(AppError::new(
                "sync_plugin_changed",
                format!(
                    "{plugin_id} canonical content changed without a revision bump at {current_revision}: current {current_hash}, expected {}",
                    export.logical_hash
                ),
            )
            .with_details(json!({
                "plugin": plugin_id,
                "session_id": session_id,
                "revision": current_revision,
                "current_hash": current_hash,
                "expected_hash": export.logical_hash,
            })));
        }
    }

    let records = read_normalized_plugin_records(plugin_id, source)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch("PRAGMA defer_foreign_keys=ON;")?;
    for table in learning_schema::canonical_tables(plugin_id)
        .expect("fixed plugin")
        .iter()
        .rev()
    {
        tx.execute(&format!("DELETE FROM {}", quote_sql(table)), [])?;
    }
    for record in records {
        insert_normalized_plugin_record(&tx, &record)?;
    }
    tx.execute(
        "UPDATE plugin_meta SET value=?1 WHERE key='store_id'",
        [&export.store_id],
    )?;
    tx.execute(
        "UPDATE plugin_meta SET value=?1 WHERE key='revision'",
        [export.revision.to_string()],
    )?;
    let actual = learning_schema::canonical_logical_hash(plugin_id, &tx)
        .map_err(|message| AppError::new("sync_plugin_invalid", message))?;
    if actual != export.logical_hash {
        return Err(AppError::new(
            "sync_plugin_invalid",
            "materialized canonical plugin hash differs from its manifest",
        ));
    }
    tx.commit()?;
    run_plugin_status(plugin_id)?;
    record_ready_plugin_receipt(&mut connection, plugin_id, session_id, export)?;
    discard_stale_live_export(&home, session_id, plugin_id, source)?;
    Ok(true)
}

fn discard_stale_live_export(
    home: &Path,
    session_id: &str,
    plugin_id: &str,
    materialized_source: &Path,
) -> Result<()> {
    let live = plugin_live_export_path(home, session_id, plugin_id);
    if live != materialized_source && live.is_dir() {
        fs::remove_dir_all(live)?;
    }
    Ok(())
}

fn canonical_record_count(plugin_id: &str, connection: &Connection) -> Result<u64> {
    let mut count = 0_u64;
    for table in learning_schema::canonical_tables(plugin_id).expect("fixed plugin") {
        let table_count = connection.query_row(
            &format!("SELECT COUNT(*) FROM {}", quote_sql(table)),
            [],
            |row| row.get::<_, i64>(0),
        )?;
        count =
            count
                .checked_add(u64::try_from(table_count).map_err(|_| {
                    AppError::new("sync_plugin_invalid", "plugin row count is invalid")
                })?)
                .ok_or_else(|| AppError::new("sync_plugin_invalid", "plugin row count overflow"))?;
    }
    Ok(count)
}

fn read_normalized_plugin_records(
    plugin_id: &str,
    source: &Path,
) -> Result<Vec<NormalizedPluginRecord>> {
    let input = fs::File::open(source.join("records.ndjson"))?;
    let mut records = Vec::new();
    for line in BufReader::new(input).lines() {
        let line = line?;
        if line.is_empty() {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "normalized plugin records contain an empty line",
            ));
        }
        let record: NormalizedPluginRecord = serde_json::from_str(&line)
            .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
        if !learning_schema::canonical_tables(plugin_id)
            .expect("fixed plugin")
            .contains(&record.table.as_str())
            || record.key.is_empty()
        {
            return Err(AppError::new(
                "sync_plugin_invalid",
                "normalized plugin record is outside the fixed schema",
            ));
        }
        records.push(record);
    }
    Ok(records)
}

fn insert_normalized_plugin_record(
    tx: &rusqlite::Transaction<'_>,
    record: &NormalizedPluginRecord,
) -> Result<()> {
    let mut schema = tx.prepare(&format!("PRAGMA table_info({})", quote_sql(&record.table)))?;
    let columns = schema
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(schema);
    if columns.len() != record.values.len()
        || columns
            .iter()
            .any(|column| !record.values.contains_key(column))
    {
        return Err(AppError::new(
            "sync_plugin_invalid",
            format!("normalized {} record columns differ", record.table),
        ));
    }
    let values = columns
        .iter()
        .map(|column| normalized_json_to_sql(&record.values[column]))
        .collect::<Result<Vec<_>>>()?;
    let placeholders = (1..=columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",");
    tx.execute(
        &format!(
            "INSERT INTO {}({}) VALUES({placeholders})",
            quote_sql(&record.table),
            columns
                .iter()
                .map(|column| quote_sql(column))
                .collect::<Vec<_>>()
                .join(",")
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

fn normalized_json_to_sql(value: &Value) -> Result<rusqlite::types::Value> {
    match value {
        Value::Null => Ok(rusqlite::types::Value::Null),
        Value::Number(value) if value.is_i64() => Ok(rusqlite::types::Value::Integer(
            value.as_i64().expect("checked integer"),
        )),
        Value::Number(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(rusqlite::types::Value::Real)
            .ok_or_else(|| AppError::new("sync_plugin_invalid", "invalid normalized number")),
        Value::String(value) => Ok(rusqlite::types::Value::Text(value.clone())),
        Value::Object(value) if value.len() == 1 && value.contains_key("$blob_hex") => {
            let value = value["$blob_hex"]
                .as_str()
                .ok_or_else(|| AppError::new("sync_plugin_invalid", "invalid normalized blob"))?;
            if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(AppError::new(
                    "sync_plugin_invalid",
                    "invalid normalized blob",
                ));
            }
            let bytes = value
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                        .ok_or_else(|| {
                            AppError::new("sync_plugin_invalid", "invalid normalized blob")
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(rusqlite::types::Value::Blob(bytes))
        }
        _ => Err(AppError::new(
            "sync_plugin_invalid",
            "unsupported normalized SQLite value",
        )),
    }
}

fn materialize_plugin_blobs(plugin_id: &str, source: &Path, root: &Path) -> Result<()> {
    if plugin_id != "book" {
        return Ok(());
    }
    let manifest: PluginManifest = serde_json::from_slice(&fs::read(source.join("manifest.json"))?)
        .map_err(|error| AppError::new("sync_plugin_invalid", error.to_string()))?;
    for blob in manifest.blobs {
        let relative = format!("blobs/sha256/{}/{}", &blob.sha256[..2], blob.sha256);
        let source = source.join(&relative);
        let destination = root.join(&relative);
        if destination.is_file() {
            if file_digest(&destination)? != blob.sha256 {
                return Err(AppError::new(
                    "sync_plugin_diverged",
                    "existing Book blob differs from its content hash",
                ));
            }
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let staging = destination.with_extension(format!(
            "sync-{}-{}",
            std::process::id(),
            STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::hard_link(&source, &staging).or_else(|_| fs::copy(&source, &staging).map(|_| ()))?;
        if file_digest(&staging)? != blob.sha256 {
            let _ = fs::remove_file(&staging);
            return Err(AppError::new(
                "sync_checksum_mismatch",
                "staged Book blob differs from its content hash",
            ));
        }
        fs::rename(staging, destination)?;
    }
    Ok(())
}

fn run_plugin_status(plugin_id: &str) -> Result<()> {
    let status = match plugin_id {
        "tutor" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Tutor)?,
        "book" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Book)?,
        "practice" => crate::learning_runtime::status(crate::learning_runtime::Plugin::Practice)?,
        _ => return Err(AppError::new("sync_plugin_invalid", "unknown fixed plugin")),
    };
    if status["installed"] != true {
        return Err(AppError::new(
            "learning_runtime_missing",
            format!("{plugin_id} runtime is not installed"),
        ));
    }
    let runtime = status["runtime"]
        .as_str()
        .ok_or_else(|| AppError::new("sync_plugin_invalid", "runtime path is invalid"))?;
    let output = Command::new(Path::new(runtime).join(format!("lwc-{plugin_id}")))
        .arg("status")
        .env("LWC_PLUGIN_SKIP_UPDATE", "1")
        .env("LWC_PLUGIN_NO_BACKGROUND", "1")
        .output()?;
    if !output.status.success() {
        return Err(AppError::new(
            "sync_plugin_materialize_failed",
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(())
}

fn record_ready_plugin_receipt(
    connection: &mut Connection,
    plugin_id: &str,
    session_id: &str,
    export: &PluginExportInventory,
) -> Result<()> {
    let plugin = match plugin_id {
        "tutor" => crate::learning::Plugin::Tutor,
        "book" => crate::learning::Plugin::Book,
        "practice" => crate::learning::Plugin::Practice,
        _ => return Err(AppError::new("sync_plugin_invalid", "unknown fixed plugin")),
    };
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::learning::record_sync_receipt(
        &tx,
        plugin,
        session_id,
        export.revision as i64,
        export.revision as i64,
        &export.logical_hash,
        "ready",
    )
    .map_err(|error| AppError::new(error.code(), error.message()))?;
    tx.commit()?;
    Ok(())
}

fn valid_plugin_artifact_relative(path: &str) -> bool {
    if matches!(path, "manifest.json" | "records.ndjson") {
        return true;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    parts.len() == 4
        && parts[0] == "blobs"
        && parts[1] == "sha256"
        && parts[2].len() == 2
        && valid_sha256(parts[3])
        && parts[3].starts_with(parts[2])
}

pub(crate) fn peer() -> Result<Value> {
    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());
    let bytes = read_bounded_line(&mut stdin, MAX_PROTOCOL_BYTES)?;
    let request: PeerRequest = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::new(
            "sync_protocol_invalid",
            format!("invalid Sync peer request: {error}"),
        )
    })?;
    if request.protocol != PROTOCOL_VERSION {
        return Err(AppError::new(
            "sync_protocol_mismatch",
            format!(
                "peer protocol {} is incompatible with {PROTOCOL_VERSION}",
                request.protocol
            ),
        ));
    }
    validate_session_id(&request.session_id)?;
    let directory = explicit_peer_directory(request.scope, request.directory.as_deref())?;
    let paths = peer_store_paths(request.scope, &directory)?;
    if request.action == "handshake" {
        let mut stores = Vec::with_capacity(paths.len());
        for path in paths {
            let missing = !canonical_store_exists(&path.path)?;
            let identity = if missing {
                missing_store_identity()
            } else {
                Store::open_for_read(scope_name(path.scope), &path.path)?.identity()?
            };
            stores.push(PeerStore {
                scope: path.scope,
                identity,
                missing,
            });
        }
        return serde_json::to_value(PeerResponse {
            protocol: PROTOCOL_VERSION,
            action: "handshake".to_string(),
            session_id: request.session_id.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            stores,
            plugins: fixed_plugin_inventory(&request.session_id)?,
        })
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()));
    }
    if request.action == "plugin-cleanup" {
        cleanup_plugin_session(&request.session_id)?;
        return Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "action": "plugin-cleaned",
            "session_id": request.session_id,
        }));
    }
    if matches!(
        request.action.as_str(),
        "plugin-export" | "plugin-publish" | "plugin-materialize"
    ) {
        let plugin = request.plugin.as_ref().ok_or_else(|| {
            AppError::new("sync_protocol_invalid", "fixed plugin request is missing")
        })?;
        if !FIXED_PLUGINS.contains(&plugin.plugin_id.as_str())
            || !valid_plugin_store_id(&plugin.store_id)
            || !valid_sha256(&plugin.records_sha256)
            || !valid_sha256(&plugin.logical_hash)
        {
            return Err(AppError::new(
                "sync_protocol_invalid",
                "fixed plugin request identity is invalid",
            ));
        }
        if matches!(
            request.action.as_str(),
            "plugin-export" | "plugin-materialize"
        ) {
            let entry = fixed_plugin_inventory(&request.session_id)?
                .into_iter()
                .find(|entry| entry.plugin_id == plugin.plugin_id)
                .ok_or_else(|| {
                    AppError::new("sync_plugin_changed", "remote plugin export is missing")
                })?;
            let inventory = entry
                .history
                .into_iter()
                .find(|inventory| {
                    inventory.store_id == plugin.store_id
                        && inventory.revision == plugin.revision
                        && inventory.logical_hash == plugin.logical_hash
                        && (request.action == "plugin-materialize"
                            || inventory.records_sha256 == plugin.records_sha256)
                })
                .ok_or_else(|| {
                    AppError::new("sync_plugin_changed", "remote plugin export changed")
                })?;
            if inventory.store_id != plugin.store_id
                || inventory.revision != plugin.revision
                || (request.action != "plugin-materialize"
                    && inventory.records_sha256 != plugin.records_sha256)
                || inventory.logical_hash != plugin.logical_hash
            {
                return Err(AppError::new(
                    "sync_plugin_changed",
                    "remote plugin export changed after the handshake",
                ));
            }
            if request.action == "plugin-materialize" {
                let rebuilt = materialize_existing_plugin(
                    &request.session_id,
                    &plugin.plugin_id,
                    &inventory,
                )?;
                return Ok(json!({
                    "protocol": PROTOCOL_VERSION,
                    "action": "plugin-materialized",
                    "session_id": request.session_id,
                    "plugin": plugin.plugin_id,
                    "rebuilt": rebuilt,
                }));
            }
            let artifact =
                plugin_artifact_path(&request.session_id, &plugin.plugin_id, "peer-export")?;
            build_plugin_artifact(
                &request.session_id,
                &plugin.plugin_id,
                &inventory,
                &artifact,
            )?;
            let transfer = PeerPluginTransfer {
                protocol: PROTOCOL_VERSION,
                action: "plugin-export".to_owned(),
                session_id: request.session_id,
                plugin: plugin.clone(),
                size: fs::metadata(&artifact)?.len(),
                artifact_digest: file_digest(&artifact)?,
            };
            let stdout = std::io::stdout();
            let mut stdout = stdout.lock();
            serde_json::to_writer(&mut stdout, &transfer)
                .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
            stdout.write_all(b"\n")?;
            std::io::copy(&mut fs::File::open(artifact)?, &mut stdout)?;
            stdout.flush()?;
            return Ok(Value::Null);
        }
        let size = request
            .payload_size
            .filter(|size| *size <= MAX_TRANSFER_BYTES)
            .ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "plugin payload size is invalid")
            })?;
        let digest = request.state_digest.as_deref().ok_or_else(|| {
            AppError::new("sync_protocol_invalid", "plugin artifact digest is missing")
        })?;
        let artifact =
            plugin_artifact_path(&request.session_id, &plugin.plugin_id, "peer-publish")?;
        if artifact.is_file() {
            fs::remove_file(&artifact)?;
        }
        receive_exact_file(&mut stdin, &artifact, size)?;
        require_payload_eof(&mut stdin)?;
        if file_digest(&artifact)? != digest {
            return Err(AppError::new(
                "sync_checksum_mismatch",
                "published plugin artifact checksum differs",
            ));
        }
        let committed =
            publish_plugin_artifact(&request.session_id, &plugin.plugin_id, plugin, &artifact)?;
        return Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "action": "plugin-published",
            "session_id": request.session_id,
            "plugin": plugin.plugin_id,
            "committed": committed,
        }));
    }
    if paths.len() != 1 || request.store_scope != Some(request.scope) {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "export and publish require one exact store scope",
        ));
    }
    let path = &paths[0];
    if request.action == "codegraph" {
        return Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "action": "codegraph_rebuilt",
            "session_id": request.session_id,
            "scope": path.scope,
            "derived": rebuild_codegraph(path),
        }));
    }
    if request.action == "rebuild" {
        let state_digest = request.state_digest.as_deref().ok_or_else(|| {
            AppError::new(
                "sync_protocol_invalid",
                "remote rebuild requires the published state digest",
            )
        })?;
        let publication = sync_publication_receipt(&path.path, &request.session_id, state_digest)?
            .ok_or_else(|| {
                AppError::new(
                    "sync_receipt_missing",
                    "remote rebuild has no matching canonical publication receipt",
                )
            })?;
        let normalized =
            session_scope_directory(path, &request.session_id)?.join("peer-publish.db");
        if !normalized.is_file() || sync_state_digest(&normalized)? != state_digest {
            return Err(AppError::new(
                "sync_state_invalid",
                "remote rebuild has no matching staged normalized state",
            ));
        }
        let continuity = replay_sync_continuity(path, &normalized);
        let derived = if continuity["status"] == "completed" {
            rebuild_derived(path, &publication)
        } else {
            json!({
                "status":"failed",
                "committed":true,
                "error":"sync_continuity_incomplete",
                "next_action":"resume_continuity",
            })
        };
        return Ok(json!({
            "protocol": PROTOCOL_VERSION,
            "action": "rebuilt",
            "session_id": request.session_id,
            "scope": path.scope,
            "continuity": continuity,
            "derived": derived,
        }));
    }
    let expected = request.expected.as_ref().ok_or_else(|| {
        AppError::new(
            "sync_protocol_invalid",
            "store identity precondition is missing",
        )
    })?;
    let missing = !canonical_store_exists(&path.path)?;
    let identity = if missing {
        missing_store_identity()
    } else {
        Store::open_for_read(scope_name(path.scope), &path.path)?.identity()?
    };
    if &identity != expected
        || (missing && !matches!(request.action.as_str(), "publish" | "export"))
    {
        if request.action == "publish"
            && request.state_digest.as_deref().is_some_and(|digest| {
                sync_commit_exists(&path.path, &request.session_id, digest).unwrap_or(false)
            })
        {
            let size = request
                .payload_size
                .filter(|size| *size <= MAX_TRANSFER_BYTES)
                .ok_or_else(|| {
                    AppError::new("sync_protocol_invalid", "publish payload size is invalid")
                })?;
            let expected_digest = request
                .state_digest
                .as_deref()
                .expect("checked state digest");
            let directory = session_scope_directory(path, &request.session_id)?;
            let replay = directory.join(format!(
                "peer-replay-{}-{}.db",
                std::process::id(),
                STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            receive_exact_file(&mut stdin, &replay, size)?;
            require_payload_eof(&mut stdin)?;
            let actual = sync_state_digest(&replay)?;
            if actual != expected_digest {
                let _ = fs::remove_file(&replay);
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    "replayed publish state checksum differs",
                ));
            }
            let mut publication =
                sync_publication_receipt(&path.path, &request.session_id, expected_digest)?
                    .ok_or_else(|| {
                        AppError::new(
                            "sync_receipt_missing",
                            "committed Sync has no matching canonical publication receipt",
                        )
                    })?;
            publication["recovered"] = Value::Bool(true);
            let continuity = replay_sync_continuity(path, &replay);
            let derived = if continuity_succeeded(&continuity) {
                rebuild_derived(path, &publication)
            } else {
                json!({
                    "status":"failed",
                    "committed":true,
                    "error":"sync_continuity_incomplete",
                    "next_action":"resume_continuity",
                })
            };
            let _ = fs::remove_file(&replay);
            return Ok(json!({
                "protocol": PROTOCOL_VERSION,
                "action": "published",
                "session_id": request.session_id,
                "scope": path.scope,
                "committed": true,
                "idempotent": true,
                "ending_identity": identity,
                "publication": publication,
                "continuity": continuity,
                "derived": derived,
            }));
        }
        return Err(AppError::new(
            "sync_store_changed",
            "remote store changed after the Sync handshake",
        )
        .with_details(json!({"current": identity})));
    }
    if missing && matches!(request.action.as_str(), "publish" | "export") {
        let runtime = path
            .path
            .parent()
            .ok_or_else(|| AppError::new("invalid_store_path", "Wiki has no runtime directory"))?;
        ensure_real_directory(runtime)?;
    }
    let directory = session_scope_directory(path, &request.session_id)?;
    match request.action.as_str() {
        "ack" => {
            let requester_store_id = request.requester_store_id.as_deref().ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "requester store ID is missing")
            })?;
            let normalized = directory.join("peer-export.db");
            if !normalized.is_file() {
                return Err(AppError::new(
                    "sync_state_invalid",
                    "acknowledgement has no bound exported state",
                ));
            }
            let digest = sync_state_digest(&normalized)?;
            if request.state_digest.as_deref() != Some(digest.as_str()) {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    "acknowledged state checksum differs",
                ));
            }
            update_baseline(path, requester_store_id, "local", &normalized)?;
            Ok(json!({
                "protocol": PROTOCOL_VERSION,
                "action": "acknowledged",
                "session_id": request.session_id,
                "scope": path.scope,
                "state_digest": digest,
            }))
        }
        "export" => {
            let normalized = directory.join("peer-export.db");
            reject_non_regular_artifact(&normalized)?;
            if !normalized.exists() {
                if missing {
                    create_empty_sync_state(&normalized)?;
                } else {
                    let store = Store::open_for_read(scope_name(path.scope), &path.path)?;
                    store.begin_read_snapshot()?;
                    let bound_identity = store.identity()?;
                    if &bound_identity != expected {
                        return Err(AppError::new(
                            "sync_store_changed",
                            "remote store changed before the export snapshot",
                        )
                        .with_details(json!({"current": bound_identity})));
                    }
                    export_sync_state_with_continuity(path, &store, &normalized)?;
                }
            }
            let requester_store_id = request.requester_store_id.as_deref().ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "requester store ID is missing")
            })?;
            let peer_baseline = baseline_path(path, requester_store_id, "local")?;
            let compatible_baseline = if let Some(expected) = request.baseline_digest.as_deref() {
                if peer_baseline.is_file() && sync_state_digest(&peer_baseline)? == expected {
                    Some(peer_baseline.as_path())
                } else {
                    None
                }
            } else {
                None
            };
            let artifact = directory.join("peer-export.payload");
            reject_non_regular_artifact(&artifact)?;
            if artifact.is_file() {
                fs::remove_file(&artifact)?;
            }
            let transfer = prepare_sync_transfer(compatible_baseline, &normalized, &artifact)?;
            if transfer.size > MAX_TRANSFER_BYTES {
                return Err(AppError::new(
                    "sync_transfer_too_large",
                    "normalized Sync state exceeds transfer limit",
                ));
            }
            let header = PeerTransfer {
                protocol: PROTOCOL_VERSION,
                action: "export".to_string(),
                session_id: request.session_id,
                scope: path.scope,
                kind: transfer.kind,
                size: transfer.size,
                state_digest: transfer.state_digest,
                baseline_digest: transfer.baseline_digest,
                artifact_digest: file_digest(&artifact)?,
                identity: expected.clone(),
            };
            let stdout = std::io::stdout();
            let mut stdout = stdout.lock();
            serde_json::to_writer(&mut stdout, &header)
                .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
            stdout.write_all(b"\n")?;
            let mut file = fs::File::open(artifact)?;
            std::io::copy(&mut file, &mut stdout)?;
            stdout.flush()?;
            Ok(Value::Null)
        }
        "publish" => {
            let requester_store_id = request.requester_store_id.as_deref().ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "requester store ID is missing")
            })?;
            let size = request
                .payload_size
                .filter(|size| *size <= MAX_TRANSFER_BYTES)
                .ok_or_else(|| {
                    AppError::new("sync_protocol_invalid", "publish payload size is invalid")
                })?;
            let expected_digest = request.state_digest.as_deref().ok_or_else(|| {
                AppError::new("sync_protocol_invalid", "publish state digest is missing")
            })?;
            let normalized = directory.join("peer-publish.db");
            reject_non_regular_artifact(&normalized)?;
            if normalized.is_file() {
                fs::remove_file(&normalized)?;
            }
            receive_exact_file(&mut stdin, &normalized, size)?;
            require_payload_eof(&mut stdin)?;
            let actual = sync_state_digest(&normalized)?;
            if actual != expected_digest {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    "published state checksum differs",
                ));
            }
            let summary = if missing {
                Store::publish_sync_state_to_missing(
                    scope_name(path.scope),
                    &path.path,
                    &normalized,
                    &request.session_id,
                )?
            } else {
                let mut store = Store::open(scope_name(path.scope), &path.path)?;
                store.publish_sync_state(&normalized, expected, &request.session_id)?
            };
            let publication = serde_json::to_value(summary)
                .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
            let store = Store::open(scope_name(path.scope), &path.path)?;
            let ending_identity = store.identity()?;
            drop(store);
            let baseline = match update_baseline(path, requester_store_id, "local", &normalized) {
                Ok(()) => json!({"status":"completed"}),
                Err(error) => json!({
                    "status":"failed",
                    "error":error.code,
                    "committed":true,
                    "fallback":"next Sync transfers a full normalized state",
                }),
            };
            let continuity = replay_sync_continuity(path, &normalized);
            let derived = if continuity_succeeded(&continuity) {
                rebuild_derived(path, &publication)
            } else {
                json!({
                    "status":"failed",
                    "error":"sync_continuity_incomplete",
                    "committed":true,
                    "next_action":"resume_continuity",
                })
            };
            Ok(json!({
                "protocol": PROTOCOL_VERSION,
                "action": "published",
                "session_id": request.session_id,
                "scope": path.scope,
                "publication": publication,
                "committed": true,
                "ending_identity": ending_identity,
                "baseline": baseline,
                "continuity": continuity,
                "derived": derived,
            }))
        }
        _ => Err(AppError::new(
            "sync_protocol_invalid",
            "unsupported Sync peer action",
        )),
    }
}

fn export_sync_state_with_continuity(
    live: &StorePath,
    store: &Store,
    normalized: &Path,
) -> Result<SyncExportSummary> {
    let origin_store_id = store.identity()?.store_id;
    let drafts = crate::changeset::export_detached_intents(live)?;
    let audits = crate::work::terminal_sync_audits(&live.path, &origin_store_id)?;
    let inherited_audits = load_inherited_sync_audits(&live.path)?;
    let mut summary = store.export_sync_state(normalized)?;
    let conn = Connection::open(normalized)?;

    for draft in drafts {
        let key = format!("{origin_store_id}\0{}", draft.intent.origin_changeset_id);
        insert_continuity_object(
            &conn,
            "draft_intent",
            &key,
            &json!({"origin_store_id": origin_store_id, "intent": draft.intent}),
        )?;
        for blob in draft.blobs {
            let metadata = fs::symlink_metadata(&blob.draft_database)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(AppError::new(
                    "changeset_sync_invalid",
                    "draft blob source is not a regular non-symlink SQLite file",
                ));
            }
            let draft_database = blob.draft_database.to_string_lossy().into_owned();
            conn.execute("ATTACH DATABASE ?1 AS sync_draft", [&draft_database])?;
            let copied = (|| -> Result<()> {
                let actual: Option<(String, i64)> = conn
                    .query_row(
                        "SELECT content_hash,length(CAST(content AS BLOB))
                         FROM sync_draft.sources WHERE id=?1",
                        [blob.source_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if actual
                    != Some((
                        blob.content_hash.clone(),
                        i64::try_from(blob.bytes).map_err(|_| {
                            AppError::new("changeset_sync_limit", "draft source is too large")
                        })?,
                    ))
                {
                    return Err(AppError::new(
                        "changeset_sync_changed",
                        "draft source changed after its detached intent snapshot",
                    ));
                }
                conn.execute(
                    "INSERT OR IGNORE INTO sync_blobs(content_hash,content)
                     SELECT content_hash,CAST(content AS BLOB)
                     FROM sync_draft.sources WHERE id=?1 AND content_hash=?2",
                    rusqlite::params![blob.source_id, blob.content_hash],
                )?;
                Ok(())
            })();
            let detached = conn.execute_batch("DETACH DATABASE sync_draft");
            copied?;
            detached?;
        }
    }
    for audit in audits {
        let key = audit.audit_key.clone();
        let payload = serde_json::to_value(audit)
            .map_err(|error| AppError::new("sync_audit_invalid", error.to_string()))?;
        insert_continuity_object(&conn, "work_audit", &key, &payload)?;
    }
    for (key, payload) in inherited_audits {
        insert_continuity_object(&conn, "work_audit", &key, &payload)?;
    }
    conn.execute_batch("VACUUM;")?;
    summary.object_count =
        conn.query_row("SELECT COUNT(*) FROM sync_objects", [], |row| row.get(0))?;
    summary.blob_count = conn.query_row("SELECT COUNT(*) FROM sync_blobs", [], |row| row.get(0))?;
    drop(conn);
    summary.state_digest = sync_state_digest(normalized)?;
    Ok(summary)
}

fn insert_continuity_object(
    conn: &Connection,
    kind: &str,
    logical_key: &str,
    payload: &Value,
) -> Result<()> {
    let encoded = serde_json::to_string(payload)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    let existing = conn
        .query_row(
            "SELECT payload_json FROM sync_objects WHERE kind=?1 AND logical_key=?2",
            rusqlite::params![kind, logical_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != encoded {
            return Err(AppError::new(
                "sync_continuity_conflict",
                format!("continuity object differs for {kind}:{logical_key}"),
            ));
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO sync_objects(kind,logical_key,payload_json,payload_hash)
         VALUES(?1,?2,?3,?4)",
        rusqlite::params![kind, logical_key, encoded, hex_digest(encoded.as_bytes())],
    )?;
    Ok(())
}

fn load_inherited_sync_audits(database: &Path) -> Result<Vec<(String, Value)>> {
    const MAX_AUDITS: usize = 4_096;
    let conn = Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare(
        "SELECT target,detail_json FROM operations
         WHERE action='sync_work_audit' ORDER BY target,id",
    )?;
    let mut audits = Vec::new();
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (key, encoded) = row?;
        let payload = serde_json::from_str(&encoded)
            .map_err(|error| AppError::new("sync_audit_invalid", error.to_string()))?;
        audits.push((key, payload));
    }
    audits.sort_by(|left, right| left.0.cmp(&right.0));
    audits.dedup_by(|left, right| left.0 == right.0 && left.1 == right.1);
    if audits.len() > MAX_AUDITS {
        return Err(AppError::new(
            "sync_audit_limit",
            "inherited terminal Work audit count exceeds the fixed Sync limit",
        ));
    }
    Ok(audits)
}

fn replay_sync_continuity(live: &StorePath, normalized: &Path) -> Value {
    match replay_sync_continuity_inner(live, normalized) {
        Ok(drafts) => json!({
            "status": "completed",
            "drafts": drafts,
            "terminal_work": "audited",
            "active_work": "local_only",
        }),
        Err(error) => json!({
            "status": "failed",
            "error": error.code,
            "committed": true,
            "next_action": "resume_continuity",
        }),
    }
}

fn replay_sync_continuity_inner(live: &StorePath, normalized: &Path) -> Result<Vec<Value>> {
    sync_state_digest(normalized)?;
    let target_store_id = Store::open_for_read(scope_name(live.scope), &live.path)?
        .identity()?
        .store_id;
    let conn = Connection::open_with_flags(normalized, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare(
        "SELECT payload_json FROM sync_objects
         WHERE kind='draft_intent' ORDER BY logical_key",
    )?;
    let payloads = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut replays = Vec::new();
    for encoded in payloads {
        let payload: Value = serde_json::from_str(&encoded)
            .map_err(|error| AppError::new("changeset_replay_invalid", error.to_string()))?;
        let origin_store_id = payload["origin_store_id"].as_str().ok_or_else(|| {
            AppError::new(
                "changeset_replay_invalid",
                "detached intent has no origin store ID",
            )
        })?;
        if origin_store_id == target_store_id {
            continue;
        }
        let intent: DetachedChangesetIntent = serde_json::from_value(payload["intent"].clone())
            .map_err(|error| AppError::new("changeset_replay_invalid", error.to_string()))?;
        let replay =
            crate::changeset::replay_detached_intent(live, origin_store_id, &intent, |_| {
                Ok(normalized.to_path_buf())
            })?;
        replays.push(
            serde_json::to_value(replay)
                .map_err(|error| AppError::new("changeset_replay_invalid", error.to_string()))?,
        );
    }
    Ok(replays)
}

fn sync_commit_exists(database: &Path, session_id: &str, state_digest: &str) -> Result<bool> {
    let connection =
        Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let detail: Option<String> = connection
        .query_row(
            "SELECT detail_json FROM operations
             WHERE action='sync_merge' AND target=?1 ORDER BY id DESC LIMIT 1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(detail
        .and_then(|detail| serde_json::from_str::<Value>(&detail).ok())
        .is_some_and(|detail| detail["state_digest"] == state_digest))
}

fn rebuild_derived(store: &StorePath, publication: &Value) -> Value {
    let fts = publication.get("derived").cloned().unwrap_or_else(|| {
        json!({
            "status":"failed",
            "error":"sync_fts_receipt_missing",
            "committed":true,
            "next_action":"resume_derived_rebuild",
        })
    });
    let publication = normalized_publication_receipt(publication);
    let markdown = match Store::open(scope_name(store.scope), &store.path)
        .and_then(|mut store| store.materialize_sync_selection(&publication))
    {
        Ok(response) => json!({"status":"completed","files":response.files}),
        Err(error) => json!({"status":"failed","error":error.code}),
    };
    let mut graph = crate::external_graph::passive_status(scope_name(store.scope), &store.path)
        .unwrap_or_else(|error| json!({"status":"failed","error":error.code}));
    let selected = if publication["derived_selection"] == "exact" {
        match publication["affected_graph_documents"].as_array() {
            Some(documents) => {
                let mut selected = Vec::with_capacity(documents.len());
                for document in documents {
                    let Some(pair) = document.as_array().filter(|pair| pair.len() == 2) else {
                        return json!({
                            "status":"failed",
                            "error":"sync_receipt_invalid",
                            "committed":true,
                            "next_action":"resume_derived_rebuild",
                        });
                    };
                    let (Some(kind), Some(identifier)) = (pair[0].as_str(), pair[1].as_str())
                    else {
                        return json!({
                            "status":"failed",
                            "error":"sync_receipt_invalid",
                            "committed":true,
                            "next_action":"resume_derived_rebuild",
                        });
                    };
                    selected.push((kind.to_owned(), identifier.to_owned()));
                }
                Some(selected)
            }
            None => {
                return json!({
                    "status":"failed",
                    "error":"sync_receipt_invalid",
                    "committed":true,
                    "next_action":"resume_derived_rebuild",
                });
            }
        }
    } else if publication["derived_selection"] == "full" {
        None
    } else {
        return json!({
            "status":"failed",
            "error":"sync_receipt_invalid",
            "committed":true,
            "next_action":"resume_derived_rebuild",
        });
    };
    if graph["status"] != "disabled" && markdown["status"] != "failed" {
        graph = crate::external_graph::project_documents(
            scope_name(store.scope),
            &store.path,
            selected.as_deref(),
            &mut |_done, _total, _phase| Ok(()),
        )
        .unwrap_or_else(|error| json!({"status":"failed","error":error.code}));
    }
    let failed =
        fts["status"] == "failed" || markdown["status"] == "failed" || graph["status"] == "failed";
    json!({
        "status": if failed { "failed" } else { "completed" },
        "committed": true,
        "fts": fts,
        "markdown": markdown,
        "graph": graph,
        "codegraph": "deferred_until_after_git",
        "next_action": failed.then_some("resume_derived_rebuild"),
    })
}

fn normalized_publication_receipt(publication: &Value) -> Value {
    if publication.get("affected").is_some() || publication["derived_selection"] != "exact" {
        return publication.clone();
    }
    let mut normalized = publication.clone();
    normalized["affected"] = json!({
        "page": publication["affected_pages"],
        "source": publication["affected_sources"],
        "memory": publication["affected_memory"],
        "todo": publication["affected_todos"],
        "plan": publication["affected_plans"],
        "meta": publication["affected_meta"],
        "tag": publication["affected_tags"],
        "semantic_relation": publication["affected_relations"],
    });
    normalized
}

fn derived_succeeded(value: &Value) -> bool {
    value["status"] != "failed" && value["graph"]["status"] != "failed"
}

fn continuity_succeeded(value: &Value) -> bool {
    value["status"] == "completed"
}

fn rebuild_codegraph(store: &StorePath) -> Value {
    if store.scope != Scope::Project {
        return json!({"status":"not_applicable"});
    }
    let status = crate::codegraph::status(store)
        .unwrap_or_else(|error| json!({"initialized":false,"error":error.code}));
    if status.get("error").is_some() {
        json!({"status":"failed","error":status["error"]})
    } else if status["initialized"] == true {
        crate::codegraph::run(store, &[OsString::from("sync")])
            .unwrap_or_else(|error| json!({"status":"failed","error":error.code}))
    } else {
        json!({"status":"not_initialized"})
    }
}

fn rebuild_codegraph_after_git(
    state: &mut SessionState,
    stores: &[StorePath],
    host: &str,
    remote_directory: Option<&Path>,
    session_id: &str,
) -> Result<()> {
    let mut codegraph = state.git_derived.clone().unwrap_or_else(
        || json!({"local":{"status":"not_required"},"remote":{"status":"not_required"}}),
    );
    if state.git_applied_local
        && matches!(
            codegraph["local"]["status"].as_str(),
            Some("not_required" | "failed")
        )
        && let Some(project_store) = stores.iter().find(|store| store.scope == Scope::Project)
        && canonical_store_exists(&project_store.path)?
    {
        codegraph["local"] = rebuild_codegraph(project_store);
    }
    let remote_initialized = state
        .peer_stores
        .iter()
        .find(|store| store.scope == Scope::Project)
        .is_some_and(|store| !store.missing)
        || state
            .units
            .iter()
            .find(|unit| unit.scope == Scope::Project)
            .is_some_and(|unit| unit.published_remote);
    if state.git_published_remote
        && remote_initialized
        && matches!(
            codegraph["remote"]["status"].as_str(),
            Some("not_required" | "failed")
        )
    {
        let request = PeerRequest {
            protocol: PROTOCOL_VERSION,
            action: "codegraph".to_string(),
            session_id: session_id.to_string(),
            scope: Scope::Project,
            directory: remote_directory.map(Path::to_path_buf),
            store_scope: Some(Scope::Project),
            payload_size: None,
            state_digest: None,
            expected: None,
            baseline_digest: None,
            requester_store_id: None,
            plugin: None,
        };
        let response = call_peer_value(host, &request)?;
        if response["protocol"] != PROTOCOL_VERSION
            || response["action"] != "codegraph_rebuilt"
            || response["session_id"] != session_id
        {
            return Err(AppError::new(
                "sync_protocol_invalid",
                "remote CodeGraph response does not match the session",
            ));
        }
        codegraph["remote"] = response["derived"].clone();
    }
    state.git_derived = Some(codegraph);
    Ok(())
}

fn git_derived_failed(state: &SessionState) -> bool {
    state.git_derived.as_ref().is_some_and(|derived| {
        derived["local"]["status"] == "failed" || derived["remote"]["status"] == "failed"
    })
}

fn call_peer(host: &str, request: &PeerRequest) -> Result<PeerResponse> {
    serde_json::from_value(call_peer_value(host, request)?)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))
}

fn call_peer_value(host: &str, request: &PeerRequest) -> Result<Value> {
    let mut child = Command::new("ssh")
        .args(["--", host, "lwc", "__sync-peer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::new("sync_transport_failed", error.to_string()))?;
    let mut request = serde_json::to_vec(request)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
    request.push(b'\n');
    child
        .stdin
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdin is unavailable"))?
        .write_all(&request)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stderr is unavailable"))?;
    let stdout = thread::spawn(move || read_bounded(stdout, MAX_PROTOCOL_BYTES));
    let stderr = thread::spawn(move || read_bounded(stderr, MAX_PROTOCOL_BYTES));
    let deadline = Instant::now() + SSH_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new(
                "sync_transport_timeout",
                "SSH peer handshake exceeded 30 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = stdout
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stdout reader failed"))??;
    let stderr = stderr
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stderr reader failed"))??;
    if !status.success() {
        let remote = serde_json::from_slice::<Value>(&stderr).unwrap_or_else(|_| {
            json!({"message": String::from_utf8_lossy(&stderr).chars().take(4096).collect::<String>()})
        });
        return Err(AppError::new(
            "sync_remote_failed",
            "remote Sync peer rejected the request",
        )
        .with_details(json!({"remote": remote})));
    }
    serde_json::from_slice::<Value>(&stdout).map_err(|error| {
        AppError::new(
            "sync_protocol_invalid",
            format!("remote Sync response is invalid: {error}"),
        )
    })
}

fn call_peer_export(
    host: &str,
    request: &PeerRequest,
    baseline: Option<&Path>,
    destination: &Path,
) -> Result<PeerTransfer> {
    let mut child = spawn_peer(host)?;
    write_peer_request(
        child
            .stdin
            .take()
            .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdin is unavailable"))?,
        request,
        None,
    )?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stderr is unavailable"))?;
    let destination = destination.to_path_buf();
    let output_path = destination.with_extension(format!(
        "download-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let reader_path = output_path.clone();
    let stdout = thread::spawn(move || receive_peer_export(stdout, &reader_path));
    let stderr = thread::spawn(move || read_bounded(stderr, MAX_PROTOCOL_BYTES));
    let status = wait_child(
        &mut child,
        SYNC_TIMEOUT,
        "SSH Sync export exceeded 120 seconds",
    )?;
    let transfer = stdout
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH export reader failed"))?;
    let stderr = stderr
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stderr reader failed"))??;
    if !status.success() {
        let _ = fs::remove_file(&output_path);
        return Err(remote_peer_error(&stderr));
    }
    let transfer = match transfer {
        Ok(transfer) => transfer,
        Err(error) => {
            let _ = fs::remove_file(&output_path);
            return Err(error);
        }
    };
    if transfer.protocol != PROTOCOL_VERSION
        || transfer.action != "export"
        || transfer.session_id != request.session_id
        || transfer.scope != request.scope
        || request.expected.as_ref() != Some(&transfer.identity)
        || (transfer.baseline_digest.is_some()
            && transfer.baseline_digest != request.baseline_digest)
        || (transfer.kind == SyncTransferKind::Delta
            && transfer.baseline_digest != request.baseline_digest)
    {
        let _ = fs::remove_file(&output_path);
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote export metadata does not match the request",
        ));
    }
    let reconstructed = destination.with_extension(format!(
        "state-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if file_digest(&output_path)? != transfer.artifact_digest {
        let _ = fs::remove_file(&output_path);
        return Err(AppError::new(
            "sync_checksum_mismatch",
            "downloaded Sync artifact checksum differs",
        ));
    }
    let summary = SyncTransferSummary {
        kind: transfer.kind,
        size: transfer.size,
        state_digest: transfer.state_digest.clone(),
        baseline_digest: transfer.baseline_digest.clone(),
    };
    if let Err(error) =
        apply_sync_transfer_artifact(baseline, &output_path, &summary, &reconstructed)
    {
        let _ = fs::remove_file(&output_path);
        return Err(error);
    }
    fs::remove_file(&output_path)?;
    replace_file(&reconstructed, &destination)?;
    Ok(transfer)
}

fn call_peer_publish(host: &str, request: &PeerRequest, payload: &Path) -> Result<Value> {
    let mut child = spawn_peer(host)?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdin is unavailable"))?;
    let request_owned = serde_json::to_vec(request)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
    let payload = payload.to_path_buf();
    let writer = thread::spawn(move || -> Result<()> {
        let mut stdin = stdin;
        stdin.write_all(&request_owned)?;
        stdin.write_all(b"\n")?;
        let mut file = fs::File::open(payload)?;
        std::io::copy(&mut file, &mut stdin)?;
        stdin.flush()?;
        Ok(())
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stderr is unavailable"))?;
    let stdout = thread::spawn(move || read_bounded(stdout, MAX_PROTOCOL_BYTES));
    let stderr = thread::spawn(move || read_bounded(stderr, MAX_PROTOCOL_BYTES));
    let status = wait_child(
        &mut child,
        SYNC_TIMEOUT,
        "SSH Sync publication exceeded 120 seconds",
    )?;
    writer
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH publish writer failed"))??;
    let stdout = stdout
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stdout reader failed"))??;
    let stderr = stderr
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stderr reader failed"))??;
    if !status.success() {
        return Err(remote_peer_error(&stderr));
    }
    let response: Value = serde_json::from_slice(&stdout).map_err(|error| {
        AppError::new(
            "sync_protocol_invalid",
            format!("remote publish response is invalid: {error}"),
        )
    })?;
    let expected_action = if request.action == "plugin-publish" {
        "plugin-published"
    } else {
        "published"
    };
    if response["protocol"] != PROTOCOL_VERSION
        || response["action"] != expected_action
        || response["session_id"] != request.session_id
    {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote publication response does not match the request",
        ));
    }
    Ok(response)
}

fn call_peer_plugin_export(
    host: &str,
    request: &PeerRequest,
    destination: &Path,
) -> Result<PeerPluginTransfer> {
    let mut child = spawn_peer(host)?;
    write_peer_request(
        child
            .stdin
            .take()
            .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdin unavailable"))?,
        request,
        None,
    )?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("sync_transport_failed", "SSH stderr unavailable"))?;
    let output = destination.with_extension(format!(
        "download-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let reader_path = output.clone();
    let reader = thread::spawn(move || receive_plugin_export(stdout, &reader_path));
    let stderr = thread::spawn(move || read_bounded(stderr, MAX_PROTOCOL_BYTES));
    let status = wait_child(
        &mut child,
        SYNC_TIMEOUT,
        "SSH plugin export exceeded 120 seconds",
    )?;
    let transfer = reader
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "plugin reader failed"))?;
    let stderr = stderr
        .join()
        .map_err(|_| AppError::new("sync_transport_failed", "SSH stderr reader failed"))??;
    if !status.success() {
        let _ = fs::remove_file(&output);
        return Err(remote_peer_error(&stderr));
    }
    let transfer = transfer?;
    if transfer.protocol != PROTOCOL_VERSION
        || transfer.action != "plugin-export"
        || transfer.session_id != request.session_id
        || request.plugin.as_ref() != Some(&transfer.plugin)
        || file_digest(&output)? != transfer.artifact_digest
    {
        let _ = fs::remove_file(&output);
        return Err(AppError::new(
            "sync_protocol_invalid",
            "remote plugin export metadata does not match the request",
        ));
    }
    replace_file(&output, destination)?;
    Ok(transfer)
}

fn receive_plugin_export(input: impl Read, destination: &Path) -> Result<PeerPluginTransfer> {
    let mut input = BufReader::new(input);
    let header = read_bounded_line(&mut input, MAX_PROTOCOL_BYTES)?;
    let transfer: PeerPluginTransfer = serde_json::from_slice(&header)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
    if transfer.size > MAX_TRANSFER_BYTES {
        return Err(AppError::new(
            "sync_transfer_too_large",
            "remote plugin artifact exceeds transfer limit",
        ));
    }
    receive_exact_file(&mut input, destination, transfer.size)?;
    require_payload_eof(&mut input)?;
    Ok(transfer)
}

fn spawn_peer(host: &str) -> Result<std::process::Child> {
    Command::new("ssh")
        .args(["--", host, "lwc", "__sync-peer"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::new("sync_transport_failed", error.to_string()))
}

fn write_peer_request(
    mut stdin: impl Write,
    request: &PeerRequest,
    payload: Option<&Path>,
) -> Result<()> {
    serde_json::to_writer(&mut stdin, request)
        .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
    stdin.write_all(b"\n")?;
    if let Some(payload) = payload {
        std::io::copy(&mut fs::File::open(payload)?, &mut stdin)?;
    }
    stdin.flush()?;
    Ok(())
}

fn wait_child(
    child: &mut std::process::Child,
    timeout: Duration,
    message: &str,
) -> Result<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new("sync_transport_timeout", message));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn remote_peer_error(stderr: &[u8]) -> AppError {
    let remote = serde_json::from_slice::<Value>(stderr).unwrap_or_else(|_| {
        json!({"message": String::from_utf8_lossy(stderr).chars().take(4096).collect::<String>()})
    });
    AppError::new(
        "sync_remote_failed",
        "remote Sync peer rejected the request",
    )
    .with_details(json!({"remote": remote}))
}

fn receive_peer_export(input: impl Read, destination: &Path) -> Result<PeerTransfer> {
    let result = (|| {
        let mut input = BufReader::new(input);
        let header = read_bounded_line(&mut input, MAX_PROTOCOL_BYTES)?;
        let transfer: PeerTransfer = serde_json::from_slice(&header)
            .map_err(|error| AppError::new("sync_protocol_invalid", error.to_string()))?;
        if transfer.size > MAX_TRANSFER_BYTES {
            return Err(AppError::new(
                "sync_transfer_too_large",
                "remote Sync state exceeds transfer limit",
            ));
        }
        receive_exact_file(&mut input, destination, transfer.size)?;
        let mut extra = [0_u8; 1];
        if input.read(&mut extra)? != 0 {
            return Err(AppError::new(
                "sync_protocol_invalid",
                "remote sent bytes beyond declared payload",
            ));
        }
        Ok(transfer)
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn read_bounded_line(input: &mut impl BufRead, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > limit || bytes.is_empty() {
        return Err(AppError::new(
            "sync_protocol_too_large",
            "Sync protocol header is missing or too large",
        ));
    }
    if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    Ok(bytes)
}

fn receive_exact_file(input: &mut impl Read, destination: &Path, size: u64) -> Result<()> {
    if let Some(parent) = destination.parent() {
        ensure_real_directory(parent)?;
    }
    reject_non_regular_artifact(destination)?;
    let mut limited = input.take(size);
    if destination.exists() {
        let copied = std::io::copy(&mut limited, &mut std::io::sink())?;
        if copied != size {
            return Err(AppError::new(
                "sync_protocol_invalid",
                "Sync payload ended before declared size",
            ));
        }
        return Ok(());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(destination)?;
    let copied = std::io::copy(&mut limited, &mut file)?;
    if copied != size {
        let _ = fs::remove_file(destination);
        return Err(AppError::new(
            "sync_protocol_invalid",
            "Sync payload ended before declared size",
        ));
    }
    file.sync_all()?;
    Ok(())
}

fn require_payload_eof(input: &mut impl Read) -> Result<()> {
    let mut extra = [0_u8; 1];
    if input.read(&mut extra)? == 0 {
        Ok(())
    } else {
        Err(AppError::new(
            "sync_protocol_invalid",
            "Sync payload exceeds its declared size",
        ))
    }
}

fn read_bounded(input: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(AppError::new(
            "sync_protocol_too_large",
            format!("Sync protocol message exceeds {limit} bytes"),
        ));
    }
    Ok(bytes)
}

fn validate_host(host: &str) -> Result<()> {
    let valid = !host.is_empty()
        && host.len() <= 255
        && !host.starts_with('-')
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'-' | b'_' | b'@' | b':' | b'[' | b']')
        });
    if !valid {
        return Err(AppError::new(
            "invalid_sync_host",
            "Sync host must be a safe SSH host or alias without whitespace or shell syntax",
        ));
    }
    Ok(())
}

fn validate_remote_directory(scope: Scope, directory: Option<&Path>) -> Result<Option<PathBuf>> {
    if scope != Scope::Global && directory.is_none() {
        return Err(AppError::new(
            "invalid_sync_directory",
            "project and all Sync scopes require an absolute remote project directory",
        ));
    }
    if let Some(directory) = directory {
        if !directory.is_absolute() {
            return Err(AppError::new(
                "invalid_sync_directory",
                "remote project directory must be absolute",
            ));
        }
        return Ok(Some(directory.to_path_buf()));
    }
    Ok(None)
}

fn explicit_peer_directory(scope: Scope, directory: Option<&Path>) -> Result<PathBuf> {
    if scope == Scope::Global && directory.is_none() {
        return std::env::current_dir().map_err(Into::into);
    }
    let directory = validate_remote_directory(scope, directory)?.expect("validated project path");
    let canonical = fs::canonicalize(&directory).map_err(|error| {
        AppError::new(
            "invalid_sync_directory",
            format!(
                "remote project directory {} is unavailable: {error}",
                directory.display()
            ),
        )
    })?;
    if !canonical.is_dir() {
        return Err(AppError::new(
            "invalid_sync_directory",
            "remote project directory is not a directory",
        ));
    }
    Ok(canonical)
}

fn peer_store_paths(scope: Scope, directory: &Path) -> Result<Vec<StorePath>> {
    let mut paths = Vec::with_capacity(if scope == Scope::All { 2 } else { 1 });
    for requested in match scope {
        Scope::All => &[Scope::Project, Scope::Global][..],
        Scope::Project => &[Scope::Project][..],
        Scope::Global => &[Scope::Global][..],
    } {
        match resolve_explicit_read_store_paths(*requested, directory) {
            Ok(mut existing) => paths.push(existing.pop().expect("one exact Sync store")),
            Err(error) if error.code == "store_not_found" => {
                let path = match requested {
                    Scope::Project => {
                        StorePath::new(Scope::Project, directory.join(".lwc/wiki.db"))
                    }
                    Scope::Global => init_store_path(Scope::Global, directory)?,
                    Scope::All => unreachable!(),
                };
                paths.push(path);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(paths)
}

fn canonical_store_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(AppError::new(
            "invalid_store_path",
            format!("Wiki database is not a regular file: {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn missing_store_identity() -> StoreIdentity {
    StoreIdentity {
        store_id: "0".repeat(64),
        revision: "0".repeat(64),
        operation_id: -1,
    }
}

fn new_session_id() -> Result<String> {
    Connection::open_in_memory()?
        .query_row("SELECT LOWER(HEX(RANDOMBLOB(16)))", [], |row| row.get(0))
        .map_err(Into::into)
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.len() != 32
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::new(
            "invalid_sync_session",
            "Sync session ID must be 32 lowercase hex characters",
        ));
    }
    Ok(())
}

fn validate_resolution_envelope(
    scope: Scope,
    resolution: &Value,
    state: Option<&SessionState>,
) -> Result<()> {
    let object = resolution.as_object().ok_or_else(|| {
        AppError::new(
            "sync_resolution_invalid",
            "resolution packet must be an object",
        )
    })?;
    if resolution["version"] != 1 {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution packet version must be 1",
        ));
    }
    if scope != Scope::All {
        if object.len() != 2 || !object.contains_key("decisions") {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "resolution packet accepts only version and decisions",
            ));
        }
        if !resolution["decisions"].is_array() {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "resolution decisions must be an array",
            ));
        }
        return Ok(());
    }
    if object.len() != 2 || !object.contains_key("scopes") {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "all-scope resolution accepts only version and scopes",
        ));
    }
    let scopes = resolution["scopes"].as_object().ok_or_else(|| {
        AppError::new(
            "sync_resolution_invalid",
            "resolution scopes must be an object",
        )
    })?;
    for (name, packet) in scopes {
        let requested = match name.as_str() {
            "project" => Scope::Project,
            "global" => Scope::Global,
            _ => {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    format!("unknown resolution scope: {name}"),
                ));
            }
        };
        if state.is_some_and(|state| !state.units.iter().any(|unit| unit.scope == requested)) {
            return Err(AppError::new(
                "sync_resolution_invalid",
                format!("resolution scope {name} is not part of this session"),
            ));
        }
        let packet = packet.as_object().ok_or_else(|| {
            AppError::new(
                "sync_resolution_invalid",
                "scoped resolution must be an object",
            )
        })?;
        if packet.len() != 1 || !packet.get("decisions").is_some_and(Value::is_array) {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "scoped resolution accepts only a decisions array",
            ));
        }
    }
    Ok(())
}

fn state_path(store: &StorePath, session_id: &str) -> Result<PathBuf> {
    validate_session_id(session_id)?;
    let root = require_database_runtime_root(&store.path)?.join("sync");
    ensure_real_directory(&root)?;
    let session = root.join(session_id);
    ensure_real_directory(&session)?;
    Ok(session.join("state.json"))
}

fn session_scope_directory(store: &StorePath, session_id: &str) -> Result<PathBuf> {
    let session = state_path(store, session_id)?
        .parent()
        .ok_or_else(|| AppError::new("sync_state_invalid", "Sync session has no directory"))?
        .to_path_buf();
    let directory = session.join(scope_name(store.scope));
    ensure_real_directory(&directory)?;
    Ok(directory)
}

fn baseline_path(store: &StorePath, peer_store_id: &str, lane: &str) -> Result<PathBuf> {
    if peer_store_id.len() != 64
        || !peer_store_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::new(
            "sync_protocol_invalid",
            "peer store ID is invalid",
        ));
    }
    if !matches!(lane, "local" | "remote") {
        return Err(AppError::new(
            "sync_state_invalid",
            "Sync baseline lane must be local or remote",
        ));
    }
    let root = require_database_runtime_root(&store.path)?.join("sync");
    ensure_real_directory(&root)?;
    let baselines = root.join("baselines");
    ensure_real_directory(&baselines)?;
    let peer = baselines.join(peer_store_id);
    ensure_real_directory(&peer)?;
    Ok(peer.join(format!("{}-{lane}.db", scope_name(store.scope))))
}

fn update_baseline(
    store: &StorePath,
    peer_store_id: &str,
    lane: &str,
    source: &Path,
) -> Result<()> {
    let destination = baseline_path(store, peer_store_id, lane)?;
    if fs::symlink_metadata(&destination).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::new(
            "sync_state_invalid",
            "Sync baseline must not be a symbolic link",
        ));
    }
    let temporary = destination.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    reject_non_regular_artifact(source)?;
    reject_non_regular_artifact(&temporary)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temporary)?;
    let mut input = fs::File::open(source)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    if let Err(error) = replace_file(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn reject_non_regular_artifact(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(AppError::new(
            "sync_state_invalid",
            format!("Sync artifact is not a regular file: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remote_directory_for_store(scope: Scope, directory: Option<&Path>) -> Option<PathBuf> {
    if scope == Scope::Global {
        None
    } else {
        directory.map(Path::to_path_buf)
    }
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::new(
                "sync_state_invalid",
                format!(
                    "Sync state path is not a real directory: {}",
                    path.display()
                ),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn read_state_with_digest(store: &StorePath, session_id: &str) -> Result<(SessionState, String)> {
    let path = state_path(store, session_id)?;
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(AppError::new(
            "sync_state_invalid",
            "Sync state file must not be a symbolic link",
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        AppError::new(
            "sync_session_not_found",
            format!("cannot read Sync session {session_id}: {error}"),
        )
    })?;
    let state: SessionState = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    if state.session_id != session_id || state.protocol != PROTOCOL_VERSION {
        return Err(AppError::new(
            "sync_state_invalid",
            "Sync state identity or protocol does not match its path",
        ));
    }
    Ok((state, hex_digest(&bytes)))
}

fn read_state_consistent(stores: &[StorePath], session_id: &str) -> Result<SessionState> {
    let mut states = Vec::with_capacity(stores.len());
    let mut failures = Vec::new();
    for store in stores {
        match read_state_with_digest(store, session_id) {
            Ok(state) => states.push(state),
            Err(error) => failures.push((store.scope, error.code)),
        }
    }
    if states.is_empty() {
        let (scope, cause) = failures
            .into_iter()
            .next()
            .expect("at least one Sync store");
        return Err(AppError::new(
            "sync_state_conflict",
            "Sync session receipts are incomplete across requested scopes",
        )
        .with_details(json!({"cause": cause, "scope": scope})));
    }
    let newest = states
        .iter()
        .max_by_key(|(state, _digest)| state.state_revision)
        .expect("at least one Sync store")
        .clone();
    for candidate in &states {
        if candidate != &newest
            && !session_state_is_predecessor(&candidate.0, &candidate.1, &newest.0)
        {
            return Err(AppError::new(
                "sync_state_conflict",
                "Sync session receipts disagree across requested scopes",
            )
            .with_details(json!({
                "newest_revision": newest.0.state_revision,
                "candidate_revision": candidate.0.state_revision,
                "expected_previous_digest": newest.0.previous_state_digest,
                "candidate_digest": candidate.1,
            })));
        }
    }
    if !failures.is_empty() || states.iter().any(|state| state != &newest) {
        write_state_copies(stores, &newest.0)?;
    }
    Ok(newest.0)
}

fn session_state_is_predecessor(
    older: &SessionState,
    older_digest: &str,
    newer: &SessionState,
) -> bool {
    if newer.state_revision != older.state_revision.saturating_add(1)
        || newer.previous_state_digest.as_deref() != Some(older_digest)
        || older.protocol != newer.protocol
        || older.session_id != newer.session_id
        || older.mode != newer.mode
        || older.scope != newer.scope
        || older.host != newer.host
        || older.remote_directory != newer.remote_directory
    {
        return false;
    }
    true
}

fn write_state_all(stores: &[StorePath], state: &mut SessionState) -> Result<()> {
    let previous_path = state_path(&stores[0], &state.session_id)?;
    state.previous_state_digest = match fs::read(previous_path) {
        Ok(bytes) => Some(hex_digest(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    state.state_revision = state
        .state_revision
        .checked_add(1)
        .ok_or_else(|| AppError::new("sync_state_invalid", "Sync session revision overflowed"))?;
    write_state_copies(stores, state)
}

fn write_state_copies(stores: &[StorePath], state: &SessionState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    for store in stores {
        write_atomic(&state_path(store, &state.session_id)?, &bytes)?;
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Err(error) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Some(parent) = path.parent()
        && let Ok(directory) = fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn now_unix_ms() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|error| AppError::new("clock_error", error.to_string()))
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

fn mode_name(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Merge => "merge",
        SyncMode::Pull => "pull",
        SyncMode::Push => "push",
    }
}

fn remote_cli_directory(directory: Option<&Path>) -> String {
    directory
        .map(|path| format!(" {}", path.display()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_export_does_not_leave_a_resumable_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("remote.db");
        let transfer = PeerTransfer {
            protocol: PROTOCOL_VERSION,
            action: "export".to_string(),
            session_id: "0123456789abcdef0123456789abcdef".to_string(),
            scope: Scope::Project,
            kind: SyncTransferKind::Full,
            size: 3,
            state_digest: "unused".to_string(),
            baseline_digest: None,
            artifact_digest: hex_digest(b"abc"),
            identity: StoreIdentity {
                store_id: "0".repeat(64),
                revision: "0".repeat(64),
                operation_id: 0,
            },
        };
        let mut wire = serde_json::to_vec(&transfer).unwrap();
        wire.extend_from_slice(b"\nabcx");

        assert!(receive_peer_export(wire.as_slice(), &destination).is_err());
        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[test]
    fn receive_rejects_a_symlink_artifact_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let artifact = temp.path().join("artifact");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, &artifact).unwrap();

        let error = receive_exact_file(&mut b"evil".as_slice(), &artifact, 4).unwrap_err();
        assert_eq!(error.code, "sync_state_invalid");
        assert_eq!(fs::read(target).unwrap(), b"keep");
    }

    #[test]
    fn declared_payload_size_requires_immediate_eof() {
        assert!(require_payload_eof(&mut b"".as_slice()).is_ok());
        let error = require_payload_eof(&mut b"x".as_slice()).unwrap_err();
        assert_eq!(error.code, "sync_protocol_invalid");
    }

    #[test]
    fn continuity_requires_an_explicit_completed_status() {
        assert!(continuity_succeeded(&json!({"status":"completed"})));
        assert!(!continuity_succeeded(&json!({"status":"failed"})));
        assert!(!continuity_succeeded(&json!({})));
    }
}
