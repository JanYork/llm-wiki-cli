fn resolve_graph_node(conn: &Connection, identifier: &str) -> Result<String> {
    let identifier = identifier.trim();
    if identifier.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "graph identifier cannot be empty",
        ));
    }
    let candidate = if identifier.contains(':') {
        identifier.to_string()
    } else {
        format!("page:{identifier}")
    };
    let exists: bool = if let Some(slug) = candidate.strip_prefix("page:") {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE slug = ?1)",
            [slug],
            |row| row.get(0),
        )?
    } else if let Some(id) = candidate.strip_prefix("source:") {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sources WHERE CAST(id AS TEXT) = ?1)",
            [id],
            |row| row.get(0),
        )?
    } else if candidate.starts_with("span:") {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM search_spans WHERE span_id = ?1 AND active = 1)",
            [&candidate],
            |row| row.get(0),
        )?
    } else {
        false
    };
    if exists {
        return Ok(candidate);
    }
    Err(AppError::new(
        "graph_node_not_found",
        format!("graph node {identifier:?} was not found"),
    ))
}

#[allow(clippy::too_many_arguments)]
fn graph_relation_set_value(
    conn: &mut Connection,
    scope: &str,
    from: &str,
    relation_type: &str,
    to: &str,
    provenance: &str,
    reason: &str,
    confidence: f64,
    source_ids: &[i64],
) -> Result<Value> {
    semantic_relation_set_value(
        conn,
        scope,
        from,
        relation_type,
        to,
        provenance,
        reason,
        confidence,
        source_ids,
    )
}

fn graph_relation_list_value(
    conn: &Connection,
    scope: &str,
    from: Option<&str>,
    to: Option<&str>,
    relation_type: Option<&str>,
    limit: usize,
) -> Result<Value> {
    semantic_relation_list_value(conn, scope, from, to, relation_type, limit)
}

fn graph_relation_retract_value(
    conn: &mut Connection,
    scope: &str,
    from: &str,
    relation_type: &str,
    to: &str,
    reason: &str,
) -> Result<Value> {
    semantic_relation_retract_value(conn, scope, from, relation_type, to, reason)
}

#[allow(clippy::too_many_arguments)]
fn semantic_relation_set_value(
    conn: &mut Connection,
    scope: &str,
    from: &str,
    relation_type: &str,
    to: &str,
    provenance: &str,
    reason: &str,
    confidence: f64,
    source_ids: &[i64],
) -> Result<Value> {
    let relation_type = relation_type.trim().to_uppercase();
    let provenance = provenance.trim().to_lowercase();
    let reason = reason.trim();
    if !matches!(
        relation_type.as_str(),
        "SUPPORTS" | "CONTRADICTS" | "REFINES" | "SUPERSEDES" | "CAUSES" | "DEPENDS_ON"
    ) {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "semantic relation type is not supported",
        ));
    }
    if !matches!(
        provenance.as_str(),
        "source-grounded" | "user-provided" | "agent-observed" | "hypothesis"
    ) {
        return Err(AppError::new(
            "invalid_provenance",
            "semantic relation provenance is not supported",
        ));
    }
    if reason.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "semantic relation reason cannot be empty",
        ));
    }
    if !(0.0..=1.0).contains(&confidence) || !confidence.is_finite() {
        return Err(AppError::new(
            "invalid_confidence",
            "confidence must be a finite value between 0 and 1",
        ));
    }
    let source_ids = dedupe_i64(source_ids.to_vec());
    if provenance == "source-grounded" && source_ids.is_empty() {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "source-grounded semantic relations require at least one --source",
        ));
    }
    let from = resolve_graph_node(conn, from)?;
    let to = resolve_graph_node(conn, to)?;
    if from == to {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "semantic relation endpoints must be different",
        ));
    }
    let id = format!(
        "edge:{}",
        hash_content(&format!("manual\0{relation_type}\0{from}\0{to}"))
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_sources(&tx, &source_ids)?;
    tx.execute(
        &format!(
            "INSERT INTO semantic_relations(
                id, relation_type, from_identifier, to_identifier,
                confidence, provenance, reason, source_ids_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, {TIMESTAMP_SQL}, {TIMESTAMP_SQL})
             ON CONFLICT(id) DO UPDATE SET
                confidence = excluded.confidence,
                provenance = excluded.provenance,
                reason = excluded.reason,
                source_ids_json = excluded.source_ids_json,
                updated_at = excluded.updated_at"
        ),
        params![
            &id,
            &relation_type,
            &from,
            &to,
            confidence,
            &provenance,
            reason,
            json!(source_ids).to_string(),
        ],
    )?;
    let relation = json!({
        "identifier": id,
        "type": relation_type,
        "from": from,
        "to": to,
        "confidence": confidence,
        "provenance": provenance,
        "reason": reason,
        "source_ids": source_ids,
    });
    record_operation(&tx, "graph_relation_set", &id, &relation)?;
    tx.commit()?;
    Ok(json!({"scope": scope, "relation": relation}))
}

