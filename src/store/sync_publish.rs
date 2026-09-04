#[derive(Debug, Clone, Serialize)]
pub(crate) struct SyncPublishSummary {
    pub(crate) committed: bool,
    pub(crate) revision: String,
    pub(crate) checkpoint: PathBuf,
    pub(crate) derived: Value,
    pub(crate) derived_selection: String,
    pub(crate) affected_counts: BTreeMap<String, usize>,
    pub(crate) affected_digest: String,
    pub(crate) affected: Option<Value>,
    pub(crate) affected_pages: Vec<String>,
    pub(crate) affected_sources: Vec<String>,
    pub(crate) affected_memory: Vec<String>,
    pub(crate) affected_todos: Vec<String>,
    pub(crate) affected_plans: Vec<String>,
    pub(crate) affected_meta: Vec<String>,
    pub(crate) affected_tags: Vec<String>,
    pub(crate) affected_relations: Vec<String>,
    pub(crate) affected_graph_documents: Vec<(String, String)>,
}

#[derive(Default)]
struct SyncAffected {
    by_kind: BTreeMap<String, BTreeSet<String>>,
}

impl SyncAffected {
    fn keys(&self, kind: &str) -> impl Iterator<Item = &str> {
        self.by_kind
            .get(kind)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }

    fn owned(&self, kind: &str) -> Vec<String> {
        self.keys(kind).map(str::to_owned).collect()
    }
}

struct PreparedSyncState {
    digest: String,
    objects: BTreeMap<(String, String), Value>,
    normalized: PathBuf,
    blob_count: usize,
    #[cfg(test)]
    buffered_blob_bytes: usize,
}

const SYNC_SOURCE_INDEX_MAX_BYTES: i64 = 64 * 1024 * 1024;
const SYNC_DERIVED_SELECTION_MAX_ITEMS: usize = 4_096;
const SYNC_DERIVED_SELECTION_MAX_BYTES: usize = 256 * 1024;

struct BoundedSyncSelection {
    mode: &'static str,
    counts: BTreeMap<String, usize>,
    digest: String,
    affected: Option<Value>,
    graph_documents: Vec<(String, String)>,
}

fn bounded_sync_selection(
    affected: &SyncAffected,
    graph_documents: &[(String, String)],
) -> Result<BoundedSyncSelection> {
    let mut by_kind = affected.by_kind.clone();
    for kind in [
        "meta",
        "source",
        "page",
        "tag",
        "ingest",
        "retrieval_weight",
        "retrieval_feedback",
        "semantic_relation",
        "memory",
        "todo",
        "plan",
        "work_audit",
        "draft_intent",
    ] {
        by_kind.entry(kind.to_owned()).or_default();
    }
    let counts = by_kind
        .iter()
        .map(|(kind, keys)| (kind.clone(), keys.len()))
        .collect::<BTreeMap<_, _>>();
    let affected_value = serde_json::to_value(&by_kind)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    let canonical = json!({
        "affected": affected_value,
        "affected_graph_documents": graph_documents,
    });
    let encoded = serde_json::to_string(&canonical)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    let item_count = counts.values().sum::<usize>() + graph_documents.len();
    let exact = item_count <= SYNC_DERIVED_SELECTION_MAX_ITEMS
        && encoded.len() <= SYNC_DERIVED_SELECTION_MAX_BYTES;
    Ok(BoundedSyncSelection {
        mode: if exact { "exact" } else { "full" },
        counts,
        digest: hash_content(&encoded),
        affected: exact.then_some(affected_value),
        graph_documents: if exact {
            graph_documents.to_vec()
        } else {
            Vec::new()
        },
    })
}

pub(crate) fn sync_publication_receipt(
    database: &Path,
    session_id: &str,
    state_digest: &str,
) -> Result<Option<Value>> {
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let Some((operation_id, value, ending)) =
        read_sync_publication_receipt(&conn, session_id, state_digest)?
    else {
        return Ok(None);
    };
    if operation_id != ending.operation_id || store_identity(&conn)? != ending {
        return Err(sync_store_changed());
    }
    Ok(Some(value))
}

pub(crate) fn archive_publication_receipt(
    database: &Path,
    session_id: &str,
    state_digest: &str,
) -> Result<Option<Value>> {
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let Some((operation_id, value, ending)) =
        read_sync_publication_receipt(&conn, session_id, state_digest)?
    else {
        return Ok(None);
    };
    let current = store_identity(&conn)?;
    if operation_id != ending.operation_id
        || current.store_id != ending.store_id
        || current.operation_id < ending.operation_id
        || (current.operation_id == ending.operation_id && current.revision != ending.revision)
    {
        return Err(sync_store_changed());
    }
    Ok(Some(value))
}

