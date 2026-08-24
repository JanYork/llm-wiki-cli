use crate::learning::*;
use crate::tutor_plan::PlanCommand;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, sync::atomic::Ordering};

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
    Learner {
        #[command(subcommand)]
        command: LearnerCommand,
    },
    Soul {
        #[command(subcommand)]
        command: SoulCommand,
    },
    Goal {
        #[command(subcommand)]
        command: GoalCommand,
    },
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
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
enum LearnerCommand {
    Fact {
        #[command(subcommand)]
        command: FactCommand,
    },
}

#[derive(Subcommand)]
enum FactCommand {
    Record {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Revise {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
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

#[derive(Subcommand)]
enum GoalCommand {
    Create {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Show {
        id: String,
    },
    Evidence {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Complete {
        id: String,
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FactRecord {
    scope: String,
    #[serde(default)]
    subject_id: Option<String>,
    claim: String,
    confidence: f64,
    evidence_refs: Vec<String>,
    origin: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FactRevise {
    action: String,
    #[serde(default)]
    claim: Option<String>,
    evidence_refs: Vec<String>,
    confidence: f64,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    corroborating_subject_ids: Vec<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GoalCreate {
    subject_id: String,
    statement: String,
    criteria: Vec<String>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CriterionEvidence {
    criterion_id: String,
    evidence_refs: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GoalEvidence {
    criteria: Vec<CriterionEvidence>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GoalComplete {
    confirmed_by: String,
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
         );
         CREATE TABLE IF NOT EXISTS learner_facts(
           id TEXT PRIMARY KEY,
           scope TEXT NOT NULL CHECK(scope IN ('global','subject')),
           subject_id TEXT REFERENCES subjects(id),
           claim TEXT NOT NULL CHECK(trim(claim)<>''),
           status TEXT NOT NULL CHECK(status IN ('provisional','confirmed','superseded')),
           confidence REAL NOT NULL CHECK(confidence>=0 AND confidence<=1),
           evidence_refs_json TEXT NOT NULL,
           origin TEXT NOT NULL CHECK(origin IN ('agent','learner')),
           supersedes_id TEXT REFERENCES learner_facts(id),
           revision INTEGER NOT NULL CHECK(revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           CHECK((scope='global' AND subject_id IS NULL) OR
                 (scope='subject' AND subject_id IS NOT NULL))
         );
         CREATE INDEX IF NOT EXISTS learner_facts_scope
           ON learner_facts(scope,subject_id,status,id);
         CREATE TABLE IF NOT EXISTS learner_fact_history(
           fact_id TEXT NOT NULL REFERENCES learner_facts(id),
           revision INTEGER NOT NULL,
           snapshot_json TEXT NOT NULL,
           changed_at TEXT NOT NULL,
           PRIMARY KEY(fact_id,revision)
         );
         CREATE TABLE IF NOT EXISTS tutor_goals(
           id TEXT PRIMARY KEY,
           subject_id TEXT NOT NULL REFERENCES subjects(id),
           statement TEXT NOT NULL CHECK(trim(statement)<>''),
           status TEXT NOT NULL CHECK(status IN (
             'active','ready_to_complete','completed','paused','abandoned'
           )),
           revision INTEGER NOT NULL CHECK(revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           completed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS tutor_goals_subject
           ON tutor_goals(subject_id,status,id);
         CREATE TABLE IF NOT EXISTS tutor_goal_criteria(
           id TEXT PRIMARY KEY,
           goal_id TEXT NOT NULL REFERENCES tutor_goals(id),
           ordinal INTEGER NOT NULL CHECK(ordinal>=0),
           description TEXT NOT NULL CHECK(trim(description)<>''),
           UNIQUE(goal_id,ordinal)
         );
         CREATE TABLE IF NOT EXISTS tutor_goal_evidence(
           goal_id TEXT NOT NULL REFERENCES tutor_goals(id),
           criterion_id TEXT NOT NULL REFERENCES tutor_goal_criteria(id),
           evidence_refs_json TEXT NOT NULL,
           goal_revision INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           PRIMARY KEY(goal_id,criterion_id,goal_revision)
         );
         CREATE TABLE IF NOT EXISTS tutor_goal_history(
           goal_id TEXT NOT NULL REFERENCES tutor_goals(id),
           revision INTEGER NOT NULL,
           snapshot_json TEXT NOT NULL,
           changed_at TEXT NOT NULL,
           PRIMARY KEY(goal_id,revision)
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
    crate::tutor_plan::initialize(connection)?;
    materialize_soul(connection, root)?;
    materialize_fact_wiki(connection, root)
}

fn run(cli: TutorCli) -> Result<Value> {
    let mut store = Store::open(Plugin::Tutor)?;
    initialize(&store.connection, &store.root)?;
    match cli.command {
        TutorCommand::Subject { command } => run_subject(Plugin::Tutor, &mut store, command),
        TutorCommand::Session { command } => run_session(&mut store, command),
        TutorCommand::Turn { command } => run_turn(&mut store, command),
        TutorCommand::Learner { command } => run_learner(&mut store, command),
        TutorCommand::Soul { command } => run_soul(&mut store, command),
        TutorCommand::Goal { command } => run_goal(&mut store, command),
        TutorCommand::Plan { command } => crate::tutor_plan::run(&mut store, command),
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

fn run_learner(store: &mut Store, command: LearnerCommand) -> Result<Value> {
    match command {
        LearnerCommand::Fact { command } => run_fact(store, command),
    }
}

fn run_fact(store: &mut Store, command: FactCommand) -> Result<Value> {
    match command {
        FactCommand::Record { json } => {
            let input: FactRecord = read_json(&json)?;
            validate_fact_record(&store.connection, &input)?;
            let fingerprint = fingerprint(&input)?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            let status = if input.origin == "learner" {
                "confirmed"
            } else {
                "provisional"
            };
            let evidence = normalized_refs(input.evidence_refs)?;
            tx.execute(
                "INSERT INTO learner_facts(
                   id,scope,subject_id,claim,status,confidence,evidence_refs_json,origin,
                   supersedes_id,revision,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,1,?9,?9)",
                params![
                    id,
                    input.scope,
                    input.subject_id,
                    input.claim,
                    status,
                    input.confidence,
                    serde_json::to_string(&evidence)
                        .map_err(|error| Error::new("json_error", error.to_string()))?,
                    input.origin,
                    timestamp,
                ],
            )?;
            let fact = fact(&tx, &id)?;
            append_fact_history(&tx, &fact)?;
            let value = envelope(Plugin::Tutor, "learner.fact.record", json!({"fact": fact}));
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            materialize_fact_wiki(&store.connection, &store.root)?;
            Ok(value)
        }
        FactCommand::Revise {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            if if_revision < 1 {
                return Err(Error::new("invalid_input", "if_revision must be positive"));
            }
            let mut input: FactRevise = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_confidence(input.confidence)?;
            input.evidence_refs = normalized_refs(input.evidence_refs)?;
            let fingerprint = fingerprint(&json!({
                "id": id,
                "if_revision": if_revision,
                "input": input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = fact(&tx, &id)?;
            let revision = current["revision"].as_i64().unwrap();
            if revision != if_revision {
                return Err(
                    Error::new("revision_conflict", "learner fact revision is stale")
                        .details(json!({"expected": if_revision, "current": revision})),
                );
            }
            if current["status"] == "superseded" {
                return Err(Error::new(
                    "fact_superseded",
                    "a superseded learner fact cannot be revised",
                ));
            }
            let value = match input.action.as_str() {
                "corroborate" => corroborate_fact(&tx, &id, &current, &input)?,
                "contradict" | "correct" => replace_fact(&tx, &id, &current, &input, false)?,
                "promote" => replace_fact(&tx, &id, &current, &input, true)?,
                _ => {
                    return Err(Error::new(
                        "invalid_input",
                        "action must be corroborate, contradict, correct, or promote",
                    ));
                }
            };
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            materialize_fact_wiki(&store.connection, &store.root)?;
            Ok(value)
        }
        FactCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "learner.fact.show",
            json!({"fact": fact(&store.connection, &id)?}),
        )),
    }
}

fn validate_fact_record(connection: &Connection, input: &FactRecord) -> Result<()> {
    validate_request_id(&input.request_id)?;
    validate_text("claim", &input.claim, 16 * 1024)?;
    validate_confidence(input.confidence)?;
    normalized_refs(input.evidence_refs.clone())?;
    validate_origin(&input.origin)?;
    match (input.scope.as_str(), input.subject_id.as_deref()) {
        ("global", None) => Ok(()),
        ("subject", Some(subject_id)) => {
            subject(connection, subject_id)?;
            Ok(())
        }
        ("global", Some(_)) | ("subject", None) => Err(Error::new(
            "invalid_input",
            "global facts omit subject_id; subject facts require subject_id",
        )),
        _ => Err(Error::new(
            "invalid_input",
            "scope must be global or subject",
        )),
    }
}

fn corroborate_fact(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    current: &Value,
    input: &FactRevise,
) -> Result<Value> {
    if input.claim.is_some()
        || input.origin.is_some()
        || !input.corroborating_subject_ids.is_empty()
    {
        return Err(Error::new(
            "invalid_input",
            "corroborate accepts only evidence_refs, confidence, and request_id",
        ));
    }
    let evidence = merged_evidence(current, &input.evidence_refs)?;
    let next_revision = current["revision"].as_i64().unwrap() + 1;
    let timestamp = now(tx)?;
    tx.execute(
        "UPDATE learner_facts SET status='confirmed',confidence=?2,
           evidence_refs_json=?3,revision=?4,updated_at=?5 WHERE id=?1",
        params![
            id,
            input.confidence,
            serde_json::to_string(&evidence)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            next_revision,
            timestamp,
        ],
    )?;
    let fact = fact(tx, id)?;
    append_fact_history(tx, &fact)?;
    Ok(envelope(
        Plugin::Tutor,
        "learner.fact.revise",
        json!({"fact": fact}),
    ))
}

fn replace_fact(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    current: &Value,
    input: &FactRevise,
    promote: bool,
) -> Result<Value> {
    let (scope, subject_id, claim, origin, status) = if promote {
        if current["scope"] != "subject" || input.claim.is_some() || input.origin.is_some() {
            return Err(Error::new(
                "invalid_input",
                "promote applies to a subject fact without changing claim or origin",
            ));
        }
        let mut subjects = BTreeSet::new();
        for subject_id in &input.corroborating_subject_ids {
            subject(tx, subject_id)?;
            subjects.insert(subject_id);
        }
        if subjects.len() < 2 {
            return Err(Error::new(
                "cross_subject_evidence_required",
                "global promotion requires evidence from at least two subjects",
            ));
        }
        (
            "global",
            None,
            current["claim"].as_str().unwrap(),
            "agent",
            "confirmed",
        )
    } else {
        if !input.corroborating_subject_ids.is_empty() {
            return Err(Error::new(
                "invalid_input",
                "corroborating_subject_ids is only valid for promotion",
            ));
        }
        let claim = input
            .claim
            .as_deref()
            .ok_or_else(|| Error::new("invalid_input", "replacement claim is required"))?;
        validate_text("claim", claim, 16 * 1024)?;
        let origin = input
            .origin
            .as_deref()
            .ok_or_else(|| Error::new("invalid_input", "replacement origin is required"))?;
        validate_origin(origin)?;
        if input.action == "correct" && origin != "learner" {
            return Err(Error::new(
                "invalid_input",
                "a correction must have learner origin",
            ));
        }
        (
            current["scope"].as_str().unwrap(),
            current["subject_id"].as_str(),
            claim,
            origin,
            if origin == "learner" {
                "confirmed"
            } else {
                "provisional"
            },
        )
    };
    let timestamp = now(tx)?;
    let old_revision = current["revision"].as_i64().unwrap() + 1;
    tx.execute(
        "UPDATE learner_facts SET status='superseded',revision=?2,updated_at=?3 WHERE id=?1",
        params![id, old_revision, timestamp],
    )?;
    let previous = fact(tx, id)?;
    append_fact_history(tx, &previous)?;

    let next_id = new_id(Plugin::Tutor, &input.request_id);
    let evidence = merged_evidence(current, &input.evidence_refs)?;
    tx.execute(
        "INSERT INTO learner_facts(
           id,scope,subject_id,claim,status,confidence,evidence_refs_json,origin,
           supersedes_id,revision,created_at,updated_at
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10,?10)",
        params![
            next_id,
            scope,
            subject_id,
            claim,
            status,
            input.confidence,
            serde_json::to_string(&evidence)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            origin,
            id,
            timestamp,
        ],
    )?;
    let fact = fact(tx, &next_id)?;
    append_fact_history(tx, &fact)?;
    Ok(envelope(
        Plugin::Tutor,
        "learner.fact.revise",
        json!({"previous": previous, "fact": fact}),
    ))
}

fn fact(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    connection
        .query_row(
            "SELECT id,scope,subject_id,claim,status,confidence,evidence_refs_json,
                    origin,supersedes_id,revision,created_at,updated_at
             FROM learner_facts WHERE id=?1",
            [id],
            fact_row,
        )
        .optional()?
        .ok_or_else(|| Error::new("fact_not_found", format!("learner fact {id} was not found")))
}

fn fact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let evidence: String = row.get(6)?;
    let evidence: Value = serde_json::from_str(&evidence).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "scope": row.get::<_, String>(1)?,
        "subject_id": row.get::<_, Option<String>>(2)?,
        "claim": row.get::<_, String>(3)?,
        "status": row.get::<_, String>(4)?,
        "confidence": row.get::<_, f64>(5)?,
        "evidence_refs": evidence,
        "origin": row.get::<_, String>(7)?,
        "supersedes_id": row.get::<_, Option<String>>(8)?,
        "revision": row.get::<_, i64>(9)?,
        "created_at": row.get::<_, String>(10)?,
        "updated_at": row.get::<_, String>(11)?,
    }))
}

fn append_fact_history(tx: &rusqlite::Transaction<'_>, fact: &Value) -> Result<()> {
    tx.execute(
        "INSERT INTO learner_fact_history(fact_id,revision,snapshot_json,changed_at)
         VALUES(?1,?2,?3,?4)",
        params![
            fact["id"].as_str().unwrap(),
            fact["revision"].as_i64().unwrap(),
            serde_json::to_string(fact)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            fact["updated_at"].as_str().unwrap(),
        ],
    )?;
    Ok(())
}

fn merged_evidence(current: &Value, added: &[String]) -> Result<Vec<String>> {
    let mut values = current["evidence_refs"]
        .as_array()
        .ok_or_else(|| Error::new("corrupt_store", "fact evidence is not an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::new("corrupt_store", "fact evidence is not text"))
        })
        .collect::<Result<Vec<_>>>()?;
    values.extend_from_slice(added);
    normalized_refs(values)
}

fn normalized_refs(mut refs: Vec<String>) -> Result<Vec<String>> {
    for reference in &mut refs {
        *reference = reference.trim().to_owned();
        if reference.is_empty() || reference.len() > 512 {
            return Err(Error::new(
                "invalid_input",
                "evidence refs must contain 1..=512 UTF-8 bytes",
            ));
        }
    }
    refs.sort();
    refs.dedup();
    if refs.is_empty() || refs.len() > 256 {
        return Err(Error::new(
            "invalid_input",
            "learner facts require 1..=256 evidence refs",
        ));
    }
    Ok(refs)
}

fn validate_origin(origin: &str) -> Result<()> {
    if matches!(origin, "agent" | "learner") {
        Ok(())
    } else {
        Err(Error::new(
            "invalid_input",
            "origin must be agent or learner",
        ))
    }
}

fn validate_confidence(confidence: f64) -> Result<()> {
    if confidence.is_finite() && (0.0..=1.0).contains(&confidence) {
        Ok(())
    } else {
        Err(Error::new(
            "invalid_input",
            "confidence must be between 0 and 1",
        ))
    }
}

fn materialize_fact_wiki(connection: &Connection, root: &Path) -> Result<()> {
    let wiki = root.join("wiki");
    let directory = wiki.join("subjects");
    for path in [wiki.clone(), directory.clone()] {
        reject_symlink(&path)?;
        fs::create_dir_all(path)?;
    }
    let mut subjects = connection.prepare(
        "SELECT DISTINCT subject_id FROM learner_facts
         WHERE subject_id IS NOT NULL ORDER BY subject_id",
    )?;
    let subject_ids = subjects
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for subject_id in subject_ids {
        let mut statement = connection.prepare(
            "SELECT id,scope,subject_id,claim,status,confidence,evidence_refs_json,
                    origin,supersedes_id,revision,created_at,updated_at
             FROM learner_facts WHERE subject_id=?1 ORDER BY created_at,id",
        )?;
        let facts = statement
            .query_map([&subject_id], fact_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut body = format!("# Tutor subject {subject_id}\n\n");
        for fact in facts {
            body.push_str(&format!(
                "- [{}] {} (confidence: {}, origin: {}, id: {})\n",
                fact["status"].as_str().unwrap(),
                fact["claim"].as_str().unwrap(),
                fact["confidence"],
                fact["origin"].as_str().unwrap(),
                fact["id"].as_str().unwrap(),
            ));
        }
        write_private_file(&directory.join(format!("{subject_id}.md")), body.as_bytes())?;
    }
    let mut statement = connection.prepare(
        "SELECT id,scope,subject_id,claim,status,confidence,evidence_refs_json,
                origin,supersedes_id,revision,created_at,updated_at
         FROM learner_facts WHERE scope='global' ORDER BY created_at,id",
    )?;
    let facts = statement
        .query_map([], fact_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut body = "# Tutor learner profile\n\n".to_owned();
    for fact in facts {
        body.push_str(&format!(
            "- [{}] {} (confidence: {}, origin: {}, id: {})\n",
            fact["status"].as_str().unwrap(),
            fact["claim"].as_str().unwrap(),
            fact["confidence"],
            fact["origin"].as_str().unwrap(),
            fact["id"].as_str().unwrap(),
        ));
    }
    write_private_file(&wiki.join("learner.md"), body.as_bytes())?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_symlink(path)?;
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("unsafe_store_path", "materialized path has no parent"))?;
    let temporary = parent.join(format!(
        ".materialize-{}-{}.tmp",
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
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn run_goal(store: &mut Store, command: GoalCommand) -> Result<Value> {
    match command {
        GoalCommand::Create { json } => {
            let input: GoalCreate = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_text("goal statement", &input.statement, 16 * 1024)?;
            if input.criteria.is_empty() {
                return Err(Error::new(
                    "goal_criteria_required",
                    "a goal requires at least one observable criterion",
                ));
            }
            if input.criteria.len() > 128 {
                return Err(Error::new(
                    "invalid_input",
                    "a goal accepts at most 128 criteria",
                ));
            }
            for criterion in &input.criteria {
                validate_text("goal criterion", criterion, 4 * 1024)?;
            }
            subject(&store.connection, &input.subject_id)?;
            let fingerprint = fingerprint(&input)?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO tutor_goals(
                   id,subject_id,statement,status,revision,created_at,updated_at,completed_at
                 ) VALUES(?1,?2,?3,'active',1,?4,?4,NULL)",
                params![id, input.subject_id, input.statement, timestamp],
            )?;
            for (ordinal, description) in input.criteria.iter().enumerate() {
                let criterion_id = new_id(
                    Plugin::Tutor,
                    &format!("{}:criterion:{ordinal}", input.request_id),
                );
                tx.execute(
                    "INSERT INTO tutor_goal_criteria(id,goal_id,ordinal,description)
                     VALUES(?1,?2,?3,?4)",
                    params![criterion_id, id, ordinal as i64, description],
                )?;
            }
            let goal = goal(&tx, &id)?;
            append_goal_history(&tx, &goal)?;
            let value = envelope(Plugin::Tutor, "goal.create", json!({"goal": goal}));
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
        GoalCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "goal.show",
            json!({"goal": goal(&store.connection, &id)?}),
        )),
        GoalCommand::Evidence {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            let mut input: GoalEvidence = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            let fingerprint = fingerprint(&json!({
                "id": id,
                "if_revision": if_revision,
                "input": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = goal(&tx, &id)?;
            require_goal_revision(&current, if_revision)?;
            if current["status"] != "active" {
                return Err(Error::new(
                    "goal_not_active",
                    "criterion evidence requires an active goal",
                ));
            }
            let expected = current["criteria"]
                .as_array()
                .unwrap()
                .iter()
                .map(|criterion| criterion["id"].as_str().unwrap().to_owned())
                .collect::<BTreeSet<_>>();
            let mut supplied = BTreeSet::new();
            for item in &mut input.criteria {
                validate_id(&item.criterion_id)?;
                item.evidence_refs = normalized_refs(std::mem::take(&mut item.evidence_refs))?;
                if !supplied.insert(item.criterion_id.clone()) {
                    return Err(Error::new(
                        "invalid_input",
                        "criterion evidence contains a duplicate criterion",
                    ));
                }
            }
            if supplied != expected {
                return Err(Error::new(
                    "goal_criteria_incomplete",
                    "evidence must cover every current goal criterion exactly once",
                ));
            }
            let next_revision = if_revision + 1;
            let timestamp = now(&tx)?;
            for item in &input.criteria {
                tx.execute(
                    "INSERT INTO tutor_goal_evidence(
                       goal_id,criterion_id,evidence_refs_json,goal_revision,created_at
                     ) VALUES(?1,?2,?3,?4,?5)",
                    params![
                        id,
                        item.criterion_id,
                        serde_json::to_string(&item.evidence_refs)
                            .map_err(|error| Error::new("json_error", error.to_string()))?,
                        next_revision,
                        timestamp,
                    ],
                )?;
            }
            tx.execute(
                "UPDATE tutor_goals SET status='ready_to_complete',revision=?2,updated_at=?3
                 WHERE id=?1",
                params![id, next_revision, timestamp],
            )?;
            let goal = goal(&tx, &id)?;
            append_goal_history(&tx, &goal)?;
            let value = envelope(Plugin::Tutor, "goal.evidence", json!({"goal": goal}));
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
        GoalCommand::Complete {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            let input: GoalComplete = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            let fingerprint = fingerprint(&json!({
                "id": id,
                "if_revision": if_revision,
                "input": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = goal(&tx, &id)?;
            require_goal_revision(&current, if_revision)?;
            if current["status"] != "ready_to_complete" {
                return Err(Error::new(
                    "goal_not_ready",
                    "goal completion requires complete criterion evidence",
                ));
            }
            if input.confirmed_by != "learner" {
                return Err(Error::new(
                    "learner_confirmation_required",
                    "only explicit learner confirmation completes a goal",
                ));
            }
            let timestamp = now(&tx)?;
            tx.execute(
                "UPDATE tutor_goals SET status='completed',revision=?2,
                   updated_at=?3,completed_at=?3 WHERE id=?1",
                params![id, if_revision + 1, timestamp],
            )?;
            let goal = goal(&tx, &id)?;
            append_goal_history(&tx, &goal)?;
            let value = envelope(Plugin::Tutor, "goal.complete", json!({"goal": goal}));
            remember(&tx, &input.request_id, &fingerprint, &value)?;
            tx.commit()?;
            Ok(value)
        }
    }
}

fn require_goal_revision(goal: &Value, expected: i64) -> Result<()> {
    if expected < 1 {
        return Err(Error::new("invalid_input", "if_revision must be positive"));
    }
    let current = goal["revision"].as_i64().unwrap();
    if current == expected {
        Ok(())
    } else {
        Err(Error::new("revision_conflict", "goal revision is stale")
            .details(json!({"expected": expected, "current": current})))
    }
}

fn goal(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    let row = connection
        .query_row(
            "SELECT id,subject_id,statement,status,revision,created_at,updated_at,completed_at
             FROM tutor_goals WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("goal_not_found", format!("goal {id} was not found")))?;
    let mut statement = connection.prepare(
        "SELECT c.id,c.ordinal,c.description,
                (SELECT e.evidence_refs_json FROM tutor_goal_evidence e
                 WHERE e.goal_id=c.goal_id AND e.criterion_id=c.id
                 ORDER BY e.goal_revision DESC LIMIT 1)
         FROM tutor_goal_criteria c WHERE c.goal_id=?1 ORDER BY c.ordinal",
    )?;
    let criteria = statement
        .query_map([id], |row| {
            let evidence = row.get::<_, Option<String>>(3)?;
            let evidence = evidence
                .map(|value| serde_json::from_str::<Value>(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "ordinal": row.get::<_, i64>(1)?,
                "description": row.get::<_, String>(2)?,
                "evidence_refs": evidence,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({
        "id": row.0,
        "subject_id": row.1,
        "statement": row.2,
        "status": row.3,
        "revision": row.4,
        "criteria": criteria,
        "created_at": row.5,
        "updated_at": row.6,
        "completed_at": row.7,
    }))
}

fn append_goal_history(tx: &rusqlite::Transaction<'_>, goal: &Value) -> Result<()> {
    tx.execute(
        "INSERT INTO tutor_goal_history(goal_id,revision,snapshot_json,changed_at)
         VALUES(?1,?2,?3,?4)",
        params![
            goal["id"].as_str().unwrap(),
            goal["revision"].as_i64().unwrap(),
            serde_json::to_string(goal)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            goal["updated_at"].as_str().unwrap(),
        ],
    )?;
    Ok(())
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
