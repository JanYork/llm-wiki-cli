const WORD_GRAPH_MAX_QUERY_TERMS: usize = 8;
const WORD_GRAPH_MAX_DOCUMENTS: usize = 25;
const WORD_GRAPH_MAX_TERMS: usize = 30;
const WORD_GRAPH_MAX_NODES: usize = 200;
const WORD_GRAPH_MAX_EDGES: usize = 500;
const WORD_GRAPH_MAX_INSPECTED_BYTES: usize = 4 * 1024 * 1024;
const WORD_GRAPH_MAX_SPANS_PER_DOCUMENT: usize = 4;
const WORD_GRAPH_MAX_METADATA_BYTES_PER_DOCUMENT: usize = 16 * 1024;
const WORD_GRAPH_MAX_OFFSET: usize = 10_000;

const WORD_GRAPH_CANDIDATE_SQL: &str = "SELECT
        search_fts.doc_type,
        search_fts.identifier,
        CASE search_fts.doc_type WHEN 'page' THEN p.title ELSE s.title END,
        CASE search_fts.doc_type WHEN 'page' THEN p.kind ELSE NULL END,
        CASE search_fts.doc_type WHEN 'page' THEN p.summary ELSE NULL END,
        CASE search_fts.doc_type WHEN 'page' THEN p.slug ELSE s.origin END,
        bm25(search_fts, 0.0, 0.0, 8.0, 6.0, 4.0, 1.0) AS fts_rank
     FROM search_fts
     LEFT JOIN pages p
       ON search_fts.doc_type = 'page' AND p.slug = search_fts.identifier
     LEFT JOIN sources s
       ON search_fts.doc_type = 'source'
      AND s.id = CAST(search_fts.identifier AS INTEGER)
     WHERE search_fts MATCH ?1
     ORDER BY fts_rank ASC, search_fts.doc_type ASC, search_fts.identifier ASC
     LIMIT ?2 OFFSET ?3";

