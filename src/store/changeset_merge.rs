fn create_source_stage(database: &Path) -> Result<(fs::File, PathBuf)> {
    let directory = database
        .parent()
        .ok_or_else(|| AppError::new("io_error", "Wiki database has no parent directory"))?;
    for _ in 0..100 {
        let sequence = SOURCE_STAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            ".source-add-stage-{}-{sequence}.jsonl",
            std::process::id()
        ));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::new(
        "io_error",
        "could not allocate a unique source staging file",
    ))
}

fn insert_source(tx: &Transaction<'_>, input: &SourceAddInput) -> Result<(i64, bool)> {
    let content_hash = hash_content(&input.content);
    let title = input
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&input.origin)
        .to_string();
    let created = tx.execute(
        &format!(
            "INSERT OR IGNORE INTO sources(
                content_hash, title, origin, content, structural_navigation, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, {TIMESTAMP_SQL})"
        ),
        params![
            &content_hash,
            &title,
            &input.origin,
            &input.content,
            has_structural_navigation_marker(&input.content)
        ],
    )? == 1;
    let source_id = tx.query_row(
        "SELECT id FROM sources WHERE content_hash = ?1",
        params![&content_hash],
        |row| row.get::<_, i64>(0),
    )?;
    if created {
        index_source(
            tx,
            None,
            source_id,
            Some(&title),
            &input.origin,
            input.content.as_str(),
        )?;
    }
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO ingest_jobs(source_id, status, updated_at)
             VALUES (?1, 'pending', {TIMESTAMP_SQL})"
        ),
        params![source_id],
    )?;
    let path_revision = input
        .tracked_path
        .as_deref()
        .map(|path| record_source_path_revision(tx, path, source_id))
        .transpose()?;
    record_operation(
        tx,
        "source_add",
        &input.origin,
        &json!({
            "source_id": source_id,
            "created": created,
            "tracked_path": input.tracked_path,
            "path_revision": path_revision.map(|value| value.0),
            "path_advanced": path_revision.map(|value| value.1),
        }),
    )?;
    Ok((source_id, created))
}

fn record_source_path_revision(
    tx: &Transaction<'_>,
    tracked_path: &str,
    source_id: i64,
) -> Result<(i64, bool)> {
    record_source_path_revision_at(tx, tracked_path, source_id, None)
}

fn record_source_path_revision_at(
    tx: &Transaction<'_>,
    tracked_path: &str,
    source_id: i64,
    observed_at: Option<&str>,
) -> Result<(i64, bool)> {
    if tracked_path.trim().is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "tracked source path must not be empty",
        ));
    }
    let latest = tx
        .query_row(
            "SELECT revision, source_id
             FROM source_path_revisions
             WHERE tracked_path = ?1
             ORDER BY revision DESC
             LIMIT 1",
            params![tracked_path],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((revision, latest_source_id)) = latest
        && latest_source_id == source_id
    {
        return Ok((revision, false));
    }
    let revision = latest.map_or(1, |(revision, _)| revision + 1);
    match observed_at {
        Some(observed_at) => tx.execute(
            "INSERT INTO source_path_revisions(tracked_path, revision, source_id, observed_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![tracked_path, revision, source_id, observed_at],
        )?,
        None => tx.execute(
            &format!(
                "INSERT INTO source_path_revisions(tracked_path, revision, source_id, observed_at)
                 VALUES (?1, ?2, ?3, {TIMESTAMP_SQL})"
            ),
            params![tracked_path, revision, source_id],
        )?,
    };
    Ok((revision, true))
}

fn store_identity(conn: &Connection) -> Result<StoreIdentity> {
    let (store_id, revision) = conn.query_row(
        "SELECT
            MAX(CASE WHEN key = 'store_id' THEN value END),
            MAX(CASE WHEN key = 'store_revision' THEN value END)
         FROM meta
         WHERE key IN ('store_id', 'store_revision')",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let operation_id =
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM operations", [], |row| {
            row.get(0)
        })?;
    Ok(StoreIdentity {
        store_id: store_id
            .ok_or_else(|| AppError::new("corrupt_store", "wiki store_id metadata is missing"))?,
        revision: revision.ok_or_else(|| {
            AppError::new("corrupt_store", "wiki store_revision metadata is missing")
        })?,
        operation_id,
    })
}

