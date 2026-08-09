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