const WORD_GRAPH_SPAN_SQL: &str = "SELECT
        CASE n.document_type
            WHEN 'page' THEN SUBSTR(CAST(p.body AS BLOB), n.byte_start + 1, n.byte_end - n.byte_start)
            ELSE SUBSTR(CAST(s.content AS BLOB), n.byte_start + 1, n.byte_end - n.byte_start)
        END
     FROM search_spans n
     LEFT JOIN pages p
       ON n.document_type = 'page' AND p.slug = n.document_identifier
     LEFT JOIN sources s
       ON n.document_type = 'source'
      AND s.id = CAST(n.document_identifier AS INTEGER)
     WHERE n.active = 1
       AND n.span_type = 'passage'
       AND n.document_type = ?1
       AND n.document_identifier = ?2
     ORDER BY n.ordinal ASC, n.span_id ASC
     LIMIT ?3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordGraphOptions {
    pub document_limit: usize,
    pub term_limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphLimits {
    pub max_query_terms: usize,
    pub document_limit: usize,
    pub term_limit: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_inspected_bytes: usize,
    pub max_spans_per_document: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphDiagnostics {
    pub inspected_documents: usize,
    pub inspected_spans: usize,
    pub inspected_bytes: usize,
    pub inspected_tokens: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphDocumentNode {
    pub id: String,
    pub label: String,
    pub document_type: String,
    pub identifier: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphTermNode {
    pub id: String,
    pub label: String,
    pub sample_document_frequency: usize,
    pub sample_occurrences: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphEdge {
    pub id: String,
    pub term: String,
    pub document: String,
    pub sample_occurrences: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WordGraphResponse {
    pub query: String,
    pub query_terms: Vec<String>,
    pub documents: Vec<WordGraphDocumentNode>,
    pub terms: Vec<WordGraphTermNode>,
    pub edges: Vec<WordGraphEdge>,
    pub has_more: bool,
    pub truncated: bool,
    pub truncation_reasons: Vec<String>,
    pub limits: WordGraphLimits,
    pub diagnostics: WordGraphDiagnostics,
}

#[derive(Debug)]
struct WordCandidate {
    document_type: String,
    identifier: String,
    title: String,
    kind: Option<String>,
    summary: Option<String>,
    path: String,
}

impl Store {
    pub fn word_graph(
        &self,
        query: &str,
        options: &WordGraphOptions,
    ) -> Result<WordGraphResponse> {
        let query = query.trim();
        let query_terms = tokenize_for_query(query);
        if query_terms.is_empty() {
            return Err(AppError::new(
                "invalid_query",
                "word graph query must contain at least one searchable term",
            ));
        }
        if query_terms.len() > WORD_GRAPH_MAX_QUERY_TERMS {
            return Err(AppError::new(
                "invalid_query",
                format!(
                    "word graph query contains more than {WORD_GRAPH_MAX_QUERY_TERMS} searchable terms"
                ),
            ));
        }

        let document_limit = options.document_limit.clamp(1, WORD_GRAPH_MAX_DOCUMENTS);
        let term_limit = options.term_limit.clamp(1, WORD_GRAPH_MAX_TERMS);
        let offset = options.offset.min(WORD_GRAPH_MAX_OFFSET);
        let match_query = word_match_query(&query_terms);
        let mut candidates = self.word_graph_candidates(
            &match_query,
            document_limit.saturating_add(1),
            offset,
        )?;
        let has_more = candidates.len() > document_limit;
        candidates.truncate(document_limit);

        let mut reasons = BTreeSet::new();
        if has_more {
            reasons.insert("documents".to_string());
        }
        if options.document_limit > WORD_GRAPH_MAX_DOCUMENTS
            || options.term_limit > WORD_GRAPH_MAX_TERMS
            || options.offset > WORD_GRAPH_MAX_OFFSET
        {
            reasons.insert("request_limits".to_string());
        }

        let mut memberships = BTreeMap::<String, BTreeMap<usize, usize>>::new();
        let mut diagnostics = WordGraphDiagnostics {
            inspected_documents: candidates.len(),
            inspected_spans: 0,
            inspected_bytes: 0,
            inspected_tokens: 0,
        };

        for (document_index, candidate) in candidates.iter().enumerate() {
            let metadata_full = format!(
                "{}\n{}\n{}",
                candidate.title,
                candidate.summary.as_deref().unwrap_or(""),
                candidate.path
            );
            let metadata = utf8_prefix(&metadata_full, WORD_GRAPH_MAX_METADATA_BYTES_PER_DOCUMENT);
            if metadata.len() < metadata_full.len() {
                reasons.insert("metadata".to_string());
            }
            add_word_sample(
                metadata,
                document_index,
                &mut memberships,
                &mut diagnostics,
                &mut reasons,
            );
        }

        if diagnostics.inspected_bytes < WORD_GRAPH_MAX_INSPECTED_BYTES {
            let mut spans = self.conn.prepare(WORD_GRAPH_SPAN_SQL)?;
            'documents: for (document_index, candidate) in candidates.iter().enumerate() {
                let rows = spans.query_map(
                    params![
                        &candidate.document_type,
                        &candidate.identifier,
                        WORD_GRAPH_MAX_SPANS_PER_DOCUMENT as i64
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )?;
                for row in rows {
                    let bytes = row?;
                    let text = std::str::from_utf8(&bytes).map_err(|_| {
                        AppError::new(
                            "invalid_store",
                            "word graph span is not valid UTF-8",
                        )
                    })?;
                    diagnostics.inspected_spans += 1;
                    let complete = add_word_sample(
                        text,
                        document_index,
                        &mut memberships,
                        &mut diagnostics,
                        &mut reasons,
                    );
                    if !complete {
                        break 'documents;
                    }
                }
            }
        }

        let selected_terms = select_word_terms(
            &query_terms,
            &memberships,
            term_limit,
            &mut reasons,
        );
        let documents = candidates
            .iter()
            .map(|candidate| WordGraphDocumentNode {
                id: format!("{}:{}", candidate.document_type, candidate.identifier),
                label: candidate.title.clone(),
                document_type: candidate.document_type.clone(),
                identifier: candidate.identifier.clone(),
                kind: candidate.kind.clone(),
            })
            .collect::<Vec<_>>();
        let terms = selected_terms
            .iter()
            .map(|term| {
                let counts = &memberships[term];
                WordGraphTermNode {
                    id: format!("term:{term}"),
                    label: term.clone(),
                    sample_document_frequency: counts.len(),
                    sample_occurrences: counts.values().sum(),
                }
            })
            .collect::<Vec<_>>();

        let mut edges = Vec::new();
        'terms: for term in &selected_terms {
            let term_id = format!("term:{term}");
            for (document_index, count) in &memberships[term] {
                if edges.len() == WORD_GRAPH_MAX_EDGES {
                    reasons.insert("edges".to_string());
                    break 'terms;
                }
                let document_id = &documents[*document_index].id;
                edges.push(WordGraphEdge {
                    id: format!("{term_id}->{document_id}"),
                    term: term_id.clone(),
                    document: document_id.clone(),
                    sample_occurrences: *count,
                });
            }
        }

        let truncation_reasons = reasons.into_iter().collect::<Vec<_>>();
        Ok(WordGraphResponse {
            query: query.to_string(),
            query_terms,
            documents,
            terms,
            edges,
            has_more,
            truncated: !truncation_reasons.is_empty(),
            truncation_reasons,
            limits: WordGraphLimits {
                max_query_terms: WORD_GRAPH_MAX_QUERY_TERMS,
                document_limit,
                term_limit,
                max_nodes: WORD_GRAPH_MAX_NODES,
                max_edges: WORD_GRAPH_MAX_EDGES,
                max_inspected_bytes: WORD_GRAPH_MAX_INSPECTED_BYTES,
                max_spans_per_document: WORD_GRAPH_MAX_SPANS_PER_DOCUMENT,
                offset,
            },
            diagnostics,
        })
    }

    fn word_graph_candidates(
        &self,
        match_query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<WordCandidate>> {
        let mut statement = self.conn.prepare(WORD_GRAPH_CANDIDATE_SQL)?;
        statement
            .query_map(params![match_query, limit as i64, offset as i64], |row| {
                Ok(WordCandidate {
                    document_type: row.get(0)?,
                    identifier: row.get(1)?,
                    title: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    kind: row.get(3)?,
                    summary: row.get(4)?,
                    path: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    #[cfg(test)]
    fn word_graph_candidate_query_plan(&self, query: &str) -> Result<Vec<String>> {
        let terms = tokenize_for_query(query);
        let explain = format!("EXPLAIN QUERY PLAN {WORD_GRAPH_CANDIDATE_SQL}");
        let mut statement = self.conn.prepare(&explain)?;
        statement
            .query_map(params![word_match_query(&terms), 2_i64, 0_i64], |row| {
                row.get(3)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    #[cfg(test)]
    fn word_graph_span_query_plan(&self) -> Result<Vec<String>> {
        let explain = format!("EXPLAIN QUERY PLAN {WORD_GRAPH_SPAN_SQL}");
        let mut statement = self.conn.prepare(&explain)?;
        statement
            .query_map(params!["page", "example", 4_i64], |row| row.get(3))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn word_match_query(terms: &[String]) -> String {
    terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn add_word_sample(
    text: &str,
    document_index: usize,
    memberships: &mut BTreeMap<String, BTreeMap<usize, usize>>,
    diagnostics: &mut WordGraphDiagnostics,
    reasons: &mut BTreeSet<String>,
) -> bool {
    let remaining = WORD_GRAPH_MAX_INSPECTED_BYTES.saturating_sub(diagnostics.inspected_bytes);
    if remaining == 0 {
        reasons.insert("bytes".to_string());
        return false;
    }
    let sample = utf8_prefix(text, remaining);
    let complete = sample.len() == text.len();
    if !complete {
        reasons.insert("bytes".to_string());
    }
    diagnostics.inspected_bytes += sample.len();
    let occurrences = crate::tokenize::tokenize_for_graph_with_positions(sample);
    diagnostics.inspected_tokens += occurrences.len();
    for occurrence in occurrences {
        *memberships
            .entry(occurrence.normalized)
            .or_default()
            .entry(document_index)
            .or_default() += 1;
    }
    complete
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn select_word_terms(
    query_terms: &[String],
    memberships: &BTreeMap<String, BTreeMap<usize, usize>>,
    term_limit: usize,
    reasons: &mut BTreeSet<String>,
) -> Vec<String> {
    let query_set = query_terms.iter().cloned().collect::<BTreeSet<_>>();
    let mut selected = query_terms
        .iter()
        .filter(|term| memberships.contains_key(*term))
        .take(term_limit)
        .cloned()
        .collect::<Vec<_>>();
    let mut discovered = memberships
        .iter()
        .filter(|(term, counts)| !query_set.contains(*term) && counts.len() >= 2)
        .map(|(term, counts)| {
            (
                term.clone(),
                counts.len(),
                counts.values().sum::<usize>(),
            )
        })
        .collect::<Vec<_>>();
    discovered.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    let available = selected.len() + discovered.len();
    for (term, _, _) in discovered {
        if selected.len() == term_limit {
            break;
        }
        selected.push(term);
    }
    if available > selected.len() || query_terms.len() > selected.len() {
        reasons.insert("terms".to_string());
    }
    selected
}

#[cfg(test)]
mod word_graph_tests {
    use super::*;
    use tempfile::TempDir;

    fn word_store() -> (TempDir, Store) {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (store, _) = Store::initialize("project", database).unwrap();
        (temp, store)
    }

    fn put_page(store: &mut Store, slug: &str, title: &str, body: String) {
        store
            .page_put(PagePutInput {
                slug: slug.to_string(),
                title: title.to_string(),
                kind: Some("concept".to_string()),
                summary: None,
                body,
                source_ids: Vec::new(),
                provenance: vec!["user-provided".to_string()],
            })
            .unwrap();
    }

    fn options(document_limit: usize, term_limit: usize, offset: usize) -> WordGraphOptions {
        WordGraphOptions {
            document_limit,
            term_limit,
            offset,
        }
    }

    #[test]
    fn word_graph_connects_pages_and_sources_through_normalized_shared_terms() {
        let (_temp, mut store) = word_store();
        put_page(
            &mut store,
            "alpha",
            "Alpha 库存",
            "库存 共享 inventory inventory alpha".to_string(),
        );
        put_page(
            &mut store,
            "beta",
            "Beta 库存",
            "库存 共享 inventory beta".to_string(),
        );
        put_page(
            &mut store,
            "unrelated",
            "Unrelated",
            "separate material".to_string(),
        );
        let source = store
            .source_add(SourceAddInput {
                title: Some("Source 库存".to_string()),
                origin: "fixtures/inventory.txt".to_string(),
                tracked_path: None,
                content: "库存 共享 inventory warehouse".to_string(),
            })
            .unwrap();

        let graph = store.word_graph("库存", &options(10, 10, 0)).unwrap();
        assert_eq!(graph.query_terms, vec!["库存"]);
        assert_eq!(graph.documents.len(), 3);
        assert!(graph.documents.iter().any(|node| node.id == "page:alpha"));
        assert!(graph.documents.iter().any(|node| node.id == "page:beta"));
        assert!(
            graph
                .documents
                .iter()
                .any(|node| node.id == format!("source:{}", source.source.id))
        );
        assert!(!graph.documents.iter().any(|node| node.id == "page:unrelated"));

        let query_term = graph
            .terms
            .iter()
            .find(|node| node.id == "term:库存")
            .unwrap();
        assert_eq!(query_term.sample_document_frequency, 3);
        assert!(
            graph
                .terms
                .iter()
                .any(|node| node.id == "term:共享" && node.sample_document_frequency == 3)
        );
        assert!(!graph.terms.iter().any(|node| node.id == "term:库"));
        assert_eq!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.term == "term:库存")
                .count(),
            3
        );
        assert!(graph.edges.iter().all(|edge| edge.sample_occurrences > 0));
        assert_eq!(graph.diagnostics.inspected_documents, 3);
        assert!(graph.diagnostics.inspected_bytes <= graph.limits.max_inspected_bytes);
    }

    #[test]
    fn word_graph_clamps_dense_results_and_pages_deterministically() {
        let (_temp, mut store) = word_store();
        for index in 0..40 {
            put_page(
                &mut store,
                &format!("dense-{index:02}"),
                &format!("Dense {index:02}"),
                format!("needle shared common group{}", index % 4),
            );
        }

        let first = store.word_graph("needle", &options(100, 100, 0)).unwrap();
        let repeated = store.word_graph("needle", &options(100, 100, 0)).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(first.limits.document_limit, 25);
        assert_eq!(first.limits.term_limit, 30);
        assert_eq!(first.documents.len(), 25);
        assert!(first.terms.len() <= 30);
        assert!(first.edges.len() <= 500);
        assert!(first.documents.len() + first.terms.len() <= 200);
        assert!(first.has_more);
        assert!(first.truncated);
        assert!(first.truncation_reasons.contains(&"documents".to_string()));

        let second_page = store.word_graph("needle", &options(25, 30, 25)).unwrap();
        assert_eq!(second_page.documents.len(), 15);
        assert!(!second_page.has_more);
        assert!(
            first
                .documents
                .iter()
                .all(|left| second_page.documents.iter().all(|right| left.id != right.id))
        );
    }

    #[test]
    fn word_graph_bounds_sampled_passage_bytes_before_tokenization() {
        let (_temp, mut store) = word_store();
        let large = "needle shared ".repeat(20_000);
        for index in 0..25 {
            put_page(
                &mut store,
                &format!("large-{index:02}"),
                &format!("Large {index:02}"),
                large.clone(),
            );
        }

        let graph = store.word_graph("needle", &options(25, 30, 0)).unwrap();
        assert!(graph.diagnostics.inspected_bytes <= 4 * 1024 * 1024);
        assert!(graph.diagnostics.inspected_documents <= 25);
        assert!(graph.diagnostics.inspected_spans <= 25 * 4);
        assert!(graph.truncated);
        assert!(graph.truncation_reasons.contains(&"bytes".to_string()));
    }

    #[test]
    fn word_graph_rejects_empty_or_excessive_queries_and_uses_fts_plan() {
        let (_temp, store) = word_store();
        for query in ["", "the is a", "one two three four five six seven eight nine"] {
            let error = store.word_graph(query, &options(10, 10, 0)).unwrap_err();
            assert_eq!(error.code, "invalid_query");
        }

        let details = store.word_graph_candidate_query_plan("needle").unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("VIRTUAL TABLE INDEX")),
            "{details:?}"
        );
        assert!(details.iter().any(|detail| detail.contains("search_fts")));
        let span_details = store.word_graph_span_query_plan().unwrap();
        assert!(
            span_details
                .iter()
                .any(|detail| detail.contains("search_spans_document")),
            "{span_details:?}"
        );
    }
}
