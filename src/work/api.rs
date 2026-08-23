#[cfg(test)]
#[path = "sync_audit_tests.rs"]
mod sync_audit_tests;

const TERMINAL_SYNC_AUDIT_MAX_ITEMS: usize = 4_096;

pub(crate) fn terminal_sync_audits(
    database: &Path,
    origin_store_id: &str,
) -> Result<Vec<TerminalSyncAudit>> {
    const MAX_STATE_BYTES: u64 = 64 * 1024;
    if origin_store_id.len() != 64
        || !origin_store_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::new(
            "sync_audit_invalid",
            "origin store ID must be 64 hexadecimal characters",
        ));
    }
    let root = work_root(database)?;
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AppError::new(
                "work_invalid",
                "work root is not a real directory",
            ));
        }
        Ok(_) => {}
        Err(error) => return Err(error.into()),
    }
    let mut audits = Vec::new();
    for entry in fs::read_dir(&root)? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if validate_id(&id).is_err() {
            continue;
        }
        let path = entry.path().join("state.json");
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_STATE_BYTES
        {
            continue;
        }
        let Ok(state) = read_json::<WorkState>(&path) else {
            continue;
        };
        if state.id != id
            || !terminal(&state.state)
            || !matches!(
                state.kind.as_str(),
                "schema-migrate"
                    | "maintenance-compact"
                    | "maintenance-reindex"
                    | "maintenance-materialize"
                    | "graph-project"
            )
        {
            continue;
        }
        let result_digest = state
            .result
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| AppError::new("sync_audit_invalid", error.to_string()))?
            .map(|bytes| hex_digest(&bytes));
        let error_code = match state.error.as_ref() {
            None => None,
            Some(error) => {
                let Some(code) = error.get("code").and_then(Value::as_str) else {
                    continue;
                };
                if code.is_empty()
                    || code.len() > 64
                    || !code.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-' | b'.')
                    })
                {
                    continue;
                }
                Some(code.to_string())
            }
        };
        let canonical = json!({
            "kind": state.kind,
            "state": state.state,
            "completed": state.completed,
            "total": state.total,
            "updated_at_unix_ms": state.updated_at_unix_ms,
            "result_digest": result_digest,
            "error_code": error_code,
        });
        let canonical = serde_json::to_vec(&canonical)
            .map_err(|error| AppError::new("sync_audit_invalid", error.to_string()))?;
        let digest = hex_digest(&canonical);
        let audit_key = hex_digest(format!("{origin_store_id}\0{id}").as_bytes());
        audits.push(TerminalSyncAudit {
            audit_key,
            digest,
            origin_store_id: origin_store_id.to_string(),
            origin_work_id: id,
            kind: state.kind,
            state: state.state,
            completed: state.completed,
            total: state.total,
            updated_at_unix_ms: state.updated_at_unix_ms,
            result_digest,
            error_code,
        });
    }
    audits.sort_by(|left, right| left.audit_key.cmp(&right.audit_key));
    if audits.len() > TERMINAL_SYNC_AUDIT_MAX_ITEMS {
        return Err(AppError::new(
            "sync_audit_limit",
            "terminal Work audit count exceeds the fixed Sync limit",
        ));
    }
    Ok(audits)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
    let root = work_root(&store.path)?;
    loop {
        let state = load_state(store, id)?;
        if terminal(&state.state) {
            release_active(&root, id)?;
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
