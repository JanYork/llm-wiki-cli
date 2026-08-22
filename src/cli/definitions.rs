use clap::{Parser, Subcommand, ValueEnum};
use crate::{changeset, codegraph, config, source_diff, store, trans, view, work};
use crate::error::{AppError, Result};
use crate::import::collect_documents;
use crate::scope::{
    Scope, StorePath, ensure_project_path, ensure_scope_supported, init_store_path,
    resolve_read_store_paths as resolve_live_read_store_paths,
    resolve_store_path as resolve_live_store_path,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use crate::source_diff::{
    DEFAULT_DIFF_OUTPUT_CHARS, MAX_DIFF_INPUT_BYTES, MAX_DIFF_OUTPUT_CHARS, render_diff,
};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::Command as ProcessCommand,
    time::{SystemTime, UNIX_EPOCH},
};
use crate::store::{
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
    /// Serve LWC to Agent hosts over a foreground protocol transport.
    Serve {
        /// Use the standard MCP JSON-RPC protocol over stdin/stdout.
        #[arg(long)]
        mcp: bool,
        /// Bind the MCP server to an explicit Agent workspace.
        #[arg(long, value_name = "DIRECTORY", requires = "mcp")]
        path: Option<PathBuf>,
    },
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
    /// Inspect and use the project-local CodeGraph index.
    Cg {
        #[command(subcommand)]
        command: CgCommand,
    },
    /// Run a read-only OfficeCLI command through LWC's pinned global runtime.
    Office {
        #[command(subcommand)]
        command: OfficeCommand,
    },
    /// Start a foreground, loopback-only, read-only project viewer.
    #[command(
        long_about = "Start the read-only LWC viewer on 127.0.0.1. The viewer never refreshes, migrates, or mutates project state."
    )]
    View {
        /// Loopback port. Use 0 to select an available port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Print the URL without opening a browser.
        #[arg(long)]
        no_open: bool,
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
    /// Assign explicit strong-load tags to core Wiki pages.
    Tag {
        #[command(subcommand)]
        command: TagCommand,
    },
    /// Manage an independent durable backlog of deferred work.
    Todo { #[command(subcommand)] command: TodoCommand },
    /// Manage an independent durable current execution plan.
    Plan { #[command(subcommand)] command: PlanCommand },
    /// Load deterministic Wiki context without search.
    Load {
        #[command(subcommand)]
        command: LoadCommand,
    },
    /// Internal lifecycle integration used by installed Agent hooks.
    #[command(hide = true)]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
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
    /// Record one normalized temporal-memory event capsule.
    #[command(
        after_help = "Examples:\n  lwc remember --json '{\"type\":\"decision\",\"context\":\"deploy\",\"decision\":[\"use blue\"]}'\n  lwc remember --json - < event.json\n  lwc remember --json @event.json"
    )]
    Remember {
        /// Inline JSON, '-' for stdin, or '@PATH' for a scoped UTF-8 file.
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    /// Recall, inspect, and rate temporal-memory events.
    #[command(
        after_help = "Examples:\n  lwc memory recall \"payment retry\" --limit 5\n  lwc memory show EVENT_ID\n  lwc memory feedback EVENT_ID --signal useful --reason \"prevented a repeated failure\""
    )]
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect and update layered graph, trans, memory, and global Office settings.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Convert one authorized local file with the configured trans engine.
    #[command(
        long_about = "Run the resolved anydoc or markitdown engine against one authorized local file and write the UTF-8 Markdown result to an explicit new output path.\n\nThis command does not add sources, pages, citations, or operation-log entries.",
        after_help = "Examples:\n  lwc trans docs/report.docx --output out/report.md\n  lwc trans ../shared/file.pdf --output out/file.md --allow-external-source"
    )]
    Trans {
        /// Existing local file to convert.
        file: PathBuf,
        /// Explicit destination path; must not already exist.
        #[arg(long)]
        output: PathBuf,
        /// Permit a project input that resolves outside the active project root.
        #[arg(long)]
        allow_external_source: bool,
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
#[command(disable_help_subcommand = true)]
enum CgCommand {
    /// Download the pinned LWC CodeGraph runtime and build the project index.
    Init {
        /// Show detailed CodeGraph indexing progress.
        #[arg(long)]
        verbose: bool,
    },
    /// Report local runtime and index availability without downloading or writing.
    Status,
    /// Forward a project-scoped CodeGraph command through the pinned runtime.
    #[command(external_subcommand)]
    Run(Vec<OsString>),
}

#[derive(Subcommand)]
#[command(disable_help_subcommand = true)]
enum OfficeCommand {
    /// Forward a read-only command through the pinned OfficeCLI runtime.
    #[command(external_subcommand)]
    Run(Vec<OsString>),
}

#[derive(Subcommand)]
enum MemoryCommand {
    /// Search a bounded set of temporal memories without changing them.
    Recall {
        query: String,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        include_superseded: bool,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Return one complete event capsule by identifier.
    Show { event_id: String },
    /// Append an explicit usefulness judgment without rewriting the event.
    Feedback {
        event_id: String,
        #[arg(long, value_enum)]
        signal: MemoryFeedbackSignalArg,
        #[arg(long)]
        reason: String,
    },
    /// Report bounded temporal-memory storage and outcome metrics.
    Status,
    /// Enforce configured age and capacity retention now.
    Maintain,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MemoryFeedbackSignalArg {
    Useful,
    NotUseful,
}

impl MemoryFeedbackSignalArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::NotUseful => "not-useful",
        }
    }
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
enum TagCommand {
    Set {
        tag: String,
        page: String,
        #[arg(long, default_value_t = 0)]
        priority: i32,
        #[arg(long)]
        reason: String,
    },
    Remove { tag: String, page: String },
    Delete { tag: String },
    Autoload {
        tag: String,
        #[arg(long, required_unless_present = "disable", conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long, default_value_t = 0)]
        priority: i32,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value_t = 50_000)]
        max_chars: usize,
        #[arg(long)]
        reason: String,
    },
    List,
}

