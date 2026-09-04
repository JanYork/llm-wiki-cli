use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath, init_store_path, resolve_store_path},
    store::{
        Store, StoreIdentity, archive_publication_receipt, cleanup_sync_conflict_candidates,
        create_empty_sync_state, merge_sync_states_directional, next_sync_conflict_batch,
        resolve_sync_conflicts, sync_state_digest,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAGIC: &[u8; 8] = b"LWCARC01";
const FORMAT: u8 = 1;
const MAX_HEADER_BYTES: u32 = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_SESSION_STATE_BYTES: u64 = 64 * 1024;
const MAX_CONFLICT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECOVERY_SCAN_ENTRIES: usize = 64;
const MAX_RECOVERY_ITEMS: usize = 3;
const WARNING: &str =
    "This archive contains full plaintext memory; share it only with a trusted recipient.";
static UNIQUE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveHeader {
    format: u8,
    scope: String,
    compression: String,
    payload_bytes: u64,
    payload_sha256: String,
    state_digest: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveSession {
    format: u8,
    scope: Scope,
    session_id: String,
    phase: String,
    kind: String,
    state_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    incoming_digest: Option<String>,
    local_digest: Option<String>,
    base_left_digest: Option<String>,
    base_right_digest: Option<String>,
    merged_digest: String,
    target_identity: Option<StoreIdentity>,
    conflict_count: usize,
    conflicts_sha256: String,
}

struct DecodedArchive {
    header: ArchiveHeader,
    path: PathBuf,
}

impl Drop for DecodedArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn compress(cwd: &Path, scope: Scope, output: Option<&Path>) -> Result<Value> {
    require_scope(scope)?;
    let store_path = resolve_store_path(scope, cwd)?;
    let runtime = store_path
        .path
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "Wiki store has no runtime"))?;
    let destination = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| runtime.join("memory.lwc.zst"));
    validate_output(&destination, output.is_none())?;

    let normalized = unique_sibling(&store_path.path, "archive-export", "db");
    let cleanup = FileCleanup::new(normalized.clone());
    let store = Store::open_for_read(scope_name(scope), &store_path.path)?;
    let summary = store.export_sync_state(&normalized)?;
    set_private_permissions(&normalized)?;
    let payload_bytes = regular_file_len(&normalized, "archive_invalid")?;
    if payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(invalid_archive(
            "normalized state exceeds the archive limit",
        ));
    }
    let payload_sha256 = hash_file(&normalized)?;
    let header = ArchiveHeader {
        format: FORMAT,
        scope: scope_name(scope).to_owned(),
        compression: "zstd".to_owned(),
        payload_bytes,
        payload_sha256: payload_sha256.clone(),
        state_digest: summary.state_digest.clone(),
    };
    write_archive(&normalized, &destination, &header, output.is_none())?;
    drop(cleanup);
    Ok(json!({
        "action":"compressed",
        "scope":scope_name(scope),
        "archive":destination,
        "payload_bytes":payload_bytes,
        "payload_sha256":payload_sha256,
        "state_digest":summary.state_digest,
        "object_count":summary.object_count,
        "blob_count":summary.blob_count,
        "warning":WARNING,
    }))
}

