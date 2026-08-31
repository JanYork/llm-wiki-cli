#[allow(clippy::too_many_arguments)]
fn page_content_fingerprint(
    title: &str,
    kind: Option<&str>,
    summary: Option<&str>,
    body: &str,
    structural_navigation: bool,
    source_ids: &[i64],
    provenance: &[String],
    links: &[String],
) -> String {
    let provenance = provenance.iter().collect::<BTreeSet<_>>();
    hash_content(
        &json!({
            "title": title,
            "kind": kind,
            "summary": summary,
            "body": body,
            "structural_navigation": structural_navigation,
            "source_ids": source_ids,
            "provenance": provenance,
            "links": links,
        })
        .to_string(),
    )
}

fn validate_page_slug(slug: &str) -> Result<()> {
    if slug.trim().is_empty()
        || slug != slug.trim()
        || slug == "."
        || slug == ".."
        || slug.contains(['/', '\\'])
        || slug.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "invalid_slug",
            "slug must be one safe filename segment",
        ));
    }
    Ok(())
}

fn validate_sources(tx: &Transaction<'_>, source_ids: &[i64]) -> Result<()> {
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
    }
    Ok(())
}

fn dedupe_i64(values: Vec<i64>) -> Vec<i64> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_explicit_provenance(values: Vec<String>) -> Result<Vec<String>> {
    let values = values.into_iter().collect::<BTreeSet<_>>();
    if let Some(value) = values
        .iter()
        .find(|value| !EXPLICIT_PROVENANCE.contains(&value.as_str()))
    {
        return Err(AppError::new(
            "invalid_provenance",
            format!(
                "unsupported provenance {value:?}; use user-provided, agent-observed, or hypothesis"
            ),
        ));
    }
    Ok(EXPLICIT_PROVENANCE
        .iter()
        .filter(|value| values.contains(**value))
        .map(|value| (*value).to_string())
        .collect())
}

fn provenance_from_parts(has_sources: bool, explicit: Option<String>) -> Vec<String> {
    let explicit = explicit
        .as_deref()
        .unwrap_or("")
        .split(',')
        .collect::<BTreeSet<_>>();
    let mut provenance = Vec::with_capacity(EXPLICIT_PROVENANCE.len() + usize::from(has_sources));
    if has_sources {
        provenance.push(SOURCE_GROUNDED.to_string());
    }
    provenance.extend(
        EXPLICIT_PROVENANCE
            .iter()
            .filter(|value| explicit.contains(**value))
            .map(|value| (*value).to_string()),
    );
    provenance
}

fn extract_links(body: &str) -> Vec<String> {
    let mut links = BTreeSet::new();
    let mut code_ranges = Vec::new();
    let mut code_depth = 0usize;
    for (event, range) in Parser::new_ext(body, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                code_depth += 1;
                code_ranges.push(range);
            }
            Event::End(TagEnd::CodeBlock) => {
                code_ranges.push(range);
                code_depth = code_depth.saturating_sub(1);
            }
            Event::Code(_) => code_ranges.push(range),
            Event::Start(Tag::Link { dest_url, .. }) if code_depth == 0 => {
                if let Some(target) = markdown_link_target(&dest_url) {
                    links.insert(target);
                }
            }
            _ if code_depth > 0 => code_ranges.push(range),
            _ => {}
        }
    }
    let mut offset = 0usize;
    while let Some(relative_start) = body[offset..].find("[[") {
        let start = offset + relative_start;
        let after_open = &body[start + 2..];
        let Some(end) = after_open.find("]]") else {
            break;
        };
        if !code_ranges
            .iter()
            .any(|range| range.start <= start && start < range.end)
        {
            let target = after_open[..end]
                .split('|')
                .next()
                .unwrap_or_default()
                .split('#')
                .next()
                .unwrap_or_default()
                .trim();
            if !target.is_empty() {
                links.insert(target.to_string());
            }
        }
        offset = start + 2 + end + 2;
    }
    links.into_iter().collect()
}

fn markdown_link_target(destination: &str) -> Option<String> {
    let path = destination.split(['#', '?']).next()?;
    if !path.to_ascii_lowercase().ends_with(".md") {
        return None;
    }
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn segment_error(error: crate::segment::SegmentError) -> AppError {
    AppError::new(
        "graph_index_capacity_exceeded",
        format!("document segmentation failed: {error:?}"),
    )
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn index_source(
    tx: &Transaction<'_>,
    rowid: Option<i64>,
    source_id: i64,
    title: Option<&str>,
    origin: &str,
    content: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
        params![source_id.to_string()],
    )?;
    tx.execute(
        "INSERT INTO search_fts(
            rowid, doc_type, identifier, title_terms, path_terms, summary_terms, body_terms
         ) VALUES (?1, 'source', ?2, ?3, ?4, '', ?5)",
        params![
            rowid,
            source_id.to_string(),
            source_title_terms(title.unwrap_or(""), origin),
            joined_terms(source_parent(origin)),
            joined_terms(content)
        ],
    )?;
    if search_spans_available(tx)? {
        index_spans(
            tx,
            "source",
            &source_id.to_string(),
            title.unwrap_or(origin),
            origin,
            content,
        )?;
    }
    Ok(())
}

