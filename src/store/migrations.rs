fn configure_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    enable_wal(conn)?;
    enable_persistent_wal(conn, MAIN_DB)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(())
}

fn configure_read_only_connection(conn: &Connection, timeout: Duration) -> Result<()> {
    conn.busy_timeout(timeout)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

fn enable_persistent_wal(conn: &Connection, database_name: &CStr) -> Result<()> {
    let mut enabled = 1_i32;
    // SAFETY: the connection and static database-name pointer remain valid for this call.
    let rc = unsafe {
        ffi::sqlite3_file_control(
            conn.handle(),
            database_name.as_ptr(),
            ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&mut enabled as *mut i32).cast(),
        )
    };
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(AppError::new(
            "database_error",
            format!("failed to enable persistent WAL mode (sqlite rc {rc})"),
        ))
    }
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => {
                return Err(AppError::new(
                    "database_error",
                    format!("SQLite refused WAL mode and returned {mode}"),
                ));
            }
            Err(error)
                if started.elapsed() < BUSY_TIMEOUT
                    && matches!(
                        &error,
                        rusqlite::Error::SqliteFailure(inner, _)
                            if matches!(
                                inner.code,
                                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                            )
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn prepare_store(
    conn: &mut Connection,
    allow_create: bool,
    _migration_progress: Option<&mut MigrationProgress<'_>>,
) -> Result<bool> {
    let mut version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        return if allow_create {
            bootstrap_schema(conn)
        } else {
            Err(AppError::new(
                "unsupported_store_version",
                "wiki database has no recognized schema; run `lwc init` in a new directory",
            ))
        };
    }
    if !(1..=USER_VERSION).contains(&version) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    if version < SEARCH_INDEX_VERSION {
        migrate_search_index(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == SEARCH_INDEX_VERSION {
        migrate_ingest_workflow(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == INGEST_WORKFLOW_VERSION {
        migrate_compound_wiki(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == COMPOUND_WIKI_VERSION {
        migrate_page_provenance(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == PAGE_PROVENANCE_VERSION {
        migrate_source_path_revisions(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == SOURCE_PATH_REVISIONS_VERSION {
        migrate_retrieval_weighting(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == RETRIEVAL_WEIGHTING_VERSION {
        migrate_changesets(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if matches!(version, CHANGESETS_VERSION | 11) {
        migrate_external_graph_schema(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == EXTERNAL_GRAPH_VERSION {
        migrate_tags(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == TAGS_VERSION {
        migrate_temporal_memory(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == TEMPORAL_MEMORY_VERSION {
        migrate_agent_state_v15(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == AGENT_STATE_VERSION {
        migrate_todo_features_v16(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == TODO_FEATURES_VERSION {
        migrate_structured_span_index_v17(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == STRUCTURED_SPAN_INDEX_VERSION {
        migrate_agent_tracking_v18(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version != USER_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    validate_store(conn)?;
    Ok(false)
}

fn migrate_agent_state_v15(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (AGENT_STATE_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if current != TEMPORAL_MEMORY_VERSION {
        return Err(AppError::new("unsupported_store_version", format!("cannot migrate wiki database version {current} to {USER_VERSION}")));
    }
    create_todo_schema(&tx).map_err(|error| AppError::new("store_migration_failed", format!("failed to prepare v{AGENT_STATE_VERSION} Todo schema: {error}")))?;
    create_plan_schema(&tx).map_err(|error| AppError::new("store_migration_failed", format!("failed to prepare v{AGENT_STATE_VERSION} Plan schema: {error}")))?;
    tx.execute("INSERT INTO meta(key,value) VALUES ('format_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [AGENT_STATE_VERSION.to_string()])?;
    tx.pragma_update(None, "user_version", AGENT_STATE_VERSION)?;
    tx.commit().map_err(|error| AppError::new("store_migration_failed", format!("failed to commit v{AGENT_STATE_VERSION} Todo and Plan migration: {error}")))
}

fn migrate_todo_features_v16(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (TODO_FEATURES_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if current != AGENT_STATE_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {TODO_FEATURES_VERSION}"),
        ));
    }
    let has_parent: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('todo_items') WHERE name='parent_id')",
        [],
        |row| row.get(0),
    )?;
    let has_target: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('todo_items') WHERE name='target_at')",
        [],
        |row| row.get(0),
    )?;
    if !has_parent {
        tx.execute_batch(
            "ALTER TABLE todo_items ADD COLUMN parent_id TEXT REFERENCES todo_items(id) ON DELETE RESTRICT;",
        )?;
    }
    if !has_target {
        tx.execute_batch("ALTER TABLE todo_items ADD COLUMN target_at TEXT;")?;
    }
    tx.execute_batch(
        "CREATE INDEX IF NOT EXISTS todo_items_parent ON todo_items(parent_id,created_at,id);
         CREATE INDEX IF NOT EXISTS todo_items_reminder ON todo_items(state,target_at,created_at,id);",
    )?;
    tx.execute("INSERT INTO meta(key,value) VALUES ('format_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [TODO_FEATURES_VERSION.to_string()])?;
    tx.pragma_update(None, "user_version", TODO_FEATURES_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{TODO_FEATURES_VERSION} Todo migration: {error}"),
        )
    })
}

fn migrate_structured_span_index_v17(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current == STRUCTURED_SPAN_INDEX_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if current != TODO_FEATURES_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "cannot migrate wiki database version {current} to {STRUCTURED_SPAN_INDEX_VERSION}"
            ),
        ));
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS span_fts;
         DROP TABLE IF EXISTS span_fts_data;
         DROP TABLE IF EXISTS span_fts_idx;
         DROP TABLE IF EXISTS span_fts_content;
         DROP TABLE IF EXISTS span_fts_docsize;
         DROP TABLE IF EXISTS span_fts_config;
         CREATE VIRTUAL TABLE span_fts USING fts5(
            span_id UNINDEXED, span_type UNINDEXED,
            document_type UNINDEXED, document_identifier UNINDEXED,
            title_terms, path_terms, heading_terms, body_terms,
            content='', contentless_delete=1, contentless_unindexed=1
         );",
    )
    .map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to prepare v{STRUCTURED_SPAN_INDEX_VERSION} structured span index: {error}"
            ),
        )
    })?;
    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to rebuild v{STRUCTURED_SPAN_INDEX_VERSION} structured span index: {error}"
            ),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES ('format_version',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [STRUCTURED_SPAN_INDEX_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", STRUCTURED_SPAN_INDEX_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to commit v{STRUCTURED_SPAN_INDEX_VERSION} structured span migration: {error}"
            ),
        )
    })
}

fn create_todo_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(r#"
      CREATE TABLE IF NOT EXISTS todo_items(
        id TEXT PRIMARY KEY CHECK(LENGTH(id)=32), request_id TEXT, fingerprint TEXT NOT NULL CHECK(LENGTH(fingerprint)=64),
        title TEXT NOT NULL CHECK(TRIM(title)<>''), cue TEXT, detail TEXT,
        state TEXT NOT NULL CHECK(state IN ('open','done','cancelled')), result TEXT, cancel_reason TEXT,
        revision INTEGER NOT NULL CHECK(revision>=1), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, closed_at TEXT,
        parent_id TEXT REFERENCES todo_items(id) ON DELETE RESTRICT, target_at TEXT);
      CREATE UNIQUE INDEX IF NOT EXISTS todo_items_request ON todo_items(request_id) WHERE request_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS todo_items_state_updated ON todo_items(state,updated_at DESC,id);
      CREATE TABLE IF NOT EXISTS todo_tags(todo_id TEXT NOT NULL REFERENCES todo_items(id) ON DELETE CASCADE, tag_name TEXT NOT NULL CHECK(TRIM(tag_name)<>''), PRIMARY KEY(todo_id,tag_name));
      CREATE INDEX IF NOT EXISTS todo_tags_lookup ON todo_tags(tag_name,todo_id);
      CREATE INDEX IF NOT EXISTS todo_items_parent ON todo_items(parent_id,created_at,id);
      CREATE INDEX IF NOT EXISTS todo_items_reminder ON todo_items(state,target_at,created_at,id);
      CREATE VIRTUAL TABLE IF NOT EXISTS todo_fts USING fts5(todo_id UNINDEXED,title_terms,tag_terms,cue_terms,detail_terms,content='',contentless_delete=1,contentless_unindexed=1);
    "#)?;
    Ok(())
}

fn create_plan_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(r#"
      CREATE TABLE IF NOT EXISTS plans(
        id TEXT PRIMARY KEY CHECK(LENGTH(id)=32), request_id TEXT, fingerprint TEXT NOT NULL CHECK(LENGTH(fingerprint)=64),
        title TEXT NOT NULL CHECK(TRIM(title)<>''), objective TEXT NOT NULL CHECK(TRIM(objective)<>''), done_when TEXT NOT NULL CHECK(TRIM(done_when)<>''),
        state TEXT NOT NULL CHECK(state IN ('active','completed','abandoned')), result TEXT, completion_evidence TEXT,
        done_when_checked INTEGER NOT NULL DEFAULT 0 CHECK(done_when_checked IN (0,1)), abandoned_reason TEXT,
        revision INTEGER NOT NULL CHECK(revision>=1), created_at TEXT NOT NULL, updated_at TEXT NOT NULL, closed_at TEXT);
      CREATE UNIQUE INDEX IF NOT EXISTS plans_request ON plans(request_id) WHERE request_id IS NOT NULL;
      CREATE INDEX IF NOT EXISTS plans_state_updated ON plans(state,updated_at DESC,id);
      CREATE TABLE IF NOT EXISTS plan_tags(plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE CASCADE, tag_name TEXT NOT NULL CHECK(TRIM(tag_name)<>''), PRIMARY KEY(plan_id,tag_name));
      CREATE INDEX IF NOT EXISTS plan_tags_lookup ON plan_tags(tag_name,plan_id);
      CREATE TABLE IF NOT EXISTS plan_constraints(plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE CASCADE, ordinal INTEGER NOT NULL, value TEXT NOT NULL CHECK(TRIM(value)<>''), PRIMARY KEY(plan_id,ordinal));
      CREATE TABLE IF NOT EXISTS plan_steps(plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE CASCADE, step_id TEXT NOT NULL CHECK(LENGTH(step_id)=32), ordinal INTEGER NOT NULL, title TEXT NOT NULL CHECK(TRIM(title)<>''), status TEXT NOT NULL CHECK(status IN ('pending','in_progress','blocked','completed','skipped')), verify TEXT, result TEXT, blocker TEXT, created_revision INTEGER NOT NULL, updated_revision INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(plan_id,step_id), UNIQUE(plan_id,ordinal));
      CREATE INDEX IF NOT EXISTS plan_steps_status ON plan_steps(plan_id,status,ordinal);
      CREATE TABLE IF NOT EXISTS plan_history(id INTEGER PRIMARY KEY AUTOINCREMENT, plan_id TEXT NOT NULL REFERENCES plans(id) ON DELETE CASCADE, revision INTEGER NOT NULL, action TEXT NOT NULL, reason TEXT, step_id TEXT, result TEXT, created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}));
      CREATE VIRTUAL TABLE IF NOT EXISTS plan_fts USING fts5(plan_id UNINDEXED,title_terms,tag_terms,objective_terms,constraint_terms,step_terms,content='',contentless_delete=1,contentless_unindexed=1);
    "#))?;
    Ok(())
}

fn create_agent_tracking_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS agent_plan_tracks(
          context_id TEXT PRIMARY KEY,
          plan_id TEXT NOT NULL,
          created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
          updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
          FOREIGN KEY(plan_id) REFERENCES plans(id) DEFERRABLE INITIALLY DEFERRED
        );
        CREATE INDEX IF NOT EXISTS agent_plan_tracks_plan ON agent_plan_tracks(plan_id);
        CREATE TABLE IF NOT EXISTS agent_todo_tracks(
          context_id TEXT NOT NULL,
          todo_id TEXT NOT NULL,
          created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
          PRIMARY KEY(context_id,todo_id),
          FOREIGN KEY(todo_id) REFERENCES todo_items(id) DEFERRABLE INITIALLY DEFERRED
        );
        CREATE INDEX IF NOT EXISTS agent_todo_tracks_todo ON agent_todo_tracks(todo_id);
        "#
    ))?;
    Ok(())
}

fn migrate_agent_tracking_v18(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current == AGENT_TRACKING_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if current != STRUCTURED_SPAN_INDEX_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {USER_VERSION}"),
        ));
    }
    create_agent_tracking_schema(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{AGENT_TRACKING_VERSION} Agent tracking schema: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES ('format_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [AGENT_TRACKING_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", AGENT_TRACKING_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{AGENT_TRACKING_VERSION} Agent tracking migration: {error}"),
        )
    })
}

fn prepare_store_read_only(conn: &Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        return Err(AppError::new(
            "unsupported_store_version",
            "wiki database has no recognized schema; run `lwc init` in a new directory",
        ));
    }
    if !(1..=USER_VERSION).contains(&version) || version != USER_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    validate_store_read_only(conn)
}

fn migrate_ingest_workflow(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        INGEST_WORKFLOW_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        SEARCH_INDEX_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(&format!(
        "
        CREATE TABLE ingest_jobs(
            source_id INTEGER PRIMARY KEY,
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'analyzing', 'generating', 'completed', 'failed')
            ),
            attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
            analysis TEXT,
            last_error TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
        );

        CREATE INDEX ingest_jobs_status_source
        ON ingest_jobs(status, source_id);

        INSERT INTO ingest_jobs(source_id, status, updated_at)
        SELECT id, 'pending', {TIMESTAMP_SQL}
        FROM sources;
        "
    ))?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema', ?1)",
        params![DEFAULT_SCHEMA],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('purpose', ?1)",
        params![DEFAULT_PURPOSE],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![INGEST_WORKFLOW_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", INGEST_WORKFLOW_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{INGEST_WORKFLOW_VERSION} workflow migration: {error}"),
        )
    })
}

