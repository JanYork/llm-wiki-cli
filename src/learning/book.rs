use crate::learning::*;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
};

const MAX_BOOK_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Parser)]
struct BookCli {
    #[command(subcommand)]
    command: BookCommand,
}

#[derive(Subcommand)]
enum BookCommand {
    Subject {
        #[command(subcommand)]
        command: SubjectCommand,
    },
    Import {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Prepare {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Search {
        id: String,
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Peek {
        id: String,
        #[arg(long)]
        start: i64,
        #[arg(long, default_value_t = 1)]
        count: usize,
    },
    Read {
        #[command(subcommand)]
        command: ReadCommand,
    },
    Synthesis {
        #[command(subcommand)]
        command: SynthesisCommand,
    },
    Show {
        id: String,
    },
    Status,
}

#[derive(Subcommand)]
enum SynthesisCommand {
    Publish {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
}

#[derive(Subcommand)]
enum ReadCommand {
    Next {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Commit {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Renew {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Takeover {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BookImport {
    subject_id: String,
    path: PathBuf,
    title: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    edition_of: Option<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BookPrepare {
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadNext {
    owner: String,
    #[serde(default)]
    budget: Option<ReadBudget>,
    #[serde(default = "default_lease_seconds")]
    lease_seconds: i64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadRenew {
    owner: String,
    lease_seconds: i64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadTakeover {
    entity_id: String,
    old_owner: String,
    new_owner: String,
    if_revision: i64,
    sync_session_id: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadBudget {
    unit: String,
    value: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadCommit {
    owner: String,
    range_hash: String,
    summary: String,
    key_points: Vec<ReportPoint>,
    new_concepts: Vec<String>,
    prior_links: Vec<String>,
    open_threads: Vec<String>,
    anomalies: Vec<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportPoint {
    text: String,
    block_id: String,
    source_hash: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SynthesisPublish {
    #[serde(default)]
    supersedes_revision: Option<i64>,
    #[serde(default)]
    anomaly_dispositions: Vec<AnomalyDisposition>,
    summaries: Vec<SynthesisSummary>,
    mainline: Vec<SynthesisMainline>,
    relations: Vec<SynthesisRelation>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnomalyDisposition {
    id: String,
    status: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SynthesisSummary {
    level: String,
    title: String,
    summary: String,
    start_ordinal: i64,
    end_ordinal: i64,
    source_hashes: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SynthesisMainline {
    text: String,
    start_ordinal: i64,
    end_ordinal: i64,
    source_hashes: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SynthesisRelation {
    from: String,
    to: String,
    kind: String,
    confidence: f64,
    source_hashes: Vec<String>,
}

pub(crate) fn main() {
    finish(run(BookCli::parse()));
}

fn run(cli: BookCli) -> Result<Value> {
    let mut store = Store::open(Plugin::Book)?;
    initialize(&store.connection, &store.root)?;
    match cli.command {
        BookCommand::Subject { command } => run_subject(Plugin::Book, &mut store, command),
        BookCommand::Import { json } => import(&mut store, read_json(&json)?),
        BookCommand::Prepare {
            id,
            if_revision,
            json,
        } => prepare(&mut store, &id, if_revision, read_json(&json)?),
        BookCommand::Search { id, query, limit } => {
            search(&store.connection, &store.root, &id, &query, limit)
        }
        BookCommand::Peek { id, start, count } => {
            peek(&store.connection, &store.root, &id, start, count)
        }
        BookCommand::Read { command } => match command {
            ReadCommand::Next {
                id,
                if_revision,
                json,
            } => read_next(&mut store, &id, if_revision, read_json(&json)?),
            ReadCommand::Commit {
                id,
                if_revision,
                json,
            } => read_commit(&mut store, &id, if_revision, read_json(&json)?),
            ReadCommand::Renew {
                id,
                if_revision,
                json,
            } => read_renew(&mut store, &id, if_revision, read_json(&json)?),
            ReadCommand::Takeover { json } => read_takeover(&mut store, read_json(&json)?),
        },
        BookCommand::Synthesis { command } => match command {
            SynthesisCommand::Publish {
                id,
                if_revision,
                json,
            } => publish_synthesis(&mut store, &id, if_revision, read_json(&json)?),
        },
        BookCommand::Show { id } => Ok(envelope(
            Plugin::Book,
            "show",
            json!({"book": book(&store.connection, &id)?}),
        )),
        BookCommand::Status => {
            let books = store
                .connection
                .query_row("SELECT COUNT(*) FROM books", [], |row| row.get::<_, i64>(0))?;
            let identity = store.identity(Plugin::Book)?;
            Ok(envelope(
                Plugin::Book,
                "status",
                json!({"books":books,"store":identity}),
            ))
        }
    }
}

fn initialize(connection: &Connection, root: &Path) -> Result<()> {
    let existing = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type='table' AND name='books'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if existing {
        validate_book_schema(connection)?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS books(
           id TEXT PRIMARY KEY,
           subject_id TEXT NOT NULL REFERENCES subjects(id),
           title TEXT NOT NULL CHECK(trim(title)<>''),
           author TEXT,
           format TEXT NOT NULL CHECK(format IN ('txt','markdown','epub','pdf')),
           original_sha256 TEXT NOT NULL UNIQUE,
           original_bytes INTEGER NOT NULL CHECK(original_bytes>0),
           normalized_sha256 TEXT,
           normalized_bytes INTEGER,
           converter TEXT,
           converter_args_json TEXT,
           edition_of TEXT REFERENCES books(id),
           state TEXT NOT NULL CHECK(state IN
             ('imported','normalized','structured','indexed','ready','reading','covered','synthesized')),
           revision INTEGER NOT NULL CHECK(revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS books_subject ON books(subject_id,state,id);
         CREATE INDEX IF NOT EXISTS books_edition ON books(edition_of,id);",
    )?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS book_blocks(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL REFERENCES books(id),
           ordinal INTEGER NOT NULL CHECK(ordinal>=0),
           logical_block_id TEXT NOT NULL,
           subordinal INTEGER NOT NULL CHECK(subordinal>=0),
           byte_start INTEGER NOT NULL CHECK(byte_start>=0),
           byte_end INTEGER NOT NULL CHECK(byte_end>byte_start),
           text_hash TEXT NOT NULL,
           heading_path_json TEXT NOT NULL,
           UNIQUE(book_id,ordinal),
           UNIQUE(book_id,logical_block_id,subordinal),
           UNIQUE(book_id,byte_start,byte_end)
         );
         CREATE INDEX IF NOT EXISTS book_blocks_order ON book_blocks(book_id,ordinal);
         CREATE TABLE IF NOT EXISTS book_anomalies(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL REFERENCES books(id),
           kind TEXT NOT NULL,
           details TEXT NOT NULL,
           status TEXT NOT NULL CHECK(status IN ('open','accepted','resolved')),
           created_at TEXT NOT NULL,
           UNIQUE(book_id,kind,details)
         );
         CREATE INDEX IF NOT EXISTS book_anomalies_book
           ON book_anomalies(book_id,status,id);
         CREATE VIRTUAL TABLE IF NOT EXISTS book_blocks_fts USING fts5(
           text,content='',tokenize='trigram'
         );
         CREATE TABLE IF NOT EXISTS book_cursors(
           book_id TEXT PRIMARY KEY REFERENCES books(id),
           next_ordinal INTEGER NOT NULL CHECK(next_ordinal>=0),
           committed_blocks INTEGER NOT NULL CHECK(committed_blocks>=0),
           revision INTEGER NOT NULL CHECK(revision>=1),
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS book_leases(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL REFERENCES books(id),
           owner TEXT NOT NULL,
           start_ordinal INTEGER NOT NULL,
           end_ordinal INTEGER NOT NULL,
           range_hash TEXT NOT NULL,
           requested_unit TEXT NOT NULL,
           requested_value INTEGER NOT NULL,
           source_limit INTEGER NOT NULL,
           used_bytes INTEGER NOT NULL,
           used_chars INTEGER NOT NULL,
           coverage_committed_before INTEGER NOT NULL,
           coverage_total INTEGER NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('active','committed','expired','superseded')),
           revision INTEGER NOT NULL CHECK(revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           expires_at TEXT NOT NULL,
           committed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS book_leases_book
           ON book_leases(book_id,state,created_at,id);
         CREATE UNIQUE INDEX IF NOT EXISTS book_one_active_lease
           ON book_leases(book_id) WHERE state='active';
         CREATE TABLE IF NOT EXISTS book_window_reports(
           lease_id TEXT PRIMARY KEY REFERENCES book_leases(id),
           summary TEXT NOT NULL,
           key_points_json TEXT NOT NULL,
           new_concepts_json TEXT NOT NULL,
           prior_links_json TEXT NOT NULL,
           open_threads_json TEXT NOT NULL,
           anomalies_json TEXT NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS book_lease_owner_history(
           lease_id TEXT NOT NULL REFERENCES book_leases(id),
           revision INTEGER NOT NULL CHECK(revision>=2),
           old_owner TEXT NOT NULL,
           new_owner TEXT NOT NULL,
           sync_session_id TEXT NOT NULL,
           changed_at TEXT NOT NULL,
           PRIMARY KEY(lease_id,revision)
         );
         CREATE INDEX IF NOT EXISTS book_lease_owner_history_session
           ON book_lease_owner_history(sync_session_id,lease_id,revision);
         CREATE TABLE IF NOT EXISTS book_syntheses(
           book_id TEXT NOT NULL REFERENCES books(id),
           revision INTEGER NOT NULL,
           supersedes_revision INTEGER,
           created_at TEXT NOT NULL,
           PRIMARY KEY(book_id,revision)
         );
         CREATE TABLE IF NOT EXISTS book_summaries(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL,
           synthesis_revision INTEGER NOT NULL,
           level TEXT NOT NULL CHECK(level IN ('section','chapter','part','book')),
           title TEXT NOT NULL,
           summary TEXT NOT NULL,
           start_ordinal INTEGER NOT NULL,
           end_ordinal INTEGER NOT NULL,
           source_hashes_json TEXT NOT NULL,
           FOREIGN KEY(book_id,synthesis_revision) REFERENCES book_syntheses(book_id,revision)
         );
         CREATE TABLE IF NOT EXISTS book_mainline(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL,
           synthesis_revision INTEGER NOT NULL,
           ordinal INTEGER NOT NULL,
           text TEXT NOT NULL,
           start_ordinal INTEGER NOT NULL,
           end_ordinal INTEGER NOT NULL,
           source_hashes_json TEXT NOT NULL,
           FOREIGN KEY(book_id,synthesis_revision) REFERENCES book_syntheses(book_id,revision),
           UNIQUE(book_id,synthesis_revision,ordinal)
         );
         CREATE TABLE IF NOT EXISTS book_relations(
           id TEXT PRIMARY KEY,
           book_id TEXT NOT NULL,
           synthesis_revision INTEGER NOT NULL,
           from_text TEXT NOT NULL,
           to_text TEXT NOT NULL,
           kind TEXT NOT NULL CHECK(kind IN ('explicit','inferred')),
           confidence REAL NOT NULL CHECK(confidence>=0 AND confidence<=1),
           source_hashes_json TEXT NOT NULL,
           FOREIGN KEY(book_id,synthesis_revision) REFERENCES book_syntheses(book_id,revision)
         );",
    )?;
    validate_book_schema(connection)?;
    rebuild_book_fts(connection, root)?;
    let mut statement = connection.prepare(
        "SELECT b.id,MAX(s.revision) FROM books b JOIN book_syntheses s ON s.book_id=b.id
         WHERE b.state='synthesized' GROUP BY b.id ORDER BY b.id",
    )?;
    let synthesized = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (book_id, revision) in synthesized {
        let page = root
            .join("wiki")
            .join("books")
            .join(format!("{book_id}.md"));
        reject_symlink(&page)?;
        if !page.is_file() {
            materialize_book_wiki(connection, root, &book_id, revision)?;
        }
    }
    Ok(())
}

fn validate_book_schema(connection: &Connection) -> Result<()> {
    for (table, expected) in [
        (
            "books",
            &[
                "id",
                "subject_id",
                "title",
                "author",
                "format",
                "original_sha256",
                "original_bytes",
                "normalized_sha256",
                "normalized_bytes",
                "converter",
                "converter_args_json",
                "edition_of",
                "state",
                "revision",
                "created_at",
                "updated_at",
            ][..],
        ),
        (
            "book_blocks",
            &[
                "id",
                "book_id",
                "ordinal",
                "logical_block_id",
                "subordinal",
                "byte_start",
                "byte_end",
                "text_hash",
                "heading_path_json",
            ][..],
        ),
        (
            "book_anomalies",
            &["id", "book_id", "kind", "details", "status", "created_at"],
        ),
        (
            "book_cursors",
            &[
                "book_id",
                "next_ordinal",
                "committed_blocks",
                "revision",
                "updated_at",
            ],
        ),
        (
            "book_leases",
            &[
                "id",
                "book_id",
                "owner",
                "start_ordinal",
                "end_ordinal",
                "range_hash",
                "requested_unit",
                "requested_value",
                "source_limit",
                "used_bytes",
                "used_chars",
                "coverage_committed_before",
                "coverage_total",
                "state",
                "revision",
                "created_at",
                "updated_at",
                "expires_at",
                "committed_at",
            ][..],
        ),
        (
            "book_window_reports",
            &[
                "lease_id",
                "summary",
                "key_points_json",
                "new_concepts_json",
                "prior_links_json",
                "open_threads_json",
                "anomalies_json",
                "created_at",
            ][..],
        ),
        (
            "book_lease_owner_history",
            &[
                "lease_id",
                "revision",
                "old_owner",
                "new_owner",
                "sync_session_id",
                "changed_at",
            ],
        ),
        (
            "book_syntheses",
            &["book_id", "revision", "supersedes_revision", "created_at"],
        ),
        (
            "book_summaries",
            &[
                "id",
                "book_id",
                "synthesis_revision",
                "level",
                "title",
                "summary",
                "start_ordinal",
                "end_ordinal",
                "source_hashes_json",
            ][..],
        ),
        (
            "book_mainline",
            &[
                "id",
                "book_id",
                "synthesis_revision",
                "ordinal",
                "text",
                "start_ordinal",
                "end_ordinal",
                "source_hashes_json",
            ][..],
        ),
        (
            "book_relations",
            &[
                "id",
                "book_id",
                "synthesis_revision",
                "from_text",
                "to_text",
                "kind",
                "confidence",
                "source_hashes_json",
            ][..],
        ),
        ("book_blocks_fts", &["text"]),
    ] {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let actual = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if actual != expected {
            return Err(Error::new(
                "corrupt_store",
                format!("Book table {table} has an invalid schema"),
            ));
        }
    }
    for index in [
        "books_subject",
        "books_edition",
        "book_blocks_order",
        "book_anomalies_book",
        "book_leases_book",
        "book_one_active_lease",
        "book_lease_owner_history_session",
    ] {
        let exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE type='index' AND name=?1",
                [index],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(Error::new(
                "corrupt_store",
                format!("Book index {index} is missing"),
            ));
        }
    }
    let invalid_states = connection.query_row(
        "SELECT
           (SELECT COUNT(*) FROM books WHERE state NOT IN
             ('imported','normalized','structured','indexed','ready','reading','covered','synthesized')) +
           (SELECT COUNT(*) FROM book_anomalies WHERE status NOT IN ('open','accepted','resolved')) +
           (SELECT COUNT(*) FROM book_leases WHERE state NOT IN
             ('active','committed','expired','superseded'))",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if invalid_states != 0 {
        return Err(Error::new(
            "corrupt_store",
            "Book store contains an unsupported future state",
        ));
    }
    Ok(())
}

fn rebuild_book_fts(connection: &Connection, root: &Path) -> Result<()> {
    let block_count = connection.query_row("SELECT COUNT(*) FROM book_blocks", [], |row| {
        row.get::<_, i64>(0)
    })?;
    let indexed_count =
        connection.query_row("SELECT COUNT(*) FROM book_blocks_fts", [], |row| {
            row.get::<_, i64>(0)
        })?;
    if block_count == indexed_count {
        return Ok(());
    }
    let mut statement = connection.prepare(
        "SELECT b.rowid,b.book_id,b.byte_start,b.byte_end,b.text_hash,k.normalized_sha256
         FROM book_blocks b JOIN books k ON k.id=b.book_id
         ORDER BY k.normalized_sha256,b.byte_start,b.rowid",
    )?;
    let records = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let tx = connection.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO book_blocks_fts(book_blocks_fts) VALUES('delete-all')",
        [],
    )?;
    let mut open_hash = String::new();
    let mut open_blob: Option<fs::File> = None;
    for (rowid, book_id, start, end, text_hash, normalized_hash) in records {
        let normalized_hash = normalized_hash.ok_or_else(|| {
            Error::new(
                "corrupt_store",
                format!("book {book_id} has blocks without a normalized blob"),
            )
        })?;
        if normalized_hash != open_hash {
            let path = blob_path(root, &normalized_hash)?;
            reject_symlink(&path)?;
            open_blob = Some(fs::File::open(path)?);
            open_hash = normalized_hash;
        }
        let text = read_block_from_file(open_blob.as_mut().unwrap(), start, end, &text_hash)?;
        tx.execute(
            "INSERT INTO book_blocks_fts(rowid,text) VALUES(?1,?2)",
            params![rowid, text],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn import(store: &mut Store, input: BookImport) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    validate_id(&input.subject_id)?;
    validate_text("title", &input.title, 4096)?;
    if let Some(author) = input.author.as_deref() {
        validate_text("author", author, 4096)?;
    }
    if let Some(id) = input.edition_of.as_deref() {
        validate_id(id)?;
    }
    subject(&store.connection, &input.subject_id)?;
    if let Some(id) = input.edition_of.as_deref() {
        book(&store.connection, id)?;
    }
    let format = book_format(&input.path)?;
    let source = source_path(&input.path)?;
    let staged = stage_original(&store.root, &source)?;
    let fingerprint = fingerprint(&json!({
        "input": &input,
        "original_sha256": staged.sha256,
        "original_bytes": staged.bytes,
    }))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        discard_staged(&staged.path);
        return Ok(value);
    }
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM books WHERE original_sha256=?1",
            [&staged.sha256],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        publish_blob(&store.root, &staged)?;
        let value = envelope(
            Plugin::Book,
            "import",
            json!({"book": book(&tx, &id)?, "deduplicated": true}),
        );
        let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
        tx.commit()?;
        return Ok(value);
    }
    publish_blob(&store.root, &staged)?;
    let id = new_id(Plugin::Book, &input.request_id);
    let timestamp = now(&tx)?;
    tx.execute(
        "INSERT INTO books(
           id,subject_id,title,author,format,original_sha256,original_bytes,
           normalized_sha256,normalized_bytes,edition_of,state,revision,created_at,updated_at
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,'imported',1,?9,?9)",
        params![
            id,
            input.subject_id,
            input.title.trim(),
            input.author.as_deref().map(str::trim),
            format,
            staged.sha256,
            staged.bytes as i64,
            input.edition_of,
            timestamp,
        ],
    )?;
    tx.execute(
        "INSERT INTO book_cursors(book_id,next_ordinal,committed_blocks,revision,updated_at)
         VALUES(?1,0,0,1,?2)",
        params![id, timestamp],
    )?;
    let value = envelope(
        Plugin::Book,
        "import",
        json!({"book": book(&tx, &id)?, "deduplicated": false}),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn prepare(store: &mut Store, id: &str, if_revision: i64, input: BookPrepare) -> Result<Value> {
    validate_id(id)?;
    validate_request_id(&input.request_id)?;
    if if_revision < 1 {
        return Err(Error::new("invalid_input", "if_revision must be positive"));
    }
    let current = book(&store.connection, id)?;
    if current["revision"] != if_revision {
        return Err(Error::new("revision_conflict", "book revision is stale")
            .details(json!({"expected":if_revision,"current":current["revision"]})));
    }
    if current["state"] != "imported" {
        return Err(Error::new(
            "book_not_imported",
            "only imported books can be prepared",
        ));
    }
    let fingerprint = fingerprint(&json!({
        "id": id,
        "if_revision": if_revision,
        "input": &input,
        "source_sha256": current["original_sha256"],
    }))?;
    let source = blob_path(&store.root, current["original_sha256"].as_str().unwrap())?;
    let converter = if matches!(current["format"].as_str(), Some("txt" | "markdown")) {
        None
    } else {
        Some(book_converter_config()?)
    };
    let normalized = match converter.as_ref() {
        Some(config) => normalize_converted(&store.root, &source, config),
        None => normalize_direct(&store.root, &source),
    };
    let normalized = match normalized {
        Ok(normalized) => normalized,
        Err(error) if error.code() == "invalid_book_text" => {
            record_anomaly(
                &mut store.connection,
                id,
                "invalid_utf8",
                "normalized direct text is not valid UTF-8",
            )?;
            return Err(error);
        }
        Err(error) if converter.is_some() => {
            record_anomaly(&mut store.connection, id, "converter_error", error.code())?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let blocks = match blocks(&normalized.path) {
        Ok(blocks) if !blocks.is_empty() => blocks,
        Ok(_) => {
            discard_staged(&normalized.path);
            record_anomaly(
                &mut store.connection,
                id,
                "empty_normalized_text",
                "normalization produced no readable text",
            )?;
            return Err(Error::new(
                "empty_book_text",
                "normalization produced no readable text",
            ));
        }
        Err(error) => {
            discard_staged(&normalized.path);
            return Err(error);
        }
    };
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        discard_staged(&normalized.path);
        return Ok(value);
    }
    let current = book(&tx, id)?;
    if current["revision"] != if_revision || current["state"] != "imported" {
        discard_staged(&normalized.path);
        return Err(Error::new(
            "revision_conflict",
            "book changed during preparation",
        ));
    }
    publish_blob(&store.root, &normalized)?;
    for block in &blocks {
        let block_id = block_id(id, block);
        tx.execute(
            "INSERT INTO book_blocks(
               id,book_id,ordinal,logical_block_id,subordinal,byte_start,byte_end,
               text_hash,heading_path_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                block_id,
                id,
                block.ordinal,
                logical_block_id(id, block),
                block.subordinal,
                block.byte_start,
                block.byte_end,
                block.text_hash,
                serde_json::to_string(&block.heading_path)
                    .map_err(|error| Error::new("json_error", error.to_string()))?,
            ],
        )?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO book_blocks_fts(rowid,text) VALUES(?1,?2)",
            params![rowid, block.text],
        )?;
    }
    let timestamp = now(&tx)?;
    tx.execute(
        "UPDATE books SET normalized_sha256=?2,normalized_bytes=?3,converter=?4,
           converter_args_json=?5,state='ready',revision=?6,updated_at=?7 WHERE id=?1",
        params![
            id,
            normalized.sha256,
            normalized.bytes as i64,
            converter.as_ref().map(|config| config.engine.as_str()),
            converter
                .as_ref()
                .map(|config| json_string(&config.args))
                .transpose()?,
            if_revision + 1,
            timestamp,
        ],
    )?;
    let value = envelope(Plugin::Book, "prepare", json!({"book": book(&tx, id)?}));
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn read_next(store: &mut Store, id: &str, if_revision: i64, input: ReadNext) -> Result<Value> {
    validate_id(id)?;
    validate_request_id(&input.request_id)?;
    validate_text("owner", &input.owner, 256)?;
    validate_lease_seconds(input.lease_seconds)?;
    let (requested_unit, requested_value, source_limit, count_chars) =
        read_budget(input.budget.as_ref())?;
    let fingerprint = fingerprint(&json!({"book_id":id,"if_revision":if_revision,"input":&input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = book(&tx, id)?;
    if current["revision"] != if_revision {
        return Err(Error::new("revision_conflict", "book revision is stale")
            .details(json!({"expected":if_revision,"current":current["revision"]})));
    }
    if !matches!(current["state"].as_str(), Some("ready" | "reading")) {
        return Err(Error::new(
            "book_not_readable",
            "book must be ready or reading before issuing a lease",
        ));
    }
    expire_active_leases(&tx, id)?;
    if let Some(lease_id) = tx
        .query_row(
            "SELECT id FROM book_leases WHERE book_id=?1 AND state='active'",
            [id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let lease = lease(&tx, &store.root, &lease_id)?;
        if lease["owner"] != input.owner {
            return Err(Error::new(
                "stale_owner",
                "another owner holds the active book lease",
            ));
        }
        let value = envelope(Plugin::Book, "read.next", json!({"lease":lease}));
        tx.commit()?;
        return Ok(value);
    }
    let (next, committed_before, total_before) = tx.query_row(
        "SELECT next_ordinal,committed_blocks,
                (SELECT COUNT(*) FROM book_blocks WHERE book_id=book_cursors.book_id)
         FROM book_cursors WHERE book_id=?1",
        [id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    let mut statement = tx.prepare(
        "SELECT id,ordinal,byte_start,byte_end,text_hash FROM book_blocks
         WHERE book_id=?1 AND ordinal>=?2 ORDER BY ordinal",
    )?;
    let normalized_hash = current["normalized_sha256"].as_str().unwrap();
    let mut rows = statement.query(params![id, next])?;
    let mut selected = Vec::new();
    let mut used_bytes = 0_i64;
    let mut used_chars = 0_i64;
    while let Some(row) = rows.next()? {
        let block_id = row.get::<_, String>(0)?;
        let ordinal = row.get::<_, i64>(1)?;
        let byte_start = row.get::<_, i64>(2)?;
        let byte_end = row.get::<_, i64>(3)?;
        let text_hash = row.get::<_, String>(4)?;
        let text = read_block_text(
            &store.root,
            normalized_hash,
            byte_start,
            byte_end,
            &text_hash,
        )?;
        let bytes = text.len() as i64;
        let chars = text.chars().count() as i64;
        let used = if count_chars { used_chars } else { used_bytes };
        let size = if count_chars { chars } else { bytes };
        if used + size > source_limit {
            if selected.is_empty() {
                return Err(Error::new(
                    "read_budget_too_small",
                    "budget cannot hold the next stable source block",
                ));
            }
            break;
        }
        used_bytes += bytes;
        used_chars += chars;
        selected.push((block_id, ordinal, text_hash));
    }
    drop(rows);
    drop(statement);
    if selected.is_empty() {
        return Err(Error::new("book_covered", "book has no unread blocks"));
    }
    let start = selected.first().unwrap().1;
    let end = selected.last().unwrap().1;
    let range_identity = selected
        .iter()
        .map(|(block_id, ordinal, hash)| format!("{block_id}:{ordinal}:{hash}"))
        .collect::<Vec<_>>()
        .join(":");
    let range_hash = hash_bytes(range_identity.as_bytes());
    let lease_id = new_id(Plugin::Book, &input.request_id);
    let timestamp = now(&tx)?;
    tx.execute(
        "INSERT INTO book_leases(
           id,book_id,owner,start_ordinal,end_ordinal,range_hash,requested_unit,
           requested_value,source_limit,used_bytes,used_chars,coverage_committed_before,
           coverage_total,state,revision,
           created_at,updated_at,expires_at,committed_at
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'active',1,
                  ?14,?14,strftime('%Y-%m-%dT%H:%M:%fZ','now',?15),NULL)",
        params![
            lease_id,
            id,
            input.owner,
            start,
            end,
            range_hash,
            requested_unit,
            requested_value,
            source_limit,
            used_bytes,
            used_chars,
            committed_before,
            total_before,
            timestamp,
            format!("+{} seconds", input.lease_seconds),
        ],
    )?;
    tx.execute(
        "UPDATE books SET state='reading',revision=revision+1,updated_at=?2 WHERE id=?1",
        params![id, timestamp],
    )?;
    let value = envelope(
        Plugin::Book,
        "read.next",
        json!({"lease":lease(&tx,&store.root,&lease_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn read_commit(
    store: &mut Store,
    lease_id: &str,
    if_revision: i64,
    input: ReadCommit,
) -> Result<Value> {
    validate_id(lease_id)?;
    validate_request_id(&input.request_id)?;
    validate_text("owner", &input.owner, 256)?;
    validate_text("range_hash", &input.range_hash, 128)?;
    validate_text("summary", &input.summary, 1024 * 1024)?;
    validate_report(&input)?;
    let fingerprint =
        fingerprint(&json!({"lease_id":lease_id,"if_revision":if_revision,"input":&input}))?;
    expire_lease(&mut store.connection, lease_id)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = lease(&tx, &store.root, lease_id)?;
    if current["state"] == "expired" {
        return Err(Error::new(
            "lease_expired",
            "expired book leases cannot advance coverage",
        ));
    }
    if current["revision"] != if_revision || current["state"] != "active" {
        return Err(Error::new(
            "revision_conflict",
            "book lease revision is stale",
        ));
    }
    if current["owner"] != input.owner {
        return Err(Error::new(
            "stale_owner",
            "only the current lease owner may commit",
        ));
    }
    if current["range_hash"] != input.range_hash {
        return Err(Error::new(
            "lease_hash_mismatch",
            "lease range hash does not match",
        ));
    }
    validate_report_sources(&current, &input)?;
    let book_id = current["book_id"].as_str().unwrap();
    let start = current["start_ordinal"].as_i64().unwrap();
    let end = current["end_ordinal"].as_i64().unwrap();
    let (next, committed) = tx.query_row(
        "SELECT next_ordinal,committed_blocks FROM book_cursors WHERE book_id=?1",
        [book_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    if next != start {
        return Err(Error::new(
            "coverage_order_conflict",
            "lease no longer starts at the next unread block",
        ));
    }
    let timestamp = now(&tx)?;
    let changed = tx.execute(
        "UPDATE book_leases
         SET state='committed',revision=revision+1,updated_at=?2,committed_at=?2
         WHERE id=?1 AND owner=?3 AND state='active' AND revision=?4 AND range_hash=?5",
        params![
            lease_id,
            timestamp,
            input.owner,
            if_revision,
            input.range_hash,
        ],
    )?;
    if changed != 1 {
        return Err(Error::new(
            "revision_conflict",
            "book lease changed during commit",
        ));
    }
    tx.execute(
        "INSERT INTO book_window_reports(
           lease_id,summary,key_points_json,new_concepts_json,prior_links_json,
           open_threads_json,anomalies_json,created_at
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            lease_id,
            input.summary,
            json_string(&input.key_points)?,
            json_string(&input.new_concepts)?,
            json_string(&input.prior_links)?,
            json_string(&input.open_threads)?,
            json_string(&input.anomalies)?,
            timestamp,
        ],
    )?;
    let added = end - start + 1;
    let total = tx.query_row(
        "SELECT COUNT(*) FROM book_blocks WHERE book_id=?1",
        [book_id],
        |row| row.get::<_, i64>(0),
    )?;
    let committed = committed + added;
    tx.execute(
        "UPDATE book_cursors SET next_ordinal=?2,committed_blocks=?3,revision=revision+1,
           updated_at=?4 WHERE book_id=?1",
        params![book_id, end + 1, committed, timestamp],
    )?;
    tx.execute(
        "UPDATE books SET state=?2,revision=revision+1,updated_at=?3 WHERE id=?1",
        params![
            book_id,
            if committed == total {
                "covered"
            } else {
                "reading"
            },
            timestamp,
        ],
    )?;
    let value = envelope(
        Plugin::Book,
        "read.commit",
        json!({"lease":lease(&tx,&store.root,lease_id)?,"coverage":coverage(&tx,book_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn read_renew(
    store: &mut Store,
    lease_id: &str,
    if_revision: i64,
    input: ReadRenew,
) -> Result<Value> {
    validate_id(lease_id)?;
    validate_request_id(&input.request_id)?;
    validate_text("owner", &input.owner, 256)?;
    validate_lease_seconds(input.lease_seconds)?;
    let fingerprint =
        fingerprint(&json!({"lease_id":lease_id,"if_revision":if_revision,"input":&input}))?;
    expire_lease(&mut store.connection, lease_id)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = lease(&tx, &store.root, lease_id)?;
    if current["state"] == "expired" {
        return Err(Error::new(
            "lease_expired",
            "expired book leases cannot be renewed",
        ));
    }
    if current["state"] != "active" || current["revision"] != if_revision {
        return Err(Error::new(
            "revision_conflict",
            "book lease revision is stale",
        ));
    }
    if current["owner"] != input.owner {
        return Err(Error::new(
            "stale_owner",
            "only the current lease owner may renew",
        ));
    }
    let timestamp = now(&tx)?;
    tx.execute(
        "UPDATE book_leases SET revision=revision+1,updated_at=?2,
         expires_at=strftime('%Y-%m-%dT%H:%M:%fZ','now',?3) WHERE id=?1",
        params![
            lease_id,
            timestamp,
            format!("+{} seconds", input.lease_seconds),
        ],
    )?;
    let value = envelope(
        Plugin::Book,
        "read.renew",
        json!({"lease":lease(&tx,&store.root,lease_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn read_takeover(store: &mut Store, input: ReadTakeover) -> Result<Value> {
    validate_id(&input.entity_id)?;
    validate_text("old_owner", &input.old_owner, 256)?;
    validate_text("new_owner", &input.new_owner, 256)?;
    validate_request_id(&input.sync_session_id)?;
    validate_request_id(&input.request_id)?;
    if input.if_revision < 1 || input.old_owner == input.new_owner {
        return Err(Error::new(
            "invalid_takeover",
            "takeover requires a positive revision and a different new owner",
        ));
    }
    let fingerprint = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    require_latest_sync_receipt(&tx, Plugin::Book, &input.sync_session_id)?;
    let timestamp = now(&tx)?;
    let changed = tx.execute(
        "UPDATE book_leases SET owner=?2,state='active',revision=revision+1,updated_at=?3,
           expires_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','+900 seconds')
         WHERE id=?1 AND owner=?4 AND revision=?5 AND state IN ('active','expired')",
        params![
            input.entity_id,
            input.new_owner,
            timestamp,
            input.old_owner,
            input.if_revision,
        ],
    )?;
    if changed != 1 {
        let current = lease(&tx, &store.root, &input.entity_id)?;
        if current["owner"] != input.old_owner {
            return Err(Error::new("stale_owner", "book lease owner changed"));
        }
        return Err(Error::new(
            "revision_conflict",
            "book lease revision or state changed",
        ));
    }
    tx.execute(
        "INSERT INTO book_lease_owner_history(
           lease_id,revision,old_owner,new_owner,sync_session_id,changed_at
         ) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            input.entity_id,
            input.if_revision + 1,
            input.old_owner,
            input.new_owner,
            input.sync_session_id,
            timestamp,
        ],
    )?;
    let value = envelope(
        Plugin::Book,
        "read.takeover",
        json!({"lease":lease(&tx,&store.root,&input.entity_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    Ok(value)
}

fn publish_synthesis(
    store: &mut Store,
    book_id: &str,
    if_revision: i64,
    input: SynthesisPublish,
) -> Result<Value> {
    validate_id(book_id)?;
    validate_request_id(&input.request_id)?;
    let fingerprint =
        fingerprint(&json!({"book_id":book_id,"if_revision":if_revision,"input":&input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = book(&tx, book_id)?;
    if current["revision"] != if_revision {
        return Err(Error::new("revision_conflict", "book revision is stale"));
    }
    let latest_synthesis = tx.query_row(
        "SELECT MAX(revision) FROM book_syntheses WHERE book_id=?1",
        [book_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let correction = current["state"] == "synthesized";
    if current["state"] != "covered" && !correction {
        return Err(Error::new(
            "book_not_covered",
            "book must have 100% committed ordered coverage before synthesis",
        ));
    }
    match (correction, input.supersedes_revision, latest_synthesis) {
        (false, None, None) => {}
        (true, Some(supersedes), Some(latest)) if supersedes == latest => {}
        (true, _, _) => {
            return Err(Error::new(
                "revision_conflict",
                "synthesis correction must supersede the latest synthesis revision",
            ));
        }
        _ => {
            return Err(Error::new(
                "invalid_synthesis",
                "initial synthesis cannot supersede another revision",
            ));
        }
    }
    if input.summaries.is_empty() || input.mainline.is_empty() || input.relations.is_empty() {
        return Err(Error::new(
            "incomplete_synthesis",
            "synthesis requires summaries, mainline, and relations",
        ));
    }
    for disposition in &input.anomaly_dispositions {
        validate_id(&disposition.id)?;
        if !matches!(disposition.status.as_str(), "accepted" | "resolved") {
            return Err(Error::new(
                "invalid_anomaly_disposition",
                "anomaly disposition must be accepted or resolved",
            ));
        }
        if tx.execute(
            "UPDATE book_anomalies SET status=?3 WHERE id=?1 AND book_id=?2 AND status='open'",
            params![disposition.id, book_id, disposition.status],
        )? != 1
        {
            return Err(Error::new(
                "invalid_anomaly_disposition",
                "anomaly is not an open anomaly for this book",
            ));
        }
    }
    let open_anomalies = tx.query_row(
        "SELECT COUNT(*) FROM book_anomalies WHERE book_id=?1 AND status='open'",
        [book_id],
        |row| row.get::<_, i64>(0),
    )?;
    if open_anomalies != 0 {
        return Err(Error::new(
            "unresolved_book_anomalies",
            "all conversion anomalies must be resolved or accepted",
        ));
    }
    let total = current["block_count"].as_i64().unwrap();
    let required_headings = required_heading_summaries(&tx, book_id)?;
    if !input.summaries.iter().any(|summary| {
        summary.level == "book" && summary.start_ordinal == 0 && summary.end_ordinal == total - 1
    }) || required_headings.iter().any(|required| {
        required.level == "chapter"
            && !input.summaries.iter().any(|summary| {
                summary.level == required.level
                    && summary.title == required.title
                    && summary.start_ordinal == required.start
                    && summary.end_ordinal == required.end
            })
    }) {
        return Err(Error::new(
            "incomplete_synthesis",
            "synthesis requires complete chapter and book summaries",
        ));
    }
    for required in required_headings {
        if !input.summaries.iter().any(|summary| {
            summary.level == required.level
                && summary.title == required.title
                && summary.start_ordinal == required.start
                && summary.end_ordinal == required.end
        }) {
            return Err(Error::new(
                "incomplete_synthesis",
                format!(
                    "missing {} summary for {} at {}..={}",
                    required.level, required.title, required.start, required.end
                ),
            ));
        }
    }
    for summary in &input.summaries {
        if !matches!(
            summary.level.as_str(),
            "section" | "chapter" | "part" | "book"
        ) {
            return Err(Error::new("invalid_synthesis", "invalid summary level"));
        }
        validate_text("summary title", &summary.title, 16 * 1024)?;
        validate_text("summary", &summary.summary, 1024 * 1024)?;
        validate_span_sources(
            &tx,
            book_id,
            summary.start_ordinal,
            summary.end_ordinal,
            &summary.source_hashes,
        )?;
    }
    for node in &input.mainline {
        validate_text("mainline", &node.text, 1024 * 1024)?;
        validate_span_sources(
            &tx,
            book_id,
            node.start_ordinal,
            node.end_ordinal,
            &node.source_hashes,
        )?;
    }
    let mut next_mainline_ordinal = 0;
    for node in &input.mainline {
        if node.start_ordinal != next_mainline_ordinal {
            return Err(Error::new(
                "incomplete_synthesis",
                "mainline must cover the whole book once in source order",
            ));
        }
        next_mainline_ordinal = node.end_ordinal + 1;
    }
    if next_mainline_ordinal != total {
        return Err(Error::new(
            "incomplete_synthesis",
            "mainline must cover the whole book once in source order",
        ));
    }
    for relation in &input.relations {
        validate_text("relation from", &relation.from, 64 * 1024)?;
        validate_text("relation to", &relation.to, 64 * 1024)?;
        if !matches!(relation.kind.as_str(), "explicit" | "inferred")
            || !relation.confidence.is_finite()
            || !(0.0..=1.0).contains(&relation.confidence)
        {
            return Err(Error::new("invalid_synthesis", "invalid relation"));
        }
        validate_book_hashes(&tx, book_id, &relation.source_hashes)?;
    }
    let synthesis_revision = tx.query_row(
        "SELECT COALESCE(MAX(revision),0)+1 FROM book_syntheses WHERE book_id=?1",
        [book_id],
        |row| row.get::<_, i64>(0),
    )?;
    let timestamp = now(&tx)?;
    tx.execute(
        "INSERT INTO book_syntheses(book_id,revision,supersedes_revision,created_at)
         VALUES(?1,?2,?3,?4)",
        params![
            book_id,
            synthesis_revision,
            input.supersedes_revision,
            timestamp
        ],
    )?;
    for (ordinal, summary) in input.summaries.iter().enumerate() {
        tx.execute(
            "INSERT INTO book_summaries(
               id,book_id,synthesis_revision,level,title,summary,start_ordinal,end_ordinal,
               source_hashes_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                new_id(
                    Plugin::Book,
                    &format!("{}:summary:{ordinal}", input.request_id)
                ),
                book_id,
                synthesis_revision,
                summary.level,
                summary.title,
                summary.summary,
                summary.start_ordinal,
                summary.end_ordinal,
                json_string(&summary.source_hashes)?,
            ],
        )?;
    }
    for (ordinal, node) in input.mainline.iter().enumerate() {
        tx.execute(
            "INSERT INTO book_mainline(
               id,book_id,synthesis_revision,ordinal,text,start_ordinal,end_ordinal,
               source_hashes_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                new_id(
                    Plugin::Book,
                    &format!("{}:mainline:{ordinal}", input.request_id)
                ),
                book_id,
                synthesis_revision,
                ordinal as i64,
                node.text,
                node.start_ordinal,
                node.end_ordinal,
                json_string(&node.source_hashes)?,
            ],
        )?;
    }
    for (ordinal, relation) in input.relations.iter().enumerate() {
        tx.execute(
            "INSERT INTO book_relations(
               id,book_id,synthesis_revision,from_text,to_text,kind,confidence,source_hashes_json
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                new_id(
                    Plugin::Book,
                    &format!("{}:relation:{ordinal}", input.request_id)
                ),
                book_id,
                synthesis_revision,
                relation.from,
                relation.to,
                relation.kind,
                relation.confidence,
                json_string(&relation.source_hashes)?,
            ],
        )?;
    }
    tx.execute(
        "UPDATE books SET state='synthesized',revision=revision+1,updated_at=?2 WHERE id=?1",
        params![book_id, timestamp],
    )?;
    let value = envelope(
        Plugin::Book,
        "synthesis.publish",
        json!({
            "book":book(&tx,book_id)?,
            "synthesis_revision":synthesis_revision,
            "supersedes_revision":input.supersedes_revision,
        }),
    );
    let value = finalize_mutation(&tx, Plugin::Book, &input.request_id, &fingerprint, value)?;
    tx.commit()?;
    if let Err(error) =
        materialize_book_wiki(&store.connection, &store.root, book_id, synthesis_revision)
    {
        return Err(Error::new(
            "book_materialization_pending",
            "synthesis committed but its private Wiki projection still needs materialization",
        )
        .details(json!({
            "committed": true,
            "retryable": true,
            "book_id": book_id,
            "synthesis_revision": synthesis_revision,
            "cause": error.code(),
        })));
    }
    Ok(value)
}

struct RequiredSummary {
    level: &'static str,
    title: String,
    start: i64,
    end: i64,
}

fn required_heading_summaries(
    connection: &Connection,
    book_id: &str,
) -> Result<Vec<RequiredSummary>> {
    let mut statement = connection.prepare(
        "SELECT ordinal,heading_path_json FROM book_blocks WHERE book_id=?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([book_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows = rows
        .into_iter()
        .map(|(ordinal, raw)| {
            serde_json::from_str::<Vec<String>>(&raw)
                .map(|path| (ordinal, path))
                .map_err(|error| Error::new("corrupt_store", error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    let max_depth = rows.iter().map(|(_, path)| path.len()).max().unwrap_or(0);
    let mut required: Vec<RequiredSummary> = Vec::new();
    for depth in 1..=max_depth {
        let level = match (max_depth, depth) {
            (1 | 2, 1) => "chapter",
            (2, 2) => "section",
            (_, 1) => "part",
            (_, 2) => "chapter",
            _ => "section",
        };
        for (ordinal, path) in &rows {
            if path.len() < depth {
                continue;
            }
            let key = path[..depth].join(" / ");
            if let Some(last) = required.last_mut() {
                if last.level == level && last.title == key && last.end + 1 == *ordinal {
                    last.end = *ordinal;
                    continue;
                }
            }
            required.push(RequiredSummary {
                level,
                title: key,
                start: *ordinal,
                end: *ordinal,
            });
        }
    }
    Ok(required)
}

fn validate_span_sources(
    connection: &Connection,
    book_id: &str,
    start: i64,
    end: i64,
    hashes: &[String],
) -> Result<()> {
    if start < 0 || end < start {
        return Err(Error::new("invalid_synthesis", "invalid source span"));
    }
    let mut statement = connection.prepare(
        "SELECT text_hash FROM book_blocks WHERE book_id=?1 AND ordinal BETWEEN ?2 AND ?3
         ORDER BY ordinal",
    )?;
    let expected = statement
        .query_map(params![book_id, start, end], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if expected.is_empty() || expected != hashes {
        return Err(Error::new(
            "synthesis_source_mismatch",
            "synthesis span must cite every exact source hash in order",
        ));
    }
    Ok(())
}

fn validate_book_hashes(connection: &Connection, book_id: &str, hashes: &[String]) -> Result<()> {
    if hashes.is_empty() {
        return Err(Error::new(
            "synthesis_source_mismatch",
            "relation requires source hashes",
        ));
    }
    for hash in hashes {
        let exists = connection
            .query_row(
                "SELECT 1 FROM book_blocks WHERE book_id=?1 AND text_hash=?2",
                params![book_id, hash],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(Error::new(
                "synthesis_source_mismatch",
                "relation source hash does not belong to the book",
            ));
        }
    }
    Ok(())
}

fn materialize_book_wiki(
    connection: &Connection,
    root: &Path,
    book_id: &str,
    revision: i64,
) -> Result<()> {
    let current = book(connection, book_id)?;
    let mut body = format!(
        "# {}\n\n- Book ID: `{book_id}`\n- Source: `{}`\n- Synthesis revision: {revision}\n\n## Summaries\n",
        current["title"].as_str().unwrap(),
        current["normalized_sha256"].as_str().unwrap()
    );
    let mut statement = connection.prepare(
        "SELECT level,title,summary,start_ordinal,end_ordinal FROM book_summaries
         WHERE book_id=?1 AND synthesis_revision=?2 ORDER BY start_ordinal,end_ordinal,level",
    )?;
    let summaries = statement
        .query_map(params![book_id, revision], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (level, title, summary, start, end) in summaries {
        body.push_str(&format!(
            "\n### {title}\n\n{summary}\n\n`{level}` blocks {start}..{end}\n"
        ));
    }
    body.push_str("\n## Mainline\n");
    let mut statement = connection.prepare(
        "SELECT text FROM book_mainline WHERE book_id=?1 AND synthesis_revision=?2 ORDER BY ordinal",
    )?;
    for text in statement
        .query_map(params![book_id, revision], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
    {
        body.push_str(&format!("\n- {text}"));
    }
    body.push_str("\n\n## Relations\n");
    let mut statement = connection.prepare(
        "SELECT from_text,to_text,kind,confidence FROM book_relations
         WHERE book_id=?1 AND synthesis_revision=?2 ORDER BY id",
    )?;
    for (from, to, kind, confidence) in statement
        .query_map(params![book_id, revision], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    {
        body.push_str(&format!("\n- {from} -> {to} ({kind}, {confidence:.2})"));
    }
    body.push('\n');
    let wiki = root.join("wiki");
    let books = wiki.join("books");
    for path in [&wiki, &books] {
        reject_symlink(path)?;
        fs::create_dir(path).or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
    }
    let destination = books.join(format!("{book_id}.md"));
    let temporary = books.join(format!(
        ".book-wiki-{}-{}.tmp",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    publish_wiki_file(&temporary, &destination, body.as_bytes())?;
    Ok(())
}

fn publish_wiki_file(temporary: &Path, destination: &Path, body: &[u8]) -> Result<()> {
    reject_symlink(temporary)?;
    reject_symlink(destination)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(temporary)?;
    output.write_all(body)?;
    output.sync_all()?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn search(
    connection: &Connection,
    root: &Path,
    id: &str,
    query: &str,
    limit: usize,
) -> Result<Value> {
    validate_id(id)?;
    let normalized_hash = normalized_book_hash(connection, id)?;
    validate_text("query", query, 4096)?;
    if !(1..=100).contains(&limit) {
        return Err(Error::new("invalid_input", "limit must be within 1..=100"));
    }
    let phrase = format!("\"{}\"", query.replace('"', "\"\""));
    let mut statement = connection.prepare(
        "SELECT b.id,b.ordinal,b.logical_block_id,b.subordinal,b.byte_start,b.byte_end,
                b.text_hash,b.heading_path_json
         FROM book_blocks_fts f JOIN book_blocks b ON b.rowid=f.rowid
         WHERE book_blocks_fts MATCH ?1 AND b.book_id=?2
         ORDER BY bm25(book_blocks_fts),b.ordinal LIMIT ?3",
    )?;
    let records = statement
        .query_map(params![phrase, id, limit as i64], stored_block_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let hits = records
        .iter()
        .map(|record| block_value(root, &normalized_hash, id, record, true))
        .collect::<Result<Vec<_>>>()?;
    Ok(envelope(Plugin::Book, "search", json!({"hits": hits})))
}

fn peek(connection: &Connection, root: &Path, id: &str, start: i64, count: usize) -> Result<Value> {
    validate_id(id)?;
    let normalized_hash = normalized_book_hash(connection, id)?;
    if start < 0 || !(1..=100).contains(&count) {
        return Err(Error::new(
            "invalid_input",
            "peek requires start >= 0 and count within 1..=100",
        ));
    }
    let mut statement = connection.prepare(
        "SELECT id,ordinal,logical_block_id,subordinal,byte_start,byte_end,text_hash,
                heading_path_json
         FROM book_blocks WHERE book_id=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3",
    )?;
    let records = statement
        .query_map(params![id, start, count as i64], stored_block_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let blocks = records
        .iter()
        .map(|record| block_value(root, &normalized_hash, id, record, false))
        .collect::<Result<Vec<_>>>()?;
    Ok(envelope(Plugin::Book, "peek", json!({"blocks":blocks})))
}

fn read_budget(budget: Option<&ReadBudget>) -> Result<(String, i64, i64, bool)> {
    match budget {
        None => Ok(("fallback_utf8_bytes".to_owned(), 65_536, 65_536, false)),
        Some(budget)
            if budget.unit == "utf8_bytes" && (32_768..=16_777_216).contains(&budget.value) =>
        {
            Ok((budget.unit.clone(), budget.value, budget.value, false))
        }
        Some(budget) if budget.unit == "tokens" && (1024..=4_000_000).contains(&budget.value) => {
            Ok((
                budget.unit.clone(),
                budget.value,
                budget.value.saturating_mul(55) / 100,
                true,
            ))
        }
        _ => Err(Error::new(
            "invalid_read_budget",
            "budget must be utf8_bytes 32768..=16777216 or tokens 1024..=4000000",
        )),
    }
}

fn default_lease_seconds() -> i64 {
    15 * 60
}

fn validate_lease_seconds(seconds: i64) -> Result<()> {
    if (30..=24 * 60 * 60).contains(&seconds) {
        Ok(())
    } else {
        Err(Error::new(
            "invalid_lease_expiry",
            "lease_seconds must be within 30..=86400",
        ))
    }
}

fn expire_active_leases(connection: &Connection, book_id: &str) -> Result<()> {
    connection.execute(
        "UPDATE book_leases SET state='expired',revision=revision+1,
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE book_id=?1 AND state='active'
           AND expires_at<=strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [book_id],
    )?;
    Ok(())
}

fn expire_lease(connection: &mut Connection, lease_id: &str) -> Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE book_leases SET state='expired',revision=revision+1,
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id=?1 AND state='active'
           AND expires_at<=strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [lease_id],
    )?;
    if changed != 0 {
        bump_store_revision(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

fn lease(connection: &Connection, root: &Path, id: &str) -> Result<Value> {
    validate_id(id)?;
    let mut value = connection
        .query_row(
            "SELECT book_id,owner,start_ordinal,end_ordinal,range_hash,requested_unit,
                    requested_value,source_limit,used_bytes,used_chars,
                    coverage_committed_before,coverage_total,state,revision,
                    created_at,updated_at,expires_at,committed_at
             FROM book_leases WHERE id=?1",
            [id],
            |row| {
                let committed_before: i64 = row.get(10)?;
                let total: i64 = row.get(11)?;
                let percent = if total == 0 {
                    0.0
                } else {
                    committed_before as f64 * 100.0 / total as f64
                };
                Ok(json!({
                    "id":id,
                    "book_id":row.get::<_,String>(0)?,
                    "owner":row.get::<_,String>(1)?,
                    "start_ordinal":row.get::<_,i64>(2)?,
                    "end_ordinal":row.get::<_,i64>(3)?,
                    "range_hash":row.get::<_,String>(4)?,
                    "budget":{
                        "requested_unit":row.get::<_,String>(5)?,
                        "requested_value":row.get::<_,i64>(6)?,
                        "source_limit":row.get::<_,i64>(7)?,
                        "used_bytes":row.get::<_,i64>(8)?,
                        "used_chars":row.get::<_,i64>(9)?,
                    },
                    "coverage_before":{
                        "committed_blocks":committed_before,
                        "total_blocks":total,
                        "percent":percent,
                    },
                    "state":row.get::<_,String>(12)?,
                    "revision":row.get::<_,i64>(13)?,
                    "created_at":row.get::<_,String>(14)?,
                    "updated_at":row.get::<_,String>(15)?,
                    "expires_at":row.get::<_,String>(16)?,
                    "committed_at":row.get::<_,Option<String>>(17)?,
                }))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("book_lease_not_found", format!("lease {id} was not found")))?;
    let book_id = value["book_id"].as_str().unwrap().to_owned();
    let normalized_hash = normalized_book_hash(connection, &book_id)?;
    let start = value["start_ordinal"].as_i64().unwrap();
    let end = value["end_ordinal"].as_i64().unwrap();
    let mut statement = connection.prepare(
        "SELECT id,ordinal,logical_block_id,subordinal,byte_start,byte_end,text_hash,
                heading_path_json
         FROM book_blocks WHERE book_id=?1 AND ordinal BETWEEN ?2 AND ?3 ORDER BY ordinal",
    )?;
    let records = statement
        .query_map(params![book_id, start, end], stored_block_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let blocks = records
        .iter()
        .map(|record| block_value(root, &normalized_hash, &book_id, record, false))
        .collect::<Result<Vec<_>>>()?;
    let text = blocks
        .iter()
        .map(|block| block["text"].as_str().unwrap())
        .collect::<String>();
    let object = value.as_object_mut().unwrap();
    object.insert("blocks".to_owned(), json!(blocks));
    object.insert("text".to_owned(), json!(text));
    Ok(value)
}

fn coverage(connection: &Connection, book_id: &str) -> Result<Value> {
    let (committed, total) = connection.query_row(
        "SELECT c.committed_blocks,(SELECT COUNT(*) FROM book_blocks WHERE book_id=c.book_id)
         FROM book_cursors c WHERE c.book_id=?1",
        [book_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let percent = if total == 0 {
        0.0
    } else {
        committed as f64 * 100.0 / total as f64
    };
    Ok(json!({"committed_blocks":committed,"total_blocks":total,"percent":percent}))
}

fn validate_report(input: &ReadCommit) -> Result<()> {
    if input.key_points.is_empty() || input.key_points.len() > 1024 {
        return Err(Error::new(
            "invalid_read_report",
            "reading report requires 1..=1024 key points",
        ));
    }
    for point in &input.key_points {
        validate_text("key point", &point.text, 64 * 1024)?;
        validate_id(&point.block_id)?;
        validate_text("source_hash", &point.source_hash, 128)?;
    }
    for values in [
        &input.new_concepts,
        &input.prior_links,
        &input.open_threads,
        &input.anomalies,
    ] {
        if values.len() > 1024 {
            return Err(Error::new(
                "invalid_read_report",
                "report list is too large",
            ));
        }
        for value in values {
            validate_text("report item", value, 64 * 1024)?;
        }
    }
    Ok(())
}

fn validate_report_sources(lease: &Value, input: &ReadCommit) -> Result<()> {
    for point in &input.key_points {
        let matches =
            lease["blocks"].as_array().unwrap().iter().any(|block| {
                block["id"] == point.block_id && block["text_hash"] == point.source_hash
            });
        if !matches {
            return Err(Error::new(
                "report_source_mismatch",
                "key point must cite an exact block and source hash from this lease",
            ));
        }
    }
    Ok(())
}

fn json_string(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::new("json_error", error.to_string()))
}

fn record_anomaly(
    connection: &mut Connection,
    book_id: &str,
    kind: &str,
    details: &str,
) -> Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let timestamp = now(&tx)?;
    let changed = tx.execute(
        "INSERT OR IGNORE INTO book_anomalies(id,book_id,kind,details,status,created_at)
         VALUES(?1,?2,?3,?4,'open',?5)",
        params![
            new_id(Plugin::Book, &format!("{book_id}:{kind}:{details}")),
            book_id,
            kind,
            details,
            timestamp,
        ],
    )?;
    if changed != 0 {
        bump_store_revision(&tx)?;
    }
    tx.commit()?;
    Ok(())
}

fn blob_path(root: &Path, hash: &str) -> Result<PathBuf> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::new(
            "corrupt_store",
            "book blob hash is not a lowercase SHA-256 digest",
        ));
    }
    Ok(root
        .join("blobs")
        .join("sha256")
        .join(&hash[..2])
        .join(hash))
}

struct StoredBlock {
    id: String,
    ordinal: i64,
    logical_block_id: String,
    subordinal: i64,
    byte_start: i64,
    byte_end: i64,
    text_hash: String,
    heading_path_json: String,
}

fn stored_block_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredBlock> {
    Ok(StoredBlock {
        id: row.get(0)?,
        ordinal: row.get(1)?,
        logical_block_id: row.get(2)?,
        subordinal: row.get(3)?,
        byte_start: row.get(4)?,
        byte_end: row.get(5)?,
        text_hash: row.get(6)?,
        heading_path_json: row.get(7)?,
    })
}

fn normalized_book_hash(connection: &Connection, id: &str) -> Result<String> {
    validate_id(id)?;
    connection
        .query_row(
            "SELECT normalized_sha256 FROM books WHERE id=?1",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .ok_or_else(|| {
            Error::new(
                "book_not_prepared",
                format!("book {id} has no normalized source blob"),
            )
        })
}

fn read_block_text(
    root: &Path,
    normalized_hash: &str,
    byte_start: i64,
    byte_end: i64,
    expected_hash: &str,
) -> Result<String> {
    let path = blob_path(root, normalized_hash)?;
    reject_symlink(&path)?;
    let mut file = fs::File::open(&path)?;
    read_block_from_file(&mut file, byte_start, byte_end, expected_hash)
}

fn read_block_from_file(
    file: &mut fs::File,
    byte_start: i64,
    byte_end: i64,
    expected_hash: &str,
) -> Result<String> {
    if byte_start < 0 || byte_end <= byte_start || byte_end as u64 > MAX_BOOK_BYTES {
        return Err(Error::new(
            "corrupt_store",
            "book block has invalid byte offsets",
        ));
    }
    let length = (byte_end - byte_start) as usize;
    if file.metadata()?.len() < byte_end as u64 {
        return Err(Error::new(
            "corrupt_blob",
            "normalized book blob is shorter than its block metadata",
        ));
    }
    file.seek(SeekFrom::Start(byte_start as u64))?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)?;
    if hash_bytes(&bytes) != expected_hash {
        return Err(Error::new(
            "corrupt_blob",
            "normalized book block hash does not match its metadata",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| Error::new("corrupt_blob", "normalized book block is not UTF-8"))
}

fn block_value(
    root: &Path,
    normalized_hash: &str,
    book_id: &str,
    block: &StoredBlock,
    search_style: bool,
) -> Result<Value> {
    let heading_path: Value = serde_json::from_str(&block.heading_path_json)
        .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
    let text = read_block_text(
        root,
        normalized_hash,
        block.byte_start,
        block.byte_end,
        &block.text_hash,
    )?;
    let locator = json!({
        "book_id": book_id,
        "logical_block_id": block.logical_block_id,
        "subordinal": block.subordinal,
        "byte_start": block.byte_start,
        "byte_end": block.byte_end,
        "source_hash": block.text_hash,
    });
    if search_style {
        Ok(json!({
            "block_id": block.id,
            "book_id": book_id,
            "ordinal": block.ordinal,
            "locator": {
                "book_id": book_id,
                "logical_block_id": block.logical_block_id,
                "subordinal": block.subordinal,
                "byte_start": block.byte_start,
                "byte_end": block.byte_end,
                "heading_path": heading_path,
                "source_hash": block.text_hash,
            },
            "text": text,
            "text_hash": block.text_hash,
            "query_only": true,
        }))
    } else {
        Ok(json!({
            "id": block.id,
            "book_id": book_id,
            "ordinal": block.ordinal,
            "byte_start": block.byte_start,
            "byte_end": block.byte_end,
            "text": text,
            "text_hash": block.text_hash,
            "heading_path": heading_path,
            "locator": locator,
            "query_only": true,
        }))
    }
}

fn book_converter_config() -> Result<crate::trans_adapter::Config> {
    let raw = std::env::var("LWC_BOOK_TRANS_CONFIG").map_err(|_| {
        Error::new(
            "book_converter_required",
            "EPUB and text PDF preparation require a configured converter adapter",
        )
    })?;
    if raw.len() > 64 * 1024 {
        return Err(Error::new(
            "invalid_converter_config",
            "Book converter configuration exceeds 64 KiB",
        ));
    }
    serde_json::from_str(&raw)
        .map_err(|error| Error::new("invalid_converter_config", error.to_string()))
}

fn normalize_converted(
    root: &Path,
    source: &Path,
    config: &crate::trans_adapter::Config,
) -> Result<StagedBlob> {
    let path = root.join(format!(
        ".normalized-{}-{}.tmp",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    reject_symlink(&path)?;
    if let Err(error) = crate::trans_adapter::run(config, source, &path) {
        discard_staged(&path);
        return Err(Error::new(error.code, error.message).details(error.details));
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        discard_staged(&path);
        return Err(Error::new(
            "trans_unsafe_output",
            "converter output is not a regular file",
        ));
    }
    if metadata.len() == 0 {
        discard_staged(&path);
        return Err(Error::new(
            "trans_empty_output",
            "converter output is empty",
        ));
    }
    if metadata.len() > MAX_BOOK_BYTES {
        discard_staged(&path);
        return Err(Error::new(
            "trans_output_too_large",
            "converter output exceeds the Book size limit",
        ));
    }
    let mut file = fs::File::open(&path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if std::str::from_utf8(&buffer[..read]).is_err() {
            let bytes = fs::read(&path)?;
            if std::str::from_utf8(&bytes).is_err() {
                discard_staged(&path);
                return Err(Error::new(
                    "trans_invalid_utf8",
                    "converter output is not valid UTF-8",
                ));
            }
        }
        hasher.update(&buffer[..read]);
    }
    Ok(StagedBlob {
        path,
        sha256: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes: metadata.len(),
    })
}

fn normalize_direct(root: &Path, source: &Path) -> Result<StagedBlob> {
    let path = root.join(format!(
        ".normalized-{}-{}.tmp",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    reject_symlink(&path)?;
    let mut input = fs::File::open(source)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut first = true;
    let mut pending_cr = false;
    let mut bytes = 0_u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let mut start = 0;
        if first {
            first = false;
            if buffer[..read].starts_with(&[0xef, 0xbb, 0xbf]) {
                start = 3;
            }
        }
        let mut normalized = Vec::with_capacity(read - start + 1);
        for &byte in &buffer[start..read] {
            if pending_cr {
                normalized.push(b'\n');
                pending_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' {
                pending_cr = true;
            } else {
                normalized.push(byte);
            }
        }
        bytes += normalized.len() as u64;
        hasher.update(&normalized);
        output.write_all(&normalized)?;
    }
    if pending_cr {
        bytes += 1;
        hasher.update(b"\n");
        output.write_all(b"\n")?;
    }
    output.sync_all()?;
    drop(output);
    let mut reader = BufReader::new(fs::File::open(&path)?);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                discard_staged(&path);
                return Err(Error::new(
                    "invalid_book_text",
                    "normalized direct text is not valid UTF-8",
                ));
            }
            Err(error) => {
                discard_staged(&path);
                return Err(error.into());
            }
        }
    }
    Ok(StagedBlob {
        path,
        sha256: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes,
    })
}

struct Block {
    ordinal: i64,
    logical_ordinal: i64,
    subordinal: i64,
    logical_hash: String,
    byte_start: i64,
    byte_end: i64,
    text: String,
    text_hash: String,
    heading_path: Vec<String>,
}

fn blocks(path: &Path) -> Result<Vec<Block>> {
    const TARGET_BYTES: usize = 32 * 1024;
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut line = String::new();
    let mut heading_path = Vec::new();
    let mut logical_heading = Vec::new();
    let mut logical = String::new();
    let mut result = Vec::new();
    let mut start = 0_i64;
    let mut has_visible_text = false;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        has_visible_text |= !line.trim().is_empty();
        if let Some((level, title)) = markdown_heading(&line) {
            push_logical_block(
                &mut result,
                &mut logical,
                &logical_heading,
                &mut start,
                TARGET_BYTES,
            );
            heading_path.truncate(level.saturating_sub(1));
            heading_path.push(title.to_owned());
            logical_heading = heading_path.clone();
            logical.push_str(&line);
            continue;
        }
        if logical.is_empty() {
            logical_heading = heading_path.clone();
        }
        logical.push_str(&line);
        if line.trim().is_empty() {
            push_logical_block(
                &mut result,
                &mut logical,
                &logical_heading,
                &mut start,
                TARGET_BYTES,
            );
        }
    }
    push_logical_block(
        &mut result,
        &mut logical,
        &logical_heading,
        &mut start,
        TARGET_BYTES,
    );
    if has_visible_text {
        Ok(result)
    } else {
        Ok(Vec::new())
    }
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_end();
    let hashes = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
        let title = trimmed[hashes + 1..].trim();
        (!title.is_empty()).then_some((hashes, title))
    } else {
        None
    }
}

fn utf8_prefix(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn push_logical_block(
    result: &mut Vec<Block>,
    logical: &mut String,
    heading_path: &[String],
    start: &mut i64,
    target_bytes: usize,
) {
    if logical.is_empty() {
        return;
    }
    let text = std::mem::take(logical);
    let logical_ordinal = result.last().map_or(0, |block| block.logical_ordinal + 1);
    let logical_hash = hash_bytes(text.as_bytes());
    let mut remaining = text.as_str();
    let mut subordinal = 0;
    while !remaining.is_empty() {
        let take = utf8_prefix(remaining, target_bytes);
        let part = remaining[..take].to_owned();
        let end = *start + part.len() as i64;
        result.push(Block {
            ordinal: result.len() as i64,
            logical_ordinal,
            subordinal,
            logical_hash: logical_hash.clone(),
            byte_start: *start,
            byte_end: end,
            text_hash: hash_bytes(part.as_bytes()),
            text: part,
            heading_path: heading_path.to_vec(),
        });
        *start = end;
        subordinal += 1;
        remaining = &remaining[take..];
    }
}

fn logical_block_id(book_id: &str, block: &Block) -> String {
    hash_bytes(
        format!(
            "{book_id}:logical:{}:{}",
            block.logical_ordinal, block.logical_hash
        )
        .as_bytes(),
    )[..32]
        .to_owned()
}

fn block_id(book_id: &str, block: &Block) -> String {
    hash_bytes(
        format!(
            "{}:{}:{}:{}:{}",
            logical_block_id(book_id, block),
            block.subordinal,
            block.byte_start,
            block.byte_end,
            block.text_hash
        )
        .as_bytes(),
    )[..32]
        .to_owned()
}

fn hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct StagedBlob {
    path: PathBuf,
    sha256: String,
    bytes: u64,
}

fn stage_original(root: &Path, source: &Path) -> Result<StagedBlob> {
    let metadata = fs::metadata(source)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_BOOK_BYTES {
        return Err(Error::new(
            "invalid_book_source",
            format!("book source must be a non-empty regular file up to {MAX_BOOK_BYTES} bytes"),
        ));
    }
    let path = root.join(format!(
        ".book-{}-{}.tmp",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    reject_symlink(&path)?;
    let mut input = fs::File::open(source)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > MAX_BOOK_BYTES {
            discard_staged(&path);
            return Err(Error::new(
                "book_too_large",
                "book source exceeds the size limit",
            ));
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(StagedBlob {
        path,
        sha256,
        bytes,
    })
}

fn publish_blob(root: &Path, staged: &StagedBlob) -> Result<()> {
    let blobs = root.join("blobs");
    let sha = blobs.join("sha256");
    let prefix = sha.join(&staged.sha256[..2]);
    for path in [&blobs, &sha, &prefix] {
        reject_symlink(path)?;
        fs::create_dir(path).or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
    }
    let destination = prefix.join(&staged.sha256);
    reject_symlink(&destination)?;
    if destination.is_file() {
        let (actual_hash, actual_bytes) = file_hash(&destination)?;
        if actual_bytes != staged.bytes || actual_hash != staged.sha256 {
            return Err(Error::new(
                "corrupt_blob",
                "existing book blob does not match its content address",
            ));
        }
        discard_staged(&staged.path);
    } else {
        fs::rename(&staged.path, destination)?;
    }
    Ok(())
}

fn file_hash(path: &Path) -> Result<(String, u64)> {
    reject_symlink(path)?;
    let mut file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::new(
            "corrupt_blob",
            "book blob is not a regular file",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > MAX_BOOK_BYTES {
            return Err(Error::new(
                "corrupt_blob",
                "book blob exceeds the size limit",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes,
    ))
}

fn discard_staged(path: &Path) {
    let _ = fs::remove_file(path);
}

fn source_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::new(
            "invalid_book_path",
            "book path must be relative to the current directory",
        ));
    }
    let cwd = fs::canonicalize(std::env::current_dir()?)?;
    let joined = cwd.join(path);
    reject_symlink(&joined)?;
    let source = fs::canonicalize(&joined)?;
    if !source.starts_with(&cwd) {
        return Err(Error::new(
            "invalid_book_path",
            "book path resolves outside the current directory",
        ));
    }
    Ok(source)
}

fn book_format(path: &Path) -> Result<&'static str> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "txt" => Ok("txt"),
        "md" | "markdown" => Ok("markdown"),
        "epub" => Ok("epub"),
        "pdf" => Ok("pdf"),
        _ => Err(Error::new(
            "unsupported_book_format",
            "Book supports EPUB, TXT, Markdown, and text PDF",
        )
        .details(json!({
            "guidance": "convert HTML, MOBI, or AZW3 to EPUB/TXT/Markdown; scanned PDFs require OCR outside Book"
        }))),
    }
}

fn book(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    let mut value = connection
        .query_row(
            "SELECT subject_id,title,author,format,original_sha256,original_bytes,
                    normalized_sha256,normalized_bytes,edition_of,state,revision,created_at,updated_at,
                    converter,converter_args_json
             FROM books WHERE id=?1",
            [id],
            |row| {
                Ok(json!({
                    "id": id,
                    "subject_id": row.get::<_, String>(0)?,
                    "title": row.get::<_, String>(1)?,
                    "author": row.get::<_, Option<String>>(2)?,
                    "format": row.get::<_, String>(3)?,
                    "original_sha256": row.get::<_, String>(4)?,
                    "original_bytes": row.get::<_, i64>(5)?,
                    "normalized_sha256": row.get::<_, Option<String>>(6)?,
                    "normalized_bytes": row.get::<_, Option<i64>>(7)?,
                    "edition_of": row.get::<_, Option<String>>(8)?,
                    "state": row.get::<_, String>(9)?,
                    "revision": row.get::<_, i64>(10)?,
                    "created_at": row.get::<_, String>(11)?,
                    "updated_at": row.get::<_, String>(12)?,
                    "converter": row.get::<_, Option<String>>(13)?,
                    "converter_args": row.get::<_, Option<String>>(14)?
                        .map(|value| serde_json::from_str::<Value>(&value))
                        .transpose()
                        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                            14,rusqlite::types::Type::Text,Box::new(error)
                        ))?,
                }))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("book_not_found", format!("book {id} was not found")))?;
    let block_count = connection.query_row(
        "SELECT COUNT(*) FROM book_blocks WHERE book_id=?1",
        [id],
        |row| row.get::<_, i64>(0),
    )?;
    let anomaly_count = connection.query_row(
        "SELECT COUNT(*) FROM book_anomalies WHERE book_id=?1",
        [id],
        |row| row.get::<_, i64>(0),
    )?;
    let object = value.as_object_mut().unwrap();
    object.insert("block_count".to_owned(), json!(block_count));
    object.insert("anomaly_count".to_owned(), json!(anomaly_count));
    object.insert("coverage".to_owned(), coverage(connection, id)?);
    Ok(value)
}

fn validate_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max_bytes {
        Err(Error::new(
            "invalid_input",
            format!("{field} must contain 1..={max_bytes} UTF-8 bytes"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn wiki_publish_rejects_a_preexisting_temporary_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let temporary = directory.path().join("temporary");
        let destination = directory.path().join("destination");
        fs::write(&target, b"untouched").unwrap();
        symlink(&target, &temporary).unwrap();
        let error = publish_wiki_file(&temporary, &destination, b"replacement").unwrap_err();
        assert_eq!(error.code(), "unsafe_store_path");
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert!(!destination.exists());
    }

    #[test]
    fn malformed_blob_hash_is_an_error_not_a_path_slice_panic() {
        let error = blob_path(Path::new("/tmp"), "a").unwrap_err();
        assert_eq!(error.code(), "corrupt_store");
    }
}