fn index_page(
    tx: &Transaction<'_>,
    rowid: Option<i64>,
    slug: &str,
    title: &str,
    summary: Option<&str>,
    body: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
        params![slug],
    )?;
    tx.execute(
        "INSERT INTO search_fts(
            rowid, doc_type, identifier, title_terms, path_terms, summary_terms, body_terms
         ) VALUES (?1, 'page', ?2, ?3, ?4, ?5, ?6)",
        params![
            rowid,
            slug,
            joined_terms(title),
            joined_terms(slug),
            joined_terms(summary.unwrap_or("")),
            joined_terms(body)
        ],
    )?;
    if search_spans_available(tx)? {
        index_spans(tx, "page", slug, title, slug, body)?;
    }
    Ok(())
}

fn search_spans_available(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'search_spans')",
        [],
        |row| row.get(0),
    )?)
}

fn index_spans(
    tx: &Transaction<'_>,
    document_type: &str,
    document_identifier: &str,
    title: &str,
    path: &str,
    content: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE search_spans SET active = 0
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM span_fts WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    let fingerprint = hash_content(content);
    let segmented = crate::segment::segment_document(content).map_err(segment_error)?;
    let mut insert_span = tx.prepare(
        "INSERT INTO search_spans(
            span_id, span_type, document_type, document_identifier,
            parent_identifier, ordinal, byte_start, byte_end,
            content_fingerprint, segmenter_version, active
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)
         ON CONFLICT(span_id) DO UPDATE SET active = 1",
    )?;
    let mut insert_fts = tx.prepare(
        "INSERT INTO span_fts(
            span_id, span_type, document_type, document_identifier,
            title_terms, path_terms, heading_terms, body_terms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    let document_key = format!("{document_type}:{document_identifier}");
    for passage in segmented.passages {
        let heading_path = passage.heading_path.join(" ");
        let passage_id = format!(
            "span:{}",
            hash_content(&format!(
                "passage\0{document_key}\0{fingerprint}\0{}\0{}",
                passage.range.start, passage.range.end
            ))
        );
        insert_search_span(
            &mut insert_span,
            &mut insert_fts,
            &passage_id,
            "passage",
            document_type,
            document_identifier,
            &document_key,
            passage.ordinal,
            passage.range.clone(),
            &fingerprint,
            title,
            path,
            &heading_path,
            content,
        )?;
        for sentence in passage.sentences {
            let sentence_id = format!(
                "span:{}",
                hash_content(&format!(
                    "sentence\0{document_key}\0{fingerprint}\0{}\0{}",
                    sentence.range.start, sentence.range.end
                ))
            );
            insert_search_span(
                &mut insert_span,
                &mut insert_fts,
                &sentence_id,
                "sentence",
                document_type,
                document_identifier,
                &passage_id,
                sentence.ordinal,
                sentence.range,
                &fingerprint,
                title,
                path,
                &heading_path,
                content,
            )?;
        }
    }
    Ok(())
}