pub(crate) fn decompress(
    cwd: &Path,
    scope: Scope,
    archive: Option<&Path>,
    overwrite: bool,
    confirmation: Option<&str>,
) -> Result<Value> {
    require_scope(scope)?;
    let target = init_store_path(scope, cwd)?;
    let archive = archive_path(&target, archive);
    let decoded = decode_archive(&archive, scope)?;
    if !target.path.is_file() {
        let session = stage_missing(&target, &decoded, "decompress")?;
        drop(decoded);
        return continue_session(&target, session, None);
    }

    let store = Store::open_for_read(scope_name(scope), &target.path)?;
    let identity = store.identity()?;
    let local = unique_sibling(&target.path, "archive-local", "db");
    let cleanup = FileCleanup::new(local.clone());
    let local_summary = store.export_sync_state(&local)?;
    set_private_permissions(&local)?;
    if local_summary.state_digest == decoded.header.state_digest {
        return Ok(json!({
            "action":"unchanged",
            "scope":scope_name(scope),
            "committed":false,
            "state_digest":decoded.header.state_digest,
        }));
    }

    if overwrite {
        drop(store);
        drop(cleanup);
        let token = confirmation_token(
            scope,
            &target.path,
            &decoded.header.state_digest,
            &decoded.header.payload_sha256,
            &identity,
        )?;
        if confirmation.is_none() {
            return Ok(json!({
                "action":"confirmation_required",
                "scope":scope_name(scope),
                "requires_consent":true,
                "confirmation_token":token,
                "warning":WARNING,
            }));
        }
        if confirmation != Some(token.as_str()) {
            return Err(AppError::new(
                "archive_confirmation_stale",
                "the archive overwrite confirmation no longer matches the archive and target",
            ));
        }
        let session = stage_overwrite(&target, &decoded, identity)?;
        drop(decoded);
        return continue_session(&target, session, None).map_err(map_confirmation_error);
    }

    let session = stage_existing(&target, &decoded, &local, identity, "decompress")?;
    drop(store);
    drop(cleanup);
    drop(decoded);
    Ok(staged_response(scope, &session.session_id))
}

pub(crate) fn merge(
    cwd: &Path,
    scope: Scope,
    archive: Option<&Path>,
    resume: Option<&str>,
    resolution: Option<&Path>,
) -> Result<Value> {
    require_scope(scope)?;
    let target = init_store_path(scope, cwd)?;
    if let Some(session_id) = resume {
        let session = load_session(&target, session_id)?;
        return continue_session(&target, session, resolution);
    }

    let archive = archive_path(&target, archive);
    let decoded = decode_archive(&archive, scope)?;
    if !target.path.is_file() {
        let session = stage_missing(&target, &decoded, "merge")?;
        drop(decoded);
        return continue_session(&target, session, None);
    }
    let store = Store::open_for_read(scope_name(scope), &target.path)?;
    let identity = store.identity()?;
    let local = unique_sibling(&target.path, "archive-local", "db");
    let cleanup = FileCleanup::new(local.clone());
    let local_summary = store.export_sync_state(&local)?;
    set_private_permissions(&local)?;
    if local_summary.state_digest == decoded.header.state_digest {
        return Ok(json!({
            "action":"unchanged",
            "scope":scope_name(scope),
            "committed":false,
            "state_digest":decoded.header.state_digest,
        }));
    }
    let session = stage_existing(&target, &decoded, &local, identity, "merge")?;
    drop(store);
    drop(cleanup);
    drop(decoded);
    continue_session(&target, session, None)
}

pub(crate) fn recovery_readiness(
    scope: Scope,
    cwd: &Path,
    deadline: Option<Instant>,
) -> Option<Value> {
    let scopes = match scope {
        Scope::All => &[Scope::Project, Scope::Global][..],
        Scope::Project => &[Scope::Project][..],
        Scope::Global => &[Scope::Global][..],
    };
    let mut recoveries = Vec::new();
    for scope in scopes {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        append_recoveries(*scope, cwd, deadline, &mut recoveries);
        if recoveries.len() == MAX_RECOVERY_ITEMS
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            break;
        }
    }
    (!recoveries.is_empty()).then(|| json!({"recoveries":recoveries}))
}

fn append_recoveries(
    scope: Scope,
    cwd: &Path,
    deadline: Option<Instant>,
    recoveries: &mut Vec<Value>,
) {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return;
    }
    let Ok(target) = resolve_store_path(scope, cwd) else {
        return;
    };
    let Some(runtime) = target.path.parent() else {
        return;
    };
    let imports = runtime.join("imports");
    let Ok(metadata) = fs::symlink_metadata(&imports) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(imports) else {
        return;
    };
    let mut entries = entries
        .take(MAX_RECOVERY_SCAN_ENTRIES)
        .filter_map(std::result::Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if recoveries.len() == MAX_RECOVERY_ITEMS
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            break;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_session_id(&id) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        let Ok(state) = read_session_state(&path.join("state.json")) else {
            continue;
        };
        if state.scope == scope && state.session_id == id && state.phase == "committed" {
            recoveries.push(json!({
                "scope":scope_name(scope),
                "session_id":id,
                "status":"committed",
                "resume":format!("lwc --scope {} merge --resume {}", scope_name(scope), state.session_id),
                "shared_store_maintenance":true,
            }));
        }
    }
}

