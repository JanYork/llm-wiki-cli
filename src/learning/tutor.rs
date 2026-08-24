use crate::learning::*;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, sync::atomic::Ordering};

const SOUL_MAX_BYTES: usize = 64 * 1024;
const INITIAL_SOUL: &str = "# 老师的灵魂\n\n保持客观、科学、诚实，禁止谄媚。先识别学生的具体阻塞，再逐级提供提示；评价必须引用可观察证据。\n";

#[derive(Parser)]
struct TutorCli {
    #[command(subcommand)]
    command: TutorCommand,
}

#[derive(Subcommand)]
enum TutorCommand {
    Subject {
        #[command(subcommand)]
        command: SubjectCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Turn {
        #[command(subcommand)]
        command: TurnCommand,
    },
    Soul {
        #[command(subcommand)]
        command: SoulCommand,
    },
    Status,
}

#[derive(Subcommand)]
enum SessionCommand {
    Create {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum TurnCommand {
    Begin {
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
    Pending {
        #[arg(long)]
        session: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum SoulCommand {
    Show,
    Publish {
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionCreate {
    subject_id: String,
    mode: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TurnBegin {
    session_id: String,
    owner: String,
    input: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TurnCommit {
    reply: String,
    checkpoint: Value,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoulPublish {
    body: String,
    #[serde(default)]
    fact_refs: Vec<String>,
    reason: String,
    sensitivity: String,
    #[serde(default)]
    approved: bool,
    request_id: String,
}

pub(crate) fn main() {
    finish(run(TutorCli::parse()));
}

fn initialize(connection: &Connection, root: &Path) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tutor_sessions(
           id TEXT PRIMARY KEY,
           subject_id TEXT NOT NULL REFERENCES subjects(id),
           mode TEXT NOT NULL CHECK(mode IN ('learning','question','exam')),
           state TEXT NOT NULL CHECK(state IN ('active','closed')),
           revision INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS tutor_sessions_subject ON tutor_sessions(subject_id,state,id);
         CREATE TABLE IF NOT EXISTS tutor_turns(
           id TEXT PRIMARY KEY,
           session_id TEXT NOT NULL REFERENCES tutor_sessions(id),
           owner TEXT NOT NULL,
           input TEXT NOT NULL,
           reply TEXT,
           checkpoint_json TEXT,
           state TEXT NOT NULL CHECK(state IN ('pending','committed')),
           revision INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           committed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS tutor_turns_pending
           ON tutor_turns(session_id,state,created_at,id);
         CREATE TABLE IF NOT EXISTS soul_versions(
           revision INTEGER PRIMARY KEY,
           body TEXT NOT NULL,
           body_hash TEXT NOT NULL,
           fact_refs_json TEXT NOT NULL,
           reason TEXT NOT NULL,
           sensitivity TEXT NOT NULL,
           approved INTEGER NOT NULL,
           created_at TEXT NOT NULL
         );",
    )?;
    if connection
        .query_row("SELECT 1 FROM soul_versions LIMIT 1", [], |_| Ok(()))
        .optional()?
        .is_none()
    {
        let timestamp = now(connection)?;
        connection.execute(
            "INSERT INTO soul_versions(
               revision,body,body_hash,fact_refs_json,reason,sensitivity,approved,created_at
             ) VALUES(1,?1,?2,'[]','initial teacher contract','ordinary',1,?3)",
            params![INITIAL_SOUL, sha256(INITIAL_SOUL.as_bytes()), timestamp],
        )?;
    }
    materialize_soul(connection, root)
}

fn run(cli: TutorCli) -> Result<Value> {
    let mut store = Store::open(Plugin::Tutor)?;
    initialize(&store.connection, &store.root)?;
    match cli.command {
        TutorCommand::Subject { command } => run_subject(Plugin::Tutor, &mut store, command),
        TutorCommand::Session { command } => run_session(&mut store, command),
        TutorCommand::Turn { command } => run_turn(&mut store, command),
        TutorCommand::Soul { command } => run_soul(&mut store, command),
        TutorCommand::Status => {
            let sessions = store.connection.query_row(
                "SELECT COUNT(*) FROM tutor_sessions WHERE state='active'",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            let pending = store.connection.query_row(
                "SELECT COUNT(*) FROM tutor_turns WHERE state='pending'",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            Ok(envelope(
                Plugin::Tutor,
                "status",
                json!({"active_sessions": sessions, "pending_turns": pending}),
            ))
        }
    }
}

fn run_session(store: &mut Store, command: SessionCommand) -> Result<Value> {
    match command {
        SessionCommand::Create { json } => {
            let input: SessionCreate = read_json(&json)?;
            validate_id(&input.subject_id)?;
            validate_request_id(&input.request_id)?;
            if !matches!(input.mode.as_str(), "learning" | "question" | "exam") {
                return Err(Error::new(
                    "invalid_input",
                    "session mode must be learning, question, or exam",
                ));
            }
            let fingerprint = fingerprint(&input)?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            subject(&tx, &input.subject_id)?;
            let id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO tutor_sessions(id,subject_id,mode,state,revision,created_at,updated_at)
                 VALUES(?1,?2,?3,'active',1,?4,?4)",
                params![id, input.subject_id, input.mode, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "session.create",
                json!({"session": session(&tx, &id)?}),
            );
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
        SessionCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "session.show",
            json!({"session": session(&store.connection, &id)?}),
        )),
    }
}

fn run_turn(store: &mut Store, command: TurnCommand) -> Result<Value> {
    match command {
        TurnCommand::Begin { json } => {
            let input: TurnBegin = read_json(&json)?;
            validate_id(&input.session_id)?;
            validate_request_id(&input.request_id)?;
            validate_text("owner", &input.owner, 256)?;
            validate_text("input", &input.input, 1024 * 1024)?;
            let fingerprint = fingerprint(&input)?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = session(&tx, &input.session_id)?;
            if current["state"] != "active" {
                return Err(Error::new("session_closed", "Tutor session is closed"));
            }
            let id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO tutor_turns(
                   id,session_id,owner,input,state,revision,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,'pending',1,?5,?5)",
                params![id, input.session_id, input.owner, input.input, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "turn.begin",
                json!({"turn": turn(&tx, &id)?}),
            );
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
        TurnCommand::Commit {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            let input: TurnCommit = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_text("reply", &input.reply, 1024 * 1024)?;
            if !input.checkpoint.is_object() {
                return Err(Error::new("invalid_input", "checkpoint must be an object"));
            }
            let fingerprint = fingerprint(&json!({
                "turn_id": id,
                "if_revision": if_revision,
                "commit": input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = turn(&tx, &id)?;
            if current["state"] != "pending" || current["revision"] != if_revision {
                return Err(Error::new("revision_conflict", "turn revision is stale")
                    .details(json!({"expected": if_revision, "current": current["revision"]})));
            }
            let checkpoint = serde_json::to_string(&input.checkpoint)
                .map_err(|error| Error::new("json_error", error.to_string()))?;
            let timestamp = now(&tx)?;
            tx.execute(
                "UPDATE tutor_turns
                 SET reply=?2,checkpoint_json=?3,state='committed',revision=2,
                     updated_at=?4,committed_at=?4 WHERE id=?1",
                params![id, input.reply, checkpoint, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "turn.commit",
                json!({"turn": turn(&tx, &id)?}),
            );
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
        TurnCommand::Pending { session: id } => {
            validate_id(&id)?;
            session(&store.connection, &id)?;
            let mut statement = store.connection.prepare(
                "SELECT id FROM tutor_turns
                 WHERE session_id=?1 AND state='pending' ORDER BY created_at,id",
            )?;
            let ids = statement
                .query_map([id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let turns = ids
                .iter()
                .map(|id| turn(&store.connection, id))
                .collect::<Result<Vec<_>>>()?;
            Ok(envelope(
                Plugin::Tutor,
                "turn.pending",
                json!({"turns": turns}),
            ))
        }
        TurnCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "turn.show",
            json!({"turn": turn(&store.connection, &id)?}),
        )),
    }
}

fn run_soul(store: &mut Store, command: SoulCommand) -> Result<Value> {
    match command {
        SoulCommand::Show => Ok(envelope(
            Plugin::Tutor,
            "soul.show",
            json!({"soul": soul(&store.connection)?}),
        )),
        SoulCommand::Publish { if_revision, json } => {
            let input: SoulPublish = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            if input.body.trim().is_empty() {
                return Err(Error::new("invalid_input", "Soul body must not be empty"));
            }
            validate_text("reason", &input.reason, 4096)?;
            if input.body.len() > SOUL_MAX_BYTES {
                return Err(Error::new(
                    "soul_too_large",
                    format!("Soul body exceeds {SOUL_MAX_BYTES} UTF-8 bytes"),
                ));
            }
            if !matches!(
                input.sensitivity.as_str(),
                "ordinary" | "sensitive" | "behavior-changing"
            ) {
                return Err(Error::new("invalid_input", "invalid Soul sensitivity"));
            }
            if input.sensitivity != "ordinary" && !input.approved {
                return Err(Error::new(
                    "soul_approval_required",
                    "sensitive or behavior-changing Soul revisions require learner approval",
                ));
            }
            if input.fact_refs.len() > 1024
                || input
                    .fact_refs
                    .iter()
                    .any(|reference| reference.trim().is_empty())
            {
                return Err(Error::new("invalid_input", "Soul fact_refs are invalid"));
            }
            let fingerprint = fingerprint(&json!({
                "if_revision": if_revision,
                "publish": input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = soul(&tx)?;
            if current["revision"] != if_revision {
                return Err(Error::new("revision_conflict", "Soul revision is stale")
                    .details(json!({"expected": if_revision, "current": current["revision"]})));
            }
            let revision = if_revision + 1;
            let timestamp = now(&tx)?;
            let refs = serde_json::to_string(&input.fact_refs)
                .map_err(|error| Error::new("json_error", error.to_string()))?;
            tx.execute(
                "INSERT INTO soul_versions(
                   revision,body,body_hash,fact_refs_json,reason,sensitivity,approved,created_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    revision,
                    input.body,
                    sha256(input.body.as_bytes()),
                    refs,
                    input.reason,
                    input.sensitivity,
                    input.approved,
                    timestamp,
                ],
            )?;
            let value = envelope(Plugin::Tutor, "soul.publish", json!({"soul": soul(&tx)?}));
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            materialize_soul(&store.connection, &store.root)?;
            Ok(value)
        }
    }
}

fn session(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    connection
        .query_row(
            "SELECT id,subject_id,mode,state,revision,created_at,updated_at
             FROM tutor_sessions WHERE id=?1",
            [id],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "subject_id": row.get::<_, String>(1)?,
                    "mode": row.get::<_, String>(2)?,
                    "state": row.get::<_, String>(3)?,
                    "revision": row.get::<_, i64>(4)?,
                    "created_at": row.get::<_, String>(5)?,
                    "updated_at": row.get::<_, String>(6)?,
                }))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("session_not_found", format!("session {id} was not found")))
}

fn turn(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    let row = connection
        .query_row(
            "SELECT id,session_id,owner,input,reply,checkpoint_json,state,revision,
                    created_at,updated_at,committed_at
             FROM tutor_turns WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("turn_not_found", format!("turn {id} was not found")))?;
    let checkpoint: Option<Value> = row
        .5
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|error| Error::new("corrupt_store", error.to_string()))
        })
        .transpose()?;
    Ok(json!({
        "id": row.0,
        "session_id": row.1,
        "owner": row.2,
        "input": row.3,
        "reply": row.4,
        "checkpoint": checkpoint,
        "state": row.6,
        "revision": row.7,
        "created_at": row.8,
        "updated_at": row.9,
        "committed_at": row.10,
    }))
}

fn soul(connection: &Connection) -> Result<Value> {
    let row = connection.query_row(
        "SELECT revision,body,body_hash,fact_refs_json,reason,sensitivity,approved,created_at
         FROM soul_versions ORDER BY revision DESC LIMIT 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, String>(7)?,
            ))
        },
    )?;
    let fact_refs: Value = serde_json::from_str(&row.3)
        .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
    Ok(json!({
        "revision": row.0,
        "body": row.1,
        "body_sha256": row.2,
        "body_bytes": row.1.len(),
        "max_bytes": SOUL_MAX_BYTES,
        "fact_refs": fact_refs,
        "reason": row.4,
        "sensitivity": row.5,
        "approved": row.6,
        "created_at": row.7,
    }))
}

fn materialize_soul(connection: &Connection, root: &Path) -> Result<()> {
    let body = connection.query_row(
        "SELECT body FROM soul_versions ORDER BY revision DESC LIMIT 1",
        [],
        |row| row.get::<_, String>(0),
    )?;
    let path = root.join("soul.md");
    reject_symlink(&path)?;
    if fs::read(&path).ok().as_deref() == Some(body.as_bytes()) {
        return Ok(());
    }
    let temporary = root.join(format!(
        ".soul-{}-{}.tmp",
        std::process::id(),
        ID_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    use std::io::Write;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, &path)?;
    Ok(())
}

fn validate_text(field: &str, value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(Error::new(
            "invalid_input",
            format!("{field} must contain 1..={max_bytes} UTF-8 bytes"),
        ));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