fn migrate_compound_wiki(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        COMPOUND_WIKI_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        INGEST_WORKFLOW_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(
        "ALTER TABLE ingest_jobs
         ADD COLUMN no_derived_pages_reason TEXT;

         UPDATE sources
         SET title = origin
         WHERE title IS NULL OR TRIM(title) = '';",
    )?;
    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{COMPOUND_WIKI_VERSION} compact search index: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![COMPOUND_WIKI_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", COMPOUND_WIKI_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{COMPOUND_WIKI_VERSION} Wiki migration: {error}"),
        )
    })
}

fn migrate_page_provenance(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        PAGE_PROVENANCE_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        COMPOUND_WIKI_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(
        "CREATE TABLE page_provenance(
            page_slug TEXT NOT NULL,
            provenance TEXT NOT NULL CHECK(
                provenance IN ('user-provided', 'agent-observed', 'hypothesis')
            ),
            PRIMARY KEY(page_slug, provenance),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![PAGE_PROVENANCE_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", PAGE_PROVENANCE_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{PAGE_PROVENANCE_VERSION} provenance migration: {error}"),
        )
    })
}

fn migrate_source_path_revisions(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        SOURCE_PATH_REVISIONS_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        PAGE_PROVENANCE_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    create_source_path_revisions(&tx)?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SOURCE_PATH_REVISIONS_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", SOURCE_PATH_REVISIONS_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to commit v{SOURCE_PATH_REVISIONS_VERSION} source path migration: {error}"
            ),
        )
    })
}

