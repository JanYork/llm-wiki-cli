fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn attached_store_identity(conn: &Connection) -> Result<StoreIdentity> {
    let (store_id, revision) = conn.query_row(
        "SELECT
            MAX(CASE WHEN key = 'store_id' THEN value END),
            MAX(CASE WHEN key = 'store_revision' THEN value END)
         FROM candidate.meta
         WHERE key IN ('store_id', 'store_revision')",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let operation_id = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM candidate.operations",
        [],
        |row| row.get(0),
    )?;
    Ok(StoreIdentity {
        store_id: store_id.ok_or_else(|| {
            AppError::new("changeset_corrupt", "draft store_id metadata is missing")
        })?,
        revision: revision.ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "draft store_revision metadata is missing",
            )
        })?,
        operation_id,
    })
}

fn validate_changeset_table_inventory(conn: &Connection, schema: &str) -> Result<()> {
    const TABLES: [&str; 28] = [
        "changesets",
        "ingest_jobs",
        "links",
        "meta",
        "operations",
        "page_provenance",
        "page_sources",
        "page_tags",
        "pages",
        "retrieval_feedback",
        "retrieval_weights",
        "search_fts",
        "search_fts_config",
        "search_fts_content",
        "search_fts_data",
        "search_fts_docsize",
        "search_fts_idx",
        "search_spans",
        "semantic_relations",
        "source_path_revisions",
        "sources",
        "span_fts",
        "span_fts_config",
        "span_fts_content",
        "span_fts_data",
        "span_fts_docsize",
        "span_fts_idx",
        "tags",
    ];
    let sql = format!(
        "SELECT name FROM {schema}.sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
         ORDER BY name"
    );
    let mut statement = conn.prepare(&sql)?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if actual != TABLES {
        return Err(AppError::new(
            "changeset_corrupt",
            format!("{schema} Wiki table inventory does not match store format v{USER_VERSION}"),
        ));
    }
    Ok(())
}

type ChangedSearchDocuments = (Vec<(i64, Option<i64>)>, Vec<(String, Option<i64>)>);

fn changed_search_documents(
    conn: &Connection,
    source_schema: &str,
) -> Result<ChangedSearchDocuments> {
    if source_schema != "candidate" {
        return Err(AppError::new(
            "changeset_corrupt",
            "unsupported attached changeset schema",
        ));
    }
    let mut source_statement = conn.prepare(
        "WITH changed(id) AS (
             SELECT live.id FROM main.sources live
             WHERE NOT EXISTS (
                 SELECT 1 FROM candidate.sources draft
                 WHERE draft.id = live.id
                   AND draft.content_hash IS live.content_hash
                   AND draft.title IS live.title
                   AND draft.origin IS live.origin
                   AND draft.content IS live.content
                   AND draft.structural_navigation IS live.structural_navigation
                   AND draft.created_at IS live.created_at
             )
             UNION
             SELECT draft.id FROM candidate.sources draft
             WHERE NOT EXISTS (
                 SELECT 1 FROM main.sources live
                 WHERE live.id = draft.id
                   AND live.content_hash IS draft.content_hash
                   AND live.title IS draft.title
                   AND live.origin IS draft.origin
                   AND live.content IS draft.content
                   AND live.structural_navigation IS draft.structural_navigation
                   AND live.created_at IS draft.created_at
             )
         )
         SELECT changed.id,
                (SELECT rowid FROM candidate.search_fts
                 WHERE doc_type = 'source'
                   AND identifier = CAST(changed.id AS TEXT)
                 LIMIT 1)
         FROM changed ORDER BY changed.id",
    )?;
    let sources = source_statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut page_statement = conn.prepare(
        "WITH changed(slug) AS (
             SELECT live.slug FROM main.pages live
             WHERE NOT EXISTS (
                 SELECT 1 FROM candidate.pages draft
                 WHERE draft.slug = live.slug
                   AND draft.title IS live.title
                   AND draft.kind IS live.kind
                   AND draft.summary IS live.summary
                   AND draft.body IS live.body
                   AND draft.structural_navigation IS live.structural_navigation
                   AND draft.created_at IS live.created_at
                   AND draft.updated_at IS live.updated_at
             )
             UNION
             SELECT draft.slug FROM candidate.pages draft
             WHERE NOT EXISTS (
                 SELECT 1 FROM main.pages live
                 WHERE live.slug = draft.slug
                   AND live.title IS draft.title
                   AND live.kind IS draft.kind
                   AND live.summary IS draft.summary
                   AND live.body IS draft.body
                   AND live.structural_navigation IS draft.structural_navigation
                   AND live.created_at IS draft.created_at
                   AND live.updated_at IS draft.updated_at
             )
         )
         SELECT changed.slug,
                (SELECT rowid FROM candidate.search_fts
                 WHERE doc_type = 'page' AND identifier = changed.slug
                 LIMIT 1)
         FROM changed ORDER BY changed.slug",
    )?;
    let pages = page_statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((sources, pages))
}

