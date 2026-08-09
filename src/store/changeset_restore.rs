fn validate_sparse_changeset_operations(conn: &Connection) -> Result<()> {
    let begin_operation_id = conn
        .query_row(
            "SELECT begin_operation_id FROM changesets
             WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(begin_operation_id) = begin_operation_id else {
        return Ok(());
    };
    let mut statement = conn.prepare("SELECT action FROM operations WHERE id > ?1 ORDER BY id")?;
    let actions = statement
        .query_map(params![begin_operation_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    validate_sparse_operation_actions(actions.iter().map(String::as_str))
}

fn validate_sparse_operation_actions<'a>(actions: impl IntoIterator<Item = &'a str>) -> Result<()> {
    for action in actions {
        if !matches!(
            action,
            "source_add"
                | "ingest_claim"
                | "ingest_analyze"
                | "ingest_complete"
                | "ingest_fail"
                | "ingest_retry"
                | "page_put"
                | "page_remove"
                | "schema_set"
                | "purpose_set"
                | "search"
        ) {
            return Err(AppError::new(
                "changeset_sparse_unsupported",
                format!("{action} does not yet have an exact sparse Changeset patch"),
            )
            .with_details(json!({
                "action": action,
                "mutated": false,
                "reason": "refusing to report a partial Changeset commit",
            })));
        }
    }
    Ok(())
}

fn merge_sparse_sources(
    tx: &Transaction<'_>,
    operations: &[(String, String, String)],
) -> Result<()> {
    let source_ids = operations
        .iter()
        .filter(|(action, _, _)| action == "source_add")
        .filter_map(|(_, _, detail)| serde_json::from_str::<Value>(detail).ok())
        .filter_map(|detail| detail.get("source_id").and_then(Value::as_i64))
        .collect::<BTreeSet<_>>();
    for source_id in source_ids {
        let source = tx
            .query_row(
                "SELECT content_hash, title, origin, content, structural_navigation, created_at
                 FROM candidate.sources WHERE id = ?1",
                params![source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, bool>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    format!("staged source {source_id} is missing"),
                )
            })?;
        let collision: Option<String> = tx
            .query_row(
                "SELECT content_hash FROM sources WHERE id = ?1",
                params![source_id],
                |row| row.get(0),
            )
            .optional()?;
        if collision.as_deref().is_some_and(|hash| hash != source.0) {
            return Err(AppError::new(
                "changeset_conflict",
                format!("source identifier {source_id} was allocated by another write"),
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO sources(
                    id, content_hash, title, origin, content,
                    structural_navigation, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                source_id, source.0, source.1, source.2, source.3, source.4, source.5
            ],
        )?;
        let resolved_id: i64 = tx.query_row(
            "SELECT id FROM sources WHERE content_hash = ?1",
            params![&source.0],
            |row| row.get(0),
        )?;
        if resolved_id != source_id {
            return Err(AppError::new(
                "changeset_conflict",
                "the staged source content was added concurrently under another identifier",
            )
            .with_details(json!({
                "entity_type": "source",
                "identifier": source_id,
                "live_identifier": resolved_id,
            })));
        }
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO ingest_jobs(source_id, status, updated_at)
                 VALUES (?1, 'pending', {TIMESTAMP_SQL})"
            ),
            params![source_id],
        )?;
        tx.execute(
            "INSERT INTO ingest_jobs(
                source_id, status, attempts, analysis, last_error,
                no_derived_pages_reason, updated_at
             ) SELECT source_id, status, attempts, analysis, last_error,
                      no_derived_pages_reason, updated_at
               FROM candidate.ingest_jobs WHERE source_id = ?1
             ON CONFLICT(source_id) DO UPDATE SET
                status = excluded.status, attempts = excluded.attempts,
                analysis = excluded.analysis, last_error = excluded.last_error,
                no_derived_pages_reason = excluded.no_derived_pages_reason,
                updated_at = excluded.updated_at",
            params![source_id],
        )?;
        let paths = {
            let mut statement = tx.prepare(
                "SELECT tracked_path, revision, observed_at
                 FROM candidate.source_path_revisions
                 WHERE source_id = ?1 ORDER BY tracked_path, revision",
            )?;
            statement
                .query_map(params![source_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (tracked_path, revision, observed_at) in paths {
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT source_id FROM source_path_revisions
                     WHERE tracked_path = ?1 AND revision = ?2",
                    params![&tracked_path, revision],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.is_some_and(|existing| existing != source_id) {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("source path {tracked_path} advanced while the changeset was open"),
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO source_path_revisions(
                    tracked_path, revision, source_id, observed_at
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![tracked_path, revision, source_id, observed_at],
            )?;
        }
        index_source(
            tx,
            None,
            source_id,
            source.1.as_deref(),
            &source.2,
            &source.3,
        )?;
    }
    Ok(())
}

fn merge_sparse_page(tx: &Transaction<'_>, slug: &str) -> Result<()> {
    let candidate = tx
        .query_row(
            "SELECT title, kind, summary, body, structural_navigation, created_at
             FROM candidate.pages WHERE slug = ?1",
            params![slug],
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
        .optional()?;
    let before = load_page_mutation_base(tx, slug)?;
    let Some((title, kind, summary, body, structural_navigation, created_at)) = candidate else {
        if before.is_some() {
            tx.execute(
                "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
                params![slug],
            )?;
            tx.execute("DELETE FROM pages WHERE slug = ?1", params![slug])?;
        }
        return Ok(());
    };
    tx.execute(
        &format!(
            "INSERT INTO pages(
                slug, title, kind, summary, body, structural_navigation,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, {TIMESTAMP_SQL})
             ON CONFLICT(slug) DO UPDATE SET
                title = excluded.title, kind = excluded.kind,
                summary = excluded.summary, body = excluded.body,
                structural_navigation = excluded.structural_navigation,
                updated_at = excluded.updated_at"
        ),
        params![
            slug,
            &title,
            kind.as_deref(),
            summary.as_deref(),
            &body,
            structural_navigation,
            created_at,
        ],
    )?;
    tx.execute(
        "DELETE FROM page_sources WHERE page_slug = ?1",
        params![slug],
    )?;
    tx.execute(
        "INSERT INTO page_sources(page_slug, source_id)
         SELECT page_slug, source_id FROM candidate.page_sources WHERE page_slug = ?1",
        params![slug],
    )?;
    tx.execute(
        "DELETE FROM page_provenance WHERE page_slug = ?1",
        params![slug],
    )?;
    tx.execute(
        "INSERT INTO page_provenance(page_slug, provenance)
         SELECT page_slug, provenance FROM candidate.page_provenance WHERE page_slug = ?1",
        params![slug],
    )?;
    tx.execute("DELETE FROM links WHERE from_slug = ?1", params![slug])?;
    tx.execute(
        "INSERT INTO links(from_slug, to_slug)
         SELECT from_slug, to_slug FROM candidate.links WHERE from_slug = ?1",
        params![slug],
    )?;
    index_page(tx, None, slug, &title, summary.as_deref(), &body)?;
    Ok(())
}

fn rollback_sparse_changeset(
    conn: &mut Connection,
    path: &Path,
    input: &ChangesetRollbackInput,
) -> Result<ChangesetRollbackState> {
    let inverse = load_sparse_inverse(path)?;
    if inverse.payload.store_id != input.store_id
        || inverse.payload.changeset_id != input.history.id
    {
        return Err(AppError::new(
            "changeset_corrupt",
            "sparse inverse patch belongs to another Wiki or changeset",
        ));
    }
    let expected_post_revision = input.history.post_revision.as_deref().ok_or_else(|| {
        AppError::new(
            "changeset_corrupt",
            "committed changeset has no post revision",
        )
    })?;
    let pre_commit_checkpoint =
        input
            .history
            .pre_commit_checkpoint
            .as_deref()
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "committed changeset has no pre-commit checkpoint",
                )
            })?;
    let locked_at = Instant::now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if store_identity(&tx)?.store_id != input.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            "changeset is not bound to this live Wiki",
        ));
    }
    let current: Option<(String, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT status, post_revision, pre_commit_checkpoint
             FROM changesets WHERE id = ?1",
            params![&input.history.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if current
        != Some((
            "committed".to_string(),
            Some(expected_post_revision.to_string()),
            Some(pre_commit_checkpoint.to_string()),
        ))
    {
        return Err(AppError::new(
            "changeset_rollback_conflict",
            "changeset history changed before rollback",
        ));
    }
    for page in &inverse.payload.pages {
        let observed = load_sparse_page_snapshot(&tx, &page.slug)?
            .as_ref()
            .map(sparse_page_fingerprint)
            .unwrap_or_else(|| "absent".into());
        if observed != page.after_fingerprint {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("page {} changed after this changeset committed", page.slug),
            )
            .with_details(json!({"entity_type": "page", "identifier": page.slug})));
        }
    }
    for entry in &inverse.payload.meta {
        let observed: String = tx.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![&entry.key],
            |row| row.get(0),
        )?;
        if hash_content(&observed) != entry.after_fingerprint {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("{} changed after this changeset committed", entry.key),
            )
            .with_details(json!({"entity_type": "meta", "identifier": entry.key})));
        }
    }
    for source in &inverse.payload.sources {
        let observed = load_sparse_source_snapshot(&tx, source.source_id)?
            .as_ref()
            .map(sparse_source_fingerprint)
            .unwrap_or_else(|| "absent".into());
        if observed != source.after_fingerprint {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!(
                    "source {} changed after this changeset committed",
                    source.source_id
                ),
            )
            .with_details(json!({
                "entity_type": "source",
                "identifier": source.source_id,
            })));
        }
    }
    for page in &inverse.payload.pages {
        tx.execute(
            "DELETE FROM links WHERE from_slug = ?1",
            params![&page.slug],
        )?;
    }
    for page in &inverse.payload.pages {
        restore_sparse_page(&tx, page)?;
    }
    for entry in &inverse.payload.meta {
        tx.execute(
            "UPDATE meta SET value = ?2 WHERE key = ?1",
            params![&entry.key, &entry.before],
        )?;
        let action = if entry.key == "schema" {
            "schema_set"
        } else {
            "purpose_set"
        };
        record_operation(&tx, action, &entry.key, &json!({"rollback": true}))?;
    }
    for source in &inverse.payload.sources {
        restore_sparse_source(&tx, source)?;
    }
    let mut rollback_detail = json!({
        "name": input.history.name,
        "pre_commit_checkpoint": pre_commit_checkpoint,
        "committed_post_revision": expected_post_revision,
        "pre_rollback_checkpoint": input.pre_rollback_checkpoint,
        "storage": "sparse-v1",
    });
    let rollback_revision = record_operation(
        &tx,
        "changeset_rollback",
        &input.history.id,
        &rollback_detail,
    )?;
    rollback_detail["rollback_revision"] = json!(&rollback_revision);
    tx.execute(
        "UPDATE operations SET detail_json = ?1 WHERE id = last_insert_rowid()",
        params![
            serde_json::to_string(&rollback_detail)
                .map_err(|error| AppError::new("json_error", error.to_string()))?
        ],
    )?;
    tx.execute(
        &format!(
            "UPDATE changesets
             SET status = 'rolled_back', rolled_back_at = {TIMESTAMP_SQL}
             WHERE id = ?1"
        ),
        params![&input.history.id],
    )?;
    tx.commit()?;
    Ok(ChangesetRollbackState {
        changeset_id: input.history.id.clone(),
        name: input.history.name.clone(),
        rollback_revision,
        checkpoint: input.pre_rollback_checkpoint.clone(),
        locked_rollback_ms: elapsed_millis(locked_at),
    })
}