fn read_sync_publication_receipt(
    conn: &Connection,
    session_id: &str,
    state_digest: &str,
) -> Result<Option<(i64, Value, StoreIdentity)>> {
    let receipt = conn
        .query_row(
            "SELECT id,detail_json FROM operations
             WHERE action='sync_merge' AND target=?1 ORDER BY id DESC LIMIT 1",
            [session_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((operation_id, detail)) = receipt else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&detail)
        .map_err(|error| AppError::new("sync_receipt_invalid", error.to_string()))?;
    if value["state_digest"] != state_digest {
        return Ok(None);
    }
    let ending: StoreIdentity = serde_json::from_value(value["ending_identity"].clone())
        .map_err(|error| AppError::new("sync_receipt_invalid", error.to_string()))?;
    Ok(Some((operation_id, value, ending)))
}

impl Store {
    pub(crate) fn persist_sync_derived_receipt(
        &self,
        session_id: &str,
        state_digest: &str,
        derived: &Value,
    ) -> Result<()> {
        persist_sync_derived_receipt(&self.conn, session_id, state_digest, derived)
    }

    pub(crate) fn publish_sync_state_to_missing(
        scope: impl Into<String>,
        database: &Path,
        normalized: &Path,
        session_id: &str,
    ) -> Result<SyncPublishSummary> {
        let state = prepare_sync_state(normalized)?;
        if let Ok(metadata) = fs::symlink_metadata(database) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(unsafe_sync_target(database));
            }
            return Err(AppError::new(
                "sync_target_exists",
                format!("Sync target already exists: {}", database.display()),
            ));
        }
        let parent = database.parent().ok_or_else(|| {
            AppError::new("invalid_store_path", "Sync target database has no parent")
        })?;
        ensure_initial_sync_directory(parent)?;
        let sidecars = [
            database.with_extension("db-wal"),
            database.with_extension("db-shm"),
        ];
        for sidecar in &sidecars {
            ensure_initial_sync_path_absent(sidecar)?;
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let database_file = options.open(database)?;
        let mut created = vec![CreatedSyncFile::new(database, &database_file.metadata()?)];
        drop(database_file);

        let scope = scope.into();
        let initialized = Store::initialize(scope, database);
        let (mut store, _) = match initialized {
            Ok(value) => value,
            Err(error) => {
                created.extend(
                    sidecars
                        .iter()
                        .filter_map(|path| CreatedSyncFile::capture(path)),
                );
                remove_created_sync_files(created);
                return Err(error);
            }
        };
        let mut expected_identity = None;
        let mut checkpoint_candidate = None;
        let outcome = (|| -> Result<SyncPublishSummary> {
            let expected = store.identity()?;
            expected_identity = Some(expected.clone());
            let checkpoint = checkpoint_path(
                database,
                &format!(
                    "sync-{}-{}-{}",
                    &hash_content(session_id)[..8],
                    &expected.revision[..8],
                    &state.digest[..8]
                ),
            )?;
            ensure_initial_sync_path_absent(&checkpoint)?;
            checkpoint_candidate = Some(checkpoint);
            store.publish_sync_state(normalized, &expected, session_id)
        })();
        match outcome {
            Ok(summary) => Ok(summary),
            Err(error) => {
                let unchanged = expected_identity.as_ref().is_none_or(|expected| {
                    store.identity().is_ok_and(|identity| identity == *expected)
                });
                drop(store);
                if unchanged {
                    created.extend(
                        sidecars
                            .iter()
                            .chain(checkpoint_candidate.iter())
                            .filter_map(|path| CreatedSyncFile::capture(path)),
                    );
                    remove_created_sync_files(created);
                }
                Err(error)
            }
        }
    }

    pub(crate) fn publish_sync_state(
        &mut self,
        normalized: &Path,
        expected: &StoreIdentity,
        session_id: &str,
    ) -> Result<SyncPublishSummary> {
        let state = prepare_sync_state(normalized)?;
        if self.identity()? != *expected {
            return Err(sync_store_changed());
        }

        let checkpoint = checkpoint_path(
            &self.database,
            &format!(
                "sync-{}-{}-{}",
                &hash_content(session_id)[..8],
                &expected.revision[..8],
                &state.digest[..8]
            ),
        )?;
        match fs::symlink_metadata(&checkpoint) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(AppError::new(
                        "checkpoint_path_invalid",
                        "the Sync checkpoint target is not a regular non-symlink file",
                    ));
                }
                let saved = Store::open_for_read(self.scope.clone(), &checkpoint)?.identity()?;
                if saved != *expected {
                    return Err(AppError::new(
                        "checkpoint_exists",
                        "the existing Sync recovery checkpoint does not match the expected store",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_checkpoint(&self.conn, &checkpoint)?;
            }
            Err(error) => return Err(error.into()),
        }

        let previous_normalized = checkpoint.with_extension(format!(
            "sync-before-{}-{}.db",
            std::process::id(),
            SOURCE_STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _previous_normalized = TemporaryDatabase(previous_normalized.clone());
        Store::open_for_read(self.scope.clone(), &checkpoint)?
            .export_sync_object_inventory(&previous_normalized)?;
        let previous = load_sync_objects(&previous_normalized)?
            .into_iter()
            .map(|(key, row)| (key, row.payload))
            .collect::<BTreeMap<_, _>>();
        let affected = derived_sync_affected(&previous, &state.objects);

        let normalized_database = state.normalized.to_string_lossy().into_owned();
        self.conn.execute(
            "ATTACH DATABASE ?1 AS sync_normalized",
            [normalized_database],
        )?;
        let mut old_source_ids = BTreeMap::new();
        let mut old_relation_documents = BTreeSet::new();
        let mut affected_graph_documents = Vec::new();
        let mut bounded_selection = None;
        let result = (|| -> Result<String> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if store_identity(&tx)? != *expected {
                return Err(sync_store_changed());
            }
            old_source_ids = source_ids_for_hashes(&tx, affected.keys("source"))?;
            old_relation_documents = relation_documents_for_ids(
                &tx,
                affected.keys("semantic_relation"),
            )?;
            apply_prepared_sync_state(&tx, &state)?;
            affected_graph_documents = sync_graph_documents(
                &tx,
                &affected,
                &old_source_ids,
                &old_relation_documents,
            )?;
            bounded_selection = Some(bounded_sync_selection(
                &affected,
                &affected_graph_documents,
            )?);
            let selection = bounded_selection.as_ref().expect("selection was prepared");
            validate_database_integrity(&tx)
                .map_err(|error| AppError::new("sync_state_invalid", error.message))?;
            let mut detail = json!({
                "state_digest": state.digest,
                "blob_count": state.blob_count,
                "checkpoint": checkpoint.file_name().map(|name| name.to_string_lossy()),
                "starting_identity": expected,
                "derived_selection": selection.mode,
                "affected_counts": selection.counts,
                "affected_digest": selection.digest,
                "affected": selection.affected,
                "affected_graph_documents": selection.graph_documents,
            });
            let revision = record_operation(
                &tx,
                "sync_merge",
                session_id,
                &detail,
            )?;
            let operation_id = tx.last_insert_rowid();
            detail["ending_identity"] = json!({
                "store_id": expected.store_id,
                "revision": revision,
                "operation_id": operation_id,
            });
            let detail_json = serde_json::to_string(&detail)
                .map_err(|error| AppError::new("json_error", error.to_string()))?;
            tx.execute(
                "UPDATE operations SET detail_json=?1 WHERE id=?2",
                params![detail_json, operation_id],
            )?;
            validate_database_integrity(&tx)
                .map_err(|error| AppError::new("sync_state_invalid", error.message))?;
            tx.commit()?;
            Ok(revision)
        })();
        let detach = self.conn.execute_batch("DETACH DATABASE sync_normalized");
        let revision = settle_sync_publication(result, detach)?;
        let derived = match (|| -> Result<()> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            rebuild_sync_indexes(&tx, &state, &affected, &old_source_ids)?;
            validate_database_integrity(&tx)
                .map_err(|error| AppError::new("sync_derived_invalid", error.message))?;
            tx.commit()?;
            Ok(())
        })() {
            Ok(()) => json!({
                "status": "completed",
                "indexed_pages": affected.owned("page").len(),
                "indexed_sources": affected.owned("source").len(),
            }),
            Err(error) => json!({
                "status": "failed",
                "error": error.code,
                "committed": true,
                "limit_bytes": SYNC_SOURCE_INDEX_MAX_BYTES,
                "next_action": "upgrade LWC or split the source before rebuilding derived indexes",
            }),
        };
        // The canonical merge is already committed. Persist the post-commit
        // FTS outcome in its recovery receipt so a lost transport response
        // never turns an unknown or failed derived state into guessed success.
        let _ = persist_sync_derived_receipt(&self.conn, session_id, &state.digest, &derived);
        let selection = bounded_selection.expect("committed publication has a selection");
        let exact = selection.mode == "exact";
        let selected = |kind| {
            if exact {
                affected.owned(kind)
            } else {
                Vec::new()
            }
        };
        Ok(SyncPublishSummary {
            committed: true,
            revision,
            checkpoint,
            derived,
            derived_selection: selection.mode.to_owned(),
            affected_counts: selection.counts,
            affected_digest: selection.digest,
            affected: selection.affected,
            affected_pages: selected("page"),
            affected_sources: selected("source"),
            affected_memory: selected("memory"),
            affected_todos: selected("todo"),
            affected_plans: selected("plan"),
            affected_meta: selected("meta"),
            affected_tags: selected("tag"),
            affected_relations: selected("semantic_relation"),
            affected_graph_documents: selection.graph_documents,
        })
    }
}

