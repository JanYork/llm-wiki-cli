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
const EXTERNAL_GRAPH_VERSION: i32 = 12;
const USER_VERSION: i32 = 13;
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
pub struct TagPageIdentity {
    pub scope: String,
    pub tag: String,
    pub page_slug: String,
    pub priority: i32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaggedPage {
    pub scope: String,
    pub tag: String,
    pub priority: i32,
    pub reason: String,
    pub ordinal: usize,
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
struct TagPolicySnapshot {
    name: String,
    autoload: bool,
    autoload_priority: i32,
    autoload_limit: i64,
    autoload_max_chars: i64,
    reason: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PageTagSnapshot {
    tag_name: String,
    page_slug: String,
    priority: i32,
    reason: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseTagSnapshot {
    policy: Option<TagPolicySnapshot>,
    memberships: Vec<PageTagSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SparseTagInverse {
    name: String,
    before: SparseTagSnapshot,
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
    #[serde(default)]
    tags: Vec<SparseTagInverse>,
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
