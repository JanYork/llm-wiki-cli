use crate::learning::*;
use crate::tutor_plan::PlanCommand;
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path, sync::atomic::Ordering};

const SOUL_DEFAULT_MAX_BYTES: usize = 64 * 1024;
const SOUL_HARD_MAX_BYTES: usize = 256 * 1024;
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
    Diagnosis {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
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
    Takeover {
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
    History,
    Configure {
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Propose {
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Approve {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    PublishProposal {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
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
struct DiagnosisRecord {
    outcome: String,
    reason: String,
    evidence_refs: Vec<String>,
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
    owner: String,
    reply: String,
    checkpoint: Value,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TurnTakeover {
    entity_id: String,
    old_owner: String,
    new_owner: String,
    if_revision: i64,
    sync_session_id: String,
    request_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeachingCheckpoint {
    kind: String,
    blocked_by: String,
    hint_level: i64,
    learner_attempted: bool,
    explicit_answer_request: bool,
    full_answer: bool,
    feedback_evidence_refs: Vec<String>,
    #[serde(default)]
    praise: Option<String>,
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
struct SoulConfigure {
    max_bytes: usize,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoulProposalCreate {
    body: String,
    #[serde(default)]
    fact_refs: Vec<String>,
    reason: String,
    sensitivity: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoulApproval {
    approved_by: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SoulProposalPublish {
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
    let existing = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN (
           'tutor_sessions','tutor_diagnoses','tutor_turns','tutor_turn_owner_history',
           'soul_versions','soul_settings','soul_settings_history','soul_proposals',
           'learner_facts','learner_fact_history','tutor_goals','tutor_goal_criteria',
           'tutor_goal_evidence','tutor_goal_history','tutor_plans','tutor_plan_versions',
           'tutor_plan_steps','tutor_plan_step_history')",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if existing != 0 {
        validate_tutor_schema(connection)?;
    }
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
         CREATE TABLE IF NOT EXISTS tutor_diagnoses(
           id TEXT PRIMARY KEY,
           session_id TEXT NOT NULL REFERENCES tutor_sessions(id),
           outcome TEXT NOT NULL CHECK(outcome IN ('completed','shortened','skipped')),
           reason TEXT NOT NULL CHECK(trim(reason)<>''),
           evidence_refs_json TEXT NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS tutor_diagnoses_session
           ON tutor_diagnoses(session_id,created_at,id);
         CREATE TABLE IF NOT EXISTS tutor_turns(
           id TEXT PRIMARY KEY,
           session_id TEXT NOT NULL REFERENCES tutor_sessions(id),
           owner TEXT NOT NULL,
           input TEXT NOT NULL,
           reply TEXT,
           checkpoint_json TEXT,
           checkpoint_kind TEXT,
           hint_level INTEGER,
           full_answer INTEGER,
           state TEXT NOT NULL CHECK(state IN ('pending','committed')),
           revision INTEGER NOT NULL,
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           committed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS tutor_turns_pending
           ON tutor_turns(session_id,state,created_at,id);
         CREATE TABLE IF NOT EXISTS tutor_turn_owner_history(
           turn_id TEXT NOT NULL REFERENCES tutor_turns(id),
           revision INTEGER NOT NULL CHECK(revision>=2),
           old_owner TEXT NOT NULL,
           new_owner TEXT NOT NULL,
           sync_session_id TEXT NOT NULL,
           changed_at TEXT NOT NULL,
           PRIMARY KEY(turn_id,revision)
         );
         CREATE INDEX IF NOT EXISTS tutor_turn_owner_history_session
           ON tutor_turn_owner_history(sync_session_id,turn_id,revision);
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
         CREATE TABLE IF NOT EXISTS soul_settings(
           singleton INTEGER PRIMARY KEY CHECK(singleton=1),
           max_bytes INTEGER NOT NULL CHECK(max_bytes>=65536 AND max_bytes<=262144),
           revision INTEGER NOT NULL CHECK(revision>=1),
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS soul_settings_history(
           revision INTEGER PRIMARY KEY,
           max_bytes INTEGER NOT NULL,
           changed_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS soul_proposals(
           id TEXT PRIMARY KEY,
           base_revision INTEGER NOT NULL REFERENCES soul_versions(revision),
           body TEXT NOT NULL,
           body_hash TEXT NOT NULL,
           fact_refs_json TEXT NOT NULL,
           reason TEXT NOT NULL,
           sensitivity TEXT NOT NULL CHECK(sensitivity IN (
             'ordinary','sensitive','behavior-changing'
           )),
           state TEXT NOT NULL CHECK(state IN ('proposed','approved','published')),
           approved_by TEXT,
           published_revision INTEGER REFERENCES soul_versions(revision),
           revision INTEGER NOT NULL CHECK(revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
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
    if connection
        .query_row("SELECT 1 FROM soul_settings WHERE singleton=1", [], |_| {
            Ok(())
        })
        .optional()?
        .is_none()
    {
        let timestamp = now(connection)?;
        connection.execute(
            "INSERT INTO soul_settings(singleton,max_bytes,revision,updated_at)
             VALUES(1,?1,1,?2)",
            params![SOUL_DEFAULT_MAX_BYTES as i64, timestamp],
        )?;
        connection.execute(
            "INSERT INTO soul_settings_history(revision,max_bytes,changed_at)
             VALUES(1,?1,?2)",
            params![SOUL_DEFAULT_MAX_BYTES as i64, timestamp],
        )?;
    }
    crate::tutor_plan::initialize(connection)?;
    validate_tutor_schema(connection)?;
    materialize_soul(connection, root)?;
    materialize_fact_wiki(connection, root)
}

fn validate_tutor_schema(connection: &Connection) -> Result<()> {
    for (table, columns) in [
        ("tutor_sessions", &["id","subject_id","mode","state","revision","created_at","updated_at"][..]),
        ("tutor_diagnoses", &["id","session_id","outcome","reason","evidence_refs_json","created_at"]),
        ("tutor_turns", &["id","session_id","owner","input","reply","checkpoint_json","checkpoint_kind","hint_level","full_answer","state","revision","created_at","updated_at","committed_at"][..]),
        ("tutor_turn_owner_history", &["turn_id","revision","old_owner","new_owner","sync_session_id","changed_at"]),
        ("soul_versions", &["revision","body","body_hash","fact_refs_json","reason","sensitivity","approved","created_at"]),
        ("soul_settings", &["singleton","max_bytes","revision","updated_at"]),
        ("soul_settings_history", &["revision","max_bytes","changed_at"]),
        ("soul_proposals", &["id","base_revision","body","body_hash","fact_refs_json","reason","sensitivity","state","approved_by","published_revision","revision","created_at","updated_at"][..]),
        ("learner_facts", &["id","scope","subject_id","claim","status","confidence","evidence_refs_json","origin","supersedes_id","revision","created_at","updated_at"][..]),
        ("learner_fact_history", &["fact_id","revision","snapshot_json","changed_at"]),
        ("tutor_goals", &["id","subject_id","statement","status","revision","created_at","updated_at","completed_at"]),
        ("tutor_goal_criteria", &["id","goal_id","ordinal","description"]),
        ("tutor_goal_evidence", &["goal_id","criterion_id","evidence_refs_json","goal_revision","created_at"]),
        ("tutor_goal_history", &["goal_id","revision","snapshot_json","changed_at"]),
        ("tutor_plans", &["id","subject_id","goal_id","mode","status","current_revision","created_at","updated_at"]),
        ("tutor_plan_versions", &["plan_id","revision","goal_id","deadline","weekly_minutes","core_content_json","order_json","pace","method","exercise_ratio","actor","trigger","reason","evidence_refs_json","rolled_back_to","created_at"][..]),
        ("tutor_plan_steps", &["id","plan_id","ordinal","title","estimated_minutes","status","practice_target_kind","practice_target_id","revision","created_at","updated_at"][..]),
        ("tutor_plan_step_history", &["step_id","revision","status","actor","reason","evidence_refs_json","created_at"]),
    ] {
        require_table_schema(connection, table, columns)?;
    }
    require_indexes(
        connection,
        &[
            "tutor_sessions_subject",
            "tutor_diagnoses_session",
            "tutor_turns_pending",
            "tutor_turn_owner_history_session",
            "learner_facts_scope",
            "tutor_goals_subject",
            "tutor_plans_subject",
            "tutor_plan_steps_plan",
        ],
    )?;
    let invalid = connection.query_row(
        "SELECT
           (SELECT COUNT(*) FROM tutor_sessions WHERE mode NOT IN ('learning','question','exam') OR state NOT IN ('active','closed')) +
           (SELECT COUNT(*) FROM tutor_turns WHERE state NOT IN ('pending','committed')) +
           (SELECT COUNT(*) FROM soul_proposals WHERE state NOT IN ('proposed','approved','published')) +
           (SELECT COUNT(*) FROM learner_facts WHERE status NOT IN ('provisional','confirmed','superseded')) +
           (SELECT COUNT(*) FROM tutor_goals WHERE status NOT IN ('active','ready_to_complete','completed','paused','abandoned')) +
           (SELECT COUNT(*) FROM tutor_plans WHERE status NOT IN ('active','completed','abandoned')) +
           (SELECT COUNT(*) FROM tutor_plan_steps WHERE status NOT IN ('planned','in_progress','completed','missed','deferred','skipped'))",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if invalid != 0 {
        return Err(Error::new(
            "corrupt_store",
            "Tutor store contains an unsupported future state",
        ));
    }
    Ok(())
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
        SessionCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "session.show",
            json!({"session": session(&store.connection, &id)?}),
        )),
        SessionCommand::Diagnosis {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            if if_revision < 1 {
                return Err(Error::new("invalid_input", "if_revision must be positive"));
            }
            let input: DiagnosisRecord = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_text("reason", &input.reason, 16 * 1024)?;
            if !matches!(
                input.outcome.as_str(),
                "completed" | "shortened" | "skipped"
            ) {
                return Err(Error::new(
                    "invalid_input",
                    "diagnosis outcome must be completed, shortened, or skipped",
                ));
            }
            let evidence = normalized_refs(input.evidence_refs.clone())?;
            let fingerprint = fingerprint(&json!({
                "session_id": id,
                "if_revision": if_revision,
                "diagnosis": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = session(&tx, &id)?;
            if current["revision"] != if_revision {
                return Err(Error::new("revision_conflict", "session revision is stale")
                    .details(json!({"expected": if_revision, "current": current["revision"]})));
            }
            let diagnosis_id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO tutor_diagnoses(
                   id,session_id,outcome,reason,evidence_refs_json,created_at
                 ) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    diagnosis_id,
                    id,
                    input.outcome,
                    input.reason,
                    serde_json::to_string(&evidence)
                        .map_err(|error| Error::new("json_error", error.to_string()))?,
                    timestamp,
                ],
            )?;
            tx.execute(
                "UPDATE tutor_sessions SET revision=revision+1,updated_at=?2 WHERE id=?1",
                params![id, timestamp],
            )?;
            let diagnosis = json!({
                "id": diagnosis_id,
                "session_id": id,
                "outcome": input.outcome,
                "reason": input.reason,
                "evidence_refs": evidence,
                "created_at": timestamp,
            });
            let value = envelope(
                Plugin::Tutor,
                "session.diagnosis",
                json!({"diagnosis": diagnosis, "session": session(&tx, &id)?}),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
            validate_text("owner", &input.owner, 256)?;
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
            if current["owner"] != input.owner {
                return Err(Error::new(
                    "stale_owner",
                    "only the current turn owner may commit",
                ));
            }
            let teaching = validate_teaching_checkpoint(&tx, &current, &input.checkpoint)?;
            let checkpoint = serde_json::to_string(&input.checkpoint)
                .map_err(|error| Error::new("json_error", error.to_string()))?;
            let timestamp = now(&tx)?;
            let changed = tx.execute(
                "UPDATE tutor_turns
                 SET reply=?2,checkpoint_json=?3,checkpoint_kind=?4,hint_level=?5,
                     full_answer=?6,state='committed',revision=revision+1,
                     updated_at=?7,committed_at=?7
                 WHERE id=?1 AND owner=?8 AND revision=?9 AND state='pending'",
                params![
                    id,
                    input.reply,
                    checkpoint,
                    teaching.as_ref().map(|value| value.kind.as_str()),
                    teaching.as_ref().map(|value| value.hint_level),
                    teaching.as_ref().map(|value| value.full_answer),
                    timestamp,
                    input.owner,
                    if_revision,
                ],
            )?;
            if changed != 1 {
                return Err(Error::new(
                    "revision_conflict",
                    "turn changed during commit",
                ));
            }
            let value = envelope(
                Plugin::Tutor,
                "turn.commit",
                json!({"turn": turn(&tx, &id)?}),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
        TurnCommand::Takeover { json } => takeover_turn(store, read_json(&json)?),
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

fn takeover_turn(store: &mut Store, input: TurnTakeover) -> Result<Value> {
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
    require_latest_sync_receipt(&tx, Plugin::Tutor, &input.sync_session_id)?;
    let timestamp = now(&tx)?;
    let changed = tx.execute(
        "UPDATE tutor_turns SET owner=?2,revision=revision+1,updated_at=?3
         WHERE id=?1 AND owner=?4 AND revision=?5 AND state='pending'",
        params![
            input.entity_id,
            input.new_owner,
            timestamp,
            input.old_owner,
            input.if_revision,
        ],
    )?;
    if changed != 1 {
        let current = turn(&tx, &input.entity_id)?;
        if current["owner"] != input.old_owner {
            return Err(Error::new("stale_owner", "turn owner changed"));
        }
        if current["state"] != "pending" {
            return Err(Error::new(
                "turn_committed",
                "committed Tutor turns cannot be taken over",
            ));
        }
        return Err(Error::new(
            "revision_conflict",
            "turn revision changed",
        ));
    }
    tx.execute(
        "INSERT INTO tutor_turn_owner_history(
           turn_id,revision,old_owner,new_owner,sync_session_id,changed_at
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
    let value = finalize_mutation(
        &tx,
        Plugin::Tutor,
        &input.request_id,
        &fingerprint,
        envelope(
            Plugin::Tutor,
            "turn.takeover",
            json!({"turn":turn(&tx,&input.entity_id)?}),
        ),
    )?;
    tx.commit()?;
    Ok(value)
}

fn validate_teaching_checkpoint(
    connection: &Connection,
    turn: &Value,
    checkpoint: &Value,
) -> Result<Option<TeachingCheckpoint>> {
    if checkpoint.get("kind").and_then(Value::as_str) != Some("teaching") {
        return Ok(None);
    }
    let checkpoint: TeachingCheckpoint = serde_json::from_value(checkpoint.clone())
        .map_err(|error| Error::new("invalid_input", error.to_string()))?;
    validate_text("blocked_by", &checkpoint.blocked_by, 16 * 1024)?;
    if checkpoint.kind != "teaching" || !(0..=32).contains(&checkpoint.hint_level) {
        return Err(Error::new(
            "invalid_input",
            "teaching hint_level must be within 0..=32",
        ));
    }
    let session_id = turn["session_id"].as_str().unwrap();
    let mode = connection.query_row(
        "SELECT mode FROM tutor_sessions WHERE id=?1",
        [session_id],
        |row| row.get::<_, String>(0),
    )?;
    if mode == "exam" && checkpoint.hint_level > 0 {
        return Err(Error::new(
            "exam_hints_forbidden",
            "exam sessions do not permit hints",
        ));
    }
    let previous = connection.query_row(
        "SELECT COALESCE(MAX(hint_level),0) FROM tutor_turns
         WHERE session_id=?1 AND state='committed' AND checkpoint_kind='teaching'",
        [session_id],
        |row| row.get::<_, i64>(0),
    )?;
    if checkpoint.hint_level > previous + 1 {
        return Err(Error::new(
            "hint_level_not_progressive",
            "teaching hints may advance by only one level at a time",
        ));
    }
    if checkpoint.full_answer
        && !checkpoint.learner_attempted
        && !checkpoint.explicit_answer_request
    {
        return Err(Error::new(
            "full_answer_not_allowed",
            "a full answer requires a learner attempt or explicit request",
        ));
    }
    let evidence = normalized_refs(checkpoint.feedback_evidence_refs.clone()).or_else(|error| {
        if checkpoint.praise.is_none() && checkpoint.feedback_evidence_refs.is_empty() {
            Ok(Vec::new())
        } else {
            Err(error)
        }
    })?;
    if let Some(praise) = &checkpoint.praise {
        validate_text("praise", praise, 16 * 1024)?;
        if evidence.is_empty() {
            return Err(Error::new(
                "feedback_evidence_required",
                "praise and feedback must cite observed evidence",
            ));
        }
    }
    Ok(Some(checkpoint))
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
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
        SoulCommand::History => Ok(envelope(
            Plugin::Tutor,
            "soul.history",
            soul_history(&store.connection)?,
        )),
        SoulCommand::Configure { if_revision, json } => {
            let input: SoulConfigure = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            if !(SOUL_DEFAULT_MAX_BYTES..=SOUL_HARD_MAX_BYTES).contains(&input.max_bytes) {
                return Err(Error::new(
                    "invalid_input",
                    format!(
                        "Soul max_bytes must be within {SOUL_DEFAULT_MAX_BYTES}..={SOUL_HARD_MAX_BYTES}"
                    ),
                ));
            }
            let fingerprint = fingerprint(&json!({
                "if_revision": if_revision,
                "configure": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            let current = soul_settings(&tx)?;
            if current["revision"] != if_revision {
                return Err(
                    Error::new("revision_conflict", "Soul settings revision is stale")
                        .details(json!({"expected": if_revision, "current": current["revision"]})),
                );
            }
            let body_bytes = soul(&tx)?["body_bytes"].as_u64().unwrap() as usize;
            if input.max_bytes < body_bytes {
                return Err(Error::new(
                    "soul_budget_below_current",
                    "Soul max_bytes cannot be smaller than the current body",
                ));
            }
            let revision = if_revision + 1;
            let max_bytes = input.max_bytes as i64;
            let timestamp = now(&tx)?;
            tx.execute(
                "UPDATE soul_settings SET max_bytes=?1,revision=?2,updated_at=?3
                 WHERE singleton=1",
                params![max_bytes, revision, timestamp],
            )?;
            tx.execute(
                "INSERT INTO soul_settings_history(revision,max_bytes,changed_at)
                 VALUES(?1,?2,?3)",
                params![revision, max_bytes, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "soul.configure",
                json!({"settings": soul_settings(&tx)?}),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
        SoulCommand::Propose { if_revision, json } => {
            let input: SoulProposalCreate = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_soul_content(
                &store.connection,
                &input.body,
                &input.fact_refs,
                &input.reason,
                &input.sensitivity,
            )?;
            let fingerprint = fingerprint(&json!({
                "if_revision": if_revision,
                "proposal": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            require_soul_revision(&tx, if_revision)?;
            let id = new_id(Plugin::Tutor, &input.request_id);
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO soul_proposals(
                   id,base_revision,body,body_hash,fact_refs_json,reason,sensitivity,
                   state,approved_by,published_revision,revision,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,'proposed',NULL,NULL,1,?8,?8)",
                params![
                    id,
                    if_revision,
                    input.body,
                    sha256(input.body.as_bytes()),
                    serde_json::to_string(&input.fact_refs)
                        .map_err(|error| Error::new("json_error", error.to_string()))?,
                    input.reason,
                    input.sensitivity,
                    timestamp,
                ],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "soul.propose",
                json!({"proposal": soul_proposal(&tx, &id)?}),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
        SoulCommand::Approve {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            let input: SoulApproval = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            if input.approved_by != "learner" {
                return Err(Error::new(
                    "learner_confirmation_required",
                    "Soul approval must come from the learner",
                ));
            }
            let fingerprint = fingerprint(&json!({
                "id": id,
                "if_revision": if_revision,
                "approval": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            require_soul_revision(&tx, if_revision)?;
            let proposal = soul_proposal(&tx, &id)?;
            if proposal["base_revision"] != if_revision || proposal["state"] != "proposed" {
                return Err(Error::new(
                    "soul_proposal_conflict",
                    "Soul proposal is stale or is not awaiting approval",
                ));
            }
            let timestamp = now(&tx)?;
            tx.execute(
                "UPDATE soul_proposals SET state='approved',approved_by=?2,
                   revision=revision+1,updated_at=?3 WHERE id=?1",
                params![id, input.approved_by, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "soul.approve",
                json!({"proposal": soul_proposal(&tx, &id)?}),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            Ok(value)
        }
        SoulCommand::PublishProposal {
            id,
            if_revision,
            json,
        } => {
            validate_id(&id)?;
            let input: SoulProposalPublish = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            let fingerprint = fingerprint(&json!({
                "id": id,
                "if_revision": if_revision,
                "publish": &input,
            }))?;
            let tx = store
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
                return Ok(value);
            }
            require_soul_revision(&tx, if_revision)?;
            let proposal = soul_proposal(&tx, &id)?;
            if proposal["base_revision"] != if_revision || proposal["state"] == "published" {
                return Err(Error::new(
                    "soul_proposal_conflict",
                    "Soul proposal is stale or already published",
                ));
            }
            if proposal["sensitivity"] != "ordinary" && proposal["state"] != "approved" {
                return Err(Error::new(
                    "soul_approval_required",
                    "sensitive or behavior-changing Soul proposals require learner approval",
                ));
            }
            let revision = if_revision + 1;
            let timestamp = now(&tx)?;
            tx.execute(
                "INSERT INTO soul_versions(
                   revision,body,body_hash,fact_refs_json,reason,sensitivity,approved,created_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    revision,
                    proposal["body"].as_str().unwrap(),
                    proposal["body_sha256"].as_str().unwrap(),
                    serde_json::to_string(&proposal["fact_refs"])
                        .map_err(|error| Error::new("json_error", error.to_string()))?,
                    proposal["reason"].as_str().unwrap(),
                    proposal["sensitivity"].as_str().unwrap(),
                    proposal["state"] == "approved",
                    timestamp,
                ],
            )?;
            tx.execute(
                "UPDATE soul_proposals SET state='published',published_revision=?2,
                   revision=revision+1,updated_at=?3 WHERE id=?1",
                params![id, revision, timestamp],
            )?;
            let value = envelope(
                Plugin::Tutor,
                "soul.publish-proposal",
                json!({
                    "soul": soul(&tx)?,
                    "proposal": soul_proposal(&tx, &id)?,
                }),
            );
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            materialize_soul(&store.connection, &store.root)?;
            Ok(value)
        }
        SoulCommand::Publish { if_revision, json } => {
            let input: SoulPublish = read_json(&json)?;
            validate_request_id(&input.request_id)?;
            validate_soul_content(
                &store.connection,
                &input.body,
                &input.fact_refs,
                &input.reason,
                &input.sensitivity,
            )?;
            if input.sensitivity != "ordinary" && !input.approved {
                return Err(Error::new(
                    "soul_approval_required",
                    "sensitive or behavior-changing Soul revisions require learner approval",
                ));
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
            let value =
                finalize_mutation(&tx, Plugin::Tutor, &input.request_id, &fingerprint, value)?;
            tx.commit()?;
            materialize_soul(&store.connection, &store.root)?;
            Ok(value)
        }
    }
}

fn validate_soul_content(
    connection: &Connection,
    body: &str,
    fact_refs: &[String],
    reason: &str,
    sensitivity: &str,
) -> Result<()> {
    if body.trim().is_empty() {
        return Err(Error::new("invalid_input", "Soul body must not be empty"));
    }
    let max_bytes = soul_limit(connection)?;
    if body.len() > max_bytes {
        return Err(Error::new(
            "soul_too_large",
            format!("Soul body exceeds {max_bytes} UTF-8 bytes"),
        ));
    }
    validate_text("reason", reason, 4096)?;
    if !matches!(sensitivity, "ordinary" | "sensitive" | "behavior-changing") {
        return Err(Error::new("invalid_input", "invalid Soul sensitivity"));
    }
    if fact_refs.len() > 1024
        || fact_refs
            .iter()
            .any(|reference| reference.trim().is_empty())
    {
        return Err(Error::new("invalid_input", "Soul fact_refs are invalid"));
    }
    Ok(())
}

fn require_soul_revision(connection: &Connection, expected: i64) -> Result<()> {
    let current = soul(connection)?["revision"].as_i64().unwrap();
    if current == expected {
        Ok(())
    } else {
        Err(Error::new("revision_conflict", "Soul revision is stale")
            .details(json!({"expected": expected, "current": current})))
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
        "max_bytes": soul_limit(connection)?,
        "fact_refs": fact_refs,
        "reason": row.4,
        "sensitivity": row.5,
        "approved": row.6,
        "created_at": row.7,
    }))
}

fn soul_limit(connection: &Connection) -> Result<usize> {
    connection
        .query_row(
            "SELECT max_bytes FROM soul_settings WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value as usize)
        .map_err(Into::into)
}

fn soul_settings(connection: &Connection) -> Result<Value> {
    connection
        .query_row(
            "SELECT max_bytes,revision,updated_at FROM soul_settings WHERE singleton=1",
            [],
            |row| {
                Ok(json!({
                    "max_bytes": row.get::<_, i64>(0)?,
                    "revision": row.get::<_, i64>(1)?,
                    "updated_at": row.get::<_, String>(2)?,
                }))
            },
        )
        .map_err(Into::into)
}

fn soul_proposal(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    let row = connection
        .query_row(
            "SELECT id,base_revision,body,body_hash,fact_refs_json,reason,sensitivity,
                    state,approved_by,published_revision,revision,created_at,updated_at
             FROM soul_proposals WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            Error::new(
                "soul_proposal_not_found",
                format!("Soul proposal {id} was not found"),
            )
        })?;
    let refs: Value = serde_json::from_str(&row.4)
        .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
    Ok(json!({
        "id": row.0,
        "base_revision": row.1,
        "body": row.2,
        "body_sha256": row.3,
        "body_bytes": row.2.len(),
        "fact_refs": refs,
        "reason": row.5,
        "sensitivity": row.6,
        "state": row.7,
        "approved_by": row.8,
        "published_revision": row.9,
        "revision": row.10,
        "created_at": row.11,
        "updated_at": row.12,
    }))
}

fn soul_history(connection: &Connection) -> Result<Value> {
    let mut versions =
        connection.prepare("SELECT revision FROM soul_versions ORDER BY revision")?;
    let revisions = versions
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let versions = revisions
        .into_iter()
        .map(|revision| {
            let row = connection.query_row(
                "SELECT body,body_hash,fact_refs_json,reason,sensitivity,approved,created_at
                 FROM soul_versions WHERE revision=?1",
                [revision],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, bool>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )?;
            let refs: Value = serde_json::from_str(&row.2)
                .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
            Ok(json!({
                "revision": revision,
                "body": row.0,
                "body_sha256": row.1,
                "fact_refs": refs,
                "reason": row.3,
                "sensitivity": row.4,
                "approved": row.5,
                "created_at": row.6,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut proposals =
        connection.prepare("SELECT id FROM soul_proposals ORDER BY created_at,id")?;
    let proposal_ids = proposals
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let proposals = proposal_ids
        .iter()
        .map(|id| soul_proposal(connection, id))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"versions": versions, "proposals": proposals}))
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

#[cfg(test)]
mod takeover_tests {
    use super::*;

    #[test]
    fn latest_real_receipt_allows_one_takeover_then_becomes_stale() {
        let home = tempfile::tempdir().unwrap();
        // This binary test process owns HOME; Store::open therefore exercises the real
        // shared migration and receipt schema without a hand-built drift fixture.
        unsafe { std::env::set_var("HOME", home.path()) };
        let mut store = Store::open(Plugin::Tutor).unwrap();
        initialize(&store.connection, &store.root).unwrap();
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "INSERT INTO subjects(id,name,parent_id,tags_json,revision,created_at,updated_at)
             VALUES('11111111111111111111111111111111','s',NULL,'[]',1,
                    '2026-08-24T00:00:00.000Z','2026-08-24T00:00:00.000Z')",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO tutor_sessions(id,subject_id,mode,state,revision,created_at,updated_at)
             VALUES('22222222222222222222222222222222','11111111111111111111111111111111',
                    'learning','active',1,'2026-08-24T00:00:00.000Z','2026-08-24T00:00:00.000Z')",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO tutor_turns(id,session_id,owner,input,state,revision,created_at,updated_at)
             VALUES('33333333333333333333333333333333','22222222222222222222222222222222',
                    'mac-old','pending','pending',1,'2026-08-24T00:00:00.000Z','2026-08-24T00:00:00.000Z')",
            [],
        )
        .unwrap();
        bump_store_revision(&tx).unwrap();
        tx.commit().unwrap();

        let identity = store.identity(Plugin::Tutor).unwrap();
        let logical_hash = canonical_logical_hash(Plugin::Tutor, &store.connection).unwrap();
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        for session in ["sync-z-old", "sync-a-latest"] {
            record_sync_receipt(
                &tx,
                Plugin::Tutor,
                session,
                identity.revision,
                identity.revision,
                &logical_hash,
                "ready",
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let stale = takeover_turn(
            &mut store,
            TurnTakeover {
                entity_id: "33333333333333333333333333333333".into(),
                old_owner: "mac-old".into(),
                new_owner: "mac-new".into(),
                if_revision: 1,
                sync_session_id: "sync-z-old".into(),
                request_id: "takeover-old-receipt".into(),
            },
        )
        .unwrap_err();
        assert_eq!(stale.code(), "stale_sync_receipt");

        let taken = takeover_turn(
            &mut store,
            TurnTakeover {
                entity_id: "33333333333333333333333333333333".into(),
                old_owner: "mac-old".into(),
                new_owner: "mac-new".into(),
                if_revision: 1,
                sync_session_id: "sync-a-latest".into(),
                request_id: "takeover-latest-receipt".into(),
            },
        )
        .unwrap();
        assert_eq!(taken["result"]["turn"]["owner"], "mac-new");
        assert_eq!(taken["result"]["turn"]["revision"], 2);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM tutor_turn_owner_history WHERE turn_id=?1",
                    ["33333333333333333333333333333333"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        let post_sync = takeover_turn(
            &mut store,
            TurnTakeover {
                entity_id: "33333333333333333333333333333333".into(),
                old_owner: "mac-new".into(),
                new_owner: "mac-third".into(),
                if_revision: 2,
                sync_session_id: "sync-a-latest".into(),
                request_id: "takeover-post-sync-mutation".into(),
            },
        )
        .unwrap_err();
        assert_eq!(post_sync.code(), "stale_sync_receipt");

        let old_owner_commit = run_turn(
            &mut store,
            TurnCommand::Commit {
                id: "33333333333333333333333333333333".into(),
                if_revision: 2,
                json: serde_json::json!({
                    "owner":"mac-old","reply":"stale","checkpoint":{"kind":"checkpoint"},
                    "request_id":"old-owner-commit"
                })
                .to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(old_owner_commit.code(), "stale_owner");
    }
}