fn semantic_relation_list_value(
    conn: &Connection,
    scope: &str,
    from: Option<&str>,
    to: Option<&str>,
    relation_type: Option<&str>,
    limit: usize,
) -> Result<Value> {
    let from = from
        .map(|value| resolve_graph_node(conn, value))
        .transpose()?;
    let to = to
        .map(|value| resolve_graph_node(conn, value))
        .transpose()?;
    let relation_type = relation_type.map(|value| value.trim().to_uppercase());
    let mut statement = conn.prepare(
        "SELECT id, relation_type, from_identifier, to_identifier,
                confidence, provenance, reason, source_ids_json
         FROM semantic_relations
         WHERE (?1 IS NULL OR from_identifier = ?1)
           AND (?2 IS NULL OR to_identifier = ?2)
           AND (?3 IS NULL OR relation_type = ?3)
         ORDER BY relation_type, from_identifier, to_identifier, id LIMIT ?4",
    )?;
    let relations = statement
        .query_map(params![from, to, relation_type, limit as i64], |row| {
            let source_ids = row.get::<_, String>(7)?;
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "from": row.get::<_, String>(2)?,
                "to": row.get::<_, String>(3)?,
                "confidence": row.get::<_, Option<f64>>(4)?,
                "provenance": row.get::<_, String>(5)?,
                "reason": row.get::<_, Option<String>>(6)?,
                "source_ids": serde_json::from_str::<Value>(&source_ids).unwrap_or(json!([])),
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"scope": scope, "relations": relations, "limit": limit}))
}

fn semantic_relation_retract_value(
    conn: &mut Connection,
    scope: &str,
    from: &str,
    relation_type: &str,
    to: &str,
    reason: &str,
) -> Result<Value> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "semantic relation retraction reason cannot be empty",
        ));
    }
    let relation_type = relation_type.trim().to_uppercase();
    let from = resolve_graph_node(conn, from)?;
    let to = resolve_graph_node(conn, to)?;
    let id = format!(
        "edge:{}",
        hash_content(&format!("manual\0{relation_type}\0{from}\0{to}"))
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if tx.execute("DELETE FROM semantic_relations WHERE id = ?1", [&id])? != 1 {
        return Err(AppError::new(
            "semantic_relation_not_found",
            "explicit semantic relation was not found",
        ));
    }
    record_operation(
        &tx,
        "graph_relation_retract",
        &id,
        &json!({"reason": reason}),
    )?;
    tx.commit()?;
    Ok(json!({
        "scope": scope,
        "identifier": id,
        "retracted": true,
        "reason": reason,
    }))
}

fn load_span_record(conn: &Connection, identifier: &str) -> Result<SpanRecord> {
    load_search_span_record(conn, identifier)
}

