use rusqlite::session::{ConflictAction, Session};

const SYNC_STATE_FORMAT: i64 = 1;
#[cfg(test)]
static SYNC_BLOB_MAX_BUFFERED_BYTES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SyncExportSummary {
    pub(crate) object_count: i64,
    pub(crate) blob_count: i64,
    pub(crate) state_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SyncTransferKind {
    Full,
    Delta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SyncTransferSummary {
    pub(crate) kind: SyncTransferKind,
    pub(crate) size: u64,
    pub(crate) state_digest: String,
    pub(crate) baseline_digest: Option<String>,
}

#[derive(Debug, Clone)]
#[cfg(test)]
pub(crate) struct SyncTransfer {
    pub(crate) kind: SyncTransferKind,
    pub(crate) bytes: Vec<u8>,
    pub(crate) state_digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SyncMergeSummary {
    pub(crate) state_digest: String,
    pub(crate) conflict_count: usize,
    pub(crate) conflict_kinds: Vec<String>,
    pub(crate) conflicts: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SyncObjectRow {
    payload: Value,
}

impl Store {
    pub(crate) fn export_sync_state(&self, path: &Path) -> Result<SyncExportSummary> {
        let parent = path.parent().ok_or_else(|| {
            AppError::new("invalid_store_path", "Sync state has no parent directory")
        })?;
        fs::create_dir_all(parent)?;
        let snapshot_path = parent.join(format!(
            ".sync-snapshot-{}-{}.db",
            std::process::id(),
            SOURCE_STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        create_sync_live_snapshot(&self.conn, &snapshot_path)?;
        let _snapshot = TemporaryDatabase(snapshot_path.clone());
        Store::open_for_read(self.scope.clone(), &snapshot_path)?
            .export_sync_state_from_snapshot(path)
    }

    fn export_sync_state_from_snapshot(&self, path: &Path) -> Result<SyncExportSummary> {
        if path.exists() {
            return Err(AppError::new(
                "sync_state_exists",
                format!("Sync state already exists: {}", path.display()),
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let output = Connection::open(path)?;
        output.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             CREATE TABLE sync_manifest(
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE sync_objects(
                 kind TEXT NOT NULL,
                 logical_key TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 payload_hash TEXT NOT NULL,
                 PRIMARY KEY(kind,logical_key)
             ) WITHOUT ROWID;
             CREATE TABLE sync_blobs(
                 content_hash TEXT NOT NULL PRIMARY KEY,
                 content BLOB NOT NULL
             );",
        )?;
        output.execute(
            "INSERT INTO sync_manifest(key,value) VALUES('format',?1)",
            [SYNC_STATE_FORMAT.to_string()],
        )?;
        output.execute(
            "INSERT INTO sync_manifest(key,value) VALUES('store_format',?1)",
            [USER_VERSION.to_string()],
        )?;

        self.export_sync_objects(&output, true)?;

        output.execute_batch("VACUUM;")?;
        let object_count =
            output.query_row("SELECT COUNT(*) FROM sync_objects", [], |r| r.get(0))?;
        let blob_count = output.query_row("SELECT COUNT(*) FROM sync_blobs", [], |r| r.get(0))?;
        drop(output);
        Ok(SyncExportSummary {
            object_count,
            blob_count,
            state_digest: sync_state_digest(path)?,
        })
    }

    fn export_sync_object_inventory(&self, path: &Path) -> Result<()> {
        if path.exists() {
            return Err(AppError::new(
                "sync_state_exists",
                format!("Sync inventory already exists: {}", path.display()),
            ));
        }
        let output = Connection::open(path)?;
        output.execute_batch(
            "PRAGMA journal_mode=DELETE;
             CREATE TABLE sync_objects(
                 kind TEXT NOT NULL,
                 logical_key TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 payload_hash TEXT NOT NULL,
                 PRIMARY KEY(kind,logical_key)
             ) WITHOUT ROWID;",
        )?;
        self.export_sync_objects(&output, false)
    }

    fn export_sync_objects(&self, output: &Connection, copy_blobs: bool) -> Result<()> {
        self.export_sync_meta(output)?;
        let source_hashes = self.export_sync_sources(output, copy_blobs)?;
        self.export_sync_pages(output, &source_hashes)?;
        self.export_sync_tags(output)?;
        self.export_sync_ingest(output, &source_hashes)?;
        self.export_sync_retrieval(output, &source_hashes)?;
        self.export_sync_relations(output, &source_hashes)?;
        self.export_sync_memory(output)?;
        self.export_sync_todos(output)?;
        self.export_sync_plans(output)
    }

    fn export_sync_meta(&self, output: &Connection) -> Result<()> {
        for key in ["schema", "purpose"] {
            if let Some(value) = self
                .conn
                .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            {
                insert_sync_object(output, "meta", key, &json!({"value": value}))?;
            }
        }
        Ok(())
    }

    fn export_sync_sources(
        &self,
        output: &Connection,
        copy_blobs: bool,
    ) -> Result<BTreeMap<i64, String>> {
        if copy_blobs {
            let snapshot_database = self.database.to_string_lossy().into_owned();
            output.execute(
                "ATTACH DATABASE ?1 AS sync_live_snapshot",
                [snapshot_database],
            )?;
        }
        let mut ids = BTreeMap::new();
        let mut statement = self.conn.prepare(
            "SELECT id,content_hash,title,origin,structural_navigation,created_at
             FROM sources ORDER BY content_hash",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, content_hash, title, origin, structural_navigation, created_at) = row?;
            ids.insert(id, content_hash.clone());
            let portable_origin = if Path::new(&origin).is_absolute() {
                None
            } else {
                Some(origin)
            };
            insert_sync_object(
                output,
                "source",
                &content_hash,
                &json!({
                    "content_hash": content_hash,
                    "title": title,
                    "origin": portable_origin,
                    "structural_navigation": structural_navigation,
                    "created_at": created_at,
                }),
            )?;
            if copy_blobs {
                output.execute(
                    "INSERT INTO sync_blobs(content_hash,content)
                     SELECT content_hash,CAST(content AS BLOB)
                     FROM sync_live_snapshot.sources WHERE id=?1",
                    [id],
                )?;
            }
        }
        Ok(ids)
    }

    fn export_sync_pages(
        &self,
        output: &Connection,
        source_hashes: &BTreeMap<i64, String>,
    ) -> Result<()> {
        let mut statement = self.conn.prepare("SELECT slug FROM pages ORDER BY slug")?;
        let slugs = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for slug in slugs {
            let page = self.page_show(&slug)?.page;
            let structural_navigation: bool = self.conn.query_row(
                "SELECT structural_navigation FROM pages WHERE slug=?1",
                [&slug],
                |row| row.get(0),
            )?;
            let hashes = page
                .source_ids
                .iter()
                .map(|id| {
                    source_hashes.get(id).cloned().ok_or_else(|| {
                        AppError::new(
                            "corrupt_store",
                            format!("page {slug} cites missing source {id}"),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            insert_sync_object(
                output,
                "page",
                &slug,
                &json!({
                    "slug": page.slug,
                    "title": page.title,
                    "kind": page.kind,
                    "summary": page.summary,
                    "body": page.body,
                    "structural_navigation": structural_navigation,
                    "source_hashes": hashes,
                    "provenance": page.provenance,
                    "created_at": page.created_at,
                    "updated_at": page.updated_at,
                }),
            )?;
        }
        Ok(())
    }

    fn export_sync_tags(&self, output: &Connection) -> Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT name,autoload,autoload_priority,autoload_limit,autoload_max_chars,reason,updated_at
             FROM tags ORDER BY name",
        )?;
        let rows = statement.query_map([], |r| {
            Ok(json!({
                "name": r.get::<_, String>(0)?,
                "autoload": r.get::<_, bool>(1)?,
                "autoload_priority": r.get::<_, i64>(2)?,
                "autoload_limit": r.get::<_, i64>(3)?,
                "autoload_max_chars": r.get::<_, i64>(4)?,
                "reason": r.get::<_, String>(5)?,
                "updated_at": r.get::<_, String>(6)?,
            }))
        })?;
        for mut tag in rows.collect::<rusqlite::Result<Vec<_>>>()? {
            let name = tag["name"].as_str().unwrap().to_string();
            let mut pages = self.conn.prepare(
                "SELECT page_slug,priority,reason,created_at,updated_at
                 FROM page_tags WHERE tag_name=?1 ORDER BY page_slug",
            )?;
            tag["pages"] = json!(
                pages
                    .query_map([&name], |r| Ok(json!({
                        "slug": r.get::<_, String>(0)?,
                        "priority": r.get::<_, i64>(1)?,
                        "reason": r.get::<_, String>(2)?,
                        "created_at": r.get::<_, String>(3)?,
                        "updated_at": r.get::<_, String>(4)?,
                    })))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            );
            insert_sync_object(output, "tag", &name, &tag)?;
        }
        Ok(())
    }

    fn export_sync_ingest(
        &self,
        output: &Connection,
        source_hashes: &BTreeMap<i64, String>,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT source_id,status,analysis,no_derived_pages_reason,updated_at
             FROM ingest_jobs ORDER BY source_id",
        )?;
        for row in statement.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })? {
            let (id, status, analysis, no_derived_pages_reason, updated_at) = row?;
            let Some(hash) = source_hashes.get(&id) else {
                continue;
            };
            insert_sync_object(
                output,
                "ingest",
                hash,
                &json!({
                    "source_hash": hash,
                    "status": status,
                    "analysis": analysis,
                    "no_derived_pages_reason": no_derived_pages_reason,
                    "updated_at": updated_at,
                }),
            )?;
        }
        Ok(())
    }

    fn export_sync_retrieval(
        &self,
        output: &Connection,
        source_hashes: &BTreeMap<i64, String>,
    ) -> Result<()> {
        let stable = |target_type: &str, identifier: String| -> Option<String> {
            if target_type == "source" {
                identifier
                    .parse::<i64>()
                    .ok()
                    .and_then(|id| source_hashes.get(&id).cloned())
            } else {
                Some(identifier)
            }
        };
        let mut weights = self.conn.prepare(
            "SELECT target_type,target_identifier,provenance,weight,reason,updated_at
             FROM retrieval_weights ORDER BY target_type,target_identifier,provenance",
        )?;
        for row in weights.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })? {
            let (target_type, identifier, provenance, weight, reason, updated_at) = row?;
            let Some(identifier) = stable(&target_type, identifier) else {
                continue;
            };
            let key = format!("{target_type}\0{identifier}\0{provenance}");
            insert_sync_object(
                output,
                "retrieval_weight",
                &key,
                &json!({
                    "target_type": target_type, "target_identifier": identifier,
                    "provenance": provenance, "weight": weight, "reason": reason,
                    "updated_at": updated_at,
                }),
            )?;
        }
        let mut feedback = self.conn.prepare(
            "SELECT query_fingerprint,target_type,target_identifier,provenance,signal,reason,updated_at
             FROM retrieval_feedback
             ORDER BY query_fingerprint,target_type,target_identifier,provenance",
        )?;
        for row in feedback.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })? {
            let (query, target_type, identifier, provenance, signal, reason, updated_at) = row?;
            let Some(identifier) = stable(&target_type, identifier) else {
                continue;
            };
            let key = format!("{query}\0{target_type}\0{identifier}\0{provenance}");
            insert_sync_object(
                output,
                "retrieval_feedback",
                &key,
                &json!({
                    "query_fingerprint": query, "target_type": target_type,
                    "target_identifier": identifier, "provenance": provenance,
                    "signal": signal, "reason": reason, "updated_at": updated_at,
                }),
            )?;
        }
        Ok(())
    }

    fn export_sync_relations(
        &self,
        output: &Connection,
        source_hashes: &BTreeMap<i64, String>,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(
            "SELECT id,relation_type,from_identifier,to_identifier,confidence,provenance,
                    reason,source_ids_json,created_at,updated_at
             FROM semantic_relations ORDER BY id",
        )?;
        for row in statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
            ))
        })? {
            let (
                id,
                relation_type,
                from,
                to,
                confidence,
                provenance,
                reason,
                source_ids,
                created_at,
                updated_at,
            ) = row?;
            let ids: Vec<i64> = serde_json::from_str(&source_ids).map_err(|error| {
                AppError::new(
                    "corrupt_store",
                    format!("invalid semantic source IDs: {error}"),
                )
            })?;
            let hashes = ids
                .into_iter()
                .filter_map(|id| source_hashes.get(&id).cloned())
                .collect::<Vec<_>>();
            insert_sync_object(
                output,
                "semantic_relation",
                &id,
                &json!({
                    "id": id, "relation_type": relation_type,
                    "from": stable_relation_endpoint(&from, source_hashes)?,
                    "to": stable_relation_endpoint(&to, source_hashes)?,
                    "confidence": confidence, "provenance": provenance, "reason": reason,
                    "source_hashes": hashes, "created_at": created_at, "updated_at": updated_at,
                }),
            )?;
        }
        Ok(())
    }

    fn export_sync_memory(&self, output: &Connection) -> Result<()> {
        let mut statement = self
            .conn
            .prepare("SELECT id FROM memory_events ORDER BY id")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            let mut event = load_memory_event(&self.conn, &id)?;
            event.as_object_mut().unwrap().remove("request_id");
            let mut feedback = self.conn.prepare(
                "SELECT signal,reason,created_at FROM memory_feedback
                 WHERE event_id=?1 ORDER BY created_at,id",
            )?;
            event["feedback"] = json!(
                feedback
                    .query_map([&id], |r| Ok(json!({
                        "signal": r.get::<_, String>(0)?, "reason": r.get::<_, String>(1)?,
                        "created_at": r.get::<_, String>(2)?,
                    })))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            );
            insert_sync_object(output, "memory", &id, &event)?;
        }
        Ok(())
    }

    fn export_sync_todos(&self, output: &Connection) -> Result<()> {
        let mut statement = self.conn.prepare("SELECT id FROM todo_items ORDER BY id")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            let mut todo = load_todo(&self.conn, &id, &self.scope)?;
            let object = todo.as_object_mut().unwrap();
            object.remove("request_id");
            object.remove("scope");
            object.remove("children");
            object.remove("revision");
            insert_sync_object(output, "todo", &id, &todo)?;
        }
        Ok(())
    }

    fn export_sync_plans(&self, output: &Connection) -> Result<()> {
        let mut statement = self.conn.prepare("SELECT id FROM plans ORDER BY id")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            let mut plan = load_plan(&self.conn, &id, &self.scope)?;
            let object = plan.as_object_mut().unwrap();
            object.remove("request_id");
            object.remove("scope");
            object.remove("revision");
            if let Some(steps) = object.get_mut("steps").and_then(Value::as_array_mut) {
                for step in steps {
                    let step = step.as_object_mut().ok_or_else(|| {
                        AppError::new("corrupt_store", "Plan step is not an object")
                    })?;
                    step.remove("created_revision");
                    step.remove("updated_revision");
                }
            }
            let mut history = self.conn.prepare(
                "SELECT action,reason,step_id,result,created_at
                 FROM plan_history WHERE plan_id=?1 ORDER BY revision,id",
            )?;
            let events = history
                .query_map([&id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            plan["history"] = Value::Array(
                events
                    .into_iter()
                    .enumerate()
                    .map(|(ordinal, (action, reason, step_id, result, created_at))| {
                        json!({
                            "ordinal": ordinal as i64, "action": action, "reason": reason,
                            "step_id": step_id, "result": result, "created_at": created_at,
                        })
                    })
                    .collect(),
            );
            insert_sync_object(output, "plan", &id, &plan)?;
        }
        Ok(())
    }
}

