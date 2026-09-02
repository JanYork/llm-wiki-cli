fn bootstrap_schema(conn: &mut Connection) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current != 0 {
        tx.commit()?;
        return Ok(false);
    }

    tx.execute_batch(&format!(
        "
        CREATE TABLE meta(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE sources(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_hash TEXT NOT NULL UNIQUE,
            title TEXT,
            origin TEXT NOT NULL,
            content TEXT NOT NULL,
            structural_navigation INTEGER NOT NULL DEFAULT 0
                CHECK(structural_navigation IN (0, 1)),
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );

        CREATE TABLE pages(
            slug TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            kind TEXT,
            summary TEXT,
            body TEXT NOT NULL,
            structural_navigation INTEGER NOT NULL DEFAULT 0
                CHECK(structural_navigation IN (0, 1)),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE page_sources(
            page_slug TEXT NOT NULL,
            source_id INTEGER NOT NULL,
            PRIMARY KEY(page_slug, source_id),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE,
            FOREIGN KEY(source_id) REFERENCES sources(id)
        );

        CREATE TABLE page_provenance(
            page_slug TEXT NOT NULL,
            provenance TEXT NOT NULL CHECK(
                provenance IN ('user-provided', 'agent-observed', 'hypothesis')
            ),
            PRIMARY KEY(page_slug, provenance),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );

        CREATE TABLE tags(
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
            tag_name TEXT NOT NULL,
            page_slug TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(tag_name, page_slug),
            FOREIGN KEY(tag_name) REFERENCES tags(name) ON DELETE CASCADE,
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );

        CREATE INDEX page_tags_lookup
        ON page_tags(tag_name, priority DESC, page_slug ASC);

        CREATE INDEX page_tags_page ON page_tags(page_slug, tag_name);

        CREATE TABLE links(
            from_slug TEXT NOT NULL,
            to_slug TEXT NOT NULL,
            PRIMARY KEY(from_slug, to_slug),
            FOREIGN KEY(from_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );

        CREATE TABLE semantic_relations(
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
        );

        CREATE TABLE operations(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );

        CREATE TABLE ingest_jobs(
            source_id INTEGER PRIMARY KEY,
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'analyzing', 'generating', 'completed', 'failed')
            ),
            attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
            analysis TEXT,
            last_error TEXT,
            no_derived_pages_reason TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
        );

        CREATE INDEX ingest_jobs_status_source
        ON ingest_jobs(status, source_id);

        CREATE TABLE source_path_revisions(
            tracked_path TEXT NOT NULL CHECK(TRIM(tracked_path) <> ''),
            revision INTEGER NOT NULL CHECK(revision >= 1),
            source_id INTEGER NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY(tracked_path, revision),
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE RESTRICT
        );

        CREATE INDEX source_path_revisions_source
        ON source_path_revisions(source_id, tracked_path, revision);

        CREATE TABLE retrieval_weights(
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
        ON retrieval_feedback(target_type, target_identifier, query_fingerprint);

        CREATE VIRTUAL TABLE search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
            path_terms,
            summary_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );

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
            span_id UNINDEXED,
            span_type UNINDEXED,
            document_type UNINDEXED,
            document_identifier UNINDEXED,
            title_terms,
            path_terms,
            heading_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );

        INSERT INTO meta(key, value) VALUES ('format_version', '{USER_VERSION}');
        INSERT INTO meta(key, value) VALUES ('tokenizer', '{TOKENIZER_ID}');
        INSERT INTO meta(key, value) VALUES ('store_id', LOWER(HEX(RANDOMBLOB(32))));
        INSERT INTO meta(key, value) VALUES ('store_revision', LOWER(HEX(RANDOMBLOB(32))));
        PRAGMA user_version = {USER_VERSION};
        "
    ))?;
    create_temporal_memory_schema(&tx)?;
    create_todo_schema(&tx)?;
    create_plan_schema(&tx)?;
    create_agent_tracking_schema(&tx)?;
    create_changeset_state(&tx)?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('schema', ?1)",
        params![DEFAULT_SCHEMA],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('purpose', ?1)",
        params![DEFAULT_PURPOSE],
    )?;
    tx.commit()?;
    Ok(true)
}

fn migrate_search_index(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (SEARCH_INDEX_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if !matches!(current, 1..=3) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {SEARCH_INDEX_VERSION}"),
        ));
    }

    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{SEARCH_INDEX_VERSION} search index: {error}"),
        )
    })?;

    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SEARCH_INDEX_VERSION.to_string()],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('tokenizer', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![TOKENIZER_ID],
    )?;
    tx.pragma_update(None, "user_version", SEARCH_INDEX_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{SEARCH_INDEX_VERSION} search migration: {error}"),
        )
    })
}