fn migrate_retrieval_weighting(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        RETRIEVAL_WEIGHTING_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        SOURCE_PATH_REVISIONS_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    add_structural_navigation_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} retrieval features: {error}"),
        )
    })?;
    create_retrieval_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} retrieval state: {error}"),
        )
    })?;
    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} weighted search index: {error}"
            ),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![RETRIEVAL_WEIGHTING_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", RETRIEVAL_WEIGHTING_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{RETRIEVAL_WEIGHTING_VERSION} retrieval migration: {error}"),
        )
    })
}

fn migrate_changesets(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        CHANGESETS_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        RETRIEVAL_WEIGHTING_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    create_changeset_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{USER_VERSION} changeset state: {error}"),
        )
    })?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO meta(key, value)
         VALUES ('store_id', LOWER(HEX(RANDOMBLOB(32))));
         INSERT OR IGNORE INTO meta(key, value)
         VALUES ('store_revision', LOWER(HEX(RANDOMBLOB(32))));",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![CHANGESETS_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", CHANGESETS_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{CHANGESETS_VERSION} changeset migration: {error}"),
        )
    })
}

fn migrate_external_graph_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (EXTERNAL_GRAPH_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if !matches!(current, CHANGESETS_VERSION | 11) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {USER_VERSION}"),
        ));
    }
    tx.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS semantic_relations(
            id TEXT PRIMARY KEY,
            relation_type TEXT NOT NULL,
            from_identifier TEXT NOT NULL,
            to_identifier TEXT NOT NULL,
            confidence REAL,
            provenance TEXT NOT NULL,
            reason TEXT,
            source_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );"
    ))?;
    let has_legacy_graph: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'graph_edges')",
        [],
        |row| row.get(0),
    )?;
    if has_legacy_graph {
        tx.execute(
            "INSERT OR IGNORE INTO semantic_relations(
                id, relation_type, from_identifier, to_identifier,
                confidence, provenance, reason, source_ids_json, created_at, updated_at
             )
             SELECT edge_id, edge_type, from_node_id, to_node_id,
                    confidence, COALESCE(provenance, 'agent-observed'), reason,
                    COALESCE(json_extract(properties_json, '$.source_ids'), '[]'),
                    created_at, updated_at
             FROM graph_edges WHERE owner_type = 'manual'",
            [],
        )?;
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS graph_occurrences;
         DROP TABLE IF EXISTS graph_edges;
         DROP TABLE IF EXISTS graph_deltas;
         DROP TABLE IF EXISTS graph_generations;
         DROP TABLE IF EXISTS graph_projection_state;
         DROP TABLE IF EXISTS term_pair_contributions;
         DROP TABLE IF EXISTS term_pair_totals;
         DROP TABLE IF EXISTS document_index_state;
         DROP TABLE IF EXISTS span_fts;
         DROP TABLE IF EXISTS span_fts_data;
         DROP TABLE IF EXISTS span_fts_idx;
         DROP TABLE IF EXISTS span_fts_content;
         DROP TABLE IF EXISTS span_fts_docsize;
         DROP TABLE IF EXISTS span_fts_config;
         DROP TABLE IF EXISTS search_spans;
         DROP TABLE IF EXISTS graph_nodes;
         DELETE FROM meta WHERE key LIKE 'graph_digest_%';
         CREATE TABLE search_spans(
            span_id TEXT PRIMARY KEY,
            span_type TEXT NOT NULL CHECK(span_type IN ('passage', 'sentence')),
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL,
            parent_identifier TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            byte_start INTEGER NOT NULL,
            byte_end INTEGER NOT NULL,
            content_fingerprint TEXT NOT NULL,
            segmenter_version INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1))
         );
         CREATE INDEX search_spans_document
         ON search_spans(document_type, document_identifier, active);
         CREATE VIRTUAL TABLE span_fts USING fts5(
            span_id UNINDEXED, span_type UNINDEXED,
            document_type UNINDEXED, document_identifier UNINDEXED,
            title_terms, path_terms, heading_terms, body_terms,
            content='', contentless_delete=1, contentless_unindexed=1
         );",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![EXTERNAL_GRAPH_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", EXTERNAL_GRAPH_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to remove legacy graph schema: {error}"),
        )
    })
}

