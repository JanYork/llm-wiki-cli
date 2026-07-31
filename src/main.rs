mod artifacts;
mod error;
mod graph;
mod import;
mod scope;
mod store;
mod tokenize;

use clap::{Parser, Subcommand, ValueEnum};
use error::{AppError, Result};
use import::collect_documents;
use scope::{
    Scope, StorePath, ensure_scope_supported, init_store_path, resolve_read_store_paths,
    resolve_store_path,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};
use store::{PagePutInput, SearchMode, SearchOptions, SourceAddInput, Store};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceManifest {
    sources: Vec<SourceManifestEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceManifestEntry {
    path: PathBuf,
    title: Option<String>,
}

#[derive(Parser)]
#[command(
    name = "lwc",
    version,
    about = "Build and maintain a persistent LLM-written wiki",
    long_about = "Build and maintain a persistent, source-grounded wiki for the current project or the user.\n\n\
SQLite stores immutable sources, Agent-written pages, citations, links, ingest state, search indexes, and the operation log. \
Markdown under .lwc/ is a human-readable projection for Obsidian and can be rebuilt from SQLite.\n\n\
Every successful command prints JSON to stdout. Failures print a structured JSON error to stderr and exit non-zero.",
    after_help = "Agent operating contract:\n  \
- Read stdout as JSON; on failure read stderr.error.code and stderr.error.message.\n  \
- Do not edit .lwc/wiki.db or generated Markdown directly; mutate knowledge through lwc commands.\n  \
- Treat `source add` as collection only. A source is integrated only after the ingest loop completes.\n  \
- Ground factual pages with repeated --source IDs and preserve uncertainty in page bodies.\n  \
- Search compiled pages first, inspect cited sources when needed, and write durable answers back as kind=query pages.\n  \
- Current stores stay read-only for read commands; a writable legacy store is migrated transactionally once before reading.\n  \
- Run lint after a batch of changes; compact storage only during an idle maintenance window.\n\n\
Persistent workflow:\n  \
1. lwc init\n  \
2. lwc source add-dir docs/\n  \
3. lwc ingest next --source-max-chars 100000\n  \
4. lwc ingest analyze <SOURCE_ID> --file analysis.md\n  \
5. lwc page put source-<SOURCE_ID> --title ... --kind source --file summary.md --source <SOURCE_ID>\n  \
6. lwc page put <SHARED-SLUG> --title ... --kind concept --file concept.md --source <SOURCE_ID>\n  \
7. lwc ingest complete <SOURCE_ID>\n  \
8. lwc search \"question\"\n\n\
Scopes:\n  \
project  Use the nearest ancestor .lwc/wiki.db (default).\n  \
         Set LWC_PROJECT_ROOT to cap discovery and initialization at an authorized root.\n  \
global   Use ~/.lwc/wiki.db for reusable cross-project knowledge.\n  \
all      Read project and global stores together; valid only for search and context.\n  \
         search --record appends the query operation to both selected stores.\n\n\
Run `lwc <COMMAND> --help` for command-specific examples and side effects."
)]
struct Cli {
    /// Wiki scope. Mutating commands accept project or global; all is for search/context.
    #[arg(long, value_enum, default_value = "project", global = true)]
    scope: Scope,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the selected Wiki, locally exclude project state from Git, and materialize it.
    #[command(after_help = "Examples:\n  lwc init\n  lwc --scope global init")]
    Init {
        /// Do not add the project .lwc directory to Git's local exclude file.
        #[arg(long)]
        no_git_exclude: bool,
    },
    /// Read or replace the durable instructions that govern Wiki maintenance.
    #[command(
        long_about = "Manage the durable schema that tells every Agent how pages, citations, links, naming, uncertainty, and maintenance should work.",
        after_help = "When to use:\n  Read `schema show` before making domain-sensitive Wiki changes. Use `schema set` only when the maintenance contract itself changes.\n\nNext action:\n  After changing the schema, run `context` to verify the effective Agent instructions."
    )]
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Read or replace the durable goal, questions, and scope of the Wiki.
    #[command(
        long_about = "Manage the durable purpose that tells every Agent what this Wiki should help users understand or decide, and which questions or sources matter.",
        after_help = "When to use:\n  Read `purpose show` before broad synthesis. Use `purpose set` when the Wiki's goal, audience, authority, or research boundaries change.\n\nNext action:\n  After changing purpose, run `context` and keep subsequent pages aligned with it."
    )]
    Purpose {
        #[command(subcommand)]
        command: PurposeCommand,
    },
    /// Add, inspect, and trace immutable source snapshots.
    #[command(
        long_about = "Manage immutable evidence. Adding a source stores a content-addressed snapshot, indexes its raw text, records provenance, and creates a pending ingest job; it does not create synthesized Wiki knowledge.",
        after_help = "When to use:\n  Use `add` for one curated source, `add-manifest` for an atomic reviewed set, and `add-dir` for a deterministic UTF-8 text corpus. Use `show` when an Agent needs exact evidence, and `refs` to find every Wiki page citing it.\n\nNext action:\n  Claim returned manifest IDs with `lwc ingest claim`; otherwise use `lwc ingest next`. Do not treat a pending job as integrated knowledge."
    )]
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Create and inspect Agent-maintained Wiki pages and links.
    #[command(
        long_about = "Manage the persistent, compounding Wiki layer. Pages carry a stable slug, kind, one-line summary, Markdown body, source citations, and [[wikilinks]].",
        after_help = "When to use:\n  Use `put` after source analysis or when a valuable query answer should persist. Use `show` for full content, `links` for graph maintenance, and `list` for bounded discovery.\n\nDecision rule:\n  kind=source summarizes one source; kind=concept/entity updates shared knowledge; kind=query preserves a durable answer; kind=comparison and kind=synthesis combine multiple sources.\n\nNext action:\n  After ingest page updates, call `ingest complete <SOURCE_ID>`; after query write-back, run `lint`."
    )]
    Page {
        #[command(subcommand)]
        command: PageCommand,
    },
    /// Drive the persistent Agent ingest state machine.
    #[command(
        long_about = "Compile immutable sources into persistent Wiki knowledge through a crash-safe state machine:\n  pending -> analyzing -> generating -> completed\n\nFailed or interrupted work can be returned to pending with retry.",
        after_help = "Required Agent loop:\n  1. `ingest next --source-max-chars N` atomically claims one source and returns bounded context.\n  2. Continue long sources with `source show <ID> --offset-chars N --max-chars N` until window.has_more=false.\n  3. Analyze claims, entities, concepts, contradictions, uncertainty, and affected pages.\n  4. `ingest analyze <ID> --file ...` persists that plan and enters generating.\n  5. Write/update pages with `page put`; always create a cited kind=source summary and integrate the source into non-source knowledge.\n  6. `ingest complete <ID>` enforces both gates. If no non-source page should change, pass a specific --no-derived-pages-reason.\n  7. Run `lint` after a batch.\n\nNever skip directly from raw search results to completed."
    )]
    Ingest {
        #[command(subcommand)]
        command: IngestCommand,
    },
    /// Rank related pages using links, citations, neighbors, and page types.
    #[command(
        long_about = "Explore already-compiled Wiki relationships. Graph results explain their direct-link, shared-source, common-neighbor, and type-affinity signals.",
        after_help = "When to use:\n  Call `related` after search/page show when an Agent needs adjacent concepts, possible synthesis targets, or missing cross-links.\n\nNext action:\n  Inspect candidate pages with `page show`; update links or synthesis only when supported by cited evidence."
    )]
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    /// Rebuild derived search or Markdown artifacts from SQLite.
    #[command(
        long_about = "Repair or compact derived artifacts without changing canonical source or page knowledge. SQLite remains authoritative.",
        after_help = "When to use:\n  Use `materialize` when generated Markdown is missing or stale. Use `reindex` only when lint reports FTS integrity problems or after a tokenizer migration. Use `compact` during an idle maintenance window to optimize FTS and reclaim WAL space.\n\nNext action:\n  After repair, run `lint` again and verify its total is zero. After compact, inspect busy and after_bytes."
    )]
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceCommand,
    },
    /// Create, list, and safely restore complete SQLite checkpoints.
    #[command(
        long_about = "Manage recoverable full-database checkpoints using SQLite's online backup API. Restore validates the checkpoint, preserves the current database as pre-restore-*, and then refreshes generated projections.",
        after_help = "Use a checkpoint before a multi-source ingest or broad replacement of existing pages."
    )]
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    /// Search compiled Wiki pages and immutable raw sources.
    #[command(
        long_about = "Search compiled Wiki pages and immutable raw sources with SQLite FTS5.\n\n\
The default --type auto ranks Wiki pages first, hides their paired raw sources, and falls back to raw sources when needed. \
Use --type page for compiled knowledge, --type source for immutable evidence, or --type all for both. Repeat --kind to restrict page kinds. \
Read results[].type, kind, identifier, title, snippet, rank, and scope; a lower numeric rank is more relevant. \
The command is read-only by default and does not persist the query. Use --record only when the query itself should become part of the operation history. \
Use --scope all to merge project and global results; ranking remains deterministic across stores. \
Combining --scope all with --record appends the query operation to each selected store.",
        after_help = "Examples:\n  lwc search \"注意力机制\"\n  lwc search \"release policy\" --type page --kind concept --kind synthesis --limit 10\n  lwc search \"exact evidence\" --type source\n  lwc search \"audit both layers\" --type all\n  lwc --scope all search \"shared convention\"\n  lwc search \"durable research question\" --record"
    )]
    Search {
        /// Natural-language or keyword query. FTS syntax is escaped automatically.
        query: String,
        /// Search auto-ranked Wiki pages with source fallback, pages only, sources only, or both.
        #[arg(long = "type", value_enum, default_value = "auto")]
        target: SearchTarget,
        /// Restrict page results to this kind; repeat for multiple kinds.
        #[arg(long = "kind")]
        kinds: Vec<String>,
        /// Maximum number of merged results to return (1..=1000).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Persist the query; with scope=all, append it to each selected store.
        #[arg(long)]
        record: bool,
    },
    /// Return Agent-ready schema, purpose, page index, and recent operations.
    #[command(
        long_about = "Return bounded context for an Agent before it analyzes a source or answers a question.\n\n\
Each selected store includes its schema, purpose, page summaries, and recent operations. Use --scope all to include project and global context.",
        after_help = "Examples:\n  lwc context\n  lwc context --limit 100\n  lwc --scope all context --limit 25"
    )]
    Context {
        /// Maximum pages and recent operations returned per store (1..=1000).
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Report deterministic structural maintenance issues.
    #[command(
        long_about = "Read-only by default. Check the complete Wiki for missing schema, untitled sources, shallow completed ingests, uncited or orphaned pages, dangling links, and missing, orphaned, or duplicate search-index rows.\n\n\
counts and total describe the complete Wiki; limit and offset paginate only the returned issues. Semantic contradictions and stale claims remain the Agent's responsibility. Use --record only when this validation event belongs in durable history.",
        after_help = "Examples:\n  lwc lint\n  lwc lint --limit 100 --offset 100\n  lwc lint --record"
    )]
    Lint {
        /// Maximum issues returned in this page (1..=1000).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Zero-based issue offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Append this lint pass to the operation log.
        #[arg(long)]
        record: bool,
    },
    /// Show newest ingest, page, query, lint, and maintenance operations first.
    #[command(after_help = "Examples:\n  lwc log\n  lwc log --limit 100")]
    Log {
        /// Maximum operations returned (1..=1000).
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Replace the Wiki schema from a UTF-8 file or stdin.
    #[command(
        long_about = "Replace the durable instructions that tell Agents how to structure pages, cite sources, link concepts, and perform maintenance.\n\n\
FILE may be '-' to read UTF-8 Markdown from stdin. The update is stored transactionally and immediately projected to .lwc/schema.md.",
        after_help = "Examples:\n  lwc schema set AGENTS-WIKI.md\n  printf '# Schema\\nEvery factual page cites sources.' | lwc schema set -"
    )]
    Set {
        /// UTF-8 schema file, or '-' for stdin (maximum 64 MiB).
        file: PathBuf,
    },
    /// Print the current durable schema as JSON.
    #[command(after_help = "Example:\n  lwc schema show")]
    Show,
}