fn rebuild_search_index(tx: &Transaction<'_>) -> Result<(usize, usize)> {
    tx.execute_batch(
        "
        DROP TRIGGER IF EXISTS main.sources_ai;
        DROP TRIGGER IF EXISTS main.sources_au;
        DROP TRIGGER IF EXISTS main.sources_ad;
        DROP TRIGGER IF EXISTS main.pages_ai;
        DROP TRIGGER IF EXISTS main.pages_au;
        DROP TRIGGER IF EXISTS main.pages_ad;
        DROP TABLE IF EXISTS main.source_fts;
        DROP TABLE IF EXISTS main.page_fts;
        DROP TABLE IF EXISTS main.search_fts;
        DROP TABLE IF EXISTS main.search_fts_data;
        DROP TABLE IF EXISTS main.search_fts_idx;
        DROP TABLE IF EXISTS main.search_fts_content;
        DROP TABLE IF EXISTS main.search_fts_docsize;
        DROP TABLE IF EXISTS main.search_fts_config;
        CREATE VIRTUAL TABLE main.search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
            path_terms,
            summary_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );
        ",
    )?;

    let mut source_count = 0usize;
    {
        let mut statement =
            tx.prepare("SELECT id, title, origin, content FROM sources ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, origin, content) = row?;
            index_source(tx, None, id, title.as_deref(), &origin, &content)?;
            source_count += 1;
        }
    }

    let mut page_count = 0usize;
    {
        let mut statement =
            tx.prepare("SELECT slug, title, summary, body FROM pages ORDER BY slug")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (slug, title, summary, body) = row?;
            index_page(tx, None, &slug, &title, summary.as_deref(), &body)?;
            page_count += 1;
        }
    }
    Ok((source_count, page_count))
}

fn validate_store_read_only(conn: &Connection) -> Result<()> {
    let essential_tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'table' AND name IN (
            'meta', 'sources', 'pages', 'page_sources', 'page_provenance', 'links',
            'operations', 'ingest_jobs', 'source_path_revisions', 'retrieval_weights',
            'retrieval_feedback', 'changesets', 'search_fts', 'search_spans',
            'span_fts', 'semantic_relations', 'memory_events', 'memory_fragments',
            'memory_changes', 'memory_evidence', 'memory_relations', 'memory_feedback',
            'memory_hint_state', 'memory_state', 'memory_fts',
            'todo_items', 'todo_tags', 'todo_fts', 'plans', 'plan_tags',
            'plan_constraints', 'plan_steps', 'plan_history', 'plan_fts'
         )",
        [],
        |row| row.get(0),
    )?;
    if essential_tables != 34 {
        return Err(AppError::new(
            "corrupt_store",
            "wiki database schema is incomplete",
        ));
    }
    let metadata = conn
        .query_row(
            "SELECT
                MAX(CASE WHEN key = 'format_version' THEN value END),
                MAX(CASE WHEN key = 'tokenizer' THEN value END),
                MAX(CASE WHEN key = 'store_id' THEN value END),
                MAX(CASE WHEN key = 'store_revision' THEN value END)
             FROM meta
             WHERE key IN ('format_version', 'tokenizer', 'store_id', 'store_revision')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .unwrap_or_default();
    if metadata.0.as_deref() != Some(USER_VERSION.to_string().as_str()) {
        return Err(AppError::new(
            "corrupt_store",
            "wiki format metadata does not match the store version",
        ));
    }
    if metadata.1.as_deref() != Some(TOKENIZER_ID) {
        return Err(AppError::new(
            "incompatible_search_index",
            "wiki search tokenizer is incompatible",
        ));
    }
    for value in [metadata.2, metadata.3] {
        if !value.as_deref().is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(AppError::new(
                "corrupt_store",
                "wiki store identity metadata is missing or invalid",
            ));
        }
    }
    Ok(())
}