fn load_search_span_record(conn: &Connection, identifier: &str) -> Result<SpanRecord> {
    let row = conn
        .query_row(
            "SELECT n.span_id, n.span_type, n.document_type, n.document_identifier,
                    n.parent_identifier, n.ordinal, n.byte_start, n.byte_end,
                    n.content_fingerprint, n.segmenter_version, n.active,
                    CASE n.document_type WHEN 'page' THEN p.body ELSE s.content END
             FROM search_spans n
             LEFT JOIN pages p
               ON n.document_type = 'page' AND p.slug = n.document_identifier
             LEFT JOIN sources s
               ON n.document_type = 'source'
              AND s.id = CAST(n.document_identifier AS INTEGER)
             WHERE n.span_id = ?1",
            [identifier],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, bool>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::new("span_not_found", "span locator was not found"))?;
    let (
        identifier,
        span_type,
        document_type,
        document_identifier,
        parent_identifier,
        ordinal,
        byte_start,
        byte_end,
        content_fingerprint,
        segmenter_version,
        active,
        content,
    ) = row;
    let current_fingerprint = content.as_deref().map(hash_content);
    if !active || current_fingerprint.as_deref() != Some(&content_fingerprint) {
        return Err(AppError::new(
            "stale_span",
            "span locator belongs to an older document fingerprint",
        )
        .with_details(json!({
            "identifier": identifier,
            "document": {"type": document_type, "identifier": document_identifier},
            "prior": {"content_fingerprint": content_fingerprint, "segmenter_version": segmenter_version},
            "current": {
                "content_fingerprint": current_fingerprint,
                "segmenter_version": crate::segment::SEGMENTER_VERSION,
            },
        })));
    }
    let content = content.unwrap_or_default();
    let byte_start = usize::try_from(byte_start)
        .map_err(|_| AppError::new("invalid_span", "span byte_start is invalid"))?;
    let byte_end = usize::try_from(byte_end)
        .map_err(|_| AppError::new("invalid_span", "span byte_end is invalid"))?;
    let text = content.get(byte_start..byte_end).ok_or_else(|| {
        AppError::new(
            "stale_span",
            "span byte range no longer matches the indexed document",
        )
    })?;
    Ok(SpanRecord {
        identifier,
        span_type,
        document: SearchDocumentRef {
            document_type,
            identifier: document_identifier,
        },
        parent_identifier,
        ordinal: usize::try_from(ordinal).unwrap_or_default(),
        byte_start,
        byte_end,
        content_fingerprint,
        segmenter_version: u32::try_from(segmenter_version).unwrap_or_default(),
        text: text.to_string(),
    })
}

