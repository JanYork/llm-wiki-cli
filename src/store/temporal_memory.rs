fn create_temporal_memory_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(
        "CREATE TABLE memory_events(
            id TEXT PRIMARY KEY CHECK(TRIM(id) <> ''),
            request_id TEXT CHECK(request_id IS NULL OR TRIM(request_id) <> ''),
            fingerprint TEXT NOT NULL CHECK(LENGTH(fingerprint) = 64),
            event_type TEXT NOT NULL CHECK(TRIM(event_type) <> ''),
            context TEXT NOT NULL CHECK(TRIM(context) <> ''),
            occurred_at TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            valid_from TEXT,
            valid_until TEXT,
            pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0, 1)),
            logical_bytes INTEGER NOT NULL CHECK(logical_bytes >= 0)
        );
        CREATE UNIQUE INDEX memory_events_request_id
        ON memory_events(request_id) WHERE request_id IS NOT NULL;
        CREATE INDEX memory_events_context
        ON memory_events(event_type, context, occurred_at DESC, id);
        CREATE INDEX memory_events_retention
        ON memory_events(occurred_at, id);

        CREATE TABLE memory_fragments(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK(kind IN (
                'observed', 'decision', 'constraint', 'learned', 'unresolved', 'outcome'
            )),
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            value TEXT NOT NULL CHECK(TRIM(value) <> ''),
            PRIMARY KEY(event_id, kind, ordinal)
        );

        CREATE TABLE memory_changes(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            subject TEXT NOT NULL CHECK(TRIM(subject) <> ''),
            before_value TEXT,
            after_value TEXT,
            reason TEXT,
            PRIMARY KEY(event_id, ordinal)
        );

        CREATE TABLE memory_evidence(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            reference TEXT NOT NULL CHECK(TRIM(reference) <> ''),
            excerpt TEXT,
            PRIMARY KEY(event_id, ordinal)
        );

        CREATE TABLE memory_relations(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            relation_type TEXT NOT NULL CHECK(relation_type IN (
                'supersedes', 'contradicts', 'resolves', 'supports', 'related'
            )),
            target_event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            basis TEXT,
            PRIMARY KEY(event_id, ordinal)
        );
        CREATE INDEX memory_relations_target
        ON memory_relations(target_event_id, relation_type, event_id);

        CREATE TABLE memory_feedback(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            signal TEXT NOT NULL CHECK(signal IN ('useful', 'not-useful')),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );
        CREATE INDEX memory_feedback_event
        ON memory_feedback(event_id, created_at, id);

        CREATE TABLE memory_hint_state(
            candidate_key TEXT PRIMARY KEY CHECK(TRIM(candidate_key) <> ''),
            hint_type TEXT NOT NULL CHECK(TRIM(hint_type) <> ''),
            last_emitted_at TEXT NOT NULL,
            next_eligible_at TEXT NOT NULL
        );

        CREATE TABLE memory_state(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            record_attempts INTEGER NOT NULL DEFAULT 0 CHECK(record_attempts >= 0),
            inserted_events INTEGER NOT NULL DEFAULT 0 CHECK(inserted_events >= 0),
            idempotent_replays INTEGER NOT NULL DEFAULT 0 CHECK(idempotent_replays >= 0),
            feedback_useful INTEGER NOT NULL DEFAULT 0 CHECK(feedback_useful >= 0),
            feedback_not_useful INTEGER NOT NULL DEFAULT 0 CHECK(feedback_not_useful >= 0),
            age_evictions INTEGER NOT NULL DEFAULT 0 CHECK(age_evictions >= 0),
            capacity_evictions INTEGER NOT NULL DEFAULT 0 CHECK(capacity_evictions >= 0),
            event_count INTEGER NOT NULL DEFAULT 0 CHECK(event_count >= 0),
            logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0)
        );
        INSERT INTO memory_state(id) VALUES (1);

        CREATE VIRTUAL TABLE memory_fts USING fts5(
            event_id UNINDEXED,
            event_type,
            context_terms,
            content_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );"
    ))?;
    Ok(())
}

fn migrate_temporal_memory(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current == USER_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if current != TAGS_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {USER_VERSION}"),
        ));
    }
    create_temporal_memory_schema(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{USER_VERSION} temporal memory schema: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![USER_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{USER_VERSION} temporal memory migration: {error}"),
        )
    })
}