fn require_scope(scope: Scope) -> Result<()> {
    if scope == Scope::All {
        return Err(AppError::new(
            "archive_scope_unsupported",
            "--scope all is not supported for memory archives",
        ));
    }
    Ok(())
}

fn archive_path(target: &StorePath, requested: Option<&Path>) -> PathBuf {
    requested.map(Path::to_path_buf).unwrap_or_else(|| {
        target
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("memory.lwc.zst")
    })
}

fn write_archive(
    normalized: &Path,
    destination: &Path,
    header: &ArchiveHeader,
    replace: bool,
) -> Result<()> {
    let header = serde_json::to_vec(header)
        .map_err(|error| AppError::new("archive_invalid", error.to_string()))?;
    if header.len() > MAX_HEADER_BYTES as usize {
        return Err(invalid_archive("archive header exceeds its limit"));
    }
    let temporary = unique_sibling(destination, "archive-write", "tmp");
    let mut cleanup = FileCleanup::new(temporary.clone());
    let mut file = create_private_file(&temporary)?;
    file.write_all(MAGIC)?;
    file.write_all(&(header.len() as u32).to_be_bytes())?;
    file.write_all(&header)?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 3)
        .map_err(|error| invalid_archive(error.to_string()))?;
    io::copy(
        &mut BufReader::new(fs::File::open(normalized)?),
        &mut encoder,
    )?;
    let file = encoder
        .finish()
        .map_err(|error| invalid_archive(error.to_string()))?;
    file.sync_all()?;
    publish_file(&temporary, destination, replace)?;
    sync_parent(destination)?;
    cleanup.disarm();
    Ok(())
}

fn decode_archive(path: &Path, scope: Scope) -> Result<DecodedArchive> {
    reject_symlink_ancestors(path)?;
    ensure_regular(path, "archive_unsafe_path")?;
    let file = fs::File::open(path).map_err(|error| invalid_archive(error.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut magic = [0_u8; 8];
    reader
        .read_exact(&mut magic)
        .map_err(|error| invalid_archive(error.to_string()))?;
    if &magic != MAGIC {
        return Err(invalid_archive("archive magic is invalid"));
    }
    let mut length = [0_u8; 4];
    reader
        .read_exact(&mut length)
        .map_err(|error| invalid_archive(error.to_string()))?;
    let length = u32::from_be_bytes(length);
    if length == 0 || length > MAX_HEADER_BYTES {
        return Err(invalid_archive("archive header length is invalid"));
    }
    let mut bytes = vec![0_u8; length as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| invalid_archive(error.to_string()))?;
    let header: ArchiveHeader = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_archive(format!("invalid archive header: {error}")))?;
    validate_header(&header, scope)?;

    let decoded = env::temp_dir().join(format!(
        "lwc-archive-decode-{}.db",
        unique_hex("archive-decode")
    ));
    let mut cleanup = FileCleanup::new(decoded.clone());
    let mut output = create_private_file(&decoded)?;
    let mut decoder = zstd::stream::read::Decoder::with_buffer(reader)
        .map_err(|error| invalid_archive(error.to_string()))?
        .single_frame();
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| invalid_archive(error.to_string()))?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| invalid_archive("archive payload length overflow"))?;
        if copied > header.payload_bytes || copied > MAX_PAYLOAD_BYTES {
            return Err(invalid_archive(
                "archive payload exceeds its declared length",
            ));
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    let mut trailing = decoder.finish();
    if !trailing
        .fill_buf()
        .map_err(|error| invalid_archive(error.to_string()))?
        .is_empty()
    {
        return Err(invalid_archive("archive has trailing compressed data"));
    }
    output.sync_all()?;
    drop(output);
    if copied != header.payload_bytes {
        return Err(invalid_archive("archive payload is truncated"));
    }
    if hex_digest(hasher.finalize()) != header.payload_sha256 {
        return Err(invalid_archive("archive payload checksum differs"));
    }
    let digest = sync_state_digest(&decoded)
        .map_err(|error| invalid_archive(format!("invalid archive payload: {}", error.message)))?;
    if digest != header.state_digest {
        return Err(invalid_archive("archive state digest differs"));
    }
    cleanup.disarm();
    Ok(DecodedArchive {
        header,
        path: decoded,
    })
}