fn persist_sync_derived_receipt(
    conn: &Connection,
    session_id: &str,
    state_digest: &str,
    derived: &Value,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let (operation_id, encoded) = tx.query_row(
        "SELECT id,detail_json FROM operations
         WHERE action='sync_merge' AND target=?1 ORDER BY id DESC LIMIT 1",
        [session_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut detail: Value = serde_json::from_str(&encoded)
        .map_err(|error| AppError::new("sync_receipt_invalid", error.to_string()))?;
    if detail["state_digest"] != state_digest {
        return Err(AppError::new(
            "sync_receipt_invalid",
            "post-commit derived receipt does not match the published state",
        ));
    }
    detail["derived"] = derived.clone();
    tx.execute(
        "UPDATE operations SET detail_json=?1 WHERE id=?2",
        rusqlite::params![
            serde_json::to_string(&detail)
                .map_err(|error| AppError::new("sync_receipt_invalid", error.to_string()))?,
            operation_id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

fn changed_sync_objects(
    previous: &BTreeMap<(String, String), Value>,
    current: &BTreeMap<(String, String), Value>,
) -> SyncAffected {
    let mut keys = previous
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut affected = SyncAffected::default();
    for (kind, key) in std::mem::take(&mut keys) {
        if previous.get(&(kind.clone(), key.clone()))
            != current.get(&(kind.clone(), key.clone()))
        {
            affected.by_kind.entry(kind).or_default().insert(key);
        }
    }
    affected
}

fn derived_sync_affected(
    previous: &BTreeMap<(String, String), Value>,
    current: &BTreeMap<(String, String), Value>,
) -> SyncAffected {
    let mut affected = changed_sync_objects(previous, current);
    // Continuity has its own bounded replay/audit receipt and never changes
    // Markdown, FTS, or graph projections.
    affected.by_kind.remove("draft_intent");
    affected.by_kind.remove("work_audit");
    affected
}

fn source_ids_for_hashes<'a>(
    tx: &Connection,
    hashes: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<String, Vec<i64>>> {
    let mut result = BTreeMap::new();
    let mut statement = tx.prepare("SELECT id FROM sources WHERE content_hash=?1 ORDER BY id")?;
    for hash in hashes {
        let ids = statement
            .query_map([hash], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.insert(hash.to_owned(), ids);
    }
    Ok(result)
}

fn relation_documents_for_ids<'a>(
    tx: &Connection,
    ids: impl Iterator<Item = &'a str>,
) -> Result<BTreeSet<(String, String)>> {
    let mut documents = BTreeSet::new();
    for id in ids {
        let from = tx
            .query_row(
                "SELECT from_identifier FROM semantic_relations WHERE id=?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(from) = from {
            documents.extend(documents_for_relation_endpoint(tx, &from)?);
        }
    }
    Ok(documents)
}

fn documents_for_relation_endpoint(
    tx: &Connection,
    endpoint: &str,
) -> Result<BTreeSet<(String, String)>> {
    if let Some(slug) = endpoint.strip_prefix("page:") {
        return Ok(BTreeSet::from([("page".to_owned(), slug.to_owned())]));
    }
    if let Some(id) = endpoint.strip_prefix("source:") {
        return Ok(BTreeSet::from([("source".to_owned(), id.to_owned())]));
    }
    let document = tx
        .query_row(
            "SELECT document_type,document_identifier FROM search_spans WHERE span_id=?1",
            [endpoint],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(document.into_iter().collect())
}

fn sync_graph_documents(
    tx: &Connection,
    affected: &SyncAffected,
    old_source_ids: &BTreeMap<String, Vec<i64>>,
    old_relation_documents: &BTreeSet<(String, String)>,
) -> Result<Vec<(String, String)>> {
    let mut documents = old_relation_documents.clone();
    documents.extend(
        affected
            .keys("page")
            .map(|slug| ("page".to_owned(), slug.to_owned())),
    );
    for hash in affected.keys("source") {
        documents.extend(
            old_source_ids
                .get(hash)
                .into_iter()
                .flatten()
                .map(|id| ("source".to_owned(), id.to_string())),
        );
        let ids = source_ids_for_hashes(tx, std::iter::once(hash))?;
        documents.extend(
            ids.get(hash)
                .into_iter()
                .flatten()
                .map(|id| ("source".to_owned(), id.to_string())),
        );
    }
    documents.extend(relation_documents_for_ids(
        tx,
        affected.keys("semantic_relation"),
    )?);
    Ok(documents.into_iter().collect())
}

fn settle_sync_publication(
    publication: Result<String>,
    detach: rusqlite::Result<()>,
) -> Result<String> {
    match publication {
        Ok(revision) => {
            // Canonical commit is the irreversible boundary. A best-effort
            // post-commit detach must never turn that success into an ambiguous
            // retryable error.
            let _ = detach;
            Ok(revision)
        }
        Err(error) => Err(error),
    }
}

struct CreatedSyncFile {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl CreatedSyncFile {
    #[cfg_attr(not(unix), allow(unused_variables))]
    fn new(path: &Path, metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            path: path.to_path_buf(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }

    fn capture(path: &Path) -> Option<Self> {
        let metadata = fs::symlink_metadata(path).ok()?;
        (!metadata.file_type().is_symlink() && metadata.is_file())
            .then(|| Self::new(path, &metadata))
    }

    fn still_same(&self) -> bool {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return false;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            metadata.dev() == self.device && metadata.ino() == self.inode
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

fn remove_created_sync_files(files: Vec<CreatedSyncFile>) {
    for file in files {
        if file.still_same() {
            let _ = fs::remove_file(file.path);
        }
    }
}

fn ensure_initial_sync_directory(directory: &Path) -> Result<()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(unsafe_sync_target(directory));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = directory
                .parent()
                .ok_or_else(|| unsafe_sync_target(directory))?;
            ensure_initial_sync_directory(parent)?;
            fs::create_dir(directory)?;
            let metadata = fs::symlink_metadata(directory)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(unsafe_sync_target(directory));
            }
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_initial_sync_path_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(unsafe_sync_target(path)),
        Err(error) => Err(error.into()),
    }
}

fn unsafe_sync_target(path: &Path) -> AppError {
    AppError::new(
        "sync_target_unsafe",
        format!(
            "initial Sync target is not an absent regular path: {}",
            path.display()
        ),
    )
}

fn sync_store_changed() -> AppError {
    AppError::new(
        "sync_store_changed",
        "the live store changed after Sync captured its expected revision",
    )
}

fn prepare_sync_state(path: &Path) -> Result<PreparedSyncState> {
    let digest = sync_state_digest(path)
        .map_err(|error| AppError::new("sync_state_invalid", error.message))?;
    let rows = load_sync_objects(path)
        .map_err(|error| AppError::new("sync_state_invalid", error.message))?;
    let allowed = [
        "meta",
        "source",
        "page",
        "tag",
        "ingest",
        "retrieval_weight",
        "retrieval_feedback",
        "semantic_relation",
        "memory",
        "todo",
        "plan",
        "work_audit",
        "draft_intent",
    ];
    let mut objects = BTreeMap::new();
    for ((kind, key), row) in rows {
        if !allowed.contains(&kind.as_str()) || !row.payload.is_object() {
            return Err(AppError::new(
                "sync_state_invalid",
                format!("unsupported or malformed normalized object: {kind}:{key}"),
            ));
        }
        objects.insert((kind, key), row.payload);
    }
    let draft_count = objects.keys().filter(|(kind, _)| kind == "draft_intent").count();
    let audit_count = objects.keys().filter(|(kind, _)| kind == "work_audit").count();
    if draft_count > 64 || audit_count > 4_096 {
        return Err(AppError::new(
            "sync_state_invalid",
            "normalized continuity objects exceed their fixed limits",
        ));
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    let mut statement = conn
        .prepare("SELECT content_hash FROM sync_blobs ORDER BY content_hash")
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    let mut blob_count = 0_usize;
    for row in statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
    {
        row.map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        blob_count += 1;
    }
    for (kind, key) in objects.keys() {
        let exists = kind != "source"
            || conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_blobs WHERE content_hash=?1)",
                [key],
                |row| row.get::<_, bool>(0),
            )?;
        if !exists {
            return Err(AppError::new(
                "sync_state_invalid",
                format!("Sync source {key} has no content blob"),
            ));
        }
    }
    for ((kind, key), payload) in &objects {
        if kind == "draft_intent" {
            for hash in required_sync_draft_blobs(key, payload)? {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_blobs WHERE content_hash=?1)",
                    [&hash],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(AppError::new(
                        "sync_state_invalid",
                        format!("normalized draft_intent '{key}' required blob is missing"),
                    ));
                }
            }
        } else if kind == "work_audit" {
            validated_sync_work_audit(key, payload)?;
        }
    }
    Ok(PreparedSyncState {
        digest,
        objects,
        normalized: path.to_path_buf(),
        blob_count,
        #[cfg(test)]
        buffered_blob_bytes: 0,
    })
}

fn apply_prepared_sync_state(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    let todo_revisions = local_object_revisions(tx, "todo_items")?;
    let plan_revisions = local_object_revisions(tx, "plans")?;
    let memory_requests = local_object_request_ids(tx, "memory_events")?;
    let todo_requests = local_object_request_ids(tx, "todo_items")?;
    let plan_requests = local_object_request_ids(tx, "plans")?;
    clear_synced_semantic_state(tx)?;
    import_sync_meta(tx, state)?;
    let source_ids = import_sync_sources(tx, state)?;
    import_sync_pages(tx, state, &source_ids)?;
    import_sync_tags(tx, state)?;
    import_sync_ingest(tx, state, &source_ids)?;
    import_sync_retrieval(tx, state, &source_ids)?;
    import_sync_relations(tx, state, &source_ids)?;
    import_sync_memory(tx, state, &memory_requests)?;
    import_sync_todos(tx, state, &todo_revisions, &todo_requests)?;
    import_sync_plans(tx, state, &plan_revisions, &plan_requests)?;
    tx.execute_batch(
        "DELETE FROM agent_todo_tracks
         WHERE NOT EXISTS(SELECT 1 FROM todo_items WHERE id=agent_todo_tracks.todo_id);
         DELETE FROM agent_plan_tracks
         WHERE NOT EXISTS(SELECT 1 FROM plans WHERE id=agent_plan_tracks.plan_id);",
    )?;
    import_sync_work_audits(tx, state)?;
    validate_sync_draft_intents(state)?;
    validate_sync_domain_invariants(tx)?;
    Ok(())
}

fn import_sync_work_audits(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    for (audit_key, payload) in objects_of_kind(state, "work_audit") {
        validated_sync_work_audit(audit_key, payload)?;
        let existing = tx
            .query_row(
                "SELECT detail_json FROM operations
                 WHERE action='sync_work_audit' AND target=?1 ORDER BY id",
                [audit_key],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            let existing: Value = serde_json::from_str(&existing)
                .map_err(|error| AppError::new("sync_audit_invalid", error.to_string()))?;
            if existing != *payload {
                return Err(AppError::new(
                    "sync_audit_conflict",
                    format!("terminal Work audit differs for {audit_key}"),
                ));
            }
            continue;
        }
        record_operation(tx, "sync_work_audit", audit_key, payload)?;
    }
    Ok(())
}

fn validated_sync_work_audit(
    audit_key: &str,
    payload: &Value,
) -> Result<crate::work::TerminalSyncAudit> {
    ensure_payload_key(payload, "audit_key", audit_key, "work_audit")?;
    let audit: crate::work::TerminalSyncAudit = serde_json::from_value(payload.clone())
        .map_err(|error| invalid_object("work_audit", audit_key, &error.to_string()))?;
    let canonical = serde_json::to_value(&audit)
        .map_err(|error| invalid_object("work_audit", audit_key, &error.to_string()))?;
    let canonical_digest = json!({
        "kind": audit.kind,
        "state": audit.state,
        "completed": audit.completed,
        "total": audit.total,
        "updated_at_unix_ms": audit.updated_at_unix_ms,
        "result_digest": audit.result_digest,
        "error_code": audit.error_code,
    });
    let canonical_digest = serde_json::to_string(&canonical_digest)
        .map_err(|error| invalid_object("work_audit", audit_key, &error.to_string()))?;
    let expected_key = hash_content(&format!(
        "{}\0{}",
        audit.origin_store_id, audit.origin_work_id
    ));
    let result_digest_valid = audit
        .result_digest
        .as_deref()
        .is_none_or(|digest| is_lower_sync_hex(digest, 64));
    let error_code_valid = audit.error_code.as_deref().is_none_or(|code| {
        !code.is_empty()
            && code.len() <= 64
            && code.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'-' | b'.')
            })
    });
    if canonical != *payload
        || !is_lower_sync_hex(&audit.digest, 64)
        || !is_lower_sync_hex(&audit.origin_store_id, 64)
        || audit.origin_work_id.is_empty()
        || audit.origin_work_id.len() > 128
        || audit.origin_work_id.chars().any(char::is_control)
        || !matches!(
            audit.kind.as_str(),
            "schema-migrate"
                | "maintenance-compact"
                | "maintenance-reindex"
                | "maintenance-materialize"
                | "graph-project"
        )
        || !matches!(audit.state.as_str(), "succeeded" | "failed" | "cancelled")
        || audit.total.is_some_and(|total| audit.completed > total)
        || !result_digest_valid
        || !error_code_valid
        || audit.audit_key != expected_key
        || audit.digest != hash_content(&canonical_digest)
    {
        return Err(invalid_object(
            "work_audit",
            audit_key,
            "is non-canonical or has invalid origin labels",
        ));
    }
    Ok(audit)
}

fn validate_sync_draft_intents(state: &PreparedSyncState) -> Result<()> {
    for (logical_key, payload) in objects_of_kind(state, "draft_intent") {
        required_sync_draft_blobs(logical_key, payload)?;
    }
    Ok(())
}

fn local_object_request_ids(tx: &Transaction<'_>, table: &str) -> Result<BTreeMap<String, String>> {
    let sql = match table {
        "memory_events" => "SELECT id,request_id FROM memory_events WHERE request_id IS NOT NULL",
        "todo_items" => "SELECT id,request_id FROM todo_items WHERE request_id IS NOT NULL",
        "plans" => "SELECT id,request_id FROM plans WHERE request_id IS NOT NULL",
        _ => {
            return Err(AppError::new(
                "sync_state_invalid",
                "unsupported request-id table",
            ));
        }
    };
    let mut statement = tx.prepare(sql)?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

fn validate_sync_domain_invariants(tx: &Transaction<'_>) -> Result<()> {
    let mut statement = tx.prepare("SELECT id,parent_id FROM todo_items")?;
    let parents = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    for id in parents.keys() {
        let mut seen = BTreeSet::new();
        let mut current = Some(id.as_str());
        while let Some(todo_id) = current {
            if !seen.insert(todo_id) {
                return Err(AppError::new(
                    "sync_state_invalid",
                    format!("normalized Todo parent cycle includes '{todo_id}'"),
                ));
            }
            current = parents.get(todo_id).and_then(Option::as_deref);
        }
    }

    let invalid_plan: Option<String> = tx
        .query_row(
            "SELECT p.id FROM plans p
             WHERE (p.state='active' AND
                    (SELECT COUNT(*) FROM plan_steps s
                     WHERE s.plan_id=p.id AND s.status IN ('in_progress','blocked')) <> 1)
                OR (p.state<>'active' AND
                    EXISTS(SELECT 1 FROM plan_steps s
                           WHERE s.plan_id=p.id AND s.status IN ('in_progress','blocked')))
             ORDER BY p.id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = invalid_plan {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("normalized Plan '{id}' has an invalid focal step set"),
        ));
    }
    Ok(())
}

fn local_object_revisions(tx: &Transaction<'_>, table: &str) -> Result<BTreeMap<String, i64>> {
    let sql = match table {
        "todo_items" => "SELECT id,revision FROM todo_items",
        "plans" => "SELECT id,revision FROM plans",
        _ => {
            return Err(AppError::new(
                "sync_state_invalid",
                "unsupported revision table",
            ));
        }
    };
    let mut statement = tx.prepare(sql)?;
    Ok(statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

fn clear_synced_semantic_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM retrieval_feedback;
         DELETE FROM retrieval_weights;
         DELETE FROM semantic_relations;
         DELETE FROM page_tags;
         DELETE FROM tags;
         DELETE FROM links;
         DELETE FROM page_provenance;
         DELETE FROM page_sources;
         DELETE FROM pages;
         DELETE FROM ingest_jobs;
         DELETE FROM memory_feedback;
         DELETE FROM memory_relations;
         DELETE FROM memory_changes;
         DELETE FROM memory_evidence;
         DELETE FROM memory_fragments;
         DELETE FROM memory_events;
         DELETE FROM todo_tags;
         UPDATE todo_items SET parent_id=NULL;
         DELETE FROM todo_items;
         DELETE FROM plan_history;
         DELETE FROM plan_steps;
         DELETE FROM plan_constraints;
         DELETE FROM plan_tags;
         DELETE FROM plans;",
    )?;
    Ok(())
}

fn import_sync_meta(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    for key in ["schema", "purpose"] {
        if let Some(value) = object(state, "meta", key) {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, required_str(value, "value")?],
            )?;
        }
    }
    Ok(())
}