fn migrate_tags(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (TAGS_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if current != EXTERNAL_GRAPH_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {TAGS_VERSION}"),
        ));
    }
    tx.execute_batch(
        "CREATE TABLE tags(
            name TEXT PRIMARY KEY,
            autoload INTEGER NOT NULL DEFAULT 0 CHECK(autoload IN (0, 1)),
            autoload_priority INTEGER NOT NULL DEFAULT 0,
            autoload_limit INTEGER NOT NULL DEFAULT 10
                CHECK(autoload_limit BETWEEN 1 AND 100),
            autoload_max_chars INTEGER NOT NULL DEFAULT 50000
                CHECK(autoload_max_chars BETWEEN 1 AND 100000),
            reason TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE page_tags(
            tag_name TEXT NOT NULL REFERENCES tags(name) ON DELETE CASCADE,
            page_slug TEXT NOT NULL REFERENCES pages(slug) ON DELETE CASCADE,
            priority INTEGER NOT NULL DEFAULT 0,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(tag_name, page_slug)
        );
        CREATE INDEX page_tags_lookup
        ON page_tags(tag_name, priority DESC, page_slug ASC);
        CREATE INDEX page_tags_page ON page_tags(page_slug, tag_name);",
    )
    .map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{TAGS_VERSION} tag schema: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![TAGS_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", TAGS_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{TAGS_VERSION} tag migration: {error}"),
        )
    })
}