fn validate_header(header: &ArchiveHeader, scope: Scope) -> Result<()> {
    if header.format != FORMAT || header.compression != "zstd" {
        return Err(invalid_archive("unsupported archive format or compression"));
    }
    if header.scope != scope_name(scope) {
        return Err(AppError::new(
            "archive_scope_mismatch",
            "archive scope does not match the selected Wiki scope",
        ));
    }
    if header.payload_bytes > MAX_PAYLOAD_BYTES
        || !is_sha256(&header.payload_sha256)
        || !is_sha256(&header.state_digest)
    {
        return Err(invalid_archive("archive header values are invalid"));
    }
    Ok(())
}

fn stage_missing(
    target: &StorePath,
    decoded: &DecodedArchive,
    kind: &str,
) -> Result<ArchiveSession> {
    let (directory, session_id) = create_session_directory(target)?;
    copy_private(&decoded.path, &directory.join("merged.db"))?;
    let state = ArchiveSession {
        format: FORMAT,
        scope: target.scope,
        session_id,
        phase: "ready".to_owned(),
        kind: kind.to_owned(),
        state_digest: decoded.header.state_digest.clone(),
        incoming_digest: None,
        local_digest: None,
        base_left_digest: None,
        base_right_digest: None,
        merged_digest: decoded.header.state_digest.clone(),
        target_identity: None,
        conflict_count: 0,
        conflicts_sha256: write_conflicts(&directory, &[])?,
    };
    write_session_state(&directory, &state)?;
    Ok(state)
}

fn stage_overwrite(
    target: &StorePath,
    decoded: &DecodedArchive,
    identity: StoreIdentity,
) -> Result<ArchiveSession> {
    let (directory, session_id) = create_session_directory(target)?;
    copy_private(&decoded.path, &directory.join("merged.db"))?;
    let state = ArchiveSession {
        format: FORMAT,
        scope: target.scope,
        session_id,
        phase: "ready".to_owned(),
        kind: "overwrite".to_owned(),
        state_digest: decoded.header.state_digest.clone(),
        incoming_digest: None,
        local_digest: None,
        base_left_digest: None,
        base_right_digest: None,
        merged_digest: decoded.header.state_digest.clone(),
        target_identity: Some(identity),
        conflict_count: 0,
        conflicts_sha256: write_conflicts(&directory, &[])?,
    };
    write_session_state(&directory, &state)?;
    Ok(state)
}

fn stage_existing(
    target: &StorePath,
    decoded: &DecodedArchive,
    local: &Path,
    identity: StoreIdentity,
    kind: &str,
) -> Result<ArchiveSession> {
    let (directory, session_id) = create_session_directory(target)?;
    copy_private(&decoded.path, &directory.join("incoming.db"))?;
    copy_private(local, &directory.join("local.db"))?;
    create_empty_sync_state(&directory.join("base-left.db"))?;
    create_empty_sync_state(&directory.join("base-right.db"))?;
    set_private_permissions(&directory.join("base-left.db"))?;
    set_private_permissions(&directory.join("base-right.db"))?;
    let summary = merge_sync_states_directional(
        &directory.join("base-left.db"),
        &directory.join("local.db"),
        &directory.join("base-right.db"),
        &directory.join("incoming.db"),
        &directory.join("merged.db"),
    )?;
    set_private_permissions(&directory.join("merged.db"))?;
    let conflicts_sha256 = write_conflicts(&directory, &summary.conflicts)?;
    let state = ArchiveSession {
        format: FORMAT,
        scope: target.scope,
        session_id,
        phase: if summary.conflicts.is_empty() {
            "ready"
        } else {
            "conflicts"
        }
        .to_owned(),
        kind: kind.to_owned(),
        state_digest: decoded.header.state_digest.clone(),
        incoming_digest: Some(decoded.header.state_digest.clone()),
        local_digest: Some(sync_state_digest(&directory.join("local.db"))?),
        base_left_digest: Some(sync_state_digest(&directory.join("base-left.db"))?),
        base_right_digest: Some(sync_state_digest(&directory.join("base-right.db"))?),
        merged_digest: summary.state_digest,
        target_identity: Some(identity),
        conflict_count: summary.conflicts.len(),
        conflicts_sha256,
    };
    write_session_state(&directory, &state)?;
    Ok(state)
}