fn restore_sparse_page(tx: &Transaction<'_>, inverse: &SparsePageInverse) -> Result<()> {
    let slug = &inverse.slug;
    let Some(page) = inverse.before.as_ref() else {
        let inbound: i64 = tx.query_row(
            "SELECT COUNT(*) FROM links WHERE to_slug = ?1 AND from_slug <> ?1",
            params![slug],
            |row| row.get(0),
        )?;
        if inbound > 0 {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("page {slug} gained {inbound} inbound Wiki link(s) after commit"),
            ));
        }
        record_operation(tx, "page_remove", slug, &json!({"rollback": true}))?;
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
            params![slug],
        )?;
        tx.execute("DELETE FROM pages WHERE slug = ?1", params![slug])?;
        return Ok(());
    };
    tx.execute(
        "INSERT INTO pages(
            slug, title, kind, summary, body, structural_navigation,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(slug) DO UPDATE SET
            title = excluded.title, kind = excluded.kind,
            summary = excluded.summary, body = excluded.body,
            structural_navigation = excluded.structural_navigation,
            created_at = excluded.created_at, updated_at = excluded.updated_at",
        params![
            slug,
            &page.title,
            page.kind.as_deref(),
            page.summary.as_deref(),
            &page.body,
            page.structural_navigation,
            &page.created_at,
            &page.updated_at,
        ],
    )?;
    tx.execute(
        "DELETE FROM page_sources WHERE page_slug = ?1",
        params![slug],
    )?;
    for source_id in &page.source_ids {
        tx.execute(
            "INSERT INTO page_sources(page_slug, source_id) VALUES (?1, ?2)",
            params![slug, source_id],
        )?;
    }
    tx.execute(
        "DELETE FROM page_provenance WHERE page_slug = ?1",
        params![slug],
    )?;
    for provenance in &page.provenance {
        tx.execute(
            "INSERT INTO page_provenance(page_slug, provenance) VALUES (?1, ?2)",
            params![slug, provenance],
        )?;
    }
    tx.execute("DELETE FROM links WHERE from_slug = ?1", params![slug])?;
    for link in &page.links {
        tx.execute(
            "INSERT INTO links(from_slug, to_slug) VALUES (?1, ?2)",
            params![slug, link],
        )?;
    }
    index_page(
        tx,
        None,
        slug,
        &page.title,
        page.summary.as_deref(),
        &page.body,
    )?;
    record_operation(tx, "page_put", slug, &json!({"rollback": true}))?;
    Ok(())
}