fn create_sync_live_snapshot(source: &Connection, path: &Path) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)?;
    let result = (|| -> Result<()> {
        let mut destination = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let backup = Backup::new(source, &mut destination)?;
        backup.run_to_completion(100, Duration::from_millis(10), None)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn stable_relation_endpoint(
    endpoint: &str,
    source_hashes: &BTreeMap<i64, String>,
) -> Result<String> {
    let Some(id) = endpoint
        .strip_prefix("source:")
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return Ok(endpoint.to_owned());
    };
    source_hashes
        .get(&id)
        .map(|hash| format!("source-hash:{hash}"))
        .ok_or_else(|| {
            AppError::new(
                "corrupt_store",
                format!("semantic relation references missing source {id}"),
            )
        })
}

fn insert_sync_object(
    output: &Connection,
    kind: &str,
    logical_key: &str,
    payload: &Value,
) -> Result<()> {
    let payload_json = serde_json::to_string(payload)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    output.execute(
        "INSERT INTO sync_objects(kind,logical_key,payload_json,payload_hash)
         VALUES(?1,?2,?3,?4)",
        params![kind, logical_key, payload_json, hash_content(&payload_json)],
    )?;
    Ok(())
}

pub(crate) fn sync_state_digest(path: &Path) -> Result<String> {
    let conn = validate_sync_state_file(path)?;
    let format: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_manifest WHERE key='format'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if format.as_deref() != Some("1") {
        return Err(AppError::new(
            "sync_state_invalid",
            "unsupported normalized Sync state",
        ));
    }
    let store_format: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_manifest WHERE key='store_format'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let expected_store_format = USER_VERSION.to_string();
    if store_format.as_deref() != Some(expected_store_format.as_str()) {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("unsupported normalized store format: {store_format:?}"),
        ));
    }
    let mut hasher = Sha256::new();
    for sql in [
        "SELECT key,value,NULL FROM sync_manifest ORDER BY key",
        "SELECT kind||char(0)||logical_key,payload_hash,NULL FROM sync_objects ORDER BY kind,logical_key",
    ] {
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?;
        for row in rows {
            let (key, value, blob) = row?;
            for bytes in [
                key.as_bytes(),
                value.as_bytes(),
                blob.as_deref().unwrap_or_default(),
            ] {
                hasher.update((bytes.len() as u64).to_be_bytes());
                hasher.update(bytes);
            }
        }
    }
    let mut statement = conn.prepare(
        "SELECT rowid,content_hash,length(content) FROM sync_blobs ORDER BY content_hash",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (rowid, content_hash, content_len) = row?;
        let content_len_text = content_len.to_string();
        for bytes in [content_hash.as_bytes(), content_len_text.as_bytes()] {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        hasher.update((content_len as u64).to_be_bytes());
        let actual_hash = hash_and_validate_sync_blob(&conn, rowid, &mut hasher)?;
        let expected_content_hash = content_hash
            .strip_prefix("sync-candidate:")
            .unwrap_or(&content_hash);
        if actual_hash != expected_content_hash {
            return Err(AppError::new(
                "sync_checksum_mismatch",
                format!("Sync blob checksum differs for {content_hash}"),
            ));
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hash_and_validate_sync_blob(
    conn: &Connection,
    rowid: i64,
    state_hasher: &mut Sha256,
) -> Result<String> {
    const CHUNK_BYTES: usize = 64 * 1024;
    let blob = conn.blob_open(MAIN_DB, "sync_blobs", "content", rowid, true)?;
    let mut content_hasher = Sha256::new();
    let mut chunk = vec![0_u8; CHUNK_BYTES];
    let mut carry = Vec::with_capacity(4);
    let mut offset = 0_usize;
    while offset < blob.len() {
        let count = blob.read_at(&mut chunk, offset)?;
        if count == 0 {
            return Err(AppError::new(
                "sync_state_invalid",
                "incremental Sync blob read ended early",
            ));
        }
        let bytes = &chunk[..count];
        #[cfg(test)]
        SYNC_BLOB_MAX_BUFFERED_BYTES
            .fetch_max((carry.len() + bytes.len()) as u64, Ordering::Relaxed);
        content_hasher.update(bytes);
        state_hasher.update(bytes);
        let mut utf8 = Vec::with_capacity(carry.len() + bytes.len());
        utf8.extend_from_slice(&carry);
        utf8.extend_from_slice(bytes);
        carry.clear();
        if let Err(error) = std::str::from_utf8(&utf8) {
            if error.error_len().is_some() {
                return Err(AppError::new(
                    "sync_state_invalid",
                    "Sync source content is not UTF-8",
                ));
            }
            carry.extend_from_slice(&utf8[error.valid_up_to()..]);
            if carry.len() > 3 {
                return Err(AppError::new(
                    "sync_state_invalid",
                    "Sync source content is not UTF-8",
                ));
            }
        }
        offset += count;
    }
    if !carry.is_empty() {
        return Err(AppError::new(
            "sync_state_invalid",
            "Sync source content is not UTF-8",
        ));
    }
    Ok(content_hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_sync_state_file(path: &Path) -> Result<Connection> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        AppError::new(
            "sync_state_invalid",
            format!("normalized Sync state is unavailable: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(AppError::new(
            "sync_state_invalid",
            "normalized Sync state must be a regular non-symlink file",
        ));
    }
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| {
            AppError::new(
                "sync_state_invalid",
                format!("invalid SQLite state: {error}"),
            )
        })?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    if integrity != "ok" {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("normalized Sync state integrity check failed: {integrity}"),
        ));
    }

    let actual_schema = {
        let mut statement = conn
            .prepare(
                "SELECT type,name FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
            )
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
    };
    let expected_schema = vec![
        ("table".to_string(), "sync_blobs".to_string()),
        ("table".to_string(), "sync_manifest".to_string()),
        ("table".to_string(), "sync_objects".to_string()),
    ];
    if actual_schema != expected_schema {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("unexpected normalized Sync schema: {actual_schema:?}"),
        ));
    }

    for (table, expected) in [
        (
            "sync_manifest",
            vec![("key", "TEXT", 1, 1), ("value", "TEXT", 1, 0)],
        ),
        (
            "sync_objects",
            vec![
                ("kind", "TEXT", 1, 1),
                ("logical_key", "TEXT", 1, 2),
                ("payload_json", "TEXT", 1, 0),
                ("payload_hash", "TEXT", 1, 0),
            ],
        ),
        (
            "sync_blobs",
            vec![("content_hash", "TEXT", 1, 1), ("content", "BLOB", 1, 0)],
        ),
    ] {
        let sql = format!("PRAGMA table_info({table})");
        let mut statement = conn
            .prepare(&sql)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        let actual = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        let expected = expected
            .into_iter()
            .map(|(name, kind, not_null, primary_key)| {
                (name.to_string(), kind.to_string(), not_null, primary_key)
            })
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(AppError::new(
                "sync_state_invalid",
                format!("unexpected normalized Sync schema for {table}: {actual:?}"),
            ));
        }
    }

    let manifest_keys = {
        let mut statement = conn
            .prepare("SELECT key FROM sync_manifest ORDER BY key")
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
    };
    if manifest_keys != ["format", "store_format"] {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("unexpected normalized Sync manifest keys: {manifest_keys:?}"),
        ));
    }
    Ok(conn)
}