fn refresh_changed_search_documents(
    tx: &Transaction<'_>,
    (sources, pages): ChangedSearchDocuments,
) -> Result<()> {
    for (source_id, _) in &sources {
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
            params![source_id.to_string()],
        )?;
    }
    for (slug, _) in &pages {
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
            params![slug],
        )?;
    }

    for (source_id, rowid) in sources {
        let source = tx
            .query_row(
                "SELECT title, origin, content FROM sources WHERE id = ?1",
                params![source_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        match (source, rowid) {
            (Some((title, origin, content)), Some(rowid)) => index_source(
                tx,
                Some(rowid),
                source_id,
                title.as_deref(),
                &origin,
                &content,
            )?,
            (None, None) => {}
            _ => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "candidate source and search index do not match",
                ));
            }
        }
    }
    for (slug, rowid) in pages {
        let page = tx
            .query_row(
                "SELECT title, summary, body FROM pages WHERE slug = ?1",
                params![&slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        match (page, rowid) {
            (Some((title, summary, body)), Some(rowid)) => {
                index_page(tx, Some(rowid), &slug, &title, summary.as_deref(), &body)?;
            }
            (None, None) => {}
            _ => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "candidate page and search index do not match",
                ));
            }
        }
    }
    Ok(())
}

fn replace_main_from_attached(tx: &Transaction<'_>, source_schema: &str) -> Result<()> {
    if source_schema != "candidate" {
        return Err(AppError::new(
            "changeset_corrupt",
            "unsupported attached changeset schema",
        ));
    }
    tx.execute_batch(
        "DELETE FROM semantic_relations;
         DELETE FROM search_spans;
         DELETE FROM page_tags;
         DELETE FROM tags;
         DELETE FROM page_sources;
         DELETE FROM page_provenance;
         DELETE FROM links;
         DELETE FROM ingest_jobs;
         DELETE FROM source_path_revisions;
         DELETE FROM retrieval_weights;
         DELETE FROM retrieval_feedback;
         DELETE FROM changesets;
         DELETE FROM operations;
         DELETE FROM pages;
         DELETE FROM sources;
         DELETE FROM meta;

         INSERT INTO meta(key, value)
         SELECT key, value FROM candidate.meta;
         DELETE FROM meta WHERE key = 'changeset_frozen';
         INSERT INTO sources(
             id, content_hash, title, origin, content, structural_navigation, created_at
         ) SELECT
             id, content_hash, title, origin, content, structural_navigation, created_at
           FROM candidate.sources;
         INSERT INTO pages(
             slug, title, kind, summary, body, structural_navigation, created_at, updated_at
         ) SELECT
             slug, title, kind, summary, body, structural_navigation, created_at, updated_at
           FROM candidate.pages;",
    )
    .map_err(changeset_copy_error)?;
    changeset_test_fault("mid_copy")?;
    tx.execute_batch(
        "
         INSERT INTO page_sources(page_slug, source_id)
         SELECT page_slug, source_id FROM candidate.page_sources;
         INSERT INTO page_provenance(page_slug, provenance)
         SELECT page_slug, provenance FROM candidate.page_provenance;
         INSERT INTO tags(
             name, autoload, autoload_priority, autoload_limit,
             autoload_max_chars, reason, updated_at
         ) SELECT
             name, autoload, autoload_priority, autoload_limit,
             autoload_max_chars, reason, updated_at
           FROM candidate.tags;
         INSERT INTO page_tags(
             tag_name, page_slug, priority, reason, created_at, updated_at
         ) SELECT
             tag_name, page_slug, priority, reason, created_at, updated_at
           FROM candidate.page_tags;
         INSERT INTO links(from_slug, to_slug)
         SELECT from_slug, to_slug FROM candidate.links;
         INSERT INTO operations(id, action, target, detail_json, created_at)
         SELECT id, action, target, detail_json, created_at FROM candidate.operations;
         INSERT INTO ingest_jobs(
             source_id, status, attempts, analysis, last_error,
             no_derived_pages_reason, updated_at
         ) SELECT
             source_id, status, attempts, analysis, last_error,
             no_derived_pages_reason, updated_at
           FROM candidate.ingest_jobs;
         INSERT INTO source_path_revisions(tracked_path, revision, source_id, observed_at)
         SELECT tracked_path, revision, source_id, observed_at
           FROM candidate.source_path_revisions;
         INSERT INTO retrieval_weights(
             target_type, target_identifier, provenance, weight, reason, updated_at
         ) SELECT
             target_type, target_identifier, provenance, weight, reason, updated_at
           FROM candidate.retrieval_weights;
         INSERT INTO retrieval_feedback(
             query_fingerprint, target_type, target_identifier,
             provenance, signal, reason, updated_at
         ) SELECT
             query_fingerprint, target_type, target_identifier,
             provenance, signal, reason, updated_at
           FROM candidate.retrieval_feedback;
         INSERT INTO changesets(
             id, name, status, base_revision, base_operation_id, begin_operation_id,
             pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at
         ) SELECT
             id, name, status, base_revision, base_operation_id, begin_operation_id,
             pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at
           FROM candidate.changesets;
         INSERT INTO semantic_relations(
             id, relation_type, from_identifier, to_identifier,
             confidence, provenance, reason, source_ids_json, created_at, updated_at
         ) SELECT
             id, relation_type, from_identifier, to_identifier,
             confidence, provenance, reason, source_ids_json, created_at, updated_at
           FROM candidate.semantic_relations;
         INSERT INTO search_spans(
             span_id, span_type, document_type, document_identifier,
             parent_identifier, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, active
         ) SELECT
             span_id, span_type, document_type, document_identifier,
             parent_identifier, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, active
           FROM candidate.search_spans;",
    )
    .map_err(changeset_copy_error)?;
    rebuild_span_index(tx)?;
    Ok(())
}

