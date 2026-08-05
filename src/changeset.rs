use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath},
    store::{ChangesetDraftState, ChangesetPublishInput, ChangesetRollbackInput, Store},
};
use serde::Serialize;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Debug, Serialize)]
pub struct ChangesetBeginResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ChangesetShowResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub draft_revision: String,
    pub staged_operation_count: usize,
    pub action_counts: std::collections::BTreeMap<String, usize>,
    pub operations: Vec<crate::store::OperationRecord>,
    pub lint_issues: usize,
    pub empty: bool,
    pub conflict: bool,
    pub committable: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ChangesetListResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changesets: Vec<ChangesetShowResponse>,
}

#[derive(Debug, Serialize)]
pub struct ChangesetDiscardResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ChangesetCommitResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
    pub base_revision: String,
    pub post_revision: String,
    pub checkpoint: String,
    pub staged_operation_count: usize,
    pub lint_issues: usize,
    pub materialized: bool,
    pub wal_checkpointed: bool,
    pub duration_ms: u64,
    pub checkpoint_ms: u64,
    pub locked_publish_ms: u64,
    pub wal_checkpoint_ms: u64,
    pub cleanup_ms: u64,
    pub materialization_ms: u64,
}

#[derive(Debug, Serialize)]
pub struct ChangesetRollbackResponse {
    pub scope: &'static str,
    pub database: PathBuf,
    pub changeset_id: String,
    pub name: String,
    pub status: &'static str,
    pub rollback_revision: String,
    pub checkpoint: String,
    pub materialized: bool,
    pub wal_checkpointed: bool,
    pub duration_ms: u64,
    pub checkpoint_ms: u64,
    pub locked_rollback_ms: u64,
    pub wal_checkpoint_ms: u64,
    pub materialization_ms: u64,
}

pub fn begin(live: &StorePath, name: &str) -> Result<ChangesetBeginResponse> {
    let started = Instant::now();
    validate_name(name)?;
    let path = draft_path(live, name, true)?;
    reject_existing_draft(&path)?;

    let live_store = Store::open_for_read(scope_name(live.scope), &live.path)?;
    let base = live_store.identity()?;
    live_store.snapshot_to(&path).map_err(|error| {
        if error.code == "checkpoint_exists" {
            AppError::new(
                "changeset_exists",
                format!("draft changeset already exists: {name}"),
            )
        } else {
            error
        }
    })?;

    let result = (|| -> Result<ChangesetDraftState> {
        let mut draft = Store::open(scope_name(live.scope), &path)?;
        draft.changeset_begin(name, &base)
    })();
    let state = match result {
        Ok(state) => state,
        Err(error) => {
            let _ = remove_draft_files(&path);
            return Err(error);
        }
    };
    Ok(ChangesetBeginResponse {
        scope: scope_name(live.scope),
        database: path,
        changeset_id: state.id,
        name: state.name,
        status: state.status,
        base_revision: state.base_revision,
        duration_ms: elapsed_millis(started),
    })
}

pub fn resolve_effective(live: StorePath, name: Option<&str>) -> Result<StorePath> {
    let Some(name) = name else {
        return Ok(live);
    };
    let path = draft_path(&live, name, false)?;
    validate_draft_binding(&live, name, &path, 0)?;
    Ok(live.with_database(path))
}

pub fn show(live: &StorePath, name: &str, limit: usize) -> Result<ChangesetShowResponse> {
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    show_path(live, name, path, limit)
}

pub fn list(live: &StorePath, limit: usize) -> Result<ChangesetListResponse> {
    let directory = changeset_directory(live, false)?;
    if !directory.exists() {
        return Ok(ChangesetListResponse {
            scope: scope_name(live.scope),
            database: live.path.clone(),
            changesets: Vec::new(),
        });
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_path(&path));
        }
        if !metadata.is_file() || path.extension().and_then(|value| value.to_str()) != Some("db") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            return Err(invalid_path(&path));
        };
        validate_name(name)?;
        names.push(name.to_string());
    }
    names.sort();
    let mut changesets = Vec::with_capacity(names.len().min(limit));
    for name in names.into_iter().take(limit) {
        changesets.push(show(live, &name, 0)?);
    }
    Ok(ChangesetListResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changesets,
    })
}

pub fn discard(live: &StorePath, name: &str) -> Result<ChangesetDiscardResponse> {
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    let state = validate_draft_binding(live, name, &path, 0)?;
    remove_draft_files(&path)?;
    Ok(ChangesetDiscardResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: state.id,
        name: state.name,
        status: "discarded",
    })
}