pub(crate) fn prepare_sync_transfer(
    baseline: Option<&Path>,
    current: &Path,
    artifact: &Path,
) -> Result<SyncTransferSummary> {
    if artifact.exists() {
        return Err(AppError::new(
            "sync_state_exists",
            format!("Sync transfer already exists: {}", artifact.display()),
        ));
    }
    if let Some(parent) = artifact.parent() {
        fs::create_dir_all(parent)?;
    }
    let state_digest = sync_state_digest(current)?;
    let baseline_digest = baseline.map(sync_state_digest).transpose()?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut output = options.open(artifact)?;
    let mut kind = SyncTransferKind::Full;

    if let Some(baseline) = baseline {
        ensure_sync_store_format_matches(baseline, current)?;
        let conn = Connection::open_with_flags(current, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.execute(
            "ATTACH DATABASE ?1 AS baseline",
            [baseline.to_string_lossy().as_ref()],
        )?;
        let mut session = Session::new(&conn)?;
        for table in ["sync_manifest", "sync_objects", "sync_blobs"] {
            session.diff::<&str, &str>("baseline", table)?;
        }
        session.changeset_strm(&mut output)?;
        output.sync_all()?;
        if output.metadata()?.len() < fs::metadata(current)?.len() {
            kind = SyncTransferKind::Delta;
        } else {
            output.set_len(0)?;
            output.rewind()?;
            let mut input = fs::File::open(current)?;
            std::io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        }
    } else {
        let mut input = fs::File::open(current)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    let size = output.metadata()?.len();
    Ok(SyncTransferSummary {
        kind,
        size,
        state_digest,
        baseline_digest,
    })
}

pub(crate) fn apply_sync_transfer_artifact(
    baseline: Option<&Path>,
    artifact: &Path,
    summary: &SyncTransferSummary,
    destination: &Path,
) -> Result<()> {
    if destination.exists() {
        return Err(AppError::new(
            "sync_state_exists",
            "Sync destination already exists",
        ));
    }
    let result = (|| -> Result<()> {
        match summary.kind {
            SyncTransferKind::Full => {
                fs::copy(artifact, destination)?;
            }
            SyncTransferKind::Delta => {
                let baseline = baseline.ok_or_else(|| {
                    AppError::new(
                        "sync_baseline_missing",
                        "a delta requires its confirmed baseline",
                    )
                })?;
                let actual_baseline = sync_state_digest(baseline)?;
                if summary.baseline_digest.as_deref() != Some(actual_baseline.as_str()) {
                    return Err(AppError::new(
                        "sync_baseline_mismatch",
                        "the delta baseline checksum differs",
                    ));
                }
                fs::copy(baseline, destination)?;
                let conn = Connection::open(destination)?;
                let mut input = fs::File::open(artifact)?;
                conn.apply_strm(&mut input, None::<fn(&str) -> bool>, |_kind, _item| {
                    ConflictAction::SQLITE_CHANGESET_ABORT
                })?;
            }
        }
        if sync_state_digest(destination)? != summary.state_digest {
            return Err(AppError::new(
                "sync_checksum_mismatch",
                "reconstructed Sync state checksum differs",
            ));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

fn ensure_sync_store_format_matches(baseline: &Path, current: &Path) -> Result<()> {
    let store_format = |path: &Path| -> Result<String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.query_row(
            "SELECT value FROM sync_manifest WHERE key='store_format'",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
    };
    if store_format(baseline)? != store_format(current)? {
        return Err(AppError::new(
            "sync_baseline_mismatch",
            "normalized store formats differ",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn select_sync_transfer(
    baseline: Option<&Path>,
    current: &Path,
) -> Result<SyncTransfer> {
    let state_digest = sync_state_digest(current)?;
    let full = fs::read(current)?;
    let Some(baseline) = baseline.filter(|path| path.is_file()) else {
        return Ok(SyncTransfer {
            kind: SyncTransferKind::Full,
            bytes: full,
            state_digest,
        });
    };
    sync_state_digest(baseline)?;
    let conn = Connection::open_with_flags(current, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS baseline",
        [baseline.to_string_lossy().as_ref()],
    )?;
    let mut session = Session::new(&conn)?;
    session.diff::<&str, &str>("baseline", "sync_manifest")?;
    session.diff::<&str, &str>("baseline", "sync_objects")?;
    session.diff::<&str, &str>("baseline", "sync_blobs")?;
    let mut delta = Vec::new();
    session.changeset_strm(&mut delta)?;
    if delta.len() < full.len() {
        Ok(SyncTransfer {
            kind: SyncTransferKind::Delta,
            bytes: delta,
            state_digest,
        })
    } else {
        Ok(SyncTransfer {
            kind: SyncTransferKind::Full,
            bytes: full,
            state_digest,
        })
    }
}

#[cfg(test)]
pub(crate) fn apply_sync_transfer(
    baseline: Option<&Path>,
    transfer: &SyncTransfer,
    destination: &Path,
) -> Result<()> {
    if destination.exists() {
        return Err(AppError::new(
            "sync_state_exists",
            "Sync destination already exists",
        ));
    }
    match transfer.kind {
        SyncTransferKind::Full => fs::write(destination, &transfer.bytes)?,
        SyncTransferKind::Delta => {
            let baseline = baseline.ok_or_else(|| {
                AppError::new(
                    "sync_baseline_missing",
                    "a delta requires its confirmed baseline",
                )
            })?;
            fs::copy(baseline, destination)?;
            let conn = Connection::open(destination)?;
            conn.apply_strm(
                &mut transfer.bytes.as_slice(),
                None::<fn(&str) -> bool>,
                |_kind, _item| ConflictAction::SQLITE_CHANGESET_ABORT,
            )?;
        }
    }
    let actual = sync_state_digest(destination)?;
    if actual != transfer.state_digest {
        let _ = fs::remove_file(destination);
        return Err(AppError::new(
            "sync_checksum_mismatch",
            "reconstructed Sync state checksum differs",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn merge_sync_states(
    baseline: &Path,
    local: &Path,
    remote: &Path,
    destination: &Path,
) -> Result<SyncMergeSummary> {
    merge_sync_states_directional(baseline, local, baseline, remote, destination)
}

pub(crate) fn merge_sync_states_directional(
    baseline_local: &Path,
    current_local: &Path,
    baseline_remote: &Path,
    current_remote: &Path,
    destination: &Path,
) -> Result<SyncMergeSummary> {
    for path in [
        baseline_local,
        current_local,
        baseline_remote,
        current_remote,
    ] {
        sync_state_digest(path)?;
    }
    if destination.exists() {
        return Err(AppError::new(
            "sync_state_exists",
            "Sync merge destination already exists",
        ));
    }
    let base_local_objects = load_sync_objects(baseline_local)?;
    let local_objects = load_sync_objects(current_local)?;
    let base_remote_objects = load_sync_objects(baseline_remote)?;
    let remote_objects = load_sync_objects(current_remote)?;
    let keys = base_local_objects
        .keys()
        .chain(local_objects.keys())
        .chain(base_remote_objects.keys())
        .chain(remote_objects.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut merged = BTreeMap::new();
    let mut conflicts = Vec::new();
    let mut conflict_candidates = BTreeMap::new();
    for key in keys {
        let base_local = base_local_objects.get(&key);
        let left = local_objects.get(&key);
        let base_remote = base_remote_objects.get(&key);
        let right = remote_objects.get(&key);
        let value = merge_sync_object_directional(
            &key.0,
            &key.1,
            base_local,
            left,
            base_remote,
            right,
            &mut conflicts,
            &mut conflict_candidates,
        )?;
        if let Some(value) = value {
            merged.insert(key, value);
        }
    }

    let mut source_blobs = merged
        .keys()
        .filter(|(kind, _)| kind == "source")
        .map(|(_, logical_key)| logical_key.clone())
        .collect::<BTreeSet<_>>();
    for ((kind, logical_key), row) in &merged {
        if kind == "draft_intent" {
            source_blobs.extend(required_sync_draft_blobs(logical_key, &row.payload)?);
        }
    }
    fs::copy(baseline_local, destination)?;
    let conn = Connection::open(destination)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM sync_objects", [])?;
    for ((kind, logical_key), row) in merged {
        insert_sync_object(&tx, &kind, &logical_key, &row.payload)?;
    }
    tx.execute("DELETE FROM sync_blobs", [])?;
    merge_sync_blobs_into(
        &tx,
        &[
            baseline_local,
            current_local,
            baseline_remote,
            current_remote,
        ],
        &source_blobs,
    )?;
    for (hash, content) in conflict_candidates {
        tx.execute(
            "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,?2)",
            params![hash, content],
        )?;
    }
    tx.commit()?;
    drop(conn);

    conflicts.sort_by(|a, b| {
        (a["kind"].as_str(), a["logical_key"].as_str())
            .cmp(&(b["kind"].as_str(), b["logical_key"].as_str()))
    });
    let conflict_kinds = conflicts
        .iter()
        .filter_map(|conflict| conflict["kind"].as_str().map(str::to_string))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(SyncMergeSummary {
        state_digest: sync_state_digest(destination)?,
        conflict_count: conflicts.len(),
        conflict_kinds,
        conflicts,
    })
}

fn load_sync_objects(path: &Path) -> Result<BTreeMap<(String, String), SyncObjectRow>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = conn.prepare(
        "SELECT kind,logical_key,payload_json,payload_hash
         FROM sync_objects ORDER BY kind,logical_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut objects = BTreeMap::new();
    for row in rows {
        let (kind, key, encoded, expected) = row?;
        if hash_content(&encoded) != expected {
            return Err(AppError::new(
                "sync_checksum_mismatch",
                format!("normalized {kind} object checksum differs"),
            ));
        }
        let payload = serde_json::from_str(&encoded)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        objects.insert((kind, key), SyncObjectRow { payload });
    }
    Ok(objects)
}

#[allow(clippy::too_many_arguments)]
fn merge_sync_object_directional(
    kind: &str,
    logical_key: &str,
    base_left: Option<&SyncObjectRow>,
    left: Option<&SyncObjectRow>,
    base_right: Option<&SyncObjectRow>,
    right: Option<&SyncObjectRow>,
    conflicts: &mut Vec<Value>,
    conflict_candidates: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Option<SyncObjectRow>> {
    if left == right {
        return Ok(left.cloned());
    }
    if is_mechanical_sync_kind(kind) {
        return Ok(merge_mechanical_sync_object(
            base_left, left, base_right, right,
        ));
    }
    let left_changed = left != base_left;
    let right_changed = right != base_right;
    if kind == "memory" && (left.is_none() || right.is_none()) && !(left_changed && right_changed) {
        return Ok(left.or(right).cloned());
    }
    if base_left == base_right {
        if !left_changed && right_changed {
            return Ok(right.cloned());
        }
        if left_changed && !right_changed {
            return Ok(left.cloned());
        }
    }
    match (left, right) {
        (Some(left), Some(right)) => {
            if base_left.is_some() != base_right.is_some() {
                let candidate_refs = store_sync_conflict_candidates(
                    kind,
                    logical_key,
                    [Some(&left.payload), Some(&right.payload)],
                    conflict_candidates,
                )?;
                push_sync_conflict(
                    conflicts,
                    json!({
                        "kind": kind,
                        "logical_key": logical_key,
                        "conflict": "independent_create",
                        "candidate_refs": candidate_refs,
                        "fields": [{
                            "path": "$object",
                            "base": Value::Null,
                            "candidates": [
                                bounded_sync_value(&left.payload),
                                bounded_sync_value(&right.payload),
                            ],
                        }],
                    }),
                );
                return Ok(Some(
                    if canonical_sync_value(&left.payload) <= canonical_sync_value(&right.payload) {
                        left.clone()
                    } else {
                        right.clone()
                    },
                ));
            }
            let mut fields = Vec::new();
            let payload = merge_sync_value_directional(
                base_left.map(|row| &row.payload),
                &left.payload,
                base_right.map(|row| &row.payload),
                &right.payload,
                "",
                &mut fields,
            );
            if !fields.is_empty() {
                let candidate_refs = store_sync_conflict_candidates(
                    kind,
                    logical_key,
                    [Some(&left.payload), Some(&right.payload)],
                    conflict_candidates,
                )?;
                push_sync_conflict(
                    conflicts,
                    json!({
                        "kind": kind,
                        "logical_key": logical_key,
                        "conflict": "concurrent_edit",
                        "fields": fields,
                        "candidate_refs": candidate_refs,
                    }),
                );
            }
            Ok(Some(SyncObjectRow { payload }))
        }
        (Some(value), None) | (None, Some(value)) => {
            if left_changed != right_changed {
                return Ok(if left_changed {
                    left.cloned()
                } else {
                    right.cloned()
                });
            }
            if !left_changed {
                return Ok(Some(value.clone()));
            }
            let candidate_refs = store_sync_conflict_candidates(
                kind,
                logical_key,
                [left.map(|row| &row.payload), right.map(|row| &row.payload)],
                conflict_candidates,
            )?;
            push_sync_conflict(
                conflicts,
                json!({
                    "kind": kind,
                    "logical_key": logical_key,
                    "conflict": "delete_vs_edit",
                    "candidate_refs": candidate_refs,
                        "fields": [{
                        "path": "$object",
                        "base": common_sync_base(base_left, base_right)
                            .map(|row| bounded_sync_value(&row.payload)),
                        "candidates": [Value::Null, bounded_sync_value(&value.payload)],
                    }],
                }),
            );
            Ok(Some(value.clone()))
        }
        (None, None) => Ok(None),
    }
}

fn push_sync_conflict(conflicts: &mut Vec<Value>, mut conflict: Value) {
    let conflict_id = hash_content(&canonical_sync_value(&conflict));
    conflict["conflict_id"] = Value::String(conflict_id);
    conflicts.push(conflict);
}

pub(crate) fn next_sync_conflict_batch(conflicts: &[Value]) -> Vec<Value> {
    const MAX_CONFLICTS: usize = 20;
    const MAX_BYTES: usize = 256 * 1024;
    let mut batch = Vec::new();
    let mut bytes = 2_usize;
    for conflict in conflicts.iter().take(MAX_CONFLICTS) {
        let encoded_bytes = canonical_sync_value(conflict).len() + usize::from(!batch.is_empty());
        if !batch.is_empty() && bytes + encoded_bytes > MAX_BYTES {
            break;
        }
        bytes += encoded_bytes;
        batch.push(conflict.clone());
    }
    batch
}

fn is_mechanical_sync_kind(kind: &str) -> bool {
    matches!(
        kind,
        "meta"
            | "source"
            | "tag"
            | "ingest"
            | "retrieval_weight"
            | "retrieval_feedback"
            | "semantic_relation"
            | "work_audit"
            | "draft_intent"
    )
}

fn required_sync_draft_blobs(logical_key: &str, payload: &Value) -> Result<BTreeSet<String>> {
    let object = payload.as_object().ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            format!("normalized draft_intent '{logical_key}' must be an object"),
        )
    })?;
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["intent", "origin_store_id"])
    {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("normalized draft_intent '{logical_key}' has unknown or missing fields"),
        ));
    }
    let origin = payload["origin_store_id"].as_str().ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            format!("normalized draft_intent '{logical_key}' origin is missing"),
        )
    })?;
    if !is_lower_sync_hex(origin, 64) {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("normalized draft_intent '{logical_key}' origin is invalid"),
        ));
    }
    let intent: DetachedChangesetIntent = serde_json::from_value(payload["intent"].clone())
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    let canonical = serde_json::to_value(&intent)
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    if canonical != payload["intent"]
        || intent.version != 1
        || logical_key != format!("{origin}\0{}", intent.origin_changeset_id)
    {
        return Err(AppError::new(
            "sync_state_invalid",
            format!("normalized draft_intent '{logical_key}' is not canonical"),
        ));
    }
    let mut hashes = BTreeSet::new();
    for source in intent.sources {
        if !is_lower_sync_hex(&source.content_hash, 64) {
            return Err(AppError::new(
                "sync_state_invalid",
                format!("normalized draft_intent '{logical_key}' source hash is invalid"),
            ));
        }
        if source.content_required {
            hashes.insert(source.content_hash);
        }
    }
    Ok(hashes)
}

fn is_lower_sync_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn merge_mechanical_sync_object(
    base_left: Option<&SyncObjectRow>,
    left: Option<&SyncObjectRow>,
    base_right: Option<&SyncObjectRow>,
    right: Option<&SyncObjectRow>,
) -> Option<SyncObjectRow> {
    match (left, right) {
        (Some(left), Some(right)) => {
            let mut discarded_conflicts = Vec::new();
            Some(SyncObjectRow {
                payload: merge_sync_value_directional(
                    base_left.map(|row| &row.payload),
                    &left.payload,
                    base_right.map(|row| &row.payload),
                    &right.payload,
                    "",
                    &mut discarded_conflicts,
                ),
            })
        }
        (Some(value), None) | (None, Some(value)) => Some(value.clone()),
        (None, None) => None,
    }
}

fn common_sync_base<'a>(
    left: Option<&'a SyncObjectRow>,
    right: Option<&'a SyncObjectRow>,
) -> Option<&'a SyncObjectRow> {
    (left == right).then_some(left).flatten()
}

fn store_sync_conflict_candidates(
    kind: &str,
    logical_key: &str,
    candidates: [Option<&Value>; 2],
    blobs: &mut BTreeMap<String, Vec<u8>>,
) -> Result<Vec<String>> {
    let mut refs = Vec::new();
    for candidate in candidates {
        let encoded = canonical_sync_value(candidate.unwrap_or(&Value::Null));
        let reference = format!("sync-candidate:{}", hash_content(&encoded));
        if let Some(existing) = blobs.get(&reference) {
            if existing != encoded.as_bytes() {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    format!("conflict candidate hash collision for {kind}:{logical_key}"),
                ));
            }
        } else {
            blobs.insert(reference.clone(), encoded.into_bytes());
        }
        refs.push(reference);
    }
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn merge_sync_value_directional(
    base_left: Option<&Value>,
    left: &Value,
    base_right: Option<&Value>,
    right: &Value,
    path: &str,
    conflicts: &mut Vec<Value>,
) -> Value {
    if left == right {
        return left.clone();
    }
    let left_changed = base_left != Some(left);
    let right_changed = base_right != Some(right);
    if !left_changed && !right_changed {
        return left.clone();
    }
    if base_left == base_right {
        if !left_changed && right_changed {
            return right.clone();
        }
        if left_changed && !right_changed {
            return left.clone();
        }
    }
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let base_left = base_left.and_then(Value::as_object);
            let base_right = base_right.and_then(Value::as_object);
            let keys = left
                .keys()
                .chain(right.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut merged = serde_json::Map::new();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (left.get(&key), right.get(&key)) {
                    (Some(left), Some(right)) => {
                        merged.insert(
                            key.clone(),
                            merge_sync_value_directional(
                                base_left.and_then(|value| value.get(&key)),
                                left,
                                base_right.and_then(|value| value.get(&key)),
                                right,
                                &child_path,
                                conflicts,
                            ),
                        );
                    }
                    (Some(value), None) | (None, Some(value)) => {
                        let left_value = left.get(&key);
                        let right_value = right.get(&key);
                        let left_base = base_left.and_then(|base| base.get(&key));
                        let right_base = base_right.and_then(|base| base.get(&key));
                        let left_changed = left_value != left_base;
                        let right_changed = right_value != right_base;
                        if left_changed && !right_changed {
                            if let Some(value) = left_value {
                                merged.insert(key, value.clone());
                            }
                            continue;
                        }
                        if !left_changed && right_changed {
                            if let Some(value) = right_value {
                                merged.insert(key, value.clone());
                            }
                            continue;
                        }
                        if !left_changed && !right_changed {
                            merged.insert(key, value.clone());
                            continue;
                        }
                        let base_value = (left_base == right_base).then_some(left_base).flatten();
                        record_sync_field_conflict(
                            &child_path,
                            base_value,
                            value,
                            &Value::Null,
                            conflicts,
                        );
                        merged.insert(key, value.clone());
                    }
                    (None, None) => {}
                }
            }
            Value::Object(merged)
        }
        (Value::Array(left), Value::Array(right)) => {
            let mut nested = Vec::new();
            let merged = merge_sync_arrays_directional(
                base_left,
                left,
                base_right,
                right,
                path,
                &mut nested,
            );
            if !nested.is_empty() {
                record_sync_field_conflict(
                    path,
                    (base_left == base_right).then_some(base_left).flatten(),
                    &Value::Array(left.clone()),
                    &Value::Array(right.clone()),
                    conflicts,
                );
            }
            merged
        }
        _ if path.ends_with("updated_at") || path.ends_with("recorded_at") => {
            if canonical_sync_value(left) >= canonical_sync_value(right) {
                left.clone()
            } else {
                right.clone()
            }
        }
        _ if path.ends_with("created_at") => {
            if canonical_sync_value(left) <= canonical_sync_value(right) {
                left.clone()
            } else {
                right.clone()
            }
        }
        _ if path.ends_with("revision") && left.is_number() && right.is_number() => {
            if left.as_i64().unwrap_or_default() >= right.as_i64().unwrap_or_default() {
                left.clone()
            } else {
                right.clone()
            }
        }
        _ if !left_changed && right_changed => right.clone(),
        _ if left_changed && !right_changed => left.clone(),
        _ => {
            if left_changed && right_changed {
                record_sync_field_conflict(
                    path,
                    (base_left == base_right).then_some(base_left).flatten(),
                    left,
                    right,
                    conflicts,
                );
            }
            deterministic_sync_value(left, right)
        }
    }
}