#[derive(Subcommand)]
enum PurposeCommand {
    /// Replace the Wiki purpose from a UTF-8 file or stdin.
    #[command(
        long_about = "Replace the durable statement of the Wiki's goal, key questions, authority boundaries, and intended users.\n\n\
FILE may be '-' to read UTF-8 Markdown from stdin. The update is stored transactionally and immediately projected to .lwc/purpose.md.",
        after_help = "Examples:\n  lwc purpose set PURPOSE.md\n  printf '# Goal\\nTrack architecture decisions.' | lwc purpose set -"
    )]
    Set {
        /// UTF-8 purpose file, or '-' for stdin (maximum 64 MiB).
        file: PathBuf,
    },
    /// Print the current durable purpose as JSON.
    #[command(after_help = "Example:\n  lwc purpose show")]
    Show,
}

#[derive(Subcommand)]
enum SourceCommand {
    /// Snapshot one immutable UTF-8 source and enqueue it for Agent ingestion.
    #[command(
        long_about = "Store an immutable snapshot of one UTF-8 source, index it, append a source_add operation, and create a pending ingest job.\n\n\
Content is deduplicated by SHA-256. Re-adding identical bytes returns the existing source and preserves its first title and origin. The original file is never modified.",
        after_help = "Examples:\n  lwc source add docs/design.md\n  lwc source add docs/paper.md --title \"Attention Is All You Need\""
    )]
    Add {
        /// Source file to snapshot; stdin is intentionally unsupported.
        file: PathBuf,
        /// Human-readable title; defaults deterministically to the source origin.
        #[arg(long)]
        title: Option<String>,
        /// Permit a project source that resolves outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
        /// Confirm that a flagged source was reviewed and is safe to snapshot.
        #[arg(long)]
        acknowledge_sensitive_source: bool,
    },
    /// Recursively snapshot supported UTF-8 text files and enqueue them.
    #[command(
        long_about = "Recursively import supported UTF-8 text files from DIRECTORY in deterministic path order.\n\n\
Supported extensions: md, mdx, txt, csv, json, html, htm, rtf, xml, yaml, yml, org, sql, and base. \
Hidden directories, .git, .lwc, .obsidian, .claudian, and symbolic links are skipped. Content hashes make retries idempotent.\n\n\
Valid files are committed even if other files are empty, oversized, unreadable, or invalid UTF-8. In that case the command returns a non-zero partial_import error with examples; fix those files and rerun safely.",
        after_help = "Examples:\n  lwc source add-dir docs/\n  lwc --scope global source add-dir ~/shared-notes/"
    )]
    AddDir {
        /// Root directory to scan recursively.
        directory: PathBuf,
        /// Permit a project source directory outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
        /// Confirm that flagged sources were reviewed and are safe to snapshot.
        #[arg(long)]
        acknowledge_sensitive_source: bool,
    },
    /// Atomically add a curated JSON list of source paths and optional titles.
    AddManifest {
        /// JSON manifest; relative source paths resolve from its parent directory.
        manifest: PathBuf,
        /// Permit project sources that resolve outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
        /// Confirm that flagged sources were reviewed and are safe to snapshot.
        #[arg(long)]
        acknowledge_sensitive_source: bool,
    },
    /// List source metadata without returning full source bodies.
    #[command(
        long_about = "Return source metadata in deterministic ID order without loading bodies.\n\n\
Read sources, limit, offset, and has_more from the JSON response. When has_more=true, add limit to offset and request the next page.",
        after_help = "Examples:\n  lwc source list --limit 100\n  lwc source list --limit 100 --offset 100"
    )]
    List {
        /// Maximum sources returned (1..=1000).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Zero-based source offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Return a resumable Unicode-safe window of one immutable source.
    #[command(
        long_about = "Return source metadata plus a Unicode-character window of the immutable body.\n\n\
Omit --max-chars to read from --offset-chars through the end. When window.has_more=true, continue from window.next_offset_chars; byte offsets are never required.",
        after_help = "Examples:\n  lwc source show 42\n  lwc source show 42 --max-chars 100000\n  lwc source show 42 --offset-chars 100000 --max-chars 100000"
    )]
    Show {
        /// Numeric source ID returned by source add/list or ingest next.
        id: i64,
        /// Unicode character offset for resumable reads.
        #[arg(long, default_value_t = 0)]
        offset_chars: usize,
        /// Maximum Unicode characters returned; omit for the remaining source.
        #[arg(long)]
        max_chars: Option<usize>,
    },
    /// List Wiki pages that cite a source.
    Refs {
        /// Numeric source ID.
        id: i64,
        /// Maximum citing pages returned (1..=1000).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Zero-based citing-page offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Remove one source only when no Wiki page cites it.
    Remove {
        /// Numeric source ID.
        id: i64,
    },
}

