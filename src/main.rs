mod artifacts;
mod changeset;
mod config;
mod error;
mod external_graph;
pub mod graph;
mod import;
mod scope;
pub mod segment;
mod source_diff;
mod store;
pub mod tokenize;
mod work;

use clap::{Parser, Subcommand, ValueEnum};
use error::{AppError, Result};
use import::collect_documents;
use scope::{
    Scope, StorePath, ensure_project_path, ensure_scope_supported, init_store_path,
    resolve_read_store_paths as resolve_live_read_store_paths,
    resolve_store_path as resolve_live_store_path,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source_diff::{
    DEFAULT_DIFF_OUTPUT_CHARS, MAX_DIFF_INPUT_BYTES, MAX_DIFF_OUTPUT_CHARS, render_diff,
};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    process::Command as ProcessCommand,
    time::{SystemTime, UNIX_EPOCH},
};
use store::{
    PagePutInput, SearchGranularity, SearchGrouping, SearchMode, SearchOptions, SourceAddInput,
    Store,
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(any(target_os = "android", target_os = "linux"))]
const LIVE_SOURCE_NONBLOCK: i32 = 0o4000;
#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const LIVE_SOURCE_NONBLOCK: i32 = 0x0004;

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

#[derive(Serialize)]
struct SourceStatusResponse {
    scope: String,
    database: String,
    checked_at_unix_ms: u128,
    checks: Vec<SourceStatusCheck>,
    untracked_source_ids: Vec<i64>,
}

#[derive(Serialize)]
struct SourceStatusCheck {
    requested_source_id: i64,
    tracked_path: String,
    head_source_id: i64,
    head_revision: i64,
    lineage_state: &'static str,
    filesystem_state: &'static str,
    head_content_hash: String,
    live_content_hash: Option<String>,
    live_bytes: Option<u64>,
    message: Option<String>,
}

struct LiveSourceStatus {
    state: &'static str,
    content_hash: Option<String>,
    bytes: Option<u64>,
    message: Option<String>,
}

enum PreparedLiveSource {
    Ready {
        path: PathBuf,
        file: fs::File,
        before: FileFingerprint,
    },
    Terminal {
        path: PathBuf,
        observed: Option<FileFingerprint>,
        status: LiveSourceStatus,
    },
}