fn merge_sync_arrays_directional(
    base_left: Option<&Value>,
    left: &[Value],
    base_right: Option<&Value>,
    right: &[Value],
    path: &str,
    conflicts: &mut Vec<Value>,
) -> Value {
    if left.iter().chain(right).all(Value::is_string) {
        let base_left_members = base_left
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let base_right_members = base_right
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let left = left
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let right = right
            .iter()
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let members = base_left_members
            .iter()
            .chain(base_right_members.iter())
            .chain(left.iter())
            .chain(right.iter())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut membership_conflict = false;
        let values = members
            .into_iter()
            .filter(|value| {
                let base_left_has = base_left_members.contains(value);
                let base_right_has = base_right_members.contains(value);
                let left_has = left.contains(value);
                let right_has = right.contains(value);
                let left_changed = left_has != base_left_has;
                let right_changed = right_has != base_right_has;
                if left_has == right_has || (left_changed && !right_changed) {
                    left_has
                } else if !left_changed && right_changed {
                    right_has
                } else if !left_changed {
                    left_has
                } else {
                    membership_conflict = true;
                    left_has || right_has
                }
            })
            .map(str::to_owned)
            .map(Value::String)
            .collect::<Vec<_>>();
        if membership_conflict {
            record_sync_field_conflict(
                path,
                (base_left == base_right).then_some(base_left).flatten(),
                &Value::Array(
                    left.iter()
                        .map(|value| Value::String((*value).to_owned()))
                        .collect(),
                ),
                &Value::Array(
                    right
                        .iter()
                        .map(|value| Value::String((*value).to_owned()))
                        .collect(),
                ),
                conflicts,
            );
        }
        return Value::Array(values);
    }
    let identity = |value: &Value| {
        value
            .get("id")
            .or_else(|| value.get("revision"))
            .or_else(|| value.get("slug"))
            .map(canonical_sync_value)
            .unwrap_or_else(|| canonical_sync_value(value))
    };
    let to_map = |values: &[Value]| {
        values
            .iter()
            .map(|value| (identity(value), value.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let base_left_map: BTreeMap<String, Value> = base_left
        .and_then(Value::as_array)
        .map(|values| to_map(values))
        .unwrap_or_default();
    let base_right_map: BTreeMap<String, Value> = base_right
        .and_then(Value::as_array)
        .map(|values| to_map(values))
        .unwrap_or_default();
    let left_map = to_map(left);
    let right_map = to_map(right);
    let keys = left_map
        .keys()
        .chain(right_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut merged = Vec::new();
    for key in keys {
        let base_left = base_left_map.get(&key);
        let left = left_map.get(&key);
        let base_right = base_right_map.get(&key);
        let right = right_map.get(&key);
        if left == right {
            if let Some(value) = left {
                merged.push(value.clone());
            }
            continue;
        }
        let left_changed = left != base_left;
        let right_changed = right != base_right;
        if left_changed && !right_changed {
            if let Some(value) = left {
                merged.push(value.clone());
            }
        } else if !left_changed && right_changed {
            if let Some(value) = right {
                merged.push(value.clone());
            }
        } else if !left_changed {
            if let Some(value) = left.or(right) {
                merged.push(value.clone());
            }
        } else if let (Some(left), Some(right)) = (left, right) {
            merged.push(merge_sync_value_directional(
                base_left,
                left,
                base_right,
                right,
                &format!("{path}[{key}]"),
                conflicts,
            ));
        } else {
            let value = left.or(right).expect("one directional array value");
            record_sync_field_conflict(
                &format!("{path}[{key}]"),
                (base_left == base_right).then_some(base_left).flatten(),
                left.unwrap_or(&Value::Null),
                right.unwrap_or(&Value::Null),
                conflicts,
            );
            merged.push(value.clone());
        }
    }
    Value::Array(merged)
}

#[cfg(test)]
fn merge_sync_arrays(
    base: Option<&Value>,
    left: &[Value],
    right: &[Value],
    path: &str,
    conflicts: &mut Vec<Value>,
) -> Value {
    merge_sync_arrays_directional(base, left, base, right, path, conflicts)
}

fn record_sync_field_conflict(
    path: &str,
    base: Option<&Value>,
    left: &Value,
    right: &Value,
    conflicts: &mut Vec<Value>,
) {
    let mut candidates = vec![bounded_sync_value(left), bounded_sync_value(right)];
    candidates.sort_by_key(canonical_sync_value);
    candidates.dedup();
    conflicts.push(json!({
        "path": path,
        "base": base.map(bounded_sync_value),
        "candidates": candidates,
    }));
}

fn deterministic_sync_value(left: &Value, right: &Value) -> Value {
    if canonical_sync_value(left) <= canonical_sync_value(right) {
        left.clone()
    } else {
        right.clone()
    }
}

fn bounded_sync_value(value: &Value) -> Value {
    let encoded = canonical_sync_value(value);
    if encoded.len() <= 16 * 1024 {
        value.clone()
    } else {
        json!({"sha256": hash_content(&encoded), "bytes": encoded.len(), "truncated": true})
    }
}

fn canonical_sync_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn merge_sync_blobs_into(
    tx: &Transaction<'_>,
    paths: &[&Path],
    wanted: &BTreeSet<String>,
) -> Result<()> {
    for (index, path) in paths.iter().enumerate() {
        let alias = format!("sync_merge_{index}");
        let sql = format!("ATTACH DATABASE ?1 AS {alias}");
        tx.execute(&sql, [path.to_string_lossy().into_owned()])?;
    }
    for hash in wanted {
        let mut found = false;
        for index in 0..paths.len() {
            let alias = format!("sync_merge_{index}");
            let insert = format!(
                "INSERT OR IGNORE INTO sync_blobs(content_hash,content)
                 SELECT content_hash,content FROM {alias}.sync_blobs WHERE content_hash=?1"
            );
            tx.execute(&insert, [hash])?;
            let matches = format!(
                "SELECT NOT EXISTS(
                     SELECT 1 FROM {alias}.sync_blobs source
                     JOIN main.sync_blobs target USING(content_hash)
                     WHERE source.content_hash=?1 AND source.content<>target.content
                 )"
            );
            let equal: bool = tx.query_row(&matches, [hash], |row| row.get(0))?;
            if !equal {
                return Err(AppError::new(
                    "sync_checksum_mismatch",
                    format!("content-addressed blob {hash} differs between replicas"),
                ));
            }
            let exists =
                format!("SELECT EXISTS(SELECT 1 FROM {alias}.sync_blobs WHERE content_hash=?1)");
            found |= tx.query_row(&exists, [hash], |row| row.get::<_, bool>(0))?;
        }
        if !found {
            return Err(AppError::new(
                "sync_state_invalid",
                format!("merged source {hash} has no content blob"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn create_empty_sync_state(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(AppError::new(
            "sync_state_exists",
            "empty Sync state already exists",
        ));
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         CREATE TABLE sync_manifest(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
         CREATE TABLE sync_objects(
             kind TEXT NOT NULL,logical_key TEXT NOT NULL,payload_json TEXT NOT NULL,
             payload_hash TEXT NOT NULL,PRIMARY KEY(kind,logical_key)
         ) WITHOUT ROWID;
         CREATE TABLE sync_blobs(
             content_hash TEXT NOT NULL PRIMARY KEY,content BLOB NOT NULL
         );",
    )?;
    conn.execute(
        "INSERT INTO sync_manifest(key,value) VALUES('format',?1)",
        [SYNC_STATE_FORMAT.to_string()],
    )?;
    conn.execute(
        "INSERT INTO sync_manifest(key,value) VALUES('store_format',?1)",
        [USER_VERSION.to_string()],
    )?;
    Ok(())
}

pub(crate) fn resolve_sync_conflicts(
    merged: &Path,
    conflicts: &[Value],
    resolution: &Value,
) -> Result<String> {
    validate_sync_resolution_schema(conflicts, resolution)?;
    if resolution["version"] != 1 {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution packet version must be 1",
        ));
    }
    let decisions = resolution["decisions"].as_array().ok_or_else(|| {
        AppError::new(
            "sync_resolution_invalid",
            "resolution decisions must be an array",
        )
    })?;
    let expected_ids = conflicts
        .iter()
        .map(|conflict| {
            let kind = conflict["kind"]
                .as_str()
                .ok_or_else(|| AppError::new("sync_state_invalid", "conflict kind is missing"))?;
            let key = conflict["logical_key"].as_str().ok_or_else(|| {
                AppError::new("sync_state_invalid", "conflict logical key is missing")
            })?;
            let conflict_id = conflict["conflict_id"]
                .as_str()
                .ok_or_else(|| AppError::new("sync_state_invalid", "conflict ID is missing"))?;
            Ok(((kind.to_owned(), key.to_owned()), conflict_id.to_owned()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut indexed = BTreeMap::new();
    let mut preserve_both = BTreeSet::new();
    for decision in decisions {
        let kind = decision["kind"]
            .as_str()
            .ok_or_else(|| AppError::new("sync_resolution_invalid", "decision kind is required"))?;
        let key = decision["logical_key"].as_str().ok_or_else(|| {
            AppError::new(
                "sync_resolution_invalid",
                "decision logical_key is required",
            )
        })?;
        let conflict_id = decision["conflict_id"].as_str().ok_or_else(|| {
            AppError::new(
                "sync_resolution_stale",
                "resolution decision conflict_id is required",
            )
        })?;
        if expected_ids
            .get(&(kind.to_owned(), key.to_owned()))
            .map(String::as_str)
            != Some(conflict_id)
        {
            return Err(AppError::new(
                "sync_resolution_stale",
                format!("resolution conflict ID is stale or unknown for {kind}:{key}"),
            ));
        }
        if decision["strategy"] == "preserve_both" {
            if !preserve_both.insert((kind.to_string(), key.to_string())) {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "preserve-both decisions must be unique per object",
                ));
            }
            continue;
        }
        let path = decision["path"]
            .as_str()
            .ok_or_else(|| AppError::new("sync_resolution_invalid", "decision path is required"))?;
        let candidate = decision["candidate"].as_u64().ok_or_else(|| {
            AppError::new(
                "sync_resolution_invalid",
                "decision candidate must be 0 or 1",
            )
        })?;
        if candidate > 1
            || indexed
                .insert(
                    (kind.to_string(), key.to_string(), path.to_string()),
                    candidate as usize,
                )
                .is_some()
        {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "resolution decisions must be unique and choose candidate 0 or 1",
            ));
        }
    }
    let expected = conflicts
        .iter()
        .map(|conflict| conflict["fields"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    let preserved_fields = conflicts
        .iter()
        .filter(|conflict| {
            conflict["kind"]
                .as_str()
                .zip(conflict["logical_key"].as_str())
                .is_some_and(|(kind, key)| {
                    preserve_both.contains(&(kind.to_owned(), key.to_owned()))
                })
        })
        .map(|conflict| conflict["fields"].as_array().map_or(0, Vec::len))
        .sum::<usize>();
    if indexed.len() + preserved_fields != expected {
        return Err(AppError::new(
            "sync_resolution_incomplete",
            format!(
                "resolution covers {} conflict fields; {expected} are required",
                indexed.len() + preserved_fields
            ),
        ));
    }
    let mut conn = Connection::open(merged)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for conflict in conflicts {
        let kind = conflict["kind"]
            .as_str()
            .ok_or_else(|| AppError::new("sync_state_invalid", "conflict kind is missing"))?;
        let key = conflict["logical_key"].as_str().ok_or_else(|| {
            AppError::new("sync_state_invalid", "conflict logical key is missing")
        })?;
        if preserve_both.remove(&(kind.to_string(), key.to_string())) {
            apply_sync_preserve_both(&tx, kind, key, conflict)?;
            continue;
        }
        let mut payload: Option<Value> = tx
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind=?1 AND logical_key=?2",
                params![kind, key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|encoded| serde_json::from_str(&encoded))
            .transpose()
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        for field in conflict["fields"]
            .as_array()
            .ok_or_else(|| AppError::new("sync_state_invalid", "conflict fields are missing"))?
        {
            let path = field["path"]
                .as_str()
                .ok_or_else(|| AppError::new("sync_state_invalid", "conflict path is missing"))?;
            let index = indexed
                .remove(&(kind.to_string(), key.to_string(), path.to_string()))
                .ok_or_else(|| {
                    AppError::new(
                        "sync_resolution_incomplete",
                        format!("missing resolution for {kind}:{key}:{path}"),
                    )
                })?;
            let value = field["candidates"]
                .as_array()
                .and_then(|values| values.get(index))
                .ok_or_else(|| {
                    AppError::new(
                        "sync_resolution_invalid",
                        "selected conflict candidate is unavailable",
                    )
                })?;
            if value.get("truncated") == Some(&Value::Bool(true)) {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "a truncated candidate cannot be selected directly",
                ));
            }
            if path == "$object" {
                payload = if value.is_null() {
                    None
                } else {
                    Some(value.clone())
                };
            } else {
                set_sync_json_path(
                    payload.as_mut().ok_or_else(|| {
                        AppError::new(
                            "sync_resolution_invalid",
                            "cannot set a field on a deleted object",
                        )
                    })?,
                    path,
                    value.clone(),
                )?;
            }
        }
        tx.execute(
            "DELETE FROM sync_objects WHERE kind=?1 AND logical_key=?2",
            params![kind, key],
        )?;
        if let Some(payload) = payload {
            insert_sync_object(&tx, kind, key, &payload)?;
        }
    }
    if !indexed.is_empty() {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution contains decisions that are not in the conflict packet",
        ));
    }
    if !preserve_both.is_empty() {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "preserve-both decision does not match the conflict packet",
        ));
    }
    tx.commit()?;
    sync_state_digest(merged)
}

fn validate_sync_resolution_schema(conflicts: &[Value], resolution: &Value) -> Result<()> {
    const MAX_CONFLICTS: usize = 20;
    const MAX_BYTES: usize = 256 * 1024;
    if conflicts.len() > MAX_CONFLICTS || canonical_sync_value(resolution).len() > MAX_BYTES {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "Sync resolution exceeds the bounded conflict batch",
        ));
    }
    let object = resolution
        .as_object()
        .ok_or_else(|| AppError::new("sync_resolution_invalid", "resolution must be an object"))?;
    let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if keys != BTreeSet::from(["decisions", "version"]) {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution contains unknown or missing fields",
        ));
    }
    let decisions = object["decisions"].as_array().ok_or_else(|| {
        AppError::new(
            "sync_resolution_invalid",
            "resolution decisions must be an array",
        )
    })?;
    let mut decision_conflicts = BTreeSet::new();
    for decision in decisions {
        let decision = decision.as_object().ok_or_else(|| {
            AppError::new("sync_resolution_invalid", "decision must be an object")
        })?;
        let keys = decision.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let candidate = BTreeSet::from(["candidate", "conflict_id", "kind", "logical_key", "path"]);
        let preserve = BTreeSet::from(["conflict_id", "kind", "logical_key", "strategy"]);
        if keys != candidate && keys != preserve {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "decision must be exactly one candidate or preserve-both shape",
            ));
        }
        if keys == preserve && decision["strategy"] != "preserve_both" {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "the only supported resolution strategy is preserve_both",
            ));
        }
        for field in ["kind", "logical_key"] {
            let value = decision[field].as_str().ok_or_else(|| {
                AppError::new(
                    "sync_resolution_invalid",
                    format!("decision {field} is required"),
                )
            })?;
            if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    format!("decision {field} is invalid"),
                ));
            }
        }
        let conflict_id = decision["conflict_id"].as_str().ok_or_else(|| {
            AppError::new(
                "sync_resolution_invalid",
                "decision conflict_id is required",
            )
        })?;
        if conflict_id.len() != 64 || !conflict_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "decision conflict_id must be a 64-character hexadecimal digest",
            ));
        }
        decision_conflicts.insert(conflict_id);
        if decision_conflicts.len() > MAX_CONFLICTS {
            return Err(AppError::new(
                "sync_resolution_invalid",
                "resolution contains more than 20 conflict IDs",
            ));
        }
        if keys == candidate {
            let path = decision["path"].as_str().ok_or_else(|| {
                AppError::new("sync_resolution_invalid", "decision path is required")
            })?;
            if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
                return Err(AppError::new(
                    "sync_resolution_invalid",
                    "decision path is invalid",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn cleanup_sync_conflict_candidates(merged: &Path) -> Result<String> {
    let conn = Connection::open(merged)?;
    conn.execute(
        "DELETE FROM sync_blobs WHERE content_hash LIKE 'sync-candidate:%'",
        [],
    )?;
    sync_state_digest(merged)
}

fn apply_sync_preserve_both(
    tx: &Transaction<'_>,
    kind: &str,
    logical_key: &str,
    conflict: &Value,
) -> Result<()> {
    let refs = conflict["candidate_refs"].as_array().ok_or_else(|| {
        AppError::new(
            "sync_state_invalid",
            "preserve-both candidate refs are missing",
        )
    })?;
    let mut candidates = Vec::new();
    for reference in refs {
        let reference = reference.as_str().ok_or_else(|| {
            AppError::new(
                "sync_state_invalid",
                "preserve-both candidate ref is invalid",
            )
        })?;
        let encoded: Option<Vec<u8>> = tx
            .query_row(
                "SELECT content FROM sync_blobs WHERE content_hash=?1",
                [reference],
                |row| row.get(0),
            )
            .optional()?;
        let Some(encoded) = encoded else { continue };
        let payload: Value = serde_json::from_slice(&encoded)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        candidates.push((hash_content(&canonical_sync_value(&payload)), payload));
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    candidates.dedup_by(|left, right| left.0 == right.0);
    if candidates.len() != 2 {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "preserve-both requires complete object candidates",
        ));
    }

    let complete = candidates
        .iter()
        .filter(|(_, value)| !value.is_null())
        .collect::<Vec<_>>();
    if !matches!(complete.len(), 1 | 2) {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "preserve-both has no complete object candidate",
        ));
    }
    let (losing_hash, losing) = complete.last().copied().unwrap();
    let (variant_key, variant, variant_exists) = available_sync_conflict_variant(
        tx,
        kind,
        logical_key,
        losing_hash,
        losing,
    )?;
    let delete_vs_edit = conflict["conflict"] == "delete_vs_edit";
    let original_matches = if delete_vs_edit {
        !sync_object_exists(tx, kind, logical_key)?
    } else {
        sync_object_payload_equals(tx, kind, logical_key, &complete[0].1)?
    };
    if variant_exists && original_matches {
        return Ok(());
    }

    tx.execute(
        "DELETE FROM sync_objects WHERE kind=?1 AND logical_key=?2",
        params![kind, logical_key],
    )?;
    if complete.len() == 2 {
        insert_sync_object(tx, kind, logical_key, &complete[0].1)?;
    }
    if !variant_exists {
        insert_sync_object(tx, kind, &variant_key, &variant)?;
    }
    if delete_vs_edit {
        remap_sync_conflict_references(tx, kind, logical_key, &variant_key)?;
    }
    if kind == "page" {
        label_sync_conflict_page(tx, &variant_key, &variant)?;
    }
    Ok(())
}