fn load_committed_changeset(conn: &Connection, id: &str) -> Result<Option<ChangesetCommitState>> {
    let row = conn
        .query_row(
            "SELECT c.id, c.name, c.base_revision, c.post_revision,
                c.pre_commit_checkpoint, o.detail_json
         FROM changesets c
         JOIN operations o ON o.action = 'changeset_commit' AND o.target = c.id
         WHERE c.status = 'committed' AND c.id = ?1
         ORDER BY c.rowid DESC
         LIMIT 1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, name, base_revision, post_revision, checkpoint, detail)) = row else {
        return Ok(None);
    };
    let detail: Value = serde_json::from_str(&detail)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    let staged_operation_count = detail
        .get("staged_operation_count")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "changeset commit operation lacks staged_operation_count",
            )
        })?;
    let lint_issues = detail
        .get("lint_issues")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "changeset commit operation lacks lint_issues",
            )
        })?;
    let source_id_remap = detail
        .get("source_id_remap")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?
        .unwrap_or_default();
    let graph_documents = detail
        .get("graph_documents")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?
        .unwrap_or_default();
    Ok(Some(ChangesetCommitState {
        changeset_id: id,
        name,
        base_revision,
        post_revision,
        checkpoint,
        staged_operation_count,
        lint_issues,
        locked_publish_ms: 0,
        source_id_remap,
        graph_documents,
    }))
}