#[derive(Subcommand)]
enum IngestCommand {
    /// List durable ingest jobs by state.
    #[command(
        long_about = "List persistent source-ingest jobs. Jobs survive process exits and move through pending, analyzing, generating, completed, or failed.",
        after_help = "Examples:\n  lwc ingest list\n  lwc ingest list --status pending --limit 20\n  lwc ingest list --status failed"
    )]
    List {
        /// Exact state filter: pending, analyzing, generating, completed, or failed.
        #[arg(long)]
        status: Option<String>,
        /// Maximum jobs returned (1..=1000).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Zero-based job offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Atomically claim the next pending source and return Agent context.
    #[command(
        long_about = "Atomically claim the oldest pending job, mark it analyzing, increment its attempt count, and return the immutable source plus bounded Wiki context.\n\n\
Only one concurrent Agent can claim a given source. A null job means the queue has no pending work.",
        after_help = "Examples:\n  lwc ingest next --source-max-chars 100000\n  lwc ingest next --context-limit 100 --source-max-chars 50000"
    )]
    Next {
        /// Maximum pages and recent operations included in the packet (1..=1000).
        #[arg(long, default_value_t = 50)]
        context_limit: usize,
        /// Bound the claimed source body; continue with source show --offset-chars.
        #[arg(long)]
        source_max_chars: Option<usize>,
    },
    /// Atomically claim one specific pending source and return Agent context.
    Claim {
        /// Pending source ID to claim.
        source_id: i64,
        /// Maximum pages included in the packet (1..=1000).
        #[arg(long, default_value_t = 50)]
        context_limit: usize,
        /// Bound the claimed source body; continue with source show --offset-chars.
        #[arg(long)]
        source_max_chars: Option<usize>,
    },
    /// Persist an Agent's source analysis and move the job to generating.
    #[command(
        long_about = "Store the Agent's UTF-8 analysis for a claimed source and move the job from analyzing to generating.\n\n\
The analysis should identify claims, entities, concepts, contradictions, missing information, candidate page updates, and required citations.",
        after_help = "Example:\n  lwc ingest analyze 42 --file /tmp/source-42-analysis.md"
    )]
    Analyze {
        /// Claimed source ID.
        source_id: i64,
        /// UTF-8 analysis file, or '-' for stdin (maximum 64 MiB).
        #[arg(long)]
        file: PathBuf,
    },
    /// Complete a generated source after enforcing summary and integration gates.
    #[command(
        long_about = "Move a generating job to completed only after the source is cited by at least one kind=source summary and at least one non-source Wiki page.\n\n\
If the source legitimately changes no shared knowledge, provide a specific non-empty --no-derived-pages-reason. The reason is stored with the job and audited by lint. \
This prevents completion after merely indexing raw text or writing a detached summary.",
        after_help = "Examples:\n  lwc ingest complete 42\n  lwc ingest complete 42 --no-derived-pages-reason \"Duplicate evidence; existing synthesis already covers every supported claim\""
    )]
    Complete {
        /// Source ID whose Wiki integration is complete.
        source_id: i64,
        /// Explain why this source legitimately changes no non-source Wiki page.
        #[arg(long)]
        no_derived_pages_reason: Option<String>,
    },
    /// Record a recoverable ingest failure and preserve its diagnostic.
    #[command(after_help = "Example:\n  lwc ingest fail 42 --message \"source requires OCR\"")]
    Fail {
        /// Source ID being failed.
        source_id: i64,
        /// Non-empty failure reason shown by ingest list.
        #[arg(long)]
        message: String,
    },
    /// Return a failed or interrupted job to pending.
    #[command(
        long_about = "Reset a failed, analyzing, or generating job to pending so another Agent attempt can claim it. Existing source snapshots and Wiki pages are preserved.",
        after_help = "Example:\n  lwc ingest retry 42"
    )]
    Retry {
        /// Source ID to retry.
        source_id: i64,
    },
}