fn import_sync_sources(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
) -> Result<BTreeMap<String, i64>> {
    let desired = objects_of_kind(state, "source")
        .map(|(key, _)| key.to_owned())
        .collect::<BTreeSet<_>>();
    let stale = {
        let mut statement = tx.prepare("SELECT id,content_hash FROM sources")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, hash) in stale {
        if !desired.contains(&hash) {
            tx.execute(
                "DELETE FROM source_path_revisions
                 WHERE source_id=?1 OR tracked_path IN (
                     SELECT head.tracked_path
                     FROM source_path_revisions head
                     WHERE head.source_id=?1 AND head.revision=(
                         SELECT MAX(latest.revision)
                         FROM source_path_revisions latest
                         WHERE latest.tracked_path=head.tracked_path
                     )
                 )",
                [id],
            )?;
            tx.execute("DELETE FROM sources WHERE id=?1", [id])?;
        }
    }

    let mut ids = BTreeMap::new();
    for (hash, payload) in objects_of_kind(state, "source") {
        if optional_str(payload, "content_hash")?.is_some_and(|value| value != hash) {
            return Err(invalid_object(
                "source",
                hash,
                "content_hash differs from its key",
            ));
        }
        let existing = tx
            .query_row(
                "SELECT id,origin FROM sources WHERE content_hash=?1",
                [hash],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let id = if let Some((id, origin)) = existing {
            tx.execute(
                "UPDATE sources SET title=?1,origin=?2,
                        content=(SELECT CAST(content AS TEXT) FROM sync_normalized.sync_blobs
                                 WHERE content_hash=?3),
                        structural_navigation=?4,created_at=?5 WHERE id=?6",
                params![
                    optional_str(payload, "title")?,
                    origin,
                    hash,
                    bool_i64(payload, "structural_navigation")?,
                    required_str(payload, "created_at")?,
                    id,
                ],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO sources(content_hash,title,origin,content,structural_navigation,created_at)
                 SELECT ?1,?2,?3,CAST(content AS TEXT),?4,?5
                 FROM sync_normalized.sync_blobs WHERE content_hash=?1",
                params![
                    hash,
                    optional_str(payload, "title")?,
                    optional_str(payload, "origin")?
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("sync:{hash}")),
                    bool_i64(payload, "structural_navigation")?,
                    required_str(payload, "created_at")?,
                ],
            )?;
            if tx.changes() != 1 {
                return Err(invalid_object("source", hash, "content blob is missing"));
            }
            tx.last_insert_rowid()
        };
        ids.insert(hash.to_owned(), id);
    }
    Ok(ids)
}