pub fn commit(
    live: &StorePath,
    name: &str,
    allow_lint_issues: bool,
    reason: Option<&str>,
) -> Result<ChangesetCommitResponse> {
    let started = Instant::now();
    validate_lint_override(allow_lint_issues, reason)?;
    validate_name(name)?;
    let path = draft_path(live, name, false)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(invalid_path(&path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::new(
                "changeset_not_found",
                format!("draft changeset not found: {name}"),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    let state = validate_draft_binding(live, name, &path, 0)?;
    if state.staged_operation_count == 0 {
        return Err(AppError::new(
            "changeset_empty",
            "changeset has no staged operation after begin",
        ));
    }

    let live_reader = Store::open_for_read(scope_name(live.scope), &live.path)?;
    if let Some(committed) = live_reader.changeset_committed_by_id(&state.id)? {
        drop(live_reader);
        return finish_committed(live, &path, committed, started, 0);
    }
    let live_identity = live_reader.identity()?;
    let draft = Store::open_for_read(scope_name(live.scope), &path)?;
    if live_identity.store_id != draft.identity()?.store_id
        || live_identity.revision != state.base_revision
    {
        return Err(AppError::new(
            "changeset_conflict",
            "live Wiki changed after the changeset began",
        ));
    }
    let lint_issues = draft.lint(1, 0)?.total;
    if lint_issues > 0 && !allow_lint_issues {
        return Err(AppError::new(
            "changeset_lint_failed",
            format!("changeset has {lint_issues} lint issue(s); repair it before commit"),
        ));
    }
    drop(draft);
    let mut draft_store = Store::open(scope_name(live.scope), &path)?;
    draft_store.changeset_freeze(
        &state.id,
        &state.draft_revision,
        state.draft_operation_id,
        state.staged_operation_count,
    )?;
    drop(draft_store);
    let checkpoint_started = Instant::now();
    let checkpoint = live_reader.changeset_checkpoint_create(&state.id)?;
    let checkpoint_ms = elapsed_millis(checkpoint_started);
    drop(live_reader);

    let mut live_store = Store::open(scope_name(live.scope), &live.path)?;
    let committed = live_store.changeset_publish(
        &path,
        &ChangesetPublishInput {
            id: state.id,
            name: state.name,
            store_id: live_identity.store_id,
            base_revision: state.base_revision,
            draft_revision: state.draft_revision,
            draft_operation_id: state.draft_operation_id,
            staged_operation_count: state.staged_operation_count,
            checkpoint: checkpoint.checkpoint,
            lint_issues,
            lint_override_reason: reason.map(str::to_string),
        },
    )?;
    drop(live_store);
    finish_committed(live, &path, committed, started, checkpoint_ms)
}

fn finish_committed(
    live: &StorePath,
    path: &Path,
    committed: crate::store::ChangesetCommitState,
    started: Instant,
    checkpoint_ms: u64,
) -> Result<ChangesetCommitResponse> {
    let wal_checkpoint_started = Instant::now();
    let wal_checkpointed = Store::open(scope_name(live.scope), &live.path)
        .and_then(|store| store.checkpoint_wal_truncate());
    let wal_checkpointed = match wal_checkpointed {
        Ok(checkpointed) => checkpointed,
        Err(error) => {
            return Err(AppError::new(
                "changeset_committed_wal_checkpoint_failed",
                format!("changeset committed but WAL checkpoint failed: {error}"),
            )
            .with_details(json!({
                "committed": true,
                "changeset_id": committed.changeset_id,
                "checkpoint": committed.checkpoint,
                "recovery_command": format!("lwc changeset commit {}", committed.name),
            })));
        }
    };
    let wal_checkpoint_ms = elapsed_millis(wal_checkpoint_started);
    let cleanup_started = Instant::now();
    if let Err(error) = remove_draft_files(path) {
        return Err(AppError::new(
            "changeset_committed_cleanup_failed",
            format!("changeset committed but draft cleanup failed: {error}"),
        )
        .with_details(json!({
            "committed": true,
            "changeset_id": committed.changeset_id,
            "checkpoint": committed.checkpoint,
            "recovery_command": format!("lwc changeset commit {}", committed.name),
        })));
    }
    let cleanup_ms = elapsed_millis(cleanup_started);
    let materialization_started = Instant::now();
    let materialized =
        Store::open(scope_name(live.scope), &live.path).and_then(|mut store| store.materialize());
    if let Err(error) = materialized {
        return Err(AppError::new(
            "changeset_committed_materialization_failed",
            format!("changeset committed but Markdown materialization failed: {error}"),
        )
        .with_details(json!({
            "committed": true,
            "changeset_id": committed.changeset_id,
            "checkpoint": committed.checkpoint,
            "recovery_command": "lwc maintenance materialize",
        })));
    }
    let materialization_ms = elapsed_millis(materialization_started);
    Ok(ChangesetCommitResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: committed.changeset_id,
        name: committed.name,
        status: "committed",
        base_revision: committed.base_revision,
        post_revision: committed.post_revision,
        checkpoint: committed.checkpoint,
        staged_operation_count: committed.staged_operation_count,
        lint_issues: committed.lint_issues,
        materialized: true,
        wal_checkpointed,
        duration_ms: elapsed_millis(started),
        checkpoint_ms,
        locked_publish_ms: committed.locked_publish_ms,
        wal_checkpoint_ms,
        cleanup_ms,
        materialization_ms,
    })
}

pub fn rollback(live: &StorePath, changeset_id: &str) -> Result<ChangesetRollbackResponse> {
    let started = Instant::now();
    validate_id(changeset_id)?;
    let live_reader = Store::open_for_read(scope_name(live.scope), &live.path)?;
    let history = live_reader
        .changeset_history_by_id(changeset_id)?
        .ok_or_else(|| {
            AppError::new(
                "changeset_not_found",
                format!("committed changeset not found: {changeset_id}"),
            )
        })?;
    if history.status == "rolled_back" {
        let state = live_reader
            .changeset_rollback_state_by_id(changeset_id)?
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rolled-back changeset has no rollback operation",
                )
            })?;
        drop(live_reader);
        return finish_rolled_back(live, state, started, 0);
    }
    if history.status != "committed" {
        return Err(AppError::new(
            "changeset_not_found",
            format!("changeset is not committed: {changeset_id}"),
        ));
    }
    let identity = live_reader.identity()?;
    if history.post_revision.as_deref() != Some(identity.revision.as_str()) {
        return Err(AppError::new(
            "changeset_rollback_conflict",
            "live Wiki changed after this changeset committed",
        ));
    }
    live_reader.changeset_rollback_checkpoint_validate(&history, &identity.store_id)?;
    let checkpoint_started = Instant::now();
    let checkpoint = live_reader.changeset_rollback_checkpoint_create(changeset_id)?;
    let checkpoint_ms = elapsed_millis(checkpoint_started);
    drop(live_reader);

    let mut live_store = Store::open(scope_name(live.scope), &live.path)?;
    let rolled_back = live_store.changeset_rollback(&ChangesetRollbackInput {
        history,
        store_id: identity.store_id,
        pre_rollback_checkpoint: checkpoint.checkpoint,
    })?;
    drop(live_store);
    finish_rolled_back(live, rolled_back, started, checkpoint_ms)
}