fn publish_attached_changeset(
    conn: &mut Connection,
    input: &ChangesetPublishInput,
) -> Result<ChangesetCommitState> {
    let locked_at = Instant::now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    changeset_test_fault("after_lock")?;
    validate_changeset_table_inventory(&tx, "main")?;
    validate_changeset_table_inventory(&tx, "candidate")?;

    let live = store_identity(&tx)?;
    if live.store_id != input.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            "changeset is not bound to this live Wiki",
        ));
    }

    let candidate = attached_store_identity(&tx)?;
    if candidate.store_id != input.store_id {
        return Err(AppError::new(
            "changeset_scope_mismatch",
            "draft changeset is not bound to this live Wiki",
        ));
    }
    if candidate.revision != input.draft_revision
        || candidate.operation_id != input.draft_operation_id
    {
        return Err(AppError::new(
            "changeset_changed",
            "draft changeset changed during commit preflight",
        ));
    }

    let (status, base_revision, base_operation_id, begin_operation_id): (String, String, i64, i64) =
        tx.query_row(
            "SELECT status, base_revision, base_operation_id, begin_operation_id
             FROM candidate.changesets
             WHERE id = ?1 AND name = ?2",
            params![&input.id, &input.name],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::new(
                "changeset_changed",
                "draft changeset identity disappeared during commit",
            )
        })?;
    if status != "draft" || base_revision != input.base_revision {
        return Err(AppError::new(
            "changeset_changed",
            "draft changeset metadata changed during commit",
        ));
    }
    let staged_operation_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM candidate.operations WHERE id > ?1",
        params![begin_operation_id],
        |row| row.get(0),
    )?;
    if usize::try_from(staged_operation_count).ok() != Some(input.staged_operation_count) {
        return Err(AppError::new(
            "changeset_changed",
            "draft changeset operations changed during commit",
        ));
    }

    let sparse = tx
        .query_row(
            "SELECT value = 'sparse-v1' FROM candidate.meta
             WHERE key = 'changeset_storage'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    let source_id_remap = if sparse {
        merge_sparse_candidate(&tx, begin_operation_id, true)?
    } else {
        let changed_search = changed_search_documents(&tx, "candidate")?;
        replace_main_from_attached(&tx, "candidate")?;
        refresh_changed_search_documents(&tx, changed_search)?;
        Vec::new()
    };
    changeset_test_fault("after_fts")?;
    validate_database_integrity(&tx)?;
    changeset_test_fault("after_integrity")?;

    let source_id_map = source_id_remap
        .iter()
        .map(|entry| (entry.draft_id, entry.live_id))
        .collect::<BTreeMap<_, _>>();
    let graph_documents = input
        .graph_documents
        .iter()
        .map(|(document_type, identifier)| {
            if document_type != "source" {
                return (document_type.clone(), identifier.clone());
            }
            let identifier = identifier
                .parse::<i64>()
                .ok()
                .and_then(|draft_id| source_id_map.get(&draft_id).copied())
                .map_or_else(|| identifier.clone(), |live_id| live_id.to_string());
            (document_type.clone(), identifier)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let post_revision = record_operation(
        &tx,
        "changeset_commit",
        &input.id,
        &json!({
            "name": input.name,
            "base_revision": input.base_revision,
            "checkpoint": input.checkpoint,
            "staged_operation_count": input.staged_operation_count,
            "lint_issues": input.lint_issues,
            "lint_override_reason": input.lint_override_reason,
            "source_id_remap": &source_id_remap,
            "graph_documents": &graph_documents,
        }),
    )?;
    tx.execute(
        &format!(
            "INSERT INTO changesets(
                id, name, status, base_revision, base_operation_id,
                begin_operation_id, created_at
             ) VALUES (?1, ?2, 'draft', ?3, ?4, ?5, {TIMESTAMP_SQL})
             ON CONFLICT(id) DO NOTHING"
        ),
        params![
            &input.id,
            &input.name,
            &input.base_revision,
            base_operation_id,
            begin_operation_id,
        ],
    )?;
    let updated = tx.execute(
        &format!(
            "UPDATE changesets
             SET status = 'committed', pre_commit_checkpoint = ?1,
                 post_revision = ?2, committed_at = {TIMESTAMP_SQL}
             WHERE id = ?3 AND status = 'draft'"
        ),
        params![&input.checkpoint, &post_revision, &input.id],
    )?;
    if updated != 1 {
        return Err(AppError::new(
            "changeset_changed",
            "draft changeset could not be marked committed",
        ));
    }
    validate_store(&tx)?;
    changeset_test_fault("before_commit")?;
    changeset_test_crash("before_commit");
    tx.commit()?;
    let locked_publish_ms = elapsed_millis(locked_at);
    changeset_test_crash("after_commit");
    Ok(ChangesetCommitState {
        changeset_id: input.id.clone(),
        name: input.name.clone(),
        base_revision: input.base_revision.clone(),
        post_revision,
        checkpoint: input.checkpoint.clone(),
        staged_operation_count: input.staged_operation_count,
        lint_issues: input.lint_issues,
        locked_publish_ms,
        source_id_remap,
        graph_documents,
    })
}

fn merge_sparse_candidate(
    tx: &Transaction<'_>,
    begin_operation_id: i64,
    validate_operations: bool,
) -> Result<Vec<SparseSourceIdRemap>> {
    let mut operations = {
        let mut statement = tx.prepare(
            "SELECT action, target, detail_json
             FROM candidate.operations WHERE id > ?1 ORDER BY id",
        )?;
        statement
            .query_map(params![begin_operation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if validate_operations {
        validate_sparse_operation_actions(operations.iter().map(|(action, _, _)| action.as_str()))?;
    }
    let page_targets = operations
        .iter()
        .filter(|(action, _, _)| matches!(action.as_str(), "page_put" | "page_remove"))
        .map(|(_, target, _)| target.clone())
        .collect::<BTreeSet<_>>();
    for (action, _, _) in &operations {
        let key = match action.as_str() {
            "schema_set" => "schema",
            "purpose_set" => "purpose",
            _ => continue,
        };
        let expected: String = tx.query_row(
            "SELECT value FROM candidate.meta WHERE key = ?1",
            params![format!("changeset_base_meta:{key}")],
            |row| row.get(0),
        )?;
        let observed: String = tx.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )?;
        if hash_content(&observed) != expected {
            return Err(AppError::new(
                "changeset_conflict",
                format!("{key} changed after the changeset began"),
            )
            .with_details(json!({"entity_type": "meta", "identifier": key})));
        }
    }
    for slug in &page_targets {
        let candidate_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM candidate.pages WHERE slug = ?1)",
            params![slug],
            |row| row.get(0),
        )?;
        let live_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE slug = ?1)",
            params![slug],
            |row| row.get(0),
        )?;
        let base = tx
            .query_row(
                "SELECT value FROM candidate.meta WHERE key = ?1",
                params![format!("changeset_base_page:{slug}")],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match base.as_deref() {
            Some("absent") | None if live_exists => {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("page {slug} changed after it was first touched"),
                )
                .with_details(json!({"entity_type": "page", "identifier": slug})));
            }
            Some(expected) if expected != "absent" => {
                let observed = load_page_mutation_base(tx, slug)?
                    .map(|value| value.content_fingerprint)
                    .unwrap_or_else(|| "absent".into());
                if observed != expected {
                    return Err(AppError::new(
                        "changeset_conflict",
                        format!("page {slug} changed after it was first touched"),
                    )
                    .with_details(json!({"entity_type": "page", "identifier": slug})));
                }
            }
            _ => {}
        }
        if !candidate_exists && !live_exists {
            return Err(AppError::new(
                "changeset_corrupt",
                format!("page mutation {slug} has neither an after image nor a live base"),
            ));
        }
    }
    let tag_targets = operations
        .iter()
        .filter(|(action, _, _)| {
            matches!(
                action.as_str(),
                "tag_set" | "tag_remove" | "tag_delete" | "tag_autoload"
            )
        })
        .map(|(_, _, detail)| {
            serde_json::from_str::<Value>(detail)
                .ok()
                .and_then(|value| value.get("tag").and_then(Value::as_str).map(str::to_string))
                .ok_or_else(|| AppError::new("changeset_corrupt", "tag operation lacks tag"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    for tag in &tag_targets {
        let expected: String = tx.query_row(
            "SELECT value FROM candidate.meta WHERE key = ?1",
            [format!("changeset_base_tag:{tag}")],
            |row| row.get(0),
        )?;
        let observed = tag_snapshot_fingerprint(&load_tag_snapshot(tx, "main", tag)?);
        if observed != expected {
            return Err(AppError::new(
                "changeset_conflict",
                format!("tag {tag} changed after it was first touched"),
            )
            .with_details(json!({"entity_type": "tag", "identifier": tag})));
        }
    }

    let source_id_remap = merge_sparse_sources(tx, &mut operations)?;
    let source_id_map = source_id_remap
        .iter()
        .map(|entry| (entry.draft_id, entry.live_id))
        .collect::<BTreeMap<_, _>>();
    for (action, target, detail) in &operations {
        let detail = serde_json::from_str::<Value>(detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        record_operation(tx, action, target, &detail)?;
    }
    changeset_test_fault("mid_copy")?;
    for slug in page_targets {
        merge_sparse_page(tx, &slug, &source_id_map)?;
    }
    merge_sparse_tags(tx, &operations)?;
    if operations
        .iter()
        .any(|(action, _, _)| action == "schema_set")
    {
        tx.execute(
            "UPDATE meta SET value = (SELECT value FROM candidate.meta WHERE key = 'schema')
             WHERE key = 'schema'",
            [],
        )?;
    }
    if operations
        .iter()
        .any(|(action, _, _)| action == "purpose_set")
    {
        tx.execute(
            "UPDATE meta SET value = (SELECT value FROM candidate.meta WHERE key = 'purpose')
             WHERE key = 'purpose'",
            [],
        )?;
    }
    Ok(source_id_remap)
}

fn merge_sparse_tags(tx: &Transaction<'_>, operations: &[(String, String, String)]) -> Result<()> {
    for (action, _, detail) in operations {
        if !matches!(
            action.as_str(),
            "tag_set" | "tag_remove" | "tag_delete" | "tag_autoload"
        ) {
            continue;
        }
        let detail: Value = serde_json::from_str(detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let tag = detail
            .get("tag")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::new("changeset_corrupt", "tag operation lacks tag"))?;
        match action.as_str() {
            "tag_set" => {
                tx.execute(
                    "INSERT OR IGNORE INTO tags(
                        name, autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason, updated_at
                     ) SELECT
                        name, autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason, updated_at
                       FROM candidate.tags WHERE name = ?1",
                    [tag],
                )?;
                let page = detail
                    .get("page")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AppError::new("changeset_corrupt", "tag_set operation lacks page")
                    })?;
                tx.execute(
                    "INSERT INTO page_tags(
                        tag_name, page_slug, priority, reason, created_at, updated_at
                     ) SELECT
                        tag_name, page_slug, priority, reason, created_at, updated_at
                       FROM candidate.page_tags
                       WHERE tag_name = ?1 AND page_slug = ?2
                     ON CONFLICT(tag_name, page_slug) DO UPDATE SET
                        priority = excluded.priority, reason = excluded.reason,
                        created_at = excluded.created_at, updated_at = excluded.updated_at",
                    params![tag, page],
                )?;
            }
            "tag_remove" => {
                let page = detail
                    .get("page")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AppError::new("changeset_corrupt", "tag_remove operation lacks page")
                    })?;
                tx.execute(
                    "DELETE FROM page_tags WHERE tag_name = ?1 AND page_slug = ?2",
                    params![tag, page],
                )?;
            }
            "tag_delete" => {
                tx.execute("DELETE FROM tags WHERE name = ?1", [tag])?;
            }
            "tag_autoload" => {
                let changed = tx.execute(
                    "UPDATE tags SET
                        autoload = (SELECT autoload FROM candidate.tags WHERE name = ?1),
                        autoload_priority = (SELECT autoload_priority FROM candidate.tags WHERE name = ?1),
                        autoload_limit = (SELECT autoload_limit FROM candidate.tags WHERE name = ?1),
                        autoload_max_chars = (SELECT autoload_max_chars FROM candidate.tags WHERE name = ?1),
                        reason = (SELECT reason FROM candidate.tags WHERE name = ?1),
                        updated_at = (SELECT updated_at FROM candidate.tags WHERE name = ?1)
                     WHERE name = ?1",
                    [tag],
                )?;
                if changed != 1 {
                    return Err(AppError::new(
                        "changeset_corrupt",
                        format!("staged tag {tag} is missing"),
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}