fn search_span_index(
    conn: &Connection,
    scope: &str,
    raw_query: &str,
    tokens: &[String],
    span_type: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    if tokens.len() > 64 {
        return Err(AppError::new(
            "invalid_query",
            "query contains more than 64 searchable terms",
        ));
    }
    let match_query = tokens
        .iter()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = conn.prepare(
        "SELECT
            f.span_id, f.span_type, f.document_type, f.document_identifier,
            CASE f.document_type WHEN 'page' THEN p.title ELSE s.title END,
            CASE f.document_type WHEN 'page' THEN p.kind ELSE NULL END,
            CASE f.document_type WHEN 'page' THEN p.summary ELSE NULL END,
            CASE f.document_type WHEN 'page' THEN p.slug ELSE s.origin END,
            '', n.parent_identifier, n.ordinal, n.byte_start, n.byte_end,
            n.content_fingerprint, n.segmenter_version,
            bm25(span_fts, 0.0, 0.0, 0.0, 0.0, 4.0, 2.0, 1.0, 1.0),
            CASE f.document_type
                WHEN 'page' THEN p.structural_navigation
                ELSE s.structural_navigation
            END,
            CASE f.document_type WHEN 'page' THEN p.body ELSE s.content END
         FROM span_fts f
         JOIN search_spans n ON n.span_id = f.span_id AND n.active = 1
         LEFT JOIN pages p
           ON f.document_type = 'page' AND p.slug = f.document_identifier
         LEFT JOIN sources s
           ON f.document_type = 'source'
          AND s.id = CAST(f.document_identifier AS INTEGER)
         WHERE span_fts MATCH ?1
           AND (?2 IS NULL OR f.span_type = ?2)
         ORDER BY 16 ASC, f.rowid ASC
         LIMIT ?3",
    )?;
    statement
        .query_map(params![match_query, span_type, limit as i64], |row| {
            let document_type = row.get::<_, String>(2)?;
            let title = row.get::<_, Option<String>>(4)?;
            let path = row.get::<_, String>(7)?;
            let base_rank = row.get::<_, f64>(15)?;
            let byte_start = row.get::<_, i64>(11)? as usize;
            let byte_end = row.get::<_, i64>(12)? as usize;
            let body = row.get::<_, String>(17)?;
            let text = body.get(byte_start..byte_end).unwrap_or("").to_string();
            let explanation = lexical_explanation(
                &document_type,
                title.as_deref(),
                &path,
                raw_query,
                tokens,
                base_rank,
                row.get::<_, i64>(16)? != 0,
            );
            Ok(SearchResult {
                scope: scope.to_string(),
                result_type: row.get(1)?,
                identifier: row.get(0)?,
                document: Some(SearchDocumentRef {
                    document_type,
                    identifier: row.get(3)?,
                }),
                span: Some(SearchSpanRef {
                    parent_identifier: row.get(9)?,
                    ordinal: row.get::<_, i64>(10)? as usize,
                    byte_start,
                    byte_end,
                    content_fingerprint: row.get(13)?,
                    segmenter_version: row.get::<_, i64>(14)? as u32,
                }),
                fused_score: None,
                matches: None,
                title,
                kind: row.get(5)?,
                summary: row.get(6)?,
                provenance: None,
                snippet: text,
                rank: explanation.final_rank,
                explanation: Some(explanation),
                paired_source_ids: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn apply_mixed_fusion(results: &mut [SearchResult]) {
    let mut by_granularity: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, result) in results.iter().enumerate() {
        by_granularity
            .entry(result.result_type.clone())
            .or_default()
            .push(index);
    }
    for indices in by_granularity.values_mut() {
        indices.sort_by(|left, right| {
            results[*left]
                .rank
                .total_cmp(&results[*right].rank)
                .then_with(|| results[*left].identifier.cmp(&results[*right].identifier))
        });
        for (position, index) in indices.iter().copied().enumerate() {
            let prior = match results[index].result_type.as_str() {
                "sentence" => 1.15,
                "passage" => 1.05,
                _ => 1.0,
            };
            let score = prior / (60.0 + (position + 1) as f64);
            results[index].fused_score = Some(score);
            results[index].rank = -score;
        }
    }
}

fn group_search_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut groups: BTreeMap<(String, String), Vec<SearchResult>> = BTreeMap::new();
    for result in results {
        let key = result
            .document
            .as_ref()
            .map(|document| (document.document_type.clone(), document.identifier.clone()))
            .unwrap_or_else(|| (result.result_type.clone(), result.identifier.clone()));
        groups.entry(key).or_default().push(result);
    }

    groups
        .into_iter()
        .map(|((document_type, document_identifier), mut matches)| {
            matches.sort_by(|left, right| {
                left.rank
                    .total_cmp(&right.rank)
                    .then_with(|| left.result_type.cmp(&right.result_type))
                    .then_with(|| left.identifier.cmp(&right.identifier))
            });
            let best = matches[0].clone();
            let mut grouped = matches
                .iter()
                .find(|result| {
                    result.result_type == document_type && result.identifier == document_identifier
                })
                .cloned()
                .unwrap_or_else(|| best.clone());
            let selected = matches
                .iter()
                .take(3)
                .map(|result| SearchMatch {
                    result_type: result.result_type.clone(),
                    identifier: result.identifier.clone(),
                    snippet: result.snippet.clone(),
                    rank: result.rank,
                    fused_score: result.fused_score,
                    span: result.span.clone(),
                })
                .collect::<Vec<_>>();
            let group_score = selected
                .iter()
                .enumerate()
                .map(|(index, result)| {
                    result.fused_score.unwrap_or(-result.rank)
                        * match index {
                            0 => 1.0,
                            1 => 0.5,
                            _ => 0.25,
                        }
                })
                .sum::<f64>();
            grouped.result_type = document_type;
            grouped.identifier = document_identifier;
            grouped.document = None;
            grouped.span = None;
            grouped.fused_score = Some(group_score);
            grouped.matches = Some(selected);
            grouped.snippet = best.snippet;
            grouped.rank = -group_score;
            grouped.explanation = best.explanation;
            grouped
        })
        .collect()
}

fn apply_retrieval_state(
    conn: &Connection,
    query_tokens: &[String],
    results: &mut [SearchResult],
) -> Result<()> {
    let weights = load_effective_weights(conn)?;
    let feedback = load_effective_feedback(conn, &query_fingerprint(query_tokens))?;
    for result in results {
        let key = result
            .document
            .as_ref()
            .map(|document| (document.document_type.clone(), document.identifier.clone()))
            .unwrap_or_else(|| (result.result_type.clone(), result.identifier.clone()));
        let Some(explanation) = result.explanation.as_mut() else {
            continue;
        };
        explanation.signals.manual_adjustment =
            weights.get(&key).map_or(0.0, |weight| *weight as f64 / 2.0);
        explanation.signals.feedback_adjustment =
            feedback.get(&key).copied().unwrap_or_default() as f64;
        explanation.contributions.manual = -MANUAL_WEIGHT * explanation.signals.manual_adjustment;
        explanation.contributions.feedback =
            -FEEDBACK_WEIGHT * explanation.signals.feedback_adjustment;
        explanation.final_rank = explanation.base_rank + explanation.contributions.total();
        result.rank = explanation.final_rank;
    }
    Ok(())
}

fn load_effective_weights(conn: &Connection) -> Result<BTreeMap<(String, String), i32>> {
    let mut statement = conn.prepare(
        "SELECT target_type, target_identifier, weight
         FROM retrieval_weights
         ORDER BY target_type, target_identifier,
                  CASE provenance WHEN 'user-provided' THEN 0 ELSE 1 END",
    )?;
    let mut effective = BTreeMap::new();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)?,
        ))
    })? {
        let (target_type, identifier, weight) = row?;
        effective.entry((target_type, identifier)).or_insert(weight);
    }
    Ok(effective)
}

