use crate::{
    artifacts,
    config::{self, GraphSetting},
    error::{AppError, Result},
    graph::{GraphPage, related},
    tokenize::{joined_index_terms, tokenize_for_query},
};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    backup::Backup, ffi, params,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::{BTreeMap, BTreeSet},
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
const USER_VERSION: i32 = 12;
const CHANGESET_FREEZE_KEY: &str = "changeset_frozen";
const SEARCH_INDEX_VERSION: i32 = 4;
const INGEST_WORKFLOW_VERSION: i32 = 5;
const TOKENIZER_ID: &str = "cjk-bigram@1/bounded-terms";
const SOURCE_GROUNDED: &str = "source-grounded";
const EXPLICIT_PROVENANCE: [&str; 3] = ["user-provided", "agent-observed", "hypothesis"];
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);
const TIMESTAMP_SQL: &str = "STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')";
const TITLE_WEIGHT: f64 = 32.0;
const PATH_WEIGHT: f64 = 16.0;
const GENERIC_WEIGHT: f64 = 8.0;
const GRAPH_MATCH_WEIGHT: f64 = 0.25;
const GRAPH_HUB_WEIGHT: f64 = 4.0;
const MANUAL_WEIGHT: f64 = 2.0;
const FEEDBACK_WEIGHT: f64 = 1.5;
static SOURCE_STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);
type MigrationProgress<'a> = dyn FnMut(usize, usize, &str) -> Result<()> + 'a;
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
    pub invalidated_semantic_relations: usize,
    pub engine: String,
    pub status: String,
    pub document_duration_ms: u64,
    pub queue_duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<Value>,
}

