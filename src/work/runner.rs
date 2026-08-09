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