fn rebuild_span_index(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM span_fts", [])?;
    let rows = {
        let mut statement = tx.prepare(
            "SELECT n.span_id, n.span_type, n.document_type,
                    n.document_identifier, n.byte_start, n.byte_end,
                    CASE n.document_type WHEN 'page' THEN p.body ELSE s.content END,
                    CASE n.document_type
                        WHEN 'page' THEN p.title
                        ELSE COALESCE(s.title, s.origin)
                    END AS title,
                    CASE n.document_type
                        WHEN 'page' THEN p.slug
                        ELSE s.origin
                    END AS path
             FROM search_spans n
             LEFT JOIN pages p
               ON n.document_type = 'page' AND p.slug = n.document_identifier
             LEFT JOIN sources s
               ON n.document_type = 'source'
              AND s.id = CAST(n.document_identifier AS INTEGER)
             WHERE n.active = 1
             ORDER BY n.document_type, n.document_identifier, n.span_type, n.ordinal, n.span_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (node_id, node_type, document_type, document_identifier, start, end, body, title, path) in
        rows
    {
        let start = usize::try_from(start)
            .map_err(|_| AppError::new("changeset_corrupt", "negative search span start"))?;
        let end = usize::try_from(end)
            .map_err(|_| AppError::new("changeset_corrupt", "negative search span end"))?;
        let label = body.get(start..end).ok_or_else(|| {
            AppError::new("changeset_corrupt", "search span has an invalid byte range")
        })?;
        tx.execute(
            "INSERT INTO span_fts(
                span_id, span_type, document_type, document_identifier,
                title_terms, path_terms, body_terms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                node_id,
                node_type,
                document_type,
                document_identifier,
                joined_terms(&title),
                joined_terms(&path),
                joined_terms(label),
            ],
        )?;
    }
    Ok(())
}

fn changeset_copy_error(error: rusqlite::Error) -> AppError {
    AppError::new(
        "changeset_corrupt",
        format!("changeset canonical copy failed: {error}"),
    )
}

fn wal_checkpoint_truncate(conn: &Connection, wait_for_readers: bool) -> Result<(i64, i64, i64)> {
    if wait_for_readers {
        return conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(Into::into);
    }
    conn.busy_timeout(Duration::ZERO)?;
    let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    });
    conn.busy_timeout(BUSY_TIMEOUT)?;
    checkpoint.map_err(Into::into)
}

fn changeset_test_fault(point: &str) -> Result<()> {
    if std::env::var("LWC_TEST_CHANGESET_FAULT").as_deref() == Ok(point) {
        return Err(AppError::new(
            "changeset_test_fault",
            format!("injected changeset fault at {point}"),
        ));
    }
    Ok(())
}

