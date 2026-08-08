use crate::{
    artifacts,
    config::{self, EngineSetting, PhysicalSetting},
    error::{AppError, Result},
    graph::{
        CanonicalEdge, CooccurrenceBuild, DocumentGraphInput, DocumentGraphReplacement,
        DocumentType, GraphPage, MAX_COOCCURRENCE_CONTRIBUTIONS, TermPairContribution,
        automatic_edge, build_cooccurrence, build_document_graph, rank_cooccurrence, related,
    },
    graph_backend::{
        create_hierarchical_graph_schema, decode_positions, encode_positions,
        graphqlite_projection_counts, project_graphqlite_snapshot,
    },
    tokenize::{joined_index_terms, tokenize_for_query},
};
use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    backup::Backup, ffi, params,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::cell::Cell;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::CStr,
    fs,
    io::{BufReader, BufWriter, Seek, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const COMPOUND_WIKI_VERSION: i32 = 6;
const PAGE_PROVENANCE_VERSION: i32 = 7;
const SOURCE_PATH_REVISIONS_VERSION: i32 = 8;
const RETRIEVAL_WEIGHTING_VERSION: i32 = 9;
const CHANGESETS_VERSION: i32 = 10;
const USER_VERSION: i32 = 11;
const CHANGESET_FREEZE_KEY: &str = "changeset_frozen";
const SEARCH_INDEX_VERSION: i32 = 4;
const INGEST_WORKFLOW_VERSION: i32 = 5;
const TOKENIZER_ID: &str = "cjk-bigram@1/bounded-terms";
const SOURCE_GROUNDED: &str = "source-grounded";
const EXPLICIT_PROVENANCE: [&str; 3] = ["user-provided", "agent-observed", "hypothesis"];
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const TIMESTAMP_SQL: &str = "STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')";
const TITLE_WEIGHT: f64 = 32.0;
const PATH_WEIGHT: f64 = 16.0;
const GENERIC_WEIGHT: f64 = 8.0;
const GRAPH_MATCH_WEIGHT: f64 = 0.25;
const GRAPH_HUB_WEIGHT: f64 = 4.0;
const MANUAL_WEIGHT: f64 = 2.0;
const FEEDBACK_WEIGHT: f64 = 1.5;
const MAX_COOCCURRENCE_BLOB_BYTES: usize = 16 * 1024 * 1024;
const MAX_COOCCURRENCE_TERMS: usize = 100_000;
static SOURCE_STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
type MigrationProgress<'a> = dyn FnMut(usize, usize, &str) -> Result<()> + 'a;

#[cfg(test)]
thread_local! {
    static TEST_GRAPH_DIGEST_CALLS: Cell<usize> = const { Cell::new(0) };
    static TEST_COOCCURRENCE_REBUILDS: Cell<usize> = const { Cell::new(0) };
    static TEST_GLOBAL_TERM_PAIR_LOADS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn reset_test_graph_build_counts() {
    TEST_GRAPH_DIGEST_CALLS.set(0);
    TEST_COOCCURRENCE_REBUILDS.set(0);
    TEST_GLOBAL_TERM_PAIR_LOADS.set(0);
}

#[cfg(test)]
fn test_graph_build_counts() -> (usize, usize) {
    (
        TEST_GRAPH_DIGEST_CALLS.get(),
        TEST_COOCCURRENCE_REBUILDS.get(),
    )
}

#[cfg(test)]
fn test_global_term_pair_loads() -> usize {
    TEST_GLOBAL_TERM_PAIR_LOADS.get()
}
pub const DEFAULT_SCHEMA: &str = r#"# Wiki Schema

## Page types

- `entity`: named people, organizations, products, systems, and datasets.
- `concept`: ideas, techniques, patterns, and phenomena.
- `source`: one traceable summary for each ingested source.
- `query`: durable answers and open questions.
- `comparison`: side-by-side analysis.
- `synthesis`: cross-cutting conclusions.

## Rules

- Every page declares provenance. Source-grounded pages cite source IDs;
  user-provided facts, Agent observations, and hypotheses use explicit
  provenance classes.
- Use `[[stable-slug]]` for cross-references.
- Read an existing page before replacing it and preserve still-valid knowledge.
- Record contradictions instead of silently choosing one source.
- Treat source summaries as navigation, not a substitute for entity, concept, comparison, and synthesis pages.
- Before completing ingest, update affected shared pages or record why the source needs no derived-page update.
"#;
pub const DEFAULT_PURPOSE: &str = r#"# Project Purpose

## Goal

Build a persistent, traceable wiki from curated project sources.

## Key questions

1. What should this wiki help its users understand or decide?
2. Which sources and claims are authoritative?

## Scope

Keep project knowledge here; put reusable cross-project knowledge in the global wiki.
"#;
const LINT_ISSUES_SQL: &str = r#"
WITH issues(code, page, target, message) AS (
    SELECT
        'missing_schema', NULL, NULL, 'schema has not been set'
    WHERE NOT EXISTS (
        SELECT 1 FROM meta WHERE key = 'schema' AND TRIM(value) <> ''
    )
    UNION ALL
    SELECT
        'missing_summary', slug, NULL, 'page summary is missing'
    FROM pages
    WHERE summary IS NULL OR TRIM(summary) = ''
    UNION ALL
    SELECT
        'untitled_source', NULL, CAST(id AS TEXT), 'source title is missing'
    FROM sources
    WHERE title IS NULL OR TRIM(title) = ''
    UNION ALL
    SELECT
        'shallow_ingest',
        NULL,
        CAST(ij.source_id AS TEXT),
        'completed ingest has no cited non-source page and no explicit reason'
    FROM ingest_jobs ij
    WHERE ij.status = 'completed'
      AND (ij.no_derived_pages_reason IS NULL OR TRIM(ij.no_derived_pages_reason) = '')
      AND NOT EXISTS (
          SELECT 1
          FROM page_sources ps
          JOIN pages p ON p.slug = ps.page_slug
          WHERE ps.source_id = ij.source_id
            AND LOWER(COALESCE(p.kind, '')) <> 'source'
      )
    UNION ALL
    SELECT
        'uncited_page', p.slug, NULL,
        'page has neither cited sources nor explicit provenance'
    FROM pages p
    WHERE NOT EXISTS (
        SELECT 1 FROM page_sources ps WHERE ps.page_slug = p.slug
    )
      AND NOT EXISTS (
        SELECT 1 FROM page_provenance pp WHERE pp.page_slug = p.slug
    )
    UNION ALL
    SELECT
        'orphan_page', p.slug, NULL, 'page has no inbound wikilinks'
    FROM pages p
    LEFT JOIN links l ON l.to_slug = p.slug
    WHERE l.to_slug IS NULL
    UNION ALL
    SELECT
        'dangling_link', l.from_slug, l.to_slug, 'wikilink target does not exist'
    FROM links l
    LEFT JOIN pages p ON p.slug = l.to_slug
    WHERE p.slug IS NULL
    UNION ALL
    SELECT
        'search_index_duplicate',
        doc_type || ':' || identifier,
        NULL,
        'search index contains duplicate rows for one document'
    FROM search_fts
    GROUP BY doc_type, identifier
    HAVING COUNT(*) > 1
    UNION ALL
    SELECT
        'search_index_orphan',
        f.doc_type || ':' || f.identifier,
        NULL,
        'search index row has no matching document'
    FROM search_fts f
    LEFT JOIN pages p
      ON f.doc_type = 'page' AND p.slug = f.identifier
    LEFT JOIN sources s
      ON f.doc_type = 'source' AND s.id = CAST(f.identifier AS INTEGER)
    WHERE (f.doc_type = 'page' AND p.slug IS NULL)
       OR (f.doc_type = 'source' AND s.id IS NULL)
       OR f.doc_type NOT IN ('page', 'source')
    UNION ALL
    SELECT
        'search_index_missing',
        'page:' || identifier,
        NULL,
        'document is missing from the search index'
    FROM (
        SELECT slug AS identifier FROM pages
        EXCEPT
        SELECT identifier FROM search_fts WHERE doc_type = 'page'
    )
    UNION ALL
    SELECT
        'search_index_missing',
        'source:' || identifier,
        NULL,
        'document is missing from the search index'
    FROM (
        SELECT CAST(id AS TEXT) AS identifier FROM sources
        EXCEPT
        SELECT identifier FROM search_fts WHERE doc_type = 'source'
    )
    UNION ALL
    SELECT
        'retrieval_weight_orphan',
        CASE WHEN r.target_type = 'page' THEN r.target_identifier END,
        r.target_type || ':' || r.target_identifier,
        'retrieval weight target does not exist'
    FROM retrieval_weights r
    WHERE (r.target_type = 'page' AND NOT EXISTS (
              SELECT 1 FROM pages p WHERE p.slug = r.target_identifier
          ))
       OR (r.target_type = 'source' AND NOT EXISTS (
              SELECT 1 FROM sources s WHERE CAST(s.id AS TEXT) = r.target_identifier
          ))
    UNION ALL
    SELECT
        'retrieval_feedback_orphan',
        CASE WHEN r.target_type = 'page' THEN r.target_identifier END,
        r.target_type || ':' || r.target_identifier || ':' || SUBSTR(r.query_fingerprint, 1, 12),
        'retrieval feedback target does not exist'
    FROM retrieval_feedback r
    WHERE (r.target_type = 'page' AND NOT EXISTS (
              SELECT 1 FROM pages p WHERE p.slug = r.target_identifier
          ))
       OR (r.target_type = 'source' AND NOT EXISTS (
              SELECT 1 FROM sources s WHERE CAST(s.id AS TEXT) = r.target_identifier
          ))
)
"#;

#[derive(Debug)]
pub struct Store {
    scope: String,
    database: PathBuf,
    conn: Connection,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceAddInput {
    pub title: Option<String>,
    pub origin: String,
    pub tracked_path: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PagePutInput {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub body: String,
    pub source_ids: Vec<i64>,
    pub provenance: Vec<String>,
}

#[derive(Debug)]
struct PageMutationBase {
    title: String,
    body: String,
    content_fingerprint: String,
    version_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SchemaResponse {
    pub scope: String,
    pub database: String,
    pub schema: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PurposeResponse {
    pub scope: String,
    pub database: String,
    pub purpose: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRecord {
    pub id: i64,
    pub title: Option<String>,
    pub origin: String,
    pub content_hash: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceSummary {
    pub id: i64,
    pub title: Option<String>,
    pub origin: String,
    pub content_hash: String,
    pub bytes: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceAddResponse {
    pub scope: String,
    pub database: String,
    pub source: SourceSummary,
    pub created: bool,
    pub graph: GraphMutationSummary,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphMutationSummary {
    pub generation: i64,
    pub passages: usize,
    pub sentences: usize,
    pub terms: usize,
    pub cooccurrence_truncated: usize,
    pub invalidated_semantic_relations: usize,
    pub projection_engine: String,
    pub projection_status: String,
    pub canonical_duration_ms: u64,
    pub projection_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRemoveResponse {
    pub scope: String,
    pub database: String,
    pub source_id: i64,
    pub removed: bool,
    pub removed_path_revisions: usize,
    pub untracked_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceListResponse {
    pub scope: String,
    pub database: String,
    pub sources: Vec<SourceSummary>,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceStatusTarget {
    pub requested_source_id: i64,
    pub tracked_path: String,
    pub head_source_id: i64,
    pub head_revision: i64,
    pub head_content_hash: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceStatusTargets {
    pub targets: Vec<SourceStatusTarget>,
    pub untracked_source_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceShowResponse {
    pub scope: String,
    pub database: String,
    pub source: SourceRecord,
    pub window: SourceWindow,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceWindow {
    pub offset_chars: usize,
    pub returned_chars: usize,
    pub total_chars: usize,
    pub next_offset_chars: Option<usize>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageRecord {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub body: String,
    pub source_ids: Vec<i64>,
    pub provenance: Vec<String>,
    pub links: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageSummary {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub provenance: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PagePutResponse {
    pub scope: String,
    pub database: String,
    pub page: PageWriteRecord,
    pub created: bool,
    pub graph: GraphMutationSummary,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageRemoveResponse {
    pub scope: String,
    pub database: String,
    pub slug: String,
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageWriteRecord {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub source_ids: Vec<i64>,
    pub provenance: Vec<String>,
    pub links: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageListResponse {
    pub scope: String,
    pub database: String,
    pub pages: Vec<PageSummary>,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageShowResponse {
    pub scope: String,
    pub database: String,
    pub page: PageRecord,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageLinksResponse {
    pub scope: String,
    pub database: String,
    pub page: String,
    pub outgoing: Vec<String>,
    pub backlinks: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRefsResponse {
    pub scope: String,
    pub database: String,
    pub source: SourceSummary,
    pub pages: Vec<PageSummary>,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResult {
    pub scope: String,
    #[serde(rename = "type")]
    pub result_type: String,
    pub identifier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<SearchDocumentRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SearchSpanRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fused_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<SearchMatch>>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub provenance: Option<Vec<String>>,
    pub snippet: String,
    pub rank: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<SearchExplanation>,
    #[serde(skip)]
    paired_source_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchMatch {
    #[serde(rename = "type")]
    pub result_type: String,
    pub identifier: String,
    pub snippet: String,
    pub rank: f64,
    pub fused_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SearchSpanRef>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchDocumentRef {
    #[serde(rename = "type")]
    pub document_type: String,
    pub identifier: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchSpanRef {
    pub parent_identifier: String,
    pub ordinal: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub content_fingerprint: String,
    pub segmenter_version: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchExplanation {
    pub base_rank: f64,
    pub signals: SearchSignals,
    pub contributions: SearchContributions,
    pub graph_seeds: Vec<GraphSeedEvidence>,
    pub final_rank: f64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SearchSignals {
    pub title_match: f64,
    pub path_match: f64,
    pub generic_marker: f64,
    pub graph_match: f64,
    pub graph_hub_penalty: f64,
    pub manual_adjustment: f64,
    pub feedback_adjustment: f64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SearchContributions {
    pub title: f64,
    pub path: f64,
    pub generic: f64,
    pub graph: f64,
    pub manual: f64,
    pub feedback: f64,
}

impl SearchContributions {
    fn total(&self) -> f64 {
        self.title + self.path + self.generic + self.graph + self.manual + self.feedback
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphSeedEvidence {
    pub slug: String,
    pub raw_score: f64,
    pub contribution: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SpanRecord {
    pub identifier: String,
    #[serde(rename = "type")]
    pub span_type: String,
    pub document: SearchDocumentRef,
    pub parent_identifier: String,
    pub ordinal: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub content_fingerprint: String,
    pub segmenter_version: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SpanGetResponse {
    pub scope: String,
    pub database: String,
    pub span: SpanRecord,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SpanExpandResponse {
    pub scope: String,
    pub database: String,
    pub span: SpanRecord,
    pub parent: Option<SpanRecord>,
    pub siblings: Vec<SpanRecord>,
    pub children: Vec<SpanRecord>,
    pub children_truncated: bool,
}

#[derive(Debug, Clone)]
struct StoredGraphEdge {
    edge_id: String,
    edge_type: String,
    from: String,
    to: String,
    weight: Option<f64>,
    confidence: Option<f64>,
    provenance: Option<String>,
    reason: Option<String>,
    frequency: Option<usize>,
    positions: Option<Vec<Range<usize>>>,
    first_position: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Auto,
    Page,
    Source,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchGranularity {
    Document,
    Passage,
    Sentence,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchGrouping {
    None,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub mode: SearchMode,
    pub granularity: SearchGranularity,
    pub grouping: SearchGrouping,
    pub kinds: Vec<String>,
    pub explain: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalAdjustment {
    pub target_type: String,
    pub target_identifier: String,
    pub provenance: String,
    pub weight: i32,
    pub reason: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalWeightResponse {
    pub scope: String,
    pub database: String,
    pub adjustment: RetrievalAdjustment,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalWeightListResponse {
    pub scope: String,
    pub database: String,
    pub adjustments: Vec<RetrievalAdjustment>,
    pub effective: Option<RetrievalAdjustment>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalClearResponse {
    pub scope: String,
    pub database: String,
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalFeedbackResponse {
    pub scope: String,
    pub database: String,
    pub query_fingerprint: String,
    pub target_type: String,
    pub target_identifier: String,
    pub provenance: String,
    pub signal: String,
    pub reason: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OperationRecord {
    pub id: i64,
    pub action: String,
    pub target: String,
    pub detail: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StoreIdentity {
    pub store_id: String,
    pub revision: String,
    pub operation_id: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangesetDraftState {
    pub id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub base_operation_id: i64,
    pub begin_operation_id: i64,
    pub draft_revision: String,
    pub draft_operation_id: i64,
    pub staged_operation_count: usize,
    pub action_counts: BTreeMap<String, usize>,
    pub operations: Vec<OperationRecord>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChangesetPublishInput {
    pub id: String,
    pub name: String,
    pub store_id: String,
    pub base_revision: String,
    pub draft_revision: String,
    pub draft_operation_id: i64,
    pub staged_operation_count: usize,
    pub checkpoint: String,
    pub lint_issues: usize,
    pub lint_override_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangesetCommitState {
    pub changeset_id: String,
    pub name: String,
    pub base_revision: String,
    pub post_revision: String,
    pub checkpoint: String,
    pub staged_operation_count: usize,
    pub lint_issues: usize,
    pub locked_publish_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesetHistoryState {
    pub id: String,
    pub name: String,
    pub status: String,
    pub base_revision: String,
    pub base_operation_id: i64,
    pub begin_operation_id: i64,
    pub pre_commit_checkpoint: Option<String>,
    pub post_revision: Option<String>,
    pub created_at: String,
    pub committed_at: Option<String>,
    pub rolled_back_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangesetRollbackInput {
    pub history: ChangesetHistoryState,
    pub store_id: String,
    pub pre_rollback_checkpoint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChangesetRollbackState {
    pub changeset_id: String,
    pub name: String,
    pub rollback_revision: String,
    pub checkpoint: String,
    pub locked_rollback_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparsePageSnapshot {
    slug: String,
    title: String,
    kind: Option<String>,
    summary: Option<String>,
    body: String,
    structural_navigation: bool,
    source_ids: Vec<i64>,
    provenance: Vec<String>,
    links: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparsePageInverse {
    slug: String,
    before: Option<SparsePageSnapshot>,
    after_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseMetaInverse {
    key: String,
    before: String,
    after_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseIngestSnapshot {
    status: String,
    attempts: i64,
    analysis: Option<String>,
    last_error: Option<String>,
    no_derived_pages_reason: Option<String>,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseSourcePathSnapshot {
    tracked_path: String,
    revision: i64,
    observed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseSourceSnapshot {
    id: i64,
    content_hash: String,
    title: Option<String>,
    origin: String,
    content: String,
    structural_navigation: bool,
    created_at: String,
    ingest: SparseIngestSnapshot,
    paths: Vec<SparseSourcePathSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseSourceInverse {
    source_id: i64,
    before: Option<SparseSourceSnapshot>,
    after_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseInversePayload {
    version: u32,
    changeset_id: String,
    store_id: String,
    pages: Vec<SparsePageInverse>,
    #[serde(default)]
    meta: Vec<SparseMetaInverse>,
    #[serde(default)]
    sources: Vec<SparseSourceInverse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseInverseEnvelope {
    payload: SparseInversePayload,
    checksum: String,
}

#[derive(Debug)]
struct PreparedDocumentGraph {
    graph: DocumentGraphReplacement,
    cooccurrence: CooccurrenceBuild,
    encoded_contributions: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextStore {
    pub scope: String,
    pub database: String,
    pub schema: Option<String>,
    pub purpose: Option<String>,
    pub pages: Vec<PageSummary>,
    pub recent_operations: Vec<OperationRecord>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestJobSummary {
    pub source_id: i64,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub no_derived_pages_reason: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestWork {
    pub source: SourceRecord,
    pub source_window: SourceWindow,
    pub status: String,
    pub attempts: i64,
    pub analysis: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestPacket {
    pub scope: String,
    pub database: String,
    pub job: Option<IngestWork>,
    pub schema: Option<String>,
    pub purpose: Option<String>,
    pub pages: Vec<PageSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestListResponse {
    pub scope: String,
    pub database: String,
    pub jobs: Vec<IngestJobSummary>,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestMutationResponse {
    pub scope: String,
    pub database: String,
    pub job: IngestJobSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<IngestIntegration>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IngestIntegration {
    pub source_summary_pages: usize,
    pub derived_pages: usize,
    pub no_derived_pages_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RelatedPageResponse {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub direct_links: usize,
    pub shared_sources: usize,
    pub adamic_adar: f64,
    pub type_affinity: f64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphRelatedResponse {
    pub scope: String,
    pub database: String,
    pub seed: String,
    pub related: Vec<RelatedPageResponse>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReindexResponse {
    pub scope: String,
    pub database: String,
    pub sources: usize,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MaterializeResponse {
    pub scope: String,
    pub database: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CompactResponse {
    pub scope: String,
    pub database: String,
    pub busy: bool,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckpointRecord {
    pub name: String,
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckpointResponse {
    pub scope: String,
    pub database: String,
    pub checkpoint: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety_checkpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckpointListResponse {
    pub scope: String,
    pub database: String,
    pub checkpoints: Vec<CheckpointRecord>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LintIssue {
    pub code: String,
    pub page: Option<String>,
    pub target: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LintResponse {
    pub scope: String,
    pub database: String,
    pub issues: Vec<LintIssue>,
    pub counts: BTreeMap<String, usize>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LogResponse {
    pub scope: String,
    pub database: String,
    pub operations: Vec<OperationRecord>,
}

impl Store {
    pub fn initialize(
        scope: impl Into<String>,
        database: impl AsRef<Path>,
    ) -> Result<(Self, bool)> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        configure_connection(&conn)?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        let created = prepare_store(&mut store.conn, true, None)?;
        if created {
            store.record_top_level_operation(
                "init",
                "wiki",
                json!({ "user_version": USER_VERSION, "tokenizer": TOKENIZER_ID }),
            )?;
        }
        store.reconcile_graph_projection()?;
        Ok((store, created))
    }

    pub fn open(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let mut conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection(&conn)?;
        prepare_store(&mut conn, false, None)?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        store.reconcile_graph_projection()?;
        Ok(store)
    }

    pub fn open_with_migration_progress(
        scope: impl Into<String>,
        database: impl AsRef<Path>,
        progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
    ) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let mut conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        configure_connection(&conn)?;
        prepare_store(&mut conn, false, Some(&mut *progress))?;

        let mut store = Self {
            scope,
            database,
            conn,
        };
        progress(1, 1, "projecting")?;
        store.reconcile_graph_projection()?;
        Ok(store)
    }

    pub fn open_read_only(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        if !database.is_file() {
            return Err(AppError::new(
                "store_not_found",
                format!("wiki database not found: {}", database.display()),
            ));
        }

        let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure_read_only_connection(&conn)?;
        prepare_store_read_only(&conn)?;

        Ok(Self {
            scope,
            database,
            conn,
        })
    }

    pub fn open_for_read(scope: impl Into<String>, database: impl AsRef<Path>) -> Result<Self> {
        let scope = scope.into();
        let database = database.as_ref().to_path_buf();
        match Self::open_read_only(scope.clone(), &database) {
            Ok(store) => Ok(store),
            Err(error) if error.code == "unsupported_store_version" => Self::open(scope, database),
            Err(error) => Err(error),
        }
    }

    pub fn identity(&self) -> Result<StoreIdentity> {
        store_identity(&self.conn)
    }

    pub fn validate_changeset_integrity(&self) -> Result<()> {
        validate_database_integrity(&self.conn)
    }

    pub fn reconcile_graph_projection(&mut self) -> Result<()> {
        let effective = config::resolve(&self.scope, &self.database)?;
        let changeset = self
            .database
            .components()
            .any(|component| component.as_os_str() == "changesets");
        if changeset
            || effective.physical == PhysicalSetting::Disabled
            || effective.resolved_engine == EngineSetting::Rslg
        {
            return self.reconcile_graph_projection_now();
        }
        let generation: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        let fresh: bool = self.conn.query_row(
            "SELECT engine = 'graphlite' AND status = 'fresh'
                    AND canonical_generation = ?1 AND projected_generation = ?1
             FROM graph_projection_state WHERE projection = 'physical'",
            params![generation],
            |row| row.get(0),
        )?;
        if fresh {
            return Ok(());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            &format!(
                "UPDATE graph_projection_state
                 SET engine = 'graphlite', canonical_generation = ?1,
                     status = 'pending', last_error_code = NULL,
                     last_error_message = NULL, updated_at = {TIMESTAMP_SQL}
                 WHERE projection = 'physical'"
            ),
            params![generation],
        )?;
        tx.commit()?;
        match crate::work::start_graph_projection(&self.scope, &self.database) {
            Ok(_) => Ok(()),
            Err(error) if error.code == "work_busy" => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn reconcile_graph_projection_now(&mut self) -> Result<()> {
        let mut effective = config::resolve(&self.scope, &self.database)?;
        if self
            .database
            .components()
            .any(|component| component.as_os_str() == "changesets")
        {
            effective.physical = PhysicalSetting::Enabled;
            effective.resolved_engine = EngineSetting::Rslg;
        }
        let observed_generation: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        let (observed_engine, observed_status, observed_canonical, observed_projected): (
            String,
            String,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT engine, status, canonical_generation, projected_generation
             FROM graph_projection_state WHERE projection = 'physical'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let already_resolved = if effective.physical == PhysicalSetting::Disabled {
            observed_status == "disabled"
                && observed_canonical == observed_generation
                && observed_projected == observed_generation
        } else if effective.resolved_engine == EngineSetting::Rslg {
            observed_engine == "rslg"
                && observed_status == "fresh"
                && observed_canonical == observed_generation
                && observed_projected == observed_generation
        } else {
            observed_engine == "graphlite"
                && observed_status == "fresh"
                && observed_canonical == observed_generation
                && observed_projected == observed_generation
        };
        if already_resolved {
            return Ok(());
        }
        if effective.physical == PhysicalSetting::Enabled
            && effective.resolved_engine == EngineSetting::Graphqlite
        {
            return self.project_graphqlite_until_current();
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation: i64 = tx.query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        if effective.physical == PhysicalSetting::Disabled {
            tx.execute(
                &format!(
                    "UPDATE graph_projection_state
                     SET engine = 'rslg', canonical_generation = ?1,
                         projected_generation = ?1, status = 'disabled',
                         last_error_code = NULL, last_error_message = NULL,
                         updated_at = {TIMESTAMP_SQL}
                     WHERE projection = 'physical'"
                ),
                params![generation],
            )?;
            tx.commit()?;
            return Ok(());
        }
        if effective.resolved_engine == EngineSetting::Rslg {
            tx.execute(
                &format!(
                    "UPDATE graph_projection_state
                     SET engine = 'rslg', canonical_generation = ?1,
                         projected_generation = ?1, status = 'fresh',
                         last_error_code = NULL, last_error_message = NULL,
                         updated_at = {TIMESTAMP_SQL}
                     WHERE projection = 'physical'"
                ),
                params![generation],
            )?;
            tx.commit()?;
            return Ok(());
        }
        unreachable!("physical projection engine was resolved before opening the write transaction")
    }

    fn project_graphqlite_until_current(&mut self) -> Result<()> {
        loop {
            let (generation, digest, projection) = {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Deferred)?;
                let generation: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
                    [],
                    |row| row.get(0),
                )?;
                let digest = if generation == 0 {
                    current_graph_digest(&tx)?
                } else {
                    tx.query_row(
                        "SELECT canonical_digest FROM graph_generations WHERE generation = ?1",
                        params![generation],
                        |row| row.get::<_, String>(0),
                    )?
                };
                let projection =
                    project_graphqlite_snapshot(&tx, &self.database, generation, &digest);
                tx.commit()?;
                (generation, digest, projection)
            };
            if let Err(error) = projection {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute(
                    &format!(
                        "UPDATE graph_projection_state
                         SET engine = 'graphlite', canonical_generation = ?1,
                             status = 'stale', last_error_code = ?2,
                             last_error_message = 'GraphQLite projection failed',
                             updated_at = {TIMESTAMP_SQL}
                         WHERE projection = 'physical'"
                    ),
                    params![generation, error.code],
                )?;
                tx.commit()?;
                return Err(AppError::new(
                    "graph_projection_failed",
                    "GraphQLite projection work failed; canonical graph remains available",
                )
                .with_details(json!({
                    "canonical_committed": true,
                    "generation": generation,
                    "digest": digest,
                    "cause": error.code,
                })));
            }
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: i64 = tx.query_row(
                "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                &format!(
                    "UPDATE graph_projection_state
                     SET engine = 'graphlite', canonical_generation = ?1,
                         projected_generation = ?2,
                         status = CASE WHEN ?1 = ?2 THEN 'fresh' ELSE 'pending' END,
                         last_error_code = NULL, last_error_message = NULL,
                         updated_at = {TIMESTAMP_SQL}
                     WHERE projection = 'physical'"
                ),
                params![current, generation],
            )?;
            tx.commit()?;
            if current == generation {
                return Ok(());
            }
        }
    }

    #[cfg(test)]
    pub fn snapshot_to(&self, path: &Path) -> Result<()> {
        create_checkpoint(&self.conn, path)
    }

    pub fn changeset_begin(
        &mut self,
        name: &str,
        base: &StoreIdentity,
    ) -> Result<ChangesetDraftState> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let draft_identity = store_identity(&tx)?;
        if draft_identity.store_id != base.store_id || draft_identity.revision != base.revision {
            return Err(AppError::new(
                "changeset_changed",
                "draft snapshot does not match the captured live base",
            ));
        }
        let id: String = tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(32)))", [], |row| row.get(0))?;
        record_operation(
            &tx,
            "changeset_begin",
            &id,
            &json!({"name": name, "base_revision": base.revision}),
        )?;
        let begin_operation_id = tx.last_insert_rowid();
        tx.execute(
            &format!(
                "INSERT INTO changesets(
                    id, name, status, base_revision, base_operation_id,
                    begin_operation_id, created_at
                 ) VALUES (?1, ?2, 'draft', ?3, ?4, ?5, {TIMESTAMP_SQL})"
            ),
            params![
                &id,
                name,
                &base.revision,
                base.operation_id,
                begin_operation_id
            ],
        )?;
        tx.commit()?;
        self.changeset_draft(name, 50)
    }

    pub fn changeset_begin_sparse(
        &mut self,
        name: &str,
        base: &StoreIdentity,
        schema: &str,
        purpose: &str,
        max_source_id: i64,
    ) -> Result<ChangesetDraftState> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM operations", [])?;
        tx.execute("DELETE FROM changesets", [])?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'operations'",
            params![base.operation_id],
        )?;
        tx.execute(
            "INSERT INTO sqlite_sequence(name, seq)
             SELECT 'operations', ?1
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'operations')",
            params![base.operation_id],
        )?;
        tx.execute(
            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'sources'",
            params![max_source_id],
        )?;
        tx.execute(
            "INSERT INTO sqlite_sequence(name, seq)
             SELECT 'sources', ?1
             WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'sources')",
            params![max_source_id],
        )?;
        let schema_fingerprint = hash_content(schema);
        let purpose_fingerprint = hash_content(purpose);
        for (key, value) in [
            ("store_id", base.store_id.as_str()),
            ("store_revision", base.revision.as_str()),
            ("schema", schema),
            ("purpose", purpose),
            ("changeset_storage", "sparse-v1"),
            ("changeset_base_meta:schema", schema_fingerprint.as_str()),
            ("changeset_base_meta:purpose", purpose_fingerprint.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        tx.commit()?;
        self.changeset_begin(name, base)
    }

    pub fn max_source_id(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM sources", [], |row| {
                row.get(0)
            })?)
    }

    pub fn changeset_storage_kind(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'changeset_storage'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn changeset_touched_pages(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT o.target,
                    COALESCE(m.value, 'absent')
             FROM operations o
             JOIN changesets c ON o.id > c.begin_operation_id AND c.status = 'draft'
             LEFT JOIN meta m ON m.key = 'changeset_base_page:' || o.target
             WHERE o.action IN ('page_put', 'page_remove')
             ORDER BY o.target",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn changeset_touched_meta(&self) -> Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare(
            "SELECT touched.key, m.value
             FROM (
                SELECT DISTINCT CASE o.action
                    WHEN 'schema_set' THEN 'schema'
                    WHEN 'purpose_set' THEN 'purpose'
                END AS key
                FROM operations o
                JOIN changesets c ON o.id > c.begin_operation_id AND c.status = 'draft'
                WHERE o.action IN ('schema_set', 'purpose_set')
             ) touched
             JOIN meta m ON m.key = 'changeset_base_meta:' || touched.key
             ORDER BY touched.key",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn meta_fingerprint(&self, key: &str) -> Result<String> {
        let value: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )?;
        Ok(hash_content(&value))
    }

    pub fn page_mutation_fingerprint(&self, slug: &str) -> Result<Option<String>> {
        Ok(load_page_mutation_base(&self.conn, slug)?.map(|value| value.content_fingerprint))
    }

    pub fn changeset_prepare_page_touch(
        &mut self,
        live_path: &Path,
        slug: &str,
        additional_source_ids: &[i64],
    ) -> Result<()> {
        let schema = "live_base";
        self.conn.execute(
            "ATTACH DATABASE ?1 AS live_base",
            params![live_path.to_string_lossy().as_ref()],
        )?;
        let result = (|| -> Result<()> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let base_key = format!("changeset_base_page:{slug}");
            let prepared = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![&base_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if prepared.is_none() {
                tx.execute(
                    "INSERT OR IGNORE INTO pages(
                        slug, title, kind, summary, body, structural_navigation,
                        created_at, updated_at
                     ) SELECT slug, title, kind, summary, body, structural_navigation,
                              created_at, updated_at
                       FROM live_base.pages WHERE slug = ?1",
                    params![slug],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO links(from_slug, to_slug)
                     SELECT from_slug, to_slug FROM live_base.links WHERE from_slug = ?1",
                    params![slug],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO page_provenance(page_slug, provenance)
                     SELECT page_slug, provenance FROM live_base.page_provenance
                     WHERE page_slug = ?1",
                    params![slug],
                )?;
            }
            let mut source_ids = additional_source_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            if prepared.is_none() {
                let mut statement = tx
                    .prepare("SELECT source_id FROM live_base.page_sources WHERE page_slug = ?1")?;
                for row in statement.query_map(params![slug], |row| row.get::<_, i64>(0))? {
                    source_ids.insert(row?);
                }
            }
            let mut stub_records = BTreeMap::new();
            for source_id in source_ids {
                tx.execute(
                    &format!(
                        "INSERT OR IGNORE INTO sources(
                            id, content_hash, title, origin, content,
                            structural_navigation, created_at
                         ) SELECT id, content_hash, title, origin, '',
                                  structural_navigation, created_at
                           FROM {schema}.sources WHERE id = ?1"
                    ),
                    params![source_id],
                )?;
                if prepared.is_none() {
                    tx.execute(
                        "INSERT OR IGNORE INTO page_sources(page_slug, source_id)
                         SELECT page_slug, source_id FROM live_base.page_sources
                         WHERE page_slug = ?1 AND source_id = ?2",
                        params![slug, source_id],
                    )?;
                }
                let node_id = format!("source:{source_id}");
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO graph_nodes(
                        node_id, node_type, label, properties_json
                     ) SELECT ?1, 'document', COALESCE(title, origin), '{}'
                       FROM live_base.sources WHERE id = ?2",
                    params![&node_id, source_id],
                )?;
                if inserted == 1
                    && let Some(record) = canonical_graph_record(&tx, "node", &node_id)?
                {
                    stub_records.insert(("node".into(), node_id), record);
                }
            }
            if !stub_records.is_empty() {
                apply_graph_digest_patch(&tx, &BTreeMap::new(), &stub_records)?;
            }
            if prepared.is_none() {
                let base = load_page_mutation_base(&tx, slug)?
                    .map(|value| value.content_fingerprint)
                    .unwrap_or_else(|| "absent".into());
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                    params![base_key, base],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        let _ = self.conn.execute("DETACH DATABASE live_base", []);
        result
    }

    pub fn changeset_draft(&self, name: &str, limit: usize) -> Result<ChangesetDraftState> {
        let row = self
            .conn
            .query_row(
                "SELECT id, name, status, base_revision, base_operation_id,
                        begin_operation_id, created_at
                 FROM changesets
                 WHERE name = ?1 AND status = 'draft'
                 ORDER BY created_at DESC
                 LIMIT 1",
                params![name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new(
                    "changeset_not_found",
                    format!("draft changeset not found: {name}"),
                )
            })?;
        let identity = self.identity()?;
        let staged_operation_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE id > ?1",
            params![row.5],
            |row| row.get(0),
        )?;
        let mut action_counts = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT action, COUNT(*)
                 FROM operations
                 WHERE id > ?1
                 GROUP BY action
                 ORDER BY action",
            )?;
            let rows = statement.query_map(params![row.5], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for value in rows {
                let (action, count) = value?;
                action_counts.insert(action, usize::try_from(count).unwrap_or(usize::MAX));
            }
        }
        let operations = {
            let mut statement = self.conn.prepare(
                "SELECT id, action, target, detail_json, created_at
                 FROM operations
                 WHERE id > ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )?;
            statement
                .query_map(params![row.5, limit as i64], read_operation_record)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(ChangesetDraftState {
            id: row.0,
            name: row.1,
            status: row.2,
            base_revision: row.3,
            base_operation_id: row.4,
            begin_operation_id: row.5,
            draft_revision: identity.revision,
            draft_operation_id: identity.operation_id,
            staged_operation_count: usize::try_from(staged_operation_count).map_err(|_| {
                AppError::new(
                    "database_error",
                    "changeset operation count is out of range",
                )
            })?,
            action_counts,
            operations,
            created_at: row.6,
        })
    }

    pub fn changeset_checkpoint_create(&self, changeset_id: &str) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let prefix = format!("pre-changeset-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        create_checkpoint(&self.conn, &path)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_sparse_checkpoint_create(
        &self,
        changeset_id: &str,
        draft_path: &Path,
    ) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let draft = Store::open_for_read(self.scope.clone(), draft_path)?;
        let identity = self.identity()?;
        let mut pages = Vec::new();
        for (slug, expected) in draft.changeset_touched_pages()? {
            let before = load_sparse_page_snapshot(&self.conn, &slug)?;
            let observed = before
                .as_ref()
                .map(sparse_page_fingerprint)
                .unwrap_or_else(|| "absent".into());
            if observed != expected {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("page {slug} changed while preparing its inverse patch"),
                ));
            }
            let after_fingerprint = draft
                .page_mutation_fingerprint(&slug)?
                .unwrap_or_else(|| "absent".into());
            pages.push(SparsePageInverse {
                slug,
                before,
                after_fingerprint,
            });
        }
        let mut meta = Vec::new();
        for (key, _) in draft.changeset_touched_meta()? {
            let before: String = self.conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![&key],
                |row| row.get(0),
            )?;
            let after: String = draft.conn.query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![&key],
                |row| row.get(0),
            )?;
            meta.push(SparseMetaInverse {
                key,
                before,
                after_fingerprint: hash_content(&after),
            });
        }
        let mut sources = Vec::new();
        for source_id in changeset_created_source_ids(&draft.conn)? {
            let before = load_sparse_source_snapshot(&self.conn, source_id)?;
            if before.is_some() {
                return Err(AppError::new(
                    "changeset_conflict",
                    format!("source identifier {source_id} was allocated by another write"),
                )
                .with_details(json!({"entity_type": "source", "identifier": source_id})));
            }
            let after = load_sparse_source_snapshot(&draft.conn, source_id)?.ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    format!("staged source {source_id} is missing"),
                )
            })?;
            sources.push(SparseSourceInverse {
                source_id,
                before,
                after_fingerprint: sparse_source_fingerprint(&after),
            });
        }
        let payload = SparseInversePayload {
            version: 1,
            changeset_id: changeset_id.to_string(),
            store_id: identity.store_id,
            pages,
            meta,
            sources,
        };
        let encoded = serde_json::to_vec(&payload)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let envelope = SparseInverseEnvelope {
            checksum: hash_content(
                std::str::from_utf8(&encoded)
                    .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?,
            ),
            payload,
        };
        let prefix = format!("pre-changeset-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        write_sparse_inverse(&path, &envelope)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_freeze(
        &mut self,
        id: &str,
        expected_revision: &str,
        expected_operation_id: i64,
        expected_operation_count: usize,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let identity = store_identity(&tx)?;
        let begin_operation_id: i64 = tx
            .query_row(
                "SELECT begin_operation_id FROM changesets
                 WHERE id = ?1 AND status = 'draft'",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| {
                AppError::new("changeset_changed", "draft changeset is no longer writable")
            })?;
        let operation_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM operations WHERE id > ?1",
            params![begin_operation_id],
            |row| row.get(0),
        )?;
        if identity.revision != expected_revision
            || identity.operation_id != expected_operation_id
            || usize::try_from(operation_count).ok() != Some(expected_operation_count)
        {
            return Err(AppError::new(
                "changeset_changed",
                "draft changeset changed during commit preflight",
            ));
        }
        let frozen: Option<String> = tx
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![CHANGESET_FREEZE_KEY],
                |row| row.get(0),
            )
            .optional()?;
        match frozen.as_deref() {
            Some(value) if value != id => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "draft freeze marker belongs to another changeset",
                ));
            }
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO meta(key, value) VALUES (?1, ?2)",
                    params![CHANGESET_FREEZE_KEY, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn changeset_publish(
        &mut self,
        draft_path: &Path,
        input: &ChangesetPublishInput,
    ) -> Result<ChangesetCommitState> {
        self.conn.execute(
            "ATTACH DATABASE ?1 AS candidate",
            params![draft_path.to_string_lossy().as_ref()],
        )?;
        let result = publish_attached_changeset(&mut self.conn, input);
        let _ = self.conn.execute("DETACH DATABASE candidate", []);
        result
    }

    pub fn changeset_committed_by_id(&self, id: &str) -> Result<Option<ChangesetCommitState>> {
        load_committed_changeset(&self.conn, id)
    }

    pub fn changeset_history_by_id(&self, id: &str) -> Result<Option<ChangesetHistoryState>> {
        self.conn
            .query_row(
                "SELECT id, name, status, base_revision, base_operation_id,
                        begin_operation_id, pre_commit_checkpoint, post_revision,
                        created_at, committed_at, rolled_back_at
                 FROM changesets WHERE id = ?1",
                params![id],
                read_changeset_history,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn changeset_rollback_checkpoint_create(
        &self,
        changeset_id: &str,
    ) -> Result<CheckpointResponse> {
        if changeset_id.len() != 64 || !changeset_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "changeset_corrupt",
                "changeset id is not a 64-character hexadecimal value",
            ));
        }
        let prefix = format!("pre-rollback-{}", &changeset_id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        create_checkpoint(&self.conn, &path)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_sparse_rollback_checkpoint_create(
        &self,
        history: &ChangesetHistoryState,
    ) -> Result<CheckpointResponse> {
        let checkpoint = history.pre_commit_checkpoint.as_deref().ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "committed changeset has no pre-commit checkpoint",
            )
        })?;
        let inverse = load_sparse_inverse(&checkpoint_path(&self.database, checkpoint)?)?;
        let identity = self.identity()?;
        let pages = inverse
            .payload
            .pages
            .iter()
            .map(|page| {
                let before = load_sparse_page_snapshot(&self.conn, &page.slug)?;
                Ok(SparsePageInverse {
                    slug: page.slug.clone(),
                    before,
                    after_fingerprint: page
                        .before
                        .as_ref()
                        .map(sparse_page_fingerprint)
                        .unwrap_or_else(|| "absent".into()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let meta = inverse
            .payload
            .meta
            .iter()
            .map(|entry| {
                let current: String = self.conn.query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![&entry.key],
                    |row| row.get(0),
                )?;
                Ok(SparseMetaInverse {
                    key: entry.key.clone(),
                    before: current,
                    after_fingerprint: hash_content(&entry.before),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let sources = inverse
            .payload
            .sources
            .iter()
            .map(|entry| {
                let before = load_sparse_source_snapshot(&self.conn, entry.source_id)?;
                Ok(SparseSourceInverse {
                    source_id: entry.source_id,
                    before,
                    after_fingerprint: entry
                        .before
                        .as_ref()
                        .map(sparse_source_fingerprint)
                        .unwrap_or_else(|| "absent".into()),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let payload = SparseInversePayload {
            version: 1,
            changeset_id: history.id.clone(),
            store_id: identity.store_id,
            pages,
            meta,
            sources,
        };
        let encoded = serde_json::to_vec(&payload)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let envelope = SparseInverseEnvelope {
            checksum: hash_content(
                std::str::from_utf8(&encoded)
                    .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?,
            ),
            payload,
        };
        let prefix = format!("pre-rollback-{}", &history.id[..12]);
        let checkpoint = fresh_checkpoint_name(&self.database, &prefix)?;
        let path = checkpoint_path(&self.database, &checkpoint)?;
        write_sparse_inverse(&path, &envelope)?;
        Ok(CheckpointResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            checkpoint,
            path: path.to_string_lossy().into_owned(),
            safety_checkpoint: None,
        })
    }

    pub fn changeset_rollback_checkpoint_validate(
        &self,
        history: &ChangesetHistoryState,
        store_id: &str,
    ) -> Result<bool> {
        let checkpoint = history.pre_commit_checkpoint.as_deref().ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "committed changeset has no pre-commit checkpoint",
            )
        })?;
        let path = checkpoint_path(&self.database, checkpoint)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            AppError::new(
                "changeset_corrupt",
                format!("pre-commit checkpoint is unavailable: {error}"),
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::new(
                "changeset_corrupt",
                "pre-commit checkpoint is not a regular file",
            ));
        }
        if let Ok(envelope) = load_sparse_inverse(&path) {
            if envelope.payload.store_id != store_id || envelope.payload.changeset_id != history.id
            {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "sparse inverse patch belongs to another Wiki or changeset",
                ));
            }
            return Ok(true);
        }
        let checkpoint_store = Store::open_read_only(self.scope.clone(), &path)
            .map_err(|error| AppError::new("changeset_corrupt", error.message))?;
        if checkpoint_store.identity()?.store_id != store_id {
            return Err(AppError::new(
                "changeset_corrupt",
                "pre-commit checkpoint belongs to another Wiki",
            ));
        }
        Ok(false)
    }

    pub fn changeset_rollback(
        &mut self,
        input: &ChangesetRollbackInput,
    ) -> Result<ChangesetRollbackState> {
        let checkpoint = input
            .history
            .pre_commit_checkpoint
            .as_deref()
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "committed changeset has no pre-commit checkpoint",
                )
            })?;
        let path = checkpoint_path(&self.database, checkpoint)?;
        let sparse =
            self.changeset_rollback_checkpoint_validate(&input.history, &input.store_id)?;

        if sparse {
            return rollback_sparse_changeset(&mut self.conn, &path, input);
        }

        self.conn.execute(
            "ATTACH DATABASE ?1 AS candidate",
            params![path.to_string_lossy().as_ref()],
        )?;
        let result = rollback_attached_changeset(&mut self.conn, input);
        let _ = self.conn.execute("DETACH DATABASE candidate", []);
        result
    }

    pub fn changeset_rollback_state_by_id(
        &self,
        id: &str,
    ) -> Result<Option<ChangesetRollbackState>> {
        let row = self
            .conn
            .query_row(
                "SELECT c.name, o.detail_json
                 FROM changesets c
                 JOIN operations o
                   ON o.action = 'changeset_rollback' AND o.target = c.id
                 WHERE c.id = ?1 AND c.status = 'rolled_back'
                 ORDER BY o.id DESC LIMIT 1",
                params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((name, detail)) = row else {
            return Ok(None);
        };
        let detail: Value = serde_json::from_str(&detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        let checkpoint = detail
            .get("pre_rollback_checkpoint")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rollback operation lacks pre_rollback_checkpoint",
                )
            })?;
        let rollback_revision = detail
            .get("rollback_revision")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::new(
                    "changeset_corrupt",
                    "rollback operation lacks rollback_revision",
                )
            })?;
        Ok(Some(ChangesetRollbackState {
            changeset_id: id.to_string(),
            name,
            rollback_revision: rollback_revision.to_string(),
            checkpoint: checkpoint.to_string(),
            locked_rollback_ms: 0,
        }))
    }

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
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = Vec::new();
        for input in inputs {
            inserted.push(insert_source(&tx, &input?)?);
        }
        tx.commit()?;
        let canonical_duration_ms = elapsed_millis(mutation_started);
        let projection_started = Instant::now();
        self.reconcile_graph_projection()?;
        let projection_duration_ms = elapsed_millis(projection_started);

        inserted
            .into_iter()
            .map(|(source_id, created)| {
                Ok(SourceAddResponse {
                    scope: self.scope.clone(),
                    database: self.database_string(),
                    source: self.load_source_summary(source_id)?,
                    created,
                    graph: self.load_graph_mutation_summary(
                        "source",
                        &source_id.to_string(),
                        canonical_duration_ms,
                        projection_duration_ms,
                    )?,
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
            }),
        )?;
        remove_document_graph(&tx, "source", &id.to_string())?;
        tx.commit()?;
        self.reconcile_graph_projection()?;
        Ok(SourceRemoveResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            source_id: id,
            removed: true,
            removed_path_revisions,
            untracked_paths,
        })
    }

    pub fn page_put(&mut self, input: PagePutInput) -> Result<PagePutResponse> {
        let mutation_started = Instant::now();
        validate_page_slug(&input.slug)?;
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
        let prepared_graph = prepare_document_graph(&DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: &input.slug,
            label: &input.title,
            content: &input.body,
        })?;
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

        record_operation(
            &tx,
            "page_put",
            &input.slug,
            &json!({ "created": !existed }),
        )?;
        let graph_generation = persist_prepared_document_graph(
            &tx,
            &DocumentGraphInput {
                document_type: DocumentType::Page,
                identifier: &input.slug,
                label: &input.title,
                content: &input.body,
            },
            prepared_graph,
            &page_relation_edges(&tx, &input.slug, &source_ids, &links)?,
            true,
        )?;
        persist_inbound_link_edges(&tx, &input.slug, graph_generation)?;
        tx.commit()?;
        let canonical_duration_ms = elapsed_millis(mutation_started);
        let projection_started = Instant::now();
        self.reconcile_graph_projection()?;
        let projection_duration_ms = elapsed_millis(projection_started);

        Ok(PagePutResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            page: self.load_page_write(&input.slug)?,
            created: !existed,
            graph: self.load_graph_mutation_summary(
                "page",
                &input.slug,
                canonical_duration_ms,
                projection_duration_ms,
            )?,
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
        remove_document_graph(&tx, "page", slug)?;
        tx.execute("DELETE FROM pages WHERE slug = ?1", params![slug])?;
        tx.commit()?;
        self.reconcile_graph_projection()?;
        Ok(PageRemoveResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            slug: slug.to_string(),
            removed: true,
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
                "SELECT node_type FROM graph_nodes WHERE node_id = ?1",
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
            "SELECT node_id FROM graph_nodes
             WHERE parent_node_id = ?1 AND node_type = ?2
               AND ordinal BETWEEN ?3 AND ?4
             ORDER BY ordinal, node_id",
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
            "SELECT node_id FROM graph_nodes
             WHERE parent_node_id = ?1 AND node_type IN ('passage', 'sentence')
             ORDER BY ordinal, node_id LIMIT ?2",
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

    pub fn graph_explore(
        &self,
        identifier: &str,
        depth: usize,
        limit: usize,
        direction: &str,
        edge_types: &[String],
    ) -> Result<Value> {
        graph_explore_value(
            &self.conn,
            &self.scope,
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
        graph_macro_explore_value(&self.conn, &self.scope, depth, limit, edge_types)
    }

    pub fn graph_node(&self, identifier: &str) -> Result<Value> {
        let identifier = resolve_graph_node(&self.conn, identifier)?;
        let mut node = graph_node_json(&self.conn, &identifier)?;
        node["outgoing_degree"] = json!(self.conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE from_node_id = ?1",
            params![&identifier],
            |row| row.get::<_, i64>(0),
        )?);
        node["incoming_degree"] = json!(self.conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE to_node_id = ?1",
            params![&identifier],
            |row| row.get::<_, i64>(0),
        )?);
        Ok(json!({"scope": self.scope, "node": node}))
    }

    pub fn graph_neighbors(
        &self,
        identifier: &str,
        limit: usize,
        direction: &str,
        edge_types: &[String],
    ) -> Result<Value> {
        let mut response = graph_explore_value(
            &self.conn,
            &self.scope,
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
        graph_path_value(
            &self.conn,
            &self.scope,
            from,
            to,
            max_depth,
            limit,
            direction,
            edge_types,
        )
    }

    pub fn graph_impact(&self, identifier: &str, max_depth: usize, limit: usize) -> Result<Value> {
        graph_impact_value(&self.conn, &self.scope, identifier, max_depth, limit)
    }

    pub fn graph_overview(&self, limit: usize) -> Result<Value> {
        graph_overview_value(&self.conn, &self.scope, limit)
    }

    pub fn graph_status(&self) -> Result<Value> {
        graph_status_value(&self.conn, &self.scope, &self.database)
    }

    pub fn graph_verify(&self) -> Result<Value> {
        graph_verify_value(&self.conn, &self.scope, &self.database)
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
        let response = graph_relation_set_value(
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
        self.reconcile_graph_projection()?;
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
        let response = graph_relation_retract_value(
            &mut self.conn,
            &self.scope,
            from,
            relation_type,
            to,
            reason,
        )?;
        self.reconcile_graph_projection()?;
        Ok(response)
    }

    fn load_graph_mutation_summary(
        &self,
        document_type: &str,
        document_identifier: &str,
        canonical_duration_ms: u64,
        projection_duration_ms: u64,
    ) -> Result<GraphMutationSummary> {
        let (generation, truncated): (i64, i64) = self.conn.query_row(
            "SELECT generation, cooccurrence_truncated
             FROM document_index_state
             WHERE document_type = ?1 AND document_identifier = ?2",
            params![document_type, document_identifier],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let count_nodes = |node_type: &str| -> Result<usize> {
            let count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM graph_nodes
                 WHERE document_type = ?1 AND document_identifier = ?2
                   AND node_type = ?3",
                params![document_type, document_identifier, node_type],
                |row| row.get(0),
            )?;
            usize::try_from(count)
                .map_err(|_| AppError::new("database_error", "graph node count is invalid"))
        };
        let terms: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM graph_occurrences
             WHERE document_type = ?1 AND document_identifier = ?2",
            params![document_type, document_identifier],
            |row| row.get(0),
        )?;
        let invalidated_semantic_relations: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM graph_deltas
             WHERE generation = ?1 AND action = 'remove' AND entity_type = 'edge'
               AND json_extract(before_json, '$.owner_type') = 'manual'",
            params![generation],
            |row| row.get(0),
        )?;
        let (projection_engine, projection_status) = self.conn.query_row(
            "SELECT engine, status FROM graph_projection_state WHERE projection = 'physical'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(GraphMutationSummary {
            generation,
            passages: count_nodes("passage")?,
            sentences: count_nodes("sentence")?,
            terms: usize::try_from(terms)
                .map_err(|_| AppError::new("database_error", "graph term count is invalid"))?,
            cooccurrence_truncated: usize::try_from(truncated).map_err(|_| {
                AppError::new(
                    "database_error",
                    "co-occurrence truncation count is invalid",
                )
            })?,
            invalidated_semantic_relations: usize::try_from(invalidated_semantic_relations)
                .map_err(|_| {
                    AppError::new("database_error", "semantic invalidation count is invalid")
                })?,
            projection_engine,
            projection_status,
            canonical_duration_ms,
            projection_duration_ms,
        })
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
        configure_read_only_connection(&source)?;
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
        all_issues.extend(graph_lint_issues(&self.conn)?);
        let verification = graph_verify_value(&self.conn, &self.scope, &self.database)?;
        if let Some(verification_issues) = verification["issues"].as_array() {
            for issue in verification_issues {
                let Some(code) = issue["code"].as_str() else {
                    continue;
                };
                if all_issues.iter().any(|existing| existing.code == code) {
                    continue;
                }
                all_issues.push(LintIssue {
                    code: code.to_string(),
                    page: None,
                    target: issue
                        .get("count")
                        .or_else(|| issue.get("generation"))
                        .map(Value::to_string),
                    message: "graph verification invariant failed".to_string(),
                });
            }
        }
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

fn validate_checkpoint_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || name != name.trim()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "checkpoint_name_invalid",
            "checkpoint name must be one safe filename segment of at most 80 bytes",
        ));
    }
    Ok(())
}

fn checkpoint_directory(database: &Path) -> Result<PathBuf> {
    let store_directory = database
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?;
    let directory = store_directory.join("checkpoints");
    if let Ok(metadata) = fs::symlink_metadata(&directory)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(AppError::new(
            "checkpoint_path_invalid",
            format!(
                "checkpoint directory is not a regular directory: {}",
                directory.display()
            ),
        ));
    }
    Ok(directory)
}

fn checkpoint_path(database: &Path, name: &str) -> Result<PathBuf> {
    validate_checkpoint_name(name)?;
    Ok(checkpoint_directory(database)?.join(format!("{name}.db")))
}

fn create_checkpoint(source: &Connection, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("checkpoint_path_invalid", "checkpoint has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
                "checkpoint_exists"
            } else {
                "checkpoint_create_failed"
            };
            AppError::new(code, format!("cannot create {}: {error}", path.display()))
        })?;

    let result = (|| -> Result<()> {
        let mut destination = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        {
            let backup = Backup::new(source, &mut destination)?;
            backup.run_to_completion(100, Duration::from_millis(10), None)?;
        }
        prepare_store_read_only(&destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn write_sparse_inverse(path: &Path, envelope: &SparseInverseEnvelope) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("checkpoint_path_invalid", "checkpoint has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
            "checkpoint_exists"
        } else {
            "checkpoint_create_failed"
        };
        AppError::new(code, format!("cannot create {}: {error}", path.display()))
    })?;
    let encoded = serde_json::to_vec(envelope)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if let Err(error) = file.write_all(&encoded).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

fn load_sparse_inverse(path: &Path) -> Result<SparseInverseEnvelope> {
    let encoded = fs::read(path)?;
    let envelope: SparseInverseEnvelope = serde_json::from_slice(&encoded)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if envelope.payload.version != 1 {
        return Err(AppError::new(
            "changeset_corrupt",
            "sparse inverse patch version is unsupported",
        ));
    }
    let payload = serde_json::to_vec(&envelope.payload)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    let payload = std::str::from_utf8(&payload)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if hash_content(payload) != envelope.checksum {
        return Err(AppError::new(
            "changeset_corrupt",
            "sparse inverse patch checksum does not match",
        ));
    }
    Ok(envelope)
}

fn load_sparse_page_snapshot(conn: &Connection, slug: &str) -> Result<Option<SparsePageSnapshot>> {
    let page = conn
        .query_row(
            "SELECT slug, title, kind, summary, body, structural_navigation,
                    created_at, updated_at
             FROM pages WHERE slug = ?1",
            params![slug],
            |row| {
                Ok(SparsePageSnapshot {
                    slug: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    body: row.get(4)?,
                    structural_navigation: row.get(5)?,
                    source_ids: Vec::new(),
                    provenance: Vec::new(),
                    links: Vec::new(),
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()?;
    let Some(mut page) = page else {
        return Ok(None);
    };
    page.source_ids = {
        let mut statement = conn.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    page.provenance = {
        let mut statement = conn.prepare(
            "SELECT provenance FROM page_provenance WHERE page_slug = ?1 ORDER BY provenance",
        )?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    page.links = {
        let mut statement =
            conn.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(Some(page))
}

fn sparse_page_fingerprint(page: &SparsePageSnapshot) -> String {
    page_content_fingerprint(
        &page.title,
        page.kind.as_deref(),
        page.summary.as_deref(),
        &page.body,
        page.structural_navigation,
        &page.source_ids,
        &page.provenance,
        &page.links,
    )
}

fn changeset_created_source_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT o.detail_json
         FROM operations o
         JOIN changesets c ON o.id > c.begin_operation_id
         WHERE c.status = 'draft' AND o.action = 'source_add'
         ORDER BY o.id",
    )?;
    let mut ids = BTreeSet::new();
    for row in statement.query_map([], |row| row.get::<_, String>(0))? {
        let detail: Value = serde_json::from_str(&row?)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        if detail.get("created").and_then(Value::as_bool) == Some(true) {
            let id = detail
                .get("source_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    AppError::new("changeset_corrupt", "source_add operation lacks source_id")
                })?;
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().collect())
}

fn load_sparse_source_snapshot(
    conn: &Connection,
    source_id: i64,
) -> Result<Option<SparseSourceSnapshot>> {
    let source = conn
        .query_row(
            "SELECT s.id, s.content_hash, s.title, s.origin, s.content,
                    s.structural_navigation, s.created_at,
                    j.status, j.attempts, j.analysis, j.last_error,
                    j.no_derived_pages_reason, j.updated_at
             FROM sources s JOIN ingest_jobs j ON j.source_id = s.id
             WHERE s.id = ?1",
            params![source_id],
            |row| {
                Ok(SparseSourceSnapshot {
                    id: row.get(0)?,
                    content_hash: row.get(1)?,
                    title: row.get(2)?,
                    origin: row.get(3)?,
                    content: row.get(4)?,
                    structural_navigation: row.get(5)?,
                    created_at: row.get(6)?,
                    ingest: SparseIngestSnapshot {
                        status: row.get(7)?,
                        attempts: row.get(8)?,
                        analysis: row.get(9)?,
                        last_error: row.get(10)?,
                        no_derived_pages_reason: row.get(11)?,
                        updated_at: row.get(12)?,
                    },
                    paths: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut source) = source else {
        return Ok(None);
    };
    source.paths = {
        let mut statement = conn.prepare(
            "SELECT tracked_path, revision, observed_at
             FROM source_path_revisions WHERE source_id = ?1
             ORDER BY tracked_path, revision",
        )?;
        statement
            .query_map(params![source_id], |row| {
                Ok(SparseSourcePathSnapshot {
                    tracked_path: row.get(0)?,
                    revision: row.get(1)?,
                    observed_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(Some(source))
}

fn sparse_source_fingerprint(source: &SparseSourceSnapshot) -> String {
    hash_content(&serde_json::to_string(source).unwrap_or_default())
}

fn fresh_checkpoint_name(database: &Path, prefix: &str) -> Result<String> {
    validate_checkpoint_name(prefix)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::new("system_time_error", error.to_string()))?
        .as_millis();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("{prefix}-{millis}")
        } else {
            format!("{prefix}-{millis}-{suffix}")
        };
        if !checkpoint_path(database, &name)?.exists() {
            return Ok(name);
        }
    }
    Err(AppError::new(
        "checkpoint_create_failed",
        "could not allocate a safety checkpoint name",
    ))
}

fn artifact_snapshot(tx: &Connection, include_source_content: bool) -> Result<artifacts::Snapshot> {
    if std::env::var("LWC_TEST_FORBID_FULL_ARTIFACT_SNAPSHOT").as_deref() == Ok("1") {
        return Err(AppError::new(
            "forbidden_full_artifact_snapshot",
            "injected guard rejected a complete artifact snapshot",
        ));
    }
    let meta = |key: &str, fallback: &str| -> Result<String> {
        Ok(tx
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| fallback.to_string()))
    };

    let mut sources = Vec::new();
    let mut source_paths = BTreeMap::new();
    {
        let sql = if include_source_content {
            "SELECT id, title, origin, content FROM sources ORDER BY id"
        } else {
            "SELECT id, title, origin, '' FROM sources ORDER BY id"
        };
        let mut statement = tx.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, origin, content) = row?;
            let id_text = id.to_string();
            let path = artifacts::source_artifact_rel_path(&id_text, &origin)
                .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
            source_paths.insert(id, path);
            sources.push(artifacts::Source {
                id: id_text,
                title,
                origin,
                content,
            });
        }
    }

    let mut citations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut statement = tx.prepare(
            "SELECT page_slug, source_id
             FROM page_sources
             ORDER BY page_slug, source_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (slug, source_id) = row?;
            if let Some(path) = source_paths.get(&source_id) {
                citations.entry(slug).or_default().push(path.clone());
            }
        }
    }

    let pages = {
        let mut statement = tx.prepare(
            "SELECT p.slug, p.title, p.kind, p.summary, p.body, p.created_at, p.updated_at,
                    EXISTS(
                        SELECT 1 FROM page_sources ps WHERE ps.page_slug = p.slug
                    ),
                    (
                        SELECT GROUP_CONCAT(pp.provenance, ',')
                        FROM page_provenance pp
                        WHERE pp.page_slug = p.slug
                    )
             FROM pages p
             ORDER BY p.slug",
        )?;
        statement
            .query_map([], |row| {
                let slug = row.get::<_, String>(0)?;
                Ok(artifacts::Page {
                    source_artifact_paths: citations.get(&slug).cloned().unwrap_or_default(),
                    slug,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    body: row.get(4)?,
                    provenance: provenance_from_parts(row.get::<_, i64>(7)? != 0, row.get(8)?),
                    created: row.get(5)?,
                    updated: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let operations = {
        let mut statement = tx.prepare(
            "SELECT created_at, action, target, detail_json
             FROM operations
             ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok(artifacts::Operation {
                    created_at: row.get(0)?,
                    action: row.get(1)?,
                    target: row.get(2)?,
                    detail: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(artifacts::Snapshot {
        schema: meta("schema", DEFAULT_SCHEMA)?,
        purpose: meta("purpose", DEFAULT_PURPOSE)?,
        sources,
        pages,
        operations,
    })
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    enable_wal(conn)?;
    enable_persistent_wal(conn, MAIN_DB)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    Ok(())
}

fn configure_read_only_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

fn enable_persistent_wal(conn: &Connection, database_name: &CStr) -> Result<()> {
    let mut enabled = 1_i32;
    // SAFETY: the connection and static database-name pointer remain valid for this call.
    let rc = unsafe {
        ffi::sqlite3_file_control(
            conn.handle(),
            database_name.as_ptr(),
            ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&mut enabled as *mut i32).cast(),
        )
    };
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(AppError::new(
            "database_error",
            format!("failed to enable persistent WAL mode (sqlite rc {rc})"),
        ))
    }
}

fn enable_wal(conn: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => {
                return Err(AppError::new(
                    "database_error",
                    format!("SQLite refused WAL mode and returned {mode}"),
                ));
            }
            Err(error)
                if started.elapsed() < BUSY_TIMEOUT
                    && matches!(
                        &error,
                        rusqlite::Error::SqliteFailure(inner, _)
                            if matches!(
                                inner.code,
                                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
                            )
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn prepare_store(
    conn: &mut Connection,
    allow_create: bool,
    migration_progress: Option<&mut MigrationProgress<'_>>,
) -> Result<bool> {
    let mut version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        return if allow_create {
            bootstrap_schema(conn)
        } else {
            Err(AppError::new(
                "unsupported_store_version",
                "wiki database has no recognized schema; run `lwc init` in a new directory",
            ))
        };
    }
    if !(1..=USER_VERSION).contains(&version) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    if version < SEARCH_INDEX_VERSION {
        migrate_search_index(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == SEARCH_INDEX_VERSION {
        migrate_ingest_workflow(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == INGEST_WORKFLOW_VERSION {
        migrate_compound_wiki(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == COMPOUND_WIKI_VERSION {
        migrate_page_provenance(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == PAGE_PROVENANCE_VERSION {
        migrate_source_path_revisions(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == SOURCE_PATH_REVISIONS_VERSION {
        migrate_retrieval_weighting(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == RETRIEVAL_WEIGHTING_VERSION {
        migrate_changesets(conn)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version == CHANGESETS_VERSION {
        migrate_hierarchical_graph(conn, migration_progress)?;
        version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    }
    if version != USER_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    ensure_graph_digest_accumulator(conn)?;
    validate_store(conn)?;
    Ok(false)
}

fn prepare_store_read_only(conn: &Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == 0 {
        return Err(AppError::new(
            "unsupported_store_version",
            "wiki database has no recognized schema; run `lwc init` in a new directory",
        ));
    }
    if !(1..=USER_VERSION).contains(&version) || version != USER_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!(
                "wiki database version {version} is not supported by this lwc build (expected {USER_VERSION})"
            ),
        ));
    }
    validate_store_read_only(conn)
}

fn migrate_ingest_workflow(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        INGEST_WORKFLOW_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        SEARCH_INDEX_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(&format!(
        "
        CREATE TABLE ingest_jobs(
            source_id INTEGER PRIMARY KEY,
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'analyzing', 'generating', 'completed', 'failed')
            ),
            attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
            analysis TEXT,
            last_error TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
        );

        CREATE INDEX ingest_jobs_status_source
        ON ingest_jobs(status, source_id);

        INSERT INTO ingest_jobs(source_id, status, updated_at)
        SELECT id, 'pending', {TIMESTAMP_SQL}
        FROM sources;
        "
    ))?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('schema', ?1)",
        params![DEFAULT_SCHEMA],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key, value) VALUES ('purpose', ?1)",
        params![DEFAULT_PURPOSE],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![INGEST_WORKFLOW_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", INGEST_WORKFLOW_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{INGEST_WORKFLOW_VERSION} workflow migration: {error}"),
        )
    })
}

fn migrate_compound_wiki(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        COMPOUND_WIKI_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        INGEST_WORKFLOW_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(
        "ALTER TABLE ingest_jobs
         ADD COLUMN no_derived_pages_reason TEXT;

         UPDATE sources
         SET title = origin
         WHERE title IS NULL OR TRIM(title) = '';",
    )?;
    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{COMPOUND_WIKI_VERSION} compact search index: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![COMPOUND_WIKI_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", COMPOUND_WIKI_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{COMPOUND_WIKI_VERSION} Wiki migration: {error}"),
        )
    })
}

fn migrate_page_provenance(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        PAGE_PROVENANCE_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        COMPOUND_WIKI_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    tx.execute_batch(
        "CREATE TABLE page_provenance(
            page_slug TEXT NOT NULL,
            provenance TEXT NOT NULL CHECK(
                provenance IN ('user-provided', 'agent-observed', 'hypothesis')
            ),
            PRIMARY KEY(page_slug, provenance),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![PAGE_PROVENANCE_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", PAGE_PROVENANCE_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{PAGE_PROVENANCE_VERSION} provenance migration: {error}"),
        )
    })
}

fn migrate_source_path_revisions(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        SOURCE_PATH_REVISIONS_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        PAGE_PROVENANCE_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    create_source_path_revisions(&tx)?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SOURCE_PATH_REVISIONS_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", SOURCE_PATH_REVISIONS_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to commit v{SOURCE_PATH_REVISIONS_VERSION} source path migration: {error}"
            ),
        )
    })
}

fn migrate_retrieval_weighting(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        RETRIEVAL_WEIGHTING_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        SOURCE_PATH_REVISIONS_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    add_structural_navigation_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} retrieval features: {error}"),
        )
    })?;
    create_retrieval_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} retrieval state: {error}"),
        )
    })?;
    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!(
                "failed to prepare v{RETRIEVAL_WEIGHTING_VERSION} weighted search index: {error}"
            ),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![RETRIEVAL_WEIGHTING_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", RETRIEVAL_WEIGHTING_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{RETRIEVAL_WEIGHTING_VERSION} retrieval migration: {error}"),
        )
    })
}

fn migrate_changesets(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        CHANGESETS_VERSION..=USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        RETRIEVAL_WEIGHTING_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }

    create_changeset_state(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{USER_VERSION} changeset state: {error}"),
        )
    })?;
    tx.execute_batch(
        "INSERT OR IGNORE INTO meta(key, value)
         VALUES ('store_id', LOWER(HEX(RANDOMBLOB(32))));
         INSERT OR IGNORE INTO meta(key, value)
         VALUES ('store_revision', LOWER(HEX(RANDOMBLOB(32))));",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![CHANGESETS_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", CHANGESETS_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{CHANGESETS_VERSION} changeset migration: {error}"),
        )
    })
}

fn migrate_hierarchical_graph(
    conn: &mut Connection,
    mut progress: Option<&mut MigrationProgress<'_>>,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match current {
        USER_VERSION => {
            tx.commit()?;
            return Ok(());
        }
        CHANGESETS_VERSION => {}
        other => {
            return Err(AppError::new(
                "unsupported_store_version",
                format!("cannot migrate wiki database version {other} to {USER_VERSION}"),
            ));
        }
    }
    create_hierarchical_graph_schema(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to create v{USER_VERSION} hierarchical graph schema: {error}"),
        )
    })?;
    tx.execute_batch(
        "DELETE FROM graph_deltas;
         DELETE FROM graph_generations;
         DELETE FROM graph_projection_state;
         DELETE FROM graph_occurrences;
         DELETE FROM term_pair_contributions;
         DELETE FROM term_pair_totals;
         DELETE FROM graph_edges;
         DELETE FROM span_fts;
         DELETE FROM graph_nodes;
         DELETE FROM document_index_state;",
    )?;
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO graph_projection_state(
                projection, engine, schema_version, canonical_generation,
                projected_generation, status, updated_at
             ) VALUES ('physical', 'rslg', 1, 0, 0, 'fresh', {TIMESTAMP_SQL})"
        ),
        [],
    )?;

    let sources = {
        let mut statement =
            tx.prepare("SELECT id, title, origin, content FROM sources ORDER BY id")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let pages = {
        let mut statement = tx.prepare("SELECT slug, title, body FROM pages ORDER BY slug")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    const FINALIZATION_UNITS: usize = 3;
    let total = sources.len() + pages.len() + FINALIZATION_UNITS;
    let mut completed = 0;
    report_migration_progress(&mut progress, completed, total, "indexing")?;
    if let Ok(delay) = std::env::var("LWC_TEST_MIGRATION_DELAY_MS")
        && let Ok(delay) = delay.parse::<u64>()
    {
        std::thread::sleep(Duration::from_millis(delay));
    }
    for (source_id, title, origin, content) in &sources {
        persist_document_graph(
            &tx,
            &DocumentGraphInput {
                document_type: DocumentType::Source,
                identifier: &source_id.to_string(),
                label: title.as_deref().unwrap_or(origin),
                content,
            },
            None,
            &[],
            false,
        )?;
        completed += 1;
        report_migration_progress(&mut progress, completed, total, "indexing-sources")?;
    }

    for (slug, title, body) in &pages {
        let source_ids = {
            let mut statement = tx.prepare(
                "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
            )?;
            statement
                .query_map(params![slug], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let relations = page_relation_edges(&tx, slug, &source_ids, &[])?;
        persist_document_graph(
            &tx,
            &DocumentGraphInput {
                document_type: DocumentType::Page,
                identifier: slug,
                label: title,
                content: body,
            },
            None,
            &relations,
            false,
        )?;
        completed += 1;
        report_migration_progress(&mut progress, completed, total, "indexing-pages")?;
    }

    let initial_generation = if sources.is_empty() && pages.is_empty() {
        0
    } else {
        1
    };
    if initial_generation == 1 {
        rebuild_cooccurrence_edges(&tx, &mut progress)?;
        let store_revision: String = tx.query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            &format!(
                "INSERT INTO graph_generations(
                    generation, store_revision, canonical_digest,
                    changed_document_count, created_at
                 ) VALUES (1, ?1, ?2, ?3, {TIMESTAMP_SQL})"
            ),
            params![
                store_revision,
                "0000000000000000000000000000000000000000000000000000000000000000",
                (sources.len() + pages.len()) as i64,
            ],
        )?;
        let links = {
            let mut statement = tx.prepare(
                "SELECT l.from_slug, l.to_slug
                 FROM links l
                 JOIN pages target ON target.slug = l.to_slug
                 ORDER BY l.from_slug, l.to_slug",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (from_slug, to_slug) in links {
            persist_migration_relation_edge(
                &tx,
                &automatic_edge(
                    "LINKS_TO",
                    &format!("page:{from_slug}"),
                    &format!("page:{to_slug}"),
                ),
                "page",
                &from_slug,
                initial_generation,
                "{}",
            )?;
        }
        let revisions = {
            let mut statement = tx.prepare(
                "SELECT tracked_path, revision, source_id
                 FROM source_path_revisions
                 WHERE revision > 1
                 ORDER BY tracked_path, revision",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (tracked_path, revision, source_id) in revisions {
            persist_revision_edge(
                &tx,
                &tracked_path,
                revision,
                source_id,
                Some(initial_generation),
            )?;
        }
        tx.execute("DELETE FROM graph_deltas", [])?;
        let records = canonical_database_records(&tx)?;
        {
            let mut statement = tx.prepare(&format!(
                "INSERT INTO graph_deltas(
                    generation, action, entity_type, entity_id,
                    document_type, document_identifier,
                    after_json, created_at
                 ) VALUES (1, 'add', ?1, ?2, ?3, ?4, ?5, {TIMESTAMP_SQL})"
            ))?;
            for ((entity_type, entity_id), after_json) in records {
                let record: Value = serde_json::from_str(&after_json).map_err(|error| {
                    AppError::new(
                        "store_migration_failed",
                        format!("failed to encode initial graph delta: {error}"),
                    )
                })?;
                let owner_type = if entity_type == "node" {
                    record.get("document_type")
                } else {
                    record.get("owner_type")
                }
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "page" | "source"));
                let owner_identifier = if entity_type == "node" {
                    record.get("document_identifier")
                } else {
                    record.get("owner_identifier")
                }
                .and_then(Value::as_str);
                statement.execute(params![
                    entity_type,
                    entity_id,
                    owner_type,
                    owner_identifier,
                    after_json,
                ])?;
            }
        }
        tx.execute(
            "UPDATE graph_generations
             SET canonical_digest = ?1, changed_document_count = ?2
             WHERE generation = 1",
            params![
                database_graph_digest(&tx)?,
                (sources.len() + pages.len()) as i64
            ],
        )?;
        tx.execute(
            &format!(
                "UPDATE graph_projection_state
                 SET canonical_generation = 1, projected_generation = 1,
                     status = 'fresh', updated_at = {TIMESTAMP_SQL}
                 WHERE projection = 'physical'"
            ),
            [],
        )?;
        report_migration_progress(&mut progress, 1, 1, "finalizing-digest")?;
    } else {
        report_migration_progress(&mut progress, 1, 1, "finalizing-empty")?;
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![USER_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{USER_VERSION} hierarchical graph migration: {error}"),
        )
    })?;
    report_migration_progress(&mut progress, 1, 1, "complete")
}

fn report_migration_progress(
    progress: &mut Option<&mut MigrationProgress<'_>>,
    completed: usize,
    total: usize,
    phase: &str,
) -> Result<()> {
    if let Some(progress) = progress.as_mut() {
        progress(completed, total, phase)?;
    }
    Ok(())
}

fn add_structural_navigation_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE sources ADD COLUMN structural_navigation INTEGER NOT NULL DEFAULT 0
             CHECK(structural_navigation IN (0, 1));
         ALTER TABLE pages ADD COLUMN structural_navigation INTEGER NOT NULL DEFAULT 0
             CHECK(structural_navigation IN (0, 1));
         UPDATE sources
         SET structural_navigation = CASE
             WHEN INSTR(LOWER(content), '总览文档') > 0
               OR INSTR(LOWER(content), '文档目录') > 0
               OR INSTR(LOWER(content), 'table of contents') > 0
               OR INSTR(LOWER(content), 'navigation index') > 0
               OR INSTR(LOWER(content), 'document index') > 0
             THEN 1 ELSE 0 END;
         UPDATE pages
         SET structural_navigation = CASE
             WHEN INSTR(LOWER(body), '总览文档') > 0
               OR INSTR(LOWER(body), '文档目录') > 0
               OR INSTR(LOWER(body), 'table of contents') > 0
               OR INSTR(LOWER(body), 'navigation index') > 0
               OR INSTR(LOWER(body), 'document index') > 0
             THEN 1 ELSE 0 END;",
    )?;
    Ok(())
}

fn create_source_path_revisions(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE source_path_revisions(
            tracked_path TEXT NOT NULL CHECK(TRIM(tracked_path) <> ''),
            revision INTEGER NOT NULL CHECK(revision >= 1),
            source_id INTEGER NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY(tracked_path, revision),
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE RESTRICT
        );

        CREATE INDEX source_path_revisions_source
        ON source_path_revisions(source_id, tracked_path, revision);",
    )?;
    Ok(())
}

fn create_retrieval_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(
        "CREATE TABLE retrieval_weights(
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            weight INTEGER NOT NULL CHECK(weight IN (-2, -1, 1, 2)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(target_type, target_identifier, provenance)
        );

        CREATE TABLE retrieval_feedback(
            query_fingerprint TEXT NOT NULL CHECK(LENGTH(query_fingerprint) = 64),
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            signal INTEGER NOT NULL CHECK(signal IN (-1, 1)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(query_fingerprint, target_type, target_identifier, provenance)
        );

        CREATE INDEX retrieval_feedback_target
        ON retrieval_feedback(target_type, target_identifier, query_fingerprint);"
    ))?;
    Ok(())
}

fn create_changeset_state(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS changesets(
            id TEXT PRIMARY KEY CHECK(LENGTH(id) = 64),
            name TEXT NOT NULL CHECK(TRIM(name) <> ''),
            status TEXT NOT NULL CHECK(status IN ('draft', 'committed', 'rolled_back')),
            base_revision TEXT NOT NULL CHECK(LENGTH(base_revision) = 64),
            base_operation_id INTEGER NOT NULL CHECK(base_operation_id >= 0),
            begin_operation_id INTEGER NOT NULL CHECK(begin_operation_id > base_operation_id),
            pre_commit_checkpoint TEXT,
            post_revision TEXT CHECK(post_revision IS NULL OR LENGTH(post_revision) = 64),
            created_at TEXT NOT NULL,
            committed_at TEXT,
            rolled_back_at TEXT
        );
        CREATE INDEX IF NOT EXISTS changesets_name_created ON changesets(name, created_at);",
    )?;
    Ok(())
}

fn bootstrap_schema(conn: &mut Connection) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current != 0 {
        tx.commit()?;
        return Ok(false);
    }

    tx.execute_batch(&format!(
        "
        CREATE TABLE meta(
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE sources(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content_hash TEXT NOT NULL UNIQUE,
            title TEXT,
            origin TEXT NOT NULL,
            content TEXT NOT NULL,
            structural_navigation INTEGER NOT NULL DEFAULT 0
                CHECK(structural_navigation IN (0, 1)),
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );

        CREATE TABLE pages(
            slug TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            kind TEXT,
            summary TEXT,
            body TEXT NOT NULL,
            structural_navigation INTEGER NOT NULL DEFAULT 0
                CHECK(structural_navigation IN (0, 1)),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE page_sources(
            page_slug TEXT NOT NULL,
            source_id INTEGER NOT NULL,
            PRIMARY KEY(page_slug, source_id),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE,
            FOREIGN KEY(source_id) REFERENCES sources(id)
        );

        CREATE TABLE page_provenance(
            page_slug TEXT NOT NULL,
            provenance TEXT NOT NULL CHECK(
                provenance IN ('user-provided', 'agent-observed', 'hypothesis')
            ),
            PRIMARY KEY(page_slug, provenance),
            FOREIGN KEY(page_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );

        CREATE TABLE links(
            from_slug TEXT NOT NULL,
            to_slug TEXT NOT NULL,
            PRIMARY KEY(from_slug, to_slug),
            FOREIGN KEY(from_slug) REFERENCES pages(slug) ON DELETE CASCADE
        );

        CREATE TABLE operations(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            action TEXT NOT NULL,
            target TEXT NOT NULL,
            detail_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );

        CREATE TABLE ingest_jobs(
            source_id INTEGER PRIMARY KEY,
            status TEXT NOT NULL CHECK(
                status IN ('pending', 'analyzing', 'generating', 'completed', 'failed')
            ),
            attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
            analysis TEXT,
            last_error TEXT,
            no_derived_pages_reason TEXT,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
        );

        CREATE INDEX ingest_jobs_status_source
        ON ingest_jobs(status, source_id);

        CREATE TABLE source_path_revisions(
            tracked_path TEXT NOT NULL CHECK(TRIM(tracked_path) <> ''),
            revision INTEGER NOT NULL CHECK(revision >= 1),
            source_id INTEGER NOT NULL,
            observed_at TEXT NOT NULL,
            PRIMARY KEY(tracked_path, revision),
            FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE RESTRICT
        );

        CREATE INDEX source_path_revisions_source
        ON source_path_revisions(source_id, tracked_path, revision);

        CREATE TABLE retrieval_weights(
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            weight INTEGER NOT NULL CHECK(weight IN (-2, -1, 1, 2)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(target_type, target_identifier, provenance)
        );

        CREATE TABLE retrieval_feedback(
            query_fingerprint TEXT NOT NULL CHECK(LENGTH(query_fingerprint) = 64),
            target_type TEXT NOT NULL CHECK(target_type IN ('page', 'source')),
            target_identifier TEXT NOT NULL CHECK(TRIM(target_identifier) <> ''),
            provenance TEXT NOT NULL CHECK(provenance IN ('user-provided', 'agent-observed')),
            signal INTEGER NOT NULL CHECK(signal IN (-1, 1)),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            PRIMARY KEY(query_fingerprint, target_type, target_identifier, provenance)
        );

        CREATE INDEX retrieval_feedback_target
        ON retrieval_feedback(target_type, target_identifier, query_fingerprint);

        CREATE VIRTUAL TABLE search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
            path_terms,
            summary_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );

        INSERT INTO meta(key, value) VALUES ('format_version', '{USER_VERSION}');
        INSERT INTO meta(key, value) VALUES ('tokenizer', '{TOKENIZER_ID}');
        INSERT INTO meta(key, value) VALUES ('store_id', LOWER(HEX(RANDOMBLOB(32))));
        INSERT INTO meta(key, value) VALUES ('store_revision', LOWER(HEX(RANDOMBLOB(32))));
        PRAGMA user_version = {USER_VERSION};
        "
    ))?;
    create_changeset_state(&tx)?;
    create_hierarchical_graph_schema(&tx)?;
    save_graph_digest_accumulator(&tx, &GraphDigestAccumulator::default())?;
    tx.execute(
        &format!(
            "INSERT INTO graph_projection_state(
                projection, engine, schema_version, canonical_generation,
                projected_generation, status, updated_at
             ) VALUES ('physical', 'rslg', 1, 0, 0, 'fresh', {TIMESTAMP_SQL})"
        ),
        [],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('schema', ?1)",
        params![DEFAULT_SCHEMA],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('purpose', ?1)",
        params![DEFAULT_PURPOSE],
    )?;
    tx.commit()?;
    Ok(true)
}

fn migrate_search_index(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if (SEARCH_INDEX_VERSION..=USER_VERSION).contains(&current) {
        tx.commit()?;
        return Ok(());
    }
    if !matches!(current, 1..=3) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {SEARCH_INDEX_VERSION}"),
        ));
    }

    rebuild_search_index(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{SEARCH_INDEX_VERSION} search index: {error}"),
        )
    })?;

    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SEARCH_INDEX_VERSION.to_string()],
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('tokenizer', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![TOKENIZER_ID],
    )?;
    tx.pragma_update(None, "user_version", SEARCH_INDEX_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{SEARCH_INDEX_VERSION} search migration: {error}"),
        )
    })
}

fn rebuild_search_index(tx: &Transaction<'_>) -> Result<(usize, usize)> {
    tx.execute_batch(
        "
        DROP TRIGGER IF EXISTS main.sources_ai;
        DROP TRIGGER IF EXISTS main.sources_au;
        DROP TRIGGER IF EXISTS main.sources_ad;
        DROP TRIGGER IF EXISTS main.pages_ai;
        DROP TRIGGER IF EXISTS main.pages_au;
        DROP TRIGGER IF EXISTS main.pages_ad;
        DROP TABLE IF EXISTS main.source_fts;
        DROP TABLE IF EXISTS main.page_fts;
        DROP TABLE IF EXISTS main.search_fts;
        DROP TABLE IF EXISTS main.search_fts_data;
        DROP TABLE IF EXISTS main.search_fts_idx;
        DROP TABLE IF EXISTS main.search_fts_content;
        DROP TABLE IF EXISTS main.search_fts_docsize;
        DROP TABLE IF EXISTS main.search_fts_config;
        CREATE VIRTUAL TABLE main.search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
            path_terms,
            summary_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );
        ",
    )?;

    let mut source_count = 0usize;
    {
        let mut statement =
            tx.prepare("SELECT id, title, origin, content FROM sources ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, origin, content) = row?;
            index_source(tx, None, id, title.as_deref(), &origin, &content)?;
            source_count += 1;
        }
    }

    let mut page_count = 0usize;
    {
        let mut statement =
            tx.prepare("SELECT slug, title, summary, body FROM pages ORDER BY slug")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (slug, title, summary, body) = row?;
            index_page(tx, None, &slug, &title, summary.as_deref(), &body)?;
            page_count += 1;
        }
    }
    Ok((source_count, page_count))
}

fn validate_store_read_only(conn: &Connection) -> Result<()> {
    let essential_tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'table' AND name IN (
            'meta', 'sources', 'pages', 'page_sources', 'page_provenance', 'links',
            'operations', 'ingest_jobs', 'source_path_revisions', 'retrieval_weights',
            'retrieval_feedback', 'changesets', 'search_fts', 'document_index_state',
            'graph_nodes', 'graph_edges', 'term_pair_contributions', 'graph_generations',
            'graph_deltas', 'graph_projection_state', 'span_fts'
         )",
        [],
        |row| row.get(0),
    )?;
    if essential_tables != 21 {
        return Err(AppError::new(
            "corrupt_store",
            "wiki database schema is incomplete",
        ));
    }
    let metadata = conn
        .query_row(
            "SELECT
                MAX(CASE WHEN key = 'format_version' THEN value END),
                MAX(CASE WHEN key = 'tokenizer' THEN value END),
                MAX(CASE WHEN key = 'store_id' THEN value END),
                MAX(CASE WHEN key = 'store_revision' THEN value END)
             FROM meta
             WHERE key IN ('format_version', 'tokenizer', 'store_id', 'store_revision')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .unwrap_or_default();
    if metadata.0.as_deref() != Some(USER_VERSION.to_string().as_str()) {
        return Err(AppError::new(
            "corrupt_store",
            "wiki format metadata does not match the store version",
        ));
    }
    if metadata.1.as_deref() != Some(TOKENIZER_ID) {
        return Err(AppError::new(
            "incompatible_search_index",
            "wiki search tokenizer is incompatible",
        ));
    }
    for value in [metadata.2, metadata.3] {
        if !value.as_deref().is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(AppError::new(
                "corrupt_store",
                "wiki store identity metadata is missing or invalid",
            ));
        }
    }
    Ok(())
}

fn validate_store(conn: &Connection) -> Result<()> {
    for sql in [
        "SELECT key, value FROM meta LIMIT 0",
        "SELECT id, content_hash, title, origin, content, structural_navigation, created_at FROM sources LIMIT 0",
        "SELECT slug, title, kind, summary, body, structural_navigation, created_at, updated_at FROM pages LIMIT 0",
        "SELECT page_slug, source_id FROM page_sources LIMIT 0",
        "SELECT page_slug, provenance FROM page_provenance LIMIT 0",
        "SELECT from_slug, to_slug FROM links LIMIT 0",
        "SELECT action, target, detail_json, created_at FROM operations LIMIT 0",
        "SELECT source_id, status, attempts, analysis, last_error, no_derived_pages_reason, updated_at FROM ingest_jobs LIMIT 0",
        "SELECT tracked_path, revision, source_id, observed_at FROM source_path_revisions LIMIT 0",
        "SELECT target_type, target_identifier, provenance, weight, reason, updated_at FROM retrieval_weights LIMIT 0",
        "SELECT query_fingerprint, target_type, target_identifier, provenance, signal, reason, updated_at FROM retrieval_feedback LIMIT 0",
        "SELECT id, name, status, base_revision, base_operation_id, begin_operation_id, pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at FROM changesets LIMIT 0",
        "SELECT rowid, doc_type, identifier, title_terms, path_terms, summary_terms, body_terms FROM search_fts LIMIT 0",
        "SELECT document_type, document_identifier, document_node_id, content_fingerprint, segmenter_version, generation, cooccurrence_truncated, indexed_at FROM document_index_state LIMIT 0",
        "SELECT node_id, node_type, document_type, document_identifier, parent_node_id, ordinal, byte_start, byte_end, content_fingerprint, segmenter_version, label, properties_json FROM graph_nodes LIMIT 0",
        "SELECT edge_id, edge_type, from_node_id, to_node_id, owner_type, owner_identifier, weight, confidence, provenance, reason, frequency, positions, first_position, properties_json, created_at, updated_at FROM graph_edges LIMIT 0",
        "SELECT term_node_id, document_type, document_identifier, frequency, positions, first_position FROM graph_occurrences LIMIT 0",
        "SELECT document_type, document_identifier, contributions FROM term_pair_contributions LIMIT 0",
        "SELECT from_term_id, to_term_id, raw_strength, witness_count FROM term_pair_totals LIMIT 0",
        "SELECT generation, store_revision, canonical_digest, changed_document_count, created_at FROM graph_generations LIMIT 0",
        "SELECT id, generation, action, entity_type, entity_id, document_type, document_identifier, before_json, after_json, created_at FROM graph_deltas LIMIT 0",
        "SELECT projection, engine, schema_version, canonical_generation, projected_generation, status, last_error_code, last_error_message, updated_at FROM graph_projection_state LIMIT 0",
        "SELECT rowid, span_id, span_type, document_type, document_identifier, title_terms, path_terms, body_terms FROM span_fts LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|error| {
            AppError::new(
                "corrupt_store",
                format!("wiki database schema is incomplete: {error}"),
            )
        })?;
    }
    let metadata = conn
        .query_row(
            "SELECT
                MAX(CASE WHEN key = 'format_version' THEN value END),
                MAX(CASE WHEN key = 'tokenizer' THEN value END),
                MAX(CASE WHEN key = 'store_id' THEN value END),
                MAX(CASE WHEN key = 'store_revision' THEN value END)
             FROM meta
             WHERE key IN ('format_version', 'tokenizer', 'store_id', 'store_revision')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .unwrap_or_default();
    let expected_format = USER_VERSION.to_string();
    if metadata.0.as_deref() != Some(expected_format.as_str()) {
        return Err(AppError::new(
            "corrupt_store",
            format!(
                "wiki format metadata is {:?}; expected {USER_VERSION}",
                metadata.0.as_deref().unwrap_or("missing")
            ),
        ));
    }
    if metadata.1.as_deref() != Some(TOKENIZER_ID) {
        return Err(AppError::new(
            "incompatible_search_index",
            format!(
                "wiki search index uses tokenizer {:?}; expected {TOKENIZER_ID}",
                metadata.1.as_deref().unwrap_or("unknown")
            ),
        ));
    }
    for (key, value) in [("store_id", metadata.2), ("store_revision", metadata.3)] {
        let valid = value.as_deref().is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if !valid {
            return Err(AppError::new(
                "corrupt_store",
                format!("wiki {key} is missing or invalid"),
            ));
        }
    }
    Ok(())
}

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
    let generation = if created {
        Some(persist_document_graph(
            tx,
            &DocumentGraphInput {
                document_type: DocumentType::Source,
                identifier: &source_id.to_string(),
                label: &title,
                content: &input.content,
            },
            None,
            &[],
            true,
        )?)
    } else {
        None
    };
    if let Some((revision, true)) = path_revision
        && let Some(path) = input.tracked_path.as_deref()
    {
        persist_revision_edge(tx, path, revision, source_id, generation)?;
    }
    Ok((source_id, created))
}

fn record_source_path_revision(
    tx: &Transaction<'_>,
    tracked_path: &str,
    source_id: i64,
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
    tx.execute(
        &format!(
            "INSERT INTO source_path_revisions(tracked_path, revision, source_id, observed_at)
             VALUES (?1, ?2, ?3, {TIMESTAMP_SQL})"
        ),
        params![tracked_path, revision, source_id],
    )?;
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
    Ok(Some(ChangesetCommitState {
        changeset_id: id,
        name,
        base_revision,
        post_revision,
        checkpoint,
        staged_operation_count,
        lint_issues,
        locked_publish_ms: 0,
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
    if sparse {
        merge_sparse_candidate(&tx, begin_operation_id)?;
    } else {
        let changed_search = changed_search_documents(&tx, "candidate")?;
        replace_main_from_attached(&tx, "candidate")?;
        refresh_changed_search_documents(&tx, changed_search)?;
    }
    changeset_test_fault("after_fts")?;
    validate_database_integrity(&tx)?;
    changeset_test_fault("after_integrity")?;

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
    })
}

fn merge_sparse_candidate(tx: &Transaction<'_>, begin_operation_id: i64) -> Result<()> {
    let operations = {
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

    merge_sparse_sources(tx, &operations)?;
    for (action, target, detail) in &operations {
        let detail = serde_json::from_str::<Value>(detail)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        record_operation(tx, action, target, &detail)?;
    }
    for slug in page_targets {
        merge_sparse_page(tx, &slug)?;
    }
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
            &format!(
                "INSERT OR IGNORE INTO sources(
                    id, content_hash, title, origin, content,
                    structural_navigation, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ),
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
        persist_document_graph(
            tx,
            &DocumentGraphInput {
                document_type: DocumentType::Source,
                identifier: &source_id.to_string(),
                label: source.1.as_deref().unwrap_or(&source.2),
                content: &source.3,
            },
            None,
            &[],
            true,
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
    let before_graph = before
        .as_ref()
        .map(|before| {
            build_document_graph(&DocumentGraphInput {
                document_type: DocumentType::Page,
                identifier: slug,
                label: &before.title,
                content: &before.body,
            })
            .map_err(segment_error)
        })
        .transpose()?;
    let Some((title, kind, summary, body, structural_navigation, created_at)) = candidate else {
        if before.is_some() {
            tx.execute(
                "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
                params![slug],
            )?;
            remove_document_graph(tx, "page", slug)?;
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
    let source_ids = {
        let mut statement = tx.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map(params![slug], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let links = {
        let mut statement =
            tx.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        statement
            .query_map(params![slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    index_page(tx, None, slug, &title, summary.as_deref(), &body)?;
    let generation = persist_document_graph(
        tx,
        &DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: slug,
            label: &title,
            content: &body,
        },
        before_graph.as_ref(),
        &page_relation_edges(tx, slug, &source_ids, &links)?,
        true,
    )?;
    persist_inbound_link_edges(tx, slug, generation)
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
    let before_graph = load_page_mutation_base(tx, slug)?
        .map(|before| {
            build_document_graph(&DocumentGraphInput {
                document_type: DocumentType::Page,
                identifier: slug,
                label: &before.title,
                content: &before.body,
            })
            .map_err(segment_error)
        })
        .transpose()?;
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
        remove_document_graph(tx, "page", slug)?;
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
    let generation = persist_document_graph(
        tx,
        &DocumentGraphInput {
            document_type: DocumentType::Page,
            identifier: slug,
            label: &page.title,
            content: &page.body,
        },
        before_graph.as_ref(),
        &page_relation_edges(tx, slug, &page.source_ids, &page.links)?,
        true,
    )?;
    persist_inbound_link_edges(tx, slug, generation)
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
        remove_document_graph(tx, "source", &source_id.to_string())?;
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
    persist_document_graph(
        tx,
        &DocumentGraphInput {
            document_type: DocumentType::Source,
            identifier: &source.id.to_string(),
            label: source.title.as_deref().unwrap_or(&source.origin),
            content: &source.content,
        },
        None,
        &[],
        true,
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

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn attached_store_identity(conn: &Connection) -> Result<StoreIdentity> {
    let (store_id, revision) = conn.query_row(
        "SELECT
            MAX(CASE WHEN key = 'store_id' THEN value END),
            MAX(CASE WHEN key = 'store_revision' THEN value END)
         FROM candidate.meta
         WHERE key IN ('store_id', 'store_revision')",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let operation_id = conn.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM candidate.operations",
        [],
        |row| row.get(0),
    )?;
    Ok(StoreIdentity {
        store_id: store_id.ok_or_else(|| {
            AppError::new("changeset_corrupt", "draft store_id metadata is missing")
        })?,
        revision: revision.ok_or_else(|| {
            AppError::new(
                "changeset_corrupt",
                "draft store_revision metadata is missing",
            )
        })?,
        operation_id,
    })
}

fn validate_changeset_table_inventory(conn: &Connection, schema: &str) -> Result<()> {
    const TABLES: [&str; 33] = [
        "changesets",
        "document_index_state",
        "graph_deltas",
        "graph_edges",
        "graph_generations",
        "graph_nodes",
        "graph_occurrences",
        "graph_projection_state",
        "ingest_jobs",
        "links",
        "meta",
        "operations",
        "page_provenance",
        "page_sources",
        "pages",
        "retrieval_feedback",
        "retrieval_weights",
        "search_fts",
        "search_fts_config",
        "search_fts_content",
        "search_fts_data",
        "search_fts_docsize",
        "search_fts_idx",
        "source_path_revisions",
        "sources",
        "span_fts",
        "span_fts_config",
        "span_fts_content",
        "span_fts_data",
        "span_fts_docsize",
        "span_fts_idx",
        "term_pair_contributions",
        "term_pair_totals",
    ];
    let sql = format!(
        "SELECT name FROM {schema}.sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
         ORDER BY name"
    );
    let mut statement = conn.prepare(&sql)?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if actual != TABLES {
        return Err(AppError::new(
            "changeset_corrupt",
            format!("{schema} Wiki table inventory does not match store format v{USER_VERSION}"),
        ));
    }
    Ok(())
}

type ChangedSearchDocuments = (Vec<(i64, Option<i64>)>, Vec<(String, Option<i64>)>);

fn changed_search_documents(
    conn: &Connection,
    source_schema: &str,
) -> Result<ChangedSearchDocuments> {
    if source_schema != "candidate" {
        return Err(AppError::new(
            "changeset_corrupt",
            "unsupported attached changeset schema",
        ));
    }
    let mut source_statement = conn.prepare(
        "WITH changed(id) AS (
             SELECT live.id FROM main.sources live
             WHERE NOT EXISTS (
                 SELECT 1 FROM candidate.sources draft
                 WHERE draft.id = live.id
                   AND draft.content_hash IS live.content_hash
                   AND draft.title IS live.title
                   AND draft.origin IS live.origin
                   AND draft.content IS live.content
                   AND draft.structural_navigation IS live.structural_navigation
                   AND draft.created_at IS live.created_at
             )
             UNION
             SELECT draft.id FROM candidate.sources draft
             WHERE NOT EXISTS (
                 SELECT 1 FROM main.sources live
                 WHERE live.id = draft.id
                   AND live.content_hash IS draft.content_hash
                   AND live.title IS draft.title
                   AND live.origin IS draft.origin
                   AND live.content IS draft.content
                   AND live.structural_navigation IS draft.structural_navigation
                   AND live.created_at IS draft.created_at
             )
         )
         SELECT changed.id,
                (SELECT rowid FROM candidate.search_fts
                 WHERE doc_type = 'source'
                   AND identifier = CAST(changed.id AS TEXT)
                 LIMIT 1)
         FROM changed ORDER BY changed.id",
    )?;
    let sources = source_statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut page_statement = conn.prepare(
        "WITH changed(slug) AS (
             SELECT live.slug FROM main.pages live
             WHERE NOT EXISTS (
                 SELECT 1 FROM candidate.pages draft
                 WHERE draft.slug = live.slug
                   AND draft.title IS live.title
                   AND draft.kind IS live.kind
                   AND draft.summary IS live.summary
                   AND draft.body IS live.body
                   AND draft.structural_navigation IS live.structural_navigation
                   AND draft.created_at IS live.created_at
                   AND draft.updated_at IS live.updated_at
             )
             UNION
             SELECT draft.slug FROM candidate.pages draft
             WHERE NOT EXISTS (
                 SELECT 1 FROM main.pages live
                 WHERE live.slug = draft.slug
                   AND live.title IS draft.title
                   AND live.kind IS draft.kind
                   AND live.summary IS draft.summary
                   AND live.body IS draft.body
                   AND live.structural_navigation IS draft.structural_navigation
                   AND live.created_at IS draft.created_at
                   AND live.updated_at IS draft.updated_at
             )
         )
         SELECT changed.slug,
                (SELECT rowid FROM candidate.search_fts
                 WHERE doc_type = 'page' AND identifier = changed.slug
                 LIMIT 1)
         FROM changed ORDER BY changed.slug",
    )?;
    let pages = page_statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((sources, pages))
}

fn refresh_changed_search_documents(
    tx: &Transaction<'_>,
    (sources, pages): ChangedSearchDocuments,
) -> Result<()> {
    for (source_id, _) in &sources {
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
            params![source_id.to_string()],
        )?;
    }
    for (slug, _) in &pages {
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = ?1",
            params![slug],
        )?;
    }

    for (source_id, rowid) in sources {
        let source = tx
            .query_row(
                "SELECT title, origin, content FROM sources WHERE id = ?1",
                params![source_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        match (source, rowid) {
            (Some((title, origin, content)), Some(rowid)) => index_source(
                tx,
                Some(rowid),
                source_id,
                title.as_deref(),
                &origin,
                &content,
            )?,
            (None, None) => {}
            _ => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "candidate source and search index do not match",
                ));
            }
        }
    }
    for (slug, rowid) in pages {
        let page = tx
            .query_row(
                "SELECT title, summary, body FROM pages WHERE slug = ?1",
                params![&slug],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        match (page, rowid) {
            (Some((title, summary, body)), Some(rowid)) => {
                index_page(tx, Some(rowid), &slug, &title, summary.as_deref(), &body)?;
            }
            (None, None) => {}
            _ => {
                return Err(AppError::new(
                    "changeset_corrupt",
                    "candidate page and search index do not match",
                ));
            }
        }
    }
    Ok(())
}

fn replace_main_from_attached(tx: &Transaction<'_>, source_schema: &str) -> Result<()> {
    if source_schema != "candidate" {
        return Err(AppError::new(
            "changeset_corrupt",
            "unsupported attached changeset schema",
        ));
    }
    tx.execute_batch(
        "DELETE FROM term_pair_totals;
         DELETE FROM term_pair_contributions;
         DELETE FROM graph_occurrences;
         DELETE FROM graph_edges;
         DELETE FROM graph_nodes;
         DELETE FROM document_index_state;
         DELETE FROM graph_deltas;
         DELETE FROM graph_generations;
         DELETE FROM page_sources;
         DELETE FROM page_provenance;
         DELETE FROM links;
         DELETE FROM ingest_jobs;
         DELETE FROM source_path_revisions;
         DELETE FROM retrieval_weights;
         DELETE FROM retrieval_feedback;
         DELETE FROM changesets;
         DELETE FROM operations;
         DELETE FROM pages;
         DELETE FROM sources;
         DELETE FROM meta;

         INSERT INTO meta(key, value)
         SELECT key, value FROM candidate.meta;
         DELETE FROM meta WHERE key = 'changeset_frozen';
         INSERT INTO sources(
             id, content_hash, title, origin, content, structural_navigation, created_at
         ) SELECT
             id, content_hash, title, origin, content, structural_navigation, created_at
           FROM candidate.sources;
         INSERT INTO pages(
             slug, title, kind, summary, body, structural_navigation, created_at, updated_at
         ) SELECT
             slug, title, kind, summary, body, structural_navigation, created_at, updated_at
           FROM candidate.pages;",
    )
    .map_err(changeset_copy_error)?;
    changeset_test_fault("mid_copy")?;
    tx.execute_batch(
        "
         INSERT INTO page_sources(page_slug, source_id)
         SELECT page_slug, source_id FROM candidate.page_sources;
         INSERT INTO page_provenance(page_slug, provenance)
         SELECT page_slug, provenance FROM candidate.page_provenance;
         INSERT INTO links(from_slug, to_slug)
         SELECT from_slug, to_slug FROM candidate.links;
         INSERT INTO operations(id, action, target, detail_json, created_at)
         SELECT id, action, target, detail_json, created_at FROM candidate.operations;
         INSERT INTO ingest_jobs(
             source_id, status, attempts, analysis, last_error,
             no_derived_pages_reason, updated_at
         ) SELECT
             source_id, status, attempts, analysis, last_error,
             no_derived_pages_reason, updated_at
           FROM candidate.ingest_jobs;
         INSERT INTO source_path_revisions(tracked_path, revision, source_id, observed_at)
         SELECT tracked_path, revision, source_id, observed_at
           FROM candidate.source_path_revisions;
         INSERT INTO retrieval_weights(
             target_type, target_identifier, provenance, weight, reason, updated_at
         ) SELECT
             target_type, target_identifier, provenance, weight, reason, updated_at
           FROM candidate.retrieval_weights;
         INSERT INTO retrieval_feedback(
             query_fingerprint, target_type, target_identifier,
             provenance, signal, reason, updated_at
         ) SELECT
             query_fingerprint, target_type, target_identifier,
             provenance, signal, reason, updated_at
           FROM candidate.retrieval_feedback;
         INSERT INTO changesets(
             id, name, status, base_revision, base_operation_id, begin_operation_id,
             pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at
         ) SELECT
             id, name, status, base_revision, base_operation_id, begin_operation_id,
             pre_commit_checkpoint, post_revision, created_at, committed_at, rolled_back_at
           FROM candidate.changesets;
         INSERT INTO graph_nodes(
             node_id, node_type, document_type, document_identifier,
             parent_node_id, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, label, properties_json
         ) SELECT
             node_id, node_type, document_type, document_identifier,
             parent_node_id, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, label, properties_json
           FROM candidate.graph_nodes;
         INSERT INTO graph_edges(
             edge_id, edge_type, from_node_id, to_node_id,
             owner_type, owner_identifier, weight, confidence,
             provenance, reason, frequency, positions, first_position,
             properties_json, created_at, updated_at
         ) SELECT
             edge_id, edge_type, from_node_id, to_node_id,
             owner_type, owner_identifier, weight, confidence,
             provenance, reason, frequency, positions, first_position,
             properties_json, created_at, updated_at
           FROM candidate.graph_edges;
         INSERT INTO graph_occurrences(
             term_node_id, document_type, document_identifier,
             frequency, positions, first_position
         ) SELECT
             term_node_id, document_type, document_identifier,
             frequency, positions, first_position
           FROM candidate.graph_occurrences;
         INSERT INTO term_pair_contributions(
             document_type, document_identifier, contributions
         ) SELECT document_type, document_identifier, contributions
           FROM candidate.term_pair_contributions;
         INSERT INTO term_pair_totals(
             from_term_id, to_term_id, raw_strength, witness_count
         ) SELECT from_term_id, to_term_id, raw_strength, witness_count
           FROM candidate.term_pair_totals;
         INSERT INTO document_index_state(
             document_type, document_identifier, document_node_id,
             content_fingerprint, segmenter_version, generation,
             cooccurrence_truncated, indexed_at
         ) SELECT
             document_type, document_identifier, document_node_id,
             content_fingerprint, segmenter_version, generation,
             cooccurrence_truncated, indexed_at
           FROM candidate.document_index_state;
         INSERT INTO graph_generations(
             generation, store_revision, canonical_digest,
             changed_document_count, created_at
         ) SELECT
             generation, store_revision, canonical_digest,
             changed_document_count, created_at
           FROM candidate.graph_generations;
         INSERT INTO graph_deltas(
             id, generation, action, entity_type, entity_id,
             document_type, document_identifier, before_json, after_json, created_at
         ) SELECT
             id, generation, action, entity_type, entity_id,
             document_type, document_identifier, before_json, after_json, created_at
           FROM candidate.graph_deltas;",
    )
    .map_err(changeset_copy_error)?;
    rebuild_span_index(tx)?;
    let generation: i64 = tx.query_row(
        "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
        [],
        |row| row.get(0),
    )?;
    tx.execute(
        &format!(
            "UPDATE graph_projection_state
             SET canonical_generation = ?1,
                 projected_generation = CASE
                     WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                 status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                 last_error_code = NULL, last_error_message = NULL,
                 updated_at = {TIMESTAMP_SQL}
             WHERE projection = 'physical'"
        ),
        params![generation],
    )?;
    Ok(())
}

fn rebuild_span_index(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM span_fts", [])?;
    let rows = {
        let mut statement = tx.prepare(
            "SELECT n.node_id, n.node_type, n.document_type,
                    n.document_identifier, n.label,
                    CASE n.document_type
                        WHEN 'page' THEN p.title
                        ELSE COALESCE(s.title, s.origin)
                    END AS title,
                    CASE n.document_type
                        WHEN 'page' THEN p.slug
                        ELSE s.origin
                    END AS path
             FROM graph_nodes n
             LEFT JOIN pages p
               ON n.document_type = 'page' AND p.slug = n.document_identifier
             LEFT JOIN sources s
               ON n.document_type = 'source'
              AND s.id = CAST(n.document_identifier AS INTEGER)
             WHERE n.node_type IN ('passage', 'sentence')
             ORDER BY n.document_type, n.document_identifier, n.node_type, n.ordinal, n.node_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (node_id, node_type, document_type, document_identifier, label, title, path) in rows {
        tx.execute(
            "INSERT INTO span_fts(
                span_id, span_type, document_type, document_identifier,
                title_terms, path_terms, body_terms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                node_id,
                node_type,
                document_type,
                document_identifier,
                joined_terms(&title),
                joined_terms(&path),
                joined_terms(&label),
            ],
        )?;
    }
    Ok(())
}

fn changeset_copy_error(error: rusqlite::Error) -> AppError {
    AppError::new(
        "changeset_corrupt",
        format!("changeset canonical copy failed: {error}"),
    )
}

fn wal_checkpoint_truncate(conn: &Connection, wait_for_readers: bool) -> Result<(i64, i64, i64)> {
    if wait_for_readers {
        return conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(Into::into);
    }
    conn.busy_timeout(Duration::ZERO)?;
    let checkpoint = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    });
    conn.busy_timeout(BUSY_TIMEOUT)?;
    checkpoint.map_err(Into::into)
}

fn changeset_test_fault(point: &str) -> Result<()> {
    if std::env::var("LWC_TEST_CHANGESET_FAULT").as_deref() == Ok(point) {
        return Err(AppError::new(
            "changeset_test_fault",
            format!("injected changeset fault at {point}"),
        ));
    }
    Ok(())
}

fn changeset_test_crash(point: &str) {
    if std::env::var("LWC_TEST_CHANGESET_FAULT").as_deref() == Ok(format!("crash:{point}").as_str())
    {
        std::process::abort();
    }
}

fn validate_database_integrity(conn: &Connection) -> Result<()> {
    let mut foreign_keys = conn.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        return Err(AppError::new(
            "changeset_corrupt",
            "changeset database violates a foreign key",
        ));
    }
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(AppError::new(
            "changeset_corrupt",
            format!("changeset database failed integrity_check: {integrity}"),
        ));
    }
    Ok(())
}

fn record_operation(
    tx: &Transaction<'_>,
    action: &str,
    target: &str,
    detail: &Value,
) -> Result<String> {
    if tx
        .query_row(
            "SELECT 1 FROM meta WHERE key = ?1 LIMIT 1",
            params![CHANGESET_FREEZE_KEY],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Err(AppError::new(
            "changeset_frozen",
            "changeset is frozen for commit; retry commit or discard it instead of staging more writes",
        ));
    }
    let detail_json = serde_json::to_string(detail)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    tx.execute(
        "INSERT INTO operations(action, target, detail_json) VALUES (?1, ?2, ?3)",
        params![action, target, detail_json],
    )?;
    let revision: String =
        tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(32)))", [], |row| row.get(0))?;
    let updated = tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'store_revision'",
        params![&revision],
    )?;
    if updated != 1 {
        return Err(AppError::new(
            "corrupt_store",
            "wiki store_revision metadata is missing",
        ));
    }
    Ok(revision)
}

fn read_source_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRecord> {
    Ok(SourceRecord {
        id: row.get(0)?,
        title: row.get(1)?,
        origin: row.get(2)?,
        content_hash: row.get(3)?,
        content: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn window_source(
    mut source: SourceRecord,
    offset_chars: usize,
    max_chars: Option<usize>,
) -> Result<(SourceRecord, SourceWindow)> {
    if max_chars == Some(0) {
        return Err(AppError::new(
            "invalid_limit",
            "max-chars must be greater than zero",
        ));
    }
    let total_chars = source.content.chars().count();
    if offset_chars > total_chars {
        return Err(AppError::new(
            "invalid_offset",
            format!("offset-chars {offset_chars} exceeds source length {total_chars}"),
        ));
    }
    let requested = max_chars.unwrap_or_else(|| total_chars.saturating_sub(offset_chars));
    let content = source
        .content
        .chars()
        .skip(offset_chars)
        .take(requested)
        .collect::<String>();
    let returned_chars = content.chars().count();
    let consumed = offset_chars.saturating_add(returned_chars);
    let has_more = consumed < total_chars;
    source.content = content;
    Ok((
        source,
        SourceWindow {
            offset_chars,
            returned_chars,
            total_chars,
            next_offset_chars: has_more.then_some(consumed),
            has_more,
        },
    ))
}

fn read_source_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceSummary> {
    Ok(SourceSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        origin: row.get(2)?,
        content_hash: row.get(3)?,
        bytes: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn read_source_status_target(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceStatusTarget> {
    Ok(SourceStatusTarget {
        requested_source_id: row.get(0)?,
        tracked_path: row.get(1)?,
        head_source_id: row.get(2)?,
        head_revision: row.get(3)?,
        head_content_hash: row.get(4)?,
    })
}

fn read_ingest_job_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<IngestJobSummary> {
    Ok(IngestJobSummary {
        source_id: row.get(0)?,
        status: row.get(1)?,
        attempts: row.get(2)?,
        last_error: row.get(3)?,
        no_derived_pages_reason: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn read_operation_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRecord> {
    let detail_json: String = row.get(3)?;
    let detail = serde_json::from_str(&detail_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(OperationRecord {
        id: row.get(0)?,
        action: row.get(1)?,
        target: row.get(2)?,
        detail,
        created_at: row.get(4)?,
    })
}

fn read_changeset_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChangesetHistoryState> {
    Ok(ChangesetHistoryState {
        id: row.get(0)?,
        name: row.get(1)?,
        status: row.get(2)?,
        base_revision: row.get(3)?,
        base_operation_id: row.get(4)?,
        begin_operation_id: row.get(5)?,
        pre_commit_checkpoint: row.get(6)?,
        post_revision: row.get(7)?,
        created_at: row.get(8)?,
        committed_at: row.get(9)?,
        rolled_back_at: row.get(10)?,
    })
}

fn read_retrieval_adjustment(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetrievalAdjustment> {
    Ok(RetrievalAdjustment {
        target_type: row.get(0)?,
        target_identifier: row.get(1)?,
        provenance: row.get(2)?,
        weight: row.get(3)?,
        reason: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn validate_retrieval_weight(weight: i32) -> Result<()> {
    if matches!(weight, -2 | -1 | 1 | 2) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_weight",
            "weight must be one of -2, -1, 1, or 2",
        ))
    }
}

fn validate_retrieval_provenance(provenance: &str) -> Result<()> {
    if matches!(provenance, "user-provided" | "agent-observed") {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_provenance",
            "retrieval provenance must be user-provided or agent-observed",
        ))
    }
}

fn validate_nonempty_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() {
        Err(AppError::new(
            "invalid_input",
            "retrieval adjustment reason must not be empty",
        ))
    } else {
        Ok(())
    }
}

fn normalize_retrieval_target(
    conn: &Connection,
    target_type: &str,
    identifier: &str,
) -> Result<String> {
    match target_type {
        "page" => {
            let exists = conn
                .query_row(
                    "SELECT 1 FROM pages WHERE slug = ?1",
                    params![identifier],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                Ok(identifier.to_string())
            } else {
                Err(AppError::new(
                    "page_not_found",
                    format!("page not found: {identifier}"),
                ))
            }
        }
        "source" => {
            let source_id = identifier.parse::<i64>().map_err(|_| {
                AppError::new(
                    "invalid_input",
                    format!("source identifier must be an integer: {identifier}"),
                )
            })?;
            let exists = conn
                .query_row(
                    "SELECT 1 FROM sources WHERE id = ?1",
                    params![source_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                Ok(source_id.to_string())
            } else {
                Err(AppError::new(
                    "source_not_found",
                    format!("source not found: {source_id}"),
                ))
            }
        }
        _ => Err(AppError::new(
            "invalid_input",
            "retrieval target type must be page or source",
        )),
    }
}

fn validate_ingest_status(status: &str) -> Result<()> {
    if matches!(
        status,
        "pending" | "analyzing" | "generating" | "completed" | "failed"
    ) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_ingest_status",
            format!("unsupported ingest status: {status}"),
        ))
    }
}

fn require_ingest_state(tx: &Transaction<'_>, source_id: i64, allowed: &[&str]) -> Result<()> {
    let status = tx
        .query_row(
            "SELECT status FROM ingest_jobs WHERE source_id = ?1",
            params![source_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::new(
                "ingest_job_not_found",
                format!("ingest job not found for source {source_id}"),
            )
        })?;
    if allowed.contains(&status.as_str()) {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_ingest_state",
            format!(
                "source {source_id} is {status}; expected {}",
                allowed.join(" or ")
            ),
        ))
    }
}

fn claim_ingest_job(tx: &Transaction<'_>, source_id: i64) -> Result<()> {
    let updated = tx.execute(
        &format!(
            "UPDATE ingest_jobs
             SET status = 'analyzing',
                 attempts = attempts + 1,
                 last_error = NULL,
                 updated_at = {TIMESTAMP_SQL}
             WHERE source_id = ?1 AND status = 'pending'"
        ),
        params![source_id],
    )?;
    if updated != 1 {
        return Err(AppError::new(
            "invalid_ingest_state",
            format!("source {source_id} is not pending"),
        ));
    }
    record_operation(tx, "ingest_claim", &source_id.to_string(), &json!({})).map(|_| ())
}

fn load_page_mutation_base(conn: &Connection, slug: &str) -> Result<Option<PageMutationBase>> {
    let Some((title, kind, summary, body, structural_navigation, updated_at)) = conn
        .query_row(
            "SELECT title, kind, summary, body, structural_navigation, updated_at
             FROM pages WHERE slug = ?1",
            [slug],
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
        .optional()?
    else {
        return Ok(None);
    };
    let source_ids = {
        let mut statement = conn.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map([slug], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let provenance = {
        let mut statement = conn.prepare(
            "SELECT provenance FROM page_provenance
             WHERE page_slug = ?1
             ORDER BY CASE provenance
                 WHEN 'user-provided' THEN 0
                 WHEN 'agent-observed' THEN 1
                 WHEN 'hypothesis' THEN 2
                 ELSE 3 END",
        )?;
        statement
            .query_map([slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let links = {
        let mut statement =
            conn.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        statement
            .query_map([slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let content_fingerprint = page_content_fingerprint(
        &title,
        kind.as_deref(),
        summary.as_deref(),
        &body,
        structural_navigation,
        &source_ids,
        &provenance,
        &links,
    );
    let version_fingerprint = hash_content(&format!("{content_fingerprint}\0{updated_at}"));
    Ok(Some(PageMutationBase {
        title,
        body,
        content_fingerprint,
        version_fingerprint,
    }))
}

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
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("]]") else {
            break;
        };
        let candidate = after_open[..end].trim();
        if !candidate.is_empty() {
            links.insert(candidate.to_string());
        }
        rest = &after_open[end + 2..];
    }
    links.into_iter().collect()
}

fn persist_migration_relation_edge(
    tx: &Transaction<'_>,
    edge: &CanonicalEdge,
    owner_type: &str,
    owner_identifier: &str,
    generation: i64,
    properties_json: &str,
) -> Result<()> {
    tx.execute(
        &format!(
            "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier, provenance,
                properties_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'automatic', ?7,
                       {TIMESTAMP_SQL}, {TIMESTAMP_SQL})"
        ),
        params![
            &edge.edge_id,
            edge.edge_type,
            &edge.from_node_id,
            &edge.to_node_id,
            owner_type,
            owner_identifier,
            properties_json,
        ],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO graph_deltas(
                generation, action, entity_type, entity_id,
                document_type, document_identifier,
                after_json, created_at
             ) VALUES (?1, 'add', 'edge', ?2, ?3, ?4, ?5, {TIMESTAMP_SQL})"
        ),
        params![
            generation,
            &edge.edge_id,
            if owner_type == "page" {
                Some("page")
            } else {
                None
            },
            if owner_type == "page" {
                Some(owner_identifier)
            } else {
                None
            },
            json!({
                "edge_id": edge.edge_id,
                "edge_type": edge.edge_type,
                "from_node_id": edge.from_node_id,
                "to_node_id": edge.to_node_id,
                "properties": serde_json::from_str::<Value>(properties_json)
                    .unwrap_or(Value::Null),
            })
            .to_string(),
        ],
    )?;
    Ok(())
}

fn database_graph_digest(conn: &Connection) -> Result<String> {
    #[cfg(test)]
    TEST_GRAPH_DIGEST_CALLS.set(TEST_GRAPH_DIGEST_CALLS.get() + 1);
    Ok(graph_digest_accumulator(&canonical_database_records(conn)?).digest())
}

const GRAPH_DIGEST_ALGORITHM: &str = "canonical-multiset-v1";
const GRAPH_DIGEST_ALGORITHM_KEY: &str = "graph_digest_algorithm";
const GRAPH_DIGEST_XOR_KEY: &str = "graph_digest_xor";
const GRAPH_DIGEST_SUM_KEY: &str = "graph_digest_sum";
const GRAPH_DIGEST_COUNT_KEY: &str = "graph_digest_count";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GraphDigestAccumulator {
    xor: [u8; 32],
    sum: [u8; 32],
    count: u64,
}

impl GraphDigestAccumulator {
    fn insert(&mut self, key: &(String, String), record: &str) -> Result<()> {
        let hash = graph_record_hash(key, record);
        for (target, byte) in self.xor.iter_mut().zip(hash) {
            *target ^= byte;
        }
        add_256(&mut self.sum, &hash);
        self.count = self.count.checked_add(1).ok_or_else(|| {
            AppError::new(
                "graph_index_capacity_exceeded",
                "graph record count overflow",
            )
        })?;
        Ok(())
    }

    fn remove(&mut self, key: &(String, String), record: &str) -> Result<()> {
        let hash = graph_record_hash(key, record);
        for (target, byte) in self.xor.iter_mut().zip(hash) {
            *target ^= byte;
        }
        subtract_256(&mut self.sum, &hash);
        self.count = self.count.checked_sub(1).ok_or_else(|| {
            AppError::new("graph_index_corrupt", "graph digest record count underflow")
        })?;
        Ok(())
    }

    fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(GRAPH_DIGEST_ALGORITHM.as_bytes());
        hasher.update(self.count.to_be_bytes());
        hasher.update(self.xor);
        hasher.update(self.sum);
        hex_bytes(&hasher.finalize())
    }
}

fn graph_record_hash(key: &(String, String), record: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in [key.0.as_bytes(), key.1.as_bytes(), record.as_bytes()] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn add_256(target: &mut [u8; 32], value: &[u8; 32]) {
    let mut carry = 0_u16;
    for index in (0..32).rev() {
        let sum = u16::from(target[index]) + u16::from(value[index]) + carry;
        target[index] = sum as u8;
        carry = sum >> 8;
    }
}

fn subtract_256(target: &mut [u8; 32], value: &[u8; 32]) {
    let mut borrow = 0_i16;
    for index in (0..32).rev() {
        let difference = i16::from(target[index]) - i16::from(value[index]) - borrow;
        if difference < 0 {
            target[index] = (difference + 256) as u8;
            borrow = 1;
        } else {
            target[index] = difference as u8;
            borrow = 0;
        }
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_digest_bytes(value: &str, key: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(AppError::new(
            "graph_index_corrupt",
            format!("{key} has an invalid length"),
        ));
    }
    let mut output = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let encoded = std::str::from_utf8(chunk)
            .map_err(|_| AppError::new("graph_index_corrupt", format!("{key} is invalid")))?;
        output[index] = u8::from_str_radix(encoded, 16)
            .map_err(|_| AppError::new("graph_index_corrupt", format!("{key} is invalid")))?;
    }
    Ok(output)
}

fn graph_digest_accumulator(
    records: &BTreeMap<(String, String), String>,
) -> GraphDigestAccumulator {
    let mut accumulator = GraphDigestAccumulator::default();
    for (key, record) in records {
        accumulator
            .insert(key, record)
            .expect("canonical record count fits in u64");
    }
    accumulator
}

fn load_graph_digest_accumulator(conn: &Connection) -> Result<Option<GraphDigestAccumulator>> {
    let values = conn.query_row(
        "SELECT
                MAX(CASE WHEN key = ?1 THEN value END),
                MAX(CASE WHEN key = ?2 THEN value END),
                MAX(CASE WHEN key = ?3 THEN value END),
                MAX(CASE WHEN key = ?4 THEN value END)
             FROM meta WHERE key IN (?1, ?2, ?3, ?4)",
        params![
            GRAPH_DIGEST_ALGORITHM_KEY,
            GRAPH_DIGEST_XOR_KEY,
            GRAPH_DIGEST_SUM_KEY,
            GRAPH_DIGEST_COUNT_KEY,
        ],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    match values {
        (None, None, None, None) => Ok(None),
        (Some(algorithm), Some(xor), Some(sum), Some(count))
            if algorithm == GRAPH_DIGEST_ALGORITHM =>
        {
            Ok(Some(GraphDigestAccumulator {
                xor: decode_digest_bytes(&xor, GRAPH_DIGEST_XOR_KEY)?,
                sum: decode_digest_bytes(&sum, GRAPH_DIGEST_SUM_KEY)?,
                count: count.parse().map_err(|_| {
                    AppError::new(
                        "graph_index_corrupt",
                        "graph digest record count is invalid",
                    )
                })?,
            }))
        }
        _ => Err(AppError::new(
            "graph_index_corrupt",
            "graph digest metadata is incomplete or incompatible",
        )),
    }
}

fn save_graph_digest_accumulator(
    conn: &Connection,
    accumulator: &GraphDigestAccumulator,
) -> Result<()> {
    for (key, value) in [
        (
            GRAPH_DIGEST_ALGORITHM_KEY,
            GRAPH_DIGEST_ALGORITHM.to_string(),
        ),
        (GRAPH_DIGEST_XOR_KEY, hex_bytes(&accumulator.xor)),
        (GRAPH_DIGEST_SUM_KEY, hex_bytes(&accumulator.sum)),
        (GRAPH_DIGEST_COUNT_KEY, accumulator.count.to_string()),
    ] {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

fn current_graph_digest(conn: &Connection) -> Result<String> {
    match load_graph_digest_accumulator(conn)? {
        Some(accumulator) => Ok(accumulator.digest()),
        None => database_graph_digest(conn),
    }
}

fn ensure_graph_digest_accumulator(conn: &mut Connection) -> Result<()> {
    if load_graph_digest_accumulator(conn)?.is_some() {
        return Ok(());
    }
    for _ in 0..3 {
        let baseline_generation: i64 = conn.query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        let accumulator = graph_digest_accumulator(&canonical_database_records(conn)?);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load_graph_digest_accumulator(&tx)?.is_some() {
            tx.commit()?;
            return Ok(());
        }
        let observed_generation: i64 = tx.query_row(
            "SELECT COALESCE(MAX(generation), 0) FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        if observed_generation != baseline_generation {
            tx.rollback()?;
            continue;
        }
        save_graph_digest_accumulator(&tx, &accumulator)?;
        if observed_generation > 0 {
            tx.execute(
                "UPDATE graph_generations SET canonical_digest = ?1 WHERE generation = ?2",
                params![accumulator.digest(), observed_generation],
            )?;
            tx.execute(
                &format!(
                    "UPDATE graph_projection_state
                     SET status = CASE WHEN engine = 'graphlite' THEN 'pending' ELSE status END,
                         updated_at = {TIMESTAMP_SQL}
                     WHERE projection = 'physical'"
                ),
                [],
            )?;
        }
        tx.commit()?;
        return Ok(());
    }
    Err(AppError::new(
        "database_busy",
        "graph changed repeatedly while initializing digest metadata; retry",
    ))
}

fn apply_graph_digest_patch(
    conn: &Connection,
    before: &BTreeMap<(String, String), String>,
    after: &BTreeMap<(String, String), String>,
) -> Result<String> {
    let mut accumulator = load_graph_digest_accumulator(conn)?.ok_or_else(|| {
        AppError::new(
            "graph_digest_uninitialized",
            "graph digest metadata has not been initialized",
        )
    })?;
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in keys {
        let old = before.get(&key);
        let new = after.get(&key);
        if old == new {
            continue;
        }
        if let Some(record) = old {
            accumulator.remove(&key, record)?;
        }
        if let Some(record) = new {
            accumulator.insert(&key, record)?;
        }
    }
    save_graph_digest_accumulator(conn, &accumulator)?;
    Ok(accumulator.digest())
}

fn persist_revision_edge(
    tx: &Transaction<'_>,
    tracked_path: &str,
    revision: i64,
    source_id: i64,
    generation: Option<i64>,
) -> Result<()> {
    if revision <= 1 {
        return Ok(());
    }
    let previous_source_id: i64 = tx.query_row(
        "SELECT source_id FROM source_path_revisions
         WHERE tracked_path = ?1 AND revision = ?2",
        params![tracked_path, revision - 1],
        |row| row.get(0),
    )?;
    let from_node_id = format!("source:{source_id}");
    let to_node_id = format!("source:{previous_source_id}");
    let owner_identifier = format!("{tracked_path}#{revision}");
    let edge_id = format!(
        "edge:{}",
        hash_content(&format!(
            "REVISION_OF\0{tracked_path}\0{revision}\0{from_node_id}\0{to_node_id}"
        ))
    );
    let generation = match generation {
        Some(generation) => generation,
        None => {
            let generation: i64 = tx.query_row(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM graph_generations",
                [],
                |row| row.get(0),
            )?;
            let store_revision: String = tx.query_row(
                "SELECT value FROM meta WHERE key = 'store_revision'",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO graph_generations(
                        generation, store_revision, canonical_digest,
                        changed_document_count, created_at
                     ) VALUES (?1, ?2, ?3, 0, {TIMESTAMP_SQL})"
                ),
                params![generation, store_revision, hash_content(&edge_id)],
            )?;
            generation
        }
    };
    tx.execute(
        &format!(
            "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier, provenance,
                properties_json, created_at, updated_at
             ) VALUES (
                ?1, 'REVISION_OF', ?2, ?3, 'path', ?4, 'automatic', ?5,
                {TIMESTAMP_SQL}, {TIMESTAMP_SQL}
             )"
        ),
        params![
            &edge_id,
            &from_node_id,
            &to_node_id,
            &owner_identifier,
            json!({"tracked_path": tracked_path, "revision": revision}).to_string(),
        ],
    )?;
    let digest = apply_graph_digest_patch(
        tx,
        &BTreeMap::new(),
        &singleton_graph_record(
            "edge",
            &edge_id,
            canonical_graph_record(tx, "edge", &edge_id)?,
        ),
    )?;
    let after_json = json!({
        "edge_id": &edge_id,
        "edge_type": "REVISION_OF",
        "from_node_id": from_node_id,
        "to_node_id": to_node_id,
        "tracked_path": tracked_path,
        "revision": revision,
    })
    .to_string();
    tx.execute(
        &format!(
            "INSERT INTO graph_deltas(
                generation, action, entity_type, entity_id,
                document_type, document_identifier,
                after_json, created_at
             ) VALUES (?1, 'add', 'edge', ?2, 'source', ?3, ?4, {TIMESTAMP_SQL})"
        ),
        params![generation, &edge_id, source_id.to_string(), after_json],
    )?;
    tx.execute(
        "UPDATE graph_generations SET canonical_digest = ?1 WHERE generation = ?2",
        params![digest, generation],
    )?;
    tx.execute(
        &format!(
            "UPDATE graph_projection_state
             SET canonical_generation = ?1,
                 projected_generation = CASE
                     WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                 status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                 updated_at = {TIMESTAMP_SQL}
             WHERE projection = 'physical'"
        ),
        params![generation],
    )?;
    Ok(())
}

fn page_relation_edges(
    tx: &Transaction<'_>,
    slug: &str,
    source_ids: &[i64],
    links: &[String],
) -> Result<Vec<CanonicalEdge>> {
    let from = format!("page:{slug}");
    let mut edges = source_ids
        .iter()
        .map(|source_id| automatic_edge("CITES", &from, &format!("source:{source_id}")))
        .collect::<Vec<_>>();
    for link in links {
        let exists = tx
            .query_row("SELECT 1 FROM pages WHERE slug = ?1", params![link], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if exists {
            edges.push(automatic_edge("LINKS_TO", &from, &format!("page:{link}")));
        }
    }
    Ok(edges)
}

fn canonical_database_records(conn: &Connection) -> Result<BTreeMap<(String, String), String>> {
    let mut records = BTreeMap::new();
    let mut node_statement = conn.prepare(
        "SELECT node_id, json_object(
            'node_id', node_id, 'node_type', node_type,
            'document_type', document_type,
            'document_identifier', document_identifier,
            'parent_node_id', parent_node_id, 'ordinal', ordinal,
            'byte_start', byte_start, 'byte_end', byte_end,
            'content_fingerprint', content_fingerprint,
            'segmenter_version', segmenter_version,
            'label', label, 'properties_json', properties_json
         ) FROM graph_nodes ORDER BY node_id",
    )?;
    for row in node_statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (identifier, record) = row?;
        records.insert(("node".to_string(), identifier), record);
    }
    let mut edge_statement = conn.prepare(
        "SELECT edge_id, json_object(
            'edge_id', edge_id, 'edge_type', edge_type,
            'from_node_id', from_node_id, 'to_node_id', to_node_id,
            'owner_type', owner_type, 'owner_identifier', owner_identifier,
            'weight', weight, 'confidence', confidence,
            'provenance', provenance, 'reason', reason,
            'frequency', frequency, 'positions', HEX(positions),
            'first_position', first_position,
            'properties_json', properties_json
         ) FROM graph_edges ORDER BY edge_id",
    )?;
    for row in edge_statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (identifier, record) = row?;
        records.insert(("edge".to_string(), identifier), record);
    }
    insert_occurrence_records(conn, &mut records, None)?;
    Ok(records)
}

fn canonical_graph_record(
    conn: &Connection,
    entity_type: &str,
    entity_id: &str,
) -> Result<Option<String>> {
    let sql = match entity_type {
        "node" => {
            "SELECT json_object(
                'node_id', node_id, 'node_type', node_type,
                'document_type', document_type,
                'document_identifier', document_identifier,
                'parent_node_id', parent_node_id, 'ordinal', ordinal,
                'byte_start', byte_start, 'byte_end', byte_end,
                'content_fingerprint', content_fingerprint,
                'segmenter_version', segmenter_version,
                'label', label, 'properties_json', properties_json
             ) FROM graph_nodes WHERE node_id = ?1"
        }
        "edge" => {
            "SELECT json_object(
                'edge_id', edge_id, 'edge_type', edge_type,
                'from_node_id', from_node_id, 'to_node_id', to_node_id,
                'owner_type', owner_type, 'owner_identifier', owner_identifier,
                'weight', weight, 'confidence', confidence,
                'provenance', provenance, 'reason', reason,
                'frequency', frequency, 'positions', HEX(positions),
                'first_position', first_position,
                'properties_json', properties_json
             ) FROM graph_edges WHERE edge_id = ?1"
        }
        _ => {
            return Err(AppError::new(
                "graph_index_corrupt",
                "unsupported canonical graph record type",
            ));
        }
    };
    Ok(conn
        .query_row(sql, params![entity_id], |row| row.get(0))
        .optional()?)
}

fn singleton_graph_record(
    entity_type: &str,
    entity_id: &str,
    record: Option<String>,
) -> BTreeMap<(String, String), String> {
    record
        .map(|record| BTreeMap::from([((entity_type.to_string(), entity_id.to_string()), record)]))
        .unwrap_or_default()
}

fn insert_occurrence_records(
    conn: &Connection,
    records: &mut BTreeMap<(String, String), String>,
    document: Option<(&str, &str)>,
) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT o.term_node_id, s.document_node_id, o.document_type,
                o.document_identifier, o.frequency, HEX(o.positions), o.first_position
         FROM graph_occurrences o
         JOIN document_index_state s
           ON s.document_type = o.document_type
          AND s.document_identifier = o.document_identifier
         WHERE (?1 IS NULL OR (o.document_type = ?1 AND o.document_identifier = ?2))
         ORDER BY o.term_node_id, o.document_type, o.document_identifier",
    )?;
    let (document_type, document_identifier) = document
        .map(|(kind, identifier)| (Some(kind), Some(identifier)))
        .unwrap_or((None, None));
    for row in statement.query_map(params![document_type, document_identifier], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })? {
        let (term, target, owner_type, owner_identifier, frequency, positions, first_position) =
            row?;
        let identifier = automatic_edge("OCCURS_IN", &term, &target).edge_id;
        let record = json!({
            "edge_id": identifier,
            "edge_type": "OCCURS_IN",
            "from_node_id": term,
            "to_node_id": target,
            "owner_type": owner_type,
            "owner_identifier": owner_identifier,
            "weight": Value::Null,
            "confidence": Value::Null,
            "provenance": "automatic",
            "reason": Value::Null,
            "frequency": frequency,
            "positions": positions,
            "first_position": first_position,
            "properties_json": "{}",
        })
        .to_string();
        records.insert(("edge".to_string(), identifier), record);
    }
    Ok(())
}

fn canonical_document_records(
    conn: &Connection,
    document_type: &str,
    document_identifier: &str,
    affected_terms: &BTreeSet<String>,
) -> Result<BTreeMap<(String, String), String>> {
    let mut records = BTreeMap::new();
    {
        let mut statement = conn.prepare(
            "SELECT node_id, json_object(
                'node_id', node_id, 'node_type', node_type,
                'document_type', document_type,
                'document_identifier', document_identifier,
                'parent_node_id', parent_node_id, 'ordinal', ordinal,
                'byte_start', byte_start, 'byte_end', byte_end,
                'content_fingerprint', content_fingerprint,
                'segmenter_version', segmenter_version,
                'label', label, 'properties_json', properties_json
             ) FROM graph_nodes
             WHERE document_type = ?1 AND document_identifier = ?2
             ORDER BY node_id",
        )?;
        for row in statement.query_map(params![document_type, document_identifier], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (identifier, record) = row?;
            records.insert(("node".to_string(), identifier), record);
        }
    }
    {
        let mut statement = conn.prepare(
            "SELECT edge_id, json_object(
                'edge_id', edge_id, 'edge_type', edge_type,
                'from_node_id', from_node_id, 'to_node_id', to_node_id,
                'owner_type', owner_type, 'owner_identifier', owner_identifier,
                'weight', weight, 'confidence', confidence,
                'provenance', provenance, 'reason', reason,
                'frequency', frequency, 'positions', HEX(positions),
                'first_position', first_position,
                'properties_json', properties_json
             ) FROM graph_edges e
             WHERE (owner_type = ?1 AND owner_identifier = ?2)
                OR (owner_type = 'manual' AND (
                    EXISTS (
                        SELECT 1 FROM graph_nodes n
                        WHERE n.node_id = e.from_node_id
                          AND n.document_type = ?1 AND n.document_identifier = ?2
                    )
                    OR EXISTS (
                        SELECT 1 FROM graph_nodes n
                        WHERE n.node_id = e.to_node_id
                          AND n.document_type = ?1 AND n.document_identifier = ?2
                    )
                ))
             ORDER BY edge_id",
        )?;
        for row in statement.query_map(params![document_type, document_identifier], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (identifier, record) = row?;
            records.insert(("edge".to_string(), identifier), record);
        }
    }
    let mut term_node = conn.prepare(
        "SELECT node_id, json_object(
            'node_id', node_id, 'node_type', node_type,
            'document_type', document_type,
            'document_identifier', document_identifier,
            'parent_node_id', parent_node_id, 'ordinal', ordinal,
            'byte_start', byte_start, 'byte_end', byte_end,
            'content_fingerprint', content_fingerprint,
            'segmenter_version', segmenter_version,
            'label', label, 'properties_json', properties_json
         ) FROM graph_nodes WHERE node_id = ?1",
    )?;
    let mut term_edges = conn.prepare(
        "SELECT edge_id, json_object(
            'edge_id', edge_id, 'edge_type', edge_type,
            'from_node_id', from_node_id, 'to_node_id', to_node_id,
            'owner_type', owner_type, 'owner_identifier', owner_identifier,
            'weight', weight, 'confidence', confidence,
            'provenance', provenance, 'reason', reason,
            'frequency', frequency, 'positions', HEX(positions),
            'first_position', first_position,
            'properties_json', properties_json
         ) FROM graph_edges
         WHERE edge_type = 'CO_OCCURS' AND from_node_id = ?1
         ORDER BY edge_id",
    )?;
    for term in affected_terms {
        if let Some((identifier, record)) = term_node
            .query_row([term], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?
        {
            records.insert(("node".to_string(), identifier), record);
        }
        for row in term_edges.query_map([term], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (identifier, record) = row?;
            records.insert(("edge".to_string(), identifier), record);
        }
    }
    insert_occurrence_records(
        conn,
        &mut records,
        Some((document_type, document_identifier)),
    )?;
    Ok(records)
}

fn remove_document_graph(
    tx: &Transaction<'_>,
    document_type: &str,
    document_identifier: &str,
) -> Result<()> {
    let previous_contributions = tx
        .query_row(
            "SELECT contributions FROM term_pair_contributions
             WHERE document_type = ?1 AND document_identifier = ?2",
            params![document_type, document_identifier],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|encoded| decode_term_pair_contributions(&encoded))
        .transpose()?
        .unwrap_or_default();
    let mut affected_terms = previous_contributions
        .iter()
        .flat_map(|contribution| {
            [
                contribution.from_term_id.clone(),
                contribution.to_term_id.clone(),
            ]
        })
        .collect::<BTreeSet<_>>();
    {
        let mut statement = tx.prepare(
            "SELECT term_node_id FROM graph_occurrences
             WHERE document_type = ?1 AND document_identifier = ?2",
        )?;
        for row in statement.query_map(params![document_type, document_identifier], |row| {
            row.get::<_, String>(0)
        })? {
            affected_terms.insert(row?);
        }
    }
    expand_affected_cooccurrence_sources(tx, &mut affected_terms)?;
    let before =
        canonical_document_records(tx, document_type, document_identifier, &affected_terms)?;
    tx.execute(
        "DELETE FROM graph_occurrences
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM graph_edges
         WHERE owner_type = ?1 AND owner_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    apply_term_pair_totals(tx, &previous_contributions, -1.0)?;
    tx.execute(
        "DELETE FROM term_pair_contributions
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM span_fts
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM graph_nodes
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    tx.execute(
        "DELETE FROM document_index_state
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, document_identifier],
    )?;
    expand_affected_cooccurrence_sources(tx, &mut affected_terms)?;
    rebuild_cooccurrence_edges_for(tx, Some(&affected_terms))?;
    let after =
        canonical_document_records(tx, document_type, document_identifier, &affected_terms)?;
    let digest = apply_graph_digest_patch(tx, &before, &after)?;
    let generation: i64 = tx.query_row(
        "SELECT COALESCE(MAX(generation), 0) + 1 FROM graph_generations",
        [],
        |row| row.get(0),
    )?;
    let store_revision: String = tx.query_row(
        "SELECT value FROM meta WHERE key = 'store_revision'",
        [],
        |row| row.get(0),
    )?;
    tx.execute(
        &format!(
            "INSERT INTO graph_generations(
                generation, store_revision, canonical_digest,
                changed_document_count, created_at
             ) VALUES (?1, ?2, ?3, 1, {TIMESTAMP_SQL})"
        ),
        params![generation, store_revision, digest],
    )?;
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for (entity_type, entity_id) in keys {
        let old = before.get(&(entity_type.clone(), entity_id.clone()));
        let new = after.get(&(entity_type.clone(), entity_id.clone()));
        if old == new {
            continue;
        }
        let action = match (old, new) {
            (None, Some(_)) => "add",
            (Some(_), None) => "remove",
            (Some(_), Some(_)) => "update",
            (None, None) => continue,
        };
        tx.execute(
            &format!(
                "INSERT INTO graph_deltas(
                    generation, action, entity_type, entity_id,
                    document_type, document_identifier,
                    before_json, after_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, {TIMESTAMP_SQL})"
            ),
            params![
                generation,
                action,
                entity_type,
                entity_id,
                document_type,
                document_identifier,
                old,
                new,
            ],
        )?;
    }
    tx.execute(
        &format!(
            "UPDATE graph_projection_state
             SET canonical_generation = ?1,
                 projected_generation = CASE
                     WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                 status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                 updated_at = {TIMESTAMP_SQL}
             WHERE projection = 'physical'"
        ),
        params![generation],
    )?;
    Ok(())
}

fn persist_inbound_link_edges(
    tx: &Transaction<'_>,
    target_slug: &str,
    generation: i64,
) -> Result<()> {
    let from_slugs = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT l.from_slug FROM links l
             JOIN graph_nodes source
               ON source.node_id = 'page:' || l.from_slug
             WHERE l.to_slug = ?1 AND l.from_slug <> ?1
             ORDER BY l.from_slug",
        )?;
        statement
            .query_map(params![target_slug], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let before_records = BTreeMap::new();
    let mut after_records = BTreeMap::new();
    for from_slug in from_slugs {
        let edge = automatic_edge(
            "LINKS_TO",
            &format!("page:{from_slug}"),
            &format!("page:{target_slug}"),
        );
        let inserted = tx.execute(
            &format!(
                "INSERT OR IGNORE INTO graph_edges(
                    edge_id, edge_type, from_node_id, to_node_id,
                    owner_type, owner_identifier, provenance,
                    properties_json, created_at, updated_at
                 ) VALUES (?1, 'LINKS_TO', ?2, ?3, 'page', ?4, 'automatic',
                           '{{}}', {TIMESTAMP_SQL}, {TIMESTAMP_SQL})"
            ),
            params![
                &edge.edge_id,
                &edge.from_node_id,
                &edge.to_node_id,
                &from_slug,
            ],
        )?;
        if inserted == 0 {
            continue;
        }
        let key = ("edge".to_string(), edge.edge_id.clone());
        if let Some(record) = canonical_graph_record(tx, "edge", &edge.edge_id)? {
            after_records.insert(key, record);
        }
        tx.execute(
            &format!(
                "INSERT INTO graph_deltas(
                    generation, action, entity_type, entity_id,
                    document_type, document_identifier, after_json, created_at
                 ) VALUES (?1, 'add', 'edge', ?2, 'page', ?3, ?4, {TIMESTAMP_SQL})"
            ),
            params![
                generation,
                &edge.edge_id,
                &from_slug,
                json!({
                    "edge_id": edge.edge_id,
                    "edge_type": "LINKS_TO",
                    "from_node_id": edge.from_node_id,
                    "to_node_id": edge.to_node_id,
                    "owner_type": "page",
                    "owner_identifier": from_slug,
                    "provenance": "automatic",
                })
                .to_string(),
            ],
        )?;
    }
    if !after_records.is_empty() {
        let digest = apply_graph_digest_patch(tx, &before_records, &after_records)?;
        tx.execute(
            "UPDATE graph_generations SET canonical_digest = ?1 WHERE generation = ?2",
            params![digest, generation],
        )?;
    }
    Ok(())
}

fn persist_document_graph(
    tx: &Transaction<'_>,
    input: &DocumentGraphInput<'_>,
    _before: Option<&DocumentGraphReplacement>,
    extra_edges: &[CanonicalEdge],
    finalize_generation: bool,
) -> Result<i64> {
    let prepared = prepare_document_graph(input)?;
    persist_prepared_document_graph(tx, input, prepared, extra_edges, finalize_generation)
}

fn prepare_document_graph(input: &DocumentGraphInput<'_>) -> Result<PreparedDocumentGraph> {
    let graph = build_document_graph(input).map_err(segment_error)?;
    let mut cooccurrence = build_cooccurrence(input).map_err(segment_error)?;
    let encoded_contributions = if cooccurrence.contributions.is_empty() {
        None
    } else {
        match encode_term_pair_contributions(&cooccurrence.contributions) {
            Ok(encoded) => Some(encoded),
            Err(error) if error.code == "graph_index_capacity_exceeded" => {
                cooccurrence.contributions.clear();
                cooccurrence.capacity_exceeded = true;
                None
            }
            Err(error) => return Err(error),
        }
    };
    Ok(PreparedDocumentGraph {
        graph,
        cooccurrence,
        encoded_contributions,
    })
}

fn persist_prepared_document_graph(
    tx: &Transaction<'_>,
    input: &DocumentGraphInput<'_>,
    prepared: PreparedDocumentGraph,
    extra_edges: &[CanonicalEdge],
    finalize_generation: bool,
) -> Result<i64> {
    let PreparedDocumentGraph {
        mut graph,
        cooccurrence,
        encoded_contributions,
    } = prepared;
    graph.edges.extend_from_slice(extra_edges);
    let document_type = match input.document_type {
        DocumentType::Page => "page",
        DocumentType::Source => "source",
    };
    let mut affected_terms = cooccurrence
        .contributions
        .iter()
        .flat_map(|contribution| {
            [
                contribution.from_term_id.clone(),
                contribution.to_term_id.clone(),
            ]
        })
        .collect::<BTreeSet<_>>();
    affected_terms.extend(
        graph
            .nodes
            .iter()
            .filter(|node| node.node_type == "term")
            .map(|node| node.node_id.clone()),
    );
    let previous_contributions = tx
        .query_row(
            "SELECT contributions FROM term_pair_contributions
             WHERE document_type = ?1 AND document_identifier = ?2",
            params![document_type, input.identifier],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|encoded| decode_term_pair_contributions(&encoded))
        .transpose()?
        .unwrap_or_default();
    for contribution in &previous_contributions {
        affected_terms.insert(contribution.from_term_id.clone());
        affected_terms.insert(contribution.to_term_id.clone());
    }
    {
        let mut statement = tx.prepare(
            "SELECT term_node_id FROM graph_occurrences
             WHERE document_type = ?1 AND document_identifier = ?2",
        )?;
        for row in statement.query_map(params![document_type, input.identifier], |row| {
            row.get::<_, String>(0)
        })? {
            affected_terms.insert(row?);
        }
    }
    let before_records = if finalize_generation {
        expand_affected_cooccurrence_sources(tx, &mut affected_terms)?;
        canonical_document_records(tx, document_type, input.identifier, &affected_terms)?
    } else {
        BTreeMap::new()
    };

    tx.execute(
        "DELETE FROM graph_occurrences
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, input.identifier],
    )?;
    tx.execute(
        "DELETE FROM graph_edges
         WHERE owner_type = ?1 AND owner_identifier = ?2",
        params![document_type, input.identifier],
    )?;
    apply_term_pair_totals(tx, &previous_contributions, -1.0)?;
    tx.execute(
        "DELETE FROM term_pair_contributions
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, input.identifier],
    )?;
    tx.execute(
        "DELETE FROM span_fts
         WHERE document_type = ?1 AND document_identifier = ?2",
        params![document_type, input.identifier],
    )?;
    tx.execute(
        "DELETE FROM graph_nodes
         WHERE document_type = ?1 AND document_identifier = ?2
           AND node_type IN ('passage', 'sentence')",
        params![document_type, input.identifier],
    )?;

    {
        let mut node_statement = tx.prepare(
            "INSERT INTO graph_nodes(
                node_id, node_type, document_type, document_identifier,
                parent_node_id, ordinal, byte_start, byte_end,
                content_fingerprint, segmenter_version, label, properties_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '{}')
             ON CONFLICT(node_id) DO UPDATE SET
                node_type = excluded.node_type,
                document_type = excluded.document_type,
                document_identifier = excluded.document_identifier,
                parent_node_id = excluded.parent_node_id,
                ordinal = excluded.ordinal,
                byte_start = excluded.byte_start,
                byte_end = excluded.byte_end,
                content_fingerprint = excluded.content_fingerprint,
                segmenter_version = excluded.segmenter_version,
                label = excluded.label",
        )?;
        let mut span_statement = tx.prepare(
            "INSERT INTO span_fts(
                span_id, span_type, document_type, document_identifier,
                title_terms, path_terms, body_terms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let title_terms = joined_terms(input.label);
        let path_terms = joined_terms(input.identifier);
        for node in &graph.nodes {
            let node_document_type = node.document_type.map(|value| match value {
                DocumentType::Page => "page",
                DocumentType::Source => "source",
            });
            let (byte_start, byte_end) = node
                .byte_range
                .as_ref()
                .map(|range| (Some(range.start as i64), Some(range.end as i64)))
                .unwrap_or((None, None));
            node_statement.execute(params![
                &node.node_id,
                node.node_type,
                node_document_type,
                node.document_identifier.as_deref(),
                node.parent_node_id.as_deref(),
                node.ordinal.map(|value| value as i64),
                byte_start,
                byte_end,
                node.content_fingerprint.as_deref(),
                node.segmenter_version.map(i64::from),
                if matches!(node.node_type, "passage" | "sentence") {
                    ""
                } else {
                    &node.label
                },
            ])?;
            if matches!(node.node_type, "passage" | "sentence") {
                span_statement.execute(params![
                    &node.node_id,
                    node.node_type,
                    document_type,
                    input.identifier,
                    &title_terms,
                    &path_terms,
                    joined_terms(&node.label),
                ])?;
            }
        }
    }

    {
        let mut edge_statement = tx.prepare(&format!(
            "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier,
                provenance, properties_json, created_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                'automatic', '{{}}', {TIMESTAMP_SQL}, {TIMESTAMP_SQL}
             )"
        ))?;
        let mut occurrence_statement = tx.prepare(
            "INSERT INTO graph_occurrences(
                term_node_id, document_type, document_identifier,
                frequency, positions, first_position
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for edge in &graph.edges {
            if edge.edge_type == "OCCURS_IN" {
                if edge.to_node_id == graph.document_node_id {
                    occurrence_statement.execute(params![
                        &edge.from_node_id,
                        document_type,
                        input.identifier,
                        edge.frequency.map(|value| value as i64),
                        encode_positions(&edge.positions).map_err(position_encoding_error)?,
                        edge.positions.first().map(|range| range.start as i64),
                    ])?;
                }
                continue;
            }
            if matches!(edge.edge_type, "CONTAINS" | "NEXT" | "PREVIOUS") {
                continue;
            }
            edge_statement.execute(params![
                &edge.edge_id,
                edge.edge_type,
                &edge.from_node_id,
                &edge.to_node_id,
                document_type,
                input.identifier,
            ])?;
        }
    }

    if let Some(encoded_contributions) = encoded_contributions {
        tx.execute(
            "INSERT INTO term_pair_contributions(
                document_type, document_identifier, contributions
             ) VALUES (?1, ?2, ?3)",
            params![document_type, input.identifier, encoded_contributions,],
        )?;
    }
    apply_term_pair_totals(tx, &cooccurrence.contributions, 1.0)?;
    let generation = if finalize_generation {
        expand_affected_cooccurrence_sources(tx, &mut affected_terms)?;
        rebuild_cooccurrence_edges_for(tx, Some(&affected_terms))?;

        let generation: i64 = tx.query_row(
            "SELECT COALESCE(MAX(generation), 0) + 1 FROM graph_generations",
            [],
            |row| row.get(0),
        )?;
        upsert_document_index_state(
            tx,
            document_type,
            input.identifier,
            &graph.document_node_id,
            &graph.content_fingerprint,
            generation,
            cooccurrence.truncated_sentence_count + usize::from(cooccurrence.capacity_exceeded),
        )?;

        let after_records =
            canonical_document_records(tx, document_type, input.identifier, &affected_terms)?;
        let digest = apply_graph_digest_patch(tx, &before_records, &after_records)?;
        let store_revision: String = tx.query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            &format!(
                "INSERT INTO graph_generations(
                    generation, store_revision, canonical_digest,
                    changed_document_count, created_at
                 ) VALUES (?1, ?2, ?3, 1, {TIMESTAMP_SQL})"
            ),
            params![generation, store_revision, digest],
        )?;
        let record_keys = before_records
            .keys()
            .chain(after_records.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        {
            let mut delta_statement = tx.prepare(&format!(
                "INSERT INTO graph_deltas(
                    generation, action, entity_type, entity_id,
                    document_type, document_identifier,
                    before_json, after_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, {TIMESTAMP_SQL})"
            ))?;
            for (entity_type, entity_id) in record_keys {
                let old = before_records.get(&(entity_type.clone(), entity_id.clone()));
                let new = after_records.get(&(entity_type.clone(), entity_id.clone()));
                if old == new {
                    continue;
                }
                let action = match (old, new) {
                    (None, Some(_)) => "add",
                    (Some(_), None) => "remove",
                    (Some(_), Some(_)) => "update",
                    (None, None) => continue,
                };
                delta_statement.execute(params![
                    generation,
                    action,
                    entity_type,
                    entity_id,
                    document_type,
                    input.identifier,
                    old,
                    new,
                ])?;
            }
        }
        generation
    } else {
        1
    };
    if !finalize_generation {
        upsert_document_index_state(
            tx,
            document_type,
            input.identifier,
            &graph.document_node_id,
            &graph.content_fingerprint,
            generation,
            cooccurrence.truncated_sentence_count + usize::from(cooccurrence.capacity_exceeded),
        )?;
    }
    if finalize_generation {
        tx.execute(
            &format!(
                "UPDATE graph_projection_state
                 SET canonical_generation = ?1,
                     projected_generation = CASE
                         WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                     status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                     updated_at = {TIMESTAMP_SQL}
                 WHERE projection = 'physical'"
            ),
            params![generation],
        )?;
    }
    Ok(generation)
}

#[allow(clippy::too_many_arguments)]
fn upsert_document_index_state(
    tx: &Transaction<'_>,
    document_type: &str,
    document_identifier: &str,
    document_node_id: &str,
    content_fingerprint: &str,
    generation: i64,
    cooccurrence_truncated: usize,
) -> Result<()> {
    tx.execute(
        &format!(
            "INSERT INTO document_index_state(
                document_type, document_identifier, document_node_id,
                content_fingerprint, segmenter_version, generation,
                cooccurrence_truncated, indexed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, {TIMESTAMP_SQL})
             ON CONFLICT(document_type, document_identifier) DO UPDATE SET
                document_node_id = excluded.document_node_id,
                content_fingerprint = excluded.content_fingerprint,
                segmenter_version = excluded.segmenter_version,
                generation = excluded.generation,
                cooccurrence_truncated = excluded.cooccurrence_truncated,
                indexed_at = excluded.indexed_at"
        ),
        params![
            document_type,
            document_identifier,
            document_node_id,
            content_fingerprint,
            i64::from(crate::segment::SEGMENTER_VERSION),
            generation,
            cooccurrence_truncated as i64,
        ],
    )?;
    Ok(())
}

fn apply_term_pair_totals(
    tx: &Transaction<'_>,
    contributions: &[TermPairContribution],
    direction: f64,
) -> Result<()> {
    if contributions.is_empty() {
        return Ok(());
    }
    let mut add = tx.prepare(
        "INSERT INTO term_pair_totals(
            from_term_id, to_term_id, raw_strength, witness_count
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(from_term_id, to_term_id) DO UPDATE SET
            raw_strength = raw_strength + excluded.raw_strength,
            witness_count = witness_count + excluded.witness_count",
    )?;
    let mut remove = tx.prepare(
        "DELETE FROM term_pair_totals
         WHERE from_term_id = ?1 AND to_term_id = ?2 AND witness_count = ?3",
    )?;
    let mut subtract = tx.prepare(
        "UPDATE term_pair_totals
         SET raw_strength = MAX(0.0, raw_strength - ?3),
             witness_count = witness_count - ?4
         WHERE from_term_id = ?1 AND to_term_id = ?2 AND witness_count > ?4",
    )?;
    for contribution in contributions {
        let witness = i64::try_from(contribution.witness_count).map_err(|_| {
            AppError::new(
                "graph_index_capacity_exceeded",
                "co-occurrence witness count is too large",
            )
        })?;
        let strength = contribution.sentence_weight + contribution.passage_weight;
        if direction < 0.0 {
            if remove.execute(params![
                &contribution.from_term_id,
                &contribution.to_term_id,
                witness,
            ])? == 0
                && subtract.execute(params![
                    &contribution.from_term_id,
                    &contribution.to_term_id,
                    strength,
                    witness,
                ])? != 1
            {
                return Err(AppError::new(
                    "graph_index_corrupt",
                    "co-occurrence total is missing during document replacement",
                ));
            }
        } else {
            add.execute(params![
                &contribution.from_term_id,
                &contribution.to_term_id,
                strength,
                witness,
            ])?;
        }
    }
    tx.execute(
        "DELETE FROM term_pair_totals
         WHERE witness_count <= 0 OR raw_strength <= 0.000000000001",
        [],
    )?;
    Ok(())
}

fn expand_affected_cooccurrence_sources(
    tx: &Transaction<'_>,
    affected_terms: &mut BTreeSet<String>,
) -> Result<()> {
    let targets = affected_terms.iter().cloned().collect::<Vec<_>>();
    let mut statement = tx.prepare(
        "SELECT to_term_id FROM term_pair_totals
         WHERE from_term_id = ?1 ORDER BY to_term_id",
    )?;
    for target in targets {
        for source in statement.query_map([target], |row| row.get::<_, String>(0))? {
            affected_terms.insert(source?);
        }
    }
    Ok(())
}

fn persisted_cooccurrence_weight(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn rebuild_cooccurrence_edges(
    tx: &Transaction<'_>,
    progress: &mut Option<&mut MigrationProgress<'_>>,
) -> Result<()> {
    #[cfg(test)]
    TEST_COOCCURRENCE_REBUILDS.set(TEST_COOCCURRENCE_REBUILDS.get() + 1);
    #[cfg(test)]
    TEST_GLOBAL_TERM_PAIR_LOADS.set(TEST_GLOBAL_TERM_PAIR_LOADS.get() + 1);
    let terms = {
        let mut statement =
            tx.prepare("SELECT DISTINCT from_term_id FROM term_pair_totals ORDER BY from_term_id")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let total = terms.len().max(1);
    report_migration_progress(progress, 0, total, "ranking-cooccurrence")?;
    tx.execute(
        "DELETE FROM graph_edges
         WHERE owner_type = 'global' AND edge_type = 'CO_OCCURS'",
        [],
    )?;
    for (index, term) in terms.iter().enumerate() {
        rebuild_affected_cooccurrence_edges(tx, &BTreeSet::from([term.clone()]))?;
        let completed = index + 1;
        if completed == terms.len() || completed % 64 == 0 {
            report_migration_progress(progress, completed, total, "ranking-cooccurrence")?;
        }
    }
    if terms.is_empty() {
        report_migration_progress(progress, 1, 1, "ranking-cooccurrence")?;
    }
    Ok(())
}

fn rebuild_cooccurrence_edges_for(
    tx: &Transaction<'_>,
    affected_terms: Option<&BTreeSet<String>>,
) -> Result<()> {
    if let Some(affected_terms) = affected_terms {
        return rebuild_affected_cooccurrence_edges(tx, affected_terms);
    }
    #[cfg(test)]
    TEST_GLOBAL_TERM_PAIR_LOADS.set(TEST_GLOBAL_TERM_PAIR_LOADS.get() + 1);
    let contributions = {
        let mut statement = tx.prepare(
            "SELECT from_term_id, to_term_id, raw_strength, witness_count
             FROM term_pair_totals ORDER BY from_term_id, to_term_id",
        )?;
        statement
            .query_map([], |row| {
                Ok(TermPairContribution {
                    from_term_id: row.get(0)?,
                    to_term_id: row.get(1)?,
                    sentence_weight: row.get(2)?,
                    passage_weight: 0.0,
                    witness_count: row.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    tx.execute(
        "DELETE FROM graph_edges
         WHERE owner_type = 'global' AND edge_type = 'CO_OCCURS'",
        [],
    )?;
    let ranked = rank_cooccurrence(&contributions, 32);
    let mut insert_statement = tx.prepare(&format!(
        "INSERT INTO graph_edges(
            edge_id, edge_type, from_node_id, to_node_id,
            owner_type, owner_identifier, weight, provenance,
            properties_json, created_at, updated_at
         ) VALUES (
            ?1, 'CO_OCCURS', ?2, ?3, 'global', 'cooccurrence', ?4,
            'automatic', ?5, {TIMESTAMP_SQL}, {TIMESTAMP_SQL}
         )"
    ))?;
    for edge in ranked {
        let edge_id = format!(
            "edge:{}",
            hash_content(&format!(
                "CO_OCCURS\0{}\0{}",
                edge.from_term_id, edge.to_term_id
            ))
        );
        let normalized_strength = persisted_cooccurrence_weight(edge.normalized_strength);
        insert_statement.execute(params![
            edge_id,
            edge.from_term_id,
            edge.to_term_id,
            normalized_strength,
            json!({"rank": edge.rank}).to_string(),
        ])?;
    }
    tx.execute(
        "DELETE FROM graph_nodes
         WHERE node_type = 'term'
           AND NOT EXISTS (
               SELECT 1 FROM graph_edges
               WHERE from_node_id = graph_nodes.node_id
                  OR to_node_id = graph_nodes.node_id
           )
           AND NOT EXISTS (
               SELECT 1 FROM graph_occurrences
               WHERE term_node_id = graph_nodes.node_id
           )",
        [],
    )?;
    Ok(())
}

fn rebuild_affected_cooccurrence_edges(
    tx: &Transaction<'_>,
    affected_terms: &BTreeSet<String>,
) -> Result<()> {
    let mut delete_edges = tx.prepare(
        "DELETE FROM graph_edges
         WHERE owner_type = 'global' AND edge_type = 'CO_OCCURS'
           AND from_node_id = ?1",
    )?;
    let mut load_neighbors = tx.prepare(
        "SELECT to_term_id, raw_strength, witness_count
         FROM term_pair_totals
         WHERE from_term_id = ?1
         ORDER BY to_term_id",
    )?;
    let mut load_mass = tx.prepare(
        "SELECT COALESCE(SUM(raw_strength), 0.0)
         FROM term_pair_totals WHERE from_term_id = ?1",
    )?;
    let mut insert_edge = tx.prepare(&format!(
        "INSERT INTO graph_edges(
            edge_id, edge_type, from_node_id, to_node_id,
            owner_type, owner_identifier, weight, provenance,
            properties_json, created_at, updated_at
         ) VALUES (
            ?1, 'CO_OCCURS', ?2, ?3, 'global', 'cooccurrence', ?4,
            'automatic', ?5, {TIMESTAMP_SQL}, {TIMESTAMP_SQL}
         )"
    ))?;
    let mut masses = BTreeMap::new();
    for source in affected_terms {
        delete_edges.execute([source])?;
        let neighbors = load_neighbors
            .query_map([source], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let source_mass = neighbors
            .iter()
            .map(|(_, strength, _)| strength)
            .sum::<f64>();
        masses.insert(source.clone(), source_mass);
        let mut ranked = Vec::with_capacity(neighbors.len());
        for (target, raw_strength, witness_count) in neighbors {
            let target_mass = if let Some(value) = masses.get(&target) {
                *value
            } else {
                let value = load_mass.query_row([&target], |row| row.get::<_, f64>(0))?;
                masses.insert(target.clone(), value);
                value
            };
            let denominator = source_mass + target_mass;
            let normalized_strength = if denominator > 0.0 {
                (2.0 * raw_strength / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            };
            ranked.push((target, normalized_strength, witness_count));
        }
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        ranked.truncate(32);
        for (rank, (target, normalized_strength, _)) in ranked.into_iter().enumerate() {
            let edge_id = format!(
                "edge:{}",
                hash_content(&format!("CO_OCCURS\0{source}\0{target}"))
            );
            insert_edge.execute(params![
                edge_id,
                source,
                target,
                persisted_cooccurrence_weight(normalized_strength),
                json!({"rank": rank + 1}).to_string(),
            ])?;
        }
    }
    let mut delete_orphan = tx.prepare(
        "DELETE FROM graph_nodes
         WHERE node_id = ?1 AND node_type = 'term'
           AND NOT EXISTS (
               SELECT 1 FROM graph_edges
               WHERE from_node_id = graph_nodes.node_id
                  OR to_node_id = graph_nodes.node_id
           )
           AND NOT EXISTS (
               SELECT 1 FROM graph_occurrences
               WHERE term_node_id = graph_nodes.node_id
           )",
    )?;
    for term in affected_terms {
        delete_orphan.execute([term])?;
    }
    Ok(())
}

fn segment_error(error: crate::segment::SegmentError) -> AppError {
    AppError::new(
        "graph_index_capacity_exceeded",
        format!("document segmentation failed: {error:?}"),
    )
}

fn write_compact_u64(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_compact_u64(input: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *input.get(*cursor).ok_or_else(|| {
            AppError::new(
                "graph_index_corrupt",
                "truncated co-occurrence contribution blob",
            )
        })?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(AppError::new(
        "graph_index_corrupt",
        "invalid co-occurrence contribution varint",
    ))
}

fn encode_term_pair_contributions(contributions: &[TermPairContribution]) -> Result<Vec<u8>> {
    if contributions.len() > MAX_COOCCURRENCE_CONTRIBUTIONS {
        return Err(cooccurrence_capacity_error());
    }
    let terms = contributions
        .iter()
        .flat_map(|contribution| {
            [
                contribution.from_term_id.as_str(),
                contribution.to_term_id.as_str(),
            ]
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if terms.len() > MAX_COOCCURRENCE_TERMS {
        return Err(cooccurrence_capacity_error());
    }
    let term_indexes = terms
        .iter()
        .enumerate()
        .map(|(index, term)| (*term, index as u64))
        .collect::<BTreeMap<_, _>>();
    let mut output = vec![1];
    write_compact_u64(&mut output, terms.len() as u64);
    for term in &terms {
        if output
            .len()
            .checked_add(term.len() + 10)
            .is_none_or(|size| size > MAX_COOCCURRENCE_BLOB_BYTES)
        {
            return Err(cooccurrence_capacity_error());
        }
        write_compact_u64(&mut output, term.len() as u64);
        output.extend_from_slice(term.as_bytes());
    }
    write_compact_u64(&mut output, contributions.len() as u64);
    for contribution in contributions {
        if output
            .len()
            .checked_add(46)
            .is_none_or(|size| size > MAX_COOCCURRENCE_BLOB_BYTES)
        {
            return Err(cooccurrence_capacity_error());
        }
        write_compact_u64(
            &mut output,
            term_indexes[contribution.from_term_id.as_str()],
        );
        write_compact_u64(&mut output, term_indexes[contribution.to_term_id.as_str()]);
        output.extend_from_slice(&contribution.sentence_weight.to_le_bytes());
        output.extend_from_slice(&contribution.passage_weight.to_le_bytes());
        write_compact_u64(&mut output, contribution.witness_count as u64);
    }
    Ok(output)
}

fn cooccurrence_capacity_error() -> AppError {
    AppError::new(
        "graph_index_capacity_exceeded",
        "co-occurrence contribution exceeds the bounded storage budget",
    )
}

fn decode_term_pair_contributions(input: &[u8]) -> Result<Vec<TermPairContribution>> {
    if input.len() > MAX_COOCCURRENCE_BLOB_BYTES {
        return Err(AppError::new(
            "graph_index_corrupt",
            "co-occurrence contribution blob exceeds the safety limit",
        ));
    }
    if input.first() != Some(&1) {
        return Err(AppError::new(
            "graph_index_corrupt",
            "unsupported co-occurrence contribution blob version",
        ));
    }
    let mut cursor = 1usize;
    let term_count = usize::try_from(read_compact_u64(input, &mut cursor)?).map_err(|_| {
        AppError::new(
            "graph_index_corrupt",
            "co-occurrence term count is too large",
        )
    })?;
    if term_count > MAX_COOCCURRENCE_TERMS || term_count > input.len() {
        return Err(AppError::new(
            "graph_index_corrupt",
            "co-occurrence term count exceeds the safety limit",
        ));
    }
    let mut terms = Vec::with_capacity(term_count);
    for _ in 0..term_count {
        let length = usize::try_from(read_compact_u64(input, &mut cursor)?).map_err(|_| {
            AppError::new(
                "graph_index_corrupt",
                "co-occurrence term length is too large",
            )
        })?;
        let end = cursor.checked_add(length).ok_or_else(|| {
            AppError::new("graph_index_corrupt", "co-occurrence term range overflow")
        })?;
        let term =
            std::str::from_utf8(input.get(cursor..end).ok_or_else(|| {
                AppError::new("graph_index_corrupt", "truncated co-occurrence term")
            })?)
            .map_err(|_| {
                AppError::new("graph_index_corrupt", "invalid co-occurrence term UTF-8")
            })?;
        terms.push(term.to_string());
        cursor = end;
    }
    let contribution_count =
        usize::try_from(read_compact_u64(input, &mut cursor)?).map_err(|_| {
            AppError::new(
                "graph_index_corrupt",
                "co-occurrence contribution count is too large",
            )
        })?;
    if contribution_count > MAX_COOCCURRENCE_CONTRIBUTIONS
        || contribution_count > input.len().saturating_sub(cursor) / 19
    {
        return Err(AppError::new(
            "graph_index_corrupt",
            "co-occurrence contribution count exceeds the safety limit",
        ));
    }
    let mut contributions = Vec::with_capacity(contribution_count);
    for _ in 0..contribution_count {
        let from = usize::try_from(read_compact_u64(input, &mut cursor)?).unwrap_or(usize::MAX);
        let to = usize::try_from(read_compact_u64(input, &mut cursor)?).unwrap_or(usize::MAX);
        let weights_end = cursor.checked_add(16).ok_or_else(|| {
            AppError::new("graph_index_corrupt", "co-occurrence weight range overflow")
        })?;
        let weights = input.get(cursor..weights_end).ok_or_else(|| {
            AppError::new("graph_index_corrupt", "truncated co-occurrence weights")
        })?;
        let sentence_weight = f64::from_le_bytes(weights[..8].try_into().unwrap());
        let passage_weight = f64::from_le_bytes(weights[8..].try_into().unwrap());
        cursor = weights_end;
        let witness_count =
            usize::try_from(read_compact_u64(input, &mut cursor)?).unwrap_or(usize::MAX);
        if from >= terms.len()
            || to >= terms.len()
            || from == to
            || !sentence_weight.is_finite()
            || sentence_weight < 0.0
            || !passage_weight.is_finite()
            || passage_weight < 0.0
            || witness_count == 0
        {
            return Err(AppError::new(
                "graph_index_corrupt",
                "invalid co-occurrence contribution entry",
            ));
        }
        contributions.push(TermPairContribution {
            from_term_id: terms[from].clone(),
            to_term_id: terms[to].clone(),
            sentence_weight,
            passage_weight,
            witness_count,
        });
    }
    if cursor != input.len() {
        return Err(AppError::new(
            "graph_index_corrupt",
            "trailing co-occurrence contribution bytes",
        ));
    }
    Ok(contributions)
}

fn position_encoding_error(error: crate::graph_backend::PositionEncodingError) -> AppError {
    AppError::new(
        "graph_index_invalid_positions",
        format!("could not encode term positions: {error:?}"),
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

fn verify_graph_projection(conn: &Connection, database: &Path) -> Result<()> {
    let (engine, status, canonical, projected): (String, String, i64, i64) = conn.query_row(
        "SELECT engine, status, canonical_generation, projected_generation
         FROM graph_projection_state WHERE projection = 'physical'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if status == "disabled" {
        return Ok(());
    }
    if status != "fresh" || canonical != projected {
        return Err(AppError::new(
            "graph_projection_stale",
            "physical graph projection is not synchronized with canonical graph state",
        )
        .with_details(json!({
            "canonical_generation": canonical,
            "projected_generation": projected,
            "status": status,
        })));
    }
    if engine == "graphlite" {
        let digest = if canonical == 0 {
            current_graph_digest(conn)?
        } else {
            conn.query_row(
                "SELECT canonical_digest FROM graph_generations WHERE generation = ?1",
                params![canonical],
                |row| row.get::<_, String>(0),
            )?
        };
        let physical =
            graphqlite_projection_counts(database, canonical, &digest).map_err(|error| {
                AppError::new(
                    "graph_projection_stale",
                    "GraphQLite projection cannot be verified",
                )
                .with_details(json!({"cause": error.code, "generation": canonical}))
            })?;
        let logical = (
            conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get(0))?,
            conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?,
        );
        if physical != logical {
            return Err(AppError::new(
                "graph_projection_stale",
                "GraphQLite projection counts do not match canonical graph",
            )
            .with_details(json!({
                "generation": canonical,
                "canonical": {"nodes": logical.0, "edges": logical.1},
                "physical": {"nodes": physical.0, "edges": physical.1},
            })));
        }
    }
    Ok(())
}

fn resolve_graph_node(conn: &Connection, identifier: &str) -> Result<String> {
    let identifier = identifier.trim();
    if identifier.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "graph identifier cannot be empty",
        ));
    }
    let mut candidates = vec![identifier.to_string()];
    if !identifier.contains(':') {
        candidates.push(format!("page:{identifier}"));
        if identifier.parse::<i64>().is_ok() {
            candidates.push(format!("source:{identifier}"));
        }
        let tokens = tokenize_for_query(identifier);
        if tokens.len() == 1 {
            candidates.push(format!("term:{}", tokens[0]));
        }
    }
    for candidate in candidates {
        if conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM graph_nodes WHERE node_id = ?1)",
            params![&candidate],
            |row| row.get::<_, i64>(0),
        )? != 0
        {
            return Ok(candidate);
        }
    }
    Err(AppError::new(
        "graph_node_not_found",
        format!("graph node {identifier:?} was not found"),
    ))
}

fn graph_node_json(conn: &Connection, node_id: &str) -> Result<Value> {
    let mut node = conn.query_row(
        "SELECT node_id, node_type, label, document_type, document_identifier,
                parent_node_id, ordinal
         FROM graph_nodes WHERE node_id = ?1",
        params![node_id],
        |row| {
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "label": row.get::<_, String>(2)?,
                "document": match (
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ) {
                    (Some(document_type), Some(identifier)) => {
                        Some(json!({"type": document_type, "identifier": identifier}))
                    }
                    _ => None,
                },
                "parent_identifier": row.get::<_, Option<String>>(5)?,
                "ordinal": row.get::<_, Option<i64>>(6)?,
            }))
        },
    )?;
    if matches!(node["type"].as_str(), Some("passage" | "sentence")) {
        node["label"] = json!(load_span_record(conn, node_id)?.text);
    }
    Ok(node)
}

fn validate_graph_edge_types(edge_types: &[String]) -> Result<BTreeSet<String>> {
    const TYPES: [&str; 14] = [
        "CONTAINS",
        "NEXT",
        "PREVIOUS",
        "OCCURS_IN",
        "LINKS_TO",
        "CITES",
        "REVISION_OF",
        "CO_OCCURS",
        "SUPPORTS",
        "CONTRADICTS",
        "REFINES",
        "SUPERSEDES",
        "CAUSES",
        "DEPENDS_ON",
    ];
    let valid = TYPES.into_iter().collect::<BTreeSet<_>>();
    let mut normalized = BTreeSet::new();
    for edge_type in edge_types {
        let edge_type = edge_type.trim().to_uppercase();
        if !valid.contains(edge_type.as_str()) {
            return Err(AppError::new(
                "invalid_graph_edge_type",
                format!("unsupported graph edge type {edge_type:?}"),
            ));
        }
        normalized.insert(edge_type);
    }
    Ok(normalized)
}

fn load_adjacent_graph_edges(
    conn: &Connection,
    node_id: &str,
    direction: &str,
    edge_types: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<StoredGraphEdge>> {
    let direction_clause = match direction {
        "outgoing" => "from_node_id = ?1",
        "incoming" => "to_node_id = ?1",
        "both" => "(from_node_id = ?1 OR to_node_id = ?1)",
        _ => {
            return Err(AppError::new(
                "invalid_graph_direction",
                "graph direction must be outgoing, incoming, or both",
            ));
        }
    };
    let type_clause = if edge_types.is_empty() {
        String::new()
    } else {
        format!(
            " AND edge_type IN ({})",
            edge_types
                .iter()
                .map(|edge_type| format!("'{edge_type}'"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let sql = format!(
        "SELECT edge_id, edge_type, from_node_id, to_node_id,
                weight, confidence, provenance, reason,
                frequency, positions, first_position
         FROM graph_edges
         WHERE {direction_clause}{type_clause}
         ORDER BY edge_type, from_node_id, to_node_id, edge_id
         LIMIT ?2"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut edges = statement
        .query_map(params![node_id, limit as i64], |row| {
            Ok(StoredGraphEdge {
                edge_id: row.get(0)?,
                edge_type: row.get(1)?,
                from: row.get(2)?,
                to: row.get(3)?,
                weight: row.get(4)?,
                confidence: row.get(5)?,
                provenance: row.get(6)?,
                reason: row.get(7)?,
                frequency: row.get::<_, Option<i64>>(8)?.map(|value| value as usize),
                positions: row
                    .get::<_, Option<Vec<u8>>>(9)?
                    .map(|encoded| decode_positions(&encoded))
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Blob,
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("invalid graph positions: {error:?}"),
                            )),
                        )
                    })?,
                first_position: row.get::<_, Option<i64>>(10)?.map(|value| value as usize),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    edges.extend(load_derived_graph_edges(
        conn, node_id, direction, edge_types, limit,
    )?);
    edges.sort_by(|left, right| {
        (&left.edge_type, &left.from, &left.to, &left.edge_id).cmp(&(
            &right.edge_type,
            &right.from,
            &right.to,
            &right.edge_id,
        ))
    });
    edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    edges.truncate(limit);
    Ok(edges)
}

fn load_derived_graph_edges(
    conn: &Connection,
    node_id: &str,
    direction: &str,
    edge_types: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<StoredGraphEdge>> {
    let includes = |edge_type: &str| edge_types.is_empty() || edge_types.contains(edge_type);
    let accepts = |from: &str, to: &str| match direction {
        "outgoing" => from == node_id,
        "incoming" => to == node_id,
        "both" => from == node_id || to == node_id,
        _ => false,
    };
    let make = |edge_type: &'static str, from: &str, to: &str| StoredGraphEdge {
        edge_id: automatic_edge(edge_type, from, to).edge_id,
        edge_type: edge_type.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        weight: None,
        confidence: None,
        provenance: Some("automatic".to_string()),
        reason: None,
        frequency: None,
        positions: None,
        first_position: None,
    };
    let make_occurrence = |from: &str, to: &str, positions: Vec<Range<usize>>| StoredGraphEdge {
        edge_id: automatic_edge("OCCURS_IN", from, to).edge_id,
        edge_type: "OCCURS_IN".to_string(),
        from: from.to_string(),
        to: to.to_string(),
        weight: None,
        confidence: None,
        provenance: Some("automatic".to_string()),
        reason: None,
        frequency: Some(positions.len()),
        first_position: positions.first().map(|position| position.start),
        positions: Some(positions),
    };
    let node = conn
        .query_row(
            "SELECT node_type, document_type, document_identifier,
                    parent_node_id, ordinal, byte_start, byte_end
             FROM graph_nodes WHERE node_id = ?1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((node_type, document_type, document_identifier, parent, ordinal, start, end)) = node
    else {
        return Ok(Vec::new());
    };
    let mut edges = Vec::new();
    if includes("CONTAINS") {
        if let Some(parent) = &parent
            && accepts(parent, node_id)
        {
            edges.push(make("CONTAINS", parent, node_id));
        }
        let mut children = conn.prepare(
            "SELECT node_id FROM graph_nodes
             WHERE parent_node_id = ?1 ORDER BY ordinal, node_id LIMIT ?2",
        )?;
        for child in children.query_map(params![node_id, limit as i64], |row| {
            row.get::<_, String>(0)
        })? {
            let child = child?;
            if accepts(node_id, &child) {
                edges.push(make("CONTAINS", node_id, &child));
            }
        }
    }
    if matches!(node_type.as_str(), "passage" | "sentence")
        && let (Some(parent), Some(ordinal)) = (&parent, ordinal)
    {
        for (edge_type, sibling_ordinal) in [
            ("NEXT", ordinal + 1),
            ("PREVIOUS", ordinal.saturating_sub(1)),
        ] {
            if !includes(edge_type) || (edge_type == "PREVIOUS" && ordinal == 0) {
                continue;
            }
            if let Some(sibling) = conn
                .query_row(
                    "SELECT node_id FROM graph_nodes
                         WHERE parent_node_id = ?1 AND node_type = ?2 AND ordinal = ?3",
                    params![parent, &node_type, sibling_ordinal],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .filter(|sibling| accepts(node_id, sibling))
            {
                edges.push(make(edge_type, node_id, &sibling));
            }
        }
        for (edge_type, sibling_ordinal) in [
            ("NEXT", ordinal.saturating_sub(1)),
            ("PREVIOUS", ordinal + 1),
        ] {
            if !includes(edge_type) || (edge_type == "NEXT" && ordinal == 0) {
                continue;
            }
            if let Some(sibling) = conn
                .query_row(
                    "SELECT node_id FROM graph_nodes
                         WHERE parent_node_id = ?1 AND node_type = ?2 AND ordinal = ?3",
                    params![parent, &node_type, sibling_ordinal],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .filter(|sibling| accepts(sibling, node_id))
            {
                edges.push(make(edge_type, &sibling, node_id));
            }
        }
    }
    if includes("OCCURS_IN") {
        if node_type == "term" {
            let mut occurrences = conn.prepare(
                "SELECT o.document_type, o.document_identifier, o.positions,
                        s.document_node_id
                 FROM graph_occurrences o
                 JOIN document_index_state s
                   ON s.document_type = o.document_type
                  AND s.document_identifier = o.document_identifier
                 WHERE o.term_node_id = ?1
                 ORDER BY o.document_type, o.document_identifier",
            )?;
            for row in occurrences.query_map([node_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })? {
                let (kind, identifier, encoded, document_node) = row?;
                let positions = decode_positions(&encoded).map_err(position_encoding_error)?;
                if accepts(node_id, &document_node) {
                    edges.push(make_occurrence(node_id, &document_node, positions.clone()));
                }
                let mut spans = conn.prepare(
                    "SELECT node_id, byte_start, byte_end FROM graph_nodes
                     WHERE document_type = ?1 AND document_identifier = ?2
                       AND node_type IN ('passage', 'sentence')
                     ORDER BY node_type, ordinal, node_id",
                )?;
                for span in spans.query_map(params![kind, identifier], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as usize,
                        row.get::<_, i64>(2)? as usize,
                    ))
                })? {
                    let (span_id, span_start, span_end) = span?;
                    let span_positions = positions
                        .iter()
                        .filter(|position| position.start >= span_start && position.end <= span_end)
                        .cloned()
                        .collect::<Vec<_>>();
                    if !span_positions.is_empty() && accepts(node_id, &span_id) {
                        edges.push(make_occurrence(node_id, &span_id, span_positions));
                    }
                    if edges.len() >= limit {
                        break;
                    }
                }
                if edges.len() >= limit {
                    break;
                }
            }
        } else if let (Some(kind), Some(identifier)) = (document_type, document_identifier) {
            let mut occurrences = conn.prepare(
                "SELECT term_node_id, positions FROM graph_occurrences
                 WHERE document_type = ?1 AND document_identifier = ?2
                 ORDER BY term_node_id LIMIT ?3",
            )?;
            for row in occurrences.query_map(params![kind, identifier, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })? {
                let (term, encoded) = row?;
                let all_positions = decode_positions(&encoded).map_err(position_encoding_error)?;
                let positions = match (start, end) {
                    (Some(start), Some(end)) => all_positions
                        .into_iter()
                        .filter(|position| {
                            position.start >= start as usize && position.end <= end as usize
                        })
                        .collect::<Vec<_>>(),
                    _ => all_positions,
                };
                if !positions.is_empty() && accepts(&term, node_id) {
                    edges.push(make_occurrence(&term, node_id, positions));
                }
            }
        }
    }
    Ok(edges)
}

fn graph_edge_json(edge: &StoredGraphEdge) -> Value {
    json!({
        "identifier": edge.edge_id,
        "type": edge.edge_type,
        "from": edge.from,
        "to": edge.to,
        "weight": edge.weight,
        "confidence": edge.confidence,
        "provenance": edge.provenance,
        "reason": edge.reason,
        "frequency": edge.frequency,
        "positions": edge.positions.as_ref().map(|positions| positions
            .iter()
            .map(|position| json!({"byte_start": position.start, "byte_end": position.end}))
            .collect::<Vec<_>>()),
        "first_position": edge.first_position,
    })
}

fn traversed_neighbor<'a>(
    edge: &'a StoredGraphEdge,
    node: &str,
    direction: &str,
) -> Option<&'a str> {
    match direction {
        "outgoing" if edge.from == node => Some(&edge.to),
        "incoming" if edge.to == node => Some(&edge.from),
        "both" if edge.from == node => Some(&edge.to),
        "both" if edge.to == node => Some(&edge.from),
        _ => None,
    }
}

fn graph_explore_value(
    conn: &Connection,
    scope: &str,
    identifier: &str,
    max_depth: usize,
    limit: usize,
    direction: &str,
    requested_edge_types: &[String],
) -> Result<Value> {
    let start = resolve_graph_node(conn, identifier)?;
    let edge_types = validate_graph_edge_types(requested_edge_types)?;
    let mut queue = VecDeque::from([(start.clone(), 0usize)]);
    let mut depths = BTreeMap::from([(start.clone(), 0usize)]);
    let mut selected_edges = BTreeMap::new();
    let mut truncated = false;
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let adjacent = load_adjacent_graph_edges(conn, &node, direction, &edge_types, 20_001)?;
        if adjacent.len() > 20_000 {
            truncated = true;
        }
        for edge in adjacent.into_iter().take(20_000) {
            if selected_edges.len() >= 20_000 && !selected_edges.contains_key(&edge.edge_id) {
                truncated = true;
                continue;
            }
            let Some(neighbor) = traversed_neighbor(&edge, &node, direction) else {
                continue;
            };
            let neighbor = neighbor.to_string();
            selected_edges.insert(edge.edge_id.clone(), edge);
            if depths.contains_key(&neighbor) {
                continue;
            }
            if depths.len() >= limit {
                truncated = true;
                continue;
            }
            depths.insert(neighbor.clone(), depth + 1);
            queue.push_back((neighbor, depth + 1));
        }
    }
    let nodes = depths
        .iter()
        .map(|(node_id, depth)| {
            let mut node = graph_node_json(conn, node_id)?;
            node["depth"] = json!(depth);
            Ok(node)
        })
        .collect::<Result<Vec<_>>>()?;
    let selected = selected_edges
        .values()
        .map(graph_edge_json)
        .collect::<Vec<_>>();
    Ok(json!({
        "scope": scope,
        "start": graph_node_json(conn, &start)?,
        "nodes": nodes,
        "edges": selected,
        "diagnostics": {
            "direction": direction,
            "max_depth": max_depth,
            "limit": limit,
            "truncated": truncated,
            "edge_limit": 20_000,
            "frontier_remaining": queue.len(),
            "edge_types": edge_types,
        }
    }))
}

fn graph_macro_explore_value(
    conn: &Connection,
    scope: &str,
    depth: usize,
    limit: usize,
    edge_types: &[String],
) -> Result<Value> {
    let mut statement = conn.prepare(
        "SELECT n.node_id, COUNT(DISTINCT e.edge_id) AS degree
         FROM graph_nodes n
         LEFT JOIN graph_edges e
           ON e.from_node_id = n.node_id OR e.to_node_id = n.node_id
         WHERE n.node_type IN ('document', 'term')
         GROUP BY n.node_id
         ORDER BY degree DESC, n.node_type, n.node_id
         LIMIT ?1",
    )?;
    let representatives = statement
        .query_map(params![limit.min(8) as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if representatives.is_empty() {
        return Ok(json!({
            "scope": scope,
            "keyword_free": true,
            "representatives": [],
            "nodes": [],
            "edges": [],
            "diagnostics": {"truncated": false, "max_depth": depth, "limit": limit},
        }));
    }
    let mut response = graph_explore_value(
        conn,
        scope,
        &representatives[0].0,
        depth,
        limit,
        "both",
        edge_types,
    )?;
    response["keyword_free"] = json!(true);
    response["representatives"] = Value::Array(
        representatives
            .iter()
            .map(|(identifier, degree)| {
                let mut node = graph_node_json(conn, identifier)?;
                node["degree"] = json!(degree);
                Ok(node)
            })
            .collect::<Result<Vec<_>>>()?,
    );
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
fn graph_path_value(
    conn: &Connection,
    scope: &str,
    from: &str,
    to: &str,
    max_depth: usize,
    limit: usize,
    direction: &str,
    requested_edge_types: &[String],
) -> Result<Value> {
    let from = resolve_graph_node(conn, from)?;
    let to = resolve_graph_node(conn, to)?;
    let mut edge_types = validate_graph_edge_types(requested_edge_types)?;
    if edge_types.is_empty() {
        edge_types.extend(
            [
                "LINKS_TO",
                "CITES",
                "REVISION_OF",
                "SUPPORTS",
                "CONTRADICTS",
                "REFINES",
                "SUPERSEDES",
                "CAUSES",
                "DEPENDS_ON",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    let mut queue = VecDeque::from([(from.clone(), 0usize)]);
    let mut visited = BTreeSet::from([from.clone()]);
    let mut previous: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut path_edge_records = BTreeMap::new();
    let mut explored_edges = 0usize;
    let mut truncated = false;
    while let Some((node, depth)) = queue.pop_front() {
        if node == to || depth >= max_depth {
            continue;
        }
        let remaining = 20_000usize.saturating_sub(explored_edges);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let adjacent = load_adjacent_graph_edges(
            conn,
            &node,
            direction,
            &edge_types,
            remaining.saturating_add(1),
        )?;
        if adjacent.len() > remaining {
            truncated = true;
        }
        for edge in adjacent.into_iter().take(remaining) {
            explored_edges += 1;
            let Some(neighbor) = traversed_neighbor(&edge, &node, direction) else {
                continue;
            };
            let neighbor = neighbor.to_string();
            if visited.contains(&neighbor) {
                continue;
            }
            if visited.len() >= limit {
                truncated = true;
                continue;
            }
            visited.insert(neighbor.clone());
            previous.insert(neighbor.clone(), (node.clone(), edge.edge_id.clone()));
            path_edge_records.insert(edge.edge_id.clone(), edge);
            queue.push_back((neighbor, depth + 1));
        }
    }
    let found = visited.contains(&to);
    let mut node_ids = Vec::new();
    let mut edge_ids = Vec::new();
    if found {
        let mut current = to.clone();
        node_ids.push(current.clone());
        while current != from {
            let (prior, edge_id) = previous.get(&current).ok_or_else(|| {
                AppError::new("graph_path_failed", "path predecessor chain is incomplete")
            })?;
            edge_ids.push(edge_id.clone());
            current = prior.clone();
            node_ids.push(current.clone());
        }
        node_ids.reverse();
        edge_ids.reverse();
    }
    let nodes = node_ids
        .iter()
        .map(|node_id| graph_node_json(conn, node_id))
        .collect::<Result<Vec<_>>>()?;
    let path_edges = edge_ids
        .iter()
        .map(|edge_id| {
            path_edge_records
                .get(edge_id)
                .map(graph_edge_json)
                .ok_or_else(|| AppError::new("graph_path_failed", "path edge was not found"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "scope": scope,
        "from": from,
        "to": to,
        "found": found,
        "nodes": nodes,
        "edges": path_edges,
        "hop_count": edge_ids.len(),
        "diagnostics": {
            "direction": direction,
            "max_depth": max_depth,
            "limit": limit,
            "truncated": truncated,
            "visited_nodes": visited.len(),
            "explored_edges": explored_edges,
            "edge_limit": 20_000,
            "frontier_remaining": queue.len(),
            "edge_types": edge_types,
        }
    }))
}

fn graph_impact_value(
    conn: &Connection,
    scope: &str,
    identifier: &str,
    max_depth: usize,
    limit: usize,
) -> Result<Value> {
    let start = resolve_graph_node(conn, identifier)?;
    let impact_types = [
        "DEPENDS_ON",
        "CAUSES",
        "SUPERSEDES",
        "CITES",
        "LINKS_TO",
        "SUPPORTS",
        "CONTRADICTS",
        "REFINES",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    let mut queue = VecDeque::from([(start.clone(), 0usize, 1.0f64)]);
    let mut visited = BTreeSet::from([start.clone()]);
    let mut hard = Vec::new();
    let mut review = Vec::new();
    let mut explored_edges = 0usize;
    let mut truncated = false;
    while let Some((node, depth, parent_score)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let remaining = 20_000usize.saturating_sub(explored_edges);
        if remaining == 0 {
            truncated = true;
            break;
        }
        let adjacent = load_adjacent_graph_edges(
            conn,
            &node,
            "incoming",
            &impact_types,
            remaining.saturating_add(1),
        )?;
        if adjacent.len() > remaining {
            truncated = true;
        }
        for edge in adjacent.into_iter().take(remaining) {
            explored_edges += 1;
            if visited.contains(&edge.from) {
                continue;
            }
            if visited.len() >= limit {
                truncated = true;
                continue;
            }
            visited.insert(edge.from.clone());
            let attenuation = match edge.edge_type.as_str() {
                "DEPENDS_ON" | "SUPERSEDES" => 1.0,
                "CAUSES" => 0.9,
                "CONTRADICTS" => 0.8,
                "SUPPORTS" => 0.75,
                "REFINES" => 0.7,
                "CITES" => 0.6,
                "LINKS_TO" => 0.5,
                _ => 0.0,
            };
            let score = parent_score * attenuation;
            let mut affected = graph_node_json(conn, &edge.from)?;
            affected["depth"] = json!(depth + 1);
            affected["attenuation"] = json!(attenuation);
            affected["score"] = json!(score);
            affected["via"] = graph_edge_json(&edge);
            if matches!(
                edge.edge_type.as_str(),
                "DEPENDS_ON" | "CAUSES" | "SUPERSEDES"
            ) {
                affected["classification"] = json!("hard");
                hard.push(affected);
            } else {
                affected["classification"] = json!("review");
                review.push(affected);
            }
            queue.push_back((edge.from.clone(), depth + 1, score));
        }
    }
    Ok(json!({
        "scope": scope,
        "changed": graph_node_json(conn, &start)?,
        "hard": hard,
        "review": review,
        "diagnostics": {
            "max_depth": max_depth,
            "limit": limit,
            "truncated": truncated,
            "explored_edges": explored_edges,
            "edge_limit": 20_000,
            "frontier_remaining": queue.len(),
        }
    }))
}

fn graph_overview_value(conn: &Connection, scope: &str, limit: usize) -> Result<Value> {
    let mut node_statement = conn.prepare(
        "SELECT node_type, COUNT(*) FROM graph_nodes GROUP BY node_type ORDER BY node_type",
    )?;
    let node_counts = node_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let mut edge_statement = conn.prepare(
        "SELECT edge_type, COUNT(*) FROM graph_edges GROUP BY edge_type ORDER BY edge_type",
    )?;
    let mut edge_counts = edge_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    edge_counts.insert(
        "CONTAINS".to_string(),
        conn.query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE parent_node_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?,
    );
    let peer_edges: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes
         WHERE node_type IN ('passage', 'sentence') AND ordinal > 0",
        [],
        |row| row.get(0),
    )?;
    edge_counts.insert("NEXT".to_string(), peer_edges);
    edge_counts.insert("PREVIOUS".to_string(), peer_edges);
    edge_counts.insert(
        "OCCURS_IN".to_string(),
        conn.query_row("SELECT COUNT(*) FROM graph_occurrences", [], |row| {
            row.get(0)
        })?,
    );
    let mut top_statement = conn.prepare(
        "SELECT n.node_id, n.label, COUNT(*) AS occurrences
         FROM graph_nodes n
         JOIN graph_occurrences o ON o.term_node_id = n.node_id
         WHERE n.node_type = 'term'
         GROUP BY n.node_id, n.label
         ORDER BY occurrences DESC, n.node_id ASC LIMIT ?1",
    )?;
    let top_terms = top_statement
        .query_map(params![limit as i64], |row| {
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "label": row.get::<_, String>(1)?,
                "occurrence_edges": row.get::<_, i64>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut hub_statement = conn.prepare(
        "SELECT n.node_id, n.node_type, n.label, COUNT(DISTINCT e.edge_id) AS degree
         FROM graph_nodes n
         JOIN graph_edges e ON e.from_node_id = n.node_id OR e.to_node_id = n.node_id
         GROUP BY n.node_id, n.node_type, n.label
         ORDER BY degree DESC, n.node_id LIMIT ?1",
    )?;
    let hubs = hub_statement
        .query_map(params![limit as i64], |row| {
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "label": row.get::<_, String>(2)?,
                "degree": row.get::<_, i64>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut orphan_statement = conn.prepare(
        "SELECT n.node_id, n.label FROM graph_nodes n
         WHERE n.node_type = 'document'
           AND NOT EXISTS (
               SELECT 1 FROM graph_edges e
               WHERE (e.from_node_id = n.node_id OR e.to_node_id = n.node_id)
                 AND e.edge_type IN (
                    'LINKS_TO', 'CITES', 'REVISION_OF', 'SUPPORTS', 'CONTRADICTS',
                    'REFINES', 'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
                 )
           )
         ORDER BY n.node_id LIMIT ?1",
    )?;
    let orphans = orphan_statement
        .query_map(params![limit as i64], |row| {
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "label": row.get::<_, String>(1)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let component_count: i64 = conn.query_row(
        "WITH RECURSIVE
         docs(id) AS (SELECT node_id FROM graph_nodes WHERE node_type = 'document'),
         adjacency(a, b) AS (
            SELECT from_node_id, to_node_id FROM graph_edges
            WHERE edge_type IN (
                'LINKS_TO', 'CITES', 'REVISION_OF', 'SUPPORTS', 'CONTRADICTS',
                'REFINES', 'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
            )
            UNION
            SELECT to_node_id, from_node_id FROM graph_edges
            WHERE edge_type IN (
                'LINKS_TO', 'CITES', 'REVISION_OF', 'SUPPORTS', 'CONTRADICTS',
                'REFINES', 'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
            )
         ),
         reach(root, node) AS (
            SELECT id, id FROM docs
            UNION
            SELECT reach.root, adjacency.b
            FROM reach JOIN adjacency ON adjacency.a = reach.node
         ),
         assigned(node, component) AS (
            SELECT node, MIN(root) FROM reach GROUP BY node
         )
         SELECT COUNT(DISTINCT component) FROM assigned",
        [],
        |row| row.get(0),
    )?;
    let mut bridge_statement = conn.prepare(
        "SELECT e.edge_id, e.edge_type, e.from_node_id, e.to_node_id,
                (SELECT COUNT(*) FROM graph_edges x
                 WHERE x.from_node_id = e.from_node_id OR x.to_node_id = e.from_node_id)
              + (SELECT COUNT(*) FROM graph_edges x
                 WHERE x.from_node_id = e.to_node_id OR x.to_node_id = e.to_node_id) AS score
         FROM graph_edges e
         WHERE e.edge_type IN (
            'LINKS_TO', 'CITES', 'REVISION_OF', 'SUPPORTS', 'CONTRADICTS',
            'REFINES', 'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
         )
         ORDER BY score DESC, e.edge_id LIMIT ?1",
    )?;
    let bridge_candidates = bridge_statement
        .query_map(params![limit as i64], |row| {
            Ok(json!({
                "identifier": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "from": row.get::<_, String>(2)?,
                "to": row.get::<_, String>(3)?,
                "connectivity_score": row.get::<_, i64>(4)?,
                "heuristic": true,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut recent_statement = conn.prepare(
        "SELECT generation, action, entity_type, entity_id, created_at
         FROM graph_deltas ORDER BY id DESC LIMIT ?1",
    )?;
    let recent_changes = recent_statement
        .query_map(params![limit as i64], |row| {
            Ok(json!({
                "generation": row.get::<_, i64>(0)?,
                "action": row.get::<_, String>(1)?,
                "entity_type": row.get::<_, String>(2)?,
                "identifier": row.get::<_, String>(3)?,
                "created_at": row.get::<_, String>(4)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let projection = conn.query_row(
        "SELECT engine, canonical_generation, projected_generation, status, updated_at
         FROM graph_projection_state WHERE projection = 'physical'",
        [],
        |row| {
            Ok(json!({
                "engine": row.get::<_, String>(0)?,
                "canonical_generation": row.get::<_, i64>(1)?,
                "projected_generation": row.get::<_, i64>(2)?,
                "status": row.get::<_, String>(3)?,
                "updated_at": row.get::<_, String>(4)?,
            }))
        },
    )?;
    let cooccurrence_truncated: i64 = conn.query_row(
        "SELECT COALESCE(SUM(cooccurrence_truncated), 0) FROM document_index_state",
        [],
        |row| row.get(0),
    )?;
    Ok(json!({
        "scope": scope,
        "node_counts": node_counts,
        "edge_counts": edge_counts,
        "top_terms": top_terms,
        "hubs": hubs,
        "orphans": orphans,
        "component_count": component_count,
        "bridge_candidates": bridge_candidates,
        "recent_changes": recent_changes,
        "cooccurrence_truncated": cooccurrence_truncated,
        "projection": projection,
    }))
}

fn graph_lint_issues(conn: &Connection) -> Result<Vec<LintIssue>> {
    let mut issues = Vec::new();
    let mut missing_statement = conn.prepare(
        "SELECT d.document_type, d.identifier FROM (
            SELECT 'page' AS document_type, slug AS identifier FROM pages
            UNION ALL SELECT 'source', CAST(id AS TEXT) FROM sources
         ) d
         WHERE NOT EXISTS (
            SELECT 1 FROM document_index_state s
            WHERE s.document_type = d.document_type
              AND s.document_identifier = d.identifier
         ) ORDER BY d.document_type, d.identifier",
    )?;
    for row in missing_statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (document_type, identifier) = row?;
        issues.push(LintIssue {
            code: "graph_document_index_missing".to_string(),
            page: (document_type == "page").then_some(identifier.clone()),
            target: Some(format!("{document_type}:{identifier}")),
            message: "canonical document is missing its hierarchy index".to_string(),
        });
    }
    let span_nodes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes WHERE node_type IN ('passage', 'sentence')",
        [],
        |row| row.get(0),
    )?;
    let span_rows: i64 = conn.query_row("SELECT COUNT(*) FROM span_fts", [], |row| row.get(0))?;
    if span_nodes != span_rows {
        issues.push(LintIssue {
            code: "span_index_mismatch".to_string(),
            page: None,
            target: Some(format!("nodes:{span_nodes}/fts:{span_rows}")),
            message: "span FTS rows do not match canonical span nodes".to_string(),
        });
    }
    let mut truncated_statement = conn.prepare(
        "SELECT document_type, document_identifier, cooccurrence_truncated
         FROM document_index_state WHERE cooccurrence_truncated > 0
         ORDER BY document_type, document_identifier",
    )?;
    for row in truncated_statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (document_type, identifier, count) = row?;
        issues.push(LintIssue {
            code: "cooccurrence_truncated".to_string(),
            page: (document_type == "page").then_some(identifier.clone()),
            target: Some(format!("{document_type}:{identifier}")),
            message: format!("{count} sentence co-occurrence windows were truncated"),
        });
    }
    let mut degree_statement = conn.prepare(
        "SELECT from_node_id, COUNT(*) FROM graph_edges
         WHERE edge_type = 'CO_OCCURS'
         GROUP BY from_node_id HAVING COUNT(*) > 32
         ORDER BY from_node_id",
    )?;
    for row in degree_statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (identifier, count) = row?;
        issues.push(LintIssue {
            code: "cooccurrence_degree_exceeded".to_string(),
            page: None,
            target: Some(identifier),
            message: format!("term has {count} outgoing co-occurrence edges; maximum is 32"),
        });
    }
    let projection: (String, i64, i64) = conn.query_row(
        "SELECT status, canonical_generation, projected_generation
         FROM graph_projection_state WHERE projection = 'physical'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if projection.0 == "stale" || projection.0 == "pending" || projection.1 != projection.2 {
        issues.push(LintIssue {
            code: "graph_projection_stale".to_string(),
            page: None,
            target: Some(format!("{}:{}", projection.1, projection.2)),
            message: "physical graph projection is not current".to_string(),
        });
    }
    if let Some((generation, expected)) = conn
        .query_row(
            "SELECT generation, canonical_digest FROM graph_generations
             ORDER BY generation DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        let actual = database_graph_digest(conn)?;
        issues.extend((actual != expected).then(|| LintIssue {
            code: "graph_digest_mismatch".to_string(),
            page: None,
            target: Some(generation.to_string()),
            message: "latest generation digest does not match canonical graph".to_string(),
        }));
    }
    Ok(issues)
}

fn graph_status_value(conn: &Connection, scope: &str, database: &Path) -> Result<Value> {
    let mut projection = conn.query_row(
        "SELECT engine, schema_version, canonical_generation, projected_generation,
                status, last_error_code, last_error_message, updated_at
         FROM graph_projection_state WHERE projection = 'physical'",
        [],
        |row| {
            Ok(json!({
                "engine": row.get::<_, String>(0)?,
                "schema_version": row.get::<_, i64>(1)?,
                "canonical_generation": row.get::<_, i64>(2)?,
                "projected_generation": row.get::<_, i64>(3)?,
                "status": row.get::<_, String>(4)?,
                "last_error_code": row.get::<_, Option<String>>(5)?,
                "last_error_message": row.get::<_, Option<String>>(6)?,
                "updated_at": row.get::<_, String>(7)?,
            }))
        },
    )?;
    let projected_generation = projection["projected_generation"].as_i64().unwrap_or(0);
    projection["projected_digest"] = if projected_generation == 0 {
        Value::Null
    } else {
        conn.query_row(
            "SELECT canonical_digest FROM graph_generations WHERE generation = ?1",
            [projected_generation],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(Value::String)
        .unwrap_or(Value::Null)
    };
    let counts = json!({
        "nodes": conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get::<_, i64>(0))?,
        "edges": conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get::<_, i64>(0))?,
        "stored_edges": conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get::<_, i64>(0))?,
        "occurrence_postings": conn.query_row("SELECT COUNT(*) FROM graph_occurrences", [], |row| row.get::<_, i64>(0))?,
        "documents": conn.query_row("SELECT COUNT(*) FROM document_index_state", [], |row| row.get::<_, i64>(0))?,
        "generations": conn.query_row("SELECT COUNT(*) FROM graph_generations", [], |row| row.get::<_, i64>(0))?,
        "deltas": conn.query_row("SELECT COUNT(*) FROM graph_deltas", [], |row| row.get::<_, i64>(0))?,
        "cooccurrence_truncated": conn.query_row("SELECT COALESCE(SUM(cooccurrence_truncated), 0) FROM document_index_state", [], |row| row.get::<_, i64>(0))?,
    });
    let mut node_statement = conn.prepare(
        "SELECT node_type, COUNT(*) FROM graph_nodes GROUP BY node_type ORDER BY node_type",
    )?;
    let node_counts = node_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let mut edge_statement = conn.prepare(
        "SELECT edge_type, COUNT(*) FROM graph_edges GROUP BY edge_type ORDER BY edge_type",
    )?;
    let edge_counts = edge_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let generation = conn
        .query_row(
            "SELECT generation, canonical_digest FROM graph_generations
             ORDER BY generation DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let generation_range = conn.query_row(
        "SELECT MIN(generation), MAX(generation) FROM graph_generations",
        [],
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    let delta_range = conn.query_row(
        "SELECT MIN(generation), MAX(generation) FROM graph_deltas",
        [],
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    )?;
    let effective_config = config::resolve(scope, database)?;
    let retained_sidecars = database
        .parent()
        .and_then(|parent| fs::read_dir(parent).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("graph-graphqlite-g") && name.ends_with(".db"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let projection_status = projection["status"].as_str().unwrap_or("stale").to_string();
    let resume_available = matches!(projection_status.as_str(), "stale" | "pending");
    Ok(json!({
        "scope": scope,
        "database": database,
        "format_version": USER_VERSION,
        "segmenter_version": crate::segment::SEGMENTER_VERSION,
        "config": effective_config,
        "projection": projection,
        "canonical": {
            "generation": generation.as_ref().map(|value| value.0).unwrap_or(0),
            "digest": generation.as_ref().map(|value| value.1.clone()).unwrap_or_else(|| database_graph_digest(conn).unwrap_or_default()),
            "generation_range": {"first": generation_range.0, "last": generation_range.1},
            "delta_generation_range": {"first": delta_range.0, "last": delta_range.1},
        },
        "counts": counts,
        "node_counts": node_counts,
        "edge_counts": edge_counts,
        "resume_available": resume_available,
        "rebuild_required": projection_status == "stale",
        "recovery_command": "lwc config set --engine graphqlite",
        "retained_sidecars": retained_sidecars,
        "sidecar_cleanup": "manual-review-required",
    }))
}

fn graph_verify_value(conn: &Connection, scope: &str, database: &Path) -> Result<Value> {
    let mut issues = Vec::new();
    let foreign_key_issues: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if foreign_key_issues > 0 {
        issues.push(json!({"code": "graph_foreign_key_violation", "count": foreign_key_issues}));
    }
    let missing_documents: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT 'page' AS document_type, slug AS identifier FROM pages
            UNION ALL SELECT 'source', CAST(id AS TEXT) FROM sources
         ) d
         WHERE NOT EXISTS (
            SELECT 1 FROM document_index_state s
            WHERE s.document_type = d.document_type
              AND s.document_identifier = d.identifier
         )",
        [],
        |row| row.get(0),
    )?;
    if missing_documents > 0 {
        issues.push(json!({"code": "graph_document_index_missing", "count": missing_documents}));
    }
    let span_index_mismatch: i64 = conn.query_row(
        "SELECT ABS(
            (SELECT COUNT(*) FROM graph_nodes WHERE node_type IN ('passage', 'sentence'))
            - (SELECT COUNT(*) FROM span_fts)
         )",
        [],
        |row| row.get(0),
    )?;
    if span_index_mismatch > 0 {
        issues.push(json!({"code": "span_index_mismatch", "count": span_index_mismatch}));
    }
    let cooccurrence_overflow: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT from_node_id FROM graph_edges
            WHERE edge_type = 'CO_OCCURS'
            GROUP BY from_node_id HAVING COUNT(*) > 32
         )",
        [],
        |row| row.get(0),
    )?;
    if cooccurrence_overflow > 0 {
        issues
            .push(json!({"code": "cooccurrence_degree_exceeded", "count": cooccurrence_overflow}));
    }
    let hierarchy_errors: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_nodes n
         LEFT JOIN graph_nodes parent ON parent.node_id = n.parent_node_id
         LEFT JOIN document_index_state state
           ON state.document_type = n.document_type
          AND state.document_identifier = n.document_identifier
         LEFT JOIN pages p
           ON n.document_type = 'page' AND p.slug = n.document_identifier
         LEFT JOIN sources s
           ON n.document_type = 'source' AND CAST(s.id AS TEXT) = n.document_identifier
         WHERE n.node_type IN ('passage', 'sentence')
           AND (
             parent.node_id IS NULL
             OR (n.node_type = 'passage' AND parent.node_type <> 'document')
             OR (n.node_type = 'sentence' AND parent.node_type <> 'passage')
             OR n.content_fingerprint <> state.content_fingerprint
             OR n.segmenter_version <> state.segmenter_version
             OR n.byte_start < 0 OR n.byte_end <= n.byte_start
             OR n.byte_end > CASE n.document_type
                 WHEN 'page' THEN LENGTH(CAST(p.body AS BLOB))
                 ELSE LENGTH(CAST(s.content AS BLOB)) END
           )",
        [],
        |row| row.get(0),
    )?;
    if hierarchy_errors > 0 {
        issues.push(json!({"code": "hierarchy_invariant_violation", "count": hierarchy_errors}));
    }
    let mut occurrence_statement = conn.prepare(
        "SELECT term_node_id || ':' || document_type || ':' || document_identifier,
                frequency, positions, first_position
         FROM graph_occurrences
         ORDER BY term_node_id, document_type, document_identifier",
    )?;
    let occurrences = occurrence_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let occurrence_errors = occurrences
        .iter()
        .filter(|(_, frequency, encoded, first)| {
            decode_positions(encoded).map_or(true, |positions| {
                positions.len() != *frequency as usize
                    || positions.first().map(|range| range.start as i64) != Some(*first)
            })
        })
        .count();
    if occurrence_errors > 0 {
        issues.push(json!({"code": "occurrence_position_invalid", "count": occurrence_errors}));
    }
    let (contribution_errors, contribution_aggregate) = {
        let mut statement = conn.prepare(
            "SELECT contributions FROM term_pair_contributions
             ORDER BY document_type, document_identifier",
        )?;
        let encoded = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut errors = 0usize;
        let mut aggregate: BTreeMap<(String, String), (f64, usize)> = BTreeMap::new();
        for encoded in encoded {
            match decode_term_pair_contributions(&encoded) {
                Ok(contributions) => {
                    for contribution in contributions {
                        let total = aggregate
                            .entry((contribution.from_term_id, contribution.to_term_id))
                            .or_default();
                        total.0 += contribution.sentence_weight + contribution.passage_weight;
                        total.1 += contribution.witness_count;
                    }
                }
                Err(_) => errors += 1,
            }
        }
        (errors, aggregate)
    };
    if contribution_errors > 0 {
        issues.push(json!({
            "code": "cooccurrence_contribution_invalid",
            "count": contribution_errors,
        }));
    } else {
        let totals = {
            let mut statement = conn.prepare(
                "SELECT from_term_id, to_term_id, raw_strength, witness_count
                 FROM term_pair_totals ORDER BY from_term_id, to_term_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                        (row.get::<_, f64>(2)?, row.get::<_, i64>(3)? as usize),
                    ))
                })?
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
        };
        let totals_match = contribution_aggregate.len() == totals.len()
            && contribution_aggregate.iter().all(|(pair, expected)| {
                totals.get(pair).is_some_and(|actual| {
                    (expected.0 - actual.0).abs() <= 1e-9 && expected.1 == actual.1
                })
            });
        if !totals_match {
            issues.push(json!({"code": "cooccurrence_totals_mismatch", "count": 1}));
        } else {
            let contributions = totals
                .iter()
                .map(|((from, to), (strength, witnesses))| TermPairContribution {
                    from_term_id: from.clone(),
                    to_term_id: to.clone(),
                    sentence_weight: *strength,
                    passage_weight: 0.0,
                    witness_count: *witnesses,
                })
                .collect::<Vec<_>>();
            let expected = rank_cooccurrence(&contributions, 32)
                .into_iter()
                .map(|edge| {
                    (
                        (edge.from_term_id, edge.to_term_id),
                        (
                            persisted_cooccurrence_weight(edge.normalized_strength),
                            edge.rank,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let actual = {
                let mut statement = conn.prepare(
                    "SELECT from_node_id, to_node_id, weight,
                            CAST(json_extract(properties_json, '$.rank') AS INTEGER)
                     FROM graph_edges WHERE edge_type = 'CO_OCCURS'
                     ORDER BY from_node_id, to_node_id",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((
                            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                            (row.get::<_, f64>(2)?, row.get::<_, i64>(3)? as usize),
                        ))
                    })?
                    .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
            };
            if expected != actual {
                issues.push(json!({
                    "code": "cooccurrence_projection_mismatch",
                    "count": 1,
                    "expected_edges": expected.len(),
                    "actual_edges": actual.len(),
                    "missing": expected.keys().filter(|key| !actual.contains_key(*key)).count(),
                    "unexpected": actual.keys().filter(|key| !expected.contains_key(*key)).count(),
                    "value_mismatches": expected.iter().filter(|(key, value)| {
                        actual.get(*key).is_some_and(|actual| actual != *value)
                    }).count(),
                }));
            }
        }
    }
    let automatic_owner_errors: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_edges
         WHERE edge_type IN (
            'CONTAINS', 'NEXT', 'PREVIOUS', 'OCCURS_IN', 'LINKS_TO', 'CITES',
            'REVISION_OF', 'CO_OCCURS'
         ) AND (owner_type = 'manual' OR provenance <> 'automatic')",
        [],
        |row| row.get(0),
    )?;
    if automatic_owner_errors > 0 {
        issues
            .push(json!({"code": "automatic_edge_owner_invalid", "count": automatic_owner_errors}));
    }
    let semantic_errors: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_edges
         WHERE edge_type IN (
            'SUPPORTS', 'CONTRADICTS', 'REFINES', 'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
         ) AND (
            owner_type <> 'manual' OR provenance IS NULL OR provenance = 'automatic'
            OR TRIM(COALESCE(reason, '')) = '' OR confidence IS NULL
            OR (provenance = 'source-grounded' AND (
                json_array_length(COALESCE(json_extract(properties_json, '$.source_ids'), '[]')) = 0
                OR EXISTS (
                    SELECT 1 FROM json_each(properties_json, '$.source_ids') ids
                    WHERE NOT EXISTS (SELECT 1 FROM sources s WHERE s.id = ids.value)
                )
            ))
         )",
        [],
        |row| row.get(0),
    )?;
    if semantic_errors > 0 {
        issues.push(json!({"code": "semantic_relation_invalid", "count": semantic_errors}));
    }
    let orphan_deltas: i64 = conn.query_row(
        "SELECT COUNT(*) FROM graph_deltas d
         WHERE NOT EXISTS (
            SELECT 1 FROM graph_generations g WHERE g.generation = d.generation
         )",
        [],
        |row| row.get(0),
    )?;
    if orphan_deltas > 0 {
        issues.push(json!({"code": "graph_delta_generation_missing", "count": orphan_deltas}));
    }
    let latest = conn
        .query_row(
            "SELECT generation, canonical_digest FROM graph_generations
         ORDER BY generation DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((generation, expected)) = latest {
        let actual = database_graph_digest(conn)?;
        if actual != expected {
            issues.push(json!({
                "code": "graph_digest_mismatch",
                "generation": generation,
                "expected": expected,
                "actual": actual,
            }));
        }
    }
    if let Err(error) = verify_graph_projection(conn, database) {
        issues.push(json!({"code": error.code, "details": error.details}));
    }
    Ok(json!({
        "scope": scope,
        "valid": issues.is_empty(),
        "issues": issues,
    }))
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
    const SEMANTIC_TYPES: [&str; 6] = [
        "SUPPORTS",
        "CONTRADICTS",
        "REFINES",
        "SUPERSEDES",
        "CAUSES",
        "DEPENDS_ON",
    ];
    const PROVENANCE: [&str; 4] = [
        "source-grounded",
        "user-provided",
        "agent-observed",
        "hypothesis",
    ];
    let relation_type = relation_type.trim().to_uppercase();
    let provenance = provenance.trim().to_lowercase();
    let reason = reason.trim();
    if !SEMANTIC_TYPES.contains(&relation_type.as_str()) {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "semantic relation must be SUPPORTS, CONTRADICTS, REFINES, SUPERSEDES, CAUSES, or DEPENDS_ON",
        ));
    }
    if !PROVENANCE.contains(&provenance.as_str()) {
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
    let source_ids = dedupe_i64(source_ids.to_vec());
    if provenance == "source-grounded" && source_ids.is_empty() {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "source-grounded semantic relations require at least one --source",
        ));
    }
    if !(0.0..=1.0).contains(&confidence) || !confidence.is_finite() {
        return Err(AppError::new(
            "invalid_confidence",
            "confidence must be a finite value between 0 and 1",
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
    let edge_id = format!(
        "edge:{}",
        hash_content(&format!("manual\0{relation_type}\0{from}\0{to}"))
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    validate_sources(&tx, &source_ids)?;
    let before_record = singleton_graph_record(
        "edge",
        &edge_id,
        canonical_graph_record(&tx, "edge", &edge_id)?,
    );
    let before_json = tx
        .query_row(
            "SELECT json_object(
                'identifier', edge_id, 'type', edge_type, 'from', from_node_id,
                'to', to_node_id, 'confidence', confidence,
                'provenance', provenance, 'reason', reason,
                'source_ids', json(json_extract(properties_json, '$.source_ids'))
             ) FROM graph_edges WHERE edge_id = ?1",
            params![&edge_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    tx.execute(
        &format!(
            "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier, confidence, provenance, reason,
                properties_json, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'manual', ?1, ?5, ?6, ?7, ?8,
                       {TIMESTAMP_SQL}, {TIMESTAMP_SQL})
             ON CONFLICT(edge_id) DO UPDATE SET
                confidence = excluded.confidence,
                provenance = excluded.provenance,
                reason = excluded.reason,
                properties_json = excluded.properties_json,
                updated_at = excluded.updated_at"
        ),
        params![
            &edge_id,
            &relation_type,
            &from,
            &to,
            confidence,
            &provenance,
            reason,
            json!({"source_ids": source_ids}).to_string(),
        ],
    )?;
    let after_record = singleton_graph_record(
        "edge",
        &edge_id,
        canonical_graph_record(&tx, "edge", &edge_id)?,
    );
    let after_json = json!({
        "identifier": edge_id,
        "type": relation_type,
        "from": from,
        "to": to,
        "confidence": confidence,
        "provenance": provenance,
        "reason": reason,
        "source_ids": source_ids,
    })
    .to_string();
    let store_revision = record_operation(
        &tx,
        "graph_relation_set",
        &edge_id,
        &serde_json::from_str(&after_json).unwrap_or(Value::Null),
    )?;
    let generation: i64 = tx.query_row(
        "SELECT COALESCE(MAX(generation), 0) + 1 FROM graph_generations",
        [],
        |row| row.get(0),
    )?;
    let digest = apply_graph_digest_patch(&tx, &before_record, &after_record)?;
    tx.execute(
        &format!(
            "INSERT INTO graph_generations(
                generation, store_revision, canonical_digest,
                changed_document_count, created_at
             ) VALUES (?1, ?2, ?3, 0, {TIMESTAMP_SQL})"
        ),
        params![generation, store_revision, digest],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO graph_deltas(
                generation, action, entity_type, entity_id,
                before_json, after_json, created_at
             ) VALUES (?1, ?2, 'edge', ?3, ?4, ?5, {TIMESTAMP_SQL})"
        ),
        params![
            generation,
            if before_json.is_some() {
                "update"
            } else {
                "add"
            },
            &edge_id,
            before_json,
            &after_json,
        ],
    )?;
    tx.execute(
        &format!(
            "UPDATE graph_projection_state
             SET canonical_generation = ?1,
                 projected_generation = CASE
                     WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                 status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                 updated_at = {TIMESTAMP_SQL}
             WHERE projection = 'physical'"
        ),
        params![generation],
    )?;
    tx.commit()?;
    Ok(json!({
        "scope": scope,
        "relation": serde_json::from_str::<Value>(&after_json)
            .unwrap_or(Value::Null),
        "generation": generation,
    }))
}

fn graph_relation_list_value(
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
    if relation_type.as_deref().is_some_and(|value| {
        !matches!(
            value,
            "SUPPORTS" | "CONTRADICTS" | "REFINES" | "SUPERSEDES" | "CAUSES" | "DEPENDS_ON"
        )
    }) {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "semantic relation type is not supported",
        ));
    }
    let mut statement = conn.prepare(
        "SELECT edge_id, edge_type, from_node_id, to_node_id,
                confidence, provenance, reason, properties_json
         FROM graph_edges
         WHERE owner_type = 'manual'
           AND (?1 IS NULL OR from_node_id = ?1)
           AND (?2 IS NULL OR to_node_id = ?2)
           AND (?3 IS NULL OR edge_type = ?3)
         ORDER BY edge_type, from_node_id, to_node_id, edge_id
         LIMIT ?4",
    )?;
    let rows = statement
        .query_map(params![from, to, relation_type, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let relations = rows
        .into_iter()
        .map(
            |(identifier, relation_type, from, to, confidence, provenance, reason, properties)| {
                let properties = serde_json::from_str::<Value>(&properties).unwrap_or(Value::Null);
                json!({
                    "identifier": identifier,
                    "type": relation_type,
                    "from": from,
                    "to": to,
                    "confidence": confidence,
                    "provenance": provenance,
                    "reason": reason,
                    "source_ids": properties.get("source_ids").cloned().unwrap_or(json!([])),
                })
            },
        )
        .collect::<Vec<_>>();
    Ok(json!({"scope": scope, "relations": relations, "limit": limit}))
}

fn graph_relation_retract_value(
    conn: &mut Connection,
    scope: &str,
    from: &str,
    relation_type: &str,
    to: &str,
    reason: &str,
) -> Result<Value> {
    let relation_type = relation_type.trim().to_uppercase();
    if !matches!(
        relation_type.as_str(),
        "SUPPORTS" | "CONTRADICTS" | "REFINES" | "SUPERSEDES" | "CAUSES" | "DEPENDS_ON"
    ) {
        return Err(AppError::new(
            "invalid_semantic_relation",
            "only explicit semantic relations can be retracted",
        ));
    }
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            "semantic relation retraction reason cannot be empty",
        ));
    }
    let from = resolve_graph_node(conn, from)?;
    let to = resolve_graph_node(conn, to)?;
    let edge_id = format!(
        "edge:{}",
        hash_content(&format!("manual\0{relation_type}\0{from}\0{to}"))
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before_record = singleton_graph_record(
        "edge",
        &edge_id,
        canonical_graph_record(&tx, "edge", &edge_id)?,
    );
    let before_json = tx
        .query_row(
            "SELECT json_object(
                'identifier', edge_id, 'type', edge_type, 'from', from_node_id,
                'to', to_node_id, 'confidence', confidence,
                'provenance', provenance, 'reason', reason,
                'source_ids', json(json_extract(properties_json, '$.source_ids'))
             ) FROM graph_edges
             WHERE edge_id = ?1 AND owner_type = 'manual'",
            params![&edge_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            AppError::new(
                "semantic_relation_not_found",
                "explicit semantic relation was not found",
            )
        })?;
    tx.execute(
        "DELETE FROM graph_edges WHERE edge_id = ?1 AND owner_type = 'manual'",
        params![&edge_id],
    )?;
    let digest = apply_graph_digest_patch(&tx, &before_record, &BTreeMap::new())?;
    let store_revision = record_operation(
        &tx,
        "graph_relation_retract",
        &edge_id,
        &json!({"reason": reason}),
    )?;
    let generation: i64 = tx.query_row(
        "SELECT COALESCE(MAX(generation), 0) + 1 FROM graph_generations",
        [],
        |row| row.get(0),
    )?;
    tx.execute(
        &format!(
            "INSERT INTO graph_generations(
                generation, store_revision, canonical_digest,
                changed_document_count, created_at
             ) VALUES (?1, ?2, ?3, 0, {TIMESTAMP_SQL})"
        ),
        params![generation, store_revision, digest],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO graph_deltas(
                generation, action, entity_type, entity_id,
                before_json, created_at
             ) VALUES (?1, 'remove', 'edge', ?2, ?3, {TIMESTAMP_SQL})"
        ),
        params![generation, &edge_id, before_json],
    )?;
    tx.execute(
        &format!(
            "UPDATE graph_projection_state
             SET canonical_generation = ?1,
                 projected_generation = CASE
                     WHEN engine = 'rslg' THEN ?1 ELSE projected_generation END,
                 status = CASE WHEN engine = 'rslg' THEN 'fresh' ELSE 'pending' END,
                 updated_at = {TIMESTAMP_SQL}
             WHERE projection = 'physical'"
        ),
        params![generation],
    )?;
    tx.commit()?;
    Ok(json!({
        "scope": scope,
        "identifier": edge_id,
        "retracted": true,
        "reason": reason,
        "generation": generation,
    }))
}

fn load_span_record(conn: &Connection, identifier: &str) -> Result<SpanRecord> {
    let row = conn
        .query_row(
            "SELECT n.node_id, n.node_type, n.document_type, n.document_identifier,
                    n.parent_node_id, n.ordinal, n.byte_start, n.byte_end,
                    n.content_fingerprint, n.segmenter_version,
                    CASE n.document_type WHEN 'page' THEN p.body ELSE s.content END
             FROM graph_nodes n
             LEFT JOIN pages p
               ON n.document_type = 'page' AND p.slug = n.document_identifier
             LEFT JOIN sources s
               ON n.document_type = 'source'
              AND s.id = CAST(n.document_identifier AS INTEGER)
             WHERE n.node_id = ?1 AND n.node_type IN ('passage', 'sentence')",
            params![identifier],
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
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
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
        content,
    )) = row
    else {
        let history = conn
            .query_row(
                "SELECT before_json FROM graph_deltas
                 WHERE entity_type = 'node' AND entity_id = ?1 AND action = 'remove'
                   AND before_json IS NOT NULL
                 ORDER BY id DESC LIMIT 1",
                params![identifier],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(history) = history else {
            return Err(AppError::new(
                "span_not_found",
                "span locator was not found",
            ));
        };
        let prior = serde_json::from_str::<Value>(&history).unwrap_or(Value::Null);
        let document_type = prior
            .get("document_type")
            .and_then(Value::as_str)
            .map(str::to_string);
        let document_identifier = prior
            .get("document_identifier")
            .and_then(Value::as_str)
            .map(str::to_string);
        let current = match (document_type.as_deref(), document_identifier.as_deref()) {
            (Some(document_type), Some(document_identifier)) => conn
                .query_row(
                    "SELECT content_fingerprint, segmenter_version
                     FROM document_index_state
                     WHERE document_type = ?1 AND document_identifier = ?2",
                    params![document_type, document_identifier],
                    |row| {
                        Ok(json!({
                            "content_fingerprint": row.get::<_, String>(0)?,
                            "segmenter_version": row.get::<_, i64>(1)?,
                        }))
                    },
                )
                .optional()?,
            _ => None,
        };
        return Err(AppError::new(
            "stale_span",
            "span locator belongs to an older document fingerprint",
        )
        .with_details(json!({
            "identifier": identifier,
            "document": {
                "type": document_type,
                "identifier": document_identifier,
            },
            "prior": {
                "content_fingerprint": prior.get("content_fingerprint"),
                "segmenter_version": prior.get("segmenter_version"),
                "byte_start": prior.get("byte_start"),
                "byte_end": prior.get("byte_end"),
            },
            "current": current,
        })));
    };
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
    if hash_content(&content) != content_fingerprint {
        return Err(AppError::new(
            "stale_span",
            "span fingerprint no longer matches the indexed document",
        ));
    }
    Ok(SpanRecord {
        identifier,
        span_type,
        document: SearchDocumentRef {
            document_type,
            identifier: document_identifier,
        },
        parent_identifier,
        ordinal: ordinal as usize,
        byte_start,
        byte_end,
        content_fingerprint,
        segmenter_version: segmenter_version as u32,
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
            n.label, n.parent_node_id, n.ordinal, n.byte_start, n.byte_end,
            n.content_fingerprint, n.segmenter_version,
            bm25(span_fts, 0.0, 0.0, 0.0, 0.0, 4.0, 2.0, 1.0),
            CASE f.document_type
                WHEN 'page' THEN p.structural_navigation
                ELSE s.structural_navigation
            END,
            CASE f.document_type WHEN 'page' THEN p.body ELSE s.content END
         FROM span_fts f
         JOIN graph_nodes n ON n.node_id = f.span_id
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_store() -> Store {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (store, _) = Store::initialize("project", &database).unwrap();
        std::mem::forget(temp);
        store
    }

    #[test]
    fn v10_hierarchical_migration_builds_one_complete_initial_generation() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('store_revision', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
             INSERT INTO meta VALUES ('format_version', '10');
             CREATE TABLE sources(
                id INTEGER PRIMARY KEY, content_hash TEXT NOT NULL UNIQUE, title TEXT,
                origin TEXT NOT NULL, content TEXT NOT NULL,
                structural_navigation INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL
             );
             CREATE TABLE pages(
                slug TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT, summary TEXT,
                body TEXT NOT NULL, structural_navigation INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
             );
             CREATE TABLE page_sources(page_slug TEXT NOT NULL, source_id INTEGER NOT NULL,
                PRIMARY KEY(page_slug, source_id));
             CREATE TABLE links(from_slug TEXT NOT NULL, to_slug TEXT NOT NULL,
                PRIMARY KEY(from_slug, to_slug));
             CREATE TABLE source_path_revisions(
                tracked_path TEXT NOT NULL, revision INTEGER NOT NULL,
                source_id INTEGER NOT NULL, observed_at TEXT NOT NULL,
                PRIMARY KEY(tracked_path, revision)
             );
             INSERT INTO sources VALUES
                (1, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                 'Evidence', 'evidence.md', 'alpha beta.', 0, 'now'),
                (2, 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                 'Policy', 'policy.md', 'beta gamma.', 0, 'now');
             INSERT INTO pages VALUES
                ('summary', 'Summary', 'source', NULL, 'alpha summary.', 0, 'now', 'now'),
                ('policy', 'Policy', 'concept', NULL, 'beta policy.', 0, 'now', 'now');
             INSERT INTO page_sources VALUES ('summary', 1);
             INSERT INTO page_sources VALUES ('policy', 2);
             INSERT INTO links VALUES ('summary', 'policy');
             PRAGMA user_version = 10;",
        )
        .unwrap();
        for id in 3..=50 {
            let slug = format!("page-{id}");
            conn.execute(
                "INSERT INTO sources VALUES (?1, ?2, ?3, ?4, ?5, 0, 'now')",
                params![
                    id,
                    format!("{id:064x}"),
                    format!("Source {id}"),
                    format!("source-{id}.md"),
                    "shared migration evidence ".repeat(40),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO pages VALUES (?1, ?2, 'concept', NULL, ?3, 0, 'now', 'now')",
                params![
                    &slug,
                    format!("Page {id}"),
                    "shared migration policy ".repeat(40),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO page_sources VALUES (?1, ?2)",
                params![slug, id],
            )
            .unwrap();
        }

        let mut cancelled = Connection::open_in_memory().unwrap();
        {
            let backup = Backup::new(&conn, &mut cancelled).unwrap();
            backup
                .run_to_completion(100, Duration::from_millis(1), None)
                .unwrap();
        }
        let mut progress_calls = 0;
        let error = migrate_hierarchical_graph(
            &mut cancelled,
            Some(&mut |_, _, _| {
                progress_calls += 1;
                Err(AppError::new("work_cancelled", "cancelled by test"))
            }),
        )
        .unwrap_err();
        assert_eq!(error.code, "work_cancelled");
        assert_eq!(progress_calls, 1);
        assert_eq!(
            cancelled
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            10,
            "cancelled migration must roll back the whole transaction"
        );
        assert_eq!(
            cancelled
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'graph_nodes'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        reset_test_graph_build_counts();
        let mut progress_events = Vec::new();
        migrate_hierarchical_graph(
            &mut conn,
            Some(&mut |completed, total, phase| {
                progress_events.push((completed, total, phase.to_string()));
                Ok(())
            }),
        )
        .unwrap();

        assert!(
            !progress_events.iter().any(|(completed, total, phase)| {
                phase == "finalizing-graph" && completed == total
            }),
            "migration reported 100% before graph finalization completed: {progress_events:?}"
        );

        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            11
        );
        assert_eq!(
            conn.query_row("SELECT MAX(generation) FROM graph_generations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE node_type = 'document'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            100
        );
        assert!(
            conn.query_row("SELECT COUNT(*) FROM graph_deltas", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
                > 0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE edge_type = 'CITES'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            50
        );
        assert_eq!(
            test_graph_build_counts(),
            (1, 1),
            "initial migration must finalize canonical digest and co-occurrence once"
        );
    }

    #[test]
    fn new_store_bootstraps_hierarchical_graph_schema_at_v11() {
        let store = test_store();

        assert_eq!(
            store
                .conn
                .pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            11
        );
        for table in [
            "document_index_state",
            "graph_nodes",
            "graph_edges",
            "graph_occurrences",
            "term_pair_contributions",
            "span_fts",
            "graph_generations",
            "graph_deltas",
            "graph_projection_state",
        ] {
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_schema WHERE type IN ('table', 'view') AND name = ?1",
                        [table],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing {table}"
            );
        }
    }

    #[test]
    fn source_add_atomically_builds_hierarchy_occurrences_generation_and_deltas() {
        let mut store = test_store();

        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "docs/evidence.md".to_string(),
                tracked_path: None,
                content: "alpha beta.".to_string(),
            })
            .unwrap();
        let source_id = source.source.id.to_string();
        assert_eq!(source.graph.terms, 2);

        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT document_node_id FROM document_index_state
                     WHERE document_type = 'source' AND document_identifier = ?1",
                    [&source_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            format!("source:{source_id}")
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT MAX(generation) FROM graph_generations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM graph_deltas", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
                > 0
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT canonical_generation, projected_generation, status
                     FROM graph_projection_state WHERE projection = 'physical'",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?
                    )),
                )
                .unwrap(),
            (1, 1, "disabled".to_string())
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_occurrences
                     WHERE document_type = 'source'
                       AND document_identifier = ?1",
                    [source_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_edges
                     WHERE edge_type IN ('CONTAINS', 'NEXT', 'PREVIOUS', 'OCCURS_IN')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        let term_edges = load_adjacent_graph_edges(
            &store.conn,
            "term:alpha",
            "outgoing",
            &BTreeSet::from(["OCCURS_IN".to_string()]),
            20,
        )
        .unwrap();
        assert!(
            term_edges
                .iter()
                .any(|edge| edge.to == format!("source:{source_id}"))
        );
        assert!(term_edges.iter().any(|edge| {
            store
                .conn
                .query_row(
                    "SELECT node_type FROM graph_nodes WHERE node_id = ?1",
                    [&edge.to],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|kind| matches!(kind.as_str(), "passage" | "sentence"))
        }));
        let hierarchy = load_adjacent_graph_edges(
            &store.conn,
            &format!("source:{source_id}"),
            "outgoing",
            &BTreeSet::from(["CONTAINS".to_string()]),
            20,
        )
        .unwrap();
        assert!(!hierarchy.is_empty());
    }

    #[test]
    fn compact_term_pair_contributions_round_trip_and_reject_corruption() {
        let contributions = vec![TermPairContribution {
            from_term_id: "term:alpha".to_string(),
            to_term_id: "term:知识".to_string(),
            sentence_weight: 1.25,
            passage_weight: 0.125,
            witness_count: 7,
        }];
        let encoded = encode_term_pair_contributions(&contributions).unwrap();
        assert!(encoded.len() < 64);
        assert_eq!(
            decode_term_pair_contributions(&encoded).unwrap(),
            contributions
        );
        assert_eq!(
            decode_term_pair_contributions(&encoded[..encoded.len() - 1])
                .unwrap_err()
                .code,
            "graph_index_corrupt"
        );
        let mut oversized_header = vec![1];
        write_compact_u64(&mut oversized_header, 100_001);
        assert_eq!(
            decode_term_pair_contributions(&oversized_header)
                .unwrap_err()
                .code,
            "graph_index_corrupt"
        );

        let oversized = vec![TermPairContribution {
            from_term_id: format!("term:{}", "x".repeat(16 * 1024 * 1024)),
            to_term_id: "term:bounded".to_string(),
            sentence_weight: 1.0,
            passage_weight: 0.0,
            witness_count: 1,
        }];
        assert_eq!(
            encode_term_pair_contributions(&oversized).unwrap_err().code,
            "graph_index_capacity_exceeded"
        );
    }

    #[test]
    fn graph_verify_reports_corrupt_compact_contributions() {
        let mut store = test_store();
        store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "evidence.md".to_string(),
                tracked_path: None,
                content: "alpha beta.".to_string(),
            })
            .unwrap();
        let valid_blob: Vec<u8> = store
            .conn
            .query_row(
                "SELECT contributions FROM term_pair_contributions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE term_pair_contributions SET contributions = X'02'",
                [],
            )
            .unwrap();
        let verified = graph_verify_value(&store.conn, &store.scope, &store.database).unwrap();
        assert!(verified["issues"].as_array().unwrap().iter().any(|issue| {
            issue["code"] == "cooccurrence_contribution_invalid" && issue["count"] == 1
        }));
        store
            .conn
            .execute(
                "UPDATE term_pair_contributions SET contributions = ?1",
                [valid_blob],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE term_pair_totals SET witness_count = witness_count + 1",
                [],
            )
            .unwrap();
        let verified = graph_verify_value(&store.conn, &store.scope, &store.database).unwrap();
        assert!(verified["issues"].as_array().unwrap().iter().any(|issue| {
            issue["code"] == "cooccurrence_totals_mismatch" && issue["count"] == 1
        }));
    }

    #[test]
    fn page_replacement_preserves_document_identity_and_replaces_revision_spans() {
        let mut store = test_store();
        let put = |store: &mut Store, body: &str| {
            store
                .page_put(PagePutInput {
                    slug: "mutable".to_string(),
                    title: "Mutable".to_string(),
                    kind: Some("concept".to_string()),
                    summary: None,
                    body: body.to_string(),
                    source_ids: Vec::new(),
                    provenance: vec!["agent-observed".to_string()],
                })
                .unwrap();
        };

        put(&mut store, "alpha.");
        let old_span: String = store
            .conn
            .query_row(
                "SELECT node_id FROM graph_nodes
                 WHERE document_type = 'page' AND document_identifier = 'mutable'
                   AND node_type = 'sentence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        put(&mut store, "beta.");

        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_nodes WHERE node_id = 'page:mutable'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_nodes WHERE node_id = ?1",
                    [&old_span],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT MAX(generation) FROM graph_generations", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        assert!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_deltas
                     WHERE generation = 2 AND action = 'remove' AND entity_id = 'term:alpha'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
                > 0
        );
        assert!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM graph_nodes WHERE node_id = 'term:beta'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
                > 0
        );
    }

    #[test]
    fn page_graph_adds_only_provable_link_and_citation_edges() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "evidence.md".to_string(),
                tracked_path: None,
                content: "evidence".to_string(),
            })
            .unwrap();
        store
            .page_put(PagePutInput {
                slug: "target".to_string(),
                title: "Target".to_string(),
                kind: None,
                summary: None,
                body: "target".to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        store
            .page_put(PagePutInput {
                slug: "origin".to_string(),
                title: "Origin".to_string(),
                kind: None,
                summary: None,
                body: "See [[target]] and [[missing]].".to_string(),
                source_ids: vec![source.source.id],
                provenance: Vec::new(),
            })
            .unwrap();

        let edges = store
            .conn
            .prepare(
                "SELECT edge_type, from_node_id, to_node_id
                 FROM graph_edges
                 WHERE owner_type = 'page' AND owner_identifier = 'origin'
                   AND edge_type IN ('LINKS_TO', 'CITES')
                 ORDER BY edge_type, to_node_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(
            edges,
            vec![
                (
                    "CITES".to_string(),
                    "page:origin".to_string(),
                    format!("source:{}", source.source.id),
                ),
                (
                    "LINKS_TO".to_string(),
                    "page:origin".to_string(),
                    "page:target".to_string(),
                ),
            ]
        );
    }

    #[test]
    fn duplicate_source_reuses_first_metadata_and_logs_each_attempt() {
        let mut store = test_store();

        let first = store
            .source_add(SourceAddInput {
                title: Some("First".to_string()),
                origin: "/tmp/first.md".to_string(),
                tracked_path: None,
                content: "same bytes".to_string(),
            })
            .unwrap();
        let second = store
            .source_add(SourceAddInput {
                title: Some("Second".to_string()),
                origin: "/tmp/second.md".to_string(),
                tracked_path: None,
                content: "same bytes".to_string(),
            })
            .unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.source.id, second.source.id);
        assert_eq!(second.source.title.as_deref(), Some("First"));
        assert_eq!(second.source.origin, "/tmp/first.md");

        let source_add_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'source_add'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_add_count, 2);
    }

    #[test]
    fn streamed_source_add_rolls_back_on_a_late_input_error() {
        let mut store = test_store();
        let tables = ["sources", "source_path_revisions", "operations"];
        let before = tables.map(|table| {
            store
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        });
        let inputs = std::iter::once(Ok(Some(SourceAddInput {
            title: Some("Safe".to_string()),
            origin: "docs/safe.md".to_string(),
            tracked_path: Some("docs/safe.md".to_string()),
            content: "safe evidence".to_string(),
        })))
        .chain(std::iter::once(Err(AppError::new(
            "possible_secret_detected",
            "late validation failure",
        ))));

        let error = store.source_add_stream(inputs).unwrap_err();

        assert_eq!(error.code, "possible_secret_detected");
        for (table, expected) in tables.into_iter().zip(before) {
            let count: i64 = store
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "{table} must roll back with the batch");
        }
    }

    #[test]
    fn streamed_source_add_does_not_lock_the_database_while_consuming_inputs() {
        let mut store = test_store();
        let probe = Connection::open(&store.database).unwrap();
        probe.busy_timeout(Duration::from_millis(25)).unwrap();
        let inputs = std::iter::once_with(move || {
            probe
                .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
                .map_err(|error| AppError::new("writer_probe_failed", error.to_string()))?;
            Ok(None)
        });

        let responses = store.source_add_stream(inputs).unwrap();

        assert!(responses.is_empty());
    }

    #[test]
    fn source_path_revisions_preserve_a_b_a_observations_with_content_deduplication() {
        let mut store = test_store();
        let path = "docs/source.md";

        let first = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "A".to_string(),
            })
            .unwrap();
        let second = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "B".to_string(),
            })
            .unwrap();
        let third = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "A".to_string(),
            })
            .unwrap();

        assert_eq!(first.source.id, third.source.id);
        assert_ne!(first.source.id, second.source.id);

        let revisions = store
            .conn
            .prepare(
                "SELECT revision, source_id
                 FROM source_path_revisions
                 WHERE tracked_path = ?1
                 ORDER BY revision",
            )
            .unwrap()
            .query_map(params![path], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(
            revisions,
            vec![
                (1, first.source.id),
                (2, second.source.id),
                (3, first.source.id),
            ]
        );
        let lineage = store
            .conn
            .prepare(
                "SELECT from_node_id, to_node_id, owner_identifier
                 FROM graph_edges
                 WHERE edge_type = 'REVISION_OF'
                 ORDER BY owner_identifier",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            lineage,
            vec![
                (
                    format!("source:{}", second.source.id),
                    format!("source:{}", first.source.id),
                    format!("{path}#2"),
                ),
                (
                    format!("source:{}", first.source.id),
                    format!("source:{}", second.source.id),
                    format!("{path}#3"),
                ),
            ]
        );
    }

    #[test]
    fn title_token_coverage_bridges_query_separators() {
        let query = "系统设置 支付渠道管理";
        let explanation = lexical_explanation(
            "page",
            Some("01-系统设置-支付渠道管理.md"),
            "unrelated-slug",
            query,
            &tokenize_for_query(query),
            0.0,
            false,
        );

        assert_eq!(explanation.signals.title_match, 0.9);
        assert_eq!(explanation.contributions.title, -TITLE_WEIGHT * 0.9);
    }

    #[test]
    fn changeset_search_refresh_targets_only_changed_documents() {
        let mut live = test_store();
        for (slug, body) in [("alpha", "old alpha"), ("beta", "unchanged beta")] {
            live.page_put(PagePutInput {
                slug: slug.to_string(),
                title: slug.to_string(),
                kind: None,
                summary: None,
                body: body.to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        }

        let temp = tempdir().unwrap();
        let candidate_path = temp.path().join("candidate.db");
        live.snapshot_to(&candidate_path).unwrap();
        let mut candidate = Store::open("project", &candidate_path).unwrap();
        candidate
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "alpha".to_string(),
                kind: None,
                summary: None,
                body: "new alpha".to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        drop(candidate);

        live.conn
            .execute(
                "ATTACH DATABASE ?1 AS candidate",
                params![candidate_path.to_string_lossy().as_ref()],
            )
            .unwrap();
        let tx = live
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let (sources, pages) = changed_search_documents(&tx, "candidate").unwrap();

        assert!(sources.is_empty());
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, "alpha");
        assert!(pages[0].1.is_some());
    }

    #[test]
    fn concurrent_source_adds_serialize_revisions_for_one_path() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        drop(Store::initialize("project", &database).unwrap().0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        std::thread::scope(|scope| {
            for content in ["A", "B"] {
                let database = database.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let mut store = Store::open("project", database).unwrap();
                    barrier.wait();
                    store
                        .source_add(SourceAddInput {
                            title: Some("Concurrent source".to_string()),
                            origin: "docs/concurrent.md".to_string(),
                            tracked_path: Some("docs/concurrent.md".to_string()),
                            content: content.to_string(),
                        })
                        .unwrap();
                });
            }
        });

        let store = Store::open("project", database).unwrap();
        let revisions = store
            .conn
            .prepare(
                "SELECT revision, source_id
                 FROM source_path_revisions
                 WHERE tracked_path = 'docs/concurrent.md'
                 ORDER BY revision",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].0, 1);
        assert_eq!(revisions[1].0, 2);
        assert_ne!(revisions[0].1, revisions[1].1);
    }

    #[test]
    fn page_put_deduplicates_repeated_links_and_source_ids() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
                tracked_path: None,
                content: "page evidence".to_string(),
            })
            .unwrap();

        let page = store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Alpha".to_string(),
                kind: Some("concept".to_string()),
                summary: Some("Alpha summary".to_string()),
                body: "See [[beta]] and [[beta]] and [[gamma]].".to_string(),
                source_ids: vec![source.source.id, source.source.id],
                provenance: vec![
                    "hypothesis".to_string(),
                    "agent-observed".to_string(),
                    "hypothesis".to_string(),
                ],
            })
            .unwrap();

        assert_eq!(page.page.source_ids, vec![source.source.id]);
        assert_eq!(
            page.page.provenance,
            vec![
                "source-grounded".to_string(),
                "agent-observed".to_string(),
                "hypothesis".to_string(),
            ]
        );
        assert_eq!(
            page.page.links,
            vec!["beta".to_string(), "gamma".to_string()]
        );

        let relation_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM page_sources WHERE page_slug = 'alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let link_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE from_slug = 'alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(relation_count, 1);
        assert_eq!(link_count, 2);
    }

    #[test]
    fn reverse_dependent_cooccurrence_changes_capture_complete_before_deltas() {
        let mut store = test_store();
        let put = |store: &mut Store, slug: &str, body: &str| {
            store
                .page_put(PagePutInput {
                    slug: slug.to_string(),
                    title: slug.to_string(),
                    kind: Some("concept".to_string()),
                    summary: None,
                    body: body.to_string(),
                    source_ids: Vec::new(),
                    provenance: vec!["agent-observed".to_string()],
                })
                .unwrap()
        };
        put(&mut store, "a", "alpha beta shared.");
        put(&mut store, "b", "beta gamma shared.");
        let reverse_edge: String = store
            .conn
            .query_row(
                "SELECT edge_id FROM graph_edges
                 WHERE edge_type = 'CO_OCCURS'
                   AND from_node_id = 'term:gamma' AND to_node_id = 'term:beta'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let generation = put(&mut store, "a", "alpha delta shared.").graph.generation;
        let delta = store
            .conn
            .query_row(
                "SELECT action, before_json, after_json FROM graph_deltas
                 WHERE generation = ?1 AND entity_id = ?2 ORDER BY id DESC LIMIT 1",
                params![generation, reverse_edge],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_ne!(delta.0, "add");
        assert!(delta.1.is_some());
        assert!(delta.0 == "remove" || delta.2.is_some());
        assert_eq!(
            graph_verify_value(&store.conn, &store.scope, &store.database).unwrap()["valid"],
            true
        );
    }

    #[test]
    fn point_page_mutation_never_recomputes_the_complete_graph_digest() {
        let mut store = test_store();
        reset_test_graph_build_counts();

        store
            .page_put(PagePutInput {
                slug: "bounded-update".to_string(),
                title: "Bounded update".to_string(),
                kind: Some("concept".to_string()),
                summary: None,
                body: "alpha beta gamma local evidence.".to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();

        let (complete_digest_calls, _) = test_graph_build_counts();
        assert_eq!(
            complete_digest_calls, 0,
            "a one-page mutation must update digest state from its exact old/new records"
        );
    }

    #[test]
    fn point_page_mutation_never_loads_and_ranks_all_term_pairs() {
        let mut store = test_store();
        for index in 0..8 {
            store
                .page_put(PagePutInput {
                    slug: format!("unrelated-{index}"),
                    title: format!("Unrelated {index}"),
                    kind: Some("concept".to_string()),
                    summary: None,
                    body: format!("unrelated{index} evidence{index} context{index}."),
                    source_ids: Vec::new(),
                    provenance: vec!["agent-observed".to_string()],
                })
                .unwrap();
        }
        reset_test_graph_build_counts();

        store
            .page_put(PagePutInput {
                slug: "local-pairs".to_string(),
                title: "Local pairs".to_string(),
                kind: Some("concept".to_string()),
                summary: None,
                body: "localalpha localbeta localgamma.".to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();

        assert_eq!(
            test_global_term_pair_loads(),
            0,
            "a point mutation must query and rank only affected source terms"
        );
    }

    #[test]
    fn identical_page_put_is_a_true_noop() {
        let mut store = test_store();
        let input = PagePutInput {
            slug: "stable-page".to_string(),
            title: "Stable page".to_string(),
            kind: Some("concept".to_string()),
            summary: Some("Stable summary".to_string()),
            body: "stable alpha beta evidence.".to_string(),
            source_ids: Vec::new(),
            provenance: vec!["agent-observed".to_string()],
        };
        store.page_put(input.clone()).unwrap();
        let generation_before: i64 = store
            .conn
            .query_row("SELECT MAX(generation) FROM graph_generations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let operations_before: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        reset_test_graph_build_counts();

        let response = store.page_put(input).unwrap();

        let generation_after: i64 = store
            .conn
            .query_row("SELECT MAX(generation) FROM graph_generations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let operations_after: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!response.created);
        assert_eq!(generation_after, generation_before);
        assert_eq!(operations_after, operations_before);
        assert_eq!(test_graph_build_counts(), (0, 0));
    }

    #[test]
    fn failed_page_update_leaves_page_relations_fts_and_log_unchanged() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
                tracked_path: None,
                content: "page evidence".to_string(),
            })
            .unwrap();

        store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Alpha".to_string(),
                kind: None,
                summary: Some("summary".to_string()),
                body: "oldterm with [[beta]]".to_string(),
                source_ids: vec![source.source.id],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();

        let page_put_count_before: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let error = store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Replacement".to_string(),
                kind: None,
                summary: Some("replacement".to_string()),
                body: "newterm with [[gamma]]".to_string(),
                source_ids: vec![9_999],
                provenance: vec!["hypothesis".to_string()],
            })
            .unwrap_err();
        assert_eq!(error.code, "source_not_found");

        let page = store.page_show("alpha").unwrap().page;
        assert_eq!(page.title, "Alpha");
        assert_eq!(page.links, vec!["beta".to_string()]);
        assert_eq!(page.source_ids, vec![source.source.id]);
        assert_eq!(
            page.provenance,
            vec!["source-grounded".to_string(), "agent-observed".to_string()]
        );

        let old_search = store.search("oldterm", 10).unwrap();
        assert_eq!(old_search.results.len(), 1);
        assert_eq!(old_search.results[0].identifier, "alpha");

        let new_search = store.search("newterm", 10).unwrap();
        assert!(new_search.results.is_empty());

        let page_put_count_after: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(page_put_count_after, page_put_count_before);
    }

    #[test]
    fn readonly_open_sees_fresh_wal_commits_from_live_writer() {
        let store = test_store();
        let database = store.database.clone();

        store
            .conn
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO meta(key, value) VALUES ('purpose', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params!["fresh-from-wal"],
            )
            .unwrap();

        let reader = Store::open_read_only("project", &database).unwrap();
        assert_eq!(
            reader.purpose_show().unwrap().purpose,
            Some("fresh-from-wal".to_string())
        );
    }

    #[test]
    fn concurrent_open_migrates_v1_once_and_preserves_searchable_data() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (mut store, _) = Store::initialize("project", &database).unwrap();
        store
            .page_put(PagePutInput {
                slug: "attention".to_string(),
                title: "注意力机制".to_string(),
                kind: Some("concept".to_string()),
                summary: None,
                body: "注意力机制帮助模型聚焦关键信号。".to_string(),
                source_ids: Vec::new(),
                provenance: Vec::new(),
            })
            .unwrap();
        drop(store);

        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "DROP TABLE search_fts;
             DROP TABLE retrieval_feedback;
             DROP TABLE retrieval_weights;
             DROP TABLE source_path_revisions;
             DROP TABLE page_provenance;
             DROP TABLE ingest_jobs;
             ALTER TABLE sources DROP COLUMN structural_navigation;
             ALTER TABLE pages DROP COLUMN structural_navigation;
             CREATE VIRTUAL TABLE source_fts USING fts5(
                 source_id UNINDEXED, title, content
             );
             CREATE VIRTUAL TABLE page_fts USING fts5(
                 slug UNINDEXED, title, summary, body
             );
             UPDATE meta SET value = '1' WHERE key = 'format_version';
             DELETE FROM meta WHERE key = 'tokenizer';
             PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
        drop(conn);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..4 {
                let barrier = barrier.clone();
                let database = database.clone();
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    Store::open("project", database).unwrap()
                }));
            }
            for handle in handles {
                drop(handle.join().unwrap());
            }
        });

        let migrated = Store::open("project", &database).unwrap();
        let results = migrated.search("注意力", 10).unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identifier, "attention");
        let version: i32 = migrated
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let journal_mode: String = migrated
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(version, USER_VERSION);
        assert_eq!(journal_mode, "wal");
    }

    #[test]
    fn stale_ingest_migration_step_accepts_a_newer_intermediate_version() {
        let mut store = test_store();
        store
            .conn
            .execute_batch(
                "DROP TABLE page_provenance;
                 UPDATE meta SET value = '6' WHERE key = 'format_version';
                 PRAGMA user_version = 6;",
            )
            .unwrap();

        migrate_ingest_workflow(&mut store.conn).unwrap();

        let version: i32 = store
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, COMPOUND_WIKI_VERSION);
    }

    #[test]
    fn lint_reports_missing_orphaned_and_duplicate_search_rows() {
        let mut store = test_store();
        for slug in ["missing", "duplicate"] {
            store
                .page_put(PagePutInput {
                    slug: slug.to_string(),
                    title: slug.to_string(),
                    kind: None,
                    summary: None,
                    body: format!("{slug} body"),
                    source_ids: Vec::new(),
                    provenance: Vec::new(),
                })
                .unwrap();
        }
        store
            .conn
            .execute(
                "DELETE FROM search_fts
                 WHERE doc_type = 'page' AND identifier = 'missing'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO search_fts(
                    doc_type, identifier, title_terms, summary_terms, body_terms
                 ) VALUES ('page', 'orphan', 'orphan', '', 'orphan')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO search_fts(
                    doc_type, identifier, title_terms, summary_terms, body_terms
                 )
                 SELECT doc_type, identifier, title_terms, summary_terms, body_terms
                 FROM search_fts
                 WHERE doc_type = 'page' AND identifier = 'duplicate'",
                [],
            )
            .unwrap();

        let codes = store
            .lint(100, 0)
            .unwrap()
            .issues
            .into_iter()
            .map(|issue| issue.code)
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("search_index_missing"));
        assert!(codes.contains("search_index_orphan"));
        assert!(codes.contains("search_index_duplicate"));
    }
}