fn import_sync_pages(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    source_ids: &BTreeMap<String, i64>,
) -> Result<()> {
    for (slug, payload) in objects_of_kind(state, "page") {
        ensure_payload_key(payload, "slug", slug, "page")?;
        let body = required_str(payload, "body")?;
        tx.execute(
            "INSERT INTO pages(slug,title,kind,summary,body,structural_navigation,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                slug,
                required_str(payload, "title")?,
                optional_str(payload, "kind")?,
                optional_str(payload, "summary")?,
                body,
                payload
                    .get("structural_navigation")
                    .and_then(Value::as_bool)
                    .map(i64::from)
                    .unwrap_or(0),
                required_str(payload, "created_at")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
        for hash in string_array(payload, "source_hashes")? {
            let id = source_ids
                .get(hash)
                .ok_or_else(|| invalid_object("page", slug, "cited source is missing"))?;
            tx.execute(
                "INSERT INTO page_sources(page_slug,source_id) VALUES(?1,?2)",
                params![slug, id],
            )?;
        }
        for provenance in string_array(payload, "provenance")? {
            if provenance == SOURCE_GROUNDED {
                continue;
            }
            tx.execute(
                "INSERT INTO page_provenance(page_slug,provenance) VALUES(?1,?2)",
                params![slug, provenance],
            )?;
        }
        for target in extract_links(body) {
            tx.execute(
                "INSERT OR IGNORE INTO links(from_slug,to_slug) VALUES(?1,?2)",
                params![slug, target],
            )?;
        }
    }
    Ok(())
}

fn import_sync_tags(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    for (name, payload) in objects_of_kind(state, "tag") {
        ensure_payload_key(payload, "name", name, "tag")?;
        tx.execute(
            "INSERT INTO tags(name,autoload,autoload_priority,autoload_limit,autoload_max_chars,reason,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                name,
                bool_i64(payload, "autoload")?,
                required_i64(payload, "autoload_priority")?,
                required_i64(payload, "autoload_limit")?,
                required_i64(payload, "autoload_max_chars")?,
                required_str(payload, "reason")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
        for page in required_array(payload, "pages")? {
            tx.execute(
                "INSERT INTO page_tags(tag_name,page_slug,priority,reason,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    name,
                    required_str(page, "slug")?,
                    required_i64(page, "priority")?,
                    required_str(page, "reason")?,
                    required_str(page, "created_at")?,
                    required_str(page, "updated_at")?,
                ],
            )?;
        }
    }
    Ok(())
}

fn import_sync_ingest(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    source_ids: &BTreeMap<String, i64>,
) -> Result<()> {
    for (hash, payload) in objects_of_kind(state, "ingest") {
        let source_id = source_ids
            .get(hash)
            .ok_or_else(|| invalid_object("ingest", hash, "source is missing"))?;
        tx.execute(
            "INSERT INTO ingest_jobs(source_id,status,attempts,analysis,last_error,
                                     no_derived_pages_reason,updated_at)
             VALUES(?1,?2,0,?3,NULL,?4,?5)",
            params![
                source_id,
                required_str(payload, "status")?,
                optional_str(payload, "analysis")?,
                optional_str(payload, "no_derived_pages_reason")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
    }
    tx.execute(
        &format!(
            "INSERT INTO ingest_jobs(source_id,status,updated_at)
             SELECT id,'pending',{TIMESTAMP_SQL} FROM sources
             WHERE NOT EXISTS(SELECT 1 FROM ingest_jobs WHERE source_id=sources.id)"
        ),
        [],
    )?;
    Ok(())
}

fn import_sync_retrieval(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    source_ids: &BTreeMap<String, i64>,
) -> Result<()> {
    for (key, payload) in objects_of_kind(state, "retrieval_weight") {
        let target_type = required_str(payload, "target_type")?;
        let identifier = local_target_identifier(payload, target_type, source_ids, key)?;
        tx.execute(
            "INSERT INTO retrieval_weights(target_type,target_identifier,provenance,weight,reason,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                target_type,
                identifier,
                required_str(payload, "provenance")?,
                required_i64(payload, "weight")?,
                required_str(payload, "reason")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
    }
    for (key, payload) in objects_of_kind(state, "retrieval_feedback") {
        let target_type = required_str(payload, "target_type")?;
        let identifier = local_target_identifier(payload, target_type, source_ids, key)?;
        tx.execute(
            "INSERT INTO retrieval_feedback(query_fingerprint,target_type,target_identifier,
                                            provenance,signal,reason,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                required_str(payload, "query_fingerprint")?,
                target_type,
                identifier,
                required_str(payload, "provenance")?,
                required_i64(payload, "signal")?,
                required_str(payload, "reason")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
    }
    Ok(())
}

fn import_sync_relations(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    source_ids: &BTreeMap<String, i64>,
) -> Result<()> {
    for (id, payload) in objects_of_kind(state, "semantic_relation") {
        ensure_payload_key(payload, "id", id, "semantic_relation")?;
        let ids = string_array(payload, "source_hashes")?
            .iter()
            .map(|hash| {
                source_ids
                    .get(*hash)
                    .copied()
                    .ok_or_else(|| invalid_object("semantic_relation", id, "source is missing"))
            })
            .collect::<Result<Vec<_>>>()?;
        tx.execute(
            "INSERT INTO semantic_relations(id,relation_type,from_identifier,to_identifier,
                                            confidence,provenance,reason,source_ids_json,
                                            created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id,
                required_str(payload, "relation_type")?,
                local_relation_endpoint(required_str(payload, "from")?, source_ids, id)?,
                local_relation_endpoint(required_str(payload, "to")?, source_ids, id)?,
                optional_f64(payload, "confidence")?,
                required_str(payload, "provenance")?,
                optional_str(payload, "reason")?,
                serde_json::to_string(&ids)
                    .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
                required_str(payload, "created_at")?,
                required_str(payload, "updated_at")?,
            ],
        )?;
    }
    Ok(())
}

fn local_relation_endpoint(
    endpoint: &str,
    source_ids: &BTreeMap<String, i64>,
    relation_id: &str,
) -> Result<String> {
    let Some(hash) = endpoint.strip_prefix("source-hash:") else {
        return Ok(endpoint.to_owned());
    };
    source_ids
        .get(hash)
        .map(|id| format!("source:{id}"))
        .ok_or_else(|| {
            invalid_object(
                "semantic_relation",
                relation_id,
                "source endpoint is missing",
            )
        })
}

fn import_sync_memory(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    local_requests: &BTreeMap<String, String>,
) -> Result<()> {
    for (id, payload) in objects_of_kind(state, "memory") {
        ensure_payload_key(payload, "id", id, "memory")?;
        tx.execute(
            "INSERT INTO memory_events(id,request_id,fingerprint,event_type,context,occurred_at,
                                       recorded_at,valid_from,valid_until,pinned,logical_bytes)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                local_requests.get(id),
                required_str(payload, "fingerprint")?,
                required_str(payload, "type")?,
                required_str(payload, "context")?,
                required_str(payload, "occurred_at")?,
                required_str(payload, "recorded_at")?,
                optional_str(payload, "valid_from")?,
                optional_str(payload, "valid_to")?,
                bool_i64(payload, "pinned")?,
                required_i64(payload, "logical_bytes")?,
            ],
        )?;
        for (field, kind) in [
            ("observed", "observed"),
            ("decision", "decision"),
            ("constraints", "constraint"),
            ("learned", "learned"),
            ("unresolved", "unresolved"),
            ("outcome", "outcome"),
        ] {
            for (ordinal, value) in string_array(payload, field)?.iter().enumerate() {
                tx.execute(
                    "INSERT INTO memory_fragments(event_id,kind,ordinal,value) VALUES(?1,?2,?3,?4)",
                    params![id, kind, ordinal as i64, value],
                )?;
            }
        }
        for (ordinal, value) in required_array(payload, "changes")?.iter().enumerate() {
            tx.execute(
                "INSERT INTO memory_changes(event_id,ordinal,subject,before_value,after_value,reason)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id,
                    ordinal as i64,
                    required_str(value, "subject")?,
                    optional_str(value, "before")?,
                    optional_str(value, "after")?,
                    optional_str(value, "reason")?,
                ],
            )?;
        }
        for (ordinal, value) in required_array(payload, "evidence")?.iter().enumerate() {
            tx.execute(
                "INSERT INTO memory_evidence(event_id,ordinal,reference,excerpt) VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    ordinal as i64,
                    required_str(value, "reference")?,
                    optional_str(value, "excerpt")?,
                ],
            )?;
        }
    }
    for (id, payload) in objects_of_kind(state, "memory") {
        for (ordinal, value) in required_array(payload, "relations")?.iter().enumerate() {
            tx.execute(
                "INSERT INTO memory_relations(event_id,ordinal,relation_type,target_event_id,basis)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    id,
                    ordinal as i64,
                    required_str(value, "type")?,
                    required_str(value, "target")?,
                    optional_str(value, "basis")?,
                ],
            )?;
        }
        for value in required_array(payload, "feedback")? {
            tx.execute(
                "INSERT INTO memory_feedback(event_id,signal,reason,created_at) VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    required_str(value, "signal")?,
                    required_str(value, "reason")?,
                    required_str(value, "created_at")?,
                ],
            )?;
        }
    }
    tx.execute(
        "UPDATE memory_state SET
             event_count=(SELECT COUNT(*) FROM memory_events),
             logical_bytes=COALESCE((SELECT SUM(logical_bytes) FROM memory_events),0)",
        [],
    )?;
    Ok(())
}