#[derive(PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
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
Atomic multi-command changes:\n  \
1. lwc changeset begin <NAME>\n  \
2. Route supported reads and writes with --changeset <NAME>.\n  \
3. lwc --changeset <NAME> lint\n  \
4. lwc changeset show <NAME>\n  \
5. lwc changeset commit <NAME>\n  \
Use changeset discard before commit, or rollback the exact returned ID before any later live write.\n\n\
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

    /// Run a supported command against an isolated draft changeset.
    #[arg(long, global = true, value_name = "NAME")]
    changeset: Option<String>,

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
    /// Inspect and control long-running Wiki work.
    Work {
        #[command(subcommand)]
        command: WorkCommand,
    },
    #[command(name = "__work-run", hide = true)]
    WorkRun {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        id: String,
    },
    /// Stage, inspect, publish, discard, or roll back an atomic Wiki changeset.
    Changeset {
        #[command(subcommand)]
        command: ChangesetCommand,
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
        after_help = "When to use:\n  Use `add` for one curated source, `add-manifest` for an atomic reviewed set, and `add-dir` for a deterministic UTF-8 text corpus. Use targeted `status` before relying on tracked live files, `show` for exact immutable evidence, and `refs` to find every citing Wiki page.\n\nNext action:\n  Claim returned manifest IDs with `lwc ingest claim`; otherwise use `lwc ingest next`. Do not treat a pending job as integrated knowledge."
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
    /// Inspect and update the layered graph engine setting.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Explore, explain, verify, and maintain the selected document graph.
    #[command(
        long_about = "Explore current Page and Source documents, wikilinks, citations, and explicit semantic relationships in the selected Grafeo or SurrealDB graph. Graph storage is disabled by default; enabled rebuilds and updates commit one document before the next.",
        after_help = "Examples:\n  lwc graph explore\n  lwc graph neighbors page:policy --direction outgoing\n  lwc graph path page:implementation page:policy\n  lwc graph impact page:policy\n  lwc graph overview\n  lwc graph status\n  lwc graph verify\n\nSemantic claims are explicit: use `graph relation set/list/retract` with provenance, reason, confidence, and supporting Source IDs."
    )]
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    /// Apply explicit, bounded retrieval adjustments and query-specific feedback.
    #[command(
        long_about = "Manage project-local retrieval adjustments. Document weights affect only already-matching candidates; feedback applies only to the same token fingerprint. User-provided values override Agent-observed values without deleting either row.",
        after_help = "Examples:\n  lwc weight set page payment-rules --value 2 --reason \"canonical specification\" --provenance agent-observed\n  lwc weight feedback page payment-rules --query \"payment rules\" --signal relevant --reason \"verified result\" --provenance user-provided\n  lwc weight list page payment-rules\n  lwc weight clear page payment-rules --provenance agent-observed"
    )]
    Weight {
        #[command(subcommand)]
        command: WeightCommand,
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
Use --granularity sentence or passage for direct span retrieval; --granularity all applies deterministic reciprocal-rank fusion and groups by document unless --group-by none is explicit. \
Read results[].type, kind, identifier, title, snippet, rank, and scope; a lower numeric rank is more relevant. \
The command is read-only by default and does not persist the query. Use --record only when the query itself should become part of the operation history. \
Use --scope all to merge project and global results; ranking remains deterministic across stores. \
Combining --scope all with --record appends the query operation to each selected store.",
        after_help = "Examples:\n  lwc search \"注意力机制\"\n  lwc search \"release policy\" --type page --kind concept --kind synthesis --limit 10\n  lwc search \"exact evidence\" --type source\n  lwc search \"audit both layers\" --type all\n  lwc search \"exact context\" --granularity sentence\n  lwc search \"mixed context\" --granularity all --group-by document\n  lwc --scope all search \"shared convention\"\n  lwc search \"durable research question\" --record"
    )]
    Search {
        /// Natural-language or keyword query. FTS syntax is escaped automatically.
        query: String,
        /// Search auto-ranked Wiki pages with source fallback, pages only, sources only, or both.
        #[arg(long = "type", value_enum, default_value = "auto")]
        target: SearchTarget,
        /// Retrieve whole documents, passages, sentences, or all granularities.
        #[arg(long, value_enum, default_value = "document")]
        granularity: SearchGranularityArg,
        /// Group all-granularity matches by owning document.
        #[arg(long, value_enum, default_value = "auto")]
        group_by: SearchGroupArg,
        /// Restrict page results to this kind; repeat for multiple kinds.
        #[arg(long = "kind")]
        kinds: Vec<String>,
        /// Maximum number of merged results to return (1..=1000).
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Persist the query; with scope=all, append it to each selected store.
        #[arg(long)]
        record: bool,
        /// Include exact bounded score signals and arithmetic for every result.
        #[arg(long)]
        explain: bool,
    },
    /// Resolve stable sentence and passage locators and expand local context.
    Span {
        #[command(subcommand)]
        command: SpanCommand,
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
enum ChangesetCommand {
    /// Create an isolated draft from the current live Wiki.
    Begin { name: String },
    /// List isolated drafts for the selected Wiki.
    List,
    /// Inspect staged operation metadata without running lint.
    Show {
        name: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Atomically publish one isolated draft.
    Commit {
        name: String,
        #[arg(long)]
        allow_lint_issues: bool,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Delete one isolated draft without touching the live Wiki.
    Discard { name: String },
    /// Restore the exact pre-commit snapshot when no later live write exists.
    Rollback { changeset_id: String },
}

#[derive(Subcommand)]
enum WorkCommand {
    /// List recent work for the selected Wiki.
    List,
    /// Read the latest progress for one work item.
    Status { id: String },
    /// Wait until one work item reaches a terminal state.
    Watch { id: String },
    /// Request cooperative cancellation.
    Cancel { id: String },
    /// Resume failed, cancelled, or stale interrupted work.
    Resume { id: String },
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
    /// Compare tracked files with their latest immutable snapshots.
    #[command(
        long_about = "Read-only exact freshness check. For selected source IDs, report every tracked path where each source appeared, the current path head, and a streaming SHA-256 comparison with the live file. Use --all only for explicit maintenance.",
        after_help = "Examples:\n  lwc source status 7 12\n  lwc source status --all\n  lwc source status 7 --allow-external-source"
    )]
    Status {
        /// Source IDs to check; repeat positionally for a targeted batch.
        #[arg(value_name = "SOURCE_ID", num_args = 1.., required_unless_present = "all", conflicts_with = "all")]
        source_ids: Vec<i64>,
        /// Check every currently tracked path.
        #[arg(long)]
        all: bool,
        /// Permit reading a tracked project source outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
    },
    /// Compare an immutable source with its live file or another snapshot.
    #[command(
        long_about = "Read-only bounded text comparison. By default, compare SOURCE_ID with its current tracked file. If the source has multiple tracked paths, select one exactly with --path. Use --to-source to compare two immutable snapshots without reading the filesystem.",
        after_help = "Examples:\n  lwc source diff 7\n  lwc source diff 7 --path docs/design.md\n  lwc source diff 7 --to-source 21\n  lwc source diff 7 --max-chars 100000\n\nThe unified diff is limited to 8 MiB and 200000 lines per side, three context lines, and a Unicode-safe output preview. This command never changes sources, pages, citations, or the operation log."
    )]
    Diff {
        /// Immutable source ID used as the old side.
        id: i64,
        /// Exact tracked path when the source has more than one.
        #[arg(long)]
        path: Option<String>,
        /// Compare with another immutable source instead of a live file.
        #[arg(
            long,
            value_name = "SOURCE_ID",
            conflicts_with_all = ["path", "allow_external_source", "acknowledge_sensitive_source"]
        )]
        to_source: Option<i64>,
        /// Maximum Unicode characters returned (1..=100000).
        #[arg(long, default_value_t = DEFAULT_DIFF_OUTPUT_CHARS)]
        max_chars: usize,
        /// Permit reading a tracked project source outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
        /// Confirm that a flagged live source is safe to reveal in the diff.
        #[arg(long)]
        acknowledge_sensitive_source: bool,
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
enum ConfigCommand {
    /// Show effective graph configuration and value origins.
    Show,
    /// Atomically set the graph engine setting.
    Set {
        /// Select disabled, grafeo, surrealdb, or inherit.
        #[arg(long)]
        graph: String,
    },
    /// Restore the graph engine setting to its inherited default.
    Unset {
        #[arg(long)]
        graph: bool,
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
    /// Traverse typed document relationships from a node.
    Explore {
        identifier: Option<String>,
        #[arg(long, default_value_t = 2)]
        depth: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, value_enum, default_value = "both")]
        direction: GraphDirectionArg,
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
    },
    /// Resolve one node and report bounded degree metadata.
    Node { identifier: String },
    /// Return immediate typed neighbors of one node.
    Neighbors {
        identifier: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, value_enum, default_value = "both")]
        direction: GraphDirectionArg,
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
    },
    /// Find and explain a shortest typed relationship path.
    Path {
        from: String,
        to: String,
        #[arg(long, default_value_t = 6)]
        max_depth: usize,
        #[arg(long, default_value_t = 200)]
        limit: usize,
        #[arg(long, value_enum, default_value = "outgoing")]
        direction: GraphDirectionArg,
        #[arg(long = "edge-type")]
        edge_types: Vec<String>,
    },
    /// Propagate reverse dependency impact with hard/review classification.
    Impact {
        identifier: String,
        #[arg(long, default_value_t = 4)]
        max_depth: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Summarize current document and relationship counts and hubs.
    Overview {
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Report the selected engine and projected document count.
    Status,
    /// Verify current document identities and fingerprints against SQLite.
    Verify,
    /// Persist explicit semantic relationships with provenance.
    Relation {
        #[command(subcommand)]
        command: GraphRelationCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum GraphDirectionArg {
    Outgoing,
    Incoming,
    Both,
}

impl GraphDirectionArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Outgoing => "outgoing",
            Self::Incoming => "incoming",
            Self::Both => "both",
        }
    }
}

