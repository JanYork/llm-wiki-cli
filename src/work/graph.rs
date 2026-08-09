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