fn remap_sync_conflict_references(
    tx: &Transaction<'_>,
    kind: &str,
    original: &str,
    variant: &str,
) -> Result<()> {
    let referenced_kind = match kind {
        "todo" => "todo",
        "page" => "tag",
        "memory" => "memory",
        _ => return Ok(()),
    };
    let rows = {
        let mut statement = tx.prepare(
            "SELECT logical_key,payload_json FROM sync_objects
             WHERE kind=?1 ORDER BY logical_key",
        )?;
        statement
            .query_map([referenced_kind], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (key, encoded) in rows {
        let mut payload: Value = serde_json::from_str(&encoded)
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
        let mut changed = false;
        match kind {
            "todo" => {
                if payload.get("parent_id").and_then(Value::as_str) == Some(original) {
                    payload["parent_id"] = Value::String(variant.to_owned());
                    changed = true;
                }
            }
            "page" => {
                for page in payload
                    .get_mut("pages")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if page.get("slug").and_then(Value::as_str) == Some(original) {
                        page["slug"] = Value::String(variant.to_owned());
                        changed = true;
                    }
                }
            }
            "memory" => {
                for relation in payload
                    .get_mut("relations")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if relation.get("target").and_then(Value::as_str) == Some(original) {
                        relation["target"] = Value::String(variant.to_owned());
                        changed = true;
                    }
                }
            }
            _ => {}
        }
        if changed {
            tx.execute(
                "DELETE FROM sync_objects WHERE kind=?1 AND logical_key=?2",
                params![referenced_kind, key],
            )?;
            insert_sync_object(tx, referenced_kind, &key, &payload)?;
        }
    }
    Ok(())
}