fn continue_session(
    target: &StorePath,
    mut state: ArchiveSession,
    resolution: Option<&Path>,
) -> Result<Value> {
    let directory = session_directory(target, &state.session_id)?;
    validate_session(target, &directory, &state)?;
    if target.path.is_file()
        && let Some(publication) =
            archive_publication_receipt(&target.path, &state.session_id, &state.merged_digest)?
    {
        state.phase = "committed".to_owned();
        write_session_state(&directory, &state)?;
        return finish_rebuild(target, &directory, state, &publication, true);
    }

    let mut conflicts = read_conflicts(&directory, &state.conflicts_sha256)?;
    if !conflicts.is_empty() {
        if let Some(packet) = resolution {
            let batch = next_sync_conflict_batch(&conflicts);
            let packet = read_bounded_json(packet, 256 * 1024)?;
            state.merged_digest =
                resolve_sync_conflicts(&directory.join("merged.db"), &batch, &packet)?;
            conflicts.drain(..batch.len());
            state.conflict_count = conflicts.len();
            state.conflicts_sha256 = write_conflicts(&directory, &conflicts)?;
            if conflicts.is_empty() {
                state.merged_digest =
                    cleanup_sync_conflict_candidates(&directory.join("merged.db"))?;
            }
            state.phase = if conflicts.is_empty() {
                "ready"
            } else {
                "conflicts"
            }
            .to_owned();
            write_session_state(&directory, &state)?;
        }
        if !conflicts.is_empty() {
            return Ok(conflict_response(target.scope, &state, &conflicts));
        }
    } else if resolution.is_some() {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "this archive session has no conflicts to resolve",
        ));
    }

    state.phase = "ready".to_owned();
    write_session_state(&directory, &state)?;
    test_sleep_before_publish();
    let merged = directory.join("merged.db");
    let summary = if let Some(expected) = state.target_identity.as_ref() {
        let mut store = Store::open(scope_name(target.scope), &target.path)?;
        store.publish_sync_state(&merged, expected, &state.session_id)
    } else {
        Store::publish_sync_state_to_missing(
            scope_name(target.scope),
            &target.path,
            &merged,
            &state.session_id,
        )
    }
    .map_err(map_publish_error)?;
    let publication = serde_json::to_value(summary)
        .map_err(|error| AppError::new("archive_invalid", error.to_string()))?;
    state.phase = "committed".to_owned();
    write_session_state(&directory, &state)?;
    if test_fail_after_commit() {
        return Err(committed_error(&state));
    }
    finish_rebuild(target, &directory, state, &publication, false)
}

fn finish_rebuild(
    target: &StorePath,
    directory: &Path,
    mut state: ArchiveSession,
    publication: &Value,
    recovered: bool,
) -> Result<Value> {
    let mut derived = crate::sync::rebuild_derived(target, publication);
    derived["codegraph"] = json!({"status":"not_applicable"});
    let store = Store::open(scope_name(target.scope), &target.path)?;
    store.persist_sync_derived_receipt(&state.session_id, &state.merged_digest, &derived["fts"])?;
    if derived["status"] != "completed" {
        return Err(committed_error(&state));
    }
    state.phase = "completed".to_owned();
    write_session_state(directory, &state)?;
    Ok(json!({
        "action":"completed",
        "scope":scope_name(target.scope),
        "session_id":state.session_id,
        "committed":true,
        "recovered":recovered,
        "state_digest":state.merged_digest,
        "derived":derived,
        "warning":WARNING,
    }))
}