#[derive(Subcommand)]
enum TodoCommand {
    Add { title:Option<String>, #[arg(long="tag")] tags:Vec<String>, #[arg(long)] cue:Option<String>, #[arg(long)] detail:Option<String>, #[arg(long)] parent:Option<String>, #[arg(long)] target_at:Option<String>, #[arg(long)] request_id:Option<String>, #[arg(long,value_name="JSON|-|@PATH")] json:Option<String> },
    List { #[arg(long,value_parser=["open","done","cancelled"])] state:Option<String>, #[arg(long)] tag:Option<String>, #[arg(long)] parent:Option<String>, #[arg(long,default_value_t=100)] limit:usize, #[arg(long,default_value_t=0)] offset:usize },
    Search { query:String, #[arg(long,value_parser=["open","done","cancelled"])] state:Option<String>, #[arg(long)] tag:Option<String>, #[arg(long)] parent:Option<String>, #[arg(long,default_value_t=100)] limit:usize, #[arg(long,default_value_t=0)] offset:usize },
    Show { todo_id:String },
    Update { todo_id:String, #[arg(long)] if_revision:i64, #[arg(long)] title:Option<String>, #[arg(long,conflicts_with="clear_cue")] cue:Option<String>, #[arg(long)] clear_cue:bool, #[arg(long,conflicts_with="clear_detail")] detail:Option<String>, #[arg(long)] clear_detail:bool, #[arg(long,conflicts_with="clear_target_at")] target_at:Option<String>, #[arg(long)] clear_target_at:bool, #[arg(long="add-tag")] add_tags:Vec<String>, #[arg(long="remove-tag")] remove_tags:Vec<String> },
    Done { todo_id:String, #[arg(long)] if_revision:i64, #[arg(long)] result:String },
    Cancel { todo_id:String, #[arg(long)] if_revision:i64, #[arg(long)] reason:String },
    Reopen { todo_id:String, #[arg(long)] if_revision:i64 },
}

#[derive(Subcommand)]
enum PlanCommand {
    Create { title:Option<String>, #[arg(long)] objective:Option<String>, #[arg(long)] done_when:Option<String>, #[arg(long="tag")] tags:Vec<String>, #[arg(long="constraint")] constraints:Vec<String>, #[arg(long="step")] steps:Vec<String>, #[arg(long)] request_id:Option<String>, #[arg(long,value_name="JSON|-|@PATH")] json:Option<String> },
    Current { #[arg(long)] tag:Option<String>, #[arg(long,default_value_t=100)] limit:usize, #[arg(long,default_value_t=0)] offset:usize },
    List { #[arg(long,value_parser=["active","completed","abandoned"])] state:Option<String>, #[arg(long)] tag:Option<String>, #[arg(long,default_value_t=100)] limit:usize, #[arg(long,default_value_t=0)] offset:usize },
    Search { query:String, #[arg(long,value_parser=["active","completed","abandoned"])] state:Option<String>, #[arg(long)] tag:Option<String>, #[arg(long,default_value_t=100)] limit:usize, #[arg(long,default_value_t=0)] offset:usize },
    Show { plan_id:String }, Brief { plan_id:String },
    Advance { plan_id:String, #[arg(long)] if_revision:i64, #[arg(long)] done:String, #[arg(long)] result:String, #[arg(long)] next:Option<String> },
    Block { plan_id:String, #[arg(long)] if_revision:i64, #[arg(long)] step:String, #[arg(long)] reason:String },
    Revise { plan_id:String, #[arg(long)] if_revision:i64, #[arg(long)] reason:String, #[arg(long,value_name="JSON|-|@PATH")] json:String },
    Complete { plan_id:String, #[arg(long)] if_revision:i64, #[arg(long)] result:String, #[arg(long)] evidence:String, #[arg(long)] done_when_checked:bool },
    Abandon { plan_id:String, #[arg(long)] if_revision:i64, #[arg(long)] reason:String },
}

#[derive(Subcommand)]
enum LoadCommand {
    Tag {
        tag: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum AgentCommand {
    /// Install LWC integration into detected or selected Agents.
    Install {
        #[arg(long)]
        target: Option<String>,
        #[arg(long, value_enum)]
        location: Option<AgentLocationArg>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        print_config: Option<String>,
        #[arg(long = "no-prompt-hook", alias = "no-codegraph-prompt-hook")]
        no_prompt_hook: bool,
    },
    /// Inspect installed Agent integrations.
    Status {
        #[arg(long)]
        target: Option<String>,
        #[arg(long, value_enum)]
        location: Option<AgentLocationArg>,
    },
    /// Refresh selected Agent integrations without duplicating owned entries.
    Refresh {
        #[arg(long)]
        target: Option<String>,
        #[arg(long, value_enum)]
        location: Option<AgentLocationArg>,
    },
    /// Remove only LWC-owned Agent integration state.
    Uninstall {
        #[arg(long)]
        target: Option<String>,
        #[arg(long, value_enum)]
        location: Option<AgentLocationArg>,
        #[arg(long)]
        yes: bool,
    },
    /// Compile bounded read-only context from a native Agent hook event.
    Hook {
        #[arg(long, value_enum)]
        agent: AgentArg,
        #[arg(long)]
        event: String,
        /// Print raw context for hosts whose command hooks consume stdout directly.
        #[arg(long)]
        raw: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentLocationArg {
    Global,
    Local,
}

impl From<AgentLocationArg> for crate::agent::AgentLocation {
    fn from(value: AgentLocationArg) -> Self {
        match value {
            AgentLocationArg::Global => Self::Global,
            AgentLocationArg::Local => Self::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentArg {
    Codex,
    Claude,
    Cursor,
    Gemini,
    Hermes,
    Antigravity,
    CopilotCli,
    #[value(alias = "copilot")]
    CopilotVscode,
    Kiro,
    Pi,
    Generic,
}

impl From<AgentArg> for crate::agent::AgentKind {
    fn from(value: AgentArg) -> Self {
        match value {
            AgentArg::Codex => Self::Codex,
            AgentArg::Claude => Self::Claude,
            AgentArg::Cursor => Self::Cursor,
            AgentArg::Gemini => Self::Gemini,
            AgentArg::Hermes => Self::Hermes,
            AgentArg::Antigravity => Self::Antigravity,
            AgentArg::CopilotCli => Self::CopilotCli,
            AgentArg::CopilotVscode => Self::CopilotVscode,
            AgentArg::Kiro => Self::Kiro,
            AgentArg::Pi => Self::Pi,
            AgentArg::Generic => Self::Generic,
        }
    }
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
    /// Show effective graph, trans, memory, and Office configuration plus value origins.
    Show,
    /// Atomically set graph, trans, memory, and Office configuration.
    Set {
        /// Select disabled, grafeo, surrealdb, or inherit.
        #[arg(long)]
        graph: Option<String>,
        /// Select disabled, anydoc, or markitdown.
        #[arg(long)]
        trans: Option<String>,
        /// Select disabled or officecli. Office configuration is global-only.
        #[arg(long)]
        office: Option<String>,
        /// Select disabled, enabled, or inherit.
        #[arg(long)]
        memory: Option<String>,
        /// Select disabled, enabled, or inherit for durable Todo.
        #[arg(long)]
        todo: Option<String>,
        /// Select disabled, enabled, or inherit for durable Plan.
        #[arg(long)]
        plan: Option<String>,
        /// Maximum retained memory age in days.
        #[arg(long = "memory-max-age-days")]
        memory_max_age_days: Option<u32>,
        /// Maximum logical memory payload bytes.
        #[arg(long = "memory-max-bytes")]
        memory_max_bytes: Option<u64>,
        /// Override the trans timeout in seconds (1..=900).
        #[arg(long = "trans-timeout")]
        trans_timeout: Option<u16>,
        /// Replace the selected trans engine's argument list; repeat for multiple values.
        #[arg(long = "trans-arg", allow_hyphen_values = true)]
        trans_args: Vec<String>,
    },
    /// Restore graph, trans, or memory settings to their inherited defaults.
    Unset {
        #[arg(long)]
        graph: bool,
        #[arg(long)]
        trans: bool,
        #[arg(long)]
        memory: bool,
        #[arg(long)]
        todo: bool,
        #[arg(long)]
        plan: bool,
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
