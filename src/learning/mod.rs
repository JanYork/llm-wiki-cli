use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
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
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[allow(dead_code)] // Each fixed binary constructs exactly one of the three variants.
pub enum Plugin {
    Tutor,
    Book,
    Practice,
}

impl Plugin {
    fn id(self) -> &'static str {
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
enum SubjectCommand {
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
        #[arg(long)]
        name: String,
    },
}

#[derive(Debug)]
struct Error {
    code: &'static str,
    message: String,
    details: Option<Value>,
}

impl Error {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
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

type Result<T> = std::result::Result<T, Error>;

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

pub fn main(plugin: Plugin) {
    let result = run(plugin, Cli::parse());
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
            Ok(envelope(plugin, "status", json!({"subjects": subjects})))
        }
        Command::Subject { command } => match command {
            SubjectCommand::Create { json } => {
                let input: SubjectCreate = read_json(&json)?;
                create_subject(plugin, &mut store, input)
            }
            SubjectCommand::Ensure { json } => {
                let input: SubjectEnsure = read_json(&json)?;
                ensure_subject(plugin, &mut store, input)
            }
            SubjectCommand::Show { id } => Ok(envelope(
                plugin,
                "subject.show",
                json!({"subject": subject(&store.connection, &id)?}),
            )),
            SubjectCommand::Rename {
                id,
                if_revision,
                name,
            } => rename_subject(plugin, &mut store, &id, if_revision, &name),
        },
    }
}

struct Store {
    connection: Connection,
}

impl Store {
    fn open(plugin: Plugin) -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::new("home_unavailable", "HOME is not configured"))?;
        let lwc = home.join(".lwc");
        let plugins = lwc.join("plugins");
        let root = plugins.join(plugin.id());
        for path in [&lwc, &plugins, &root] {
            reject_symlink(path)?;
            fs::create_dir_all(path)?;
        }
        let database = root.join("data.sqlite3");
        reject_symlink(&database)?;
        let connection = Connection::open(&database)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS subjects(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL CHECK(trim(name)<>''),
               parent_id TEXT REFERENCES subjects(id),
               tags_json TEXT NOT NULL,
               revision INTEGER NOT NULL CHECK(revision>=1),
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS subjects_parent ON subjects(parent_id,id);
             CREATE TABLE IF NOT EXISTS subject_name_history(
               subject_id TEXT NOT NULL REFERENCES subjects(id),
               revision INTEGER NOT NULL,
               name TEXT NOT NULL,
               changed_at TEXT NOT NULL,
               PRIMARY KEY(subject_id,revision)
             );
             CREATE TABLE IF NOT EXISTS requests(
               request_id TEXT PRIMARY KEY,
               fingerprint TEXT NOT NULL,
               result_json TEXT NOT NULL
             );
             PRAGMA user_version=1;",
        )?;
        Ok(Self { connection })
    }
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
    let value = envelope(
        plugin,
        "subject.create",
        json!({"subject": subject(&tx, &id)?}),
    );
    remember(&tx, &input.request_id, &fingerprint, &value)?;
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
    let value = envelope(
        plugin,
        "subject.ensure",
        json!({"subject": subject(&tx, &input.id)?, "created": created}),
    );
    remember(&tx, &input.request_id, &fingerprint, &value)?;
    tx.commit()?;
    Ok(value)
}

fn rename_subject(
    plugin: Plugin,
    store: &mut Store,
    id: &str,
    if_revision: i64,
    name: &str,
) -> Result<Value> {
    validate_id(id)?;
    validate_name(name)?;
    if if_revision < 1 {
        return Err(Error::new("invalid_input", "if_revision must be positive"));
    }
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
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
        params![id, name, next, now],
    )?;
    tx.execute(
        "INSERT INTO subject_name_history(subject_id,revision,name,changed_at)
         VALUES(?1,?2,?3,?4)",
        params![id, next, name, now],
    )?;
    let value = envelope(
        plugin,
        "subject.rename",
        json!({"subject": subject(&tx, id)?}),
    );
    tx.commit()?;
    Ok(value)
}

fn subject(connection: &Connection, id: &str) -> Result<Value> {
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

fn replay(tx: &Transaction<'_>, request_id: &str, fingerprint: &str) -> Result<Option<Value>> {
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

fn remember(
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

fn read_json<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T> {
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

fn validate_request_id(request_id: &str) -> Result<()> {
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

fn validate_id(id: &str) -> Result<()> {
    if id.len() < 16
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'z').contains(&byte))
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

fn fingerprint(value: &impl Serialize) -> Result<String> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| Error::new("json_error", error.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn new_id(plugin: Plugin, request_id: &str) -> String {
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

fn now(connection: &Connection) -> Result<String> {
    connection
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |row| {
            row.get(0)
        })
        .map_err(Into::into)
}

fn envelope(plugin: Plugin, command: &str, result: Value) -> Value {
    json!({
        "schema_version": 1,
        "plugin": plugin.id(),
        "command": command,
        "result": result,
    })
}

fn reject_symlink(path: &Path) -> Result<()> {
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