fn finish_rolled_back(
    live: &StorePath,
    rolled_back: crate::store::ChangesetRollbackState,
    started: Instant,
    checkpoint_ms: u64,
) -> Result<ChangesetRollbackResponse> {
    let wal_checkpoint_started = Instant::now();
    let wal_checkpointed = Store::open(scope_name(live.scope), &live.path)
        .and_then(|store| store.checkpoint_wal_truncate());
    let wal_checkpointed = match wal_checkpointed {
        Ok(checkpointed) => checkpointed,
        Err(error) => {
            return Err(AppError::new(
                "changeset_rolled_back_wal_checkpoint_failed",
                format!("changeset rolled back but WAL checkpoint failed: {error}"),
            )
            .with_details(json!({
                "rolled_back": true,
                "changeset_id": rolled_back.changeset_id,
                "checkpoint": rolled_back.checkpoint,
                "recovery_command": format!(
                    "lwc changeset rollback {}",
                    rolled_back.changeset_id
                ),
            })));
        }
    };
    let wal_checkpoint_ms = elapsed_millis(wal_checkpoint_started);
    let materialization_started = Instant::now();
    let materialized =
        Store::open(scope_name(live.scope), &live.path).and_then(|mut store| store.materialize());
    if let Err(error) = materialized {
        return Err(AppError::new(
            "changeset_rolled_back_materialization_failed",
            format!("changeset rolled back but Markdown materialization failed: {error}"),
        )
        .with_details(json!({
            "rolled_back": true,
            "changeset_id": rolled_back.changeset_id,
            "checkpoint": rolled_back.checkpoint,
            "recovery_command": "lwc maintenance materialize",
        })));
    }
    let materialization_ms = elapsed_millis(materialization_started);
    Ok(ChangesetRollbackResponse {
        scope: scope_name(live.scope),
        database: live.path.clone(),
        changeset_id: rolled_back.changeset_id,
        name: rolled_back.name,
        status: "rolled_back",
        rollback_revision: rolled_back.rollback_revision,
        checkpoint: rolled_back.checkpoint,
        materialized: true,
        wal_checkpointed,
        duration_ms: elapsed_millis(started),
        checkpoint_ms,
        locked_rollback_ms: rolled_back.locked_rollback_ms,
        wal_checkpoint_ms,
        materialization_ms,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub fn reject_selector(selector: Option<&str>, command: &str) -> Result<()> {
    if selector.is_some() {
        return Err(AppError::new(
            "changeset_command_unsupported",
            format!("{command} cannot be combined with --changeset"),
        ));
    }
    Ok(())
}

fn show_path(
    live: &StorePath,
    name: &str,
    path: PathBuf,
    limit: usize,
) -> Result<ChangesetShowResponse> {
    let state = validate_draft_binding(live, name, &path, limit)?;
    let live_identity = Store::open_for_read(scope_name(live.scope), &live.path)?.identity()?;
    let draft = Store::open_for_read(scope_name(live.scope), &path)?;
    let lint_issues = draft.lint(1, 0)?.total;
    let empty = state.staged_operation_count == 0;
    let conflict = live_identity.store_id != draft.identity()?.store_id
        || live_identity.revision != state.base_revision;
    Ok(ChangesetShowResponse {
        scope: scope_name(live.scope),
        database: path,
        changeset_id: state.id,
        name: state.name,
        status: state.status,
        base_revision: state.base_revision,
        draft_revision: state.draft_revision,
        staged_operation_count: state.staged_operation_count,
        action_counts: state.action_counts,
        operations: state.operations,
        lint_issues,
        empty,
        conflict,
        committable: !empty && !conflict && lint_issues == 0,
        created_at: state.created_at,
    })
}

fn validate_draft_binding(
    live: &StorePath,
    name: &str,
    path: &Path,
    limit: usize,
) -> Result<ChangesetDraftState> {
    require_regular_file(path)?;
    let draft = Store::open_for_read(scope_name(live.scope), path)
        .map_err(|error| map_draft_error(error, name))?;
    let state = draft
        .changeset_draft(name, limit)
        .map_err(|error| map_draft_error(error, name))?;
    let live_identity = Store::open_for_read(scope_name(live.scope), &live.path)?.identity()?;
    let draft_identity = draft.identity()?;
    if live_identity.store_id != draft_identity.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            format!("draft changeset {name} is not bound to the selected Wiki"),
        ));
    }
    Ok(state)
}