impl GraphMutationSummary {
    fn with_work(mut self, work: Option<Value>) -> Self {
        self.work = work;
        self
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRemoveResponse {
    pub scope: String,
    pub database: String,
    pub source_id: i64,
    pub removed: bool,
    pub removed_path_revisions: usize,
    pub untracked_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_work: Option<Value>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_work: Option<Value>,
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
        validate_database_integrity(&self.conn)?;
        if self.changeset_storage_kind()?.as_deref() == Some("sparse-v1") {
            validate_sparse_changeset_operations(&self.conn)?;
        }
        Ok(())
    }

    pub fn reconcile_graph_projection(&mut self) -> Result<()> {
        let _ = config::resolve(&self.scope, &self.database)?;
        Ok(())
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

    pub(crate) fn changeset_graph_documents(&self) -> Result<Vec<(String, String)>> {
        let begin_operation_id: i64 = self.conn.query_row(
            "SELECT begin_operation_id FROM changesets
             WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT action, target, detail_json FROM operations
             WHERE id > ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([begin_operation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut documents = BTreeSet::new();
        for row in rows {
            let (action, target, detail) = row?;
            match action.as_str() {
                "page_put" | "page_remove" => {
                    documents.insert(("page".to_string(), target));
                }
                "source_add" => {
                    let detail: Value = serde_json::from_str(&detail)
                        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
                    if let Some(id) = detail.get("source_id").and_then(Value::as_i64) {
                        documents.insert(("source".to_string(), id.to_string()));
                    }
                }
                "source_remove" => {
                    documents.insert(("source".to_string(), target));
                }
                _ => {}
            }
        }
        Ok(documents.into_iter().collect())
    }

    pub(crate) fn changeset_touched_source_paths(&self) -> Result<Vec<String>> {
        let begin_operation_id: i64 = self.conn.query_row(
            "SELECT begin_operation_id FROM changesets
             WHERE status = 'draft' ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT detail_json FROM operations
             WHERE id > ?1 AND action = 'source_add' ORDER BY id",
        )?;
        let rows = statement.query_map([begin_operation_id], |row| row.get::<_, String>(0))?;
        let mut paths = BTreeSet::new();
        for row in rows {
            let detail: Value = serde_json::from_str(&row?)
                .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
            if let Some(path) = detail.get("tracked_path").and_then(Value::as_str) {
                paths.insert(path.to_string());
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub(crate) fn source_path_head(&self, path: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT source_id FROM source_path_revisions
                 WHERE tracked_path = ?1 ORDER BY revision DESC LIMIT 1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
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
        if documents.is_empty()
            || config::resolve(&self.scope, &self.database)?.setting == GraphSetting::Disabled
        {
            return Ok(None);
        }
        let response = crate::work::start_graph_documents(&self.scope, &self.database, documents)?;
        Ok(Some(response["work"].clone()))
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
    _migration_progress: Option<&mut MigrationProgress<'_>>,
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
    if matches!(version, CHANGESETS_VERSION | 11) {
        migrate_external_graph_schema(conn)?;
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

fn migrate_external_graph_schema(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current == USER_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if !matches!(current, CHANGESETS_VERSION | 11) {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {USER_VERSION}"),
        ));
    }
    tx.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS semantic_relations(
            id TEXT PRIMARY KEY,
            relation_type TEXT NOT NULL,
            from_identifier TEXT NOT NULL,
            to_identifier TEXT NOT NULL,
            confidence REAL,
            provenance TEXT NOT NULL,
            reason TEXT,
            source_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );"
    ))?;
    let has_legacy_graph: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'graph_edges')",
        [],
        |row| row.get(0),
    )?;
    if has_legacy_graph {
        tx.execute(
            "INSERT OR IGNORE INTO semantic_relations(
                id, relation_type, from_identifier, to_identifier,
                confidence, provenance, reason, source_ids_json, created_at, updated_at
             )
             SELECT edge_id, edge_type, from_node_id, to_node_id,
                    confidence, COALESCE(provenance, 'agent-observed'), reason,
                    COALESCE(json_extract(properties_json, '$.source_ids'), '[]'),
                    created_at, updated_at
             FROM graph_edges WHERE owner_type = 'manual'",
            [],
        )?;
    }
    tx.execute_batch(
        "DROP TABLE IF EXISTS graph_occurrences;
         DROP TABLE IF EXISTS graph_edges;
         DROP TABLE IF EXISTS graph_deltas;
         DROP TABLE IF EXISTS graph_generations;
         DROP TABLE IF EXISTS graph_projection_state;
         DROP TABLE IF EXISTS term_pair_contributions;
         DROP TABLE IF EXISTS term_pair_totals;
         DROP TABLE IF EXISTS document_index_state;
         DROP TABLE IF EXISTS span_fts;
         DROP TABLE IF EXISTS span_fts_data;
         DROP TABLE IF EXISTS span_fts_idx;
         DROP TABLE IF EXISTS span_fts_content;
         DROP TABLE IF EXISTS span_fts_docsize;
         DROP TABLE IF EXISTS span_fts_config;
         DROP TABLE IF EXISTS search_spans;
         DROP TABLE IF EXISTS graph_nodes;
         DELETE FROM meta WHERE key LIKE 'graph_digest_%';
         CREATE TABLE search_spans(
            span_id TEXT PRIMARY KEY,
            span_type TEXT NOT NULL CHECK(span_type IN ('passage', 'sentence')),
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL,
            parent_identifier TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            byte_start INTEGER NOT NULL,
            byte_end INTEGER NOT NULL,
            content_fingerprint TEXT NOT NULL,
            segmenter_version INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1))
         );
         CREATE INDEX search_spans_document
         ON search_spans(document_type, document_identifier, active);
         CREATE VIRTUAL TABLE span_fts USING fts5(
            span_id UNINDEXED, span_type UNINDEXED,
            document_type UNINDEXED, document_identifier UNINDEXED,
            title_terms, path_terms, body_terms,
            content='', contentless_delete=1, contentless_unindexed=1
         );",
    )?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![USER_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to remove legacy graph schema: {error}"),
        )
    })
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