fn restore_sparse_source(tx: &Transaction<'_>, inverse: &SparseSourceInverse) -> Result<()> {
    let source_id = inverse.source_id;
    let Some(source) = inverse.before.as_ref() else {
        let references: i64 = tx.query_row(
            "SELECT COUNT(*) FROM page_sources WHERE source_id = ?1",
            params![source_id],
            |row| row.get(0),
        )?;
        if references > 0 {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("source {source_id} gained {references} page reference(s) after commit"),
            ));
        }
        record_operation(
            tx,
            "source_remove",
            &source_id.to_string(),
            &json!({"rollback": true}),
        )?;
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
            params![source_id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM retrieval_weights
             WHERE target_type = 'source' AND target_identifier = ?1",
            params![source_id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM retrieval_feedback
             WHERE target_type = 'source' AND target_identifier = ?1",
            params![source_id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM source_path_revisions WHERE source_id = ?1",
            params![source_id],
        )?;
        tx.execute("DELETE FROM sources WHERE id = ?1", params![source_id])?;
        return Ok(());
    };
    tx.execute(
        "INSERT INTO sources(
            id, content_hash, title, origin, content,
            structural_navigation, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            content_hash = excluded.content_hash, title = excluded.title,
            origin = excluded.origin, content = excluded.content,
            structural_navigation = excluded.structural_navigation,
            created_at = excluded.created_at",
        params![
            source.id,
            &source.content_hash,
            source.title.as_deref(),
            &source.origin,
            &source.content,
            source.structural_navigation,
            &source.created_at,
        ],
    )?;
    tx.execute(
        "INSERT INTO ingest_jobs(
            source_id, status, attempts, analysis, last_error,
            no_derived_pages_reason, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(source_id) DO UPDATE SET
            status = excluded.status, attempts = excluded.attempts,
            analysis = excluded.analysis, last_error = excluded.last_error,
            no_derived_pages_reason = excluded.no_derived_pages_reason,
            updated_at = excluded.updated_at",
        params![
            source.id,
            &source.ingest.status,
            source.ingest.attempts,
            source.ingest.analysis.as_deref(),
            source.ingest.last_error.as_deref(),
            source.ingest.no_derived_pages_reason.as_deref(),
            &source.ingest.updated_at,
        ],
    )?;
    tx.execute(
        "DELETE FROM source_path_revisions WHERE source_id = ?1",
        params![source.id],
    )?;
    for path in &source.paths {
        tx.execute(
            "INSERT INTO source_path_revisions(
                tracked_path, revision, source_id, observed_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                &path.tracked_path,
                path.revision,
                source.id,
                &path.observed_at
            ],
        )?;
    }
    index_source(
        tx,
        None,
        source.id,
        source.title.as_deref(),
        &source.origin,
        &source.content,
    )?;
    record_operation(
        tx,
        "source_add",
        &source.origin,
        &json!({"source_id": source.id, "rollback": true}),
    )?;
    Ok(())
}