#[derive(Subcommand)]
enum GraphCommand {
    /// Rank pages related to one Wiki page.
    #[command(
        long_about = "Rank related pages deterministically using bidirectional wikilinks, shared source citations, and Adamic-Adar common neighbors; page-type affinity only refines candidates that already have structural evidence.\n\n\
The result exposes each scoring signal so Agents can explain why pages are related.",
        after_help = "Examples:\n  lwc graph related customer-membership\n  lwc graph related customer-membership --limit 50"
    )]
    Related {
        /// Existing Wiki page slug.
        slug: String,
        /// Maximum related pages returned (1..=1000).
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum MaintenanceCommand {
    /// Rebuild the complete Markdown/Obsidian projection from SQLite.
    #[command(
        long_about = "Rebuild .lwc/raw, .lwc/wiki, schema.md, purpose.md, index.md, overview.md, and log.md from the transactional store.\n\n\
SQLite is authoritative. Only files tracked by lwc's private projection manifest are replaced or removed; user files and raw/assets are preserved.",
        after_help = "Example:\n  lwc maintenance materialize"
    )]
    Materialize,
    /// Rebuild all FTS5 rows from canonical source and page content.
    #[command(
        long_about = "Transactionally delete and rebuild the derived SQLite FTS5 index from immutable sources and current Wiki pages, then refresh the Markdown operation log.\n\n\
Use this after an index-integrity lint issue or a tokenizer migration; normal source and page writes update the index automatically.",
        after_help = "Example:\n  lwc maintenance reindex"
    )]
    Reindex,
    /// Optimize FTS and truncate reusable WAL space when no reader blocks checkpointing.
    #[command(
        long_about = "Optimize the derived FTS5 index, record the maintenance pass, and run a best-effort WAL TRUNCATE checkpoint.\n\n\
The command reports busy=true instead of claiming compaction when an active reader prevents a complete checkpoint.",
        after_help = "Example:\n  lwc maintenance compact"
    )]
    Compact,
}