#[derive(Subcommand)]
enum GraphRelationCommand {
    /// Create or replace one explicit semantic edge.
    Set {
        from: String,
        relation_type: String,
        to: String,
        #[arg(long)]
        provenance: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        confidence: f64,
        /// Supporting immutable Source IDs; repeat for multiple sources.
        #[arg(long = "source")]
        source_ids: Vec<i64>,
    },
    /// List explicit semantic relationships with optional endpoint/type filters.
    List {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long = "type")]
        relation_type: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Retract one explicit semantic relationship with an audit reason.
    Retract {
        from: String,
        relation_type: String,
        to: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum WeightCommand {
    /// Set or replace one bounded document adjustment.
    Set {
        #[arg(value_enum)]
        target: WeightTargetArg,
        identifier: String,
        #[arg(long, allow_hyphen_values = true)]
        value: String,
        #[arg(long)]
        reason: String,
        #[arg(long, value_enum)]
        provenance: WeightProvenanceArg,
    },
    /// List both provenance rows and the effective document adjustment.
    List {
        #[arg(value_enum)]
        target: WeightTargetArg,
        identifier: String,
    },
    /// Clear one provenance row without changing the other.
    Clear {
        #[arg(value_enum)]
        target: WeightTargetArg,
        identifier: String,
        #[arg(long, value_enum)]
        provenance: WeightProvenanceArg,
    },
    /// Record an explicit query-specific relevant or irrelevant judgment.
    Feedback {
        #[arg(value_enum)]
        target: WeightTargetArg,
        identifier: String,
        #[arg(long)]
        query: String,
        #[arg(long, value_enum)]
        signal: FeedbackSignalArg,
        #[arg(long)]
        reason: String,
        #[arg(long, value_enum)]
        provenance: WeightProvenanceArg,
    },
    /// Clear one query-specific feedback row.
    FeedbackClear {
        #[arg(value_enum)]
        target: WeightTargetArg,
        identifier: String,
        #[arg(long)]
        query: String,
        #[arg(long, value_enum)]
        provenance: WeightProvenanceArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WeightTargetArg {
    Page,
    Source,
}

impl WeightTargetArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Source => "source",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WeightProvenanceArg {
    UserProvided,
    AgentObserved,
}

impl WeightProvenanceArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserProvided => "user-provided",
            Self::AgentObserved => "agent-observed",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum FeedbackSignalArg {
    Relevant,
    Irrelevant,
}

impl FeedbackSignalArg {
    fn value(self) -> i32 {
        match self {
            Self::Relevant => 1,
            Self::Irrelevant => -1,
        }
    }
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchGranularityArg {
    Document,
    Passage,
    Sentence,
    All,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SearchGroupArg {
    Auto,
    None,
    Document,
}

#[derive(Debug, Subcommand)]
enum SpanCommand {
    /// Return the exact indexed text and locator metadata for a span.
    Get { identifier: String },
    /// Expand a span to its parent, bounded siblings, and bounded children.
    Expand {
        identifier: String,
        #[arg(long, default_value_t = 1)]
        before: usize,
        #[arg(long, default_value_t = 1)]
        after: usize,
        #[arg(long, default_value_t = 20)]
        children: usize,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProvenanceArg {
    UserProvided,
    AgentObserved,
    Hypothesis,
}

impl ProvenanceArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserProvided => "user-provided",
            Self::AgentObserved => "agent-observed",
            Self::Hypothesis => "hypothesis",
        }
    }
}

#[derive(Subcommand)]
enum PageCommand {
    /// Create or replace one Agent-owned Wiki page.
    #[command(
        long_about = "Create or replace a persistent Wiki page, its source citations, wikilinks, FTS row, operation record, and Markdown projection in one logical update.\n\n\
Use [[slug]] in the Markdown body to create graph edges. Repeat --source for every immutable source supporting the page; source citations automatically add source-grounded provenance. Repeat --provenance for user-provided facts, Agent observations, or hypotheses. \
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
        /// Explicit non-source provenance; repeat for mixed pages. Page replacement also replaces this set.
        #[arg(long = "provenance")]
        provenance: Vec<ProvenanceArg>,
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
            let mut payload = json!({"code": error.code, "message": error.message});
            if let Some(details) = error.details {
                payload["details"] = details;
            }
            eprintln!(
                "{}",
                serde_json::to_string(&json!({"error": payload})).unwrap()
            );
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<Value> {
    let cwd = env::current_dir()?;
    let selected_changeset = cli.changeset.clone();
    if !matches!(
        &cli.command,
        Command::Init { .. } | Command::Work { .. } | Command::WorkRun { .. }
    ) {
        let paths = if cli.scope == Scope::All {
            resolve_live_read_store_paths(cli.scope, &cwd, true)?
        } else {
            vec![resolve_live_store_path(cli.scope, &cwd)?]
        };
        for path in paths {
            if work::schema_migration_needed(&path.path)? {
                Store::open(scope_name(path.scope), &path.path)?;
            }
        }
    }
    match cli.command {
        Command::Init { no_git_exclude } => {
            changeset::reject_selector(selected_changeset.as_deref(), "init")?;
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
        Command::Work { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "work")?;
            ensure_scope_supported(cli.scope, false, "work")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                WorkCommand::List => work::list(&store_path),
                WorkCommand::Status { id } => work::status(&store_path, &id),
                WorkCommand::Watch { id } => work::watch(&store_path, &id),
                WorkCommand::Cancel { id } => work::cancel(&store_path, &id),
                WorkCommand::Resume { id } => work::resume(&store_path, &id),
            }
        }
        Command::WorkRun { root, id } => {
            changeset::reject_selector(selected_changeset.as_deref(), "work runner")?;
            work::run(&root, &id)
        }
        Command::Changeset { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "changeset")?;
            ensure_scope_supported(cli.scope, false, "changeset")?;
            let live = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                ChangesetCommand::Begin { name } => to_json(changeset::begin(&live, &name)?),
                ChangesetCommand::List => to_json(changeset::list(&live, 1000)?),
                ChangesetCommand::Show { name, limit } => {
                    validate_limit(limit)?;
                    to_json(changeset::show(&live, &name, limit)?)
                }
                ChangesetCommand::Commit {
                    name,
                    allow_lint_issues,
                    reason,
                } => to_json(changeset::commit(
                    &live,
                    &name,
                    allow_lint_issues,
                    reason.as_deref(),
                )?),
                ChangesetCommand::Discard { name } => to_json(changeset::discard(&live, &name)?),
                ChangesetCommand::Rollback { changeset_id } => {
                    to_json(changeset::rollback(&live, &changeset_id)?)
                }
            }
        }
        Command::Schema { command } => {
            ensure_scope_supported(cli.scope, false, "schema")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                SchemaCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let schema = read_utf8(&file, true)?;
                    require_text("schema", &schema)?;
                    let response = store.schema_set(&schema)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
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
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                PurposeCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let purpose = read_utf8(&file, true)?;
                    require_text("purpose", &purpose)?;
                    let response = store.purpose_set(&purpose)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
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
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
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
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                SourceCommand::AddDir {
                    directory,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    validate_source_scope(&store_path, &directory, allow_external_source)?;
                    let files = collect_documents(&directory).map_err(|error| {
                        AppError::new("invalid_source_directory", error.to_string())
                    })?;
                    let discovered = files.len();
                    let mut skipped_files = Vec::new();
                    let inputs = files.into_iter().map(|file| {
                        let resolved = match validate_source_scope(
                            &store_path,
                            &file,
                            allow_external_source,
                        ) {
                            Ok(path) => path,
                            Err(error) if error.code == "io_error" => {
                                skipped_files.push(file.display().to_string());
                                return Ok(None);
                            }
                            Err(error) => return Err(error),
                        };
                        let content = match read_utf8(&resolved, false) {
                            Ok(content) if !content.trim().is_empty() => content,
                            _ => {
                                skipped_files.push(file.display().to_string());
                                return Ok(None);
                            }
                        };
                        validate_sensitive_source(
                            &resolved,
                            &content,
                            acknowledge_sensitive_source,
                        )?;
                        let title = file
                            .strip_prefix(&directory)
                            .unwrap_or(&file)
                            .to_string_lossy()
                            .replace('\\', "/");
                        Ok(Some(SourceAddInput {
                            title: Some(title),
                            origin: file.display().to_string(),
                            tracked_path: Some(tracked_source_path(&store_path, &resolved)?),
                            content,
                        }))
                    });
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let responses = store.source_add_stream(inputs)?;
                    let created = responses.iter().filter(|response| response.created).count();
                    let duplicates = responses.len() - created;
                    if !responses.is_empty() {
                        materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    }
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
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
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
                SourceCommand::Status {
                    source_ids,
                    all,
                    allow_external_source,
                } => {
                    let mut store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    let selected = store.source_status_targets(source_ids.clone(), all)?;
                    let mut resolved = BTreeMap::new();
                    for target in &selected.targets {
                        if !resolved.contains_key(&target.tracked_path) {
                            resolved.insert(
                                target.tracked_path.clone(),
                                resolve_tracked_source_path(
                                    &store_path,
                                    &target.tracked_path,
                                    allow_external_source,
                                )?,
                            );
                        }
                    }
                    let mut live = BTreeMap::new();
                    for (tracked_path, path) in resolved {
                        let source = prepare_live_source(
                            &store_path,
                            path,
                            allow_external_source,
                            MAX_INPUT_BYTES,
                        )?;
                        live.insert(tracked_path, inspect_prepared_source(source));
                    }
                    let current = store.source_status_targets(source_ids, all)?;
                    ensure_source_status_unchanged(&selected, &current)?;
                    let checks = selected
                        .targets
                        .into_iter()
                        .map(|target| {
                            let live = live
                                .get(&target.tracked_path)
                                .expect("every preflighted path is inspected");
                            let filesystem_state = if live.state == "hashed" {
                                if live.content_hash.as_deref()
                                    == Some(target.head_content_hash.as_str())
                                {
                                    "current"
                                } else {
                                    "modified"
                                }
                            } else {
                                live.state
                            };
                            SourceStatusCheck {
                                requested_source_id: target.requested_source_id,
                                tracked_path: target.tracked_path,
                                head_source_id: target.head_source_id,
                                head_revision: target.head_revision,
                                lineage_state: if target.requested_source_id
                                    == target.head_source_id
                                {
                                    "current"
                                } else {
                                    "superseded"
                                },
                                filesystem_state,
                                head_content_hash: target.head_content_hash,
                                live_content_hash: live.content_hash.clone(),
                                live_bytes: live.bytes,
                                message: live.message.clone(),
                            }
                        })
                        .collect();
                    to_json(SourceStatusResponse {
                        scope: scope_name(store_path.scope).to_string(),
                        database: store_path.path.display().to_string(),
                        checked_at_unix_ms: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(|error| AppError::new("system_time_error", error.to_string()))?
                            .as_millis(),
                        checks,
                        untracked_source_ids: selected.untracked_source_ids,
                    })
                }
                SourceCommand::Diff {
                    id,
                    path,
                    to_source,
                    max_chars,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => run_source_diff(
                    &store_path,
                    id,
                    path.as_deref(),
                    to_source,
                    max_chars,
                    allow_external_source,
                    acknowledge_sensitive_source,
                ),
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
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Page { command } => {
            ensure_scope_supported(cli.scope, false, "page")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                PageCommand::Put {
                    slug,
                    title,
                    kind,
                    summary,
                    file,
                    source_ids,
                    provenance,
                } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_page_touch(&live, name, &slug, &source_ids)?;
                    }
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
                        provenance: provenance
                            .into_iter()
                            .map(|value| value.as_str().to_string())
                            .collect(),
                    })?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
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
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_page_touch(&live, name, &slug, &[])?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.page_remove(&slug)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Ingest { command } => {
            ensure_scope_supported(cli.scope, false, "ingest")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
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
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
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
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Analyze { source_id, file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let analysis = read_utf8(&file, true)?;
                    require_text("analysis", &analysis)?;
                    let response = store.ingest_analyze(source_id, &analysis)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
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
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Fail { source_id, message } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    require_text("message", &message)?;
                    let response = store.ingest_fail(source_id, &message)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Retry { source_id } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.ingest_retry(source_id)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Config { command } => {
            ensure_scope_supported(cli.scope, false, "config")?;
            if selected_changeset.is_some() {
                return Err(AppError::new(
                    "changeset_command_not_supported",
                    "graph configuration is deployment-local and cannot be changed in a changeset",
                ));
            }
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                ConfigCommand::Show => {
                    config::response(scope_name(store_path.scope), &store_path.path)
                }
                ConfigCommand::Set { graph } => {
                    let setting = config::parse_setting(&graph)?;
                    config::update(&store_path.path, setting)?;
                    Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let mut response =
                        config::response(scope_name(store_path.scope), &store_path.path)?;
                    if setting != config::GraphSetting::Disabled
                        && setting != config::GraphSetting::Inherit
                    {
                        response["work"] = work::start_graph_projection(
                            scope_name(store_path.scope),
                            &store_path.path,
                        )?["work"]
                            .clone();
                    }
                    Ok(response)
                }
                ConfigCommand::Unset { graph } => {
                    if !graph {
                        return Err(AppError::new(
                            "invalid_input",
                            "config unset requires --graph",
                        ));
                    }
                    config::update(&store_path.path, config::GraphSetting::Inherit)?;
                    Store::open(scope_name(store_path.scope), &store_path.path)?;
                    config::response(scope_name(store_path.scope), &store_path.path)
                }
            }
        }
        Command::Graph { command } => {
            ensure_scope_supported(cli.scope, false, "graph")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                GraphCommand::Related { slug, limit } => {
                    validate_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.graph_related(&slug, limit)?)
                }
                GraphCommand::Explore {
                    identifier,
                    depth,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_depth(depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    match identifier {
                        Some(identifier) => Ok(store.graph_explore(
                            &identifier,
                            depth,
                            limit,
                            direction.as_str(),
                            &edge_types,
                        )?),
                        None => Ok(store.graph_explore_macro(depth, limit, &edge_types)?),
                    }
                }
                GraphCommand::Node { identifier } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_node(&identifier)?)
                }
                GraphCommand::Neighbors {
                    identifier,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_neighbors(
                        &identifier,
                        limit,
                        direction.as_str(),
                        &edge_types,
                    )?)
                }
                GraphCommand::Path {
                    from,
                    to,
                    max_depth,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_depth(max_depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_path(
                        &from,
                        &to,
                        max_depth,
                        limit,
                        direction.as_str(),
                        &edge_types,
                    )?)
                }
                GraphCommand::Impact {
                    identifier,
                    max_depth,
                    limit,
                } => {
                    validate_graph_depth(max_depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_impact(&identifier, max_depth, limit)?)
                }
                GraphCommand::Overview { limit } => {
                    validate_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_overview(limit)?)
                }
                GraphCommand::Status => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_status()?)
                }
                GraphCommand::Verify => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_verify()?)
                }
                GraphCommand::Relation { command } => match command {
                    GraphRelationCommand::Set {
                        from,
                        relation_type,
                        to,
                        provenance,
                        reason,
                        confidence,
                        source_ids,
                    } => {
                        let mut store =
                            Store::open(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_set(
                            &from,
                            &relation_type,
                            &to,
                            &provenance,
                            &reason,
                            confidence,
                            &source_ids,
                        )?)
                    }
                    GraphRelationCommand::List {
                        from,
                        to,
                        relation_type,
                        limit,
                    } => {
                        validate_limit(limit)?;
                        let store =
                            Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_list(
                            from.as_deref(),
                            to.as_deref(),
                            relation_type.as_deref(),
                            limit,
                        )?)
                    }
                    GraphRelationCommand::Retract {
                        from,
                        relation_type,
                        to,
                        reason,
                    } => {
                        let mut store =
                            Store::open(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_retract(&from, &relation_type, &to, &reason)?)
                    }
                },
            }
        }
        Command::Weight { command } => {
            ensure_scope_supported(cli.scope, false, "weight")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                WeightCommand::List { target, identifier } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.retrieval_weight_list(target.as_str(), &identifier)?)
                }
                WeightCommand::Set {
                    target,
                    identifier,
                    value,
                    reason,
                    provenance,
                } => {
                    let value = value.parse::<i32>().map_err(|_| {
                        AppError::new("invalid_weight", "weight must be one of -2, -1, 1, or 2")
                    })?;
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_weight_set(
                        target.as_str(),
                        &identifier,
                        value,
                        &reason,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::Clear {
                    target,
                    identifier,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_weight_clear(
                        target.as_str(),
                        &identifier,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::Feedback {
                    target,
                    identifier,
                    query,
                    signal,
                    reason,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_feedback_set(
                        target.as_str(),
                        &identifier,
                        &query,
                        signal.value(),
                        &reason,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::FeedbackClear {
                    target,
                    identifier,
                    query,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_feedback_clear(
                        target.as_str(),
                        &identifier,
                        &query,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Maintenance { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "maintenance")?;
            ensure_scope_supported(cli.scope, false, "maintenance")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                MaintenanceCommand::Materialize => work::start_materialize(&store_path),
                MaintenanceCommand::Reindex => work::start_reindex(&store_path),
                MaintenanceCommand::Compact => work::start_compact(&store_path),
            }
        }
        Command::Checkpoint { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "checkpoint")?;
            ensure_scope_supported(cli.scope, false, "checkpoint")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
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
            granularity,
            group_by,
            kinds,
            limit,
            record,
            explain,
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
            let granularity = match granularity {
                SearchGranularityArg::Document => SearchGranularity::Document,
                SearchGranularityArg::Passage => SearchGranularity::Passage,
                SearchGranularityArg::Sentence => SearchGranularity::Sentence,
                SearchGranularityArg::All => SearchGranularity::All,
            };
            let grouping = match group_by {
                SearchGroupArg::Auto if granularity == SearchGranularity::All => {
                    SearchGrouping::Document
                }
                SearchGroupArg::Auto | SearchGroupArg::None => SearchGrouping::None,
                SearchGroupArg::Document if granularity == SearchGranularity::All => {
                    SearchGrouping::Document
                }
                SearchGroupArg::Document => {
                    return Err(AppError::new(
                        "invalid_input",
                        "--group-by document requires --granularity all",
                    ));
                }
            };
            let options = SearchOptions {
                mode,
                granularity,
                grouping,
                kinds,
                explain,
            };
            let paths =
                resolve_effective_read_store_paths(cli.scope, &cwd, selected_changeset.as_deref())?;
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
                    materialize_wiki_if_live(store, selected_changeset.as_deref())?;
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
        Command::Span { command } => {
            ensure_scope_supported(cli.scope, false, "span")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            match command {
                SpanCommand::Get { identifier } => {
                    require_text("identifier", &identifier)?;
                    to_json(store.span_get(&identifier)?)
                }
                SpanCommand::Expand {
                    identifier,
                    before,
                    after,
                    children,
                } => {
                    require_text("identifier", &identifier)?;
                    if before > 20 || after > 20 {
                        return Err(AppError::new(
                            "invalid_limit",
                            "before and after must not exceed 20",
                        ));
                    }
                    if !(1..=200).contains(&children) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "children must be between 1 and 200",
                        ));
                    }
                    to_json(store.span_expand(&identifier, before, after, children)?)
                }
            }
        }
        Command::Context { limit } => {
            validate_limit(limit)?;
            let paths =
                resolve_effective_read_store_paths(cli.scope, &cwd, selected_changeset.as_deref())?;
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
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            if record {
                let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                let response = store.lint(limit, offset)?;
                store.record_lint(response.total)?;
                materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                to_json(response)
            } else {
                let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                to_json(store.lint(limit, offset)?)
            }
        }
        Command::Log { limit } => {
            ensure_scope_supported(cli.scope, false, "log")?;
            validate_limit(limit)?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            to_json(store.log(limit)?)
        }
    }
}

fn resolve_effective_store_path(
    scope: Scope,
    cwd: &Path,
    selected_changeset: Option<&str>,
) -> Result<StorePath> {
    let live = resolve_live_store_path(scope, cwd)?;
    changeset::resolve_effective(live, selected_changeset)
}

fn resolve_effective_read_store_paths(
    scope: Scope,
    cwd: &Path,
    selected_changeset: Option<&str>,
) -> Result<Vec<StorePath>> {
    if selected_changeset.is_none() {
        return resolve_live_read_store_paths(scope, cwd, true);
    }
    ensure_scope_supported(scope, false, "--changeset")?;
    Ok(vec![resolve_effective_store_path(
        scope,
        cwd,
        selected_changeset,
    )?])
}

fn materialize_if_live(store: &mut Store, selected_changeset: Option<&str>) -> Result<()> {
    if selected_changeset.is_none() {
        store.materialize_incremental(true)?;
    }
    Ok(())
}

fn materialize_wiki_if_live(store: &mut Store, selected_changeset: Option<&str>) -> Result<()> {
    if selected_changeset.is_none() {
        store.materialize_incremental(false)?;
    }
    Ok(())
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
        tracked_path: Some(tracked_source_path(store_path, &resolved)?),
        content,
    })
}

