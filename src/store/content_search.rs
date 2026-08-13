impl Store {
    pub fn schema_set(&mut self, schema: &str) -> Result<SchemaResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('schema', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![schema],
        )?;
        record_operation(&tx, "schema_set", "schema", &json!({}))?;
        tx.commit()?;
        self.schema_show()
    }

    pub fn schema_show(&self) -> Result<SchemaResponse> {
        Ok(SchemaResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            schema: self.schema_text()?,
        })
    }

    pub fn purpose_set(&mut self, purpose: &str) -> Result<PurposeResponse> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('purpose', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![purpose],
        )?;
        record_operation(&tx, "purpose_set", "purpose", &json!({}))?;
        tx.commit()?;
        self.purpose_show()
    }

    pub fn purpose_show(&self) -> Result<PurposeResponse> {
        Ok(PurposeResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            purpose: self.purpose_text()?,
        })
    }

    pub fn source_add(&mut self, input: SourceAddInput) -> Result<SourceAddResponse> {
        self.source_add_many(vec![input])?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::new("invalid_input", "source input is missing"))
    }

    pub fn source_add_many(
        &mut self,
        inputs: Vec<SourceAddInput>,
    ) -> Result<Vec<SourceAddResponse>> {
        if inputs.is_empty() {
            return Err(AppError::new(
                "invalid_input",
                "at least one source is required",
            ));
        }
        self.source_add_prepared(inputs.into_iter().map(Ok))
    }

    pub(crate) fn source_add_stream<I>(&mut self, inputs: I) -> Result<Vec<SourceAddResponse>>
    where
        I: IntoIterator<Item = Result<Option<SourceAddInput>>>,
    {
        let (mut stage, stage_path) = create_source_stage(&self.database)?;
        let result = (|| {
            let mut count = 0usize;
            {
                let mut writer = BufWriter::new(&mut stage);
                for input in inputs {
                    if let Some(input) = input? {
                        serde_json::to_writer(&mut writer, &input)
                            .map_err(|error| AppError::new("staging_error", error.to_string()))?;
                        writer.write_all(b"\n")?;
                        count += 1;
                    }
                }
                writer.flush()?;
            }
            if count == 0 {
                return Ok(Vec::new());
            }
            stage.rewind()?;
            let inputs = serde_json::Deserializer::from_reader(BufReader::new(&mut stage))
                .into_iter::<SourceAddInput>()
                .map(|input| {
                    input.map_err(|error| AppError::new("staging_error", error.to_string()))
                });
            self.source_add_prepared(inputs)
        })();
        drop(stage);
        match fs::remove_file(stage_path) {
            Ok(()) => result,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => result,
            Err(_) if result.is_err() => result,
            Err(error) => Err(error.into()),
        }
    }

    fn source_add_prepared<I>(&mut self, inputs: I) -> Result<Vec<SourceAddResponse>>
    where
        I: IntoIterator<Item = Result<SourceAddInput>>,
    {
        let mutation_started = Instant::now();
        self.preflight_graph_runtime()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = Vec::new();
        let mut touched_paths = BTreeSet::new();
        for input in inputs {
            let input = input?;
            if let Some(path) = input.tracked_path.as_ref() {
                touched_paths.insert(path.clone());
            }
            inserted.push(insert_source(&tx, &input)?);
        }
        tx.commit()?;
        let canonical_duration_ms = elapsed_millis(mutation_started);
        let projection_started = Instant::now();
        self.reconcile_graph_projection()?;
        let projection_duration_ms = elapsed_millis(projection_started);
        let mut documents = inserted
            .iter()
            .map(|(source_id, _)| ("source".to_string(), source_id.to_string()))
            .collect::<Vec<_>>();
        for path in touched_paths {
            if let Some(source_id) = self
                .conn
                .query_row(
                    "SELECT source_id FROM source_path_revisions
                     WHERE tracked_path = ?1 ORDER BY revision DESC LIMIT 1 OFFSET 1",
                    [path],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
            {
                documents.push(("source".to_string(), source_id.to_string()));
            }
        }
        let work = self.schedule_graph_documents(&documents)?;

        inserted
            .into_iter()
            .map(|(source_id, created)| {
                Ok(SourceAddResponse {
                    scope: self.scope.clone(),
                    database: self.database_string(),
                    source: self.load_source_summary(source_id)?,
                    created,
                    graph: self
                        .load_graph_mutation_summary(
                            "source",
                            &source_id.to_string(),
                            canonical_duration_ms,
                            projection_duration_ms,
                        )?
                        .with_work(work.clone()),
                })
            })
            .collect()
    }

    pub fn source_list(&self, limit: usize, offset: usize) -> Result<SourceListResponse> {
        let mut statement = self.conn.prepare(
            "SELECT id, title, origin, content_hash,
                    LENGTH(CAST(content AS BLOB)), created_at
             FROM sources
             ORDER BY id ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let mut sources = statement
            .query_map(
                params![(limit + 1) as i64, offset as i64],
                read_source_summary,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = sources.len() > limit;
        sources.truncate(limit);

        Ok(SourceListResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            sources,
            limit,
            offset,
            has_more,
        })
    }

    pub fn source_status_targets(
        &mut self,
        source_ids: Vec<i64>,
        all: bool,
    ) -> Result<SourceStatusTargets> {
        let source_ids = dedupe_i64(source_ids);
        if !all && source_ids.is_empty() {
            return Err(AppError::new(
                "invalid_input",
                "source status requires at least one source ID or --all",
            ));
        }

        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let mut targets = Vec::new();
        let mut untracked_source_ids = Vec::new();
        if all {
            let mut statement = tx.prepare(
                "SELECT r.source_id, r.tracked_path, r.source_id, r.revision,
                        s.content_hash
                 FROM source_path_revisions r
                 JOIN sources s ON s.id = r.source_id
                 WHERE r.revision = (
                     SELECT MAX(head.revision)
                     FROM source_path_revisions head
                     WHERE head.tracked_path = r.tracked_path
                 )
                 ORDER BY r.tracked_path",
            )?;
            targets = statement
                .query_map([], read_source_status_target)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut untracked = tx.prepare(
                "SELECT s.id
                 FROM sources s
                 WHERE NOT EXISTS (
                     SELECT 1 FROM source_path_revisions r WHERE r.source_id = s.id
                 )
                 ORDER BY s.id",
            )?;
            untracked_source_ids = untracked
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        } else {
            for source_id in source_ids {
                let exists = tx
                    .query_row(
                        "SELECT 1 FROM sources WHERE id = ?1",
                        params![source_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    return Err(AppError::new(
                        "source_not_found",
                        format!("source not found: {source_id}"),
                    ));
                }
                let mut statement = tx.prepare(
                    "SELECT ?1, paths.tracked_path, head.source_id, head.revision,
                            s.content_hash
                     FROM (
                         SELECT DISTINCT tracked_path
                         FROM source_path_revisions
                         WHERE source_id = ?1
                     ) paths
                     JOIN source_path_revisions head
                       ON head.tracked_path = paths.tracked_path
                      AND head.revision = (
                          SELECT MAX(latest.revision)
                          FROM source_path_revisions latest
                          WHERE latest.tracked_path = paths.tracked_path
                      )
                     JOIN sources s ON s.id = head.source_id
                     ORDER BY paths.tracked_path",
                )?;
                let source_targets = statement
                    .query_map(params![source_id], read_source_status_target)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if source_targets.is_empty() {
                    untracked_source_ids.push(source_id);
                } else {
                    targets.extend(source_targets);
                }
            }
        }

        tx.commit()?;
        Ok(SourceStatusTargets {
            targets,
            untracked_source_ids,
        })
    }

    pub fn source_show(
        &self,
        id: i64,
        offset_chars: usize,
        max_chars: Option<usize>,
    ) -> Result<SourceShowResponse> {
        let (source, window) = window_source(self.load_source(id)?, offset_chars, max_chars)?;
        Ok(SourceShowResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            source,
            window,
        })
    }

    pub fn source_for_diff(&self, id: i64, max_bytes: usize) -> Result<SourceRecord> {
        let summary = self.load_source_summary(id)?;
        if summary.bytes < 0 || summary.bytes as usize > max_bytes {
            return Err(AppError::new(
                "source_diff_too_large",
                format!(
                    "source {id} is {} bytes; maximum source diff input is {max_bytes} bytes",
                    summary.bytes
                ),
            ));
        }
        self.load_source(id)
    }

    pub fn source_refs(&self, id: i64, limit: usize, offset: usize) -> Result<SourceRefsResponse> {
        let source = self.load_source_summary(id)?;
        let mut statement = self.conn.prepare(
            "SELECT p.slug, p.title, p.kind, p.summary, p.updated_at,
                    EXISTS(
                        SELECT 1 FROM page_sources cited WHERE cited.page_slug = p.slug
                    ),
                    (
                        SELECT GROUP_CONCAT(pp.provenance, ',')
                        FROM page_provenance pp
                        WHERE pp.page_slug = p.slug
                    )
             FROM page_sources ps
             JOIN pages p ON p.slug = ps.page_slug
             WHERE ps.source_id = ?1
             ORDER BY p.slug
             LIMIT ?2 OFFSET ?3",
        )?;
        let mut pages = statement
            .query_map(params![id, (limit + 1) as i64, offset as i64], |row| {
                Ok(PageSummary {
                    slug: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    updated_at: row.get(4)?,
                    provenance: provenance_from_parts(row.get::<_, i64>(5)? != 0, row.get(6)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = pages.len() > limit;
        pages.truncate(limit);
        Ok(SourceRefsResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            source,
            pages,
            limit,
            offset,
            has_more,
        })
    }

    pub fn source_remove(&mut self, id: i64) -> Result<SourceRemoveResponse> {
        self.preflight_graph_runtime()?;
        self.load_source_summary(id)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let references: i64 = tx.query_row(
            "SELECT COUNT(*) FROM page_sources WHERE source_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if references > 0 {
            return Err(AppError::new(
                "source_in_use",
                format!("source {id} is cited by {references} Wiki page(s)"),
            ));
        }
        let paths = {
            let mut statement = tx.prepare(
                "SELECT paths.tracked_path, head.source_id
                 FROM (
                     SELECT DISTINCT tracked_path
                     FROM source_path_revisions
                     WHERE source_id = ?1
                 ) paths
                 JOIN source_path_revisions head
                   ON head.tracked_path = paths.tracked_path
                  AND head.revision = (
                      SELECT MAX(latest.revision)
                      FROM source_path_revisions latest
                      WHERE latest.tracked_path = paths.tracked_path
                  )
                 ORDER BY paths.tracked_path",
            )?;
            statement
                .query_map(params![id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let affected_paths = paths
            .iter()
            .map(|(tracked_path, _)| tracked_path.clone())
            .collect::<Vec<_>>();
        let mut removed_path_revisions = 0;
        let mut untracked_paths = Vec::new();
        for (tracked_path, head_source_id) in paths {
            if head_source_id == id {
                removed_path_revisions += tx.execute(
                    "DELETE FROM source_path_revisions WHERE tracked_path = ?1",
                    params![&tracked_path],
                )?;
                untracked_paths.push(tracked_path);
            } else {
                removed_path_revisions += tx.execute(
                    "DELETE FROM source_path_revisions
                     WHERE tracked_path = ?1 AND source_id = ?2",
                    params![&tracked_path, id],
                )?;
            }
        }
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
            params![id.to_string()],
        )?;
        deactivate_search_spans(&tx, "source", &id.to_string())?;
        tx.execute(
            "DELETE FROM retrieval_weights
             WHERE target_type = 'source' AND target_identifier = ?1",
            params![id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM retrieval_feedback
             WHERE target_type = 'source' AND target_identifier = ?1",
            params![id.to_string()],
        )?;
        tx.execute("DELETE FROM sources WHERE id = ?1", params![id])?;
        record_operation(
            &tx,
            "source_remove",
            &id.to_string(),
            &json!({
                "removed_path_revisions": removed_path_revisions,
                "untracked_paths": untracked_paths,
                "affected_paths": affected_paths,
            }),
        )?;
        tx.commit()?;
        self.reconcile_graph_projection()?;
        let graph_work =
            self.schedule_graph_documents(&[("source".to_string(), id.to_string())])?;
        Ok(SourceRemoveResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            source_id: id,
            removed: true,
            removed_path_revisions,
            untracked_paths,
            graph_work,
        })
    }

    pub fn page_put(&mut self, input: PagePutInput) -> Result<PagePutResponse> {
        let mutation_started = Instant::now();
        validate_page_slug(&input.slug)?;
        self.preflight_graph_runtime()?;
        let source_ids = dedupe_i64(input.source_ids);
        let explicit_provenance = normalize_explicit_provenance(input.provenance)?;
        let links = extract_links(&input.body);
        let structural_navigation = has_structural_navigation_marker(&input.body);
        let base = load_page_mutation_base(&self.conn, &input.slug)?;
        let desired_fingerprint = page_content_fingerprint(
            &input.title,
            input.kind.as_deref(),
            input.summary.as_deref(),
            &input.body,
            structural_navigation,
            &source_ids,
            &explicit_provenance,
            &links,
        );
        if base
            .as_ref()
            .is_some_and(|base| base.content_fingerprint == desired_fingerprint)
        {
            return Ok(PagePutResponse {
                scope: self.scope.clone(),
                database: self.database_string(),
                page: self.load_page_write(&input.slug)?,
                created: false,
                graph: self.load_graph_mutation_summary(
                    "page",
                    &input.slug,
                    elapsed_millis(mutation_started),
                    0,
                )?,
            });
        }
        if let Ok(delay) = std::env::var("LWC_TEST_PAGE_PUT_PREWRITE_DELAY_MS")
            && let Ok(delay) = delay.parse::<u64>()
        {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let locked_base = load_page_mutation_base(&tx, &input.slug)?;
        if locked_base.as_ref().map(|base| &base.version_fingerprint)
            != base.as_ref().map(|base| &base.version_fingerprint)
        {
            return Err(AppError::new(
                "entity_conflict",
                format!(
                    "page {} changed while the replacement was prepared",
                    input.slug
                ),
            )
            .with_details(json!({
                "entity_type": "page",
                "identifier": input.slug,
            })));
        }
        validate_sources(&tx, &source_ids)?;
        let existed = base.is_some();

        if existed {
            tx.execute(
                &format!(
                    "UPDATE pages
                     SET title = ?2, kind = ?3, summary = ?4, body = ?5,
                         structural_navigation = ?6, updated_at = {TIMESTAMP_SQL}
                     WHERE slug = ?1"
                ),
                params![
                    &input.slug,
                    &input.title,
                    input.kind.as_deref(),
                    input.summary.as_deref(),
                    &input.body,
                    structural_navigation
                ],
            )?;
        } else {
            tx.execute(
                &format!(
                    "INSERT INTO pages(
                        slug, title, kind, summary, body, structural_navigation,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, {TIMESTAMP_SQL}, {TIMESTAMP_SQL})"
                ),
                params![
                    &input.slug,
                    &input.title,
                    input.kind.as_deref(),
                    input.summary.as_deref(),
                    &input.body,
                    structural_navigation
                ],
            )?;
        }

        tx.execute(
            "DELETE FROM page_sources WHERE page_slug = ?1",
            params![&input.slug],
        )?;
        for source_id in &source_ids {
            tx.execute(
                "INSERT INTO page_sources(page_slug, source_id) VALUES (?1, ?2)",
                params![&input.slug, source_id],
            )?;
        }

        tx.execute(
            "DELETE FROM page_provenance WHERE page_slug = ?1",
            params![&input.slug],
        )?;
        for provenance in &explicit_provenance {
            tx.execute(
                "INSERT INTO page_provenance(page_slug, provenance) VALUES (?1, ?2)",
                params![&input.slug, provenance],
            )?;
        }

        tx.execute(
            "DELETE FROM links WHERE from_slug = ?1",
            params![&input.slug],
        )?;
        for link in &links {
            tx.execute(
                "INSERT INTO links(from_slug, to_slug) VALUES (?1, ?2)",
                params![&input.slug, link],
            )?;
        }

        index_page(
            &tx,
            None,
            &input.slug,
            &input.title,
            input.summary.as_deref(),
            &input.body,
        )?;
        let invalidated_semantic_relations = tx.execute(
            "DELETE FROM semantic_relations
             WHERE from_identifier IN (
                    SELECT span_id FROM search_spans
                    WHERE document_type = 'page' AND document_identifier = ?1 AND active = 0
                 )
                OR to_identifier IN (
                    SELECT span_id FROM search_spans
                    WHERE document_type = 'page' AND document_identifier = ?1 AND active = 0
                 )",
            [&input.slug],
        )?;

        record_operation(
            &tx,
            "page_put",
            &input.slug,
            &json!({ "created": !existed }),
        )?;
        tx.commit()?;
        let canonical_duration_ms = elapsed_millis(mutation_started);
        let projection_started = Instant::now();
        self.reconcile_graph_projection()?;
        let projection_duration_ms = elapsed_millis(projection_started);

        let mut graph = self.load_graph_mutation_summary(
            "page",
            &input.slug,
            canonical_duration_ms,
            projection_duration_ms,
        )?;
        graph.invalidated_semantic_relations = invalidated_semantic_relations;
        graph.work = self.schedule_graph_documents(&[("page".to_string(), input.slug.clone())])?;
        Ok(PagePutResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            page: self.load_page_write(&input.slug)?,
            created: !existed,
            graph,
        })
    }

    pub fn page_list(&self, limit: usize, offset: usize) -> Result<PageListResponse> {
        let mut pages = self.load_page_summaries(limit + 1, offset)?;
        let has_more = pages.len() > limit;
        pages.truncate(limit);
        Ok(PageListResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            pages,
            limit,
            offset,
            has_more,
        })
    }

    pub fn page_show(&self, slug: &str) -> Result<PageShowResponse> {
        Ok(PageShowResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            page: self.load_page(slug)?,
        })
    }

    pub fn page_links(&self, slug: &str) -> Result<PageLinksResponse> {
        self.load_page(slug)?;
        let outgoing = self.load_page_links(slug)?;
        let backlinks = {
            let mut statement = self.conn.prepare(
                "SELECT from_slug
                 FROM links
                 WHERE to_slug = ?1
                 ORDER BY from_slug",
            )?;
            statement
                .query_map(params![slug], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let missing = {
            let mut statement = self.conn.prepare(
                "SELECT l.to_slug
                 FROM links l
                 LEFT JOIN pages p ON p.slug = l.to_slug
                 WHERE l.from_slug = ?1 AND p.slug IS NULL
                 ORDER BY l.to_slug",
            )?;
            statement
                .query_map(params![slug], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(PageLinksResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            page: slug.to_string(),
            outgoing,
            backlinks,
            missing,
        })
    }

    pub fn page_remove(&mut self, slug: &str) -> Result<PageRemoveResponse> {
        self.preflight_graph_runtime()?;
        self.load_page(slug)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let references: i64 = tx.query_row(
            "SELECT COUNT(*) FROM links WHERE to_slug = ?1 AND from_slug <> ?1",
            params![slug],
            |row| row.get(0),
        )?;
        if references > 0 {
            return Err(AppError::new(
                "page_in_use",
                format!("page {slug} has {references} inbound Wiki link(s)"),
            ));
        }
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
            params![slug],
        )?;
        deactivate_search_spans(&tx, "page", slug)?;
        tx.execute(
            "DELETE FROM retrieval_weights
             WHERE target_type = 'page' AND target_identifier = ?1",
            params![slug],
        )?;
        tx.execute(
            "DELETE FROM retrieval_feedback
             WHERE target_type = 'page' AND target_identifier = ?1",
            params![slug],
        )?;
        record_operation(&tx, "page_remove", slug, &json!({}))?;
        tx.execute("DELETE FROM pages WHERE slug = ?1", params![slug])?;
        tx.commit()?;
        self.reconcile_graph_projection()?;
        let graph_work =
            self.schedule_graph_documents(&[("page".to_string(), slug.to_string())])?;
        Ok(PageRemoveResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            slug: slug.to_string(),
            removed: true,
            graph_work,
        })
    }

    #[cfg(test)]
    pub fn search(&self, query: &str, limit: usize) -> Result<SearchResponse> {
        self.search_with_options(
            query,
            limit,
            &SearchOptions {
                mode: SearchMode::All,
                granularity: SearchGranularity::Document,
                grouping: SearchGrouping::None,
                kinds: Vec::new(),
                explain: false,
            },
        )
    }

    pub fn search_with_options(
        &self,
        query: &str,
        limit: usize,
        options: &SearchOptions,
    ) -> Result<SearchResponse> {
        let tokens = tokenize_for_query(query);
        let candidate_multiplier = if options.kinds.is_empty()
            && matches!(options.mode, SearchMode::Auto | SearchMode::All)
        {
            4
        } else {
            8
        };
        let candidate_limit = limit
            .saturating_mul(candidate_multiplier)
            .clamp(limit, 1000);
        let mut results = if tokens.is_empty() {
            Vec::new()
        } else {
            match options.granularity {
                SearchGranularity::Document => {
                    search_index(&self.conn, &self.scope, query, &tokens, candidate_limit)?
                }
                SearchGranularity::Passage => search_span_index(
                    &self.conn,
                    &self.scope,
                    query,
                    &tokens,
                    Some("passage"),
                    candidate_limit,
                )?,
                SearchGranularity::Sentence => search_span_index(
                    &self.conn,
                    &self.scope,
                    query,
                    &tokens,
                    Some("sentence"),
                    candidate_limit,
                )?,
                SearchGranularity::All => {
                    let mut results =
                        search_index(&self.conn, &self.scope, query, &tokens, candidate_limit)?;
                    results.extend(search_span_index(
                        &self.conn,
                        &self.scope,
                        query,
                        &tokens,
                        None,
                        candidate_limit,
                    )?);
                    results
                }
            }
        };

        let normalized_kinds = options
            .kinds
            .iter()
            .map(|kind| kind.trim().to_lowercase())
            .collect::<BTreeSet<_>>();
        results.retain(|result| {
            let document_type = result
                .document
                .as_ref()
                .map(|document| document.document_type.as_str())
                .unwrap_or(result.result_type.as_str());
            let type_matches = match options.mode {
                SearchMode::Auto | SearchMode::All => true,
                SearchMode::Page => document_type == "page",
                SearchMode::Source => document_type == "source",
            };
            let kind_matches = normalized_kinds.is_empty()
                || document_type == "source"
                || result
                    .kind
                    .as_deref()
                    .map(str::to_lowercase)
                    .is_some_and(|kind| normalized_kinds.contains(&kind));
            type_matches && kind_matches
        });

        if options.mode == SearchMode::Auto {
            let summarized_sources = results
                .iter()
                .filter(|result| {
                    result.result_type == "page"
                        && result
                            .kind
                            .as_deref()
                            .is_some_and(|kind| kind.eq_ignore_ascii_case("source"))
                })
                .flat_map(|result| result.paired_source_ids.iter().copied())
                .collect::<BTreeSet<_>>();
            results.retain(|result| {
                result.result_type != "source"
                    || result
                        .identifier
                        .parse::<i64>()
                        .map(|id| !summarized_sources.contains(&id))
                        .unwrap_or(true)
            });
        }

        apply_retrieval_state(&self.conn, &tokens, &mut results)?;

        results.sort_by(|left, right| {
            search_type_priority(left, options.mode)
                .cmp(&search_type_priority(right, options.mode))
                .then_with(|| left.rank.total_cmp(&right.rank))
                .then_with(|| left.result_type.cmp(&right.result_type))
                .then_with(|| left.identifier.cmp(&right.identifier))
        });
        self.apply_graph_reranking(&mut results)?;
        if options.granularity == SearchGranularity::All {
            apply_mixed_fusion(&mut results);
        }
        if options.grouping == SearchGrouping::Document {
            results = group_search_results(results);
        }

        results.sort_by(|left, right| {
            search_type_priority(left, options.mode)
                .cmp(&search_type_priority(right, options.mode))
                .then_with(|| left.rank.total_cmp(&right.rank))
                .then_with(|| left.result_type.cmp(&right.result_type))
                .then_with(|| left.identifier.cmp(&right.identifier))
        });
        results.truncate(limit);
        load_search_provenance(&self.conn, &mut results)?;
        if !options.explain {
            for result in &mut results {
                result.explanation = None;
            }
        }

        Ok(SearchResponse { results })
    }

    pub fn span_get(&self, identifier: &str) -> Result<SpanGetResponse> {
        Ok(SpanGetResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            span: load_span_record(&self.conn, identifier)?,
        })
    }

    pub fn span_expand(
        &self,
        identifier: &str,
        before: usize,
        after: usize,
        child_limit: usize,
    ) -> Result<SpanExpandResponse> {
        let span = load_span_record(&self.conn, identifier)?;
        let parent = self
            .conn
            .query_row(
                "SELECT span_type FROM search_spans WHERE span_id = ?1 AND active = 1",
                params![&span.parent_identifier],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .filter(|node_type| matches!(node_type.as_str(), "passage" | "sentence"))
            .map(|_| load_span_record(&self.conn, &span.parent_identifier))
            .transpose()?;
        let lower = span.ordinal.saturating_sub(before);
        let upper = span.ordinal.saturating_add(after);
        let mut statement = self.conn.prepare(
            "SELECT span_id FROM search_spans
             WHERE parent_identifier = ?1 AND span_type = ?2 AND active = 1
               AND ordinal BETWEEN ?3 AND ?4
             ORDER BY ordinal, span_id",
        )?;
        let sibling_ids = statement
            .query_map(
                params![
                    &span.parent_identifier,
                    &span.span_type,
                    lower as i64,
                    upper as i64
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let siblings = sibling_ids
            .iter()
            .map(|id| load_span_record(&self.conn, id))
            .collect::<Result<Vec<_>>>()?;

        let mut child_statement = self.conn.prepare(
            "SELECT span_id FROM search_spans
             WHERE parent_identifier = ?1 AND active = 1
             ORDER BY ordinal, span_id LIMIT ?2",
        )?;
        let child_ids = child_statement
            .query_map(
                params![identifier, child_limit.saturating_add(1) as i64],
                |row| row.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let children_truncated = child_ids.len() > child_limit;
        let children = child_ids
            .iter()
            .take(child_limit)
            .map(|id| load_span_record(&self.conn, id))
            .collect::<Result<Vec<_>>>()?;
        Ok(SpanExpandResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            span,
            parent,
            siblings,
            children,
            children_truncated,
        })
    }
}