#[derive(Subcommand)]
enum CheckpointCommand {
    /// Create a named full-database checkpoint without changing Wiki knowledge.
    Create {
        /// Safe checkpoint name; an existing checkpoint is never overwritten.
        name: String,
    },
    /// List named checkpoints in deterministic order.
    List,
    /// Restore a checkpoint after automatically saving the current database.
    Restore {
        /// Existing checkpoint name.
        name: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchTarget {
    Auto,
    Page,
    Source,
    All,
}

#[derive(Subcommand)]
enum PageCommand {
    /// Create or replace one Agent-owned Wiki page.
    #[command(
        long_about = "Create or replace a persistent Wiki page, its source citations, wikilinks, FTS row, operation record, and Markdown projection in one logical update.\n\n\
Use [[slug]] in the Markdown body to create graph edges. Repeat --source for every immutable source supporting the page. \
Typical kinds are source, concept, entity, query, comparison, and synthesis. Every completed ingest requires a cited kind=source summary plus a cited non-source page, unless completion records a specific no-derived-pages reason.",
        after_help = "Examples:\n  lwc page put source-42 --title \"Paper summary\" --kind source --summary \"Main findings\" --file summary.md --source 42\n  lwc page put attention --title \"Attention\" --kind concept --summary \"Attention mechanisms\" --file concept.md --source 42 --source 57\n  lwc page put durable-answer --title \"Architecture decision\" --kind query --file answer.md --source 42"
    )]
    Put {
        /// Stable page identifier used in filenames and [[slug]] links.
        slug: String,
        /// Human-readable page title.
        #[arg(long)]
        title: String,
        /// Page category, such as source, concept, entity, query, comparison, or synthesis.
        #[arg(long, default_value = "concept")]
        kind: String,
        /// One-line description used by index.md, context, lists, and search results.
        #[arg(long, default_value = "")]
        summary: String,
        /// UTF-8 Markdown body, or '-' for stdin (maximum 64 MiB).
        #[arg(long)]
        file: PathBuf,
        /// Supporting source ID; repeat this option for multiple citations.
        #[arg(long = "source")]
        source_ids: Vec<i64>,
    },
    /// List page metadata without returning full Markdown bodies.
    #[command(
        long_about = "Return page slug, title, kind, summary, and update time in deterministic order without loading Markdown bodies.\n\n\
Read pages, limit, offset, and has_more from the JSON response. When has_more=true, add limit to offset and request the next page.",
        after_help = "Examples:\n  lwc page list --limit 100\n  lwc page list --limit 100 --offset 100"
    )]
    List {
        /// Maximum pages returned (1..=1000).
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Zero-based page offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Return one page with its Markdown, source IDs, and outgoing links.
    Show {
        /// Existing Wiki page slug.
        slug: String,
    },
    /// Return outgoing links, backlinks, and unresolved links for one page.
    Links {
        /// Existing Wiki page slug.
        slug: String,
    },
    /// Remove one page only when no other page links to it.
    Remove {
        /// Existing Wiki page slug.
        slug: String,
    },
}