fn conflict_response(scope: Scope, state: &ArchiveSession, conflicts: &[Value]) -> Value {
    json!({
        "action":"conflicts",
        "scope":scope_name(scope),
        "session_id":state.session_id,
        "committed":false,
        "conflicts":next_sync_conflict_batch(conflicts),
        "conflict_count":state.conflict_count,
        "next_action":format!("{} --resolve PACKET", merge_resume(scope, &state.session_id)),
    })
}

fn staged_response(scope: Scope, session_id: &str) -> Value {
    json!({
        "action":"staged",
        "scope":scope_name(scope),
        "session_id":session_id,
        "committed":false,
        "next_action":merge_resume(scope, session_id),
        "warning":WARNING,
    })
}

fn committed_error(state: &ArchiveSession) -> AppError {
    AppError::new(
        "archive_rebuild_incomplete",
        "canonical memory was committed but derived state still needs rebuilding",
    )
    .with_details(json!({
        "committed":true,
        "session_id":state.session_id,
        "next_action":merge_resume(state.scope, &state.session_id),
    }))
}

fn load_session(target: &StorePath, session_id: &str) -> Result<ArchiveSession> {
    if !valid_session_id(session_id) {
        return Err(AppError::new(
            "archive_session_invalid",
            "invalid archive session ID",
        ));
    }
    let directory = session_directory(target, session_id)?;
    ensure_real_directory(&directory)?;
    let state = read_session_state(&directory.join("state.json"))?;
    if state.format != FORMAT || state.scope != target.scope || state.session_id != session_id {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session identity does not match",
        ));
    }
    Ok(state)
}

fn validate_session(target: &StorePath, directory: &Path, state: &ArchiveSession) -> Result<()> {
    let direct = state.incoming_digest.is_none()
        && state.merged_digest == state.state_digest
        && state.local_digest.is_none()
        && state.base_left_digest.is_none()
        && state.base_right_digest.is_none()
        && state.conflict_count == 0;
    let merged = state.incoming_digest.as_ref() == Some(&state.state_digest)
        && state.local_digest.is_some()
        && state.base_left_digest.is_some()
        && state.base_right_digest.is_some()
        && state.target_identity.is_some();
    if state.scope != target.scope
        || state.format != FORMAT
        || !matches!(state.kind.as_str(), "decompress" | "merge" | "overwrite")
        || !(direct || merged)
    {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session is invalid",
        ));
    }
    if let Some(digest) = state.incoming_digest.as_deref() {
        validate_artifact(directory, "incoming.db", digest)?;
    }
    validate_artifact(directory, "merged.db", &state.merged_digest)?;
    if read_conflicts(directory, &state.conflicts_sha256)?.len() != state.conflict_count {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session conflict count changed",
        ));
    }
    for (name, digest) in [
        ("local.db", state.local_digest.as_deref()),
        ("base-left.db", state.base_left_digest.as_deref()),
        ("base-right.db", state.base_right_digest.as_deref()),
    ] {
        if let Some(digest) = digest {
            validate_artifact(directory, name, digest)?;
        }
    }
    Ok(())
}

fn validate_artifact(directory: &Path, name: &str, expected: &str) -> Result<()> {
    let path = directory.join(name);
    ensure_regular(&path, "archive_session_invalid")?;
    if sync_state_digest(&path)? != expected {
        return Err(AppError::new(
            "archive_session_invalid",
            format!("archive session artifact {name} changed"),
        ));
    }
    Ok(())
}

fn create_session_directory(target: &StorePath) -> Result<(PathBuf, String)> {
    let runtime = target
        .path
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "Wiki store has no runtime"))?;
    ensure_directory(runtime)?;
    let imports = runtime.join("imports");
    ensure_directory(&imports)?;
    for _ in 0..16 {
        let id = unique_hex("archive-session")[..32].to_owned();
        let path = imports.join(&id);
        match fs::create_dir(&path) {
            Ok(()) => {
                set_private_directory_permissions(&path)?;
                return Ok((path, id));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::new(
        "archive_session_invalid",
        "could not allocate an archive session",
    ))
}

fn session_directory(target: &StorePath, session_id: &str) -> Result<PathBuf> {
    if !valid_session_id(session_id) {
        return Err(AppError::new(
            "archive_session_invalid",
            "invalid archive session ID",
        ));
    }
    Ok(target
        .path
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "Wiki store has no runtime"))?
        .join("imports")
        .join(session_id))
}