fn changeset_test_crash(point: &str) {
    if std::env::var("LWC_TEST_CHANGESET_FAULT").as_deref() == Ok(format!("crash:{point}").as_str())
    {
        std::process::abort();
    }
}

fn validate_database_integrity(conn: &Connection) -> Result<()> {
    let mut foreign_keys = conn.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        return Err(AppError::new(
            "changeset_corrupt",
            "changeset database violates a foreign key",
        ));
    }
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(AppError::new(
            "changeset_corrupt",
            format!("changeset database failed integrity_check: {integrity}"),
        ));
    }
    Ok(())
}

fn record_operation(
    tx: &Transaction<'_>,
    action: &str,
    target: &str,
    detail: &Value,
) -> Result<String> {
    if tx
        .query_row(
            "SELECT 1 FROM meta WHERE key = ?1 LIMIT 1",
            params![CHANGESET_FREEZE_KEY],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Err(AppError::new(
            "changeset_frozen",
            "changeset is frozen for commit; retry commit or discard it instead of staging more writes",
        ));
    }
    let detail_json = serde_json::to_string(detail)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    tx.execute(
        "INSERT INTO operations(action, target, detail_json) VALUES (?1, ?2, ?3)",
        params![action, target, detail_json],
    )?;
    let revision: String =
        tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(32)))", [], |row| row.get(0))?;
    let updated = tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'store_revision'",
        params![&revision],
    )?;
    if updated != 1 {
        return Err(AppError::new(
            "corrupt_store",
            "wiki store_revision metadata is missing",
        ));
    }
    Ok(revision)
}

fn read_source_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRecord> {
    Ok(SourceRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        origin: row.get(2)?,
        content_hash: row.get(3)?,
        content: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn window_source(
    mut source: SourceRecord,
    offset_chars: usize,
    max_chars: Option<usize>,
) -> Result<(SourceRecord, SourceWindow)> {
    if max_chars == Some(0) {
        return Err(AppError::new(
            "invalid_limit",
            "max-chars must be greater than zero",
        ));
    }
    let total_chars = source.content.chars().count();
    if offset_chars > total_chars {
        return Err(AppError::new(
            "invalid_offset",
            format!("offset-chars {offset_chars} exceeds source length {total_chars}"),
        ));
    }
    let requested = max_chars.unwrap_or_else(|| total_chars.saturating_sub(offset_chars));
    let content = source
        .content
        .chars()
        .skip(offset_chars)
        .take(requested)
        .collect::<String>();
    let returned_chars = content.chars().count();
    let consumed = offset_chars.saturating_add(returned_chars);
    let has_more = consumed < total_chars;
    source.content = content;
    Ok((
        source,
        SourceWindow {
            offset_chars,
            returned_chars,
            total_chars,
            next_offset_chars: has_more.then_some(consumed),
            has_more,
        },
    ))
}

fn read_source_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceSummary> {
    Ok(SourceSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        origin: row.get(2)?,
        content_hash: row.get(3)?,
        bytes: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn read_source_status_target(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceStatusTarget> {
    Ok(SourceStatusTarget {
        requested_source_id: row.get(0)?,
        tracked_path: row.get(1)?,
        head_source_id: row.get(2)?,
        head_revision: row.get(3)?,
        head_content_hash: row.get(4)?,
    })
}

fn read_ingest_job_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<IngestJobSummary> {
    Ok(IngestJobSummary {
        source_id: row.get(0)?,
        status: row.get(1)?,
        attempts: row.get(2)?,
        last_error: row.get(3)?,
        no_derived_pages_reason: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn read_operation_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    let detail_json: String = row.get(3)?;
    let detail = serde_json::from_str(&detail_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(OperationRecord {
        id: row.get(0)?,
        action: row.get(1)?,
        target: row.get(2)?,
        detail,
        created_at: row.get(4)?,
    })
}

fn read_changeset_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChangesetHistoryState> {
    Ok(ChangesetHistoryState {
        id: row.get(0)?,
        name: row.get(1)?,
        status: row.get(2)?,
        base_revision: row.get(3)?,
        base_operation_id: row.get(4)?,
        begin_operation_id: row.get(5)?,
        pre_commit_checkpoint: row.get(6)?,
        post_revision: row.get(7)?,
        created_at: row.get(8)?,
        committed_at: row.get(9)?,
        rolled_back_at: row.get(10)?,
    })
}

fn read_retrieval_adjustment(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetrievalAdjustment> {
    Ok(RetrievalAdjustment {
        target_type: row.get(0)?,
        target_identifier: row.get(1)?,
        provenance: row.get(2)?,
        weight: row.get(3)?,
        reason: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn validate_retrieval_weight(weight: i32) -> Result<()> {
    if matches!(weight, -2 | -1 | 1 | 2) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_weight",
            "weight must be one of -2, -1, 1, or 2",
        ))
    }
}

fn validate_retrieval_provenance(provenance: &str) -> Result<()> {
    if matches!(provenance, "user-provided" | "agent-observed") {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_provenance",
            "retrieval provenance must be user-provided or agent-observed",
        ))
    }
}

fn validate_nonempty_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        Err(AppError::new(
            "invalid_input",
            "retrieval adjustment reason must not be empty",
        ))
    } else {
        Ok(())
    }
}

fn normalize_retrieval_target(
    conn: &Connection,
    target_type: &str,
    identifier: &str,
) -> Result<String> {
    match target_type {
        "page" => {
            let exists = conn
                .query_row(
                    "SELECT 1 FROM pages WHERE slug = ?1",
                    params![identifier],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                Ok(identifier.to_string())
            } else {
                Err(AppError::new(
                    "page_not_found",
                    format!("page not found: {identifier}"),
                ))
            }
        }
        "source" => {
            let source_id = identifier.parse::<i64>().map_err(|_| {
                AppError::new(
                    "invalid_input",
                    format!("source identifier must be an integer: {identifier}"),
                )
            })?;
            let exists = conn
                .query_row(
                    "SELECT 1 FROM sources WHERE id = ?1",
                    params![source_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                Ok(source_id.to_string())
            } else {
                Err(AppError::new(
                    "source_not_found",
                    format!("source not found: {source_id}"),
                ))
            }
        }
        _ => Err(AppError::new(
            "invalid_input",
            "retrieval target type must be page or source",
        )),
    }
}

fn validate_ingest_status(status: &str) -> Result<()> {
    if matches!(
        status,
        "pending" | "analyzing" | "generating" | "completed" | "failed"
    ) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_ingest_status",
            format!("unsupported ingest status: {status}"),
        ))
    }
}

fn require_ingest_state(tx: &Transaction<'_>, source_id: i64, allowed: &[&str]) -> Result<()> {
    let status = tx
        .query_row(
            "SELECT status FROM ingest_jobs WHERE source_id = ?1",
            params![source_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::new(
                "ingest_job_not_found",
                format!("ingest job not found for source {source_id}"),
            )
        })?;
    if allowed.contains(&status.as_str()) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_ingest_state",
            format!(
                "source {source_id} is {status}; expected {}",
                allowed.join(" or ")
            ),
        ))
    }
}