fn validate_store(conn: &Connection) -> Result<()> {
    for sql in [
        "SELECT key, value FROM meta LIMIT 0",
        "SELECT id, content_hash, title, origin, content, structural_navigation, created_at FROM sources LIMIT 0",
        "SELECT slug, title, kind, summary, body, structural_navigation, created_at, updated_at FROM pages LIMIT 0",
        "SELECT page_slug, source_id FROM page_sources LIMIT 0",
        "SELECT page_slug, provenance FROM page_provenance LIMIT 0",
        "SELECT name, autoload, autoload_priority, autoload_limit, autoload_max_chars, reason, updated_at FROM tags LIMIT 0",
        "SELECT tag_name, page_slug, priority, reason, created_at, updated_at FROM page_tags LIMIT 0",
        "SELECT from_slug, to_slug FROM links LIMIT 0",
        "SELECT action, target, detail_json, created_at FROM operations LIMIT 0",
        "SELECT source_id, status, attempts, analysis, last_error, no_derived_pages_reason, updated_at FROM ingest_jobs LIMIT 0",
        "SELECT tracked_path, revision, source_id, observed_at FROM source_path_revisions LIMIT 0",
        "SELECT target_type, target_identifier, provenance, weight, reason, updated_at FROM retrieval_weights LIMIT 0",
        "SELECT query_fingerprint, target_type, target_identifier, provenance, signal, reason, updated_at FROM retrieval_feedback LIMIT 0",
        "SELECT id, name, status, base_revision, base_operation_id, begin_operation_id, pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at FROM changesets LIMIT 0",
        "SELECT rowid, doc_type, identifier, title_terms, path_terms, summary_terms, body_terms FROM search_fts LIMIT 0",
        "SELECT span_id, span_type, document_type, document_identifier, parent_identifier, ordinal, byte_start, byte_end, content_fingerprint, segmenter_version, active FROM search_spans LIMIT 0",
        "SELECT rowid, span_id, span_type, document_type, document_identifier, title_terms, path_terms, heading_terms, body_terms FROM span_fts LIMIT 0",
        "SELECT id, relation_type, from_identifier, to_identifier, confidence, provenance, reason, source_ids_json, created_at, updated_at FROM semantic_relations LIMIT 0",
        "SELECT id, request_id, fingerprint, event_type, context, occurred_at, recorded_at, valid_from, valid_until, pinned, logical_bytes FROM memory_events LIMIT 0",
        "SELECT event_id, kind, ordinal, value FROM memory_fragments LIMIT 0",
        "SELECT event_id, ordinal, subject, before_value, after_value, reason FROM memory_changes LIMIT 0",
        "SELECT event_id, ordinal, reference, excerpt FROM memory_evidence LIMIT 0",
        "SELECT event_id, ordinal, relation_type, target_event_id, basis FROM memory_relations LIMIT 0",
        "SELECT id, event_id, signal, reason, created_at FROM memory_feedback LIMIT 0",
        "SELECT candidate_key, hint_type, last_emitted_at, next_eligible_at FROM memory_hint_state LIMIT 0",
        "SELECT id, record_attempts, inserted_events, idempotent_replays, feedback_useful, feedback_not_useful, age_evictions, capacity_evictions, event_count, logical_bytes FROM memory_state LIMIT 0",
        "SELECT rowid, event_id, event_type, context_terms, content_terms FROM memory_fts LIMIT 0",
        "SELECT id, request_id, fingerprint, title, cue, detail, state, result, cancel_reason, revision, created_at, updated_at, closed_at, parent_id, target_at FROM todo_items LIMIT 0",
        "SELECT todo_id, tag_name FROM todo_tags LIMIT 0",
        "SELECT rowid, todo_id, title_terms, tag_terms, cue_terms, detail_terms FROM todo_fts LIMIT 0",
        "SELECT id, request_id, fingerprint, title, objective, done_when, state, result, completion_evidence, done_when_checked, abandoned_reason, revision, created_at, updated_at, closed_at FROM plans LIMIT 0",
        "SELECT plan_id, tag_name FROM plan_tags LIMIT 0",
        "SELECT plan_id, ordinal, value FROM plan_constraints LIMIT 0",
        "SELECT plan_id, step_id, ordinal, title, status, verify, result, blocker, created_revision, updated_revision, created_at, updated_at FROM plan_steps LIMIT 0",
        "SELECT id, plan_id, revision, action, reason, step_id, result, created_at FROM plan_history LIMIT 0",
        "SELECT rowid, plan_id, title_terms, tag_terms, objective_terms, constraint_terms, step_terms FROM plan_fts LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|error| {
            AppError::new(
                "corrupt_store",
                format!("wiki database schema is incomplete: {error}"),
            )
        })?;
    }
    let metadata = conn
        .query_row(
            "SELECT
                MAX(CASE WHEN key = 'format_version' THEN value END),
                MAX(CASE WHEN key = 'tokenizer' THEN value END),
                MAX(CASE WHEN key = 'store_id' THEN value END),
                MAX(CASE WHEN key = 'store_revision' THEN value END)
             FROM meta
             WHERE key IN ('format_version', 'tokenizer', 'store_id', 'store_revision')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .unwrap_or_default();
    let expected_format = USER_VERSION.to_string();
    if metadata.0.as_deref() != Some(expected_format.as_str()) {
        return Err(AppError::new(
            "corrupt_store",
            format!(
                "wiki format metadata is {:?}; expected {USER_VERSION}",
                metadata.0.as_deref().unwrap_or("missing")
            ),
        ));
    }
    if metadata.1.as_deref() != Some(TOKENIZER_ID) {
        return Err(AppError::new(
            "incompatible_search_index",
            format!(
                "wiki search index uses tokenizer {:?}; expected {TOKENIZER_ID}",
                metadata.1.as_deref().unwrap_or("unknown")
            ),
        ));
    }
    for (key, value) in [("store_id", metadata.2), ("store_revision", metadata.3)] {
        let valid = value.as_deref().is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if !valid {
            return Err(AppError::new(
                "corrupt_store",
                format!("wiki {key} is missing or invalid"),
            ));
        }
    }
    Ok(())
}
