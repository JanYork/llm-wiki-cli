use clap::{Parser, Subcommand};
use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params, types::ValueRef,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const STORE_SCHEMA_VERSION: i64 = 2;
pub(crate) static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[allow(dead_code)] // Each fixed binary constructs exactly one of the three variants.
pub(crate) enum Plugin {
    Tutor,
    Book,
    Practice,
}

impl Plugin {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Tutor => "tutor",
            Self::Book => "book",
            Self::Practice => "practice",
        }
    }
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Subject {
        #[command(subcommand)]
        command: SubjectCommand,
    },
    Status,
}

#[derive(Subcommand)]
pub(crate) enum SubjectCommand {
    Create {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Ensure {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Show {
        id: String,
    },
    Rename {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
}

#[derive(Debug)]
pub(crate) struct Error {
    code: &'static str,
    message: String,
    details: Option<Value>,
}

impl Error {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub(crate) fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    #[allow(dead_code)] // Book uses this branch discriminator; the other fixed binaries do not.
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::new("io_error", error.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("database_error", error.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubjectCreate {
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubjectEnsure {
    id: String,
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubjectRename {
    name: String,
    request_id: String,
}

#[allow(dead_code)] // Tutor has its own parser; Book and Practice use this shared entrypoint.
pub(crate) fn main(plugin: Plugin) {
    finish(run(plugin, Cli::parse()));
}

pub(crate) fn finish(result: Result<Value>) {
    match result {
        Ok(value) => println!("{}", serde_json::to_string(&value).expect("JSON response")),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::to_string(&json!({
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "details": error.details,
                    }
                }))
                .expect("JSON error")
            );
            std::process::exit(1);
        }
    }
}

fn run(plugin: Plugin, cli: Cli) -> Result<Value> {
    let mut store = Store::open(plugin)?;
    match cli.command {
        Command::Status => {
            let subjects =
                store
                    .connection
                    .query_row("SELECT COUNT(*) FROM subjects", [], |row| {
                        row.get::<_, i64>(0)
                    })?;
            let identity = store.identity(plugin)?;
            let logical_hash = canonical_logical_hash(plugin, &store.connection)?;
            let (receipt_count, receipt_digest) = sync_receipt_digest(&store.connection)?;
            let latest_receipt = latest_sync_receipt(&store.connection, plugin)?;
            let takeover_ready = latest_receipt.as_ref().is_some_and(|receipt| {
                receipt.runtime_state == "ready"
                    && receipt.state == "completed"
                    && validate_receipt(&store.connection, plugin, receipt).is_ok()
            });
            Ok(envelope(
                plugin,
                "status",
                json!({
                    "subjects": subjects,
                    "store": identity,
                    "logical_hash": logical_hash,
                    "sync_receipts": {
                        "count": receipt_count,
                        "digest": receipt_digest,
                        "latest_session_id": latest_receipt.map(|receipt| receipt.session_id),
                        "takeover_ready": takeover_ready,
                    },
                }),
            ))
        }
        Command::Subject { command } => run_subject(plugin, &mut store, command),
    }
}

pub(crate) fn run_subject(
    plugin: Plugin,
    store: &mut Store,
    command: SubjectCommand,
) -> Result<Value> {
    match command {
        SubjectCommand::Create { json } => {
            let input: SubjectCreate = read_json(&json)?;
            create_subject(plugin, store, input)
        }
        SubjectCommand::Ensure { json } => {
            let input: SubjectEnsure = read_json(&json)?;
            ensure_subject(plugin, store, input)
        }
        SubjectCommand::Show { id } => Ok(envelope(
            plugin,
            "subject.show",
            json!({"subject": subject(&store.connection, &id)?}),
        )),
        SubjectCommand::Rename {
            id,
            if_revision,
            json,
        } => {
            let input: SubjectRename = read_json(&json)?;
            rename_subject(plugin, store, &id, if_revision, input)
        }
    }
}

pub(crate) struct Store {
    pub(crate) connection: Connection,
    #[allow(dead_code)] // Used by Tutor; Book and Practice share this module without Tutor.
    pub(crate) root: PathBuf,
}

impl Store {
    pub(crate) fn open(plugin: Plugin) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::new("home_unavailable", "HOME is not configured"))?;
        let lwc = home.join(".lwc");
        let plugins = lwc.join("plugins");
        let root = plugins.join(plugin.id());
        for path in [&lwc, &plugins, &root] {
            reject_symlink(path)?;
            fs::create_dir_all(path)?;
            make_private_directory(path)?;
        }
        let database = root.join("data.sqlite3");
        reject_symlink(&database)?;
        let connection = Connection::open(&database)?;
        make_private_file(&database)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;")?;
        migrate_store(&connection, plugin)?;
        validate_store_schema(&connection, plugin)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        Ok(Self { connection, root })
    }

    pub(crate) fn identity(&self, plugin: Plugin) -> Result<PluginStoreIdentity> {
        let store_id = meta_value(&self.connection, "store_id")?;
        let revision = meta_value(&self.connection, "revision")?
            .parse::<i64>()
            .map_err(|_| Error::new("corrupt_store", "plugin store revision is invalid"))?;
        if revision < 0 {
            return Err(Error::new(
                "corrupt_store",
                "plugin store revision is invalid",
            ));
        }
        Ok(PluginStoreIdentity {
            plugin_id: plugin.id().to_owned(),
            store_id,
            revision,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PluginStoreIdentity {
    pub(crate) plugin_id: String,
    pub(crate) store_id: String,
    pub(crate) revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PluginSyncReceipt {
    pub(crate) session_id: String,
    pub(crate) plugin_id: String,
    pub(crate) store_id: String,
    pub(crate) source_revision: i64,
    pub(crate) resolved_revision: i64,
    pub(crate) logical_hash: String,
    pub(crate) completed_at: String,
    pub(crate) runtime_state: String,
    pub(crate) state: String,
    pub(crate) receipt_hash: String,
}

fn migrate_store(connection: &Connection, plugin: Plugin) -> Result<()> {
    let version = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    if version > STORE_SCHEMA_VERSION {
        return Err(Error::new(
            "unsupported_plugin_schema",
            format!("plugin store schema {version} is newer than supported {STORE_SCHEMA_VERSION}"),
        ));
    }
    if version < 0 {
        return Err(Error::new(
            "corrupt_store",
            "plugin store schema version is invalid",
        ));
    }
    if version == 0 {
        let objects = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if objects != 0 {
            return Err(Error::new(
                "corrupt_store",
                "unversioned plugin store contains unknown objects",
            ));
        }
    }
    if version == STORE_SCHEMA_VERSION {
        return Ok(());
    }
    let tx = connection.unchecked_transaction()?;
    if version == 1 {
        validate_shared_v1_schema(&tx)?;
    }
    if version == 0 {
        tx.execute_batch(
            "CREATE TABLE subjects(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL CHECK(trim(name)<>''),
               parent_id TEXT REFERENCES subjects(id),
               tags_json TEXT NOT NULL,
               revision INTEGER NOT NULL CHECK(revision>=1),
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE INDEX subjects_parent ON subjects(parent_id,id);
             CREATE TABLE subject_name_history(
               subject_id TEXT NOT NULL REFERENCES subjects(id),
               revision INTEGER NOT NULL,
               name TEXT NOT NULL,
               changed_at TEXT NOT NULL,
               PRIMARY KEY(subject_id,revision)
             );
             CREATE TABLE requests(
               request_id TEXT PRIMARY KEY,
               fingerprint TEXT NOT NULL,
               result_json TEXT NOT NULL
             );",
        )?;
    }
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS plugin_meta(
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS sync_receipts(
           session_id TEXT PRIMARY KEY,
           plugin_id TEXT NOT NULL,
           store_id TEXT NOT NULL,
           source_revision INTEGER NOT NULL CHECK(source_revision>=0),
           resolved_revision INTEGER NOT NULL CHECK(resolved_revision>=0),
           logical_hash TEXT NOT NULL CHECK(length(logical_hash)=64),
           completed_at TEXT NOT NULL,
           runtime_state TEXT NOT NULL CHECK(runtime_state IN ('ready','preserved_not_ready')),
           state TEXT NOT NULL CHECK(state='completed'),
           receipt_hash TEXT NOT NULL CHECK(length(receipt_hash)=64)
         ) WITHOUT ROWID;",
    )?;
    let store_id = new_store_id(plugin);
    tx.execute(
        "INSERT OR IGNORE INTO plugin_meta(key,value) VALUES('store_id',?1)",
        [&store_id],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO plugin_meta(key,value) VALUES('revision','0')",
        [],
    )?;
    tx.pragma_update(None, "user_version", STORE_SCHEMA_VERSION)?;
    validate_store_schema(&tx, plugin)?;
    tx.commit()?;
    Ok(())
}

fn validate_store_schema(connection: &Connection, plugin: Plugin) -> Result<()> {
    for (table, columns) in [
        (
            "subjects",
            &[
                "id",
                "name",
                "parent_id",
                "tags_json",
                "revision",
                "created_at",
                "updated_at",
            ][..],
        ),
        (
            "subject_name_history",
            &["subject_id", "revision", "name", "changed_at"][..],
        ),
        (
            "requests",
            &["request_id", "fingerprint", "result_json"][..],
        ),
        ("plugin_meta", &["key", "value"][..]),
        (
            "sync_receipts",
            &[
                "session_id",
                "plugin_id",
                "store_id",
                "source_revision",
                "resolved_revision",
                "logical_hash",
                "completed_at",
                "runtime_state",
                "state",
                "receipt_hash",
            ][..],
        ),
    ] {
        require_table_schema(connection, table, columns)?;
    }
    let identity = PluginStoreIdentity {
        plugin_id: plugin.id().to_owned(),
        store_id: meta_value(connection, "store_id")?,
        revision: meta_value(connection, "revision")?
            .parse::<i64>()
            .map_err(|_| Error::new("corrupt_store", "plugin store revision is invalid"))?,
    };
    if identity.store_id.len() != 64
        || !identity
            .store_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || identity.revision < 0
    {
        return Err(Error::new(
            "corrupt_store",
            "plugin store identity is invalid",
        ));
    }
    require_indexes(connection, &["subjects_parent"])?;
    require_canonical_table_coverage(connection, plugin)
}

fn validate_shared_v1_schema(connection: &Connection) -> Result<()> {
    for (table, columns) in [
        (
            "subjects",
            &[
                "id",
                "name",
                "parent_id",
                "tags_json",
                "revision",
                "created_at",
                "updated_at",
            ][..],
        ),
        (
            "subject_name_history",
            &["subject_id", "revision", "name", "changed_at"][..],
        ),
        (
            "requests",
            &["request_id", "fingerprint", "result_json"][..],
        ),
    ] {
        require_table_schema(connection, table, columns)?;
    }
    require_indexes(connection, &["subjects_parent"])
}

pub(crate) fn require_table_schema(
    connection: &Connection,
    table: &str,
    columns: &[&str],
) -> Result<()> {
    let actual = table_columns(connection, table)?;
    if actual != columns {
        return Err(Error::new(
            "corrupt_store",
            format!("plugin store table {table} has an invalid schema"),
        ));
    }
    Ok(())
}

pub(crate) fn require_indexes(connection: &Connection, names: &[&str]) -> Result<()> {
    for name in names {
        let present = connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE type='index' AND name=?1",
                [name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !present {
            return Err(Error::new(
                "corrupt_store",
                format!("plugin store index {name} is missing"),
            ));
        }
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn meta_value(connection: &Connection, key: &str) -> Result<String> {
    connection
        .query_row("SELECT value FROM plugin_meta WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| {
            Error::new(
                "corrupt_store",
                format!("plugin store metadata {key} is missing"),
            )
        })
}

pub(crate) fn bump_store_revision(tx: &Transaction<'_>) -> Result<i64> {
    let revision = meta_value(tx, "revision")?
        .parse::<i64>()
        .map_err(|_| Error::new("corrupt_store", "plugin store revision is invalid"))?;
    let next = revision
        .checked_add(1)
        .ok_or_else(|| Error::new("corrupt_store", "plugin store revision overflowed"))?;
    tx.execute(
        "UPDATE plugin_meta SET value=?1 WHERE key='revision'",
        [next.to_string()],
    )?;
    Ok(next)
}

pub(crate) fn canonical_logical_hash(plugin: Plugin, connection: &Connection) -> Result<String> {
    require_canonical_table_coverage(connection, plugin)?;
    let mut tables = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    tables.retain(|table| canonical_tables(plugin).contains(&table.as_str()));
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, plugin.id().as_bytes());
    for table in tables {
        hash_field(&mut hasher, table.as_bytes());
        let columns = table_columns(connection, &table)?;
        if columns.is_empty() {
            return Err(Error::new(
                "corrupt_store",
                format!("canonical table {table} has no columns"),
            ));
        }
        for column in &columns {
            hash_field(&mut hasher, column.as_bytes());
        }
        let selected = columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {selected} FROM {} ORDER BY {selected}",
            quote_identifier(&table)
        );
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hasher.update([0xff]);
            for index in 0..columns.len() {
                match row.get_ref(index)? {
                    ValueRef::Null => hasher.update([0]),
                    ValueRef::Integer(value) => {
                        hasher.update([1]);
                        hash_field(&mut hasher, &value.to_be_bytes());
                    }
                    ValueRef::Real(value) => {
                        hasher.update([2]);
                        hash_field(&mut hasher, &value.to_bits().to_be_bytes());
                    }
                    ValueRef::Text(value) => {
                        hasher.update([3]);
                        hash_field(&mut hasher, value);
                    }
                    ValueRef::Blob(value) => {
                        hasher.update([4]);
                        hash_field(&mut hasher, value);
                    }
                }
            }
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn canonical_tables(plugin: Plugin) -> &'static [&'static str] {
    const TUTOR: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "tutor_sessions",
        "tutor_diagnoses",
        "tutor_turns",
        "soul_versions",
        "soul_settings",
        "soul_settings_history",
        "soul_proposals",
        "learner_facts",
        "learner_fact_history",
        "tutor_goals",
        "tutor_goal_criteria",
        "tutor_goal_evidence",
        "tutor_goal_history",
        "tutor_plans",
        "tutor_plan_versions",
        "tutor_plan_steps",
        "tutor_plan_step_history",
    ];
    const BOOK: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "books",
        "book_blocks",
        "book_anomalies",
        "book_cursors",
        "book_leases",
        "book_lease_owner_history",
        "book_window_reports",
        "book_syntheses",
        "book_summaries",
        "book_mainline",
        "book_relations",
    ];
    const PRACTICE: &[&str] = &[
        "subjects",
        "subject_name_history",
        "requests",
        "practice_banks",
        "practice_items",
        "bank_items",
        "practice_sets",
        "set_members",
        "set_member_events",
        "papers",
        "paper_items",
        "attempts",
        "attempt_takeover_history",
        "responses",
        "response_history",
        "grades",
        "grade_history",
        "review_controls",
        "review_events",
        "fsrs_cards",
        "review_debt",
        "review_debt_events",
    ];
    match plugin {
        Plugin::Tutor => TUTOR,
        Plugin::Book => BOOK,
        Plugin::Practice => PRACTICE,
    }
}

fn require_canonical_table_coverage(connection: &Connection, plugin: Plugin) -> Result<()> {
    const SHARED_NON_CANONICAL: &[&str] = &["plugin_meta", "sync_receipts"];
    const BOOK_DERIVED: &[&str] = &[
        "book_blocks_fts",
        "book_blocks_fts_data",
        "book_blocks_fts_idx",
        "book_blocks_fts_content",
        "book_blocks_fts_docsize",
        "book_blocks_fts_config",
    ];
    let derived: &[&str] = match plugin {
        Plugin::Book => BOOK_DERIVED,
        Plugin::Tutor | Plugin::Practice => &[],
    };
    let tables = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let canonical = canonical_tables(plugin);
    if let Some(table) = tables.iter().find(|table| {
        !canonical.contains(&table.as_str())
            && !SHARED_NON_CANONICAL.contains(&table.as_str())
            && !derived.contains(&table.as_str())
    }) {
        return Err(Error::new(
            "corrupt_store",
            format!("plugin store table {table} is not in the fixed canonical schema"),
        ));
    }
    let missing_shared = canonical[..3]
        .iter()
        .find(|table| !tables.iter().any(|actual| actual == **table));
    if let Some(table) = missing_shared {
        return Err(Error::new(
            "corrupt_store",
            format!("canonical plugin store table {table} is missing"),
        ));
    }
    let domain_present = canonical[3..]
        .iter()
        .any(|table| tables.iter().any(|actual| actual == *table));
    if domain_present
        && let Some(table) = canonical[3..]
            .iter()
            .find(|table| !tables.iter().any(|actual| actual == **table))
    {
        return Err(Error::new(
            "corrupt_store",
            format!("canonical plugin store table {table} is missing"),
        ));
    }
    Ok(())
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

pub(crate) fn record_sync_receipt(
    tx: &Transaction<'_>,
    plugin: Plugin,
    session_id: &str,
    source_revision: i64,
    resolved_revision: i64,
    logical_hash: &str,
    runtime_state: &str,
) -> Result<PluginSyncReceipt> {
    validate_request_id(session_id)?;
    if source_revision < 0
        || resolved_revision < 0
        || !valid_hash(logical_hash)
        || !matches!(runtime_state, "ready" | "preserved_not_ready")
    {
        return Err(Error::new(
            "invalid_sync_receipt",
            "Sync receipt fields are invalid",
        ));
    }
    let store_id = meta_value(tx, "store_id")?;
    let current_revision = meta_value(tx, "revision")?
        .parse::<i64>()
        .map_err(|_| Error::new("corrupt_store", "plugin store revision is invalid"))?;
    if current_revision != resolved_revision {
        return Err(Error::new(
            "sync_store_changed",
            "resolved revision does not match the current plugin store",
        ));
    }
    let current_hash = canonical_logical_hash(plugin, tx)?;
    if current_hash != logical_hash {
        return Err(Error::new(
            "sync_store_changed",
            "resolved logical hash does not match the current plugin store",
        ));
    }
    if let Some(existing) = sync_receipt(tx, session_id)? {
        if existing.plugin_id == plugin.id()
            && existing.store_id == store_id
            && existing.source_revision == source_revision
            && existing.resolved_revision == resolved_revision
            && existing.logical_hash == logical_hash
            && existing.runtime_state == runtime_state
            && existing.state == "completed"
            && existing.receipt_hash == receipt_hash(&existing)?
        {
            return Ok(existing);
        }
        return Err(Error::new(
            "sync_receipt_conflict",
            "Sync session already has a different receipt",
        ));
    }
    let completed_at = now(tx)?;
    let mut receipt = PluginSyncReceipt {
        session_id: session_id.to_owned(),
        plugin_id: plugin.id().to_owned(),
        store_id,
        source_revision,
        resolved_revision,
        logical_hash: logical_hash.to_owned(),
        completed_at,
        runtime_state: runtime_state.to_owned(),
        state: "completed".to_owned(),
        receipt_hash: String::new(),
    };
    receipt.receipt_hash = receipt_hash(&receipt)?;
    tx.execute(
        "INSERT INTO sync_receipts(session_id,plugin_id,store_id,source_revision,
           resolved_revision,logical_hash,completed_at,runtime_state,state,receipt_hash)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            receipt.session_id,
            receipt.plugin_id,
            receipt.store_id,
            receipt.source_revision,
            receipt.resolved_revision,
            receipt.logical_hash,
            receipt.completed_at,
            receipt.runtime_state,
            receipt.state,
            receipt.receipt_hash,
        ],
    )?;
    Ok(receipt)
}

pub(crate) fn latest_sync_receipt(
    connection: &Connection,
    plugin: Plugin,
) -> Result<Option<PluginSyncReceipt>> {
    let receipt = connection
        .query_row(
            "SELECT session_id,plugin_id,store_id,source_revision,resolved_revision,
                    logical_hash,completed_at,runtime_state,state,receipt_hash
             FROM sync_receipts ORDER BY resolved_revision DESC,completed_at DESC,session_id DESC
             LIMIT 1",
            [],
            receipt_row,
        )
        .optional()?;
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    if receipt.plugin_id != plugin.id() || receipt.receipt_hash != receipt_hash(&receipt)? {
        return Err(Error::new(
            "invalid_sync_receipt",
            "stored Sync receipt is invalid",
        ));
    }
    Ok(Some(receipt))
}

pub(crate) fn require_latest_sync_receipt(
    connection: &Connection,
    plugin: Plugin,
    session_id: &str,
) -> Result<PluginSyncReceipt> {
    let receipt = latest_sync_receipt(connection, plugin)?.ok_or_else(|| {
        Error::new(
            "sync_receipt_missing",
            "plugin store has no completed Sync receipt",
        )
    })?;
    if receipt.session_id != session_id {
        return Err(Error::new(
            "stale_sync_receipt",
            "takeover requires the latest Sync receipt",
        ));
    }
    if receipt.runtime_state != "ready" || receipt.state != "completed" {
        return Err(Error::new(
            "sync_receipt_not_ready",
            "takeover requires a completed ready-runtime Sync receipt",
        ));
    }
    validate_receipt(connection, plugin, &receipt)?;
    Ok(receipt)
}

pub(crate) fn sync_receipt_digest(connection: &Connection) -> Result<(u64, String)> {
    let mut statement = connection
        .prepare("SELECT session_id,receipt_hash FROM sync_receipts ORDER BY session_id")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut hasher = Sha256::new();
    for (session_id, receipt_hash) in &rows {
        hash_field(&mut hasher, session_id.as_bytes());
        hash_field(&mut hasher, receipt_hash.as_bytes());
    }
    Ok((
        rows.len() as u64,
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

fn sync_receipt(connection: &Connection, session_id: &str) -> Result<Option<PluginSyncReceipt>> {
    Ok(connection
        .query_row(
            "SELECT session_id,plugin_id,store_id,source_revision,resolved_revision,
                    logical_hash,completed_at,runtime_state,state,receipt_hash
             FROM sync_receipts WHERE session_id=?1",
            [session_id],
            receipt_row,
        )
        .optional()?)
}

fn receipt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginSyncReceipt> {
    Ok(PluginSyncReceipt {
        session_id: row.get(0)?,
        plugin_id: row.get(1)?,
        store_id: row.get(2)?,
        source_revision: row.get(3)?,
        resolved_revision: row.get(4)?,
        logical_hash: row.get(5)?,
        completed_at: row.get(6)?,
        runtime_state: row.get(7)?,
        state: row.get(8)?,
        receipt_hash: row.get(9)?,
    })
}

fn validate_receipt(
    connection: &Connection,
    plugin: Plugin,
    receipt: &PluginSyncReceipt,
) -> Result<()> {
    if receipt.plugin_id != plugin.id()
        || !matches!(
            receipt.runtime_state.as_str(),
            "ready" | "preserved_not_ready"
        )
        || receipt.state != "completed"
        || receipt.store_id != meta_value(connection, "store_id")?
        || receipt.resolved_revision
            != meta_value(connection, "revision")?
                .parse::<i64>()
                .map_err(|_| Error::new("corrupt_store", "plugin store revision is invalid"))?
        || receipt.logical_hash != canonical_logical_hash(plugin, connection)?
        || receipt.receipt_hash != receipt_hash(receipt)?
    {
        return Err(Error::new(
            "stale_sync_receipt",
            "Sync receipt does not match the current plugin store",
        ));
    }
    Ok(())
}

fn receipt_hash(receipt: &PluginSyncReceipt) -> Result<String> {
    let canonical = serde_json::to_vec(&json!({
        "completed_at": receipt.completed_at,
        "logical_hash": receipt.logical_hash,
        "plugin_id": receipt.plugin_id,
        "runtime_state": receipt.runtime_state,
        "resolved_revision": receipt.resolved_revision,
        "session_id": receipt.session_id,
        "source_revision": receipt.source_revision,
        "state": receipt.state,
        "store_id": receipt.store_id,
    }))
    .map_err(|error| Error::new("json_error", error.to_string()))?;
    Ok(Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn new_store_id(plugin: Plugin) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!(
        "store:{}:{timestamp}:{}:{counter}",
        plugin.id(),
        std::process::id()
    ));
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn create_subject(plugin: Plugin, store: &mut Store, mut input: SubjectCreate) -> Result<Value> {
    validate_name(&input.name)?;
    validate_request_id(&input.request_id)?;
    validate_optional_id(input.parent_id.as_deref())?;
    normalize_tags(&mut input.tags)?;
    let fingerprint = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    ensure_parent(&tx, input.parent_id.as_deref())?;
    let id = new_id(plugin, &input.request_id);
    let now = now(&tx)?;
    let tags = serde_json::to_string(&input.tags)
        .map_err(|error| Error::new("json_error", error.to_string()))?;
    tx.execute(
        "INSERT INTO subjects(id,name,parent_id,tags_json,revision,created_at,updated_at)
         VALUES(?1,?2,?3,?4,1,?5,?5)",
        params![id, input.name, input.parent_id, tags, now],
    )?;
    tx.execute(
        "INSERT INTO subject_name_history(subject_id,revision,name,changed_at)
         VALUES(?1,1,?2,?3)",
        params![id, input.name, now],
    )?;
    let value = finalize_mutation(
        &tx,
        plugin,
        &input.request_id,
        &fingerprint,
        envelope(
            plugin,
            "subject.create",
            json!({"subject": subject(&tx, &id)?}),
        ),
    )?;
    tx.commit()?;
    Ok(value)
}

fn ensure_subject(plugin: Plugin, store: &mut Store, mut input: SubjectEnsure) -> Result<Value> {
    validate_id(&input.id)?;
    validate_name(&input.name)?;
    validate_request_id(&input.request_id)?;
    validate_optional_id(input.parent_id.as_deref())?;
    normalize_tags(&mut input.tags)?;
    let fingerprint = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let existing = subject_optional(&tx, &input.id)?;
    let created = existing.is_none();
    if let Some(existing) = existing {
        if existing["name"] != input.name
            || existing["parent_id"] != json!(input.parent_id)
            || existing["tags"] != json!(input.tags)
        {
            return Err(Error::new(
                "subject_identity_conflict",
                "subject ID already exists with different metadata",
            ));
        }
    } else {
        ensure_parent(&tx, input.parent_id.as_deref())?;
        let now = now(&tx)?;
        let tags = serde_json::to_string(&input.tags)
            .map_err(|error| Error::new("json_error", error.to_string()))?;
        tx.execute(
            "INSERT INTO subjects(id,name,parent_id,tags_json,revision,created_at,updated_at)
             VALUES(?1,?2,?3,?4,1,?5,?5)",
            params![input.id, input.name, input.parent_id, tags, now],
        )?;
        tx.execute(
            "INSERT INTO subject_name_history(subject_id,revision,name,changed_at)
             VALUES(?1,1,?2,?3)",
            params![input.id, input.name, now],
        )?;
    }
    let value = finalize_mutation(
        &tx,
        plugin,
        &input.request_id,
        &fingerprint,
        envelope(
            plugin,
            "subject.ensure",
            json!({"subject": subject(&tx, &input.id)?, "created": created}),
        ),
    )?;
    tx.commit()?;
    Ok(value)
}

fn rename_subject(
    plugin: Plugin,
    store: &mut Store,
    id: &str,
    if_revision: i64,
    input: SubjectRename,
) -> Result<Value> {
    validate_id(id)?;
    validate_name(&input.name)?;
    validate_request_id(&input.request_id)?;
    if if_revision < 1 {
        return Err(Error::new("invalid_input", "if_revision must be positive"));
    }
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let fingerprint = fingerprint(&json!({
        "id": id,
        "if_revision": if_revision,
        "input": input,
    }))?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = subject(&tx, id)?;
    let revision = current["revision"].as_i64().unwrap();
    if revision != if_revision {
        return Err(
            Error::new("revision_conflict", "subject revision is stale").details(json!({
                "expected": if_revision,
                "current": revision,
            })),
        );
    }
    let next = revision + 1;
    let now = now(&tx)?;
    tx.execute(
        "UPDATE subjects SET name=?2,revision=?3,updated_at=?4 WHERE id=?1",
        params![id, input.name, next, now],
    )?;
    tx.execute(
        "INSERT INTO subject_name_history(subject_id,revision,name,changed_at)
         VALUES(?1,?2,?3,?4)",
        params![id, next, input.name, now],
    )?;
    let value = finalize_mutation(
        &tx,
        plugin,
        &input.request_id,
        &fingerprint,
        envelope(
            plugin,
            "subject.rename",
            json!({"subject": subject(&tx, id)?}),
        ),
    )?;
    tx.commit()?;
    Ok(value)
}

pub(crate) fn subject(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    subject_optional(connection, id)?
        .ok_or_else(|| Error::new("subject_not_found", format!("subject {id} was not found")))
}

fn subject_optional(connection: &Connection, id: &str) -> Result<Option<Value>> {
    let row = connection
        .query_row(
            "SELECT id,name,parent_id,tags_json,revision,created_at,updated_at
             FROM subjects WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(id, name, parent_id, tags, revision, created_at, updated_at)| {
            let tags: Value = serde_json::from_str(&tags)
                .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
            Ok(json!({
                "id": id,
                "name": name,
                "parent_id": parent_id,
                "tags": tags,
                "revision": revision,
                "created_at": created_at,
                "updated_at": updated_at,
            }))
        },
    )
    .transpose()
}

pub(crate) fn replay(
    tx: &Transaction<'_>,
    request_id: &str,
    fingerprint: &str,
) -> Result<Option<Value>> {
    let existing = tx
        .query_row(
            "SELECT fingerprint,result_json FROM requests WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((stored, result)) = existing else {
        return Ok(None);
    };
    if stored != fingerprint {
        return Err(Error::new(
            "request_id_reused",
            "request_id was already used with different input",
        ));
    }
    serde_json::from_str(&result)
        .map(Some)
        .map_err(|error| Error::new("corrupt_store", error.to_string()))
}

pub(crate) fn remember(
    tx: &Transaction<'_>,
    request_id: &str,
    fingerprint: &str,
    value: &Value,
) -> Result<()> {
    let result = serde_json::to_string(value)
        .map_err(|error| Error::new("json_error", error.to_string()))?;
    tx.execute(
        "INSERT INTO requests(request_id,fingerprint,result_json) VALUES(?1,?2,?3)",
        params![request_id, fingerprint, result],
    )?;
    Ok(())
}

pub(crate) fn finalize_mutation(
    tx: &Transaction<'_>,
    plugin: Plugin,
    request_id: &str,
    fingerprint: &str,
    mut value: Value,
) -> Result<Value> {
    validate_request_id(request_id)?;
    let revision = bump_store_revision(tx)?;
    let store_id = meta_value(tx, "store_id")?;
    let committed_at = now(tx)?;
    let object = value.as_object_mut().ok_or_else(|| {
        Error::new(
            "invalid_mutation_result",
            "mutation result must use the canonical envelope",
        )
    })?;
    object.insert(
        "store".to_owned(),
        json!({"id": store_id, "plugin": plugin.id(), "revision": revision}),
    );
    object.insert("request_id".to_owned(), json!(request_id));
    object.insert("committed_at".to_owned(), json!(committed_at));
    remember(tx, request_id, fingerprint, &value)?;
    Ok(value)
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T> {
    let bytes = if source == "-" {
        let mut bytes = Vec::new();
        io::stdin()
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        bytes
    } else if let Some(path) = source.strip_prefix('@') {
        if path.is_empty() {
            return Err(Error::new("invalid_input", "@PATH requires a file"));
        }
        let path = PathBuf::from(path);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::new(
                "unsafe_input",
                "JSON input must be a regular file",
            ));
        }
        if metadata.len() > MAX_INPUT_BYTES {
            return Err(Error::new(
                "input_too_large",
                format!("JSON input exceeds {MAX_INPUT_BYTES} bytes"),
            ));
        }
        fs::read(path)?
    } else {
        source.as_bytes().to_vec()
    };
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(Error::new(
            "input_too_large",
            format!("JSON input exceeds {MAX_INPUT_BYTES} bytes"),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::new("invalid_input", "JSON input is not UTF-8"))?;
    serde_json::from_str(text).map_err(|error| Error::new("invalid_input", error.to_string()))
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 256 {
        return Err(Error::new(
            "invalid_input",
            "subject name must contain 1..=256 characters",
        ));
    }
    Ok(())
}

pub(crate) fn validate_request_id(request_id: &str) -> Result<()> {
    if request_id.trim().is_empty() || request_id.len() > 256 {
        return Err(Error::new(
            "invalid_input",
            "request_id must contain 1..=256 bytes",
        ));
    }
    Ok(())
}

fn validate_optional_id(id: Option<&str>) -> Result<()> {
    if let Some(id) = id {
        validate_id(id)?;
    }
    Ok(())
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    if id.len() < 16
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
    {
        return Err(Error::new("invalid_id", "subject ID is invalid"));
    }
    Ok(())
}

fn normalize_tags(tags: &mut Vec<String>) -> Result<()> {
    for tag in tags.iter_mut() {
        *tag = tag.trim().to_owned();
        if tag.is_empty() || tag.chars().count() > 64 {
            return Err(Error::new(
                "invalid_input",
                "subject tags must contain 1..=64 characters",
            ));
        }
    }
    tags.sort();
    tags.dedup();
    if tags.len() > 64 {
        return Err(Error::new(
            "invalid_input",
            "a subject accepts at most 64 tags",
        ));
    }
    Ok(())
}

fn ensure_parent(tx: &Transaction<'_>, parent_id: Option<&str>) -> Result<()> {
    if let Some(parent_id) = parent_id {
        let exists = tx
            .query_row(
                "SELECT 1 FROM subjects WHERE id=?1",
                [parent_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(Error::new(
                "subject_parent_not_found",
                format!("parent subject {parent_id} was not found"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn fingerprint(value: &impl Serialize) -> Result<String> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| Error::new("json_error", error.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(crate) fn new_id(plugin: Plugin, request_id: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!(
        "{}:{request_id}:{timestamp}:{}:{counter}",
        plugin.id(),
        std::process::id()
    ));
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn now(connection: &Connection) -> Result<String> {
    connection
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
            row.get(0)
        })
        .map_err(Into::into)
}

pub(crate) fn envelope(plugin: Plugin, command: &str, result: Value) -> Value {
    json!({
        "schema_version": 1,
        "plugin": plugin.id(),
        "command": command,
        "result": result,
    })
}

pub(crate) fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::new(
            "unsafe_store_path",
            format!("store path is a symlink: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn make_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_private_file(_path: &Path) -> Result<()> {
    Ok(())
}