fn label_sync_conflict_page(
    tx: &Transaction<'_>,
    variant_key: &str,
    variant: &Value,
) -> Result<()> {
    let created_at = variant
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::new("sync_state_invalid", "Page conflict lacks created_at"))?;
    let updated_at = variant
        .get("updated_at")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::new("sync_state_invalid", "Page conflict lacks updated_at"))?;
    let mut tag: Value = tx
        .query_row(
            "SELECT payload_json FROM sync_objects WHERE kind='tag' AND logical_key='sync-conflict'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|encoded| serde_json::from_str(&encoded))
        .transpose()
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?
        .unwrap_or_else(|| json!({
            "name": "sync-conflict",
            "autoload": false,
            "autoload_priority": 0,
            "autoload_limit": 10,
            "autoload_max_chars": 50000,
            "reason": "Deterministic Sync conflict variants",
            "updated_at": updated_at,
            "pages": [],
        }));
    let pages = tag
        .get_mut("pages")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            AppError::new("sync_state_invalid", "sync-conflict tag pages are invalid")
        })?;
    if !pages.iter().any(|page| page["slug"] == variant_key) {
        pages.push(json!({
            "slug": variant_key,
            "priority": 0,
            "reason": "Preserved by deterministic Sync conflict handling",
            "created_at": created_at,
            "updated_at": updated_at,
        }));
        pages.sort_by_key(|page| canonical_sync_value(&page["slug"]));
    }
    tx.execute(
        "DELETE FROM sync_objects WHERE kind='tag' AND logical_key='sync-conflict'",
        [],
    )?;
    insert_sync_object(tx, "tag", "sync-conflict", &tag)
}