fn tracked_source_path(store_path: &StorePath, resolved: &Path) -> Result<String> {
    let tracked = if store_path.scope == Scope::Project {
        let project_root = project_root(store_path)?;
        resolved.strip_prefix(&project_root).unwrap_or(resolved)
    } else {
        resolved
    };
    tracked
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "invalid_source_path",
                format!("source path is not valid UTF-8: {}", resolved.display()),
            )
        })
}

fn ensure_source_status_unchanged(
    before: &store::SourceStatusTargets,
    after: &store::SourceStatusTargets,
) -> Result<()> {
    if before != after {
        return Err(AppError::new(
            "source_status_unstable",
            "source path revisions changed during the status check; retry",
        ));
    }
    Ok(())
}

fn run_source_diff(
    store_path: &StorePath,
    source_id: i64,
    tracked_path: Option<&str>,
    to_source: Option<i64>,
    max_chars: usize,
    allow_external_source: bool,
    acknowledge_sensitive_source: bool,
) -> Result<Value> {
    if !(1..=MAX_DIFF_OUTPUT_CHARS).contains(&max_chars) {
        return Err(AppError::new(
            "invalid_limit",
            format!("max-chars must be between 1 and {MAX_DIFF_OUTPUT_CHARS}"),
        ));
    }
    let mut store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
    let from = store.source_for_diff(source_id, MAX_DIFF_INPUT_BYTES)?;

    if let Some(to_source_id) = to_source {
        let to = store.source_for_diff(to_source_id, MAX_DIFF_INPUT_BYTES)?;
        let changed = from.content_hash != to.content_hash;
        let rendered = render_diff(
            &from.content,
            &to.content,
            &format!("source:{}", from.id),
            &format!("source:{}", to.id),
            max_chars,
        )?;
        return Ok(json!({
            "scope": scope_name(store_path.scope),
            "database": store_path.path.display().to_string(),
            "from": {
                "kind": "source",
                "source_id": from.id,
                "content_hash": from.content_hash,
                "bytes": from.content.len(),
            },
            "to": {
                "kind": "source",
                "source_id": to.id,
                "content_hash": to.content_hash,
                "bytes": to.content.len(),
            },
            "changed": changed,
            "diff": rendered_diff_json(rendered),
        }));
    }

    let selected = store.source_status_targets(vec![source_id], false)?;
    let target = select_source_diff_target(&selected, tracked_path)?;
    let resolved =
        resolve_tracked_source_path(store_path, &target.tracked_path, allow_external_source)?;
    let prepared = prepare_live_source(
        store_path,
        resolved.clone(),
        allow_external_source,
        MAX_DIFF_INPUT_BYTES as u64,
    )?;
    let (live, content) = read_prepared_source(prepared, MAX_DIFF_INPUT_BYTES as u64, true);
    let current = store.source_status_targets(vec![source_id], false)?;
    ensure_source_status_unchanged(&selected, &current)?;
    if live.state == "unstable" {
        return Err(AppError::new(
            "source_status_unstable",
            live.message
                .unwrap_or_else(|| "tracked source changed during diff".to_string()),
        ));
    }
    if live.state == "oversized" {
        return Err(AppError::new(
            "source_diff_too_large",
            live.message
                .unwrap_or_else(|| "live source exceeds the diff input limit".to_string()),
        ));
    }
    if live.state != "hashed" {
        return Err(AppError::new(
            "source_diff_unavailable",
            format!(
                "tracked source {} is {}{}",
                target.tracked_path,
                live.state,
                live.message
                    .as_deref()
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default()
            ),
        ));
    }
    let content = String::from_utf8(content.ok_or_else(|| {
        AppError::new(
            "source_diff_unavailable",
            "live source content was not captured",
        )
    })?)
    .map_err(|_| {
        AppError::new(
            "invalid_utf8",
            format!("{} is not UTF-8", resolved.display()),
        )
    })?;
    let live_hash = live
        .content_hash
        .expect("a successfully hashed live source has a hash");
    let live_bytes = live
        .bytes
        .expect("a successfully hashed live source has a byte count");
    let changed = from.content_hash != live_hash;
    if changed {
        validate_sensitive_source(&resolved, &content, acknowledge_sensitive_source)?;
    }
    let rendered = render_diff(
        &from.content,
        &content,
        &format!("source:{}", from.id),
        &format!("live:{}", target.tracked_path),
        max_chars,
    )?;
    Ok(json!({
        "scope": scope_name(store_path.scope),
        "database": store_path.path.display().to_string(),
        "from": {
            "kind": "source",
            "source_id": from.id,
            "content_hash": from.content_hash,
            "bytes": from.content.len(),
        },
        "to": {
            "kind": "live",
            "tracked_path": target.tracked_path,
            "head_source_id": target.head_source_id,
            "head_revision": target.head_revision,
            "content_hash": live_hash,
            "bytes": live_bytes,
        },
        "changed": changed,
        "diff": rendered_diff_json(rendered),
    }))
}