fn add_structural_navigation_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE sources ADD COLUMN structural_navigation INTEGER NOT NULL DEFAULT 0
             CHECK(structural_navigation IN (0, 1));
         ALTER TABLE pages ADD COLUMN structural_navigation INTEGER NOT NULL DEFAULT 0
             CHECK(structural_navigation IN (0, 1));
         UPDATE sources
         SET structural_navigation = CASE
             WHEN INSTR(LOWER(content), '总览文档') > 0
               OR INSTR(LOWER(content), '文档目录') > 0
               OR INSTR(LOWER(content), 'table of contents') > 0
               OR INSTR(LOWER(content), 'navigation index') > 0
               OR INSTR(LOWER(content), 'document index') > 0
             THEN 1 ELSE 0 END;
         UPDATE pages
         SET structural_navigation = CASE
             WHEN INSTR(LOWER(body), '总览文档') > 0
               OR INSTR(LOWER(body), '文档目录') > 0
               OR INSTR(LOWER(body), 'table of contents') > 0
               OR INSTR(LOWER(body), 'navigation index') > 0
               OR INSTR(LOWER(body), 'document index') > 0
             THEN 1 ELSE 0 END;",
    )?;
    Ok(())
}

fn create_source_path_revisions(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE source_path_revisions(
            tracked_path TEXT NOT NULL CHECK(TRIM(tracked_path) <> ''),
            revision INTEGER NOT NULL CHECK(revision >= 1),
            source_id INTEGER NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY(tracked_path, revision),
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE RESTRICT
        );

        CREATE INDEX source_path_revisions_source
        ON source_path_revisions(source_id, tracked_path, revision);",
    )?;
    Ok(())
}