fn sync_object_exists(
    tx: &Transaction<'_>,
    kind: &str,
    logical_key: &str,
) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_objects WHERE kind=?1 AND logical_key=?2)",
        params![kind, logical_key],
        |row| row.get(0),
    )?)
}

fn sync_object_payload_equals(
    tx: &Transaction<'_>,
    kind: &str,
    logical_key: &str,
    expected: &Value,
) -> Result<bool> {
    let encoded = tx
        .query_row(
            "SELECT payload_json FROM sync_objects WHERE kind=?1 AND logical_key=?2",
            params![kind, logical_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let actual = encoded
        .map(|encoded| serde_json::from_str::<Value>(&encoded))
        .transpose()
        .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?;
    Ok(actual.as_ref() == Some(expected))
}

fn available_sync_conflict_variant(
    tx: &Transaction<'_>,
    kind: &str,
    logical_key: &str,
    losing_hash: &str,
    payload: &Value,
) -> Result<(String, Value, bool)> {
    const MAX_VARIANT_ATTEMPTS: usize = 4_096;
    for attempt in 0..MAX_VARIANT_ATTEMPTS {
        let variant_key = sync_conflict_variant_key_at(kind, logical_key, losing_hash, attempt)?;
        let variant = sync_conflict_variant_for_key(kind, logical_key, &variant_key, payload)?;
        if sync_object_payload_equals(tx, kind, &variant_key, &variant)? {
            return Ok((variant_key, variant, true));
        }
        if !sync_object_exists(tx, kind, &variant_key)? {
            return Ok((variant_key, variant, false));
        }
    }
    Err(AppError::new(
        "sync_resolution_invalid",
        "Sync conflict variant key space is exhausted",
    ))
}

#[cfg(test)]
fn sync_conflict_variant(
    kind: &str,
    logical_key: &str,
    losing_hash: &str,
    payload: &Value,
) -> Result<(String, Value)> {
    let variant_key = sync_conflict_variant_key(kind, logical_key, losing_hash)?;
    let variant = sync_conflict_variant_for_key(kind, logical_key, &variant_key, payload)?;
    Ok((variant_key, variant))
}

fn sync_conflict_variant_for_key(
    kind: &str,
    logical_key: &str,
    variant_key: &str,
    payload: &Value,
) -> Result<Value> {
    let mut variant = payload.clone();
    let id_field = if kind == "page" { "slug" } else { "id" };
    variant
        .as_object_mut()
        .ok_or_else(|| AppError::new("sync_state_invalid", "conflict candidate is not an object"))?
        .insert(id_field.to_owned(), Value::String(variant_key.to_owned()));
    if matches!(kind, "todo" | "plan") {
        let tags = variant
            .get_mut("tags")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                AppError::new("sync_state_invalid", "conflict candidate tags are missing")
            })?;
        tags.push(Value::String("sync-conflict".to_owned()));
        tags.sort_by_key(canonical_sync_value);
        tags.dedup();
    } else if kind == "memory" {
        refresh_sync_memory_variant(&mut variant, logical_key)?;
    }
    Ok(variant)
}