fn load_effective_feedback(
    conn: &Connection,
    fingerprint: &str,
) -> Result<BTreeMap<(String, String), i32>> {
    let mut statement = conn.prepare(
        "SELECT target_type, target_identifier, signal
         FROM retrieval_feedback
         WHERE query_fingerprint = ?1
         ORDER BY target_type, target_identifier,
                  CASE provenance WHEN 'user-provided' THEN 0 ELSE 1 END",
    )?;
    let mut effective = BTreeMap::new();
    for row in statement.query_map(params![fingerprint], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)?,
        ))
    })? {
        let (target_type, identifier, signal) = row?;
        effective.entry((target_type, identifier)).or_insert(signal);
    }
    Ok(effective)
}

fn query_fingerprint(tokens: &[String]) -> String {
    let mut hasher = Sha256::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            hasher.update([0x1f]);
        }
        hasher.update(token.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn load_search_provenance(conn: &Connection, results: &mut [SearchResult]) -> Result<()> {
    let slugs = results
        .iter()
        .filter(|result| result.result_type == "page")
        .map(|result| result.identifier.clone())
        .collect::<BTreeSet<_>>();
    if slugs.is_empty() {
        return Ok(());
    }

    let placeholders = std::iter::repeat_n("?", slugs.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT p.slug,
                EXISTS(SELECT 1 FROM page_sources ps WHERE ps.page_slug = p.slug),
                (SELECT GROUP_CONCAT(pp.provenance, ',')
                 FROM page_provenance pp WHERE pp.page_slug = p.slug)
         FROM pages p
         WHERE p.slug IN ({placeholders})"
    );
    let mut statement = conn.prepare(&sql)?;
    let provenance = statement
        .query_map(rusqlite::params_from_iter(slugs.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                provenance_from_parts(row.get::<_, i64>(1)? != 0, row.get(2)?),
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;

    for result in results
        .iter_mut()
        .filter(|result| result.result_type == "page")
    {
        result.provenance = Some(
            provenance
                .get(&result.identifier)
                .cloned()
                .unwrap_or_default(),
        );
    }
    Ok(())
}

fn search_type_priority(result: &SearchResult, mode: SearchMode) -> u8 {
    if matches!(mode, SearchMode::Auto | SearchMode::All) && result.result_type == "source" {
        1
    } else {
        0
    }
}

fn lexical_explanation(
    result_type: &str,
    title: Option<&str>,
    path: &str,
    query: &str,
    tokens: &[String],
    base_rank: f64,
    structural_navigation: bool,
) -> SearchExplanation {
    let title_match = if result_type == "source" {
        field_match(title.unwrap_or(""), query, tokens).max(field_match(
            source_leaf(path),
            query,
            tokens,
        ))
    } else {
        field_match(title.unwrap_or(""), query, tokens)
    };
    let path_field = if result_type == "source" {
        source_parent(path)
    } else {
        path
    };
    let path_match = field_match(path_field, query, tokens);
    let generic_marker = generic_marker(title.unwrap_or(""), path, query, structural_navigation);
    let signals = SearchSignals {
        title_match,
        path_match,
        generic_marker,
        ..SearchSignals::default()
    };
    let contributions = SearchContributions {
        title: -TITLE_WEIGHT * title_match,
        path: -PATH_WEIGHT * path_match,
        generic: GENERIC_WEIGHT * generic_marker,
        ..SearchContributions::default()
    };
    SearchExplanation {
        base_rank,
        final_rank: base_rank + contributions.total(),
        signals,
        contributions,
        graph_seeds: Vec::new(),
    }
}

fn field_match(field: &str, query: &str, tokens: &[String]) -> f64 {
    if field.trim().is_empty() || tokens.is_empty() {
        return 0.0;
    }
    let normalized_field = normalized_text(field);
    let normalized_query = normalized_text(query);
    if !normalized_query.is_empty() && normalized_field == normalized_query {
        return 1.0;
    }
    if tokens.len() > 1
        && !normalized_query.is_empty()
        && normalized_field.contains(&normalized_query)
    {
        return 0.9;
    }
    let field_terms = tokenize_for_query(field)
        .into_iter()
        .collect::<BTreeSet<_>>();
    tokens
        .iter()
        .filter(|term| field_terms.contains(*term))
        .count() as f64
        / tokens.len() as f64
}

fn normalized_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut separated = true;
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric()
            || matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{323af}')
        {
            normalized.push(ch);
            separated = false;
        } else if !separated {
            normalized.push(' ');
            separated = true;
        }
    }
    normalized.trim().to_string()
}

fn generic_marker(title: &str, path: &str, query: &str, structural_navigation: bool) -> f64 {
    const MARKERS: [&str; 10] = [
        "readme", "index", "summary", "toc", "overview", "导航", "总览", "索引", "目录", "归档",
    ];
    let candidate = format!("{} {}", normalized_text(title), normalized_text(path));
    let normalized_query = normalized_text(query);
    if (structural_navigation || MARKERS.iter().any(|marker| candidate.contains(marker)))
        && !MARKERS
            .iter()
            .any(|marker| normalized_query.contains(marker))
    {
        1.0
    } else {
        0.0
    }
}