fn select_source_diff_target(
    selected: &store::SourceStatusTargets,
    tracked_path: Option<&str>,
) -> Result<store::SourceStatusTarget> {
    if selected.targets.is_empty() {
        return Err(AppError::new(
            "source_diff_untracked",
            format!(
                "source {} has no tracked path; use --to-source to compare immutable snapshots",
                selected
                    .untracked_source_ids
                    .first()
                    .copied()
                    .unwrap_or_default()
            ),
        ));
    }
    if let Some(tracked_path) = tracked_path {
        return selected
            .targets
            .iter()
            .find(|target| target.tracked_path == tracked_path)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    "source_diff_path_not_found",
                    format!("source was never observed at tracked path {tracked_path}"),
                )
            });
    }
    if selected.targets.len() > 1 {
        return Err(AppError::new(
            "source_diff_path_required",
            format!(
                "source has multiple tracked paths; retry with --path and one exact candidate: {}",
                selected
                    .targets
                    .iter()
                    .map(|target| target.tracked_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    Ok(selected.targets[0].clone())
}

fn rendered_diff_json(rendered: source_diff::RenderedDiff) -> Value {
    json!({
        "format": "unified",
        "context_lines": 3,
        "text": rendered.text,
        "returned_chars": rendered.returned_chars,
        "total_chars": rendered.total_chars,
        "truncated": rendered.truncated,
    })
}

fn resolve_tracked_source_path(
    store_path: &StorePath,
    tracked_path: &str,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let tracked = Path::new(tracked_path);
    if tracked.is_absolute() {
        if store_path.scope == Scope::Project {
            let root = project_root(store_path)?;
            if ensure_project_path(tracked, &root).is_err() && !allow_external_source {
                return Err(AppError::new(
                    "external_source_requires_acknowledgement",
                    format!(
                        "tracked source {} is outside project root {}; retry with --allow-external-source only with current authorization",
                        tracked.display(),
                        root.display()
                    ),
                ));
            }
        }
        return Ok(tracked.to_path_buf());
    }
    if store_path.scope != Scope::Project
        || tracked.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::new(
            "corrupt_store",
            format!("invalid tracked source path: {tracked_path}"),
        ));
    }
    let root = project_root(store_path)?;
    let path = root.join(tracked);
    if let Err(error) = ensure_project_path(&path, &root)
        && !allow_external_source
    {
        return Err(AppError::new(
            "external_source_requires_acknowledgement",
            format!(
                "tracked source {} escapes project root {}; retry with --allow-external-source only with current authorization: {error}",
                path.display(),
                root.display()
            ),
        ));
    }
    Ok(path)
}

fn prepare_live_source(
    store_path: &StorePath,
    path: PathBuf,
    allow_external_source: bool,
    max_bytes: u64,
) -> Result<PreparedLiveSource> {
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(PreparedLiveSource::Terminal {
                observed: None,
                path,
                status: LiveSourceStatus {
                    state: if error.kind() == io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "unreadable"
                    },
                    content_hash: None,
                    bytes: None,
                    message: Some(error.to_string()),
                },
            });
        }
    };
    let before = file_fingerprint(&metadata);
    if !metadata.is_file() {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: Some(before),
            status: LiveSourceStatus {
                state: "unreadable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source is not a regular file".to_string()),
            },
        });
    }
    if before.len > max_bytes {
        let bytes = before.len;
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: Some(before),
            status: LiveSourceStatus {
                state: "oversized",
                content_hash: None,
                bytes: Some(bytes),
                message: Some(format!(
                    "tracked source is {bytes} bytes; maximum supported input is {max_bytes} bytes"
                )),
            },
        });
    }
    let file = match open_live_source(&path) {
        Ok(file) => file,
        Err(error) => {
            return Ok(PreparedLiveSource::Terminal {
                observed: Some(before),
                path,
                status: LiveSourceStatus {
                    state: if error.kind() == io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "unreadable"
                    },
                    content_hash: None,
                    bytes: None,
                    message: Some(error.to_string()),
                },
            });
        }
    };
    if file
        .metadata()
        .ok()
        .map(|metadata| file_fingerprint(&metadata))
        .as_ref()
        != Some(&before)
    {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: None,
            status: LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source changed while it was being opened".to_string()),
            },
        });
    }
    if store_path.scope == Scope::Project && !allow_external_source {
        let root = project_root(store_path)?;
        let resolved = fs::canonicalize(&path).map_err(|error| {
            AppError::new(
                "source_status_unstable",
                format!(
                    "tracked source {} changed during authorization: {error}",
                    path.display()
                ),
            )
        })?;
        if !resolved.starts_with(&root) {
            return Err(AppError::new(
                "external_source_requires_acknowledgement",
                format!(
                    "tracked source {} resolves outside project root {}; retry with --allow-external-source only with current authorization",
                    path.display(),
                    root.display()
                ),
            ));
        }
    }
    if fs::metadata(&path)
        .ok()
        .map(|metadata| file_fingerprint(&metadata))
        .as_ref()
        != Some(&before)
    {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: None,
            status: LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source changed during authorization".to_string()),
            },
        });
    }
    Ok(PreparedLiveSource::Ready { path, file, before })
}