fn refresh_sync_memory_variant(variant: &mut Value, original_id: &str) -> Result<()> {
    let evidence = variant
        .get_mut("evidence")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            AppError::new("sync_state_invalid", "Memory conflict evidence is missing")
        })?;
    let reference = format!("lwc:sync-conflict:{original_id}");
    if !evidence.iter().any(|item| item["reference"] == reference) {
        evidence.push(json!({"reference": reference, "excerpt": null}));
        evidence.sort_by_key(|item| canonical_sync_value(&item["reference"]));
    }
    let input = MemoryEventInput {
        request_id: None,
        event_type: required_str(variant, "type")?.to_owned(),
        context: required_str(variant, "context")?.to_owned(),
        occurred_at: Some(required_str(variant, "occurred_at")?.to_owned()),
        valid_from: optional_str(variant, "valid_from")?.map(str::to_owned),
        valid_to: optional_str(variant, "valid_to")?.map(str::to_owned),
        pinned: required_bool(variant, "pinned")?,
        observed: owned_string_array(variant, "observed")?,
        decision: owned_string_array(variant, "decision")?,
        constraints: owned_string_array(variant, "constraints")?,
        learned: owned_string_array(variant, "learned")?,
        unresolved: owned_string_array(variant, "unresolved")?,
        outcome: owned_string_array(variant, "outcome")?,
        changes: serde_json::from_value(variant["changes"].clone())
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
        evidence: serde_json::from_value(variant["evidence"].clone())
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
        relations: serde_json::from_value(variant["relations"].clone())
            .map_err(|error| AppError::new("sync_state_invalid", error.to_string()))?,
    };
    variant["fingerprint"] = Value::String(memory_fingerprint(&input)?);
    variant["logical_bytes"] = Value::Number(memory_logical_bytes(&input)?.into());
    Ok(())
}

#[cfg(test)]
fn sync_conflict_variant_key(kind: &str, logical_key: &str, losing_hash: &str) -> Result<String> {
    sync_conflict_variant_key_at(kind, logical_key, losing_hash, 0)
}

fn sync_conflict_variant_key_at(
    kind: &str,
    logical_key: &str,
    losing_hash: &str,
    attempt: usize,
) -> Result<String> {
    match kind {
        "page" => {
            let base = format!("{logical_key}--sync-{}", &losing_hash[..12]);
            Ok(if attempt == 0 {
                base
            } else {
                format!("{base}-{}", attempt + 1)
            })
        }
        "todo" | "plan" | "memory" => {
            let identity = if attempt == 0 {
                format!("sync-conflict\0{kind}\0{logical_key}\0{losing_hash}")
            } else {
                format!("sync-conflict\0{kind}\0{logical_key}\0{losing_hash}\0{attempt}")
            };
            Ok(hash_content(&identity)[..32].to_owned())
        }
        _ => Err(AppError::new(
            "sync_resolution_invalid",
            format!("preserve-both is unsupported for {kind}"),
        )),
    }
}

fn set_sync_json_path(root: &mut Value, path: &str, value: Value) -> Result<()> {
    if path.is_empty() || path.contains('[') {
        return Err(AppError::new(
            "sync_resolution_invalid",
            "resolution currently requires an object-field path",
        ));
    }
    let mut current = root;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        let object = current.as_object_mut().ok_or_else(|| {
            AppError::new(
                "sync_resolution_invalid",
                "resolution path is not an object field",
            )
        })?;
        if parts.peek().is_none() {
            object.insert(part.to_string(), value);
            return Ok(());
        }
        current = object.get_mut(part).ok_or_else(|| {
            AppError::new("sync_resolution_invalid", "resolution path does not exist")
        })?;
    }
    Err(AppError::new(
        "sync_resolution_invalid",
        "resolution path is empty",
    ))
}
