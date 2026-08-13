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
                | "tag_set"
                | "tag_remove"
                | "tag_delete"
                | "tag_autoload"
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
    operations: &mut [(String, String, String)],
) -> Result<Vec<SparseSourceIdRemap>> {
    let base_operation_id: i64 = tx.query_row(
        "SELECT base_operation_id FROM candidate.changesets
         WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    let touched_paths = operations
        .iter()
        .filter(|(action, _, _)| action == "source_add")
        .filter_map(|(_, _, detail)| serde_json::from_str::<Value>(detail).ok())
        .filter_map(|detail| {
            detail
                .get("tracked_path")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();
    let mut statement = tx.prepare(
        "SELECT action, detail_json FROM operations
         WHERE id > ?1 AND action IN ('source_add', 'source_remove') ORDER BY id",
    )?;
    for row in statement.query_map([base_operation_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (action, detail) = row?;
        let detail: Value = serde_json::from_str(&detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let changed_paths = if action == "source_add"
            && detail.get("path_advanced").and_then(Value::as_bool) == Some(true)
        {
            detail
                .get("tracked_path")
                .and_then(Value::as_str)
                .into_iter()
                .collect::<Vec<_>>()
        } else if action == "source_remove" {
            detail
                .get("affected_paths")
                .or_else(|| detail.get("untracked_paths"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if let Some(path) = changed_paths
            .into_iter()
            .find(|path| touched_paths.contains(*path))
        {
            return Err(AppError::new(
                "changeset_conflict",
                format!("source path {path} advanced while the changeset was open"),
            ));
        }
    }
    drop(statement);

    let source_ids = operations
        .iter()
        .filter(|(action, _, _)| action == "source_add")
        .filter_map(|(_, _, detail)| serde_json::from_str::<Value>(detail).ok())
        .filter_map(|detail| detail.get("source_id").and_then(Value::as_i64))
        .collect::<BTreeSet<_>>();
    let mut remaps = Vec::new();
    let mut source_id_map = BTreeMap::new();
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
        let existing_by_hash: Option<i64> = tx
            .query_row(
                "SELECT id FROM sources WHERE content_hash = ?1",
                params![&source.0],
                |row| row.get(0),
            )
            .optional()?;
        let (resolved_id, created) = if let Some(existing_id) = existing_by_hash {
            (existing_id, false)
        } else if collision.is_none() {
            tx.execute(
                "INSERT INTO sources(
                    id, content_hash, title, origin, content,
                    structural_navigation, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    source_id, source.0, source.1, source.2, source.3, source.4, source.5
                ],
            )?;
            (source_id, true)
        } else {
            tx.execute(
                "INSERT INTO sources(
                    content_hash, title, origin, content,
                    structural_navigation, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![source.0, source.1, source.2, source.3, source.4, source.5],
            )?;
            (tx.last_insert_rowid(), true)
        };
        source_id_map.insert(source_id, resolved_id);
        if source_id != resolved_id || !created {
            remaps.push(SparseSourceIdRemap {
                draft_id: source_id,
                live_id: resolved_id,
                created,
            });
        }
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO ingest_jobs(source_id, status, updated_at)
                 VALUES (?1, 'pending', {TIMESTAMP_SQL})"
            ),
            params![resolved_id],
        )?;
        if created {
            tx.execute(
            "INSERT INTO ingest_jobs(
                source_id, status, attempts, analysis, last_error,
                no_derived_pages_reason, updated_at
             ) SELECT ?2, status, attempts, analysis, last_error,
                      no_derived_pages_reason, updated_at
               FROM candidate.ingest_jobs WHERE source_id = ?1
             ON CONFLICT(source_id) DO UPDATE SET
                status = excluded.status, attempts = excluded.attempts,
                analysis = excluded.analysis, last_error = excluded.last_error,
                no_derived_pages_reason = excluded.no_derived_pages_reason,
                updated_at = excluded.updated_at",
            params![source_id, resolved_id],
            )?;
            index_source(
                tx,
                None,
                resolved_id,
                source.1.as_deref(),
                &source.2,
                &source.3,
            )?;
        }
    }
    for (action, target, _) in operations.iter_mut() {
        if !action.starts_with("ingest_") {
            continue;
        }
        let Ok(draft_source_id) = target.parse::<i64>() else {
            continue;
        };
        let Some(remap) = remaps
            .iter()
            .find(|entry| entry.draft_id == draft_source_id)
        else {
            continue;
        };
        if !remap.created {
            return Err(AppError::new(
                "changeset_conflict",
                format!(
                    "source content for {draft_source_id} was added concurrently before ingest state could merge"
                ),
            ));
        }
        *target = remap.live_id.to_string();
    }
    for (action, _, detail) in operations.iter_mut() {
        if action != "source_add" {
            continue;
        }
        let mut value: Value = serde_json::from_str(detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let Some(tracked_path) = value
            .get("tracked_path")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let draft_source_id = value
            .get("source_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| AppError::new("changeset_corrupt", "source_add lacks source_id"))?;
        let candidate_revision = value
            .get("path_revision")
            .and_then(Value::as_i64)
            .ok_or_else(|| AppError::new("changeset_corrupt", "source_add lacks path_revision"))?;
        let observed_at: String = tx.query_row(
            "SELECT observed_at FROM candidate.source_path_revisions
             WHERE tracked_path = ?1 AND revision = ?2 AND source_id = ?3",
            params![&tracked_path, candidate_revision, draft_source_id],
            |row| row.get(0),
        )?;
        let source_id = source_id_map
            .get(&draft_source_id)
            .copied()
            .unwrap_or(draft_source_id);
        let created = remaps
            .iter()
            .find(|entry| entry.draft_id == draft_source_id)
            .is_none_or(|entry| entry.created);
        let (revision, advanced) = record_source_path_revision_at(
            tx,
            &tracked_path,
            source_id,
            Some(&observed_at),
        )?;
        value["path_revision"] = json!(revision);
        value["path_advanced"] = json!(advanced);
        value["source_id"] = json!(source_id);
        value["created"] = json!(created);
        *detail = serde_json::to_string(&value)
            .map_err(|error| AppError::new("json_error", error.to_string()))?;
    }
    Ok(remaps)
}

fn merge_sparse_page(
    tx: &Transaction<'_>,
    slug: &str,
    source_id_remap: &BTreeMap<i64, i64>,
) -> Result<()> {
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
    let source_ids = {
        let mut statement = tx.prepare(
            "SELECT source_id FROM candidate.page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map(params![slug], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for source_id in source_ids {
        tx.execute(
            "INSERT INTO page_sources(page_slug, source_id) VALUES (?1, ?2)",
            params![
                slug,
                source_id_remap.get(&source_id).copied().unwrap_or(source_id)
            ],
        )?;
    }
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
    let mut graph_documents = inverse
        .payload
        .pages
        .iter()
        .map(|page| ("page".to_string(), page.slug.clone()))
        .collect::<BTreeSet<_>>();
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
    let source_id_remap = load_sparse_source_id_remap(&tx, &input.history.id)?;
    let source_id_map = source_id_remap
        .iter()
        .map(|entry| (entry.draft_id, entry.live_id))
        .collect::<BTreeMap<_, _>>();
    for source in &inverse.payload.sources {
        let source_id = source_id_map
            .get(&source.source_id)
            .copied()
            .unwrap_or(source.source_id);
        graph_documents.insert(("source".to_string(), source_id.to_string()));
    }
    for page in &inverse.payload.pages {
        let observed = load_sparse_page_snapshot(&tx, &page.slug)?
            .as_ref()
            .map(sparse_page_fingerprint)
            .unwrap_or_else(|| "absent".into());
        let expected = page.after.as_ref().map_or_else(
            || page.after_fingerprint.clone(),
            |after| {
                let mut after = after.clone();
                for source_id in &mut after.source_ids {
                    if let Some(live_id) = source_id_map.get(source_id) {
                        *source_id = *live_id;
                    }
                }
                sparse_page_fingerprint(&after)
            },
        );
        if observed != expected {
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
        if source_id_remap
            .iter()
            .any(|entry| entry.draft_id == source.source_id && !entry.created)
        {
            continue;
        }
        let source_id = source_id_map
            .get(&source.source_id)
            .copied()
            .unwrap_or(source.source_id);
        let observed = load_sparse_source_snapshot(&tx, source_id)?
            .map(|mut snapshot| {
                snapshot.id = source.source_id;
                sparse_source_fingerprint(&snapshot)
            })
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
    for path in &inverse.payload.source_paths {
        let observed = load_sparse_tracked_path(&tx, &path.tracked_path)?;
        let expected = if path.after.is_empty() {
            path.after_fingerprint.clone()
        } else {
            let mut after = path.after.clone();
            for revision in &mut after {
                if let Some(live_id) = source_id_map.get(&revision.source_id) {
                    revision.source_id = *live_id;
                }
            }
            sparse_tracked_path_fingerprint(&after)
        };
        if sparse_tracked_path_fingerprint(&observed) != expected {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!(
                    "source path {} changed after this changeset committed",
                    path.tracked_path
                ),
            )
            .with_details(json!({
                "entity_type": "source_path",
                "identifier": path.tracked_path,
            })));
        }
        for source_id in observed
            .last()
            .map(|head| head.source_id)
            .into_iter()
            .chain(path.before.last().map(|head| head.source_id))
        {
            graph_documents.insert(("source".to_string(), source_id.to_string()));
        }
    }
    for tag in &inverse.payload.tags {
        let observed = load_tag_snapshot(&tx, "main", &tag.name)?;
        if tag_snapshot_fingerprint(&observed) != tag.after_fingerprint {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("tag {} changed after this changeset committed", tag.name),
            )
            .with_details(json!({"entity_type": "tag", "identifier": tag.name})));
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
    for page in inverse
        .payload
        .pages
        .iter()
        .filter(|page| page.before.is_none())
    {
        if load_sparse_page_inbound_links(&tx, &page.slug)? != page.inbound_links {
            return Err(AppError::new(
                "changeset_rollback_conflict",
                format!("page {} inbound Wiki links changed after commit", page.slug),
            ));
        }
    }
    for tag in &inverse.payload.tags {
        tx.execute("DELETE FROM tags WHERE name = ?1", [&tag.name])?;
        restore_tag_snapshot(&tx, &tag.before)?;
        record_operation(
            &tx,
            "tag_restore",
            &tag.name,
            &json!({"rollback": true}),
        )?;
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
        let remap = source_id_remap
            .iter()
            .find(|entry| entry.draft_id == source.source_id);
        restore_sparse_source(
            &tx,
            source,
            remap.map_or(source.source_id, |entry| entry.live_id),
            remap.is_none_or(|entry| entry.created),
        )?;
    }
    for path in &inverse.payload.source_paths {
        restore_sparse_tracked_path(&tx, path)?;
    }
    let graph_documents = graph_documents.into_iter().collect::<Vec<_>>();
    let mut rollback_detail = json!({
        "name": input.history.name,
        "pre_commit_checkpoint": pre_commit_checkpoint,
        "committed_post_revision": expected_post_revision,
        "pre_rollback_checkpoint": input.pre_rollback_checkpoint,
        "storage": "sparse-v1",
        "graph_documents": graph_documents,
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
        graph_documents,
    })
}

fn restore_sparse_tracked_path(
    tx: &Transaction<'_>,
    inverse: &SparseTrackedPathInverse,
) -> Result<()> {
    tx.execute(
        "DELETE FROM source_path_revisions WHERE tracked_path = ?1",
        params![&inverse.tracked_path],
    )?;
    for revision in &inverse.before {
        tx.execute(
            "INSERT INTO source_path_revisions(
                tracked_path, revision, source_id, observed_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                &inverse.tracked_path,
                revision.revision,
                revision.source_id,
                &revision.observed_at,
            ],
        )?;
    }
    Ok(())
}

fn restore_sparse_page(tx: &Transaction<'_>, inverse: &SparsePageInverse) -> Result<()> {
    let slug = &inverse.slug;
    let Some(page) = inverse.before.as_ref() else {
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

fn restore_sparse_source(
    tx: &Transaction<'_>,
    inverse: &SparseSourceInverse,
    source_id: i64,
    remove_if_absent: bool,
) -> Result<()> {
    let Some(source) = inverse.before.as_ref() else {
        if !remove_if_absent {
            return Ok(());
        }
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

fn load_sparse_source_id_remap(
    conn: &Connection,
    changeset_id: &str,
) -> Result<Vec<SparseSourceIdRemap>> {
    let detail: String = conn.query_row(
        "SELECT detail_json FROM operations
         WHERE action = 'changeset_commit' AND target = ?1
         ORDER BY id DESC LIMIT 1",
        [changeset_id],
        |row| row.get(0),
    )?;
    let detail: Value = serde_json::from_str(&detail)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    detail
        .get("source_id_remap")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))
        .map(Option::unwrap_or_default)
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
    let mut graph_documents = changed_search
        .0
        .iter()
        .map(|(id, _)| ("source".to_string(), id.to_string()))
        .chain(
            changed_search
                .1
                .iter()
                .map(|(slug, _)| ("page".to_string(), slug.clone())),
        )
        .collect::<BTreeSet<_>>();
    graph_documents.extend(changed_source_path_head_documents(&tx)?);
    let graph_documents = graph_documents.into_iter().collect::<Vec<_>>();
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
        "graph_documents": graph_documents,
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
        graph_documents,
    })
}

fn changed_source_path_head_documents(
    tx: &Transaction<'_>,
) -> Result<BTreeSet<(String, String)>> {
    let mut statement = tx.prepare(
        "SELECT tracked_path FROM source_path_revisions
         UNION SELECT tracked_path FROM candidate.source_path_revisions
         ORDER BY tracked_path",
    )?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut documents = BTreeSet::new();
    for path in paths {
        let live = tx
            .query_row(
                "SELECT source_id FROM source_path_revisions
                 WHERE tracked_path = ?1 ORDER BY revision DESC LIMIT 1",
                [&path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let checkpoint = tx
            .query_row(
                "SELECT source_id FROM candidate.source_path_revisions
                 WHERE tracked_path = ?1 ORDER BY revision DESC LIMIT 1",
                [&path],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if live != checkpoint {
            for source_id in live.into_iter().chain(checkpoint) {
                documents.insert(("source".to_string(), source_id.to_string()));
            }
        }
    }
    Ok(documents)
}