fn main() {
    match run(Cli::parse()) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string(&json!({
                    "error": {"code": error.code, "message": error.message}
                }))
                .unwrap()
            );
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<Value> {
    let cwd = env::current_dir()?;
    match cli.command {
        Command::Init { no_git_exclude } => {
            ensure_scope_supported(cli.scope, false, "init")?;
            let store_path = init_store_path(cli.scope, &cwd)?;
            let git_exclude =
                configure_git_exclude(store_path.scope, &store_path.path, no_git_exclude)?;
            let (mut store, created) =
                Store::initialize(scope_name(store_path.scope), &store_path.path)?;
            store.materialize()?;
            Ok(json!({
                "scope": scope_name(store_path.scope),
                "database": store_path.path,
                "created": created,
                "git_exclude": git_exclude
            }))
        }
        Command::Schema { command } => {
            ensure_scope_supported(cli.scope, false, "schema")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                SchemaCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let schema = read_utf8(&file, true)?;
                    require_text("schema", &schema)?;
                    let response = store.schema_set(&schema)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                SchemaCommand::Show => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.schema_show()?)
                }
            }
        }
        Command::Purpose { command } => {
            ensure_scope_supported(cli.scope, false, "purpose")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                PurposeCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let purpose = read_utf8(&file, true)?;
                    require_text("purpose", &purpose)?;
                    let response = store.purpose_set(&purpose)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                PurposeCommand::Show => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.purpose_show()?)
                }
            }
        }
        Command::Source { command } => {
            ensure_scope_supported(cli.scope, false, "source")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                SourceCommand::Add {
                    file,
                    title,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    let input = prepare_source_input(
                        &store_path,
                        &file,
                        title,
                        allow_external_source,
                        acknowledge_sensitive_source,
                    )?;
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.source_add(input)?;
                    store.materialize()?;
                    to_json(response)
                }
                SourceCommand::AddDir {
                    directory,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    validate_source_scope(&store_path, &directory, allow_external_source)?;
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let files = collect_documents(&directory).map_err(|error| {
                        AppError::new("invalid_source_directory", error.to_string())
                    })?;
                    let discovered = files.len();
                    let mut created = 0usize;
                    let mut duplicates = 0usize;
                    let mut skipped_files = Vec::new();
                    for file in files {
                        let content = match read_utf8(&file, false) {
                            Ok(content) if !content.trim().is_empty() => content,
                            _ => {
                                skipped_files.push(file.display().to_string());
                                continue;
                            }
                        };
                        validate_sensitive_source(&file, &content, acknowledge_sensitive_source)?;
                        let title = file
                            .strip_prefix(&directory)
                            .unwrap_or(&file)
                            .to_string_lossy()
                            .replace('\\', "/");
                        let response = store.source_add(SourceAddInput {
                            title: Some(title),
                            origin: file.display().to_string(),
                            content,
                        })?;
                        if response.created {
                            created += 1;
                        } else {
                            duplicates += 1;
                        }
                    }
                    store.materialize()?;
                    if !skipped_files.is_empty() {
                        let examples = skipped_files
                            .iter()
                            .take(10)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(AppError::new(
                            "partial_import",
                            format!(
                                "imported {created} new and reused {duplicates} sources, but skipped {} files: {examples}; fix them and rerun (the import is idempotent)",
                                skipped_files.len()
                            ),
                        ));
                    }
                    Ok(json!({
                        "scope": scope_name(store_path.scope),
                        "database": store_path.path,
                        "discovered": discovered,
                        "created": created,
                        "duplicates": duplicates,
                        "skipped": skipped_files.len(),
                        "skipped_files": skipped_files,
                    }))
                }
                SourceCommand::AddManifest {
                    manifest,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    let raw = read_utf8(&manifest, false)?;
                    let parsed: SourceManifest = serde_json::from_str(&raw)
                        .map_err(|error| AppError::new("invalid_manifest", error.to_string()))?;
                    if parsed.sources.is_empty() {
                        return Err(AppError::new(
                            "invalid_manifest",
                            "manifest sources must not be empty",
                        ));
                    }
                    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
                    let mut paths = Vec::with_capacity(parsed.sources.len());
                    let mut inputs = Vec::with_capacity(parsed.sources.len());
                    for entry in parsed.sources {
                        let path = base.join(entry.path);
                        let input = prepare_source_input(
                            &store_path,
                            &path,
                            entry.title,
                            allow_external_source,
                            acknowledge_sensitive_source,
                        )?;
                        paths.push(path);
                        inputs.push(input);
                    }

                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let responses = store.source_add_many(inputs)?;
                    let created = responses.iter().filter(|response| response.created).count();
                    let duplicates = responses.len() - created;
                    let sources = paths
                        .into_iter()
                        .zip(responses)
                        .map(|(path, response)| {
                            json!({
                                "path": path,
                                "source": response.source,
                                "created": response.created
                            })
                        })
                        .collect::<Vec<_>>();
                    store.materialize()?;
                    Ok(json!({
                        "scope": scope_name(store_path.scope),
                        "database": store_path.path,
                        "created": created,
                        "duplicates": duplicates,
                        "sources": sources
                    }))
                }
                SourceCommand::List { limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.source_list(limit, offset)?)
                }
                SourceCommand::Show {
                    id,
                    offset_chars,
                    max_chars,
                } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_offset(offset_chars)?;
                    if max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "max-chars must be greater than zero",
                        ));
                    }
                    to_json(store.source_show(id, offset_chars, max_chars)?)
                }
                SourceCommand::Refs { id, limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.source_refs(id, limit, offset)?)
                }
                SourceCommand::Remove { id } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.source_remove(id)?;
                    store.materialize()?;
                    to_json(response)
                }
            }
        }
        Command::Page { command } => {
            ensure_scope_supported(cli.scope, false, "page")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                PageCommand::Put {
                    slug,
                    title,
                    kind,
                    summary,
                    file,
                    source_ids,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    require_text("slug", &slug)?;
                    require_text("title", &title)?;
                    require_text("kind", &kind)?;
                    let body = read_utf8(&file, true)?;
                    require_text("page body", &body)?;
                    let response = store.page_put(PagePutInput {
                        slug,
                        title,
                        kind: Some(kind),
                        summary: Some(summary),
                        body,
                        source_ids,
                    })?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                PageCommand::List { limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.page_list(limit, offset)?)
                }
                PageCommand::Show { slug } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.page_show(&slug)?)
                }
                PageCommand::Links { slug } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.page_links(&slug)?)
                }
                PageCommand::Remove { slug } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.page_remove(&slug)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
            }
        }
        Command::Ingest { command } => {
            ensure_scope_supported(cli.scope, false, "ingest")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                IngestCommand::List {
                    status,
                    limit,
                    offset,
                } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.ingest_list(status.as_deref(), limit, offset)?)
                }
                IngestCommand::Next {
                    context_limit,
                    source_max_chars,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(context_limit)?;
                    if source_max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "source-max-chars must be greater than zero",
                        ));
                    }
                    let response = store.ingest_next(context_limit, source_max_chars)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                IngestCommand::Claim {
                    source_id,
                    context_limit,
                    source_max_chars,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(context_limit)?;
                    if source_max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "source-max-chars must be greater than zero",
                        ));
                    }
                    let response =
                        store.ingest_claim(source_id, context_limit, source_max_chars)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                IngestCommand::Analyze { source_id, file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let analysis = read_utf8(&file, true)?;
                    require_text("analysis", &analysis)?;
                    let response = store.ingest_analyze(source_id, &analysis)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                IngestCommand::Complete {
                    source_id,
                    no_derived_pages_reason,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    if let Some(reason) = no_derived_pages_reason.as_deref() {
                        require_text("no-derived-pages-reason", reason)?;
                    }
                    let response =
                        store.ingest_complete(source_id, no_derived_pages_reason.as_deref())?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                IngestCommand::Fail { source_id, message } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    require_text("message", &message)?;
                    let response = store.ingest_fail(source_id, &message)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                IngestCommand::Retry { source_id } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.ingest_retry(source_id)?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
            }
        }
        Command::Graph { command } => {
            ensure_scope_supported(cli.scope, false, "graph")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            match command {
                GraphCommand::Related { slug, limit } => {
                    validate_limit(limit)?;
                    to_json(store.graph_related(&slug, limit)?)
                }
            }
        }
        Command::Maintenance { command } => {
            ensure_scope_supported(cli.scope, false, "maintenance")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
            match command {
                MaintenanceCommand::Materialize => to_json(store.materialize()?),
                MaintenanceCommand::Reindex => {
                    let response = store.reindex()?;
                    store.materialize_wiki()?;
                    to_json(response)
                }
                MaintenanceCommand::Compact => to_json(store.compact()?),
            }
        }
        Command::Checkpoint { command } => {
            ensure_scope_supported(cli.scope, false, "checkpoint")?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            match command {
                CheckpointCommand::Create { name } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.checkpoint_create(&name)?)
                }
                CheckpointCommand::List => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.checkpoint_list()?)
                }
                CheckpointCommand::Restore { name } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.checkpoint_restore(&name)?;
                    store.materialize()?;
                    to_json(response)
                }
            }
        }
        Command::Search {
            query,
            target,
            kinds,
            limit,
            record,
        } => {
            require_text("query", &query)?;
            validate_limit(limit)?;
            for kind in &kinds {
                require_text("kind", kind)?;
            }
            if matches!(target, SearchTarget::Source) && !kinds.is_empty() {
                return Err(AppError::new(
                    "invalid_input",
                    "--kind filters Wiki pages and cannot be combined with --type source",
                ));
            }
            let mode = match target {
                SearchTarget::Auto => SearchMode::Auto,
                SearchTarget::Page => SearchMode::Page,
                SearchTarget::Source => SearchMode::Source,
                SearchTarget::All => SearchMode::All,
            };
            let options = SearchOptions { mode, kinds };
            let paths = resolve_read_store_paths(cli.scope, &cwd, true)?;
            let mut stores = if record {
                paths
                    .into_iter()
                    .map(|store_path| Store::open(scope_name(store_path.scope), &store_path.path))
                    .collect::<Result<Vec<_>>>()?
            } else {
                paths
                    .into_iter()
                    .map(|store_path| {
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            let mut results = Vec::new();
            for store in &stores {
                results.extend(store.search_with_options(&query, limit, &options)?.results);
            }
            if record {
                for store in &mut stores {
                    store.record_search(&query, limit)?;
                    store.materialize_wiki()?;
                }
            }
            results.sort_by(|left, right| {
                search_type_priority(&left.result_type, mode)
                    .cmp(&search_type_priority(&right.result_type, mode))
                    .then_with(|| left.rank.total_cmp(&right.rank))
                    .then_with(|| scope_priority(&left.scope).cmp(&scope_priority(&right.scope)))
                    .then_with(|| left.result_type.cmp(&right.result_type))
                    .then_with(|| left.identifier.cmp(&right.identifier))
            });
            results.truncate(limit);
            Ok(json!({"results": results}))
        }
        Command::Context { limit } => {
            validate_limit(limit)?;
            let paths = resolve_read_store_paths(cli.scope, &cwd, true)?;
            let mut stores = Vec::new();
            for store_path in paths {
                let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                stores.push(store.context_store(limit)?);
            }
            Ok(json!({"stores": stores}))
        }
        Command::Lint {
            limit,
            offset,
            record,
        } => {
            ensure_scope_supported(cli.scope, false, "lint")?;
            validate_limit(limit)?;
            validate_offset(offset)?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            if record {
                let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                let response = store.lint(limit, offset)?;
                store.record_lint(response.total)?;
                store.materialize_wiki()?;
                to_json(response)
            } else {
                let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                to_json(store.lint(limit, offset)?)
            }
        }
        Command::Log { limit } => {
            ensure_scope_supported(cli.scope, false, "log")?;
            validate_limit(limit)?;
            let store_path = resolve_store_path(cli.scope, &cwd)?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            to_json(store.log(limit)?)
        }
    }
}

fn read_utf8(path: &Path, allow_stdin: bool) -> Result<String> {
    let bytes = if path == Path::new("-") {
        if !allow_stdin {
            return Err(AppError::new(
                "invalid_input",
                "stdin is not supported here",
            ));
        }
        let mut bytes = Vec::new();
        io::stdin()
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure_input_size(path, bytes.len() as u64, MAX_INPUT_BYTES)?;
        bytes
    } else {
        read_file_bounded(path, MAX_INPUT_BYTES)?
    };
    String::from_utf8(bytes)
        .map_err(|_| AppError::new("invalid_utf8", format!("{} is not UTF-8", path.display())))
}

fn read_file_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    ensure_input_size(path, fs::metadata(path)?.len(), max_bytes)?;
    let bytes = fs::read(path)?;
    ensure_input_size(path, bytes.len() as u64, max_bytes)?;
    Ok(bytes)
}