fn open_live_source(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    options.custom_flags(LIVE_SOURCE_NONBLOCK);
    options.open(path)
}

fn inspect_prepared_source(prepared: PreparedLiveSource) -> LiveSourceStatus {
    read_prepared_source(prepared, MAX_INPUT_BYTES, false).0
}

fn read_prepared_source(
    prepared: PreparedLiveSource,
    max_bytes: u64,
    capture: bool,
) -> (LiveSourceStatus, Option<Vec<u8>>) {
    let (path, mut file, before) = match prepared {
        PreparedLiveSource::Ready { path, file, before } => (path, file, before),
        PreparedLiveSource::Terminal {
            path,
            observed,
            status,
        } => {
            let current = fs::metadata(&path)
                .ok()
                .map(|metadata| file_fingerprint(&metadata));
            if current != observed {
                return (
                    LiveSourceStatus {
                        state: "unstable",
                        content_hash: None,
                        bytes: current.as_ref().map(|fingerprint| fingerprint.len),
                        message: Some("tracked source changed during status preflight".to_string()),
                    },
                    None,
                );
            }
            return (status, None);
        }
    };

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    let mut content = capture.then(|| Vec::with_capacity(before.len as usize));
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return (
                    LiveSourceStatus {
                        state: "unreadable",
                        content_hash: None,
                        bytes: Some(bytes),
                        message: Some(error.to_string()),
                    },
                    None,
                );
            }
        };
        bytes += read as u64;
        if bytes > max_bytes {
            return (
                LiveSourceStatus {
                    state: "oversized",
                    content_hash: None,
                    bytes: Some(bytes),
                    message: Some(format!(
                        "tracked source exceeded the {max_bytes}-byte input limit while reading"
                    )),
                },
                None,
            );
        }
        if let Some(content) = content.as_mut() {
            content.extend_from_slice(&buffer[..read]);
        }
        hasher.update(&buffer[..read]);
    }

    let after_handle = file
        .metadata()
        .ok()
        .map(|metadata| file_fingerprint(&metadata));
    let after_path = fs::metadata(path)
        .ok()
        .map(|metadata| file_fingerprint(&metadata));
    let content_hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if after_handle.as_ref() != Some(&before) || after_path.as_ref() != Some(&before) {
        return (
            LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: Some(bytes),
                message: Some("tracked source changed while it was being hashed".to_string()),
            },
            None,
        );
    }
    (
        LiveSourceStatus {
            state: "hashed",
            content_hash: Some(content_hash),
            bytes: Some(bytes),
            message: None,
        },
        content,
    )
}