fn rollback_attached_changeset(
    conn: &mut Connection,
    input: &ChangesetRollbackInput,
) -> Result<ChangesetRollbackState> {
    let expected_post_revision = input.history.post_revision.as_deref().ok_or_else(|| {
        AppError::new(
            "changeset_corrupt",
            "committed changeset has no post revision",
        )
    })?;
    let pre_commit_checkpoint =
        input
            .history
            .pre_commit_checkpoint
            .as_deref()
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "committed changeset has no pre-commit checkpoint",
                )
            })?;
    let locked_at = Instant::now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_changeset_table_inventory(&tx, "main")?;
    validate_changeset_table_inventory(&tx, "candidate")?;

    let live = store_identity(&tx)?;
    if live.store_id != input.store_id || live.revision != expected_post_revision {
        return Err(AppError::new(
            "changeset_rollback_conflict",
            "live Wiki changed after this changeset committed",
        ));
    }
    let current: Option<(String, Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT status, post_revision, pre_commit_checkpoint
             FROM changesets WHERE id = ?1",
            params![&input.history.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if current
        != Some((
            "committed".to_string(),
            Some(expected_post_revision.to_string()),
            Some(pre_commit_checkpoint.to_string()),
        ))
    {
        return Err(AppError::new(
            "changeset_rollback_conflict",
            "changeset history changed before rollback",
        ));
    }

    let checkpoint = attached_store_identity(&tx)?;
    if checkpoint.store_id != input.store_id
        || checkpoint.revision != input.history.base_revision
        || checkpoint.operation_id != input.history.base_operation_id
    {
        return Err(AppError::new(
            "changeset_corrupt",
            "pre-commit checkpoint does not match the changeset base",
        ));
    }

    let changed_search = changed_search_documents(&tx, "candidate")?;
    replace_main_from_attached(&tx, "candidate")?;
    refresh_changed_search_documents(&tx, changed_search)?;
    tx.execute(
        &format!(
            "INSERT INTO changesets(
                id, name, status, base_revision, base_operation_id,
                begin_operation_id, pre_commit_checkpoint, post_revision,
                created_at, committed_at, rolled_back_at
             ) VALUES (
                ?1, ?2, 'rolled_back', ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                {TIMESTAMP_SQL}
             )"
        ),
        params![
            &input.history.id,
            &input.history.name,
            &input.history.base_revision,
            input.history.base_operation_id,
            input.history.begin_operation_id,
            pre_commit_checkpoint,
            expected_post_revision,
            &input.history.created_at,
            input.history.committed_at.as_deref(),
        ],
    )?;
    let mut rollback_detail = json!({
        "name": input.history.name,
        "pre_commit_checkpoint": pre_commit_checkpoint,
        "committed_post_revision": expected_post_revision,
        "pre_rollback_checkpoint": input.pre_rollback_checkpoint,
    });
    let rollback_revision = record_operation(
        &tx,
        "changeset_rollback",
        &input.history.id,
        &rollback_detail,
    )?;
    rollback_detail["rollback_revision"] = json!(&rollback_revision);
    tx.execute(
        "UPDATE operations SET detail_json = ?1 WHERE id = last_insert_rowid()",
        params![
            serde_json::to_string(&rollback_detail)
                .map_err(|error| AppError::new("json_error", error.to_string()))?
        ],
    )?;
    validate_database_integrity(&tx)?;
    validate_store(&tx)?;
    tx.commit()?;
    Ok(ChangesetRollbackState {
        changeset_id: input.history.id.clone(),
        name: input.history.name.clone(),
        rollback_revision,
        checkpoint: input.pre_rollback_checkpoint.clone(),
        locked_rollback_ms: elapsed_millis(locked_at),
    })
}
