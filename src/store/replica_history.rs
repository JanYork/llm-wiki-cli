// Portable provenance is data, never a local filesystem subscription or an
// instruction to replay the recorded operation. IDs retain their origin scope.
fn create_replica_history_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS replica_history(
            kind TEXT NOT NULL CHECK(kind IN ('source_revision','memory_audit')),
            logical_key TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            PRIMARY KEY(kind,logical_key)
        ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn is_core_memory_action(action: &str) -> bool {
    [
        "source_",
        "page_",
        "tag_",
        "todo_",
        "plan_",
        "memory_",
        "retrieval_",
        "graph_relation_",
    ]
    .iter()
    .any(|prefix| action.starts_with(prefix))
        || matches!(
            action,
            "discussion.apply" | "schema_set"
                | "purpose_set"
                | "ingest_complete"
                | "ingest_analyze"
                | "ingest_no_derived_pages"
        )
}

fn portable_history_value(value: &mut Value) {
    match value {
        Value::String(text)
            if Path::new(text).is_absolute() || text.as_bytes().get(1) == Some(&b':') =>
        {
            *text = format!("local-path:{}", hash_content(text));
        }
        Value::Array(items) => items.iter_mut().for_each(portable_history_value),
        Value::Object(fields) => fields.values_mut().for_each(portable_history_value),
        _ => {}
    }
}

impl Store {
    fn export_replica_history(&self, output: &Connection) -> Result<()> {
        let origin: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key='store_id'", [], |r| {
                    r.get(0)
                })?;
        let mut inherited = self.conn.prepare(
            "SELECT kind,logical_key,payload_json FROM replica_history ORDER BY kind,logical_key",
        )?;
        for row in inherited.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (kind, key, raw) = row?;
            let payload: Value = serde_json::from_str(&raw)
                .map_err(|e| AppError::new("corrupt_store", e.to_string()))?;
            insert_replica_history_object(output, &kind, &key, &payload)?;
        }
        let mut paths = self.conn.prepare(
            "SELECT r.tracked_path,r.revision,s.content_hash,r.observed_at
             FROM source_path_revisions r JOIN sources s ON s.id=r.source_id
             ORDER BY r.tracked_path,r.revision",
        )?;
        for row in paths.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (path, revision, source_hash, observed_at) = row?;
            let payload = json!({"origin_store":origin,"path_key":hash_content(&path),
                "revision":revision,"source_hash":source_hash,"observed_at":observed_at});
            let key = hash_content(&canonical_sync_value(&payload));
            insert_replica_history_object(output, "source_revision", &key, &payload)?;
        }
        let mut operations = self.conn.prepare(
            "SELECT id,action,target,detail_json,created_at FROM operations ORDER BY id",
        )?;
        for row in operations.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })? {
            let (id, action, target, detail, at) = row?;
            if !is_core_memory_action(&action) {
                continue;
            }
            let detail: Value = serde_json::from_str(&detail)
                .map_err(|e| AppError::new("corrupt_store", e.to_string()))?;
            let mut payload = json!({"origin_store":origin,"operation_id":id,
                "action":action,"target":target,"detail":detail,"created_at":at});
            portable_history_value(&mut payload);
            let key = hash_content(&canonical_sync_value(&payload));
            insert_replica_history_object(output, "memory_audit", &key, &payload)?;
        }
        Ok(())
    }
}

fn import_replica_history(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    for kind in ["source_revision", "memory_audit"] {
        for (key, payload) in objects_of_kind(state, kind) {
            if hash_content(&canonical_sync_value(payload)) != key {
                return Err(invalid_object(
                    kind,
                    key,
                    "immutable history digest mismatch",
                ));
            }
            if !is_lower_sync_hex(required_str(payload, "origin_store")?, 64) {
                return Err(invalid_object(kind, key, "invalid origin store identity"));
            }
            if kind == "source_revision" {
                if required_i64(payload, "revision")? < 1
                    || !is_lower_sync_hex(required_str(payload, "source_hash")?, 64)
                    || !is_lower_sync_hex(required_str(payload, "path_key")?, 64)
                {
                    return Err(invalid_object(kind, key, "invalid source revision"));
                }
                chrono::DateTime::parse_from_rfc3339(required_str(payload, "observed_at")?)
                    .map_err(|_| invalid_object(kind, key, "invalid timestamp"))?;
            } else {
                if required_i64(payload, "operation_id")? < 1
                    || !is_core_memory_action(required_str(payload, "action")?)
                    || !payload["detail"].is_object()
                {
                    return Err(invalid_object(kind, key, "invalid core memory history"));
                }
                required_str(payload, "target")?;
                chrono::DateTime::parse_from_rfc3339(required_str(payload, "created_at")?)
                    .map_err(|_| invalid_object(kind, key, "invalid timestamp"))?;
            }
            tx.execute(
                "INSERT INTO replica_history(kind,logical_key,payload_json) VALUES(?1,?2,?3)
                 ON CONFLICT(kind,logical_key) DO NOTHING",
                params![kind, key, canonical_sync_value(payload)],
            )?;
        }
    }
    Ok(())
}

fn insert_replica_history_object(
    output: &Connection,
    kind: &str,
    key: &str,
    payload: &Value,
) -> Result<()> {
    if hash_content(&canonical_sync_value(payload)) != key {
        return Err(invalid_object(
            kind,
            key,
            "immutable history digest mismatch",
        ));
    }
    let previous: Option<String> = output
        .query_row(
            "SELECT payload_json FROM sync_objects WHERE kind=?1 AND logical_key=?2",
            params![kind, key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(previous) = previous {
        let previous: Value = serde_json::from_str(&previous)
            .map_err(|e| AppError::new("corrupt_store", e.to_string()))?;
        if previous != *payload {
            return Err(invalid_object(kind, key, "immutable history collision"));
        }
        return Ok(());
    }
    insert_sync_object(output, kind, key, payload)
}