fn import_sync_todos(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    local_revisions: &BTreeMap<String, i64>,
    local_requests: &BTreeMap<String, String>,
) -> Result<()> {
    for (id, payload) in objects_of_kind(state, "todo") {
        ensure_payload_key(payload, "id", id, "todo")?;
        let encoded = serde_json::to_string(payload)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        tx.execute(
            "INSERT INTO todo_items(id,request_id,fingerprint,title,cue,detail,state,result,
                                    cancel_reason,revision,created_at,updated_at,closed_at,parent_id,target_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,NULL,?14)",
            params![
                id,
                local_requests.get(id),
                hash_content(&encoded),
                required_str(payload, "title")?,
                optional_str(payload, "cue")?,
                optional_str(payload, "detail")?,
                required_str(payload, "state")?,
                optional_str(payload, "result")?,
                optional_str(payload, "cancel_reason")?,
                next_sync_local_revision(local_revisions.get(id).copied(), "Todo", id)?,
                required_str(payload, "created_at")?,
                required_str(payload, "updated_at")?,
                optional_str(payload, "closed_at")?,
                optional_str(payload, "target_at")?,
            ],
        )?;
        for tag in string_array(payload, "tags")? {
            tx.execute(
                "INSERT INTO todo_tags(todo_id,tag_name) VALUES(?1,?2)",
                params![id, tag],
            )?;
        }
    }
    for (id, payload) in objects_of_kind(state, "todo") {
        if let Some(parent) = optional_str(payload, "parent_id")? {
            tx.execute(
                "UPDATE todo_items SET parent_id=?1 WHERE id=?2",
                params![parent, id],
            )?;
        }
    }
    Ok(())
}

