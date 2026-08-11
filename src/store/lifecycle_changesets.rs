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
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_only_connection(&conn)?;
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
            pages.push(SparsePageInverse {
                slug,
                before,
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
            let before = load_sparse_source_snapshot(&self.conn, source_id)?;
            if before.is_some() {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("source identifier {source_id} was allocated by another write"),
                )
                .with_details(json!({"entity_type": "source", "identifier": source_id})));
            }
            let after = load_sparse_source_snapshot(&draft.conn, source_id)?.ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    format!("staged source {source_id} is missing"),
                )
            })?;
            sources.push(SparseSourceInverse {
                source_id,
                before,
                after_fingerprint: sparse_source_fingerprint(&after),
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
        let pages = inverse
            .payload
            .pages
            .iter()
            .map(|page| {
                let before = load_sparse_page_snapshot(&self.conn, &page.slug)?;
                Ok(SparsePageInverse {
                    slug: page.slug.clone(),
                    before,
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
                let before = load_sparse_source_snapshot(&self.conn, entry.source_id)?;
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
        Ok(Some(ChangesetRollbackState {
            changeset_id: id.to_string(),
            name,
            rollback_revision: rollback_revision.to_string(),
            checkpoint: checkpoint.to_string(),
            locked_rollback_ms: 0,
        }))
    }
}