        CREATE TABLE semantic_relations(
            id TEXT PRIMARY KEY,
            relation_type TEXT NOT NULL,
            from_identifier TEXT NOT NULL,
            to_identifier TEXT NOT NULL,
            confidence REAL,
            provenance TEXT NOT NULL,
            reason TEXT,
            source_ids_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            updated_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
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

        CREATE TABLE search_spans(
            span_id TEXT PRIMARY KEY,
            span_type TEXT NOT NULL CHECK(span_type IN ('passage', 'sentence')),
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL,
            parent_identifier TEXT NOT NULL,
            ordinal INTEGER NOT NULL,
            byte_start INTEGER NOT NULL,
            byte_end INTEGER NOT NULL,
            content_fingerprint TEXT NOT NULL,
            segmenter_version INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 1 CHECK(active IN (0, 1))
        );
        CREATE INDEX search_spans_document
        ON search_spans(document_type, document_identifier, active);

        CREATE VIRTUAL TABLE span_fts USING fts5(
            span_id UNINDEXED,
            span_type UNINDEXED,
            document_type UNINDEXED,
            document_identifier UNINDEXED,
            title_terms,
            path_terms,
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
            'retrieval_feedback', 'changesets', 'search_fts', 'search_spans',
            'span_fts', 'semantic_relations'
         )",
        [],
        |row| row.get(0),
    )?;
    if essential_tables != 16 {
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
        "SELECT span_id, span_type, document_type, document_identifier, parent_identifier, ordinal, byte_start, byte_end, content_fingerprint, segmenter_version, active FROM search_spans LIMIT 0",
        "SELECT rowid, span_id, span_type, document_type, document_identifier, title_terms, path_terms, body_terms FROM span_fts LIMIT 0",
        "SELECT id, relation_type, from_identifier, to_identifier, confidence, provenance, reason, source_ids_json, created_at, updated_at FROM semantic_relations LIMIT 0",
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
    validate_sparse_operation_actions(operations.iter().map(|(action, _, _)| action.as_str()))?;
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
    changeset_test_fault("mid_copy")?;
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
            "INSERT OR IGNORE INTO sources(
                    id, content_hash, title, origin, content,
                    structural_navigation, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
    const TABLES: [&str; 26] = [
        "changesets",
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
        "search_spans",
        "semantic_relations",
        "source_path_revisions",
        "sources",
        "span_fts",
        "span_fts_config",
        "span_fts_content",
        "span_fts_data",
        "span_fts_docsize",
        "span_fts_idx",
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
        "DELETE FROM semantic_relations;
         DELETE FROM search_spans;
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
         INSERT INTO semantic_relations(
             id, relation_type, from_identifier, to_identifier,
             confidence, provenance, reason, source_ids_json, created_at, updated_at
         ) SELECT
             id, relation_type, from_identifier, to_identifier,
             confidence, provenance, reason, source_ids_json, created_at, updated_at
           FROM candidate.semantic_relations;
         INSERT INTO search_spans(
             span_id, span_type, document_type, document_identifier,
             parent_identifier, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, active
         ) SELECT
             span_id, span_type, document_type, document_identifier,
             parent_identifier, ordinal, byte_start, byte_end,
             content_fingerprint, segmenter_version, active
           FROM candidate.search_spans;",
    )
    .map_err(changeset_copy_error)?;
    rebuild_span_index(tx)?;
    Ok(())
}

fn rebuild_span_index(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("DELETE FROM span_fts", [])?;
    let rows = {
        let mut statement = tx.prepare(
            "SELECT n.span_id, n.span_type, n.document_type,
                    n.document_identifier, n.byte_start, n.byte_end,
                    CASE n.document_type WHEN 'page' THEN p.body ELSE s.content END,
                    CASE n.document_type
                        WHEN 'page' THEN p.title
                        ELSE COALESCE(s.title, s.origin)
                    END AS title,
                    CASE n.document_type
                        WHEN 'page' THEN p.slug
                        ELSE s.origin
                    END AS path
             FROM search_spans n
             LEFT JOIN pages p
               ON n.document_type = 'page' AND p.slug = n.document_identifier
             LEFT JOIN sources s
               ON n.document_type = 'source'
              AND s.id = CAST(n.document_identifier AS INTEGER)
             WHERE n.active = 1
             ORDER BY n.document_type, n.document_identifier, n.span_type, n.ordinal, n.span_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (node_id, node_type, document_type, document_identifier, start, end, body, title, path) in
        rows
    {
        let start = usize::try_from(start)
            .map_err(|_| AppError::new("changeset_corrupt", "negative search span start"))?;
        let end = usize::try_from(end)
            .map_err(|_| AppError::new("changeset_corrupt", "negative search span end"))?;
        let label = body.get(start..end).ok_or_else(|| {
            AppError::new("changeset_corrupt", "search span has an invalid byte range")
        })?;
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
                joined_terms(label),
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
        content_fingerprint,
        version_fingerprint,
    }))
}

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
            title_terms, path_terms, body_terms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    let document_key = format!("{document_type}:{document_identifier}");
    for passage in segmented.passages {
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
            bm25(span_fts, 0.0, 0.0, 0.0, 0.0, 4.0, 2.0, 1.0),
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
        let operations_before: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let response = store.page_put(input).unwrap();
        let operations_after: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!response.created);
        assert_eq!(operations_after, operations_before);
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

    #[test]
    fn markdown_links_ignore_code_and_include_relative_markdown_targets() {
        let body = r#"Real [[real-target]] and [relative](../docs/other-page.md#section).

`inline [[inline-fake]]`

```sh
rg '^[[:space:]]*[[fenced-fake]]'
```

    indented [[indented-fake]]
"#;

        assert_eq!(
            extract_links(body),
            vec!["other-page".to_string(), "real-target".to_string()]
        );
    }
}
