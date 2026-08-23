fn portable_detached_origin(origin: String) -> Option<String> {
    let bytes = origin.as_bytes();
    let windows_absolute = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    (!Path::new(&origin).is_absolute() && !origin.starts_with(['/', '\\']) && !windows_absolute)
        .then_some(origin)
}

fn changeset_sync_replay_target(origin_store_id: &str, origin_changeset_id: &str) -> String {
    hash_content(&format!("{origin_store_id}\0{origin_changeset_id}"))
}

fn changeset_sync_replay_expected_key(target: &str) -> String {
    format!("changeset_sync_replay_expected:{target}")
}

fn validate_changeset_sync_replay_item(item_key: &str, item_digest: &str) -> Result<()> {
    let valid_key = (1..=512).contains(&item_key.len())
        && item_key.matches('\0').count() == 1
        && matches!(
            item_key.split_once('\0').map(|value| value.0),
            Some("source" | "page" | "meta" | "tag" | "ingest")
        )
        && !item_key
            .chars()
            .any(|character| character.is_control() && character != '\0');
    let valid_digest = item_digest.len() == 64
        && item_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid_key || !valid_digest {
        return Err(AppError::new(
            "changeset_replay_invalid",
            "replay item marker is malformed",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangesetSyncReplayItemState {
    Ready,
    PendingClean,
    PendingMutated,
    Complete,
}

impl Store {
    pub fn initialize(
        scope: impl Into<String>,
        database: impl AsRef<Path>,
    ) -> Result<(Self, bool)> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        configure_connection(&conn)?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        let created = prepare_store(&mut store.conn, true, None)?;
        if created {
            store.record_top_level_operation(
                "init",
                "wiki",
                json!({ "user_version": USER_VERSION, "tokenizer": TOKENIZER_ID }),
            )?;
        }
        store.reconcile_graph_projection()?;
        Ok((store, created))
    }

    pub fn open(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let mut conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection(&conn)?;
        prepare_store(&mut conn, false, None)?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        store.reconcile_graph_projection()?;
        Ok(store)
    }

    pub fn open_with_migration_progress(
        scope: impl Into<String>,
        database: impl AsRef<Path>,
        progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
    ) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let mut conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection(&conn)?;
        prepare_store(&mut conn, false, Some(&mut *progress))?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        progress(1, 1, "projecting")?;
        store.reconcile_graph_projection()?;
        Ok(store)
    }

    pub fn open_read_only(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        Self::open_read_only_with_timeout(scope, database, BUSY_TIMEOUT)
    }

    pub fn open_for_hook(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        Self::open_read_only_with_timeout(scope, database, Duration::from_millis(250))
    }

    fn open_read_only_with_timeout(
        scope: impl Into<String>,
        database: impl AsRef<Path>,
        timeout: Duration,
    ) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_only_connection(&conn, timeout)?;
        prepare_store_read_only(&conn)?;

        Ok(Self {
            scope,
            database,
            conn,
        })
    }

    pub fn open_for_read(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        match Self::open_read_only(scope.clone(), &database) {
            Ok(store) => Ok(store),
            Err(error) if error.code == "unsupported_store_version" => Self::open(scope, database),
            Err(error) => Err(error),
        }
    }

    pub fn identity(&self) -> Result<StoreIdentity> {
        store_identity(&self.conn)
    }

    pub fn validate_changeset_integrity(&self) -> Result<()> {
        validate_database_integrity(&self.conn)?;
        if self.changeset_storage_kind()?.as_deref() == Some("sparse-v1") {
            validate_sparse_changeset_operations(&self.conn)?;
        }
        Ok(())
    }

    pub fn reconcile_graph_projection(&mut self) -> Result<()> {
        let _ = config::resolve(&self.scope, &self.database)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn snapshot_to(&self, path: &Path) -> Result<()> {
        create_checkpoint(&self.conn, path)
    }

    pub fn changeset_begin(
        &mut self,
        name: &str,
        base: &StoreIdentity,
    ) -> Result<ChangesetDraftState> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let draft_identity = store_identity(&tx)?;
        if draft_identity.store_id != base.store_id || draft_identity.revision != base.revision {
            return Err(AppError::new(
                "changeset_changed",
                "draft snapshot does not match the captured live base",
            ));
        }
        let id: String = tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(32)))", [], |row| row.get(0))?;
        record_operation(
            &tx,
            "changeset_begin",
            &id,
            &json!({"name": name, "base_revision": base.revision}),
        )?;
        let begin_operation_id = tx.last_insert_rowid();
        tx.execute(
            &format!(
                "INSERT INTO changesets(
                    id, name, status, base_revision, base_operation_id,
                    begin_operation_id, created_at
                 ) VALUES (?1, ?2, 'draft', ?3, ?4, ?5, {TIMESTAMP_SQL})"
            ),
            params![
                &id,
                name,
                &base.revision,
                base.operation_id,
                begin_operation_id
            ],
        )?;
        tx.commit()?;
        self.changeset_draft(name, 50)
    }

    pub fn changeset_begin_sparse(
        &mut self,
        name: &str,
        base: &StoreIdentity,
        schema: &str,
        purpose: &str,
        max_source_id: i64,
    ) -> Result<ChangesetDraftState> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM operations", [])?;
        tx.execute("DELETE FROM changesets", [])?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'operations'",
            params![base.operation_id],
        )?;
        tx.execute(
            "INSERT INTO sqlite_sequence(name, seq)
             SELECT 'operations', ?1
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'operations')",
            params![base.operation_id],
        )?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'sources'",
            params![max_source_id],
        )?;
        tx.execute(
            "INSERT INTO sqlite_sequence(name, seq)
             SELECT 'sources', ?1
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'sources')",
            params![max_source_id],
        )?;
        let schema_fingerprint = hash_content(schema);
        let purpose_fingerprint = hash_content(purpose);
        for (key, value) in [
            ("store_id", base.store_id.as_str()),
            ("store_revision", base.revision.as_str()),
            ("schema", schema),
            ("purpose", purpose),
            ("changeset_storage", "sparse-v1"),
            ("changeset_base_meta:schema", schema_fingerprint.as_str()),
            ("changeset_base_meta:purpose", purpose_fingerprint.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()?;
        self.changeset_begin(name, base)
    }

    pub fn max_source_id(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM sources", [], |row| {
                row.get(0)
            })?)
    }

    pub fn changeset_storage_kind(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'changeset_storage'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn changeset_sync_replay_marker(
        &mut self,
        origin_store_id: &str,
        origin_changeset_id: &str,
        complete: bool,
    ) -> Result<bool> {
        for (label, value) in [
            ("origin store ID", origin_store_id),
            ("origin changeset ID", origin_changeset_id),
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(AppError::new(
                    "changeset_replay_invalid",
                    format!("{label} must be 64 hexadecimal characters"),
                ));
            }
        }
        let target = changeset_sync_replay_target(origin_store_id, origin_changeset_id);
        if let Some(state) =
            self.changeset_sync_replay_state(origin_store_id, origin_changeset_id)?
        {
            if !complete || state.complete {
                return Ok(false);
            }
        } else if complete {
            return Err(AppError::new(
                "changeset_replay_not_started",
                "changeset replay cannot complete before its start marker",
            ));
        }
        let action = if complete {
            "changeset_sync_replay_complete"
        } else {
            "changeset_sync_replay_start"
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = record_operation(
            &tx,
            action,
            &target,
            &json!({
                "origin_store_id": origin_store_id,
                "origin_changeset_id": origin_changeset_id,
            }),
        )?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![changeset_sync_replay_expected_key(&target), revision],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn changeset_sync_replay_state(
        &self,
        origin_store_id: &str,
        origin_changeset_id: &str,
    ) -> Result<Option<ChangesetSyncReplayState>> {
        let target = changeset_sync_replay_target(origin_store_id, origin_changeset_id);
        let started: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations
             WHERE action = 'changeset_sync_replay_start' AND target = ?1)",
            [&target],
            |row| row.get(0),
        )?;
        if !started {
            return Ok(None);
        }
        let expected: String = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [changeset_sync_replay_expected_key(&target)],
                |row| row.get(0),
            )
            .map_err(|_| {
                AppError::new(
                    "changeset_replay_conflict",
                    "replay draft lost its revision guard",
                )
            })?;
        let revision_changed = self.identity()?.revision != expected;
        let pending_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM operations started
             WHERE started.action = 'changeset_sync_replay_item_start'
               AND started.target = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM operations completed
                   WHERE completed.action = 'changeset_sync_replay_item'
                     AND completed.target = started.target
                     AND json_extract(completed.detail_json, '$.item_key') =
                         json_extract(started.detail_json, '$.item_key')
               )",
            [&target],
            |row| row.get(0),
        )?;
        if pending_count > 1 {
            return Err(AppError::new(
                "changeset_corrupt",
                "replay draft contains multiple pending items",
            ));
        }
        if revision_changed && pending_count == 0 {
            return Err(AppError::new(
                "changeset_replay_conflict",
                "replay draft was changed outside its replay helper",
            )
            .with_details(json!({"mutated": false, "reason": "draft_revision_changed"})));
        }
        let complete: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations
             WHERE action = 'changeset_sync_replay_complete' AND target = ?1)",
            [&target],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT detail_json FROM operations
             WHERE action = 'changeset_sync_replay_item' AND target = ?1 ORDER BY id",
        )?;
        let mut items = BTreeMap::new();
        for detail in statement.query_map([&target], |row| row.get::<_, String>(0))? {
            let detail: Value = serde_json::from_str(&detail?)
                .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
            let key = detail
                .get("item_key")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AppError::new("changeset_corrupt", "replay item marker lacks item_key")
                })?;
            let digest = detail
                .get("item_digest")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AppError::new("changeset_corrupt", "replay item marker lacks item_digest")
                })?;
            if items.insert(key.to_string(), digest.to_string()).is_some() {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "duplicate replay item marker",
                ));
            }
        }
        Ok(Some(ChangesetSyncReplayState { complete, items }))
    }

    pub(crate) fn changeset_sync_replay_start_item(
        &mut self,
        origin_store_id: &str,
        origin_changeset_id: &str,
        item_key: &str,
        item_digest: &str,
    ) -> Result<ChangesetSyncReplayItemState> {
        validate_changeset_sync_replay_item(item_key, item_digest)?;
        let target = changeset_sync_replay_target(origin_store_id, origin_changeset_id);
        let state = self
            .changeset_sync_replay_state(origin_store_id, origin_changeset_id)?
            .ok_or_else(|| {
                AppError::new("changeset_replay_not_started", "replay has not started")
            })?;
        if state.complete {
            return Ok(ChangesetSyncReplayItemState::Complete);
        }
        if let Some(existing) = state.items.get(item_key) {
            return if existing == item_digest {
                Ok(ChangesetSyncReplayItemState::Complete)
            } else {
                Err(AppError::new(
                    "changeset_replay_conflict",
                    "replay item digest changed",
                ))
            };
        }
        let pending: Option<(String, i64)> = self
            .conn
            .query_row(
                "SELECT json_extract(detail_json, '$.item_digest'), id
             FROM operations
             WHERE action = 'changeset_sync_replay_item_start' AND target = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM operations completed
                   WHERE completed.action = 'changeset_sync_replay_item'
                     AND completed.target = operations.target
                     AND json_extract(completed.detail_json, '$.item_key') =
                         json_extract(operations.detail_json, '$.item_key')
               )
             ORDER BY id DESC LIMIT 1",
                [&target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((pending_digest, _)) = pending {
            let pending_key: String = self.conn.query_row(
                "SELECT json_extract(detail_json, '$.item_key') FROM operations
                 WHERE action = 'changeset_sync_replay_item_start' AND target = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM operations completed
                       WHERE completed.action = 'changeset_sync_replay_item'
                         AND completed.target = operations.target
                         AND json_extract(completed.detail_json, '$.item_key') =
                             json_extract(operations.detail_json, '$.item_key')
                   ) ORDER BY id DESC LIMIT 1",
                [&target],
                |row| row.get(0),
            )?;
            if pending_key != item_key || pending_digest != item_digest {
                return Err(AppError::new(
                    "changeset_replay_conflict",
                    "another replay item is pending",
                ));
            }
            let expected: String = self.conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [changeset_sync_replay_expected_key(&target)],
                |row| row.get(0),
            )?;
            return Ok(if self.identity()?.revision == expected {
                ChangesetSyncReplayItemState::PendingClean
            } else {
                ChangesetSyncReplayItemState::PendingMutated
            });
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = record_operation(
            &tx,
            "changeset_sync_replay_item_start",
            &target,
            &json!({"item_key": item_key, "item_digest": item_digest}),
        )?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![changeset_sync_replay_expected_key(&target), revision],
        )?;
        tx.commit()?;
        Ok(ChangesetSyncReplayItemState::Ready)
    }

    pub(crate) fn changeset_sync_replay_mark_item(
        &mut self,
        origin_store_id: &str,
        origin_changeset_id: &str,
        item_key: &str,
        item_digest: &str,
    ) -> Result<bool> {
        validate_changeset_sync_replay_item(item_key, item_digest)?;
        let target = changeset_sync_replay_target(origin_store_id, origin_changeset_id);
        let started: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations
             WHERE action = 'changeset_sync_replay_start' AND target = ?1)",
            [&target],
            |row| row.get(0),
        )?;
        if !started {
            return Err(AppError::new(
                "changeset_replay_not_started",
                "replay has not started",
            ));
        }
        let complete: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations
             WHERE action = 'changeset_sync_replay_complete' AND target = ?1)",
            [&target],
            |row| row.get(0),
        )?;
        if complete {
            return Err(AppError::new(
                "changeset_replay_conflict",
                "completed replay cannot accept items",
            ));
        }
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT json_extract(detail_json, '$.item_digest') FROM operations
             WHERE action = 'changeset_sync_replay_item' AND target = ?1
               AND json_extract(detail_json, '$.item_key') = ?2",
                params![&target, item_key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing == item_digest {
                return Ok(false);
            }
            return Err(AppError::new(
                "changeset_replay_conflict",
                "replay item digest changed",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revision = record_operation(
            &tx,
            "changeset_sync_replay_item",
            &target,
            &json!({"item_key": item_key, "item_digest": item_digest}),
        )?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![changeset_sync_replay_expected_key(&target), revision],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn changeset_sync_replay_pending_mutations_match(
        &self,
        origin_store_id: &str,
        origin_changeset_id: &str,
        item_key: &str,
    ) -> Result<bool> {
        let target = changeset_sync_replay_target(origin_store_id, origin_changeset_id);
        let start_id: i64 = self.conn.query_row(
            "SELECT id FROM operations
             WHERE action = 'changeset_sync_replay_item_start' AND target = ?1
               AND json_extract(detail_json, '$.item_key') = ?2
             ORDER BY id DESC LIMIT 1",
            params![&target, item_key],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT action,target,detail_json FROM operations
             WHERE id > ?1 AND action NOT LIKE 'changeset_sync_replay_%' ORDER BY id",
        )?;
        let rows = statement
            .query_map([start_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(false);
        }
        let (kind, identifier) = item_key.split_once('\0').unwrap_or(("", ""));
        for (action, operation_target, detail_json) in rows {
            let valid = match kind {
                "source" => {
                    if action != "source_add" {
                        false
                    } else {
                        let detail: Value =
                            serde_json::from_str(&detail_json).map_err(|error| {
                                AppError::new("changeset_corrupt", error.to_string())
                            })?;
                        let source_id = detail.get("source_id").and_then(Value::as_i64);
                        source_id.is_some_and(|source_id| {
                            self.conn
                                .query_row(
                                    "SELECT content_hash = ?2 FROM sources WHERE id = ?1",
                                    params![source_id, identifier],
                                    |row| row.get::<_, bool>(0),
                                )
                                .unwrap_or(false)
                        })
                    }
                }
                "page" => {
                    matches!(action.as_str(), "page_put" | "page_remove")
                        && operation_target == identifier
                }
                "meta" => match identifier {
                    "schema" => action == "schema_set" && operation_target == "schema",
                    "purpose" => action == "purpose_set" && operation_target == "purpose",
                    _ => false,
                },
                "tag" => {
                    matches!(action.as_str(), "tag_set" | "tag_remove")
                        && operation_target.starts_with(&format!("{identifier}/"))
                        || matches!(action.as_str(), "tag_delete" | "tag_autoload")
                            && operation_target == identifier
                }
                "ingest" => {
                    let source_id = self.source_id_by_content_hash(identifier)?;
                    matches!(
                        action.as_str(),
                        "ingest_claim"
                            | "ingest_analyze"
                            | "ingest_complete"
                            | "ingest_fail"
                            | "ingest_retry"
                    ) && source_id.is_some_and(|id| operation_target == id.to_string())
                }
                _ => false,
            };
            if !valid {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn source_id_by_content_hash(&self, content_hash: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash = ?1",
                [content_hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn changeset_replay_source_from_normalized(
        &mut self,
        normalized: &Path,
        source: &DetachedSourceIntent,
        remaining_bytes: u64,
    ) -> Result<(i64, u64)> {
        if let Some(source_id) = self.source_id_by_content_hash(&source.content_hash)? {
            if self.changeset_replay_source_matches(source)? {
                return Ok((source_id, 0));
            }
            return Err(AppError::new(
                "changeset_replay_conflict",
                "existing replay source metadata differs from its after-image",
            ));
        }
        let artifact = validate_sync_state_file(normalized)?;
        let row: Option<(i64, i64)> = artifact
            .query_row(
                "SELECT rowid, length(content) FROM sync_blobs WHERE content_hash = ?1",
                [&source.content_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (rowid, byte_len) = row.ok_or_else(|| {
            AppError::new(
                "changeset_replay_hash_mismatch",
                "normalized Sync state lacks the declared source blob",
            )
        })?;
        if byte_len < 0 || u64::try_from(byte_len).unwrap_or(u64::MAX) > remaining_bytes {
            return Err(AppError::new(
                "changeset_sync_limit",
                "resolved source blob exceeds the Sync transfer byte limit",
            ));
        }
        let mut state_hasher = Sha256::new();
        let actual_hash = hash_and_validate_sync_blob(&artifact, rowid, &mut state_hasher)?;
        if actual_hash != source.content_hash {
            return Err(AppError::new(
                "changeset_replay_hash_mismatch",
                "resolved source content does not match its declared hash",
            ));
        }
        drop(artifact);

        let origin = source
            .origin
            .clone()
            .unwrap_or_else(|| format!("sync:sha256:{}", source.content_hash));
        let title = source.title.as_deref().unwrap_or(&origin).to_string();
        self.conn.execute(
            "ATTACH DATABASE ?1 AS replay_blob",
            [normalized.to_string_lossy().as_ref()],
        )?;
        let result = (|| -> Result<i64> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                &format!(
                    "INSERT INTO sources(content_hash,title,origin,content,structural_navigation,created_at)
                     SELECT ?1,?2,?3,CAST(content AS TEXT),?4,{TIMESTAMP_SQL}
                     FROM replay_blob.sync_blobs WHERE content_hash = ?1"
                ),
                params![&source.content_hash, &title, &origin, source.structural_navigation],
            )?;
            let source_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO search_fts(
                    doc_type,identifier,title_terms,path_terms,summary_terms,body_terms
                 ) SELECT 'source',?1,?2,?3,'',CAST(content AS TEXT)
                   FROM replay_blob.sync_blobs WHERE content_hash = ?4",
                params![
                    source_id.to_string(),
                    source_title_terms(&title, &origin),
                    joined_terms(source_parent(&origin)),
                    &source.content_hash,
                ],
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO ingest_jobs(source_id,status,updated_at)
                     VALUES(?1,'pending',{TIMESTAMP_SQL})"
                ),
                [source_id],
            )?;
            record_operation(
                &tx,
                "source_add",
                &origin,
                &json!({"source_id": source_id, "created": true,
                         "tracked_path": Value::Null, "path_revision": Value::Null,
                         "path_advanced": Value::Null}),
            )?;
            tx.commit()?;
            Ok(source_id)
        })();
        let _ = self.conn.execute("DETACH DATABASE replay_blob", []);
        result.map(|source_id| (source_id, u64::try_from(byte_len).unwrap_or(0)))
    }

    pub(crate) fn changeset_replay_prepare_existing_source(
        &mut self,
        live_path: &Path,
        source: &DetachedSourceIntent,
    ) -> Result<i64> {
        if let Some(source_id) = self.source_id_by_content_hash(&source.content_hash)? {
            return Ok(source_id);
        }
        self.conn.execute(
            "ATTACH DATABASE ?1 AS replay_live",
            [live_path.to_string_lossy().as_ref()],
        )?;
        let result = (|| -> Result<i64> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let inserted = tx.execute(
                "INSERT INTO sources(id,content_hash,title,origin,content,structural_navigation,created_at)
                 SELECT id,content_hash,title,origin,'',structural_navigation,created_at
                 FROM replay_live.sources WHERE content_hash = ?1",
                [&source.content_hash],
            )?;
            if inserted != 1 {
                return Err(AppError::new(
                    "changeset_replay_invalid",
                    "existing replay source is absent from the synchronized target",
                ));
            }
            let source_id: i64 = tx.query_row(
                "SELECT id FROM sources WHERE content_hash = ?1",
                [&source.content_hash],
                |row| row.get(0),
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO ingest_jobs(source_id,status,updated_at)
                     VALUES(?1,'pending',{TIMESTAMP_SQL})"
                ),
                [source_id],
            )?;
            tx.commit()?;
            Ok(source_id)
        })();
        let _ = self.conn.execute("DETACH DATABASE replay_live", []);
        result
    }

    #[cfg(test)]
    pub(crate) fn changeset_replay_blob_max_buffered_bytes() -> u64 {
        SYNC_BLOB_MAX_BUFFERED_BYTES.load(Ordering::Relaxed)
    }

    pub(crate) fn tag_exists_for_replay(&self, tag: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tags WHERE name = ?1)",
            [tag],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn changeset_replay_source_matches(
        &self,
        source: &DetachedSourceIntent,
    ) -> Result<bool> {
        let fallback_origin;
        let expected_origin = if let Some(origin) = source.origin.as_deref() {
            origin
        } else {
            fallback_origin = format!("sync:sha256:{}", source.content_hash);
            &fallback_origin
        };
        let expected_title = source.title.as_deref().unwrap_or(expected_origin);
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sources
             WHERE content_hash = ?1 AND title = ?2 AND origin = ?3
               AND structural_navigation = ?4)",
            params![
                &source.content_hash,
                expected_title,
                expected_origin,
                source.structural_navigation
            ],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn changeset_replay_page_matches(&self, page: &DetachedPageIntent) -> Result<bool> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE slug = ?1)",
            [&page.slug],
            |row| row.get(0),
        )?;
        let Some(after) = &page.after else {
            return Ok(!exists);
        };
        if !exists {
            return Ok(false);
        }
        let actual = self.load_page(&page.slug)?;
        let mut hashes = actual
            .source_ids
            .iter()
            .map(|id| {
                self.conn
                    .query_row(
                        "SELECT content_hash FROM sources WHERE id = ?1",
                        [id],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()?;
        hashes.sort();
        let mut expected_hashes = after.source_hashes.clone();
        expected_hashes.sort();
        Ok(actual.title == after.title
            && actual.kind == after.kind
            && actual.summary == after.summary
            && actual.body == after.body
            && actual.provenance == after.provenance
            && hashes == expected_hashes)
    }

    pub(crate) fn changeset_replay_meta_matches(&self, meta: &DetachedMetaIntent) -> Result<bool> {
        let actual = match meta.key.as_str() {
            "schema" => self.schema_text()?,
            "purpose" => self.purpose_text()?,
            _ => return Ok(false),
        };
        Ok(actual.as_deref() == Some(meta.value.as_str()))
    }

    pub(crate) fn changeset_replay_tag_matches(&self, tag: &DetachedTagIntent) -> Result<bool> {
        let snapshot = load_tag_snapshot(&self.conn, "main", &tag.name)?;
        let Some(after) = &tag.after else {
            return Ok(snapshot.policy.is_none());
        };
        let Some(policy) = snapshot.policy else {
            return Ok(false);
        };
        let actual_memberships = snapshot
            .memberships
            .into_iter()
            .map(|member| DetachedTagMembership {
                page_slug: member.page_slug,
                priority: member.priority,
                reason: member.reason,
            })
            .collect::<Vec<_>>();
        Ok(policy.autoload == after.autoload
            && policy.autoload_priority == after.autoload_priority
            && policy.autoload_limit == after.autoload_limit
            && policy.autoload_max_chars == after.autoload_max_chars
            && policy.reason == after.reason
            && actual_memberships == after.memberships)
    }

    pub(crate) fn changeset_replay_ingest_matches(
        &self,
        content_hash: &str,
        ingest: &DetachedIngestIntent,
    ) -> Result<bool> {
        let actual = self
            .conn
            .query_row(
                "SELECT j.status, j.attempts, j.analysis, j.no_derived_pages_reason
             FROM ingest_jobs j JOIN sources s ON s.id = j.source_id
             WHERE s.content_hash = ?1",
                [content_hash],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(actual.is_some_and(|actual| {
            actual
                == (
                    ingest.status.clone(),
                    ingest.attempts,
                    ingest.analysis.clone(),
                    ingest.no_derived_pages_reason.clone(),
                )
        }))
    }

    pub(crate) fn detached_changeset_intent(&self) -> Result<Option<DetachedChangesetStoreExport>> {
        const MAX_ACTIONS: usize = 2_048;
        const MAX_ITEMS: usize = 1_024;
        const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
        const MAX_TOTAL_BLOB_BYTES: u64 = 8 * 1024 * 1024 * 1024;

        self.validate_changeset_integrity()?;
        if self.changeset_storage_kind()?.as_deref() != Some("sparse-v1") {
            return Err(AppError::new(
                "changeset_sync_unsupported",
                "only sparse changesets can be exported as detached intent",
            ));
        }
        let state = self.changeset_draft(
            self.conn
                .query_row(
                    "SELECT name FROM changesets WHERE status = 'draft' LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )?
                .as_str(),
            MAX_ACTIONS + 1,
        )?;
        if state.staged_operation_count > MAX_ACTIONS {
            return Err(AppError::new(
                "changeset_sync_limit",
                "detached changeset has too many actions",
            ));
        }
        if state.operations.iter().any(|operation| {
            matches!(
                operation.action.as_str(),
                "changeset_sync_replay_start" | "changeset_sync_replay_complete"
            )
        }) {
            return Ok(None);
        }

        let created_source_ids = changeset_created_source_ids(&self.conn)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut source_ids = created_source_ids.clone();
        for operation in &state.operations {
            if operation.action.starts_with("ingest_") {
                source_ids.insert(operation.target.parse::<i64>().map_err(|_| {
                    AppError::new(
                        "changeset_corrupt",
                        "ingest action has an invalid source ID",
                    )
                })?);
            }
        }
        let mut sources = Vec::with_capacity(source_ids.len());
        let mut blobs = Vec::with_capacity(source_ids.len());
        let mut source_hashes = BTreeMap::new();
        let mut total_blob_bytes = 0_u64;
        for source_id in source_ids {
            let content_required = created_source_ids.contains(&source_id);
            let source = self
                .conn
                .query_row(
                    "SELECT s.content_hash, s.title, s.origin, s.structural_navigation,
                        LENGTH(CAST(s.content AS BLOB)), j.status, j.attempts,
                        j.analysis, j.no_derived_pages_reason
                 FROM sources s JOIN ingest_jobs j ON j.source_id = s.id
                 WHERE s.id = ?1",
                    [source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, bool>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    AppError::new("changeset_corrupt", "staged source after-image is missing")
                })?;
            if source.0.len() != 64
                || !source
                    .0
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "invalid source content hash",
                ));
            }
            let blob_bytes = u64::try_from(source.4).map_err(|_| {
                AppError::new("changeset_corrupt", "invalid staged source byte count")
            })?;
            if content_required {
                total_blob_bytes = total_blob_bytes.checked_add(blob_bytes).ok_or_else(|| {
                    AppError::new(
                        "changeset_sync_limit",
                        "detached source byte count overflowed",
                    )
                })?;
                if total_blob_bytes > MAX_TOTAL_BLOB_BYTES {
                    return Err(AppError::new(
                        "changeset_sync_limit",
                        "detached source blobs exceed the Sync transfer byte limit",
                    ));
                }
            }
            source_hashes.insert(source_id, source.0.clone());
            if content_required {
                blobs.push((source_id, source.0.clone(), blob_bytes));
            }
            let base_fingerprint = if content_required {
                "absent".into()
            } else {
                source.0.clone()
            };
            sources.push(DetachedSourceIntent {
                content_hash: source.0,
                title: source.1,
                origin: portable_detached_origin(source.2),
                structural_navigation: source.3,
                base_fingerprint,
                content_required,
                ingest: DetachedIngestIntent {
                    status: source.5,
                    attempts: source.6,
                    analysis: source.7,
                    no_derived_pages_reason: source.8,
                },
            });
        }

        let mut pages = Vec::new();
        for (slug, base_fingerprint) in self.changeset_touched_pages()? {
            let after = load_sparse_page_snapshot(&self.conn, &slug)?
                .map(|page| -> Result<DetachedPageAfterImage> {
                    let hashes = page
                        .source_ids
                        .iter()
                        .map(|id| {
                            source_hashes
                                .get(id)
                                .cloned()
                                .or_else(|| {
                                    self.conn
                                        .query_row(
                                            "SELECT content_hash FROM sources WHERE id = ?1",
                                            [id],
                                            |row| row.get(0),
                                        )
                                        .optional()
                                        .ok()
                                        .flatten()
                                })
                                .ok_or_else(|| {
                                    AppError::new(
                                        "changeset_corrupt",
                                        "page cites a missing source",
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Ok(DetachedPageAfterImage {
                        title: page.title,
                        kind: page.kind,
                        summary: page.summary,
                        body: page.body,
                        source_hashes: hashes,
                        provenance: page.provenance,
                    })
                })
                .transpose()?;
            pages.push(DetachedPageIntent {
                slug,
                base_fingerprint,
                after,
            });
        }

        let mut tags = Vec::new();
        for (name, base_fingerprint) in self.changeset_touched_tags()? {
            let snapshot = load_tag_snapshot(&self.conn, "main", &name)?;
            let after = snapshot.policy.map(|policy| DetachedTagAfterImage {
                autoload: policy.autoload,
                autoload_priority: policy.autoload_priority,
                autoload_limit: policy.autoload_limit,
                autoload_max_chars: policy.autoload_max_chars,
                reason: policy.reason,
                memberships: snapshot
                    .memberships
                    .into_iter()
                    .map(|member| DetachedTagMembership {
                        page_slug: member.page_slug,
                        priority: member.priority,
                        reason: member.reason,
                    })
                    .collect(),
            });
            tags.push(DetachedTagIntent {
                name,
                base_fingerprint,
                after,
            });
        }

        let mut meta = Vec::new();
        for (key, base_fingerprint) in self.changeset_touched_meta()? {
            if !matches!(key.as_str(), "schema" | "purpose") {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "unsupported detached meta key",
                ));
            }
            let value =
                self.conn
                    .query_row("SELECT value FROM meta WHERE key = ?1", [&key], |row| {
                        row.get(0)
                    })?;
            meta.push(DetachedMetaIntent {
                key,
                base_fingerprint,
                value,
            });
        }

        if sources.len() + pages.len() + tags.len() + meta.len() > MAX_ITEMS {
            return Err(AppError::new(
                "changeset_sync_limit",
                "detached changeset has too many typed items",
            ));
        }
        let mut actions = Vec::with_capacity(state.operations.len());
        for operation in state.operations.iter().rev() {
            let detail = operation.detail.as_object().ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "changeset operation detail is not an object",
                )
            })?;
            let action = match operation.action.as_str() {
                "source_add" => {
                    let id = detail
                        .get("source_id")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| {
                            AppError::new("changeset_corrupt", "source_add lacks source_id")
                        })?;
                    DetachedChangesetAction::SourceAdd {
                        content_hash: source_hashes
                            .get(&id)
                            .cloned()
                            .or_else(|| {
                                self.conn
                                    .query_row(
                                        "SELECT content_hash FROM sources WHERE id = ?1",
                                        [id],
                                        |row| row.get(0),
                                    )
                                    .optional()
                                    .ok()
                                    .flatten()
                            })
                            .ok_or_else(|| {
                                AppError::new(
                                    "changeset_corrupt",
                                    "source_add references a missing source",
                                )
                            })?,
                    }
                }
                action @ ("ingest_claim" | "ingest_analyze" | "ingest_complete" | "ingest_fail"
                | "ingest_retry") => {
                    let id = operation.target.parse::<i64>().map_err(|_| {
                        AppError::new(
                            "changeset_corrupt",
                            "ingest action has an invalid source ID",
                        )
                    })?;
                    DetachedChangesetAction::Ingest {
                        action: action.to_string(),
                        content_hash: source_hashes
                            .get(&id)
                            .cloned()
                            .or_else(|| {
                                self.conn
                                    .query_row(
                                        "SELECT content_hash FROM sources WHERE id = ?1",
                                        [id],
                                        |row| row.get(0),
                                    )
                                    .optional()
                                    .ok()
                                    .flatten()
                            })
                            .ok_or_else(|| {
                                AppError::new(
                                    "changeset_corrupt",
                                    "ingest action references a missing source",
                                )
                            })?,
                    }
                }
                "page_put" => DetachedChangesetAction::PagePut {
                    slug: operation.target.clone(),
                },
                "page_remove" => DetachedChangesetAction::PageRemove {
                    slug: operation.target.clone(),
                },
                action @ ("tag_set" | "tag_remove" | "tag_delete" | "tag_autoload") => {
                    DetachedChangesetAction::Tag {
                        action: action.to_string(),
                        name: detail
                            .get("tag")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                AppError::new("changeset_corrupt", "tag action lacks tag")
                            })?
                            .to_string(),
                    }
                }
                "schema_set" => DetachedChangesetAction::MetaSet {
                    key: "schema".into(),
                },
                "purpose_set" => DetachedChangesetAction::MetaSet {
                    key: "purpose".into(),
                },
                "search" => DetachedChangesetAction::Search,
                _ => {
                    return Err(AppError::new(
                        "changeset_corrupt",
                        "unsupported detached changeset action",
                    ));
                }
            };
            actions.push(action);
        }
        let intent = DetachedChangesetIntent {
            version: 1,
            origin_changeset_id: state.id,
            name: state.name,
            actions,
            sources,
            pages,
            tags,
            meta,
        };
        let bytes = serde_json::to_vec(&intent)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(AppError::new(
                "changeset_sync_limit",
                "detached changeset metadata exceeds the byte limit",
            ));
        }
        Ok(Some((intent, blobs)))
    }

    pub fn changeset_touched_pages(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT o.target,
                    COALESCE(m.value, 'absent')
             FROM operations o
             JOIN changesets c ON o.id > c.begin_operation_id AND c.status = 'draft'
             LEFT JOIN meta m ON m.key = 'changeset_base_page:' || o.target
             WHERE o.action IN ('page_put', 'page_remove')
             ORDER BY o.target",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn changeset_touched_tags(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT json_extract(o.detail_json, '$.tag'), m.value
             FROM operations o
             JOIN changesets c ON o.id > c.begin_operation_id AND c.status = 'draft'
             JOIN meta m ON m.key = 'changeset_base_tag:' || json_extract(o.detail_json, '$.tag')
             WHERE o.action IN ('tag_set', 'tag_remove', 'tag_delete', 'tag_autoload')
             ORDER BY 1",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn changeset_touched_meta(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT touched.key, m.value
             FROM (
                SELECT DISTINCT CASE o.action
                    WHEN 'schema_set' THEN 'schema'
                    WHEN 'purpose_set' THEN 'purpose'
                END AS key
                FROM operations o
                JOIN changesets c ON o.id > c.begin_operation_id AND c.status = 'draft'
                WHERE o.action IN ('schema_set', 'purpose_set')
             ) touched
             JOIN meta m ON m.key = 'changeset_base_meta:' || touched.key
             ORDER BY touched.key",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn meta_fingerprint(&self, key: &str) -> Result<String> {
        let value: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )?;
        Ok(hash_content(&value))
    }

    pub fn page_mutation_fingerprint(&self, slug: &str) -> Result<Option<String>> {
        Ok(load_page_mutation_base(&self.conn, slug)?.map(|value| value.content_fingerprint))
    }

    pub fn changeset_prepare_page_touch(
        &mut self,
        live_path: &Path,
        slug: &str,
        additional_source_ids: &[i64],
    ) -> Result<()> {
        let schema = "live_base";
        self.conn.execute(
            "ATTACH DATABASE ?1 AS live_base",
            params![live_path.to_string_lossy().as_ref()],
        )?;
        let result = (|| -> Result<()> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let base_key = format!("changeset_base_page:{slug}");
            let prepared = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![&base_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if prepared.is_none() {
                tx.execute(
                    "INSERT OR IGNORE INTO pages(
                        slug, title, kind, summary, body, structural_navigation,
                        created_at, updated_at
                     ) SELECT slug, title, kind, summary, body, structural_navigation,
                              created_at, updated_at
                       FROM live_base.pages WHERE slug = ?1",
                    params![slug],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO links(from_slug, to_slug)
                     SELECT from_slug, to_slug FROM live_base.links WHERE from_slug = ?1",
                    params![slug],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO page_provenance(page_slug, provenance)
                     SELECT page_slug, provenance FROM live_base.page_provenance
                     WHERE page_slug = ?1",
                    params![slug],
                )?;
            }
            let mut source_ids = additional_source_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            if prepared.is_none() {
                let mut statement = tx
                    .prepare("SELECT source_id FROM live_base.page_sources WHERE page_slug = ?1")?;
                for row in statement.query_map(params![slug], |row| row.get::<_, i64>(0))? {
                    source_ids.insert(row?);
                }
            }
            for source_id in source_ids {
                tx.execute(
                    &format!(
                        "INSERT OR IGNORE INTO sources(
                            id, content_hash, title, origin, content,
                            structural_navigation, created_at
                         ) SELECT id, content_hash, title, origin, '',
                                  structural_navigation, created_at
                           FROM {schema}.sources WHERE id = ?1"
                    ),
                    params![source_id],
                )?;
                if prepared.is_none() {
                    tx.execute(
                        "INSERT OR IGNORE INTO page_sources(page_slug, source_id)
                         SELECT page_slug, source_id FROM live_base.page_sources
                         WHERE page_slug = ?1 AND source_id = ?2",
                        params![slug, source_id],
                    )?;
                }
            }
            if prepared.is_none() {
                let base = load_page_mutation_base(&tx, slug)?
                    .map(|value| value.content_fingerprint)
                    .unwrap_or_else(|| "absent".into());
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                    params![base_key, base],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        let _ = self.conn.execute("DETACH DATABASE live_base", []);
        result
    }

    pub fn changeset_prepare_tag_touch(
        &mut self,
        live_path: &Path,
        tag: &str,
        page: Option<&str>,
    ) -> Result<()> {
        let tag = normalize_tag_name(tag)?;
        self.conn.execute(
            "ATTACH DATABASE ?1 AS live_base",
            params![live_path.to_string_lossy().as_ref()],
        )?;
        let result = (|| -> Result<()> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let base_key = format!("changeset_base_tag:{tag}");
            let prepared: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key = ?1)",
                [&base_key],
                |row| row.get(0),
            )?;
            if !prepared {
                let base = load_tag_snapshot(&tx, "live_base", &tag)?;
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                    params![&base_key, tag_snapshot_fingerprint(&base)],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO tags(
                        name, autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason, updated_at
                     ) SELECT
                        name, autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason, updated_at
                       FROM live_base.tags WHERE name = ?1",
                    [&tag],
                )?;
            }
            if let Some(page) = page {
                tx.execute(
                    "INSERT OR IGNORE INTO pages(
                        slug, title, kind, summary, body, structural_navigation,
                        created_at, updated_at
                     ) SELECT slug, title, kind, summary, '', structural_navigation,
                              created_at, updated_at
                       FROM live_base.pages WHERE slug = ?1",
                    [page],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO page_tags(
                        tag_name, page_slug, priority, reason, created_at, updated_at
                     ) SELECT
                        tag_name, page_slug, priority, reason, created_at, updated_at
                       FROM live_base.page_tags
                       WHERE tag_name = ?1 AND page_slug = ?2",
                    params![&tag, page],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        let _ = self.conn.execute("DETACH DATABASE live_base", []);
        result
    }

    pub fn changeset_draft(&self, name: &str, limit: usize) -> Result<ChangesetDraftState> {
        let row = self
            .conn
            .query_row(
                "SELECT id, name, status, base_revision, base_operation_id,
                        begin_operation_id, created_at
                 FROM changesets
                 WHERE name = ?1 AND status = 'draft'
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new(
                    "changeset_not_found",
                    format!("draft changeset not found: {name}"),
                )
            })?;
        let identity = self.identity()?;
        let staged_operation_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE id > ?1",
            params![row.5],
            |row| row.get(0),
        )?;
        let mut action_counts = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT action, COUNT(*)
                 FROM operations
                 WHERE id > ?1
                 GROUP BY action
                 ORDER BY action",
            )?;
            let rows = statement.query_map(params![row.5], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for value in rows {
                let (action, count) = value?;
                action_counts.insert(action, usize::try_from(count).unwrap_or(usize::MAX));
            }
        }
        let operations = {
            let mut statement = self.conn.prepare(
                "SELECT id, action, target, detail_json, created_at
                 FROM operations
                 WHERE id > ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )?;
            statement
                .query_map(params![row.5, limit as i64], read_operation_record)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(ChangesetDraftState {
            id: row.0,
            name: row.1,
            status: row.2,
            base_revision: row.3,
            base_operation_id: row.4,
            begin_operation_id: row.5,
            draft_revision: identity.revision,
            draft_operation_id: identity.operation_id,
            staged_operation_count: usize::try_from(staged_operation_count).map_err(|_| {
                AppError::new(
                    "database_error",
                    "changeset operation count is out of range",
                )
            })?,
            action_counts,
            operations,
            created_at: row.6,
        })
    }

    pub(crate) fn changeset_graph_documents(&self) -> Result<Vec<(String, String)>> {
        let begin_operation_id: i64 = self.conn.query_row(
            "SELECT begin_operation_id FROM changesets
             WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT action, target, detail_json FROM operations
             WHERE id > ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([begin_operation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut documents = BTreeSet::new();
        for row in rows {
            let (action, target, detail) = row?;
            match action.as_str() {
                "page_put" | "page_remove" => {
                    documents.insert(("page".to_string(), target));
                }
                "source_add" => {
                    let detail: Value = serde_json::from_str(&detail)
                        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
                    if let Some(id) = detail.get("source_id").and_then(Value::as_i64) {
                        documents.insert(("source".to_string(), id.to_string()));
                    }
                }
                "source_remove" => {
                    documents.insert(("source".to_string(), target));
                }
                _ => {}
            }
        }
        Ok(documents.into_iter().collect())
    }

    pub(crate) fn changeset_touched_source_paths(&self) -> Result<Vec<String>> {
        let begin_operation_id: i64 = self.conn.query_row(
            "SELECT begin_operation_id FROM changesets
             WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT detail_json FROM operations
             WHERE id > ?1 AND action = 'source_add' ORDER BY id",
        )?;
        let rows = statement.query_map([begin_operation_id], |row| row.get::<_, String>(0))?;
        let mut paths = BTreeSet::new();
        for row in rows {
            let detail: Value = serde_json::from_str(&row?)
                .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
            if let Some(path) = detail.get("tracked_path").and_then(Value::as_str) {
                paths.insert(path.to_string());
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub(crate) fn source_path_head(&self, path: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT source_id FROM source_path_revisions
                 WHERE tracked_path = ?1 ORDER BY revision DESC LIMIT 1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn changeset_checkpoint_create(&self, changeset_id: &str) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let prefix = format!("pre-changeset-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        create_checkpoint(&self.conn, &path)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_sparse_checkpoint_create(
        &self,
        changeset_id: &str,
        draft_path: &Path,
    ) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let draft = Store::open_for_read(self.scope.clone(), draft_path)?;
        let identity = self.identity()?;
        let mut pages = Vec::new();
        for (slug, expected) in draft.changeset_touched_pages()? {
            let before = load_sparse_page_snapshot(&self.conn, &slug)?;
            let observed = before
                .as_ref()
                .map(sparse_page_fingerprint)
                .unwrap_or_else(|| "absent".into());
            if observed != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("page {slug} changed while preparing its inverse patch"),
                ));
            }
            let after_fingerprint = draft
                .page_mutation_fingerprint(&slug)?
                .unwrap_or_else(|| "absent".into());
            let after = load_sparse_page_snapshot(&draft.conn, &slug)?;
            pages.push(SparsePageInverse {
                inbound_links: load_sparse_page_inbound_links(&self.conn, &slug)?,
                slug,
                before,
                after,
                after_fingerprint,
            });
        }
        let mut meta = Vec::new();
        for (key, _) in draft.changeset_touched_meta()? {
            let before: String = self.conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![&key],
                |row| row.get(0),
            )?;
            let after: String = draft.conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![&key],
                |row| row.get(0),
            )?;
            meta.push(SparseMetaInverse {
                key,
                before,
                after_fingerprint: hash_content(&after),
            });
        }
        let mut sources = Vec::new();
        for source_id in changeset_created_source_ids(&draft.conn)? {
            let after = load_sparse_source_snapshot(&draft.conn, source_id)?.ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    format!("staged source {source_id} is missing"),
                )
            })?;
            sources.push(SparseSourceInverse {
                source_id,
                before: None,
                after_fingerprint: sparse_source_fingerprint(&after),
            });
        }
        let mut source_paths = Vec::new();
        for tracked_path in draft.changeset_touched_source_paths()? {
            let before = load_sparse_tracked_path(&self.conn, &tracked_path)?;
            let staged = load_sparse_tracked_path(&draft.conn, &tracked_path)?;
            let after = project_sparse_tracked_path(&before, &staged);
            source_paths.push(SparseTrackedPathInverse {
                tracked_path,
                before,
                after: after.clone(),
                after_fingerprint: sparse_tracked_path_fingerprint(&after),
            });
        }
        let mut tags = Vec::new();
        for (name, expected) in draft.changeset_touched_tags()? {
            let before = load_tag_snapshot(&self.conn, "main", &name)?;
            if tag_snapshot_fingerprint(&before) != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("tag {name} changed while preparing its inverse patch"),
                ));
            }
            let after = projected_sparse_tag_snapshot(&before, &draft, &name)?;
            tags.push(SparseTagInverse {
                name,
                before,
                after_fingerprint: tag_snapshot_fingerprint(&after),
            });
        }
        let payload = SparseInversePayload {
            version: 1,
            changeset_id: changeset_id.to_string(),
            store_id: identity.store_id,
            pages,
            meta,
            sources,
            source_paths,
            tags,
        };
        let encoded = serde_json::to_vec(&payload)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let envelope = SparseInverseEnvelope {
            checksum: hash_content(
                std::str::from_utf8(&encoded)
                    .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?,
            ),
            payload,
        };
        let prefix = format!("pre-changeset-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        write_sparse_inverse(&path, &envelope)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_freeze(
        &mut self,
        id: &str,
        expected_revision: &str,
        expected_operation_id: i64,
        expected_operation_count: usize,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let identity = store_identity(&tx)?;
        let begin_operation_id: i64 = tx
            .query_row(
                "SELECT begin_operation_id FROM changesets
                 WHERE id = ?1 AND status = 'draft'",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new("changeset_changed", "draft changeset is no longer writable")
            })?;
        let operation_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM operations WHERE id > ?1",
            params![begin_operation_id],
            |row| row.get(0),
        )?;
        if identity.revision != expected_revision
            || identity.operation_id != expected_operation_id
            || usize::try_from(operation_count).ok() != Some(expected_operation_count)
        {
            return Err(AppError::new(
                "changeset_changed",
                "draft changeset changed during commit preflight",
            ));
        }
        let frozen: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![CHANGESET_FREEZE_KEY],
                |row| row.get(0),
            )
            .optional()?;
        match frozen.as_deref() {
            Some(value) if value != id => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "draft freeze marker belongs to another changeset",
                ));
            }
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                    params![CHANGESET_FREEZE_KEY, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn changeset_publish(
        &mut self,
        draft_path: &Path,
        input: &ChangesetPublishInput,
    ) -> Result<ChangesetCommitState> {
        self.conn.execute(
            "ATTACH DATABASE ?1 AS candidate",
            params![draft_path.to_string_lossy().as_ref()],
        )?;
        let result = publish_attached_changeset(&mut self.conn, input);
        let _ = self.conn.execute("DETACH DATABASE candidate", []);
        result
    }

    pub fn changeset_sparse_lint(
        &mut self,
        draft_path: &Path,
        limit: usize,
        offset: usize,
    ) -> Result<LintResponse> {
        self.conn.execute(
            "ATTACH DATABASE ?1 AS candidate",
            params![draft_path.to_string_lossy().as_ref()],
        )?;
        let begin_operation_id: i64 = self.conn.query_row(
            "SELECT begin_operation_id FROM candidate.changesets WHERE status = 'draft'",
            [],
            |row| row.get(0),
        )?;
        let result = (|| -> Result<LintResponse> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            merge_sparse_candidate(&tx, begin_operation_id, false)?;
            Self::lint_connection(
                &tx,
                self.scope.clone(),
                draft_path.to_string_lossy().into_owned(),
                limit,
                offset,
            )
        })();
        let _ = self.conn.execute("DETACH DATABASE candidate", []);
        result
    }

    pub fn changeset_committed_by_id(&self, id: &str) -> Result<Option<ChangesetCommitState>> {
        load_committed_changeset(&self.conn, id)
    }

    pub fn changeset_history_by_id(&self, id: &str) -> Result<Option<ChangesetHistoryState>> {
        self.conn
            .query_row(
                "SELECT id, name, status, base_revision, base_operation_id,
                        begin_operation_id, pre_commit_checkpoint, post_revision,
                        created_at, committed_at, rolled_back_at
                 FROM changesets WHERE id = ?1",
                params![id],
                read_changeset_history,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn changeset_rollback_checkpoint_create(
        &self,
        changeset_id: &str,
    ) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let prefix = format!("pre-rollback-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        create_checkpoint(&self.conn, &path)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_sparse_rollback_checkpoint_create(
        &self,
        history: &ChangesetHistoryState,
    ) -> Result<CheckpointResponse> {
        let checkpoint = history.pre_commit_checkpoint.as_deref().ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "committed changeset has no pre-commit checkpoint",
            )
        })?;
        let inverse = load_sparse_inverse(&checkpoint_path(&self.database, checkpoint)?)?;
        let identity = self.identity()?;
        let source_id_remap = load_sparse_source_id_remap(&self.conn, &history.id)?;
        let pages = inverse
            .payload
            .pages
            .iter()
            .map(|page| {
                let before = load_sparse_page_snapshot(&self.conn, &page.slug)?;
                Ok(SparsePageInverse {
                    inbound_links: load_sparse_page_inbound_links(&self.conn, &page.slug)?,
                    slug: page.slug.clone(),
                    before,
                    after: page.before.clone(),
                    after_fingerprint: page
                        .before
                        .as_ref()
                        .map(sparse_page_fingerprint)
                        .unwrap_or_else(|| "absent".into()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let meta = inverse
            .payload
            .meta
            .iter()
            .map(|entry| {
                let current: String = self.conn.query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![&entry.key],
                    |row| row.get(0),
                )?;
                Ok(SparseMetaInverse {
                    key: entry.key.clone(),
                    before: current,
                    after_fingerprint: hash_content(&entry.before),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let sources = inverse
            .payload
            .sources
            .iter()
            .map(|entry| {
                let live_id = source_id_remap
                    .iter()
                    .find(|remap| remap.draft_id == entry.source_id)
                    .map_or(entry.source_id, |remap| remap.live_id);
                let before = load_sparse_source_snapshot(&self.conn, live_id)?.map(|mut source| {
                    source.id = entry.source_id;
                    source
                });
                Ok(SparseSourceInverse {
                    source_id: entry.source_id,
                    before,
                    after_fingerprint: entry
                        .before
                        .as_ref()
                        .map(sparse_source_fingerprint)
                        .unwrap_or_else(|| "absent".into()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let source_paths = inverse
            .payload
            .source_paths
            .iter()
            .map(|entry| {
                Ok(SparseTrackedPathInverse {
                    tracked_path: entry.tracked_path.clone(),
                    before: load_sparse_tracked_path(&self.conn, &entry.tracked_path)?,
                    after: entry.before.clone(),
                    after_fingerprint: sparse_tracked_path_fingerprint(&entry.before),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let tags = inverse
            .payload
            .tags
            .iter()
            .map(|entry| {
                Ok(SparseTagInverse {
                    name: entry.name.clone(),
                    before: load_tag_snapshot(&self.conn, "main", &entry.name)?,
                    after_fingerprint: tag_snapshot_fingerprint(&entry.before),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let payload = SparseInversePayload {
            version: 1,
            changeset_id: history.id.clone(),
            store_id: identity.store_id,
            pages,
            meta,
            sources,
            source_paths,
            tags,
        };
        let encoded = serde_json::to_vec(&payload)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let envelope = SparseInverseEnvelope {
            checksum: hash_content(
                std::str::from_utf8(&encoded)
                    .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?,
            ),
            payload,
        };
        let prefix = format!("pre-rollback-{}", &history.id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        write_sparse_inverse(&path, &envelope)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_rollback_checkpoint_validate(
        &self,
        history: &ChangesetHistoryState,
        store_id: &str,
    ) -> Result<bool> {
        let checkpoint = history.pre_commit_checkpoint.as_deref().ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "committed changeset has no pre-commit checkpoint",
            )
        })?;
        let path = checkpoint_path(&self.database, checkpoint)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            AppError::new(
                "changeset_corrupt",
                format!("pre-commit checkpoint is unavailable: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::new(
                "changeset_corrupt",
                "pre-commit checkpoint is not a regular file",
            ));
        }
        if let Ok(envelope) = load_sparse_inverse(&path) {
            if envelope.payload.store_id != store_id || envelope.payload.changeset_id != history.id
            {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "sparse inverse patch belongs to another Wiki or changeset",
                ));
            }
            return Ok(true);
        }
        let checkpoint_store = Store::open_read_only(self.scope.clone(), &path)
            .map_err(|error| AppError::new("changeset_corrupt", error.message))?;
        if checkpoint_store.identity()?.store_id != store_id {
            return Err(AppError::new(
                "changeset_corrupt",
                "pre-commit checkpoint belongs to another Wiki",
            ));
        }
        Ok(false)
    }

    pub fn changeset_rollback(
        &mut self,
        input: &ChangesetRollbackInput,
    ) -> Result<ChangesetRollbackState> {
        let checkpoint = input
            .history
            .pre_commit_checkpoint
            .as_deref()
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "committed changeset has no pre-commit checkpoint",
                )
            })?;
        let path = checkpoint_path(&self.database, checkpoint)?;
        let sparse =
            self.changeset_rollback_checkpoint_validate(&input.history, &input.store_id)?;

        if sparse {
            return rollback_sparse_changeset(&mut self.conn, &path, input);
        }

        self.conn.execute(
            "ATTACH DATABASE ?1 AS candidate",
            params![path.to_string_lossy().as_ref()],
        )?;
        let result = rollback_attached_changeset(&mut self.conn, input);
        let _ = self.conn.execute("DETACH DATABASE candidate", []);
        result
    }

    pub fn changeset_rollback_state_by_id(
        &self,
        id: &str,
    ) -> Result<Option<ChangesetRollbackState>> {
        let row = self
            .conn
            .query_row(
                "SELECT c.name, o.detail_json
                 FROM changesets c
                 JOIN operations o
                   ON o.action = 'changeset_rollback' AND o.target = c.id
                 WHERE c.id = ?1 AND c.status = 'rolled_back'
                 ORDER BY o.id DESC LIMIT 1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((name, detail)) = row else {
            return Ok(None);
        };
        let detail: Value = serde_json::from_str(&detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let checkpoint = detail
            .get("pre_rollback_checkpoint")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rollback operation lacks pre_rollback_checkpoint",
                )
            })?;
        let rollback_revision = detail
            .get("rollback_revision")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rollback operation lacks rollback_revision",
                )
            })?;
        let graph_documents = detail
            .get("graph_documents")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?
            .unwrap_or_default();
        Ok(Some(ChangesetRollbackState {
            changeset_id: id.to_string(),
            name,
            rollback_revision: rollback_revision.to_string(),
            checkpoint: checkpoint.to_string(),
            locked_rollback_ms: 0,
            graph_documents,
        }))
    }
}