fn file_fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    FileFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn project_root(store_path: &StorePath) -> Result<PathBuf> {
    let root = store_path
        .authority_path()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            AppError::new(
                "invalid_store_path",
                "project Wiki path has no project root",
            )
        })?;
    Ok(fs::canonicalize(root)?)
}

fn validate_source_scope(
    store_path: &StorePath,
    path: &Path,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    if store_path.scope == Scope::Project && !allow_external_source {
        let project_root = project_root(store_path)?;
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

fn validate_graph_depth(depth: usize) -> Result<()> {
    if !(1..=10).contains(&depth) {
        return Err(AppError::new(
            "invalid_graph_depth",
            "graph depth must be between 1 and 10",
        ));
    }
    Ok(())
}

fn validate_graph_limit(limit: usize) -> Result<()> {
    if !(1..=5000).contains(&limit) {
        return Err(AppError::new(
            "invalid_graph_limit",
            "graph node limit must be between 1 and 5000",
        ));
    }
    Ok(())
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
    #[cfg(unix)]
    use super::{
        MAX_INPUT_BYTES, Scope, StorePath, file_fingerprint, inspect_prepared_source,
        open_live_source, prepare_live_source,
    };
    use super::{ensure_source_status_unchanged, read_file_bounded};
    use crate::store::{SourceStatusTarget, SourceStatusTargets};

    #[test]
    fn bounded_file_read_rejects_oversized_input() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oversized.txt");
        std::fs::write(&path, b"12345").unwrap();

        let error = read_file_bounded(&path, 4).unwrap_err();

        assert_eq!(error.code, "input_too_large");
    }

    #[cfg(unix)]
    #[test]
    fn file_fingerprint_detects_same_size_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("tracked.txt");
        let replacement = temp.path().join("replacement.txt");
        std::fs::write(&tracked, b"alpha").unwrap();
        std::fs::write(&replacement, b"bravo").unwrap();
        let before = file_fingerprint(&std::fs::metadata(&tracked).unwrap());

        std::fs::rename(&replacement, &tracked).unwrap();
        let after = file_fingerprint(&std::fs::metadata(&tracked).unwrap());

        assert!(before != after);
    }

    #[cfg(unix)]
    #[test]
    fn prepared_source_rejects_path_replacement_before_hashing() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("tracked.txt");
        let external_dir = tempfile::tempdir().unwrap();
        let external = external_dir.path().join("external.txt");
        std::fs::write(&tracked, b"inside").unwrap();
        std::fs::write(&external, b"outside").unwrap();
        let store_path = StorePath::new(Scope::Project, temp.path().join(".lwc/wiki.db"));
        let prepared =
            prepare_live_source(&store_path, tracked.clone(), false, MAX_INPUT_BYTES).unwrap();

        std::fs::remove_file(&tracked).unwrap();
        symlink(&external, &tracked).unwrap();
        let status = inspect_prepared_source(prepared);

        assert_eq!(status.state, "unstable");
        assert!(status.content_hash.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn live_source_open_does_not_block_on_a_fifo() {
        use std::{sync::mpsc, thread, time::Duration};

        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("source.fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || sender.send(open_live_source(&fifo)).unwrap());

        let file = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("opening a FIFO must not block")
            .unwrap();
        assert!(!file.metadata().unwrap().is_file());
    }

    #[test]
    fn source_status_rejects_a_head_change_during_live_hashing() {
        let targets =
            |head_source_id, head_revision, head_content_hash: &str| SourceStatusTargets {
                targets: vec![SourceStatusTarget {
                    requested_source_id: 1,
                    tracked_path: "docs/source.md".to_string(),
                    head_source_id,
                    head_revision,
                    head_content_hash: head_content_hash.to_string(),
                }],
                untracked_source_ids: Vec::new(),
            };
        let before = targets(1, 1, "hash-a");
        let after = targets(2, 2, "hash-b");

        let error = ensure_source_status_unchanged(&before, &after).unwrap_err();

        assert_eq!(error.code, "source_status_unstable");
    }
}