fn ensure_input_size(path: &Path, bytes: u64, max_bytes: u64) -> Result<()> {
    if bytes > max_bytes {
        return Err(AppError::new(
            "input_too_large",
            format!(
                "{} is {bytes} bytes; maximum supported input is {max_bytes} bytes",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn require_text(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} must not be empty"),
        ));
    }
    Ok(())
}

fn prepare_source_input(
    store_path: &StorePath,
    path: &Path,
    title: Option<String>,
    allow_external_source: bool,
    acknowledge_sensitive_source: bool,
) -> Result<SourceAddInput> {
    if path == Path::new("-") {
        return Err(AppError::new(
            "invalid_input",
            "source add requires a file path",
        ));
    }
    if let Some(title) = title.as_deref() {
        require_text("title", title)?;
    }
    let resolved = validate_source_scope(store_path, path, allow_external_source)?;
    let content = read_utf8(&resolved, false)?;
    require_text("source content", &content)?;
    validate_sensitive_source(&resolved, &content, acknowledge_sensitive_source)?;
    Ok(SourceAddInput {
        title,
        origin: path.display().to_string(),
        content,
    })
}

fn validate_source_scope(
    store_path: &StorePath,
    path: &Path,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    if store_path.scope == Scope::Project && !allow_external_source {
        let project_root = store_path
            .path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                AppError::new(
                    "invalid_store_path",
                    "project Wiki path has no project root",
                )
            })?;
        let project_root = fs::canonicalize(project_root)?;
        if !resolved.starts_with(&project_root) {
            return Err(AppError::new(
                "external_source_requires_acknowledgement",
                format!(
                    "source {} resolves outside project root {}; retry with --allow-external-source only after confirming it belongs in this Wiki",
                    resolved.display(),
                    project_root.display()
                ),
            ));
        }
    }
    Ok(resolved)
}