fn write_session_state(directory: &Path, state: &ArchiveSession) -> Result<()> {
    write_json_atomic(&directory.join("state.json"), state)
}

fn write_conflicts(directory: &Path, conflicts: &[Value]) -> Result<String> {
    let bytes = serde_json::to_vec(conflicts)
        .map_err(|error| AppError::new("archive_session_invalid", error.to_string()))?;
    if bytes.len() as u64 > MAX_CONFLICT_BYTES {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive conflicts exceed their limit",
        ));
    }
    let digest = hex_digest(Sha256::digest(&bytes));
    write_bytes_atomic(&directory.join("conflicts.json"), &bytes)?;
    Ok(digest)
}

fn read_conflicts(directory: &Path, expected: &str) -> Result<Vec<Value>> {
    let path = directory.join("conflicts.json");
    ensure_regular(&path, "archive_session_invalid")?;
    if fs::metadata(&path)?.len() > MAX_CONFLICT_BYTES {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive conflicts exceed their limit",
        ));
    }
    let bytes = fs::read(path)?;
    if hex_digest(Sha256::digest(&bytes)) != expected {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session conflicts changed",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::new("archive_session_invalid", error.to_string()))
}

fn read_session_state(path: &Path) -> Result<ArchiveSession> {
    ensure_regular(path, "archive_session_invalid")?;
    if fs::metadata(path)?.len() > MAX_SESSION_STATE_BYTES {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session state exceeds its limit",
        ));
    }
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| AppError::new("archive_session_invalid", error.to_string()))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AppError::new("archive_session_invalid", error.to_string()))?;
    if bytes.len() as u64 > MAX_SESSION_STATE_BYTES {
        return Err(AppError::new(
            "archive_session_invalid",
            "archive session state exceeds its limit",
        ));
    }
    write_bytes_atomic(path, &bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = unique_sibling(path, "archive-state", "tmp");
    let mut cleanup = FileCleanup::new(temporary.clone());
    let mut file = create_private_file(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    replace_file(&temporary, path)?;
    sync_parent(path)?;
    cleanup.disarm();
    Ok(())
}

fn read_bounded_json(path: &Path, limit: u64) -> Result<Value> {
    ensure_regular(path, "sync_resolution_invalid")?;
    if fs::metadata(path)?.len() > limit {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution packet exceeds its limit",
        ));
    }
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| AppError::new("sync_resolution_invalid", error.to_string()))
}

