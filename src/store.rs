use crate::{
    artifacts,
    error::{AppError, Result},
    graph::{GraphPage, related},
    tokenize::{tokenize_for_index, tokenize_for_query},
};
use rusqlite::{
    Connection, ErrorCode, MAIN_DB, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
    backup::Backup, ffi, params,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CStr,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const COMPOUND_WIKI_VERSION: i32 = 6;
const USER_VERSION: i32 = 7;
const SEARCH_INDEX_VERSION: i32 = 4;
const INGEST_WORKFLOW_VERSION: i32 = 5;
const TOKENIZER_ID: &str = "cjk-bigram@1/bounded-terms";
const SOURCE_GROUNDED: &str = "source-grounded";
const EXPLICIT_PROVENANCE: [&str; 3] = ["user-provided", "agent-observed", "hypothesis"];
const BUSY_TIMEOUT: Duration = Duration::from_secs(15);
const TIMESTAMP_SQL: &str = "STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')";
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
)
"#;

#[derive(Debug)]
pub struct Store {
    scope: String,
    database: PathBuf,
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceAddInput {
    pub title: Option<String>,
    pub origin: String,
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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceRemoveResponse {
    pub scope: String,
    pub database: String,
    pub source_id: i64,
    pub removed: bool,
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
    pub title: Option<String>,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub provenance: Option<Vec<String>>,
    pub snippet: String,
    pub rank: f64,
    #[serde(skip)]
    paired_source_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Auto,
    Page,
    Source,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub mode: SearchMode,
    pub kinds: Vec<String>,
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
        let created = prepare_store(&mut store.conn, true)?;
        if created {
            store.record_top_level_operation(
                "init",
                "wiki",
                json!({ "user_version": USER_VERSION, "tokenizer": TOKENIZER_ID }),
            )?;
        }
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
        prepare_store(&mut conn, false)?;

        Ok(Self {
            scope,
            database,
            conn,
        })
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
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = Vec::with_capacity(inputs.len());
        for input in &inputs {
            inserted.push(insert_source(&tx, input)?);
        }
        tx.commit()?;

        inserted
            .into_iter()
            .map(|(source_id, created)| {
                Ok(SourceAddResponse {
                    scope: self.scope.clone(),
                    database: self.database_string(),
                    source: self.load_source_summary(source_id)?,
                    created,
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
        tx.execute(
            "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
            params![id.to_string()],
        )?;
        tx.execute("DELETE FROM sources WHERE id = ?1", params![id])?;
        record_operation(&tx, "source_remove", &id.to_string(), &json!({}))?;
        tx.commit()?;
        Ok(SourceRemoveResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            source_id: id,
            removed: true,
        })
    }

    pub fn page_put(&mut self, input: PagePutInput) -> Result<PagePutResponse> {
        validate_page_slug(&input.slug)?;
        let source_ids = dedupe_i64(input.source_ids);
        let explicit_provenance = normalize_explicit_provenance(input.provenance)?;
        let links = extract_links(&input.body);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_sources(&tx, &source_ids)?;

        let existed = tx
            .query_row(
                "SELECT 1 FROM pages WHERE slug = ?1",
                params![&input.slug],
                |_| Ok(()),
            )
            .optional()?
            .is_some();

        if existed {
            tx.execute(
                &format!(
                    "UPDATE pages
                     SET title = ?2, kind = ?3, summary = ?4, body = ?5, updated_at = {TIMESTAMP_SQL}
                     WHERE slug = ?1"
                ),
                params![
                    &input.slug,
                    &input.title,
                    input.kind.as_deref(),
                    input.summary.as_deref(),
                    &input.body
                ],
            )?;
        } else {
            tx.execute(
                &format!(
                    "INSERT INTO pages(slug, title, kind, summary, body, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, {TIMESTAMP_SQL}, {TIMESTAMP_SQL})"
                ),
                params![
                    &input.slug,
                    &input.title,
                    input.kind.as_deref(),
                    input.summary.as_deref(),
                    &input.body
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
        tx.commit()?;

        Ok(PagePutResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            page: self.load_page_write(&input.slug)?,
            created: !existed,
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
        tx.execute("DELETE FROM pages WHERE slug = ?1", params![slug])?;
        record_operation(&tx, "page_remove", slug, &json!({}))?;
        tx.commit()?;
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
                kinds: Vec::new(),
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
        let mut results = if tokens.is_empty() {
            Vec::new()
        } else {
            search_index(
                &self.conn,
                &self.scope,
                query,
                &tokens,
                limit
                    .saturating_mul(candidate_multiplier)
                    .clamp(limit, 1000),
            )?
        };

        let normalized_kinds = options
            .kinds
            .iter()
            .map(|kind| kind.trim().to_lowercase())
            .collect::<BTreeSet<_>>();
        results.retain(|result| {
            let type_matches = match options.mode {
                SearchMode::Auto | SearchMode::All => true,
                SearchMode::Page => result.result_type == "page",
                SearchMode::Source => result.result_type == "source",
            };
            let kind_matches = normalized_kinds.is_empty()
                || result.result_type == "source"
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

        results.sort_by(|left, right| {
            search_type_priority(left, options.mode)
                .cmp(&search_type_priority(right, options.mode))
                .then_with(|| left.rank.total_cmp(&right.rank))
                .then_with(|| left.result_type.cmp(&right.result_type))
                .then_with(|| left.identifier.cmp(&right.identifier))
        });
        results.truncate(limit);
        load_search_provenance(&self.conn, &mut results)?;

        Ok(SearchResponse { results })
    }

    pub fn record_search(&mut self, query: &str, limit: usize) -> Result<()> {
        self.record_top_level_operation("search", query, json!({ "limit": limit }))
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
        tx.execute("INSERT INTO search_fts(search_fts) VALUES('optimize')", [])?;
        record_operation(
            &tx,
            "maintenance_compact",
            "wiki.db",
            &json!({ "wal_before_bytes": before_bytes }),
        )?;
        tx.commit()?;

        let (busy, log_frames, checkpointed_frames) =
            self.conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?;
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

        let safety_checkpoint = fresh_safety_checkpoint_name(&self.database)?;
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
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = artifact_snapshot(&tx, include_raw_sources)?;
        let materialize = if include_raw_sources {
            artifacts::materialize_snapshot
        } else {
            artifacts::materialize_wiki_snapshot
        };
        let files = materialize(&root, &snapshot)
            .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
        tx.commit()?;
        Ok(MaterializeResponse {
            scope: self.scope.clone(),
            database: self.database_string(),
            files,
        })
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
        let mut counts = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(&format!(
                "{LINT_ISSUES_SQL}
                 SELECT code, COUNT(*) FROM issues GROUP BY code ORDER BY code"
            ))?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (code, count) = row?;
                let count = usize::try_from(count).map_err(|_| {
                    AppError::new("database_error", "lint issue count is out of range")
                })?;
                counts.insert(code, count);
            }
        }
        let total = counts.values().sum();

        let issues = {
            let mut statement = self.conn.prepare(&format!(
                "{LINT_ISSUES_SQL}
                 SELECT code, page, target, message
                 FROM issues
                 ORDER BY code, page, target
                 LIMIT ?1 OFFSET ?2"
            ))?;
            statement
                .query_map(params![limit as i64, offset as i64], |row| {
                    Ok(LintIssue {
                        code: row.get(0)?,
                        page: row.get(1)?,
                        target: row.get(2)?,
                        message: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
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

fn fresh_safety_checkpoint_name(database: &Path) -> Result<String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::new("system_time_error", error.to_string()))?
        .as_millis();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("pre-restore-{millis}")
        } else {
            format!("pre-restore-{millis}-{suffix}")
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

fn artifact_snapshot(
    tx: &Transaction<'_>,
    include_source_content: bool,
) -> Result<artifacts::Snapshot> {
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

fn prepare_store(conn: &mut Connection, allow_create: bool) -> Result<bool> {
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
    validate_store(conn)
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
        COMPOUND_WIKI_VERSION | USER_VERSION => {
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
        USER_VERSION => {
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
        params![USER_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{USER_VERSION} provenance migration: {error}"),
        )
    })
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
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );

        CREATE TABLE pages(
            slug TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            kind TEXT,
            summary TEXT,
            body TEXT NOT NULL,
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

        CREATE VIRTUAL TABLE search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
            summary_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );

        INSERT INTO meta(key, value) VALUES ('format_version', '{USER_VERSION}');
        INSERT INTO meta(key, value) VALUES ('tokenizer', '{TOKENIZER_ID}');
        PRAGMA user_version = {USER_VERSION};
        "
    ))?;
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
        DROP TRIGGER IF EXISTS sources_ai;
        DROP TRIGGER IF EXISTS sources_au;
        DROP TRIGGER IF EXISTS sources_ad;
        DROP TRIGGER IF EXISTS pages_ai;
        DROP TRIGGER IF EXISTS pages_au;
        DROP TRIGGER IF EXISTS pages_ad;
        DROP TABLE IF EXISTS source_fts;
        DROP TABLE IF EXISTS page_fts;
        DROP TABLE IF EXISTS search_fts;
        DROP TABLE IF EXISTS search_fts_data;
        DROP TABLE IF EXISTS search_fts_idx;
        DROP TABLE IF EXISTS search_fts_content;
        DROP TABLE IF EXISTS search_fts_docsize;
        DROP TABLE IF EXISTS search_fts_config;
        CREATE VIRTUAL TABLE search_fts USING fts5(
            doc_type UNINDEXED,
            identifier UNINDEXED,
            title_terms,
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
        let mut statement = tx.prepare("SELECT id, title, content FROM sources ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, title, content) = row?;
            index_source(tx, id, title.as_deref(), &content)?;
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
            index_page(tx, &slug, &title, summary.as_deref(), &body)?;
            page_count += 1;
        }
    }
    Ok((source_count, page_count))
}

fn validate_store(conn: &Connection) -> Result<()> {
    for sql in [
        "SELECT key, value FROM meta LIMIT 0",
        "SELECT id, content_hash, title, origin, content, created_at FROM sources LIMIT 0",
        "SELECT slug, title, kind, summary, body, created_at, updated_at FROM pages LIMIT 0",
        "SELECT page_slug, source_id FROM page_sources LIMIT 0",
        "SELECT page_slug, provenance FROM page_provenance LIMIT 0",
        "SELECT from_slug, to_slug FROM links LIMIT 0",
        "SELECT action, target, detail_json, created_at FROM operations LIMIT 0",
        "SELECT source_id, status, attempts, analysis, last_error, no_derived_pages_reason, updated_at FROM ingest_jobs LIMIT 0",
        "SELECT rowid, doc_type, identifier, title_terms, summary_terms, body_terms FROM search_fts LIMIT 0",
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
                MAX(CASE WHEN key = 'tokenizer' THEN value END)
             FROM meta
             WHERE key IN ('format_version', 'tokenizer')",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
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
    Ok(())
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
            "INSERT OR IGNORE INTO sources(content_hash, title, origin, content, created_at)
             VALUES (?1, ?2, ?3, ?4, {TIMESTAMP_SQL})"
        ),
        params![&content_hash, &title, &input.origin, &input.content],
    )? == 1;
    let source_id = tx.query_row(
        "SELECT id FROM sources WHERE content_hash = ?1",
        params![&content_hash],
        |row| row.get::<_, i64>(0),
    )?;
    if created {
        index_source(tx, source_id, Some(&title), input.content.as_str())?;
    }
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO ingest_jobs(source_id, status, updated_at)
             VALUES (?1, 'pending', {TIMESTAMP_SQL})"
        ),
        params![source_id],
    )?;
    record_operation(
        tx,
        "source_add",
        &input.origin,
        &json!({ "source_id": source_id, "created": created }),
    )?;
    Ok((source_id, created))
}

fn record_operation(
    tx: &Transaction<'_>,
    action: &str,
    target: &str,
    detail: &Value,
) -> Result<()> {
    let detail_json = serde_json::to_string(detail)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    tx.execute(
        "INSERT INTO operations(action, target, detail_json) VALUES (?1, ?2, ?3)",
        params![action, target, detail_json],
    )?;
    Ok(())
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
    record_operation(tx, "ingest_claim", &source_id.to_string(), &json!({}))
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
    source_id: i64,
    title: Option<&str>,
    content: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM search_fts WHERE doc_type = 'source' AND identifier = ?1",
        params![source_id.to_string()],
    )?;
    tx.execute(
        "INSERT INTO search_fts(
            doc_type, identifier, title_terms, summary_terms, body_terms
         ) VALUES ('source', ?1, ?2, '', ?3)",
        params![
            source_id.to_string(),
            joined_terms(title.unwrap_or("")),
            joined_terms(content)
        ],
    )?;
    Ok(())
}

fn index_page(
    tx: &Transaction<'_>,
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
            doc_type, identifier, title_terms, summary_terms, body_terms
         ) VALUES ('page', ?1, ?2, ?3, ?4)",
        params![
            slug,
            joined_terms(title),
            joined_terms(summary.unwrap_or("")),
            joined_terms(body)
        ],
    )?;
    Ok(())
}

fn joined_terms(text: &str) -> String {
    let mut joined = String::new();
    for token in tokenize_for_index(text) {
        joined.push('\u{1e}');
        joined.push_str(&token);
        joined.push('\u{1f}');
    }
    joined
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
                bm25(search_fts, 0.0, 0.0, 8.0, 4.0, 1.0) AS fts_rank
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
            CASE
                WHEN INSTR(LOWER(body), LOWER(?3)) > 0 THEN
                    SUBSTR(body, MAX(INSTR(LOWER(body), LOWER(?3)) - 60, 1), 180)
                WHEN INSTR(LOWER(body), LOWER(?4)) > 0 THEN
                    SUBSTR(body, MAX(INSTR(LOWER(body), LOWER(?4)) - 60, 1), 180)
                ELSE SUBSTR(body, 1, 180)
            END AS snippet,
            CASE
                WHEN LENGTH(?3) = 0 THEN 0
                ELSE (
                    LENGTH(LOWER(body))
                    - LENGTH(REPLACE(LOWER(body), LOWER(?3), ''))
                ) / LENGTH(?3)
            END AS phrase_count,
            fts_rank,
            CASE
                WHEN doc_type = 'page' AND LOWER(COALESCE(kind, '')) = 'source'
                THEN (
                    SELECT GROUP_CONCAT(ps.source_id, ',')
                    FROM page_sources ps
                    WHERE ps.page_slug = identifier
                )
                ELSE NULL
            END AS paired_source_ids
        FROM ranked";
    let mut statement = conn.prepare(sql)?;
    statement
        .query_map(
            params![match_query, limit as i64, query, first_token],
            |row| {
                let title = row.get::<_, Option<String>>(2)?;
                let phrase_count = row.get::<_, i64>(6)?;
                let fts_rank = row.get::<_, f64>(7)?;
                let paired_source_ids = row
                    .get::<_, Option<String>>(8)?
                    .map(|ids| {
                        ids.split(',')
                            .filter_map(|id| id.parse::<i64>().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let result_type = row.get::<_, String>(0)?;
                Ok(SearchResult {
                    scope: scope.to_string(),
                    result_type,
                    identifier: row.get(1)?,
                    rank: comparable_rank(
                        title.as_deref(),
                        query,
                        phrase_count,
                        fts_rank,
                        tokens.len(),
                    ),
                    title,
                    kind: row.get(3)?,
                    summary: row.get(4)?,
                    provenance: None,
                    snippet: row.get(5)?,
                    paired_source_ids,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
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

fn comparable_rank(
    title: Option<&str>,
    query: &str,
    phrase_count: i64,
    fts_rank: f64,
    token_count: usize,
) -> f64 {
    let normalized_query = query.to_lowercase();
    let normalized_title = title.unwrap_or("").to_lowercase();
    let query_terms = tokenize_for_query(query);
    let title_terms = tokenize_for_query(title.unwrap_or(""))
        .into_iter()
        .collect::<BTreeSet<_>>();
    let title_covers_query =
        query_terms.len() > 1 && query_terms.iter().all(|term| title_terms.contains(term));
    let title_score = if !normalized_query.is_empty() && normalized_title == normalized_query {
        1_000.0
    } else if token_count > 1
        && !normalized_query.is_empty()
        && normalized_title.contains(&normalized_query)
    {
        500.0
    } else if title_covers_query {
        300.0
    } else {
        0.0
    };
    let phrase_score = if token_count > 1 {
        phrase_count.clamp(0, 10_000) as f64 * 10.0
    } else {
        0.0
    };
    fts_rank - title_score - phrase_score
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
                content: "same bytes".to_string(),
            })
            .unwrap();
        let second = store
            .source_add(SourceAddInput {
                title: Some("Second".to_string()),
                origin: "/tmp/second.md".to_string(),
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
    fn title_token_coverage_bridges_query_separators() {
        let query = "系统设置 支付渠道管理";
        let rank = comparable_rank(
            Some("01-系统设置-支付渠道管理.md"),
            query,
            0,
            0.0,
            tokenize_for_query(query).len(),
        );

        assert!(
            rank <= -250.0,
            "all title terms should receive a strong title boost despite separator differences"
        );
    }

    #[test]
    fn page_put_deduplicates_repeated_links_and_source_ids() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
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
    fn failed_page_update_leaves_page_relations_fts_and_log_unchanged() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
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
             DROP TABLE page_provenance;
             DROP TABLE ingest_jobs;
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
