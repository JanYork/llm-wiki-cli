impl Store {
    pub fn materialize(&mut self) -> Result<MaterializeResponse> {
        self.materialize_inner(true)
    }

    pub fn materialize_wiki(&mut self) -> Result<MaterializeResponse> {
        self.materialize_inner(false)
    }

    fn materialize_inner(&mut self, include_raw_sources: bool) -> Result<MaterializeResponse> {
        let root = self
            .database
            .parent()
            .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?
            .to_path_buf();
        let _projection_lock = artifacts::lock_projection(&root)
            .map_err(|error| AppError::new("artifact_busy", error.to_string()))?;
        let snapshot = artifact_snapshot(&self.conn, include_raw_sources)?;
        let materialize = if include_raw_sources {
            artifacts::materialize_snapshot
        } else {
            artifacts::materialize_wiki_snapshot
        };
        let files = materialize(&root, &snapshot)
            .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        let cursor: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(id), 0) FROM operations", [], |row| {
                    row.get(0)
                })?;
        artifacts::save_cursor(&root, cursor)
            .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        Ok(MaterializeResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            files,
        })
    }

    pub fn materialize_incremental(&self, include_raw_sources: bool) -> Result<Vec<String>> {
        let root = self
            .database
            .parent()
            .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?;
        let _projection_lock = artifacts::lock_projection(root)
            .map_err(|error| AppError::new("artifact_busy", error.to_string()))?;
        let cursor = artifacts::load_cursor(root)
            .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        let operations = {
            let mut statement = self.conn.prepare(
                "SELECT id, created_at, action, target, detail_json
                 FROM operations WHERE id > ?1 ORDER BY id",
            )?;
            statement
                .query_map(params![cursor], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        artifacts::Operation {
                            created_at: row.get(1)?,
                            action: row.get(2)?,
                            target: row.get(3)?,
                            detail: row.get(4)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if operations.is_empty() {
            return Ok(Vec::new());
        }
        let mut written = Vec::new();
        for (_, operation) in &operations {
            match operation.action.as_str() {
                "page_put" => {
                    if let Some(page) = self.load_artifact_page(&operation.target)? {
                        written.extend(artifacts::materialize_page(root, &page).map_err(
                            |error| AppError::new("artifact_write_failed", error.to_string()),
                        )?);
                    }
                }
                "page_remove" => {
                    written.extend(artifacts::remove_page(root, &operation.target).map_err(
                        |error| AppError::new("artifact_write_failed", error.to_string()),
                    )?);
                }
                "source_add" if include_raw_sources => {
                    let detail = operation
                        .detail
                        .as_deref()
                        .and_then(|value| serde_json::from_str::<Value>(value).ok());
                    if let Some(source_id) = detail
                        .as_ref()
                        .and_then(|value| value.get("source_id"))
                        .and_then(Value::as_i64)
                        && let Some(source) = self.load_artifact_source(source_id)?
                    {
                        written.extend(artifacts::materialize_source(root, &source).map_err(
                            |error| AppError::new("artifact_write_failed", error.to_string()),
                        )?);
                    }
                }
                "source_remove" if include_raw_sources => {
                    written.extend(artifacts::remove_source(root, &operation.target).map_err(
                        |error| AppError::new("artifact_write_failed", error.to_string()),
                    )?);
                }
                "schema_set" => {
                    let content = self
                        .schema_text()?
                        .unwrap_or_else(|| DEFAULT_SCHEMA.to_string());
                    artifacts::materialize_text(root, "schema.md", &content).map_err(|error| {
                        AppError::new("artifact_write_failed", error.to_string())
                    })?;
                    written.push("schema.md".into());
                }
                "purpose_set" => {
                    let content = self
                        .purpose_text()?
                        .unwrap_or_else(|| DEFAULT_PURPOSE.to_string());
                    artifacts::materialize_text(root, "purpose.md", &content).map_err(|error| {
                        AppError::new("artifact_write_failed", error.to_string())
                    })?;
                    written.push("purpose.md".into());
                }
                _ => {}
            }
        }
        artifacts::append_operations(
            root,
            &operations
                .iter()
                .map(|(_, operation)| operation.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        written.push("wiki/log.md".into());
        let cursor = operations.last().map(|(id, _)| *id).unwrap_or(cursor);
        artifacts::save_cursor(root, cursor)
            .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        written.sort();
        written.dedup();
        Ok(written)
    }

    fn load_artifact_page(&self, slug: &str) -> Result<Option<artifacts::Page>> {
        let page = self
            .conn
            .query_row(
                "SELECT slug, title, kind, summary, body, created_at, updated_at
                 FROM pages WHERE slug = ?1",
                params![slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((slug, title, kind, summary, body, created, updated)) = page else {
            return Ok(None);
        };
        let source_artifact_paths = {
            let mut statement = self.conn.prepare(
                "SELECT s.id, s.origin FROM page_sources ps
                 JOIN sources s ON s.id = ps.source_id
                 WHERE ps.page_slug = ?1 ORDER BY s.id",
            )?;
            statement
                .query_map(params![&slug], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .map(|row| {
                    let (id, origin) = row?;
                    artifacts::source_artifact_rel_path(&id.to_string(), &origin)
                        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
                })
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(Some(artifacts::Page {
            slug: slug.clone(),
            title,
            kind,
            summary,
            body,
            source_artifact_paths,
            provenance: self
                .load_page_provenance(&slug, !self.load_page_source_ids(&slug)?.is_empty())?,
            created,
            updated,
        }))
    }

    fn load_artifact_source(&self, id: i64) -> Result<Option<artifacts::Source>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, title, origin, content FROM sources WHERE id = ?1",
                params![id],
                |row| {
                    Ok(artifacts::Source {
                        id: row.get::<_, i64>(0)?.to_string(),
                        title: row.get(1)?,
                        origin: row.get(2)?,
                        content: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn context_store(&self, limit: usize) -> Result<ContextStore> {
        Ok(ContextStore {
            scope: self.scope.clone(),
            database: self.database_string(),
            schema: self.schema_text()?,
            purpose: self.purpose_text()?,
            pages: self.load_page_summaries(limit, 0)?,
            recent_operations: self.load_operations(limit)?,
        })
    }

    pub fn lint(&self, limit: usize, offset: usize) -> Result<LintResponse> {
        let scope = self.scope.clone();
        let database = self.database_string();
        let mut statement = self.conn.prepare(&format!(
            "{LINT_ISSUES_SQL}
             SELECT code, page, target, message
             FROM issues ORDER BY code, page, target"
        ))?;
        let mut all_issues = statement
            .query_map([], |row| {
                Ok(LintIssue {
                    code: row.get(0)?,
                    page: row.get(1)?,
                    target: row.get(2)?,
                    message: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        all_issues.sort_by(|left, right| {
            left.code
                .cmp(&right.code)
                .then_with(|| left.page.cmp(&right.page))
                .then_with(|| left.target.cmp(&right.target))
        });
        let mut counts = BTreeMap::new();
        for issue in &all_issues {
            *counts.entry(issue.code.clone()).or_insert(0usize) += 1;
        }
        let total = all_issues.len();
        let issues = all_issues
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let has_more = offset.saturating_add(issues.len()) < total;
        Ok(LintResponse {
            scope,
            database,
            issues,
            counts,
            total,
            limit,
            offset,
            has_more,
        })
    }

    pub fn record_lint(&mut self, issues: usize) -> Result<()> {
        self.record_top_level_operation("lint", "wiki", json!({ "issues": issues }))
    }

    pub fn log(&self, limit: usize) -> Result<LogResponse> {
        Ok(LogResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            operations: self.load_operations(limit)?,
        })
    }

    fn schema_text(&self) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(Into::into)
    }

    fn purpose_text(&self) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = 'purpose'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(Into::into)
    }

    fn load_ingest_work(
        &self,
        source_id: i64,
        source_max_chars: Option<usize>,
    ) -> Result<IngestWork> {
        let (status, attempts, analysis) = self
            .conn
            .query_row(
                "SELECT status, attempts, analysis
                 FROM ingest_jobs
                 WHERE source_id = ?1",
                params![source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new(
                    "ingest_job_not_found",
                    format!("ingest job not found for source {source_id}"),
                )
            })?;
        let (source, source_window) =
            window_source(self.load_source(source_id)?, 0, source_max_chars)?;
        Ok(IngestWork {
            source,
            source_window,
            status,
            attempts,
            analysis,
        })
    }

    fn ingest_packet(
        &self,
        source_id: Option<i64>,
        context_limit: usize,
        source_max_chars: Option<usize>,
    ) -> Result<IngestPacket> {
        Ok(IngestPacket {
            scope: self.scope.clone(),
            database: self.database_string(),
            job: source_id
                .map(|source_id| self.load_ingest_work(source_id, source_max_chars))
                .transpose()?,
            schema: self.schema_text()?,
            purpose: self.purpose_text()?,
            pages: self.load_page_summaries(context_limit, 0)?,
        })
    }

    fn ingest_mutation_response(&self, source_id: i64) -> Result<IngestMutationResponse> {
        let job = self
            .conn
            .query_row(
                "SELECT source_id, status, attempts, last_error,
                        no_derived_pages_reason, updated_at
                 FROM ingest_jobs
                 WHERE source_id = ?1",
                params![source_id],
                read_ingest_job_summary,
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new(
                    "ingest_job_not_found",
                    format!("ingest job not found for source {source_id}"),
                )
            })?;
        Ok(IngestMutationResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            job,
            integration: None,
        })
    }

    fn load_retrieval_adjustment(
        &self,
        target_type: &str,
        identifier: &str,
        provenance: &str,
    ) -> Result<RetrievalAdjustment> {
        self.conn
            .query_row(
                "SELECT target_type, target_identifier, provenance, weight, reason, updated_at
                 FROM retrieval_weights
                 WHERE target_type = ?1 AND target_identifier = ?2 AND provenance = ?3",
                params![target_type, identifier, provenance],
                read_retrieval_adjustment,
            )
            .map_err(Into::into)
    }

    fn load_retrieval_feedback(
        &self,
        fingerprint: &str,
        target_type: &str,
        identifier: &str,
        provenance: &str,
    ) -> Result<RetrievalFeedbackResponse> {
        let (signal, reason, updated_at) = self.conn.query_row(
            "SELECT signal, reason, updated_at
             FROM retrieval_feedback
             WHERE query_fingerprint = ?1
               AND target_type = ?2
               AND target_identifier = ?3
               AND provenance = ?4",
            params![fingerprint, target_type, identifier, provenance],
            |row| {
                Ok((
                    row.get::<_, i32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        Ok(RetrievalFeedbackResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            query_fingerprint: fingerprint.to_string(),
            target_type: target_type.to_string(),
            target_identifier: identifier.to_string(),
            provenance: provenance.to_string(),
            signal: if signal > 0 { "relevant" } else { "irrelevant" }.to_string(),
            reason,
            updated_at,
        })
    }

    fn load_graph_pages(&self) -> Result<Vec<GraphPage>> {
        let mut source_ids: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT page_slug, source_id
                 FROM page_sources
                 ORDER BY page_slug, source_id",
            )?;
            for row in statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })? {
                let (slug, source_id) = row?;
                source_ids.entry(slug).or_default().push(source_id);
            }
        }
        let mut outlinks: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let mut statement = self
                .conn
                .prepare("SELECT from_slug, to_slug FROM links ORDER BY from_slug, to_slug")?;
            for row in statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })? {
                let (slug, target) = row?;
                outlinks.entry(slug).or_default().push(target);
            }
        }
        let mut statement = self
            .conn
            .prepare("SELECT slug, title, kind FROM pages ORDER BY slug")?;
        statement
            .query_map([], |row| {
                let slug = row.get::<_, String>(0)?;
                Ok(GraphPage {
                    source_ids: source_ids.remove(&slug).unwrap_or_default(),
                    outlinks: outlinks.remove(&slug).unwrap_or_default(),
                    slug,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn load_source(&self, id: i64) -> Result<SourceRecord> {
        self.conn
            .query_row(
                "SELECT id, title, origin, content_hash, content, created_at
                 FROM sources
                 WHERE id = ?1",
                params![id],
                read_source_record,
            )
            .optional()?
            .ok_or_else(|| AppError::new("source_not_found", format!("source not found: {id}")))
    }

    fn load_source_summary(&self, id: i64) -> Result<SourceSummary> {
        self.conn
            .query_row(
                "SELECT id, title, origin, content_hash,
                        LENGTH(CAST(content AS BLOB)), created_at
                 FROM sources
                 WHERE id = ?1",
                params![id],
                read_source_summary,
            )
            .optional()?
            .ok_or_else(|| AppError::new("source_not_found", format!("source not found: {id}")))
    }

    fn load_page(&self, slug: &str) -> Result<PageRecord> {
        let base = self
            .conn
            .query_row(
                "SELECT slug, title, kind, summary, body, created_at, updated_at
                 FROM pages
                 WHERE slug = ?1",
                params![slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::new("page_not_found", format!("page not found: {slug}")))?;

        let source_ids = self.load_page_source_ids(slug)?;
        let provenance = self.load_page_provenance(slug, !source_ids.is_empty())?;
        let links = self.load_page_links(slug)?;
        Ok(PageRecord {
            slug: base.0,
            title: base.1,
            kind: base.2,
            summary: base.3,
            body: base.4,
            source_ids,
            provenance,
            links,
            created_at: base.5,
            updated_at: base.6,
        })
    }

    fn load_page_write(&self, slug: &str) -> Result<PageWriteRecord> {
        let base = self
            .conn
            .query_row(
                "SELECT slug, title, kind, summary, created_at, updated_at
                 FROM pages
                 WHERE slug = ?1",
                params![slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::new("page_not_found", format!("page not found: {slug}")))?;
        let source_ids = self.load_page_source_ids(slug)?;
        let provenance = self.load_page_provenance(slug, !source_ids.is_empty())?;
        Ok(PageWriteRecord {
            slug: base.0,
            title: base.1,
            kind: base.2,
            summary: base.3,
            source_ids,
            provenance,
            links: self.load_page_links(slug)?,
            created_at: base.4,
            updated_at: base.5,
        })
    }

    fn load_page_summaries(&self, limit: usize, offset: usize) -> Result<Vec<PageSummary>> {
        let mut statement = self.conn.prepare(
            "SELECT p.slug, p.title, p.kind, p.summary, p.updated_at,
                    EXISTS(
                        SELECT 1 FROM page_sources ps WHERE ps.page_slug = p.slug
                    ),
                    (
                        SELECT GROUP_CONCAT(pp.provenance, ',')
                        FROM page_provenance pp
                        WHERE pp.page_slug = p.slug
                    )
             FROM pages p
             ORDER BY p.slug ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        statement
            .query_map(params![limit as i64, offset as i64], |row| {
                Ok(PageSummary {
                    slug: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    updated_at: row.get(4)?,
                    provenance: provenance_from_parts(row.get::<_, i64>(5)? != 0, row.get(6)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(AppError::from)
    }

    fn load_page_source_ids(&self, slug: &str) -> Result<Vec<i64>> {
        let mut statement = self.conn.prepare(
            "SELECT source_id
             FROM page_sources
             WHERE page_slug = ?1
             ORDER BY source_id ASC",
        )?;
        statement
            .query_map(params![slug], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn load_page_provenance(&self, slug: &str, has_sources: bool) -> Result<Vec<String>> {
        let explicit = self.conn.query_row(
            "SELECT GROUP_CONCAT(provenance, ',')
             FROM page_provenance
             WHERE page_slug = ?1",
            params![slug],
            |row| row.get::<_, Option<String>>(0),
        )?;
        Ok(provenance_from_parts(has_sources, explicit))
    }

    fn load_page_links(&self, slug: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT to_slug
             FROM links
             WHERE from_slug = ?1
             ORDER BY to_slug ASC",
        )?;
        statement
            .query_map(params![slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn load_operations(&self, limit: usize) -> Result<Vec<OperationRecord>> {
        let mut statement = self.conn.prepare(
            "SELECT id, action, target, detail_json, created_at
             FROM operations
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        statement
            .query_map(params![limit as i64], read_operation_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn record_top_level_operation(
        &mut self,
        action: &str,
        target: &str,
        detail: Value,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        record_operation(&tx, action, target, &detail)?;
        tx.commit()?;
        Ok(())
    }

    fn database_string(&self) -> String {
        self.database.to_string_lossy().into_owned()
    }
}