fn deactivate_search_spans(
    tx: &Transaction<'_>,
    document_type: &str,
    document_identifier: &str,
) -> Result<()> {
    if !search_spans_available(tx)? {
        return Ok(());
    }
    tx.execute(
        "UPDATE search_spans SET active = 0
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM span_fts WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_search_span(
    span: &mut rusqlite::Statement<'_>,
    fts: &mut rusqlite::Statement<'_>,
    id: &str,
    span_type: &str,
    document_type: &str,
    document_identifier: &str,
    parent_identifier: &str,
    ordinal: usize,
    range: Range<usize>,
    fingerprint: &str,
    title: &str,
    path: &str,
    heading_path: &str,
    content: &str,
) -> Result<()> {
    let text = content
        .get(range.clone())
        .ok_or_else(|| AppError::new("invalid_span", "Markdown span is outside its document"))?;
    span.execute(params![
        id,
        span_type,
        document_type,
        document_identifier,
        parent_identifier,
        ordinal as i64,
        range.start as i64,
        range.end as i64,
        fingerprint,
        i64::from(crate::segment::SEGMENTER_VERSION),
    ])?;
    fts.execute(params![
        id,
        span_type,
        document_type,
        document_identifier,
        joined_terms(title),
        joined_terms(path),
        joined_terms(heading_path),
        joined_terms(text),
    ])?;
    Ok(())
}

fn source_title_terms(title: &str, origin: &str) -> String {
    let leaf = source_leaf(origin);
    if normalized_text(title) == normalized_text(origin)
        || normalized_text(title) == normalized_text(leaf)
    {
        joined_terms(leaf)
    } else {
        joined_terms(&format!("{title} {leaf}"))
    }
}

fn source_leaf(origin: &str) -> &str {
    Path::new(origin)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(origin)
}

fn source_parent(origin: &str) -> &str {
    Path::new(origin)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("")
}

fn has_structural_navigation_marker(body: &str) -> bool {
    let body = body.to_lowercase();
    [
        "总览文档",
        "文档目录",
        "table of contents",
        "navigation index",
        "document index",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

fn joined_terms(text: &str) -> String {
    joined_index_terms(text)
}

fn search_index(
    conn: &Connection,
    scope: &str,
    raw_query: &str,
    tokens: &[String],
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
    let first_token = tokens.first().map(String::as_str).unwrap_or(raw_query);
    let query = raw_query.trim();
    let sql = "WITH ranked AS (
            SELECT
                search_fts.rowid AS fts_rowid,
                search_fts.doc_type AS doc_type,
                search_fts.identifier AS identifier,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.title
                    ELSE s.title
                END AS title,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.kind
                    ELSE NULL
                END AS kind,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.summary
                    ELSE NULL
                END AS summary,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.body
                    ELSE s.content
                END AS body,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.slug
                    ELSE s.origin
                END AS path,
                CASE search_fts.doc_type
                    WHEN 'page' THEN p.structural_navigation
                    ELSE s.structural_navigation
                END AS structural_navigation,
                bm25(search_fts, 0.0, 0.0, 8.0, 6.0, 4.0, 1.0) AS fts_rank
            FROM search_fts
            LEFT JOIN pages p
                ON search_fts.doc_type = 'page'
               AND p.slug = search_fts.identifier
            LEFT JOIN sources s
                ON search_fts.doc_type = 'source'
               AND s.id = CAST(search_fts.identifier AS INTEGER)
            WHERE search_fts MATCH ?1
            ORDER BY fts_rank ASC, fts_rowid ASC
            LIMIT ?2
        )
        SELECT
            doc_type,
            identifier,
            title,
            kind,
            summary,
            path,
            CASE
                WHEN INSTR(LOWER(body), LOWER(?3)) > 0 THEN
                    SUBSTR(body, MAX(INSTR(LOWER(body), LOWER(?3)) - 60, 1), 180)
                WHEN INSTR(LOWER(body), LOWER(?4)) > 0 THEN
                    SUBSTR(body, MAX(INSTR(LOWER(body), LOWER(?4)) - 60, 1), 180)
                ELSE SUBSTR(body, 1, 180)
            END AS snippet,
            fts_rank,
            CASE
                WHEN doc_type = 'page' AND LOWER(COALESCE(kind, '')) = 'source'
                THEN (
                    SELECT GROUP_CONCAT(ps.source_id, ',')
                    FROM page_sources ps
                    WHERE ps.page_slug = identifier
                )
                ELSE NULL
            END AS paired_source_ids,
            structural_navigation
        FROM ranked";
    let mut statement = conn.prepare(sql)?;
    statement
        .query_map(
            params![match_query, limit as i64, query, first_token],
            |row| {
                let title = row.get::<_, Option<String>>(2)?;
                let result_type = row.get::<_, String>(0)?;
                let path = row.get::<_, String>(5)?;
                let fts_rank = row.get::<_, f64>(7)?;
                let paired_source_ids = row
                    .get::<_, Option<String>>(8)?
                    .map(|ids| {
                        ids.split(',')
                            .filter_map(|id| id.parse::<i64>().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let explanation = lexical_explanation(
                    &result_type,
                    title.as_deref(),
                    &path,
                    query,
                    tokens,
                    fts_rank,
                    row.get::<_, i64>(9)? != 0,
                );
                Ok(SearchResult {
                    scope: scope.to_string(),
                    result_type,
                    identifier: row.get(1)?,
                    document: None,
                    span: None,
                    fused_score: None,
                    matches: None,
                    rank: explanation.final_rank,
                    title,
                    kind: row.get(3)?,
                    summary: row.get(4)?,
                    provenance: None,
                    snippet: row.get(6)?,
                    explanation: Some(explanation),
                    paired_source_ids,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
