// Replica-only object CAS: unrelated local writes do not invalidate an accepted
// remote batch. Whole-store publication retains its stricter identity contract.
impl Store {
    fn require_replica_patch_base(
        &self,
        base: &BTreeMap<(String, String), Value>,
        state: &PreparedSyncState,
    ) -> Result<()> {
        // Metadata only, in memory and within the caller's write transaction.
        // ponytail: O(n) inventory read; add a durable object hash index if measured
        // lock time becomes material. No blob copying or disk staging under lock.
        let inventory = Connection::open_in_memory()?;
        inventory.execute_batch("CREATE TABLE sync_objects(kind TEXT, logical_key TEXT, payload_json TEXT, payload_hash TEXT, PRIMARY KEY(kind,logical_key)) WITHOUT ROWID;")?;
        self.export_sync_objects(&inventory, false)?;
        let live = load_sync_objects_from(&inventory)?;
        for (key, before) in base.iter().map(|(k, v)| (k, Some(v))).chain(
            state
                .objects
                .keys()
                .filter(|k| !base.contains_key(*k))
                .map(|k| (k, None)),
        ) {
            // Auxiliary continuity is protected by the engine's fingerprint and
            // replay receipt, not represented by canonical Store rows.
            if matches!(key.0.as_str(), "draft_intent" | "work_audit") {
                continue;
            }
            let after = state.objects.get(key);
            if before != after && live.get(key).map(|row| &row.payload) != before {
                return Err(sync_store_changed());
            }
        }
        Ok(())
    }
}

fn apply_replica_patch(
    tx: &Transaction<'_>,
    base: &BTreeMap<(String, String), Value>,
    target: &PreparedSyncState,
) -> Result<()> {
    let changed = changed_sync_objects(base, &target.objects);
    let state = PreparedSyncState {
        objects: target
            .objects
            .iter()
            .filter(|((kind, key), _)| {
                changed
                    .by_kind
                    .get(kind)
                    .is_some_and(|keys| keys.contains(key))
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        digest: target.digest.clone(),
        normalized: target.normalized.clone(),
        blob_count: target.blob_count,
        #[cfg(test)]
        buffered_blob_bytes: 0,
    };
    let todo_revisions = local_object_revisions(tx, "todo_items")?;
    let plan_revisions = local_object_revisions(tx, "plans")?;
    let memory_requests = local_object_request_ids(tx, "memory_events")?;
    let todo_requests = local_object_request_ids(tx, "todo_items")?;
    let plan_requests = local_object_request_ids(tx, "plans")?;
    // Explicitly remove only rows owned by changed objects. FK actions are
    // disabled on this connection, so new local dependents cannot be cascaded
    // away. Missing dependencies reject the entire transaction at validation.
    for (kind, keys) in &changed.by_kind {
        for key in keys {
            let sql = match kind.as_str() {
                "page" => {
                    "DELETE FROM links WHERE from_slug=?1; DELETE FROM page_provenance WHERE page_slug=?1; DELETE FROM page_sources WHERE page_slug=?1; DELETE FROM pages WHERE slug=?1;"
                }
                "tag" => "DELETE FROM page_tags WHERE tag_name=?1; DELETE FROM tags WHERE name=?1;",
                "memory" => {
                    "DELETE FROM memory_feedback WHERE event_id=?1; DELETE FROM memory_relations WHERE event_id=?1; DELETE FROM memory_changes WHERE event_id=?1; DELETE FROM memory_evidence WHERE event_id=?1; DELETE FROM memory_fragments WHERE event_id=?1; DELETE FROM memory_events WHERE id=?1;"
                }
                "todo" => {
                    "DELETE FROM todo_tags WHERE todo_id=?1; DELETE FROM todo_items WHERE id=?1;"
                }
                "plan" => {
                    "DELETE FROM plan_history WHERE plan_id=?1; DELETE FROM plan_steps WHERE plan_id=?1; DELETE FROM plan_constraints WHERE plan_id=?1; DELETE FROM plan_tags WHERE plan_id=?1; DELETE FROM plans WHERE id=?1;"
                }
                "semantic_relation" => "DELETE FROM semantic_relations WHERE id=?1;",
                "ingest" => {
                    "DELETE FROM ingest_jobs WHERE source_id=(SELECT id FROM sources WHERE content_hash=?1);"
                }
                "source" | "meta" | "retrieval_weight" | "retrieval_feedback" | "discussion"
                | "source_revision" | "memory_audit" | "work_audit" | "draft_intent" => "",
                _ => return Err(invalid_object(kind, key, "unsupported replica patch kind")),
            };
            for statement in sql.split(';').filter(|s| !s.trim().is_empty()) {
                tx.execute(statement, [key])?;
            }
            if matches!(kind.as_str(), "retrieval_weight" | "retrieval_feedback")
                && let Some(before) = base.get(&(kind.clone(), key.clone()))
            {
                let hashes = tx
                    .prepare("SELECT content_hash,id FROM sources")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                    .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
                let target_type = required_str(before, "target_type")?;
                let identifier = local_target_identifier(before, target_type, &hashes, key)?;
                let provenance = required_str(before, "provenance")?;
                if kind == "retrieval_weight" {
                    tx.execute("DELETE FROM retrieval_weights WHERE target_type=?1 AND target_identifier=?2 AND provenance=?3",params![target_type,identifier,provenance])?;
                } else {
                    tx.execute("DELETE FROM retrieval_feedback WHERE target_type=?1 AND target_identifier=?2 AND provenance=?3 AND query_fingerprint=?4",params![target_type,identifier,provenance,required_str(before,"query_fingerprint")?])?;
                }
            }
        }
    }
    for hash in changed
        .keys("source")
        .filter(|hash| object(target, "source", hash).is_none())
    {
        tx.execute("DELETE FROM source_path_revisions WHERE tracked_path IN (SELECT tracked_path FROM source_path_revisions WHERE source_id=(SELECT id FROM sources WHERE content_hash=?1))",[hash])?;
        tx.execute("DELETE FROM sources WHERE content_hash=?1", [hash])?;
    }
    import_sync_meta(tx, &state)?;
    let source_ids = import_sync_sources(tx, &state, false)?;
    import_sync_pages(tx, &state, &source_ids)?;
    import_sync_tags(tx, &state)?;
    import_sync_ingest(tx, &state, &source_ids)?;
    import_sync_retrieval(tx, &state, &source_ids)?;
    import_sync_relations(tx, &state, &source_ids)?;
    import_sync_memory(tx, &state, &memory_requests)?;
    import_sync_todos(tx, &state, &todo_revisions, &todo_requests)?;
    import_sync_plans(tx, &state, &plan_revisions, &plan_requests)?;
    if let Some(ids) = changed.by_kind.get("discussion") {
        import_sync_discussions_selected(tx, &state, Some(ids))?;
    }
    import_replica_history(tx, &state)?;
    import_sync_work_audits(tx, &state)?;
    validate_sync_draft_intents(&state)?;
    tx.execute_batch("DELETE FROM agent_todo_tracks WHERE NOT EXISTS(SELECT 1 FROM todo_items WHERE id=agent_todo_tracks.todo_id); DELETE FROM agent_plan_tracks WHERE NOT EXISTS(SELECT 1 FROM plans WHERE id=agent_plan_tracks.plan_id);")?;
    validate_sync_domain_invariants(tx)?;
    Ok(())
}