fn import_sync_plans(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    local_revisions: &BTreeMap<String, i64>,
    local_requests: &BTreeMap<String, String>,
) -> Result<()> {
    for (id, payload) in objects_of_kind(state, "plan") {
        ensure_payload_key(payload, "id", id, "plan")?;
        let encoded = serde_json::to_string(payload)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        let history_len = i64::try_from(required_array(payload, "history")?.len())
            .map_err(|_| invalid_object("plan", id, "history is too large"))?;
        let local_revision =
            next_sync_local_revision(local_revisions.get(id).copied(), "Plan", id)?
                .max(history_len.max(1));
        tx.execute(
            "INSERT INTO plans(id,request_id,fingerprint,title,objective,done_when,state,result,
                               completion_evidence,done_when_checked,abandoned_reason,revision,
                               created_at,updated_at,closed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                id,
                local_requests.get(id),
                hash_content(&encoded),
                required_str(payload, "title")?,
                required_str(payload, "objective")?,
                required_str(payload, "done_when")?,
                required_str(payload, "state")?,
                optional_str(payload, "result")?,
                optional_str(payload, "completion_evidence")?,
                bool_i64(payload, "done_when_checked")?,
                optional_str(payload, "abandoned_reason")?,
                local_revision,
                required_str(payload, "created_at")?,
                required_str(payload, "updated_at")?,
                optional_str(payload, "closed_at")?,
            ],
        )?;
        for tag in string_array(payload, "tags")? {
            tx.execute(
                "INSERT INTO plan_tags(plan_id,tag_name) VALUES(?1,?2)",
                params![id, tag],
            )?;
        }
        for (ordinal, value) in string_array(payload, "constraints")?.iter().enumerate() {
            tx.execute(
                "INSERT INTO plan_constraints(plan_id,ordinal,value) VALUES(?1,?2,?3)",
                params![id, ordinal as i64, value],
            )?;
        }
        for step in required_array(payload, "steps")? {
            tx.execute(
                "INSERT INTO plan_steps(plan_id,step_id,ordinal,title,status,verify,result,blocker,
                                        created_revision,updated_revision,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    id,
                    required_str(step, "id")?,
                    required_i64(step, "ordinal")?,
                    required_str(step, "title")?,
                    required_str(step, "status")?,
                    optional_str(step, "verify")?,
                    optional_str(step, "result")?,
                    optional_str(step, "blocker")?,
                    1,
                    local_revision,
                    required_str(step, "created_at")?,
                    required_str(step, "updated_at")?,
                ],
            )?;
        }
        for value in required_array(payload, "history")? {
            let revision = required_i64(value, "ordinal")?
                .checked_add(1)
                .ok_or_else(|| invalid_object("plan", id, "history ordinal overflows"))?;
            tx.execute(
                "INSERT INTO plan_history(plan_id,revision,action,reason,step_id,result,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    id,
                    revision,
                    required_str(value, "action")?,
                    optional_str(value, "reason")?,
                    optional_str(value, "step_id")?,
                    optional_str(value, "result")?,
                    required_str(value, "created_at")?,
                ],
            )?;
        }
    }
    Ok(())
}

fn next_sync_local_revision(current: Option<i64>, kind: &str, id: &str) -> Result<i64> {
    current.map_or(Ok(1), |revision| {
        revision.checked_add(1).ok_or_else(|| {
            AppError::new(
                "sync_state_invalid",
                format!("normalized {kind} '{id}' local revision overflows"),
            )
        })
    })
}