fn claim_ingest_job(tx: &Transaction<'_>, source_id: i64) -> Result<()> {
    let updated = tx.execute(
        &format!(
            "UPDATE ingest_jobs
             SET status = 'analyzing',
                 attempts = attempts + 1,
                 last_error = NULL,
                 updated_at = {TIMESTAMP_SQL}
             WHERE source_id = ?1 AND status = 'pending'"
        ),
        params![source_id],
    )?;
    if updated != 1 {
        return Err(AppError::new(
            "invalid_ingest_state",
            format!("source {source_id} is not pending"),
        ));
    }
    record_operation(tx, "ingest_claim", &source_id.to_string(), &json!({})).map(|_| ())
}

fn load_page_mutation_base(conn: &Connection, slug: &str) -> Result<Option<PageMutationBase>> {
    let Some((title, kind, summary, body, structural_navigation, updated_at)) = conn
        .query_row(
            "SELECT title, kind, summary, body, structural_navigation, updated_at
             FROM pages WHERE slug = ?1",
            [slug],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let source_ids = {
        let mut statement = conn.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map([slug], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let provenance = {
        let mut statement = conn.prepare(
            "SELECT provenance FROM page_provenance
             WHERE page_slug = ?1
             ORDER BY CASE provenance
                 WHEN 'user-provided' THEN 0
                 WHEN 'agent-observed' THEN 1
                 WHEN 'hypothesis' THEN 2
                 ELSE 3 END",
        )?;
        statement
            .query_map([slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let links = {
        let mut statement =
            conn.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        statement
            .query_map([slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let content_fingerprint = page_content_fingerprint(
        &title,
        kind.as_deref(),
        summary.as_deref(),
        &body,
        structural_navigation,
        &source_ids,
        &provenance,
        &links,
    );
    let version_fingerprint = hash_content(&format!("{content_fingerprint}\0{updated_at}"));
    Ok(Some(PageMutationBase {
        content_fingerprint,
        version_fingerprint,
    }))
}