fn confirmation_token(
    scope: Scope,
    database: &Path,
    state_digest: &str,
    payload_sha256: &str,
    identity: &StoreIdentity,
) -> Result<String> {
    let canonical = fs::canonicalize(database)?;
    let canonical = canonical.as_os_str().to_string_lossy();
    let identity = serde_json::to_vec(identity)
        .map_err(|error| AppError::new("archive_invalid", error.to_string()))?;
    let mut hasher = Sha256::new();
    for bytes in [
        b"lwc-archive-overwrite-v1".as_slice(),
        scope_name(scope).as_bytes(),
        canonical.as_bytes(),
        state_digest.as_bytes(),
        payload_sha256.as_bytes(),
        identity.as_slice(),
    ] {
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn validate_output(path: &Path, replace: bool) -> Result<()> {
    reject_symlink_ancestors(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(unsafe_path(path))
        }
        Ok(_) if !replace => Err(AppError::new(
            "archive_output_exists",
            format!("custom archive output already exists: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            ensure_real_directory(parent)
        }
        Err(error) => Err(error.into()),
    }
}

fn reject_symlink_ancestors(path: &Path) -> Result<()> {
    for candidate in path.ancestors() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if system_root_alias(candidate) {
                    continue;
                }
                return Err(unsafe_path(path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn system_root_alias(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        matches!(path.to_str(), Some("/tmp" | "/var" | "/etc"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

fn publish_file(temporary: &Path, destination: &Path, replace: bool) -> Result<()> {
    if replace {
        replace_file(temporary, destination)
    } else {
        fs::hard_link(temporary, destination)?;
        fs::remove_file(temporary)?;
        Ok(())
    }
}

fn ensure_regular(path: &Path, code: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if code == "archive_unsafe_path" {
            unsafe_path(path)
        } else {
            AppError::new(code, error.to_string())
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(if code == "archive_unsafe_path" {
            unsafe_path(path)
        } else {
            AppError::new(
                code,
                format!("path is not a regular file: {}", path.display()),
            )
        });
    }
    Ok(())
}

fn regular_file_len(path: &Path, code: &'static str) -> Result<u64> {
    ensure_regular(path, code)?;
    Ok(fs::metadata(path)?.len())
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unsafe_path(path))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unsafe_path(path));
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_real_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| unsafe_path(path))?;
            if parent != path {
                ensure_directory(parent)?;
            }
            fs::create_dir(path)?;
            ensure_real_directory(path)
        }
        Err(error) => Err(error.into()),
    }
}

fn unsafe_path(path: &Path) -> AppError {
    AppError::new(
        "archive_unsafe_path",
        format!(
            "archive path is not a regular non-symlink path: {}",
            path.display()
        ),
    )
}

fn invalid_archive(message: impl Into<String>) -> AppError {
    AppError::new("archive_invalid", message)
}

fn map_publish_error(error: AppError) -> AppError {
    if error.code == "sync_store_changed" {
        AppError::new(
            "archive_store_changed",
            "the target Wiki changed after the archive session was staged",
        )
    } else {
        error
    }
}

fn map_confirmation_error(error: AppError) -> AppError {
    if error.code == "archive_store_changed" || error.code == "sync_store_changed" {
        AppError::new(
            "archive_confirmation_stale",
            "the target Wiki changed after overwrite confirmation",
        )
    } else {
        error
    }
}

fn create_private_file(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn copy_private(source: &Path, destination: &Path) -> Result<()> {
    let temporary = unique_sibling(destination, "archive-copy", "tmp");
    let mut cleanup = FileCleanup::new(temporary.clone());
    let mut output = create_private_file(&temporary)?;
    io::copy(&mut BufReader::new(fs::File::open(source)?), &mut output)?;
    output.sync_all()?;
    fs::rename(&temporary, destination)?;
    cleanup.disarm();
    Ok(())
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = BufReader::new(fs::File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn unique_sibling(path: &Path, prefix: &str, extension: &str) -> PathBuf {
    let sequence = UNIQUE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("memory");
    path.with_file_name(format!(
        ".{name}.{prefix}-{}-{sequence}.{extension}",
        std::process::id()
    ))
}

fn unique_hex(domain: &str) -> String {
    let sequence = UNIQUE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(std::process::id().to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(sequence.to_be_bytes());
    hex_digest(hasher.finalize())
}

fn valid_session_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn merge_resume(scope: Scope, session_id: &str) -> String {
    match scope {
        Scope::Project => format!("lwc merge --resume {session_id}"),
        Scope::Global => format!("lwc --scope global merge --resume {session_id}"),
        Scope::All => unreachable!("archive scope was validated"),
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

fn test_sleep_before_publish() {
    if cfg!(debug_assertions)
        && let Ok(value) = env::var("LWC_TEST_ARCHIVE_BEFORE_PUBLISH_MS")
        && let Ok(milliseconds) = value.parse::<u64>()
    {
        thread::sleep(Duration::from_millis(milliseconds.min(5_000)));
    }
}

fn test_fail_after_commit() -> bool {
    cfg!(debug_assertions) && env::var("LWC_TEST_ARCHIVE_FAIL_AFTER_COMMIT").as_deref() == Ok("1")
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    fs::File::open(
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}

struct FileCleanup {
    path: PathBuf,
    active: bool,
}

impl FileCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for FileCleanup {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