fn map_draft_error(error: AppError, name: &str) -> AppError {
    if error.code == "store_not_found" || error.code == "changeset_not_found" {
        AppError::new(
            "changeset_not_found",
            format!("draft changeset not found: {name}"),
        )
    } else {
        error
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || name != name.trim()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "changeset_name_invalid",
            "changeset name must be one safe filename segment of at most 80 bytes",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(AppError::new(
        "changeset_not_found",
        "changeset id must be a 64-character hexadecimal value",
    ))
}

fn validate_lint_override(allow: bool, reason: Option<&str>) -> Result<()> {
    match (allow, reason) {
        (false, None) => return Ok(()),
        (true, Some(value)) if !value.trim().is_empty() => return Ok(()),
        _ => {}
    }
    Err(AppError::new(
        "changeset_lint_override_invalid",
        "--allow-lint-issues and a nonblank --reason must be provided together",
    ))
}

fn draft_path(live: &StorePath, name: &str, create_directory: bool) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(changeset_directory(live, create_directory)?.join(format!("{name}.db")))
}

fn changeset_directory(live: &StorePath, create: bool) -> Result<PathBuf> {
    let parent = live
        .path
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?;
    let directory = parent.join("changesets");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid_path(&directory));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            fs::create_dir(&directory)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(directory)
}

fn reject_existing_draft(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(invalid_path(path))
        }
        Ok(_) => Err(AppError::new(
            "changeset_exists",
            format!("draft changeset already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn require_regular_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(invalid_path(path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(AppError::new(
            "changeset_not_found",
            format!("draft changeset not found: {}", path.display()),
        ))?,
        Err(error) => return Err(error.into()),
    }
    for sidecar in [
        database_sidecar(path, "-wal"),
        database_sidecar(path, "-shm"),
    ] {
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path(&sidecar));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_draft_files(database: &Path) -> Result<()> {
    let paths = [
        database_sidecar(database, "-wal"),
        database_sidecar(database, "-shm"),
        database.to_path_buf(),
    ];
    for path in &paths {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path(path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in paths {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn database_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn invalid_path(path: &Path) -> AppError {
    AppError::new(
        "changeset_path_invalid",
        format!(
            "changeset path is not a regular owned path: {}",
            path.display()
        ),
    )
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}
