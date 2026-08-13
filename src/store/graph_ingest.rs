impl Store {
    pub fn graph_explore(
        &self,
        identifier: &str,
        depth: usize,
        limit: usize,
        direction: &str,
        edge_types: &[String],
    ) -> Result<Value> {
        crate::external_graph::explore(
            &self.scope,
            &self.database,
            identifier,
            depth,
            limit,
            direction,
            edge_types,
        )
    }

    pub fn graph_explore_macro(
        &self,
        depth: usize,
        limit: usize,
        edge_types: &[String],
    ) -> Result<Value> {
        let _ = (depth, edge_types);
        crate::external_graph::overview(&self.scope, &self.database, limit)
    }

    pub fn graph_node(&self, identifier: &str) -> Result<Value> {
        crate::external_graph::node(&self.scope, &self.database, identifier)
    }

    pub fn graph_neighbors(
        &self,
        identifier: &str,
        limit: usize,
        direction: &str,
        edge_types: &[String],
    ) -> Result<Value> {
        let mut response = crate::external_graph::explore(
            &self.scope,
            &self.database,
            identifier,
            1,
            limit.saturating_add(1),
            direction,
            edge_types,
        )?;
        response["limit"] = json!(limit);
        response["neighbors"] = response["nodes"]
            .as_array()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter(|node| node["depth"] == 1)
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .map(Value::Array)
            .unwrap_or_else(|| json!([]));
        Ok(response)
    }

    pub fn graph_path(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
        limit: usize,
        direction: &str,
        edge_types: &[String],
    ) -> Result<Value> {
        crate::external_graph::path(
            &self.scope,
            &self.database,
            from,
            to,
            max_depth,
            limit,
            direction,
            edge_types,
        )
    }

    pub fn graph_impact(&self, identifier: &str, max_depth: usize, limit: usize) -> Result<Value> {
        crate::external_graph::explore(
            &self.scope,
            &self.database,
            identifier,
            max_depth,
            limit,
            "incoming",
            &[],
        )
    }

    pub fn graph_overview(&self, limit: usize) -> Result<Value> {
        crate::external_graph::overview(&self.scope, &self.database, limit)
    }

    pub fn graph_status(&self) -> Result<Value> {
        crate::external_graph::status(&self.scope, &self.database)
    }

    pub fn graph_passive_status(&self) -> Result<Value> {
        crate::external_graph::passive_status(&self.scope, &self.database)
    }

    pub fn graph_verify(&self) -> Result<Value> {
        crate::external_graph::verify(&self.scope, &self.database)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn graph_relation_set(
        &mut self,
        from: &str,
        relation_type: &str,
        to: &str,
        provenance: &str,
        reason: &str,
        confidence: f64,
        source_ids: &[i64],
    ) -> Result<Value> {
        self.preflight_graph_runtime()?;
        let mut response = graph_relation_set_value(
            &mut self.conn,
            &self.scope,
            from,
            relation_type,
            to,
            provenance,
            reason,
            confidence,
            source_ids,
        )?;
        let document = self.graph_owner_document(from)?;
        if let Some(work) = self.schedule_graph_documents(&[document])? {
            response["work"] = work;
        }
        Ok(response)
    }

    pub fn graph_relation_list(
        &self,
        from: Option<&str>,
        to: Option<&str>,
        relation_type: Option<&str>,
        limit: usize,
    ) -> Result<Value> {
        graph_relation_list_value(&self.conn, &self.scope, from, to, relation_type, limit)
    }

    pub fn graph_relation_retract(
        &mut self,
        from: &str,
        relation_type: &str,
        to: &str,
        reason: &str,
    ) -> Result<Value> {
        self.preflight_graph_runtime()?;
        let mut response = graph_relation_retract_value(
            &mut self.conn,
            &self.scope,
            from,
            relation_type,
            to,
            reason,
        )?;
        let document = self.graph_owner_document(from)?;
        if let Some(work) = self.schedule_graph_documents(&[document])? {
            response["work"] = work;
        }
        Ok(response)
    }

    fn load_graph_mutation_summary(
        &self,
        _document_type: &str,
        _document_identifier: &str,
        document_duration_ms: u64,
        queue_duration_ms: u64,
    ) -> Result<GraphMutationSummary> {
        let setting = config::resolve(&self.scope, &self.database)?.setting;
        let engine = match setting {
            GraphSetting::Disabled => "disabled",
            GraphSetting::Grafeo => "grafeo",
            GraphSetting::Surrealdb => "surrealdb",
            GraphSetting::Inherit => unreachable!("resolved graph setting"),
        };
        Ok(GraphMutationSummary {
            invalidated_semantic_relations: 0,
            engine: engine.to_string(),
            status: if setting == GraphSetting::Disabled {
                "disabled".to_string()
            } else {
                "pending".to_string()
            },
            document_duration_ms,
            queue_duration_ms,
            work: None,
        })
    }

    pub(crate) fn schedule_graph_documents(
        &self,
        documents: &[(String, String)],
    ) -> Result<Option<Value>> {
        if documents.is_empty() || !self.preflight_graph_runtime()? {
            return Ok(None);
        }
        let response = if crate::external_graph::passive_status(&self.scope, &self.database)?
            ["status"]
            == "pending"
        {
            crate::work::start_graph_projection(&self.scope, &self.database)?
        } else {
            crate::work::start_graph_documents(&self.scope, &self.database, documents)?
        };
        Ok(Some(response["work"].clone()))
    }

    pub(crate) fn preflight_graph_runtime(&self) -> Result<bool> {
        if config::resolve(&self.scope, &self.database)?.setting == GraphSetting::Disabled {
            return Ok(false);
        }
        crate::scope::require_database_runtime_root(&self.database)?;
        Ok(true)
    }

    fn graph_owner_document(&self, identifier: &str) -> Result<(String, String)> {
        let identifier = resolve_graph_node(&self.conn, identifier)?;
        if let Some(slug) = identifier.strip_prefix("page:") {
            return Ok(("page".to_string(), slug.to_string()));
        }
        if let Some(id) = identifier.strip_prefix("source:") {
            return Ok(("source".to_string(), id.to_string()));
        }
        self.conn
            .query_row(
                "SELECT document_type, document_identifier FROM search_spans
                 WHERE span_id = ?1 AND active = 1",
                [identifier],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
    }

    fn apply_graph_reranking(&self, results: &mut [SearchResult]) -> Result<()> {
        let seed_slugs = results
            .iter()
            .filter(|result| result.result_type == "page")
            .take(3)
            .map(|result| result.identifier.clone())
            .collect::<Vec<_>>();
        if seed_slugs.is_empty() {
            return Ok(());
        }
        let pages = self.load_graph_pages()?;
        let pages_by_slug = pages
            .iter()
            .map(|page| (page.slug.as_str(), page))
            .collect::<BTreeMap<_, _>>();
        let mut evidence: BTreeMap<String, Vec<GraphSeedEvidence>> = BTreeMap::new();
        for (position, seed_slug) in seed_slugs.iter().enumerate() {
            let Some(seed) = pages_by_slug.get(seed_slug.as_str()) else {
                continue;
            };
            let discount = 1.0 / (position as f64 + 1.0);
            for related_page in related(seed, &pages, pages.len()) {
                let search_score =
                    related_page.direct_link_score + related_page.shared_source_score;
                if search_score <= 0.0 {
                    continue;
                }
                let contribution = search_score / (search_score + 4.0) * discount;
                evidence
                    .entry(related_page.slug)
                    .or_default()
                    .push(GraphSeedEvidence {
                        slug: seed_slug.clone(),
                        raw_score: search_score,
                        contribution,
                    });
            }
        }

        for result in results
            .iter_mut()
            .filter(|result| result.result_type == "page")
        {
            let Some(explanation) = result.explanation.as_mut() else {
                continue;
            };
            explanation.graph_seeds = evidence.remove(&result.identifier).unwrap_or_default();
            explanation.signals.graph_match = explanation
                .graph_seeds
                .iter()
                .map(|seed| seed.contribution)
                .fold(0.0, f64::max)
                .clamp(0.0, 1.0);
            explanation.signals.graph_hub_penalty = pages_by_slug
                .get(result.identifier.as_str())
                .filter(|_| explanation.signals.generic_marker > 0.0)
                .map(|page| {
                    let degree = page.outlinks.len() as f64;
                    degree / (degree + 4.0)
                })
                .unwrap_or(0.0);
            explanation.contributions.graph = -GRAPH_MATCH_WEIGHT * explanation.signals.graph_match
                + GRAPH_HUB_WEIGHT * explanation.signals.graph_hub_penalty;
            explanation.final_rank = explanation.base_rank + explanation.contributions.total();
            result.rank = explanation.final_rank;
        }
        Ok(())
    }

    pub fn record_search(&mut self, query: &str, limit: usize) -> Result<()> {
        self.record_top_level_operation("search", query, json!({ "limit": limit }))
    }

    pub fn retrieval_weight_set(
        &mut self,
        target_type: &str,
        target_identifier: &str,
        weight: i32,
        reason: &str,
        provenance: &str,
    ) -> Result<RetrievalWeightResponse> {
        validate_retrieval_weight(weight)?;
        validate_retrieval_provenance(provenance)?;
        validate_nonempty_reason(reason)?;
        let identifier = normalize_retrieval_target(&self.conn, target_type, target_identifier)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            &format!(
                "INSERT INTO retrieval_weights(
                    target_type, target_identifier, provenance, weight, reason, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, {TIMESTAMP_SQL})
                 ON CONFLICT(target_type, target_identifier, provenance)
                 DO UPDATE SET weight = excluded.weight,
                               reason = excluded.reason,
                               updated_at = excluded.updated_at"
            ),
            params![target_type, &identifier, provenance, weight, reason.trim()],
        )?;
        record_operation(
            &tx,
            "weight_set",
            &format!("{target_type}:{identifier}"),
            &json!({"provenance": provenance, "weight": weight, "reason": reason.trim()}),
        )?;
        tx.commit()?;
        Ok(RetrievalWeightResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            adjustment: self.load_retrieval_adjustment(target_type, &identifier, provenance)?,
        })
    }

    pub fn retrieval_weight_list(
        &self,
        target_type: &str,
        target_identifier: &str,
    ) -> Result<RetrievalWeightListResponse> {
        let identifier = normalize_retrieval_target(&self.conn, target_type, target_identifier)?;
        let mut statement = self.conn.prepare(
            "SELECT target_type, target_identifier, provenance, weight, reason, updated_at
             FROM retrieval_weights
             WHERE target_type = ?1 AND target_identifier = ?2
             ORDER BY CASE provenance WHEN 'user-provided' THEN 0 ELSE 1 END",
        )?;
        let adjustments = statement
            .query_map(params![target_type, &identifier], read_retrieval_adjustment)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(RetrievalWeightListResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            effective: adjustments.first().cloned(),
            adjustments,
        })
    }

    pub fn retrieval_weight_clear(
        &mut self,
        target_type: &str,
        target_identifier: &str,
        provenance: &str,
    ) -> Result<RetrievalClearResponse> {
        validate_retrieval_provenance(provenance)?;
        let identifier = normalize_retrieval_target(&self.conn, target_type, target_identifier)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM retrieval_weights
             WHERE target_type = ?1 AND target_identifier = ?2 AND provenance = ?3",
            params![target_type, &identifier, provenance],
        )? > 0;
        record_operation(
            &tx,
            "weight_clear",
            &format!("{target_type}:{identifier}"),
            &json!({"provenance": provenance, "removed": removed}),
        )?;
        tx.commit()?;
        Ok(RetrievalClearResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            removed,
        })
    }

    pub fn retrieval_feedback_set(
        &mut self,
        target_type: &str,
        target_identifier: &str,
        query: &str,
        signal: i32,
        reason: &str,
        provenance: &str,
    ) -> Result<RetrievalFeedbackResponse> {
        if !matches!(signal, -1 | 1) {
            return Err(AppError::new(
                "invalid_feedback",
                "feedback signal must be relevant or irrelevant",
            ));
        }
        validate_retrieval_provenance(provenance)?;
        validate_nonempty_reason(reason)?;
        let tokens = tokenize_for_query(query);
        if tokens.is_empty() {
            return Err(AppError::new(
                "invalid_query",
                "feedback query must contain searchable terms",
            ));
        }
        let fingerprint = query_fingerprint(&tokens);
        let identifier = normalize_retrieval_target(&self.conn, target_type, target_identifier)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            &format!(
                "INSERT INTO retrieval_feedback(
                    query_fingerprint, target_type, target_identifier, provenance,
                    signal, reason, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, {TIMESTAMP_SQL})
                 ON CONFLICT(query_fingerprint, target_type, target_identifier, provenance)
                 DO UPDATE SET signal = excluded.signal,
                               reason = excluded.reason,
                               updated_at = excluded.updated_at"
            ),
            params![
                &fingerprint,
                target_type,
                &identifier,
                provenance,
                signal,
                reason.trim()
            ],
        )?;
        record_operation(
            &tx,
            "weight_feedback",
            &format!("{target_type}:{identifier}"),
            &json!({
                "query_fingerprint": fingerprint,
                "provenance": provenance,
                "signal": signal,
                "reason": reason.trim()
            }),
        )?;
        tx.commit()?;
        self.load_retrieval_feedback(&fingerprint, target_type, &identifier, provenance)
    }

    pub fn retrieval_feedback_clear(
        &mut self,
        target_type: &str,
        target_identifier: &str,
        query: &str,
        provenance: &str,
    ) -> Result<RetrievalClearResponse> {
        validate_retrieval_provenance(provenance)?;
        let tokens = tokenize_for_query(query);
        if tokens.is_empty() {
            return Err(AppError::new(
                "invalid_query",
                "feedback query must contain searchable terms",
            ));
        }
        let fingerprint = query_fingerprint(&tokens);
        let identifier = normalize_retrieval_target(&self.conn, target_type, target_identifier)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM retrieval_feedback
             WHERE query_fingerprint = ?1
               AND target_type = ?2
               AND target_identifier = ?3
               AND provenance = ?4",
            params![&fingerprint, target_type, &identifier, provenance],
        )? > 0;
        record_operation(
            &tx,
            "weight_feedback_clear",
            &format!("{target_type}:{identifier}"),
            &json!({
                "query_fingerprint": fingerprint,
                "provenance": provenance,
                "removed": removed
            }),
        )?;
        tx.commit()?;
        Ok(RetrievalClearResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            removed,
        })
    }

    pub fn ingest_list(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<IngestListResponse> {
        if let Some(status) = status {
            validate_ingest_status(status)?;
        }
        let mut statement = self.conn.prepare(
            "SELECT source_id, status, attempts, last_error,
                    no_derived_pages_reason, updated_at
             FROM ingest_jobs
             WHERE ?1 IS NULL OR status = ?1
             ORDER BY source_id ASC
             LIMIT ?2 OFFSET ?3",
        )?;
        let mut jobs = statement
            .query_map(
                params![status, (limit + 1) as i64, offset as i64],
                read_ingest_job_summary,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = jobs.len() > limit;
        jobs.truncate(limit);
        Ok(IngestListResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            jobs,
            limit,
            offset,
            has_more,
        })
    }

    pub fn ingest_next(
        &mut self,
        context_limit: usize,
        source_max_chars: Option<usize>,
    ) -> Result<IngestPacket> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let source_id = tx
            .query_row(
                "SELECT source_id
                 FROM ingest_jobs
                 WHERE status = 'pending'
                 ORDER BY source_id ASC
                 LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(source_id) = source_id {
            claim_ingest_job(&tx, source_id)?;
        }
        tx.commit()?;

        self.ingest_packet(source_id, context_limit, source_max_chars)
    }

    pub fn ingest_claim(
        &mut self,
        source_id: i64,
        context_limit: usize,
        source_max_chars: Option<usize>,
    ) -> Result<IngestPacket> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ingest_state(&tx, source_id, &["pending"])?;
        claim_ingest_job(&tx, source_id)?;
        tx.commit()?;
        self.ingest_packet(Some(source_id), context_limit, source_max_chars)
    }

    pub fn ingest_analyze(
        &mut self,
        source_id: i64,
        analysis: &str,
    ) -> Result<IngestMutationResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ingest_state(&tx, source_id, &["analyzing"])?;
        tx.execute(
            &format!(
                "UPDATE ingest_jobs
                 SET status = 'generating',
                     analysis = ?2,
                     last_error = NULL,
                     updated_at = {TIMESTAMP_SQL}
                 WHERE source_id = ?1"
            ),
            params![source_id, analysis],
        )?;
        record_operation(&tx, "ingest_analyze", &source_id.to_string(), &json!({}))?;
        tx.commit()?;
        self.ingest_mutation_response(source_id)
    }

    pub fn ingest_complete(
        &mut self,
        source_id: i64,
        no_derived_pages_reason: Option<&str>,
    ) -> Result<IngestMutationResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ingest_state(&tx, source_id, &["generating"])?;
        let source_summary_pages: i64 = tx.query_row(
            "SELECT COUNT(*)
             FROM page_sources ps
             JOIN pages p ON p.slug = ps.page_slug
             WHERE ps.source_id = ?1 AND LOWER(COALESCE(p.kind, '')) = 'source'",
            params![source_id],
            |row| row.get(0),
        )?;
        if source_summary_pages == 0 {
            return Err(AppError::new(
                "source_summary_missing",
                format!(
                    "source {source_id} needs at least one cited page with kind `source` before completion"
                ),
            ));
        }
        let derived_pages: i64 = tx.query_row(
            "SELECT COUNT(*)
             FROM page_sources ps
             JOIN pages p ON p.slug = ps.page_slug
             WHERE ps.source_id = ?1 AND LOWER(COALESCE(p.kind, '')) <> 'source'",
            params![source_id],
            |row| row.get(0),
        )?;
        let reason = no_derived_pages_reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        if derived_pages == 0 && reason.is_none() {
            return Err(AppError::new(
                "ingest_integration_required",
                format!(
                    "source {source_id} needs at least one cited non-source page; if no shared knowledge changes, retry with --no-derived-pages-reason"
                ),
            ));
        }
        if derived_pages > 0 && reason.is_some() {
            return Err(AppError::new(
                "invalid_input",
                "--no-derived-pages-reason is only valid when no non-source page cites this source",
            ));
        }
        tx.execute(
            &format!(
                "UPDATE ingest_jobs
                 SET status = 'completed',
                     last_error = NULL,
                     no_derived_pages_reason = ?2,
                     updated_at = {TIMESTAMP_SQL}
                 WHERE source_id = ?1"
            ),
            params![source_id, reason],
        )?;
        record_operation(
            &tx,
            "ingest_complete",
            &source_id.to_string(),
            &json!({
                "source_summary_pages": source_summary_pages,
                "derived_pages": derived_pages,
                "no_derived_pages_reason": reason,
            }),
        )?;
        tx.commit()?;
        let mut response = self.ingest_mutation_response(source_id)?;
        response.integration = Some(IngestIntegration {
            source_summary_pages: source_summary_pages as usize,
            derived_pages: derived_pages as usize,
            no_derived_pages_reason: reason.map(str::to_string),
        });
        Ok(response)
    }

    pub fn ingest_fail(&mut self, source_id: i64, message: &str) -> Result<IngestMutationResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ingest_state(&tx, source_id, &["analyzing", "generating"])?;
        tx.execute(
            &format!(
                "UPDATE ingest_jobs
                 SET status = 'failed',
                     last_error = ?2,
                     updated_at = {TIMESTAMP_SQL}
                 WHERE source_id = ?1"
            ),
            params![source_id, message],
        )?;
        record_operation(
            &tx,
            "ingest_fail",
            &source_id.to_string(),
            &json!({ "message": message }),
        )?;
        tx.commit()?;
        self.ingest_mutation_response(source_id)
    }

    pub fn ingest_retry(&mut self, source_id: i64) -> Result<IngestMutationResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_ingest_state(&tx, source_id, &["analyzing", "generating", "failed"])?;
        tx.execute(
            &format!(
                "UPDATE ingest_jobs
                 SET status = 'pending',
                     last_error = NULL,
                     updated_at = {TIMESTAMP_SQL}
                 WHERE source_id = ?1"
            ),
            params![source_id],
        )?;
        record_operation(&tx, "ingest_retry", &source_id.to_string(), &json!({}))?;
        tx.commit()?;
        self.ingest_mutation_response(source_id)
    }

    pub fn graph_related(&self, slug: &str, limit: usize) -> Result<GraphRelatedResponse> {
        let pages = self.load_graph_pages()?;
        let seed = pages
            .iter()
            .find(|page| page.slug == slug)
            .ok_or_else(|| AppError::new("page_not_found", format!("page not found: {slug}")))?;
        let related = related(seed, &pages, limit)
            .into_iter()
            .map(|page| RelatedPageResponse {
                slug: page.slug,
                title: page.title,
                kind: page.kind,
                direct_links: (page.direct_link_score / 3.0).round() as usize,
                shared_sources: (page.shared_source_score / 4.0).round() as usize,
                adamic_adar: page.common_neighbor_score / 1.5,
                type_affinity: page.type_affinity_score,
                score: page.total_score,
            })
            .collect();
        Ok(GraphRelatedResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            seed: slug.to_string(),
            related,
        })
    }

    pub fn reindex(&mut self) -> Result<ReindexResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (sources, pages) = rebuild_search_index(&tx)?;
        record_operation(
            &tx,
            "reindex",
            "search_fts",
            &json!({ "sources": sources, "pages": pages }),
        )?;
        tx.commit()?;
        Ok(ReindexResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            sources,
            pages,
        })
    }

    pub fn compact(&mut self) -> Result<CompactResponse> {
        let wal_path = PathBuf::from(format!("{}-wal", self.database.display()));
        let before_bytes = fs::metadata(&wal_path).map(|meta| meta.len()).unwrap_or(0);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        record_operation(
            &tx,
            "maintenance_compact",
            "wiki.db",
            &json!({
                "wal_before_bytes": before_bytes,
            }),
        )?;
        tx.commit()?;
        let (busy, log_frames, checkpointed_frames) = wal_checkpoint_truncate(&self.conn, false)?;
        let after_bytes = fs::metadata(&wal_path).map(|meta| meta.len()).unwrap_or(0);
        Ok(CompactResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            busy: busy != 0,
            log_frames,
            checkpointed_frames,
            before_bytes,
            after_bytes,
        })
    }

    pub(crate) fn try_checkpoint_wal(&self) -> bool {
        wal_checkpoint_truncate(&self.conn, false).is_ok_and(|(busy, _, _)| busy == 0)
    }

    pub fn checkpoint_create(&self, name: &str) -> Result<CheckpointResponse> {
        validate_checkpoint_name(name)?;
        let path = checkpoint_path(&self.database, name)?;
        create_checkpoint(&self.conn, &path)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn checkpoint_list(&self) -> Result<CheckpointListResponse> {
        let directory = checkpoint_directory(&self.database)?;
        let mut checkpoints = Vec::new();
        if directory.is_dir() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || path.extension().and_then(|value| value.to_str()) != Some("db")
                {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
                    continue;
                };
                checkpoints.push(CheckpointRecord {
                    name: name.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    bytes: metadata.len(),
                });
            }
        }
        checkpoints.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(CheckpointListResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoints,
        })
    }

    pub fn checkpoint_restore(&mut self, name: &str) -> Result<CheckpointResponse> {
        validate_checkpoint_name(name)?;
        let path = checkpoint_path(&self.database, name)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            AppError::new(
                "checkpoint_not_found",
                format!("checkpoint {name} is unavailable: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::new(
                "checkpoint_invalid",
                format!("checkpoint {name} is not a regular file"),
            ));
        }

        let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_only_connection(&source, BUSY_TIMEOUT)?;
        prepare_store_read_only(&source)?;

        let safety_checkpoint = fresh_checkpoint_name(&self.database, "pre-restore")?;
        let safety_path = checkpoint_path(&self.database, &safety_checkpoint)?;
        create_checkpoint(&self.conn, &safety_path)?;

        {
            let backup = Backup::new(&source, &mut self.conn)?;
            backup.run_to_completion(100, Duration::from_millis(10), None)?;
        }
        validate_store(&self.conn)?;
        self.record_top_level_operation(
            "checkpoint_restore",
            name,
            json!({ "safety_checkpoint": safety_checkpoint }),
        )?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: Some(safety_checkpoint),
        })
    }
}