fn create_retrieval_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(
        "CREATE TABLE retrieval_weights(
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            weight INTEGER NOT NULL CHECK(weight IN (-2, -1, 1, 2)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(target_type, target_identifier, provenance)
        );

        CREATE TABLE retrieval_feedback(
            query_fingerprint TEXT NOT NULL CHECK(LENGTH(query_fingerprint) = 64),
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            signal INTEGER NOT NULL CHECK(signal IN (-1, 1)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(query_fingerprint, target_type, target_identifier, provenance)
        );

        CREATE INDEX retrieval_feedback_target
        ON retrieval_feedback(target_type, target_identifier, query_fingerprint);"
    ))?;
    Ok(())
}

fn create_changeset_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS changesets(
            id TEXT PRIMARY KEY CHECK(LENGTH(id) = 64),
            name TEXT NOT NULL CHECK(TRIM(name) <> ''),
            status TEXT NOT NULL CHECK(status IN ('draft', 'committed', 'rolled_back')),
            base_revision TEXT NOT NULL CHECK(LENGTH(base_revision) = 64),
            base_operation_id INTEGER NOT NULL CHECK(base_operation_id >= 0),
            begin_operation_id INTEGER NOT NULL CHECK(begin_operation_id > base_operation_id),
            pre_commit_checkpoint TEXT,
            post_revision TEXT CHECK(post_revision IS NULL OR LENGTH(post_revision) = 64),
            created_at TEXT NOT NULL,
            committed_at TEXT,
            rolled_back_at TEXT
        );
        CREATE INDEX IF NOT EXISTS changesets_name_created ON changesets(name, created_at);",
    )?;
    Ok(())
}