fn rebuild_sync_indexes(
    tx: &Transaction<'_>,
    state: &PreparedSyncState,
    affected: &SyncAffected,
    old_source_ids: &BTreeMap<String, Vec<i64>>,
) -> Result<()> {
    for slug in affected.keys("page") {
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type='page' AND identifier=?1",
            [slug],
        )?;
        deactivate_search_spans(tx, "page", slug)?;
        let page = tx
            .query_row(
                "SELECT title,summary,body FROM pages WHERE slug=?1",
                [slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((title, summary, body)) = page {
            index_page(tx, None, slug, &title, summary.as_deref(), &body)?;
        }
    }
    for hash in affected.keys("source") {
        let mut ids = old_source_ids.get(hash).cloned().unwrap_or_default();
        let current = tx
            .query_row(
                "SELECT id,title,origin,length(CAST(content AS BLOB))
                 FROM sources WHERE content_hash=?1",
                [hash],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((id, _, _, _)) = &current {
            ids.push(*id);
        }
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let identifier = id.to_string();
            tx.execute(
                "DELETE FROM search_fts WHERE doc_type='source' AND identifier=?1",
                [&identifier],
            )?;
            deactivate_search_spans(tx, "source", &identifier)?;
        }
        if let Some((id, title, origin, content_bytes)) = current {
            if content_bytes > SYNC_SOURCE_INDEX_MAX_BYTES {
                return Err(AppError::new(
                    "sync_source_index_too_large",
                    format!(
                        "Source {hash} is {content_bytes} bytes; the derived Sync index limit is {SYNC_SOURCE_INDEX_MAX_BYTES} bytes"
                    ),
                ));
            }
            let content: String = tx.query_row(
                "SELECT content FROM sources WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            index_source(tx, None, id, title.as_deref(), &origin, &content)?;
        }
    }
    for id in affected.keys("memory") {
        tx.execute("DELETE FROM memory_fts WHERE event_id=?1", [id])?;
        let Some(payload) = object(state, "memory", id) else {
            continue;
        };
        let input = MemoryEventInput {
            request_id: None,
            event_type: required_str(payload, "type")?.to_owned(),
            context: required_str(payload, "context")?.to_owned(),
            occurred_at: Some(required_str(payload, "occurred_at")?.to_owned()),
            valid_from: optional_str(payload, "valid_from")?.map(str::to_owned),
            valid_to: optional_str(payload, "valid_to")?.map(str::to_owned),
            pinned: required_bool(payload, "pinned")?,
            observed: owned_string_array(payload, "observed")?,
            decision: owned_string_array(payload, "decision")?,
            constraints: owned_string_array(payload, "constraints")?,
            learned: owned_string_array(payload, "learned")?,
            unresolved: owned_string_array(payload, "unresolved")?,
            outcome: owned_string_array(payload, "outcome")?,
            changes: serde_json::from_value(
                payload.get("changes").cloned().unwrap_or_else(|| json!([])),
            )
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
            evidence: serde_json::from_value(
                payload
                    .get("evidence")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
            relations: serde_json::from_value(
                payload
                    .get("relations")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
        };
        index_memory_event(tx, id, &input)?;
    }
    for id in affected.keys("todo") {
        tx.execute("DELETE FROM todo_fts WHERE todo_id=?1", [id])?;
        let Some(payload) = object(state, "todo", id) else {
            continue;
        };
        let tags = owned_string_array(payload, "tags")?;
        index_todo(
            tx,
            id,
            required_str(payload, "title")?,
            &tags,
            optional_str(payload, "cue")?,
            optional_str(payload, "detail")?,
        )?;
    }
    for id in affected.keys("plan") {
        tx.execute("DELETE FROM plan_fts WHERE plan_id=?1", [id])?;
        let Some(payload) = object(state, "plan", id) else {
            continue;
        };
        index_plan(tx, id, payload)?;
    }
    Ok(())
}

fn objects_of_kind<'a>(
    state: &'a PreparedSyncState,
    kind: &'a str,
) -> impl Iterator<Item = (&'a str, &'a Value)> {
    state
        .objects
        .iter()
        .filter_map(move |((object_kind, key), value)| {
            (object_kind == kind).then_some((key.as_str(), value))
        })
}

fn object<'a>(state: &'a PreparedSyncState, kind: &str, key: &str) -> Option<&'a Value> {
    state.objects.get(&(kind.to_owned(), key.to_owned()))
}

fn invalid_object(kind: &str, key: &str, message: &str) -> AppError {
    AppError::new(
        "sync_state_invalid",
        format!("normalized {kind} '{key}' {message}"),
    )
}

fn required_str<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            format!("field '{field}' must be text"),
        )
    })
}

fn optional_str<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_str().map(Some).ok_or_else(|| {
            AppError::new(
                "sync_state_invalid",
                format!("field '{field}' must be text or null"),
            )
        }),
    }
}

fn required_i64(value: &Value, field: &str) -> Result<i64> {
    value.get(field).and_then(Value::as_i64).ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            format!("field '{field}' must be an integer"),
        )
    })
}

fn required_bool(value: &Value, field: &str) -> Result<bool> {
    value.get(field).and_then(Value::as_bool).ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            format!("field '{field}' must be a boolean"),
        )
    })
}

fn bool_i64(value: &Value, field: &str) -> Result<i64> {
    required_bool(value, field).map(i64::from)
}

fn optional_f64(value: &Value, field: &str) -> Result<Option<f64>> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_f64().map(Some).ok_or_else(|| {
            AppError::new(
                "sync_state_invalid",
                format!("field '{field}' must be a number or null"),
            )
        }),
    }
}

fn required_array<'a>(value: &'a Value, field: &str) -> Result<&'a [Value]> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            AppError::new(
                "sync_state_invalid",
                format!("field '{field}' must be an array"),
            )
        })
}

fn string_array<'a>(value: &'a Value, field: &str) -> Result<Vec<&'a str>> {
    required_array(value, field)?
        .iter()
        .map(|value| {
            value.as_str().ok_or_else(|| {
                AppError::new(
                    "sync_state_invalid",
                    format!("field '{field}' must contain text"),
                )
            })
        })
        .collect()
}

fn owned_string_array(value: &Value, field: &str) -> Result<Vec<String>> {
    Ok(string_array(value, field)?
        .into_iter()
        .map(str::to_owned)
        .collect())
}

fn ensure_payload_key(value: &Value, field: &str, key: &str, kind: &str) -> Result<()> {
    if required_str(value, field)? != key {
        return Err(invalid_object(kind, key, "identifier differs from its key"));
    }
    Ok(())
}

fn local_target_identifier(
    payload: &Value,
    target_type: &str,
    source_ids: &BTreeMap<String, i64>,
    key: &str,
) -> Result<String> {
    let identifier = required_str(payload, "target_identifier")?;
    if target_type == "source" {
        source_ids
            .get(identifier)
            .map(i64::to_string)
            .ok_or_else(|| invalid_object("retrieval", key, "source target is missing"))
    } else {
        Ok(identifier.to_owned())
    }
}