fn validate_sensitive_source(path: &Path, content: &str, acknowledged: bool) -> Result<()> {
    if acknowledged {
        return Ok(());
    }
    let mut reasons = Vec::new();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let env_file = name == ".env"
        || (name.starts_with(".env.")
            && ![".env.example", ".env.sample", ".env.template"].contains(&name.as_str()));
    let private_file = matches!(
        name.as_str(),
        "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
    ) || path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            ["key", "p12", "pfx"]
                .iter()
                .any(|extension| value.eq_ignore_ascii_case(extension))
        })
        .unwrap_or(false);
    if env_file {
        reasons.push("environment credential file");
    }
    if private_file {
        reasons.push("private-key or credential file");
    }
    if [
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "-----BEGIN PGP PRIVATE KEY BLOCK-----",
    ]
    .iter()
    .any(|marker| content.contains(marker))
    {
        reasons.push("private-key marker");
    }
    if [
        ("AKIA", 20),
        ("ASIA", 20),
        ("ghp_", 20),
        ("github_pat_", 24),
        ("sk-proj-", 24),
        ("xoxb-", 24),
        ("xoxp-", 24),
    ]
    .iter()
    .any(|(prefix, minimum)| contains_token(content, prefix, *minimum))
    {
        reasons.push("known credential prefix");
    }

    if reasons.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        "possible_secret_detected",
        format!(
            "possible sensitive source {}: {}; inspect it and retry with --acknowledge-sensitive-source only when the immutable snapshot is safe",
            path.display(),
            reasons.join(", ")
        ),
    ))
}

fn contains_token(content: &str, prefix: &str, minimum_length: usize) -> bool {
    content.match_indices(prefix).any(|(index, _)| {
        content[index..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
            .count()
            >= minimum_length
    })
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=1000).contains(&limit) {
        return Err(AppError::new(
            "invalid_limit",
            "limit must be between 1 and 1000",
        ));
    }
    Ok(())
}

fn validate_offset(offset: usize) -> Result<()> {
    if i64::try_from(offset).is_err() {
        return Err(AppError::new(
            "invalid_offset",
            "offset must fit in a signed 64-bit integer",
        ));
    }
    Ok(())
}

fn scope_priority(scope: &str) -> u8 {
    if scope == "project" { 0 } else { 1 }
}

fn search_type_priority(result_type: &str, mode: SearchMode) -> u8 {
    if matches!(mode, SearchMode::Auto | SearchMode::All) && result_type == "source" {
        1
    } else {
        0
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

fn configure_git_exclude(scope: Scope, database: &Path, disabled: bool) -> Result<Value> {
    if scope != Scope::Project {
        return Ok(json!({ "status": "not_applicable" }));
    }
    if disabled {
        return Ok(json!({ "status": "disabled" }));
    }

    let project_root = database
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| AppError::new("git_exclude_failed", "project Wiki path has no root"))?;
    let top_level = match git_output(project_root, &["rev-parse", "--show-toplevel"])? {
        Some(output) => PathBuf::from(output),
        None => return Ok(json!({ "status": "not_git" })),
    };
    let top_level = fs::canonicalize(top_level)?;
    let project_root = fs::canonicalize(project_root)?;
    let relative = project_root.strip_prefix(&top_level).map_err(|_| {
        AppError::new(
            "git_exclude_failed",
            format!(
                "project root {} is outside Git root {}",
                project_root.display(),
                top_level.display()
            ),
        )
    })?;
    let relative_lwc = relative.join(".lwc");
    let git_path = relative_lwc.to_string_lossy().replace('\\', "/");
    let pattern = format!("/{git_path}/");

    let ignored = ProcessCommand::new("git")
        .current_dir(&top_level)
        .args(["check-ignore", "-q", "--no-index", "--", &git_path])
        .status()
        .map_err(|error| AppError::new("git_exclude_failed", error.to_string()))?;
    if ignored.success() {
        return Ok(json!({ "status": "already_ignored", "pattern": pattern }));
    }
    if ignored.code() != Some(1) {
        return Err(AppError::new(
            "git_exclude_failed",
            "git check-ignore failed",
        ));
    }

    let exclude_text = git_output(&top_level, &["rev-parse", "--git-path", "info/exclude"])?
        .ok_or_else(|| AppError::new("git_exclude_failed", "Git exclude path is unavailable"))?;
    let exclude_path = {
        let path = PathBuf::from(exclude_text);
        if path.is_absolute() {
            path
        } else {
            top_level.join(path)
        }
    };
    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = match fs::read_to_string(&exclude_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::new("git_exclude_failed", error.to_string())),
    };
    if !existing.lines().any(|line| line.trim() == pattern) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        writeln!(file, "{pattern}")?;
    }
    Ok(json!({
        "status": "added",
        "path": exclude_path,
        "pattern": pattern
    }))
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = match ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::new("git_exclude_failed", error.to_string())),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| AppError::new("git_exclude_failed", "Git returned non-UTF-8 output"))?;
    Ok(Some(value.trim().to_string()))
}

fn to_json<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|error| AppError::new("serialization_error", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::read_file_bounded;

    #[test]
    fn bounded_file_read_rejects_oversized_input() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oversized.txt");
        std::fs::write(&path, b"12345").unwrap();

        let error = read_file_bounded(&path, 4).unwrap_err();

        assert_eq!(error.code, "input_too_large");
    }
}
