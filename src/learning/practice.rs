use crate::learning::*;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use rs_fsrs::{Card, FSRS, Parameters, Rating};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const SCHEDULER_CRATE: &str = "rs-fsrs";
const SCHEDULER_VERSION: &str = "1.2.1";
const MAX_BLUEPRINT_CANDIDATES: usize = 256;
const MAX_BLUEPRINT_SEARCH_STATES: usize = 5_000;

#[derive(Parser)]
struct PracticeCli {
    #[command(subcommand)]
    command: PracticeCommand,
}

#[derive(Subcommand)]
enum PracticeCommand {
    Subject {
        #[command(subcommand)]
        command: SubjectCommand,
    },
    Bank {
        #[command(subcommand)]
        command: BankCommand,
    },
    Item {
        #[command(subcommand)]
        command: ItemCommand,
    },
    Set {
        #[command(subcommand)]
        command: SetCommand,
    },
    Paper {
        #[command(subcommand)]
        command: PaperCommand,
    },
    Attempt {
        #[command(subcommand)]
        command: AttemptCommand,
    },
    Response {
        #[command(subcommand)]
        command: ResponseCommand,
    },
    Grade {
        #[command(subcommand)]
        command: GradeCommand,
    },
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    Next {
        #[arg(long)]
        json: String,
    },
    Status,
}

#[derive(Subcommand)]
enum BankCommand {
    Create {
        #[arg(long)]
        json: String,
    },
    Add {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum ItemCommand {
    Create {
        #[arg(long)]
        json: String,
    },
    Verify {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Retire {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum SetCommand {
    Create {
        #[arg(long)]
        json: String,
    },
    Add {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Resolve {
        id: String,
        item_id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Reopen {
        id: String,
        item_id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Archive {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum PaperCommand {
    Create {
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum AttemptCommand {
    Create {
        #[arg(long)]
        json: String,
    },
    Submit {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Abandon {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Takeover {
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum ResponseCommand {
    Save {
        attempt_id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        json: String,
    },
}

#[derive(Subcommand)]
enum GradeCommand {
    Objective {
        response_id: String,
        #[arg(long)]
        json: String,
    },
    Subjective {
        response_id: String,
        #[arg(long)]
        json: String,
    },
    Override {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        json: String,
    },
    Show {
        id: String,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    Rate {
        item_id: String,
        #[arg(long)]
        json: String,
    },
    Configure {
        #[arg(long)]
        json: String,
    },
    Control {
        #[arg(long)]
        subject: String,
    },
    Queue {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        goal_bank: Option<String>,
        #[arg(long)]
        budget_minutes: i64,
        #[arg(long)]
        now: String,
    },
    Show {
        item_id: String,
    },
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SourceRef {
    kind: String,
    id: String,
    revision_or_hash: String,
    #[serde(default)]
    locator: Option<String>,
    #[serde(default)]
    subject_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BankCreate {
    subject_id: String,
    key: String,
    title: String,
    source: SourceRef,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BankAdd {
    item_id: String,
    item_revision: i64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ItemCreate {
    subject_id: String,
    item_type: String,
    grading_kind: String,
    prompt: String,
    answer: Value,
    #[serde(default)]
    rubric: Option<String>,
    max_points: f64,
    estimated_minutes: i64,
    difficulty: f64,
    #[serde(default)]
    topic: Option<String>,
    source: SourceRef,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ItemVerify {
    prompt: String,
    answer: Value,
    #[serde(default)]
    rubric: Option<String>,
    source: SourceRef,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetCreate {
    subject_id: String,
    name: String,
    kind: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetAdd {
    item_id: String,
    item_revision: i64,
    reason: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetTransition {
    reason: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PaperCreate {
    bank_id: String,
    count: i64,
    duration_minutes: i64,
    #[serde(default)]
    difficulty_min: Option<f64>,
    #[serde(default)]
    difficulty_max: Option<f64>,
    #[serde(default)]
    source_kind: Option<String>,
    #[serde(default)]
    item_type_counts: BTreeMap<String, i64>,
    #[serde(default)]
    topic_counts: BTreeMap<String, i64>,
    #[serde(default)]
    section_counts: BTreeMap<String, i64>,
    #[serde(default)]
    total_points: Option<f64>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptCreate {
    paper_id: String,
    owner: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptTransition {
    owner: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptTakeover {
    entity_id: String,
    old_owner: String,
    new_owner: String,
    if_revision: i64,
    sync_session_id: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResponseSave {
    paper_item_id: String,
    format: String,
    value: Value,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GradeRequest {
    response_revision: i64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubjectiveGrade {
    item_revision: i64,
    response_revision: i64,
    score: f64,
    rationale: String,
    confidence: f64,
    method: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GradeOverride {
    score: f64,
    reason: String,
    actor: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewRate {
    rating: u8,
    reviewed_at: String,
    estimated_minutes: i64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewControl {
    subject_id: String,
    daily_budget_minutes: i64,
    desired_retention: f64,
    actor: String,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TargetRef {
    kind: String,
    id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NextTarget {
    subject_id: String,
    #[serde(default)]
    explicit_plan_target: Option<TargetRef>,
    #[serde(default)]
    goal_bank_id: Option<String>,
    subject_default_bank_id: String,
}

pub(crate) fn main() {
    finish(run(PracticeCli::parse()));
}

fn run(cli: PracticeCli) -> Result<Value> {
    let mut store = Store::open(Plugin::Practice)?;
    initialize(&store.connection)?;
    match cli.command {
        PracticeCommand::Subject { command } => run_subject(Plugin::Practice, &mut store, command),
        PracticeCommand::Bank { command } => run_bank(&mut store, command),
        PracticeCommand::Item { command } => run_item(&mut store, command),
        PracticeCommand::Set { command } => run_set(&mut store, command),
        PracticeCommand::Paper { command } => run_paper(&mut store, command),
        PracticeCommand::Attempt { command } => run_attempt(&mut store, command),
        PracticeCommand::Response { command } => run_response(&mut store, command),
        PracticeCommand::Grade { command } => run_grade(&mut store, command),
        PracticeCommand::Review { command } => run_review(&mut store, command),
        PracticeCommand::Next { json } => resolve_target(&store.connection, read_json(&json)?),
        PracticeCommand::Status => status(&store.connection),
    }
}

fn initialize(connection: &Connection) -> Result<()> {
    let existing:i64=connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN (
          'practice_banks','practice_items','bank_items','practice_sets','set_members','set_member_events',
          'papers','paper_items','attempts','responses','response_history','grades','grade_history',
          'review_events','fsrs_cards','review_debt','review_debt_events','review_controls','attempt_takeover_history')",
        [],|row|row.get(0))?;
    if existing > 0 {
        validate_practice_schema(connection)?;
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS practice_banks(
           id TEXT PRIMARY KEY, subject_id TEXT NOT NULL REFERENCES subjects(id),
           bank_key TEXT NOT NULL, title TEXT NOT NULL CHECK(trim(title)<>''),
           source_json TEXT NOT NULL, revision INTEGER NOT NULL, created_at TEXT NOT NULL,
           UNIQUE(subject_id,bank_key));
         CREATE INDEX IF NOT EXISTS practice_banks_subject ON practice_banks(subject_id,bank_key,id);
         CREATE TABLE IF NOT EXISTS practice_items(
           id TEXT NOT NULL, revision INTEGER NOT NULL, subject_id TEXT NOT NULL REFERENCES subjects(id),
           item_type TEXT NOT NULL CHECK(item_type IN ('choice','text','numeric','flashcard')),
           grading_kind TEXT NOT NULL CHECK(grading_kind IN ('objective','subjective')),
           state TEXT NOT NULL CHECK(state IN ('draft','verified','retired')),
           prompt TEXT NOT NULL CHECK(trim(prompt)<>''), answer_json TEXT NOT NULL, rubric TEXT,
           max_points REAL NOT NULL CHECK(max_points>0), estimated_minutes INTEGER NOT NULL CHECK(estimated_minutes>0),
           difficulty REAL NOT NULL CHECK(difficulty>=0 AND difficulty<=1), topic TEXT, source_json TEXT NOT NULL,
           created_at TEXT NOT NULL, PRIMARY KEY(id,revision));
         CREATE INDEX IF NOT EXISTS practice_items_subject_state ON practice_items(subject_id,state,id,revision);
         CREATE TABLE IF NOT EXISTS bank_items(
           bank_id TEXT NOT NULL REFERENCES practice_banks(id), item_id TEXT NOT NULL,
           item_revision INTEGER NOT NULL, added_at TEXT NOT NULL,
           PRIMARY KEY(bank_id,item_id,item_revision),
           FOREIGN KEY(item_id,item_revision) REFERENCES practice_items(id,revision));
         CREATE INDEX IF NOT EXISTS bank_items_item ON bank_items(item_id,item_revision,bank_id);
         CREATE TABLE IF NOT EXISTS practice_sets(
           id TEXT PRIMARY KEY, subject_id TEXT NOT NULL REFERENCES subjects(id), name TEXT NOT NULL,
           kind TEXT NOT NULL CHECK(kind IN ('ordinary','mistake')), state TEXT NOT NULL CHECK(state IN ('active','archived')),
           revision INTEGER NOT NULL, created_at TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS practice_sets_subject ON practice_sets(subject_id,state,id);
         CREATE TABLE IF NOT EXISTS set_members(
           set_id TEXT NOT NULL REFERENCES practice_sets(id), item_id TEXT NOT NULL, item_revision INTEGER NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('active','resolved')), revision INTEGER NOT NULL,
           PRIMARY KEY(set_id,item_id,item_revision), FOREIGN KEY(item_id,item_revision) REFERENCES practice_items(id,revision));
         CREATE TABLE IF NOT EXISTS set_member_events(
           id TEXT PRIMARY KEY, set_id TEXT NOT NULL, item_id TEXT NOT NULL, item_revision INTEGER NOT NULL,
           action TEXT NOT NULL, reason TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS set_member_events_member ON set_member_events(set_id,item_id,item_revision,created_at,id);
         CREATE TABLE IF NOT EXISTS papers(
           id TEXT PRIMARY KEY, bank_id TEXT NOT NULL REFERENCES practice_banks(id), subject_id TEXT NOT NULL,
           duration_minutes INTEGER NOT NULL, revision INTEGER NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS paper_items(
           id TEXT PRIMARY KEY, paper_id TEXT NOT NULL REFERENCES papers(id), ordinal INTEGER NOT NULL, section TEXT NOT NULL,
           item_id TEXT NOT NULL, item_revision INTEGER NOT NULL, prompt TEXT NOT NULL, answer_json TEXT NOT NULL,
           rubric TEXT, source_json TEXT NOT NULL, points REAL NOT NULL, item_type TEXT NOT NULL, grading_kind TEXT NOT NULL,
           UNIQUE(paper_id,ordinal));
         CREATE INDEX IF NOT EXISTS paper_items_paper ON paper_items(paper_id,ordinal);
         CREATE TABLE IF NOT EXISTS attempts(
           id TEXT PRIMARY KEY, paper_id TEXT NOT NULL REFERENCES papers(id), owner TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('in_progress','submitted','abandoned')),
           revision INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS attempts_paper_state ON attempts(paper_id,state,id);
         CREATE TABLE IF NOT EXISTS responses(
           id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL REFERENCES attempts(id), paper_item_id TEXT NOT NULL REFERENCES paper_items(id),
           format TEXT NOT NULL CHECK(format IN ('choice','text','numeric','flashcard')), value_json TEXT NOT NULL,
           revision INTEGER NOT NULL, updated_at TEXT NOT NULL, UNIQUE(attempt_id,paper_item_id));
         CREATE TABLE IF NOT EXISTS response_history(
           response_id TEXT NOT NULL REFERENCES responses(id), revision INTEGER NOT NULL, format TEXT NOT NULL,
           value_json TEXT NOT NULL, saved_at TEXT NOT NULL, PRIMARY KEY(response_id,revision));
         CREATE TABLE IF NOT EXISTS grades(
           id TEXT PRIMARY KEY, item_id TEXT NOT NULL, item_revision INTEGER NOT NULL, response_id TEXT,
           score REAL NOT NULL, max_points REAL NOT NULL, rationale TEXT NOT NULL, confidence REAL NOT NULL,
           method TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('accepted','pending_review','overridden')),
           revision INTEGER NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS grade_history(
           grade_id TEXT NOT NULL REFERENCES grades(id), revision INTEGER NOT NULL, score REAL NOT NULL,
           rationale TEXT NOT NULL, confidence REAL NOT NULL, method TEXT NOT NULL, state TEXT NOT NULL,
           created_at TEXT NOT NULL, PRIMARY KEY(grade_id,revision));
         CREATE INDEX IF NOT EXISTS grades_item ON grades(item_id,item_revision,state,id);
         CREATE TABLE IF NOT EXISTS review_events(
           id TEXT PRIMARY KEY, item_id TEXT NOT NULL, ordinal INTEGER NOT NULL, rating INTEGER NOT NULL,
           reviewed_at TEXT NOT NULL, scheduled_days INTEGER NOT NULL, card_json TEXT NOT NULL,
           scheduler_crate TEXT NOT NULL, scheduler_version TEXT NOT NULL,
           UNIQUE(item_id,ordinal));
         CREATE INDEX IF NOT EXISTS review_events_item ON review_events(item_id,ordinal);
         CREATE TABLE IF NOT EXISTS fsrs_cards(
           item_id TEXT PRIMARY KEY, subject_id TEXT NOT NULL, card_json TEXT NOT NULL, due_at TEXT NOT NULL,
           estimated_minutes INTEGER NOT NULL, scheduler_crate TEXT NOT NULL, scheduler_version TEXT NOT NULL,
           parameters_json TEXT NOT NULL, revision INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS fsrs_cards_due ON fsrs_cards(subject_id,due_at,item_id);
         CREATE TABLE IF NOT EXISTS review_debt(
           item_id TEXT PRIMARY KEY, subject_id TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('open','served','deferred')),
           due_at TEXT NOT NULL, estimated_minutes INTEGER NOT NULL, updated_at TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS review_debt_subject ON review_debt(subject_id,state,due_at,item_id);
         CREATE TABLE IF NOT EXISTS review_debt_events(
           id TEXT PRIMARY KEY,item_id TEXT NOT NULL,subject_id TEXT NOT NULL,state TEXT NOT NULL,
           due_at TEXT NOT NULL,estimated_minutes INTEGER NOT NULL,created_at TEXT NOT NULL,
           UNIQUE(item_id,state,due_at));
         CREATE INDEX IF NOT EXISTS review_debt_events_item ON review_debt_events(item_id,created_at,id);
         CREATE TABLE IF NOT EXISTS review_controls(
           subject_id TEXT PRIMARY KEY REFERENCES subjects(id), daily_budget_minutes INTEGER NOT NULL CHECK(daily_budget_minutes>=0),
           desired_retention REAL NOT NULL CHECK(desired_retention>=0.7 AND desired_retention<=0.99),
           revision INTEGER NOT NULL, updated_at TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS attempt_takeover_history(
           id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL, old_owner TEXT NOT NULL, new_owner TEXT NOT NULL,
           sync_session_id TEXT NOT NULL, old_revision INTEGER NOT NULL, new_revision INTEGER NOT NULL, created_at TEXT NOT NULL);",
    )?;
    validate_practice_schema(connection)
}

fn validate_practice_schema(connection: &Connection) -> Result<()> {
    const TABLES: &[(&str, &[&str])] = &[
        (
            "practice_banks",
            &[
                "id",
                "subject_id",
                "bank_key",
                "title",
                "source_json",
                "revision",
                "created_at",
            ],
        ),
        (
            "practice_items",
            &[
                "id",
                "revision",
                "subject_id",
                "item_type",
                "grading_kind",
                "state",
                "prompt",
                "answer_json",
                "rubric",
                "max_points",
                "estimated_minutes",
                "difficulty",
                "topic",
                "source_json",
                "created_at",
            ],
        ),
        (
            "bank_items",
            &["bank_id", "item_id", "item_revision", "added_at"],
        ),
        (
            "practice_sets",
            &[
                "id",
                "subject_id",
                "name",
                "kind",
                "state",
                "revision",
                "created_at",
            ],
        ),
        (
            "set_members",
            &["set_id", "item_id", "item_revision", "state", "revision"],
        ),
        (
            "set_member_events",
            &[
                "id",
                "set_id",
                "item_id",
                "item_revision",
                "action",
                "reason",
                "created_at",
            ],
        ),
        (
            "papers",
            &[
                "id",
                "bank_id",
                "subject_id",
                "duration_minutes",
                "revision",
                "created_at",
            ],
        ),
        (
            "paper_items",
            &[
                "id",
                "paper_id",
                "ordinal",
                "section",
                "item_id",
                "item_revision",
                "prompt",
                "answer_json",
                "rubric",
                "source_json",
                "points",
                "item_type",
                "grading_kind",
            ],
        ),
        (
            "attempts",
            &[
                "id",
                "paper_id",
                "owner",
                "state",
                "revision",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "responses",
            &[
                "id",
                "attempt_id",
                "paper_item_id",
                "format",
                "value_json",
                "revision",
                "updated_at",
            ],
        ),
        (
            "response_history",
            &[
                "response_id",
                "revision",
                "format",
                "value_json",
                "saved_at",
            ],
        ),
        (
            "grades",
            &[
                "id",
                "item_id",
                "item_revision",
                "response_id",
                "score",
                "max_points",
                "rationale",
                "confidence",
                "method",
                "state",
                "revision",
                "created_at",
            ],
        ),
        (
            "grade_history",
            &[
                "grade_id",
                "revision",
                "score",
                "rationale",
                "confidence",
                "method",
                "state",
                "created_at",
            ],
        ),
        (
            "review_events",
            &[
                "id",
                "item_id",
                "ordinal",
                "rating",
                "reviewed_at",
                "scheduled_days",
                "card_json",
                "scheduler_crate",
                "scheduler_version",
            ],
        ),
        (
            "fsrs_cards",
            &[
                "item_id",
                "subject_id",
                "card_json",
                "due_at",
                "estimated_minutes",
                "scheduler_crate",
                "scheduler_version",
                "parameters_json",
                "revision",
            ],
        ),
        (
            "review_debt",
            &[
                "item_id",
                "subject_id",
                "state",
                "due_at",
                "estimated_minutes",
                "updated_at",
            ],
        ),
        (
            "review_controls",
            &[
                "subject_id",
                "daily_budget_minutes",
                "desired_retention",
                "revision",
                "updated_at",
            ],
        ),
        (
            "review_debt_events",
            &[
                "id",
                "item_id",
                "subject_id",
                "state",
                "due_at",
                "estimated_minutes",
                "created_at",
            ],
        ),
        (
            "attempt_takeover_history",
            &[
                "id",
                "attempt_id",
                "old_owner",
                "new_owner",
                "sync_session_id",
                "old_revision",
                "new_revision",
                "created_at",
            ],
        ),
    ];
    for (table, expected) in TABLES {
        let mut stmt = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let actual = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if actual != *expected {
            return Err(Error::new(
                "practice_schema_invalid",
                format!("{table} columns are invalid"),
            )
            .details(json!({"expected":expected,"actual":actual})));
        }
    }
    const INDEXES: &[&str] = &[
        "practice_banks_subject",
        "practice_items_subject_state",
        "bank_items_item",
        "practice_sets_subject",
        "set_member_events_member",
        "paper_items_paper",
        "attempts_paper_state",
        "grades_item",
        "review_events_item",
        "fsrs_cards_due",
        "review_debt_subject",
        "review_debt_events_item",
    ];
    for index in INDEXES {
        let exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
                [index],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(Error::new(
                "practice_schema_invalid",
                format!("required index {index} is missing"),
            ));
        }
    }
    Ok(())
}

fn run_bank(store: &mut Store, command: BankCommand) -> Result<Value> {
    match command {
        BankCommand::Create { json } => create_bank(store, read_json(&json)?),
        BankCommand::Add {
            id,
            if_revision,
            json,
        } => add_bank_item(store, &id, if_revision, read_json(&json)?),
        BankCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "bank.show",
            json!({"bank":bank(&store.connection,&id)?}),
        )),
    }
}

fn run_item(store: &mut Store, command: ItemCommand) -> Result<Value> {
    match command {
        ItemCommand::Create { json } => create_item(store, read_json(&json)?),
        ItemCommand::Verify {
            id,
            if_revision,
            json,
        } => verify_item(store, &id, if_revision, read_json(&json)?),
        ItemCommand::Retire {
            id,
            if_revision,
            json,
        } => retire_item(store, &id, if_revision, read_json(&json)?),
        ItemCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "item.show",
            json!({"item":item(&store.connection,&id,None)?}),
        )),
    }
}

fn run_set(store: &mut Store, command: SetCommand) -> Result<Value> {
    match command {
        SetCommand::Create { json } => create_set(store, read_json(&json)?),
        SetCommand::Add {
            id,
            if_revision,
            json,
        } => add_set_member(store, &id, if_revision, read_json(&json)?),
        SetCommand::Resolve {
            id,
            item_id,
            if_revision,
            json,
        } => transition_member(
            store,
            &id,
            &item_id,
            if_revision,
            "resolved",
            read_json(&json)?,
        ),
        SetCommand::Reopen {
            id,
            item_id,
            if_revision,
            json,
        } => transition_member(
            store,
            &id,
            &item_id,
            if_revision,
            "active",
            read_json(&json)?,
        ),
        SetCommand::Archive {
            id,
            if_revision,
            json,
        } => archive_set(store, &id, if_revision, read_json(&json)?),
        SetCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "set.show",
            json!({"set":set_value(&store.connection,&id)?}),
        )),
    }
}

fn run_paper(store: &mut Store, command: PaperCommand) -> Result<Value> {
    match command {
        PaperCommand::Create { json } => create_paper(store, read_json(&json)?),
        PaperCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "paper.show",
            json!({"paper":paper(&store.connection,&id)?}),
        )),
    }
}

fn run_attempt(store: &mut Store, command: AttemptCommand) -> Result<Value> {
    match command {
        AttemptCommand::Create { json } => create_attempt(store, read_json(&json)?),
        AttemptCommand::Submit {
            id,
            if_revision,
            json,
        } => transition_attempt(store, &id, if_revision, "submitted", read_json(&json)?),
        AttemptCommand::Abandon {
            id,
            if_revision,
            json,
        } => transition_attempt(store, &id, if_revision, "abandoned", read_json(&json)?),
        AttemptCommand::Takeover { json } => takeover_attempt(store, read_json(&json)?),
        AttemptCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "attempt.show",
            json!({"attempt":attempt(&store.connection,&id,true)?}),
        )),
    }
}

fn run_response(store: &mut Store, command: ResponseCommand) -> Result<Value> {
    match command {
        ResponseCommand::Save {
            attempt_id,
            if_revision,
            owner,
            json,
        } => save_response(store, &attempt_id, if_revision, &owner, read_json(&json)?),
    }
}

fn run_grade(store: &mut Store, command: GradeCommand) -> Result<Value> {
    match command {
        GradeCommand::Objective { response_id, json } => {
            grade_objective(store, &response_id, read_json(&json)?)
        }
        GradeCommand::Subjective { response_id, json } => {
            grade_subjective(store, &response_id, read_json(&json)?)
        }
        GradeCommand::Override {
            id,
            if_revision,
            json,
        } => override_grade(store, &id, if_revision, read_json(&json)?),
        GradeCommand::Show { id } => Ok(envelope(
            Plugin::Practice,
            "grade.show",
            json!({"grade":grade(&store.connection,&id)?}),
        )),
    }
}

fn run_review(store: &mut Store, command: ReviewCommand) -> Result<Value> {
    match command {
        ReviewCommand::Rate { item_id, json } => rate_review(store, &item_id, read_json(&json)?),
        ReviewCommand::Configure { json } => configure_review(store, read_json(&json)?),
        ReviewCommand::Control { subject } => Ok(envelope(
            Plugin::Practice,
            "review.control",
            json!({"control":review_control(&store.connection,&subject)?}),
        )),
        ReviewCommand::Queue {
            subject,
            goal_bank,
            budget_minutes,
            now,
        } => review_queue(
            store,
            &subject,
            goal_bank.as_deref(),
            budget_minutes,
            &now,
            "review.queue",
        ),
        ReviewCommand::Show { item_id } => Ok(envelope(
            Plugin::Practice,
            "review.show",
            json!({"card":card_value(&store.connection,&item_id)?}),
        )),
    }
}

fn valid_exact_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_exact_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_source_base(source: &SourceRef) -> Result<()> {
    if !valid_exact_id(&source.id)
        || source
            .subject_id
            .as_deref()
            .is_none_or(|id| !valid_exact_id(id))
    {
        return Err(Error::new(
            "invalid_source_ref",
            "source id and subject_id must be exact lowercase identifiers",
        ));
    }
    Ok(())
}

fn validate_bank_source(source: &SourceRef, subject_id: &str) -> Result<()> {
    validate_source_base(source)?;
    let valid = source.subject_id.as_deref() == Some(subject_id)
        && source.locator.is_none()
        && match source.kind.as_str() {
            "book" => valid_exact_hash(&source.revision_or_hash),
            "subject" => {
                source.id == subject_id
                    && source
                        .revision_or_hash
                        .parse::<i64>()
                        .is_ok_and(|revision| revision > 0)
            }
            _ => false,
        };
    if !valid {
        return Err(Error::new(
            "invalid_source_ref",
            "bank source must be an exact same-subject Book hash or subject revision",
        ));
    }
    Ok(())
}

fn validate_item_source(source: &SourceRef, subject_id: &str) -> Result<()> {
    validate_source_base(source)?;
    if source.subject_id.as_deref() != Some(subject_id) {
        return Err(Error::new(
            "source_subject_mismatch",
            "source belongs to another subject",
        ));
    }
    let valid = match source.kind.as_str() {
        "book" => {
            valid_exact_hash(&source.revision_or_hash)
                && source.locator.as_deref().is_some_and(valid_exact_id)
        }
        "tutor_turn" => {
            source.locator.is_none()
                && source
                    .revision_or_hash
                    .parse::<i64>()
                    .is_ok_and(|revision| revision > 0)
        }
        _ => false,
    };
    if !valid {
        return Err(Error::new(
            "invalid_source_ref",
            "item source must be an exact Book block/hash or committed Tutor turn revision",
        ));
    }
    Ok(())
}

fn create_bank(store: &mut Store, input: BankCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    validate_bank_source(&input.source, &input.subject_id)?;
    if input.key.trim().is_empty() || input.title.trim().is_empty() {
        return Err(Error::new(
            "invalid_input",
            "bank key and title are required",
        ));
    }
    if (input.source.kind == "book"
        && input.key != format!("book:{}", input.source.id)
        && !input.key.starts_with("subject:"))
        || (input.source.kind == "subject" && !input.key.starts_with("subject:"))
    {
        return Err(Error::new(
            "invalid_bank_key",
            "bank key must be book:<book_id> or use the subject: prefix",
        ));
    }
    let fingerprint = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    require_subject(&tx, &input.subject_id)?;
    validate_bank_source_truth(&tx, &store.root, &input.source, &input.subject_id)?;
    let id = new_id(Plugin::Practice, &input.request_id);
    let at = now(&tx)?;
    tx.execute("INSERT INTO practice_banks(id,subject_id,bank_key,title,source_json,revision,created_at) VALUES(?1,?2,?3,?4,?5,1,?6)",params![id,input.subject_id,input.key,input.title,serde_json::to_string(&input.source).unwrap(),at])?;
    let value = envelope(
        Plugin::Practice,
        "bank.create",
        json!({"bank":bank(&tx,&id)?}),
    );
    let value = finalize_mutation(
        &tx,
        Plugin::Practice,
        &input.request_id,
        &fingerprint,
        value,
    )?;
    tx.commit()?;
    Ok(value)
}

fn add_bank_item(store: &mut Store, id: &str, if_revision: i64, input: BankAdd) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = bank(&tx, id)?;
    require_revision(&current, if_revision)?;
    let item = item(&tx, &input.item_id, Some(input.item_revision))?;
    if item["state"] != "verified" {
        return Err(Error::new(
            "item_not_verified",
            "only verified item revisions may join a bank",
        ));
    }
    if current["subject_id"] != item["subject_id"] {
        return Err(Error::new(
            "subject_mismatch",
            "bank and item subjects differ",
        ));
    }
    tx.execute("INSERT OR IGNORE INTO bank_items(bank_id,item_id,item_revision,added_at) VALUES(?1,?2,?3,?4)",params![id,input.item_id,input.item_revision,now(&tx)?])?;
    tx.execute(
        "UPDATE practice_banks SET revision=revision+1 WHERE id=?1 AND revision=?2",
        params![id, if_revision],
    )?;
    let value = envelope(Plugin::Practice, "bank.add", json!({"bank":bank(&tx,id)?}));
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn create_item(store: &mut Store, input: ItemCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    validate_item_source(&input.source, &input.subject_id)?;
    validate_item_fields(&input)?;
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    require_subject(&tx, &input.subject_id)?;
    let id = new_id(Plugin::Practice, &input.request_id);
    tx.execute("INSERT INTO practice_items(id,revision,subject_id,item_type,grading_kind,state,prompt,answer_json,rubric,max_points,estimated_minutes,difficulty,topic,source_json,created_at) VALUES(?1,1,?2,?3,?4,'draft',?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![id,input.subject_id,input.item_type,input.grading_kind,input.prompt,serde_json::to_string(&input.answer).unwrap(),input.rubric,input.max_points,input.estimated_minutes,input.difficulty,input.topic,serde_json::to_string(&input.source).unwrap(),now(&tx)?])?;
    let value = envelope(
        Plugin::Practice,
        "item.create",
        json!({"item":item(&tx,&id,Some(1))?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn validate_item_fields(input: &ItemCreate) -> Result<()> {
    if !matches!(
        input.item_type.as_str(),
        "choice" | "text" | "numeric" | "flashcard"
    ) || !matches!(input.grading_kind.as_str(), "objective" | "subjective")
    {
        return Err(Error::new(
            "unsupported_response_format",
            "v1 accepts choice, text, numeric, or flashcard",
        ));
    }
    if input.prompt.trim().is_empty()
        || input.max_points <= 0.0
        || input.estimated_minutes <= 0
        || !(0.0..=1.0).contains(&input.difficulty)
        || input
            .topic
            .as_deref()
            .is_some_and(|topic| topic.trim().is_empty())
    {
        return Err(Error::new(
            "invalid_input",
            "invalid prompt, points, time, or difficulty",
        ));
    }
    if input.grading_kind == "subjective"
        && input.rubric.as_deref().is_none_or(|v| v.trim().is_empty())
    {
        return Err(Error::new(
            "missing_rubric",
            "subjective item requires rubric",
        ));
    }
    Ok(())
}

fn verify_item(store: &mut Store, id: &str, if_revision: i64, input: ItemVerify) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = item(&tx, id, None)?;
    validate_item_source(&input.source, current["subject_id"].as_str().unwrap())?;
    require_revision(&current, if_revision)?;
    if current["state"] != "draft" {
        return Err(Error::new(
            "invalid_state",
            "only draft item may be verified",
        ));
    }
    if current["prompt"] != input.prompt
        || current["answer"] != input.answer
        || current["rubric"] != serde_json::to_value(&input.rubric).unwrap()
        || current["source"] != serde_json::to_value(&input.source).unwrap()
    {
        return Err(Error::new(
            "stale_source",
            "verification must match the current prompt, answer, rubric, and exact source",
        ));
    }
    validate_source_truth(
        &store.root,
        &input.source,
        current["subject_id"].as_str().unwrap(),
    )?;
    tx.execute("INSERT INTO practice_items SELECT id,revision+1,subject_id,item_type,grading_kind,'verified',prompt,answer_json,rubric,max_points,estimated_minutes,difficulty,topic,source_json,?2 FROM practice_items WHERE id=?1 AND revision=?3",params![id,now(&tx)?,if_revision])?;
    let value = envelope(
        Plugin::Practice,
        "item.verify",
        json!({"item":item(&tx,id,Some(if_revision+1))?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn source_connection(practice_root: &Path, plugin: &str) -> Result<Connection> {
    let plugins = practice_root
        .parent()
        .ok_or_else(|| Error::new("source_truth_unavailable", "plugin root is invalid"))?;
    let root = plugins.join(plugin);
    let database = root.join("data.sqlite3");
    reject_symlink(&root)?;
    reject_symlink(&database)?;
    Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| {
        Error::new(
            "source_truth_unavailable",
            format!("{plugin} source store is unavailable"),
        )
    })
}

fn validate_bank_source_truth(
    practice: &Connection,
    practice_root: &Path,
    source: &SourceRef,
    subject_id: &str,
) -> Result<()> {
    if source.kind == "subject" {
        let revision = practice
            .query_row(
                "SELECT revision FROM subjects WHERE id=?1",
                [subject_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| Error::new("subject_not_found", "subject was not found"))?;
        if source.id != subject_id || source.revision_or_hash != revision.to_string() {
            return Err(Error::new(
                "source_truth_mismatch",
                "current subject id or revision does not match",
            ));
        }
        return Ok(());
    }
    let connection = source_connection(practice_root, "book")?;
    let exists = connection
        .query_row(
            "SELECT 1 FROM books WHERE id=?1 AND subject_id=?2 AND COALESCE(normalized_sha256,original_sha256)=?3",
            params![source.id, subject_id, source.revision_or_hash],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| Error::new("source_truth_unavailable", "Book source schema is unavailable"))?
        .is_some();
    if !exists {
        return Err(Error::new(
            "source_truth_mismatch",
            "current Book id, content hash, or subject does not match",
        ));
    }
    Ok(())
}

fn validate_source_truth(practice_root: &Path, source: &SourceRef, subject_id: &str) -> Result<()> {
    let (plugin, sql) = match source.kind.as_str() {
        "book" => {
            if source.locator.as_deref().is_none_or(str::is_empty) {
                return Err(Error::new(
                    "invalid_source_ref",
                    "book source requires an exact locator",
                ));
            }
            (
                "book",
                "SELECT 1 FROM book_blocks block JOIN books book ON book.id=block.book_id WHERE block.id=?1 AND block.book_id=?2 AND block.text_hash=?3 AND book.subject_id=?4",
            )
        }
        "tutor_turn" => (
            "tutor",
            "SELECT 1 FROM tutor_turns turn JOIN tutor_sessions session ON session.id=turn.session_id WHERE turn.id=?1 AND turn.state='committed' AND CAST(turn.revision AS TEXT)=?3 AND session.subject_id=?4",
        ),
        _ => {
            return Err(Error::new(
                "unsupported_source_kind",
                "verified items require book or tutor_turn source truth",
            ));
        }
    };
    let connection = source_connection(practice_root, plugin)?;
    let key = if plugin == "book" {
        source.locator.as_deref().unwrap()
    } else {
        source.id.as_str()
    };
    let exists = connection
        .query_row(
            sql,
            params![key, source.id, source.revision_or_hash, subject_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|_| Error::new("source_truth_unavailable", "source schema is unavailable"))?
        .is_some();
    if !exists {
        return Err(Error::new(
            "source_truth_mismatch",
            "current Book locator/hash or Tutor evidence revision does not match",
        ));
    }
    Ok(())
}

fn retire_item(
    store: &mut Store,
    id: &str,
    if_revision: i64,
    input: SetTransition,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.reason.trim().is_empty() {
        return Err(Error::new("invalid_input", "retirement reason is required"));
    }
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fp)? {
        return Ok(value);
    }
    let current = item(&tx, id, None)?;
    require_revision(&current, if_revision)?;
    if current["state"] != "verified" {
        return Err(Error::new(
            "invalid_state",
            "only a verified item may be retired",
        ));
    }
    tx.execute("INSERT INTO practice_items SELECT id,revision+1,subject_id,item_type,grading_kind,'retired',prompt,answer_json,rubric,max_points,estimated_minutes,difficulty,topic,source_json,?2 FROM practice_items WHERE id=?1 AND revision=?3",params![id,now(&tx)?,if_revision])?;
    let value = envelope(
        Plugin::Practice,
        "item.retire",
        json!({"item":item(&tx,id,Some(if_revision+1))?,"reason":input.reason}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn create_set(store: &mut Store, input: SetCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.name.trim().is_empty() || !matches!(input.kind.as_str(), "ordinary" | "mistake") {
        return Err(Error::new("invalid_input", "invalid set name or kind"));
    }
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    require_subject(&tx, &input.subject_id)?;
    let id = new_id(Plugin::Practice, &input.request_id);
    tx.execute(
        "INSERT INTO practice_sets VALUES(?1,?2,?3,?4,'active',1,?5)",
        params![id, input.subject_id, input.name, input.kind, now(&tx)?],
    )?;
    let value = envelope(
        Plugin::Practice,
        "set.create",
        json!({"set":set_value(&tx,&id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn add_set_member(store: &mut Store, id: &str, if_revision: i64, input: SetAdd) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = set_value(&tx, id)?;
    require_revision(&current, if_revision)?;
    if current["state"] != "active" {
        return Err(Error::new("set_archived", "archived set cannot change"));
    }
    item(&tx, &input.item_id, Some(input.item_revision))?;
    tx.execute("INSERT INTO set_members(set_id,item_id,item_revision,state,revision) VALUES(?1,?2,?3,'active',1) ON CONFLICT(set_id,item_id,item_revision) DO UPDATE SET state='active',revision=revision+1",params![id,input.item_id,input.item_revision])?;
    insert_member_event(
        &tx,
        id,
        &input.item_id,
        input.item_revision,
        "add",
        &input.reason,
        &input.request_id,
    )?;
    tx.execute(
        "UPDATE practice_sets SET revision=revision+1 WHERE id=?1 AND revision=?2",
        params![id, if_revision],
    )?;
    let value = envelope(
        Plugin::Practice,
        "set.add",
        json!({"set":set_value(&tx,id)?,"member":member(&tx,id,&input.item_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn transition_member(
    store: &mut Store,
    id: &str,
    item_id: &str,
    if_revision: i64,
    state: &str,
    input: SetTransition,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(
        &json!({"id":id,"item_id":item_id,"if_revision":if_revision,"state":state,"input":input}),
    )?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let set = set_value(&tx, id)?;
    require_revision(&set, if_revision)?;
    if set["state"] != "active" {
        return Err(Error::new("set_archived", "archived set cannot change"));
    }
    let item_revision: i64=tx.query_row("SELECT item_revision FROM set_members WHERE set_id=?1 AND item_id=?2 ORDER BY item_revision DESC LIMIT 1",params![id,item_id],|r|r.get(0)).optional()?.ok_or_else(||Error::new("set_member_not_found","set member was not found"))?;
    tx.execute("UPDATE set_members SET state=?3,revision=revision+1 WHERE set_id=?1 AND item_id=?2 AND item_revision=?4",params![id,item_id,state,item_revision])?;
    insert_member_event(
        &tx,
        id,
        item_id,
        item_revision,
        state,
        &input.reason,
        &input.request_id,
    )?;
    tx.execute(
        "UPDATE practice_sets SET revision=revision+1 WHERE id=?1 AND revision=?2",
        params![id, if_revision],
    )?;
    let value = envelope(
        Plugin::Practice,
        &format!("set.{state}"),
        json!({"set":set_value(&tx,id)?,"member":member(&tx,id,item_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn archive_set(
    store: &mut Store,
    id: &str,
    if_revision: i64,
    input: SetTransition,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = set_value(&tx, id)?;
    require_revision(&current, if_revision)?;
    if current["state"] != "active" {
        return Err(Error::new("set_archived", "set is already archived"));
    }
    tx.execute(
        "UPDATE practice_sets SET state='archived',revision=revision+1 WHERE id=?1 AND revision=?2",
        params![id, if_revision],
    )?;
    let value = envelope(
        Plugin::Practice,
        "set.archive",
        json!({"set":set_value(&tx,id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn create_paper(store: &mut Store, input: PaperCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.count <= 0 || input.duration_minutes <= 0 {
        return Err(Error::new(
            "invalid_input",
            "count and duration must be positive",
        ));
    }
    if input
        .difficulty_min
        .is_some_and(|v| !(0.0..=1.0).contains(&v))
        || input
            .difficulty_max
            .is_some_and(|v| !(0.0..=1.0).contains(&v))
        || input
            .difficulty_min
            .zip(input.difficulty_max)
            .is_some_and(|(a, b)| a > b)
        || input.item_type_counts.values().any(|count| *count < 0)
        || input.topic_counts.values().any(|count| *count < 0)
        || input.section_counts.values().any(|count| *count < 0)
        || input.item_type_counts.values().sum::<i64>() > input.count
        || input.topic_counts.values().sum::<i64>() > input.count
        || input.section_counts.values().sum::<i64>() > input.count
        || input.total_points.is_some_and(|points| points <= 0.0)
    {
        return Err(Error::new(
            "invalid_blueprint",
            "paper constraints are invalid",
        ));
    }
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let bank = bank(&tx, &input.bank_id)?;
    type Candidate = (
        String,
        i64,
        String,
        String,
        Option<String>,
        String,
        f64,
        String,
        String,
        i64,
        f64,
        Option<String>,
    );
    let mut stmt=tx.prepare("SELECT i.id,i.revision,i.prompt,i.answer_json,i.rubric,i.source_json,i.max_points,i.item_type,i.grading_kind,i.estimated_minutes,i.difficulty,i.topic FROM bank_items b JOIN practice_items i ON i.id=b.item_id AND i.revision=b.item_revision WHERE b.bank_id=?1 AND i.state='verified' AND NOT EXISTS(SELECT 1 FROM practice_items newer WHERE newer.id=i.id AND newer.revision>i.revision AND newer.state='retired') AND (?2 IS NULL OR i.difficulty>=?2) AND (?3 IS NULL OR i.difficulty<=?3) AND (?4 IS NULL OR json_extract(i.source_json,'$.kind')=?4) ORDER BY i.difficulty,i.id,i.revision")?;
    let candidates = stmt
        .query_map(
            params![
                input.bank_id,
                input.difficulty_min,
                input.difficulty_max,
                input.source_kind
            ],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<Candidate>, _>>()?;
    drop(stmt);
    if candidates.len() > MAX_BLUEPRINT_CANDIDATES {
        return Err(Error::new(
            "blueprint_too_complex",
            "paper blueprint candidate set exceeds the search limit",
        )
        .details(
            json!({"candidate_limit":MAX_BLUEPRINT_CANDIDATES,"candidates":candidates.len()}),
        ));
    }
    fn satisfies(rows: &[Candidate], input: &PaperCreate) -> bool {
        let count = |values: &BTreeMap<String, i64>, field: fn(&Candidate) -> Option<&str>| {
            values.iter().all(|(value, required)| {
                rows.iter()
                    .filter(|row| field(row) == Some(value.as_str()))
                    .count() as i64
                    >= *required
            })
        };
        count(&input.item_type_counts, |row| Some(row.7.as_str()))
            && count(&input.topic_counts, |row| row.11.as_deref())
            && rows.iter().map(|row| row.9).sum::<i64>() <= input.duration_minutes
            && input.total_points.is_none_or(|required| {
                (rows.iter().map(|row| row.6).sum::<f64>() - required).abs() <= f64::EPSILON
            })
    }
    fn can_meet(
        selected: &[Candidate],
        remaining: &[Candidate],
        values: &BTreeMap<String, i64>,
        field: fn(&Candidate) -> Option<&str>,
    ) -> bool {
        values.iter().all(|(value, required)| {
            selected
                .iter()
                .chain(remaining)
                .filter(|row| field(row) == Some(value.as_str()))
                .count() as i64
                >= *required
        })
    }
    fn choose(
        candidates: &[Candidate],
        input: &PaperCreate,
        start: usize,
        selected: &mut Vec<Candidate>,
        searched: &mut usize,
    ) -> Result<bool> {
        *searched += 1;
        if *searched > MAX_BLUEPRINT_SEARCH_STATES {
            return Err(Error::new(
                "blueprint_too_complex",
                "paper blueprint exceeded the deterministic search-state limit",
            )
            .details(json!({"search_state_limit":MAX_BLUEPRINT_SEARCH_STATES})));
        }
        if selected.len() as i64 == input.count {
            return Ok(satisfies(selected, input));
        }
        let remaining = input.count as usize - selected.len();
        if candidates.len().saturating_sub(start) < remaining {
            return Ok(false);
        }
        if selected.iter().map(|row| row.9).sum::<i64>() > input.duration_minutes
            || input
                .total_points
                .is_some_and(|points| selected.iter().map(|row| row.6).sum::<f64>() > points)
            || !can_meet(
                selected,
                &candidates[start..],
                &input.item_type_counts,
                |row| Some(row.7.as_str()),
            )
            || !can_meet(selected, &candidates[start..], &input.topic_counts, |row| {
                row.11.as_deref()
            })
        {
            return Ok(false);
        }
        for index in start..candidates.len() {
            selected.push(candidates[index].clone());
            if choose(candidates, input, index + 1, selected, searched)? {
                return Ok(true);
            }
            selected.pop();
        }
        Ok(false)
    }
    let mut selected = Vec::new();
    let mut searched = 0;
    if !choose(&candidates, &input, 0, &mut selected, &mut searched)? {
        return Err(Error::new("paper_shortage","blueprint cannot be satisfied").details(json!({"required":input.count,"available":candidates.len(),"missing":(input.count-candidates.len() as i64).max(0)})));
    }
    let available = selected.len() as i64;
    let estimated: i64 = selected.iter().map(|row| row.9).sum();
    let total_points: f64 = selected.iter().map(|row| row.6).sum();
    if available != input.count
        || estimated > input.duration_minutes
        || input
            .total_points
            .is_some_and(|required| (total_points - required).abs() > f64::EPSILON)
    {
        return Err(Error::new("paper_shortage","blueprint cannot be satisfied").details(json!({"required":input.count,"available":available,"missing":input.count-available,"estimated_minutes":estimated,"duration_minutes":input.duration_minutes})));
    }
    let id = new_id(Plugin::Practice, &input.request_id);
    tx.execute(
        "INSERT INTO papers VALUES(?1,?2,?3,?4,1,?5)",
        params![
            id,
            input.bank_id,
            bank["subject_id"].as_str().unwrap(),
            input.duration_minutes,
            now(&tx)?
        ],
    )?;
    let mut sections = input
        .section_counts
        .iter()
        .flat_map(|(section, count)| std::iter::repeat_n(section.clone(), *count as usize))
        .collect::<Vec<_>>();
    sections.resize(input.count as usize, "general".to_owned());
    for (ordinal, row) in selected.into_iter().enumerate() {
        let pid = new_id(Plugin::Practice, &format!("{}:{ordinal}", input.request_id));
        tx.execute(
            "INSERT INTO paper_items VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                pid,
                id,
                ordinal as i64,
                sections[ordinal].as_str(),
                row.0,
                row.1,
                row.2,
                row.3,
                row.4,
                row.5,
                row.6,
                row.7,
                row.8
            ],
        )?;
    }
    let value = envelope(
        Plugin::Practice,
        "paper.create",
        json!({"paper":paper(&tx,&id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn create_attempt(store: &mut Store, input: AttemptCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.owner.trim().is_empty() {
        return Err(Error::new("invalid_input", "owner is required"));
    }
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    paper(&tx, &input.paper_id)?;
    let id = new_id(Plugin::Practice, &input.request_id);
    let at = now(&tx)?;
    tx.execute(
        "INSERT INTO attempts VALUES(?1,?2,?3,'in_progress',1,?4,?4)",
        params![id, input.paper_id, input.owner, at],
    )?;
    let value = envelope(
        Plugin::Practice,
        "attempt.create",
        json!({"attempt":attempt(&tx,&id,false)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn transition_attempt(
    store: &mut Store,
    id: &str,
    if_revision: i64,
    state: &str,
    input: AttemptTransition,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"state":state,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = attempt(&tx, id, false)?;
    require_revision(&current, if_revision)?;
    if current["state"] != "in_progress" {
        return Err(Error::new("attempt_frozen", "attempt is already terminal"));
    }
    if current["owner"] != input.owner {
        return Err(Error::new("stale_owner", "attempt owner does not match"));
    }
    tx.execute("UPDATE attempts SET state=?3,revision=revision+1,updated_at=?4 WHERE id=?1 AND revision=?2",params![id,if_revision,state,now(&tx)?])?;
    let value = envelope(
        Plugin::Practice,
        &format!("attempt.{state}"),
        json!({"attempt":attempt(&tx,id,false)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn takeover_attempt(store: &mut Store, input: AttemptTakeover) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.new_owner.trim().is_empty() || input.new_owner == input.old_owner {
        return Err(Error::new(
            "invalid_owner",
            "new_owner must be non-empty and different",
        ));
    }
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = attempt(&tx, &input.entity_id, false)?;
    require_revision(&current, input.if_revision)?;
    if current["state"] != "in_progress" {
        return Err(Error::new(
            "attempt_frozen",
            "terminal attempt cannot be taken over",
        ));
    }
    if current["owner"] != input.old_owner {
        return Err(Error::new("stale_owner", "old_owner does not match"));
    }
    require_latest_sync_receipt(&tx, Plugin::Practice, &input.sync_session_id)?;
    let changed=tx.execute("UPDATE attempts SET owner=?3,revision=revision+1,updated_at=?4 WHERE id=?1 AND revision=?2 AND owner=?5 AND state='in_progress'",params![input.entity_id,input.if_revision,input.new_owner,now(&tx)?,input.old_owner])?;
    if changed != 1 {
        return Err(Error::new(
            "stale_attempt",
            "attempt owner, revision, or state changed during takeover",
        ));
    }
    let new_revision = input.if_revision + 1;
    tx.execute(
        "INSERT INTO attempt_takeover_history VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            new_id(Plugin::Practice, &input.request_id),
            input.entity_id,
            input.old_owner,
            input.new_owner,
            input.sync_session_id,
            input.if_revision,
            new_revision,
            now(&tx)?
        ],
    )?;
    let value = envelope(
        Plugin::Practice,
        "attempt.takeover",
        json!({"attempt":attempt(&tx,&input.entity_id,false)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn save_response(
    store: &mut Store,
    attempt_id: &str,
    if_revision: i64,
    owner: &str,
    input: ResponseSave,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if !matches!(
        input.format.as_str(),
        "choice" | "text" | "numeric" | "flashcard"
    ) {
        return Err(Error::new(
            "unsupported_response_format",
            "v1 accepts choice, text, numeric, or flashcard",
        ));
    }
    let value_valid = match input.format.as_str() {
        "choice" | "text" => input
            .value
            .as_str()
            .is_some_and(|value| !value.trim().is_empty()),
        "numeric" => input.value.is_number(),
        "flashcard" => input
            .value
            .as_i64()
            .is_some_and(|rating| (1..=4).contains(&rating)),
        _ => false,
    };
    if !value_valid {
        return Err(Error::new(
            "invalid_response_value",
            "response value does not match its format",
        ));
    }
    let fp = fingerprint(
        &json!({"attempt_id":attempt_id,"if_revision":if_revision,"owner":owner,"input":input}),
    )?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = attempt(&tx, attempt_id, false)?;
    require_revision(&current, if_revision)?;
    if current["state"] != "in_progress" {
        return Err(Error::new(
            "attempt_frozen",
            "responses cannot change after submit or abandon",
        ));
    }
    if current["owner"] != owner {
        return Err(Error::new("stale_owner", "attempt owner does not match"));
    }
    let expected:String=tx.query_row("SELECT item_type FROM paper_items WHERE id=?1 AND paper_id=(SELECT paper_id FROM attempts WHERE id=?2)",params![input.paper_item_id,attempt_id],|r|r.get(0)).optional()?.ok_or_else(||Error::new("paper_item_not_found","paper item was not found in attempt"))?;
    if expected != input.format {
        return Err(Error::new(
            "response_format_mismatch",
            "response format does not match frozen item",
        ));
    }
    let existing: Option<(String, i64)> = tx
        .query_row(
            "SELECT id,revision FROM responses WHERE attempt_id=?1 AND paper_item_id=?2",
            params![attempt_id, input.paper_item_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (response_id, revision) = existing.map_or_else(
        || (new_id(Plugin::Practice, &input.request_id), 1),
        |(id, r)| (id, r + 1),
    );
    let raw = serde_json::to_string(&input.value).unwrap();
    let at = now(&tx)?;
    tx.execute("INSERT INTO responses(id,attempt_id,paper_item_id,format,value_json,revision,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(attempt_id,paper_item_id) DO UPDATE SET format=excluded.format,value_json=excluded.value_json,revision=excluded.revision,updated_at=excluded.updated_at",params![response_id,attempt_id,input.paper_item_id,input.format,raw,revision,at])?;
    tx.execute(
        "INSERT INTO response_history VALUES(?1,?2,?3,?4,?5)",
        params![response_id, revision, input.format, raw, at],
    )?;
    tx.execute(
        "UPDATE attempts SET revision=revision+1,updated_at=?3 WHERE id=?1 AND revision=?2",
        params![attempt_id, if_revision, at],
    )?;
    let value = envelope(
        Plugin::Practice,
        "response.save",
        json!({"response":response(&tx,&response_id,true)?,"attempt":attempt(&tx,attempt_id,false)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn grade_objective(store: &mut Store, response_id: &str, input: GradeRequest) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    let fp = fingerprint(&json!({"response_id":response_id,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    require_gradeable_response(&tx, response_id, input.response_revision)?;
    let row:(String,i64,String,String,f64,String)=tx.query_row("SELECT p.item_id,p.item_revision,p.answer_json,r.value_json,p.points,p.grading_kind FROM responses r JOIN paper_items p ON p.id=r.paper_item_id WHERE r.id=?1",[response_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?.ok_or_else(||Error::new("response_not_found","response was not found"))?;
    if row.5 != "objective" {
        return Err(Error::new(
            "grading_kind_mismatch",
            "subjective item requires rubric-bound grade",
        ));
    }
    let correct: Value =
        serde_json::from_str(&row.2).map_err(|e| Error::new("corrupt_store", e.to_string()))?;
    let actual: Value =
        serde_json::from_str(&row.3).map_err(|e| Error::new("corrupt_store", e.to_string()))?;
    let score = if correct == actual { row.4 } else { 0.0 };
    let id = new_id(Plugin::Practice, &input.request_id);
    insert_grade(
        &tx,
        &id,
        &row.0,
        row.1,
        Some(response_id),
        score,
        row.4,
        if score == row.4 {
            "exact objective match"
        } else {
            "objective mismatch"
        },
        1.0,
        "deterministic",
        "accepted",
    )?;
    if score < row.4 {
        record_mistake(&tx, &row.0, row.1, &input.request_id, "objective mismatch")?;
    }
    let value = envelope(
        Plugin::Practice,
        "grade.objective",
        json!({"grade":grade(&tx,&id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn grade_subjective(store: &mut Store, response_id: &str, input: SubjectiveGrade) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.rationale.trim().is_empty()
        || input.method != "agent_rubric"
        || !(0.0..=1.0).contains(&input.confidence)
    {
        return Err(Error::new(
            "invalid_grade",
            "subjective grade requires concise rationale, confidence, and agent_rubric method",
        ));
    }
    let fp = fingerprint(&json!({"response_id":response_id,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    require_gradeable_response(&tx, response_id, input.response_revision)?;
    let row:(String,i64,i64,Option<String>,f64,String)=tx.query_row("SELECT p.item_id,p.item_revision,r.revision,p.rubric,p.points,p.grading_kind FROM responses r JOIN paper_items p ON p.id=r.paper_item_id WHERE r.id=?1",[response_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?.ok_or_else(||Error::new("response_not_found","exact response was not found"))?;
    if row.1 != input.item_revision {
        return Err(Error::new(
            "stale_item_revision",
            "grade item revision does not match frozen paper",
        ));
    }
    if row.2 != input.response_revision {
        return Err(Error::new(
            "stale_response_revision",
            "grade response revision is stale",
        ));
    }
    if row.5 != "subjective" || row.3.as_deref().is_none_or(|v| v.trim().is_empty()) {
        return Err(Error::new(
            "missing_rubric",
            "frozen subjective rubric is required",
        ));
    }
    let max = row.4;
    if !(0.0..=max).contains(&input.score) {
        return Err(Error::new("invalid_grade", "score exceeds frozen points"));
    }
    let state = if input.confidence < 0.6 {
        "pending_review"
    } else {
        "accepted"
    };
    let id = new_id(Plugin::Practice, &input.request_id);
    insert_grade(
        &tx,
        &id,
        &row.0,
        row.1,
        Some(response_id),
        input.score,
        max,
        &input.rationale,
        input.confidence,
        &input.method,
        state,
    )?;
    if state == "pending_review" || input.score < max {
        record_mistake(&tx, &row.0, row.1, &input.request_id, "grade needs review")?;
    }
    let value = envelope(
        Plugin::Practice,
        "grade.subjective",
        json!({"grade":grade(&tx,&id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn require_gradeable_response(
    connection: &Connection,
    response_id: &str,
    response_revision: i64,
) -> Result<()> {
    let (current_revision, attempt_state): (i64, String) = connection
        .query_row(
            "SELECT r.revision,a.state FROM responses r JOIN attempts a ON a.id=r.attempt_id WHERE r.id=?1",
            [response_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| Error::new("response_not_found", "response was not found"))?;
    if current_revision != response_revision {
        return Err(Error::new(
            "stale_response_revision",
            "grade response revision is stale",
        ));
    }
    if attempt_state == "in_progress" {
        return Err(Error::new(
            "attempt_not_terminal",
            "attempt must be submitted or abandoned before grading",
        ));
    }
    if connection
        .query_row(
            "SELECT 1 FROM grades WHERE response_id=?1 LIMIT 1",
            [response_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Err(Error::new(
            "grade_already_exists",
            "current response revision already has a grade",
        ));
    }
    Ok(())
}

fn override_grade(
    store: &mut Store,
    id: &str,
    if_revision: i64,
    input: GradeOverride,
) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.actor != "learner" {
        return Err(Error::new(
            "learner_control_required",
            "only the learner may override a grade",
        ));
    }
    if input.reason.trim().is_empty() {
        return Err(Error::new("invalid_input", "override reason is required"));
    }
    let fp = fingerprint(&json!({"id":id,"if_revision":if_revision,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = grade(&tx, id)?;
    require_revision(&current, if_revision)?;
    let max = current["max_points"].as_f64().unwrap();
    if !(0.0..=max).contains(&input.score) {
        return Err(Error::new("invalid_grade", "score exceeds frozen points"));
    }
    let revision = if_revision + 1;
    tx.execute("UPDATE grades SET score=?2,rationale=?3,confidence=1,method='learner_override',state='overridden',revision=?4 WHERE id=?1 AND revision=?5",params![id,input.score,input.reason,revision,if_revision])?;
    tx.execute(
        "INSERT INTO grade_history VALUES(?1,?2,?3,?4,1,'learner_override','overridden',?5)",
        params![id, revision, input.score, input.reason, now(&tx)?],
    )?;
    let value = envelope(
        Plugin::Practice,
        "grade.override",
        json!({"grade":grade(&tx,id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn insert_grade(
    tx: &Transaction<'_>,
    id: &str,
    item_id: &str,
    item_revision: i64,
    response_id: Option<&str>,
    score: f64,
    max: f64,
    rationale: &str,
    confidence: f64,
    method: &str,
    state: &str,
) -> Result<()> {
    let at = now(tx)?;
    tx.execute(
        "INSERT INTO grades VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11)",
        params![
            id,
            item_id,
            item_revision,
            response_id,
            score,
            max,
            rationale,
            confidence,
            method,
            state,
            at
        ],
    )?;
    tx.execute(
        "INSERT INTO grade_history VALUES(?1,1,?2,?3,?4,?5,?6,?7)",
        params![id, score, rationale, confidence, method, state, at],
    )?;
    Ok(())
}

fn record_mistake(
    tx: &Transaction<'_>,
    item_id: &str,
    item_revision: i64,
    request_id: &str,
    reason: &str,
) -> Result<()> {
    let subject_id: String = tx.query_row(
        "SELECT subject_id FROM practice_items WHERE id=?1 AND revision=?2",
        params![item_id, item_revision],
        |row| row.get(0),
    )?;
    let set_id=if let Some(id)=tx.query_row("SELECT id FROM practice_sets WHERE subject_id=?1 AND kind='mistake' AND state='active' ORDER BY created_at,id LIMIT 1",[&subject_id],|row|row.get::<_,String>(0)).optional()?{id}else{let id=new_id(Plugin::Practice,&format!("{request_id}:mistakes"));tx.execute("INSERT INTO practice_sets VALUES(?1,?2,'Mistakes','mistake','active',1,?3)",params![id,subject_id,now(tx)?])?;id};
    tx.execute("INSERT INTO set_members(set_id,item_id,item_revision,state,revision) VALUES(?1,?2,?3,'active',1) ON CONFLICT(set_id,item_id,item_revision) DO UPDATE SET state='active',revision=revision+1",params![set_id,item_id,item_revision])?;
    insert_member_event(
        tx,
        &set_id,
        item_id,
        item_revision,
        "mistake",
        reason,
        &format!("{request_id}:mistake"),
    )?;
    tx.execute(
        "UPDATE practice_sets SET revision=revision+1 WHERE id=?1",
        [set_id],
    )?;
    Ok(())
}

fn rate_review(store: &mut Store, item_id: &str, input: ReviewRate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.estimated_minutes <= 0 {
        return Err(Error::new(
            "invalid_input",
            "estimated_minutes must be positive",
        ));
    }
    let rating = match input.rating {
        1 => Rating::Again,
        2 => Rating::Hard,
        3 => Rating::Good,
        4 => Rating::Easy,
        _ => return Err(Error::new("invalid_rating", "rating must be 1..=4")),
    };
    let reviewed = DateTime::parse_from_rfc3339(&input.reviewed_at)
        .map_err(|_| Error::new("invalid_time", "reviewed_at must be RFC3339"))?
        .with_timezone(&Utc);
    let fp = fingerprint(&json!({"item_id":item_id,"input":input}))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    let current = item(&tx, item_id, None)?;
    if current["state"] != "verified" || current["item_type"] != "flashcard" {
        return Err(Error::new(
            "review_requires_verified_flashcard",
            "only verified flashcards may be scheduled",
        ));
    }
    let existing: Option<(String, i64)> = tx
        .query_row(
            "SELECT card_json,revision FROM fsrs_cards WHERE item_id=?1",
            [item_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (card, revision) = match existing {
        Some((raw, r)) => {
            let card = serde_json::from_str::<Card>(&raw)
                .map_err(|e| Error::new("corrupt_store", e.to_string()))?;
            if reviewed <= card.last_review {
                return Err(Error::new(
                    "non_monotonic_review_time",
                    "reviewed_at must be later than the previous review",
                ));
            }
            (card, r + 1)
        }
        None => {
            let mut c = Card::default();
            c.due = reviewed;
            c.last_review = reviewed;
            (c, 1)
        }
    };
    let subject_id = current["subject_id"].as_str().unwrap();
    let retention: Option<f64> = tx
        .query_row(
            "SELECT desired_retention FROM review_controls WHERE subject_id=?1",
            [subject_id],
            |row| row.get(0),
        )
        .optional()?;
    let mut parameters = Parameters::default();
    if let Some(retention) = retention {
        parameters.request_retention = retention;
    }
    let parameters_json=json!({"request_retention":parameters.request_retention,"maximum_interval":parameters.maximum_interval,"weights":parameters.w,"decay":parameters.decay,"factor":parameters.factor,"enable_short_term":parameters.enable_short_term,"enable_fuzz":parameters.enable_fuzz}).to_string();
    let next = FSRS::new(parameters).next(card, reviewed, rating);
    let raw =
        serde_json::to_string(&next.card).map_err(|e| Error::new("json_error", e.to_string()))?;
    let due = next.card.due.to_rfc3339();
    let ordinal: i64 = tx.query_row(
        "SELECT COUNT(*)+1 FROM review_events WHERE item_id=?1",
        [item_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO review_events VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            new_id(Plugin::Practice, &input.request_id),
            item_id,
            ordinal,
            input.rating,
            reviewed.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            next.card.scheduled_days,
            raw,
            SCHEDULER_CRATE,
            SCHEDULER_VERSION
        ],
    )?;
    tx.execute("INSERT INTO fsrs_cards VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(item_id) DO UPDATE SET card_json=excluded.card_json,due_at=excluded.due_at,estimated_minutes=excluded.estimated_minutes,parameters_json=excluded.parameters_json,revision=excluded.revision",params![item_id,subject_id,raw,due,input.estimated_minutes,SCHEDULER_CRATE,SCHEDULER_VERSION,parameters_json,revision])?;
    if let Some((old_due, old_minutes)) = tx
        .query_row(
            "SELECT due_at,estimated_minutes FROM review_debt WHERE item_id=?1 AND state='open'",
            [item_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    {
        tx.execute(
            "INSERT OR IGNORE INTO review_debt_events VALUES(?1,?2,?3,'served',?4,?5,?6)",
            params![
                new_id(Plugin::Practice, &format!("{}:served", input.request_id)),
                item_id,
                subject_id,
                old_due,
                old_minutes,
                now(&tx)?
            ],
        )?;
    }
    tx.execute("INSERT INTO review_debt VALUES(?1,?2,'open',?3,?4,?5) ON CONFLICT(item_id) DO UPDATE SET state='open',due_at=excluded.due_at,estimated_minutes=excluded.estimated_minutes,updated_at=excluded.updated_at",params![item_id,subject_id,due,input.estimated_minutes,now(&tx)?])?;
    tx.execute(
        "INSERT OR IGNORE INTO review_debt_events VALUES(?1,?2,?3,'open',?4,?5,?6)",
        params![
            new_id(Plugin::Practice, &format!("{}:open", input.request_id)),
            item_id,
            subject_id,
            due,
            input.estimated_minutes,
            now(&tx)?
        ],
    )?;
    let value = envelope(
        Plugin::Practice,
        "review.rate",
        json!({"card":card_value(&tx,item_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn configure_review(store: &mut Store, input: ReviewControl) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    if input.actor != "learner" {
        return Err(Error::new(
            "learner_control_required",
            "only the learner may change review controls",
        ));
    }
    if input.daily_budget_minutes < 0 || !(0.7..=0.99).contains(&input.desired_retention) {
        return Err(Error::new(
            "invalid_review_control",
            "budget or retention is out of range",
        ));
    }
    let fp = fingerprint(&input)?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(v) = replay(&tx, &input.request_id, &fp)? {
        return Ok(v);
    }
    require_subject(&tx, &input.subject_id)?;
    tx.execute("INSERT INTO review_controls VALUES(?1,?2,?3,1,?4) ON CONFLICT(subject_id) DO UPDATE SET daily_budget_minutes=excluded.daily_budget_minutes,desired_retention=excluded.desired_retention,revision=review_controls.revision+1,updated_at=excluded.updated_at",params![input.subject_id,input.daily_budget_minutes,input.desired_retention,now(&tx)?])?;
    let value = envelope(
        Plugin::Practice,
        "review.configure",
        json!({"control":review_control(&tx,&input.subject_id)?}),
    );
    let value = finalize_mutation(&tx, Plugin::Practice, &input.request_id, &fp, value)?;
    tx.commit()?;
    Ok(value)
}

fn review_control(connection: &Connection, subject: &str) -> Result<Value> {
    connection.query_row("SELECT subject_id,daily_budget_minutes,desired_retention,revision,updated_at FROM review_controls WHERE subject_id=?1",[subject],|r|Ok(json!({"subject_id":r.get::<_,String>(0)?,"daily_budget_minutes":r.get::<_,i64>(1)?,"desired_retention":r.get::<_,f64>(2)?,"revision":r.get::<_,i64>(3)?,"updated_at":r.get::<_,String>(4)?}))).optional()?.ok_or_else(||Error::new("review_control_not_found","learner review controls are not configured"))
}

fn resolve_target(connection: &Connection, input: NextTarget) -> Result<Value> {
    require_subject(connection, &input.subject_id)?;
    let (source, target) = if let Some(target) = input.explicit_plan_target {
        ("plan", target)
    } else if let Some(id) = input.goal_bank_id {
        (
            "goal",
            TargetRef {
                kind: "bank".to_owned(),
                id,
            },
        )
    } else {
        (
            "subject",
            TargetRef {
                kind: "bank".to_owned(),
                id: input.subject_default_bank_id,
            },
        )
    };
    let valid = match target.kind.as_str() {
        "bank" => connection
            .query_row(
                "SELECT subject_id FROM practice_banks WHERE id=?1",
                [&target.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?,
        "set" => connection
            .query_row(
                "SELECT subject_id FROM practice_sets WHERE id=?1",
                [&target.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?,
        "paper" => connection
            .query_row(
                "SELECT subject_id FROM papers WHERE id=?1",
                [&target.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?,
        _ => {
            return Err(Error::new(
                "invalid_target_kind",
                "target kind must be bank, set, or paper",
            ));
        }
    };
    let Some(target_subject) = valid else {
        return Err(Error::new(
            "target_not_found",
            "higher-precedence exact target was not found",
        ));
    };
    if target_subject != input.subject_id {
        return Err(Error::new(
            "target_subject_mismatch",
            "target belongs to another subject",
        ));
    }
    Ok(envelope(
        Plugin::Practice,
        "next",
        json!({"target":{"kind":target.kind,"id":target.id},"resolved_from":source}),
    ))
}

fn review_queue(
    store: &mut Store,
    subject: &str,
    goal_bank: Option<&str>,
    budget: i64,
    at: &str,
    command: &str,
) -> Result<Value> {
    if budget < 0 {
        return Err(Error::new(
            "invalid_input",
            "budget_minutes cannot be negative",
        ));
    }
    let queue_time = DateTime::parse_from_rfc3339(at)
        .map_err(|_| Error::new("invalid_time", "now must be RFC3339"))?
        .with_timezone(&Utc);
    let tx = store.connection.transaction()?;
    require_subject(&tx, subject)?;
    if let Some(goal_bank) = goal_bank {
        let target = bank(&tx, goal_bank)?;
        if target["subject_id"] != subject {
            return Err(Error::new(
                "target_subject_mismatch",
                "goal bank belongs to another subject",
            ));
        }
    }
    let configured: Option<i64> = tx
        .query_row(
            "SELECT daily_budget_minutes FROM review_controls WHERE subject_id=?1",
            [subject],
            |r| r.get(0),
        )
        .optional()?;
    let effective_budget = configured.map_or(budget, |hard| hard.min(budget));
    let mut stmt=tx.prepare("SELECT d.item_id,d.due_at,d.estimated_minutes,c.card_json,CASE WHEN ?2 IS NOT NULL AND EXISTS(SELECT 1 FROM bank_items goal WHERE goal.bank_id=?2 AND goal.item_id=i.id AND goal.item_revision=i.revision) THEN 1.0 ELSE 0.0 END FROM review_debt d JOIN fsrs_cards c ON c.item_id=d.item_id JOIN practice_items i ON i.id=d.item_id AND i.revision=(SELECT MAX(revision) FROM practice_items WHERE id=d.item_id) WHERE d.subject_id=?1 AND d.state='open' AND i.state='verified' AND i.item_type='flashcard'")?;
    let raw_due = stmt
        .query_map(params![subject, goal_bank], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut due = raw_due
        .into_iter()
        .map(|(id, due_at, minutes, card_json, goal_value)| {
            let due_time = DateTime::parse_from_rfc3339(&due_at)
                .map_err(|_| Error::new("corrupt_store", "FSRS due_at is invalid"))?
                .with_timezone(&Utc);
            let card: Card = serde_json::from_str(&card_json)
                .map_err(|error| Error::new("corrupt_store", error.to_string()))?;
            let overdue = (queue_time - due_time).num_seconds();
            let forgetting_risk = 1.0 - card.get_retrievability(queue_time);
            Ok((
                id,
                due_at,
                minutes,
                overdue,
                forgetting_risk,
                goal_value,
                due_time,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    due.retain(|row| row.6 <= queue_time);
    due.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| b.4.total_cmp(&a.4))
            .then_with(|| b.5.total_cmp(&a.5))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut used = 0;
    let mut selected = Vec::new();
    let mut deferred = Vec::new();
    for (id, due_at, minutes, overdue_seconds, forgetting_risk, goal_value, _) in due {
        if used + minutes <= effective_budget {
            used += minutes;
            selected.push(json!({"item_id":id,"due_at":due_at,"estimated_minutes":minutes,"overdue_seconds":overdue_seconds,"forgetting_risk":forgetting_risk,"goal_value":goal_value}));
        } else {
            deferred.push(json!({"item_id":id,"due_at":due_at,"estimated_minutes":minutes,"overdue_seconds":overdue_seconds,"forgetting_risk":forgetting_risk,"goal_value":goal_value}));
        }
    }
    let debt_minutes: i64 = deferred
        .iter()
        .map(|v| v["estimated_minutes"].as_i64().unwrap())
        .sum();
    let value = envelope(
        Plugin::Practice,
        command,
        json!({"selected":selected,"selected_minutes":used,"budget_minutes":effective_budget,"configured_budget_minutes":configured,"debt":{"count":deferred.len(),"minutes":debt_minutes,"items":deferred}}),
    );
    tx.commit()?;
    Ok(value)
}

fn status(connection: &Connection) -> Result<Value> {
    let count = |table: &str| -> Result<i64> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        Ok(connection.query_row(&sql, [], |r| r.get(0))?)
    };
    Ok(envelope(
        Plugin::Practice,
        "status",
        json!({"banks":count("practice_banks")?,"items":count("practice_items")?,"sets":count("practice_sets")?,"papers":count("papers")?,"attempts":count("attempts")?,"responses":count("responses")?,"grades":count("grades")?,"reviews":count("review_events")?}),
    ))
}

fn require_subject(connection: &Connection, id: &str) -> Result<()> {
    if connection
        .query_row("SELECT 1 FROM subjects WHERE id=?1", [id], |_| Ok(()))
        .optional()?
        .is_none()
    {
        return Err(Error::new("subject_not_found", "subject was not found"));
    }
    Ok(())
}
fn require_revision(value: &Value, expected: i64) -> Result<()> {
    if value["revision"] != expected {
        return Err(Error::new("stale_revision", "if_revision does not match")
            .details(json!({"expected":expected,"current":value["revision"]})));
    }
    Ok(())
}
fn parse(raw: String) -> Result<Value> {
    serde_json::from_str(&raw).map_err(|e| Error::new("corrupt_store", e.to_string()))
}

fn bank(connection: &Connection, id: &str) -> Result<Value> {
    let r=connection.query_row("SELECT id,subject_id,bank_key,title,source_json,revision,created_at,(SELECT COUNT(*) FROM bank_items WHERE bank_id=practice_banks.id) FROM practice_banks WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?))).optional()?.ok_or_else(||Error::new("bank_not_found","bank was not found"))?;
    Ok(
        json!({"id":r.0,"subject_id":r.1,"key":r.2,"title":r.3,"source":parse(r.4)?,"revision":r.5,"created_at":r.6,"item_count":r.7}),
    )
}

fn item(connection: &Connection, id: &str, revision: Option<i64>) -> Result<Value> {
    let r=if let Some(rev)=revision{connection.query_row("SELECT id,revision,subject_id,item_type,grading_kind,state,prompt,answer_json,rubric,max_points,estimated_minutes,difficulty,topic,source_json,created_at FROM practice_items WHERE id=?1 AND revision=?2",params![id,rev],item_row).optional()?}else{connection.query_row("SELECT id,revision,subject_id,item_type,grading_kind,state,prompt,answer_json,rubric,max_points,estimated_minutes,difficulty,topic,source_json,created_at FROM practice_items WHERE id=?1 ORDER BY revision DESC LIMIT 1",[id],item_row).optional()?}.ok_or_else(||Error::new("item_not_found","item revision was not found"))?;
    Ok(
        json!({"id":r.0,"revision":r.1,"subject_id":r.2,"item_type":r.3,"grading_kind":r.4,"state":r.5,"prompt":r.6,"answer":parse(r.7)?,"rubric":r.8,"max_points":r.9,"estimated_minutes":r.10,"difficulty":r.11,"topic":r.12,"source":parse(r.13)?,"created_at":r.14}),
    )
}
type ItemRow = (
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    f64,
    i64,
    f64,
    Option<String>,
    String,
    String,
);
fn item_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ItemRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
    ))
}

fn set_value(connection: &Connection, id: &str) -> Result<Value> {
    connection.query_row("SELECT id,subject_id,name,kind,state,revision,created_at,(SELECT COUNT(*) FROM set_members WHERE set_id=practice_sets.id AND state='active') FROM practice_sets WHERE id=?1",[id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"subject_id":r.get::<_,String>(1)?,"name":r.get::<_,String>(2)?,"kind":r.get::<_,String>(3)?,"state":r.get::<_,String>(4)?,"revision":r.get::<_,i64>(5)?,"created_at":r.get::<_,String>(6)?,"active_count":r.get::<_,i64>(7)?}))).optional()?.ok_or_else(||Error::new("set_not_found","set was not found"))
}
fn member(connection: &Connection, set_id: &str, item_id: &str) -> Result<Value> {
    connection.query_row("SELECT item_revision,state,revision,(SELECT COUNT(*) FROM set_member_events e WHERE e.set_id=m.set_id AND e.item_id=m.item_id AND e.item_revision=m.item_revision) FROM set_members m WHERE set_id=?1 AND item_id=?2 ORDER BY item_revision DESC LIMIT 1",params![set_id,item_id],|r|Ok(json!({"set_id":set_id,"item_id":item_id,"item_revision":r.get::<_,i64>(0)?,"state":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"event_count":r.get::<_,i64>(3)?}))).optional()?.ok_or_else(||Error::new("set_member_not_found","set member was not found"))
}
fn insert_member_event(
    tx: &Transaction<'_>,
    set_id: &str,
    item_id: &str,
    item_revision: i64,
    action: &str,
    reason: &str,
    request: &str,
) -> Result<()> {
    if reason.trim().is_empty() {
        return Err(Error::new("invalid_input", "set event reason is required"));
    }
    tx.execute(
        "INSERT INTO set_member_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            new_id(Plugin::Practice, request),
            set_id,
            item_id,
            item_revision,
            action,
            reason,
            now(tx)?
        ],
    )?;
    Ok(())
}

fn paper(connection: &Connection, id: &str) -> Result<Value> {
    let row=connection.query_row("SELECT id,bank_id,subject_id,duration_minutes,revision,created_at FROM papers WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?))).optional()?.ok_or_else(||Error::new("paper_not_found","paper was not found"))?;
    let mut stmt=connection.prepare("SELECT id,ordinal,section,item_id,item_revision,prompt,answer_json,rubric,source_json,points,item_type,grading_kind FROM paper_items WHERE paper_id=?1 ORDER BY ordinal")?;
    let items=stmt.query_map([id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"ordinal":r.get::<_,i64>(1)?,"section":r.get::<_,String>(2)?,"item_id":r.get::<_,String>(3)?,"item_revision":r.get::<_,i64>(4)?,"prompt":r.get::<_,String>(5)?,"answer":serde_json::from_str::<Value>(&r.get::<_,String>(6)?).unwrap(),"rubric":r.get::<_,Option<String>>(7)?,"source":serde_json::from_str::<Value>(&r.get::<_,String>(8)?).unwrap(),"points":r.get::<_,f64>(9)?,"item_type":r.get::<_,String>(10)?,"grading_kind":r.get::<_,String>(11)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(
        json!({"id":row.0,"bank_id":row.1,"subject_id":row.2,"duration_minutes":row.3,"revision":row.4,"created_at":row.5,"items":items}),
    )
}

fn attempt(connection: &Connection, id: &str, with_responses: bool) -> Result<Value> {
    let row=connection.query_row("SELECT id,paper_id,owner,state,revision,created_at,updated_at FROM attempts WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).optional()?.ok_or_else(||Error::new("attempt_not_found","attempt was not found"))?;
    let responses = if with_responses {
        let mut stmt = connection
            .prepare("SELECT id FROM responses WHERE attempt_id=?1 ORDER BY paper_item_id")?;
        stmt.query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|rid| response(connection, &rid, true))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Ok(
        json!({"id":row.0,"paper_id":row.1,"owner":row.2,"state":row.3,"revision":row.4,"created_at":row.5,"updated_at":row.6,"responses":responses}),
    )
}

fn response(connection: &Connection, id: &str, history: bool) -> Result<Value> {
    let row=connection.query_row("SELECT id,attempt_id,paper_item_id,format,value_json,revision,updated_at FROM responses WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?))).optional()?.ok_or_else(||Error::new("response_not_found","response was not found"))?;
    let events = if history {
        let mut stmt=connection.prepare("SELECT revision,format,value_json,saved_at FROM response_history WHERE response_id=?1 ORDER BY revision")?;
        stmt.query_map([id],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"format":r.get::<_,String>(1)?,"value":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap(),"saved_at":r.get::<_,String>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?
    } else {
        Vec::new()
    };
    Ok(
        json!({"id":row.0,"attempt_id":row.1,"paper_item_id":row.2,"format":row.3,"value":parse(row.4)?,"revision":row.5,"updated_at":row.6,"history":events}),
    )
}

fn grade(connection: &Connection, id: &str) -> Result<Value> {
    let row=connection.query_row("SELECT id,item_id,item_revision,response_id,score,max_points,rationale,confidence,method,state,revision,created_at FROM grades WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,f64>(4)?,r.get::<_,f64>(5)?,r.get::<_,String>(6)?,r.get::<_,f64>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,i64>(10)?,r.get::<_,String>(11)?))).optional()?.ok_or_else(||Error::new("grade_not_found","grade was not found"))?;
    let mut stmt=connection.prepare("SELECT revision,score,rationale,confidence,method,state,created_at FROM grade_history WHERE grade_id=?1 ORDER BY revision")?;
    let history=stmt.query_map([id],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"score":r.get::<_,f64>(1)?,"rationale":r.get::<_,String>(2)?,"confidence":r.get::<_,f64>(3)?,"method":r.get::<_,String>(4)?,"state":r.get::<_,String>(5)?,"created_at":r.get::<_,String>(6)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(
        json!({"id":row.0,"item_id":row.1,"item_revision":row.2,"response_id":row.3,"score":row.4,"max_points":row.5,"rationale":row.6,"confidence":row.7,"method":row.8,"state":row.9,"revision":row.10,"created_at":row.11,"history":history}),
    )
}

fn card_value(connection: &Connection, item_id: &str) -> Result<Value> {
    let row=connection.query_row("SELECT subject_id,card_json,due_at,estimated_minutes,scheduler_crate,scheduler_version,parameters_json,revision FROM fsrs_cards WHERE item_id=?1",[item_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,i64>(7)?))).optional()?.ok_or_else(||Error::new("review_state_not_found","review state was not found"))?;
    let mut stmt=connection.prepare("SELECT ordinal,rating,reviewed_at,scheduled_days FROM review_events WHERE item_id=?1 ORDER BY ordinal")?;
    let events=stmt.query_map([item_id],|r|Ok(json!({"ordinal":r.get::<_,i64>(0)?,"rating":r.get::<_,i64>(1)?,"reviewed_at":r.get::<_,String>(2)?,"scheduled_days":r.get::<_,i64>(3)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(
        json!({"item_id":item_id,"subject_id":row.0,"state":parse(row.1)?,"due_at":row.2,"estimated_minutes":row.3,"scheduler":{"crate":row.4,"version":row.5,"parameters":parse(row.6)?},"revision":row.7,"events":events}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn takeover_requires_latest_exact_sync_receipt_and_rejects_old_owner() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE plugin_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
             INSERT INTO plugin_meta VALUES('store_id','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'),('revision','0'),('plugin_id','practice');
             CREATE TABLE requests(request_id TEXT PRIMARY KEY,fingerprint TEXT NOT NULL,result_json TEXT NOT NULL);
             CREATE TABLE subjects(id TEXT PRIMARY KEY,name TEXT NOT NULL,parent_id TEXT,tags_json TEXT NOT NULL,revision INTEGER NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL);
             CREATE TABLE subject_name_history(subject_id TEXT NOT NULL,revision INTEGER NOT NULL,name TEXT NOT NULL,changed_at TEXT NOT NULL,PRIMARY KEY(subject_id,revision));
             CREATE TABLE sync_receipts(session_id TEXT NOT NULL,plugin_id TEXT NOT NULL,store_id TEXT NOT NULL,source_revision INTEGER NOT NULL,resolved_revision INTEGER NOT NULL,logical_hash TEXT NOT NULL,runtime_state TEXT NOT NULL,state TEXT NOT NULL,completed_at TEXT NOT NULL,receipt_hash TEXT NOT NULL,PRIMARY KEY(plugin_id,session_id));"
        ).unwrap();
        initialize(&connection).unwrap();
        let mut store = Store {
            connection,
            root: PathBuf::new(),
        };
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("INSERT INTO subjects VALUES('11111111111111111111111111111111','s',NULL,'[]',1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
        tx.execute("INSERT INTO subject_name_history VALUES('11111111111111111111111111111111',1,'s','2026-01-01T00:00:00Z')",[]).unwrap();
        tx.execute("INSERT INTO practice_banks VALUES('22222222222222222222222222222222','11111111111111111111111111111111','subject:default','b','{\"kind\":\"tutor\",\"id\":\"t\",\"revision_or_hash\":\"h\"}',1,'2026-01-01T00:00:00Z')",[]).unwrap();
        tx.execute("INSERT INTO papers VALUES('33333333333333333333333333333333','22222222222222222222222222222222','11111111111111111111111111111111',10,1,'2026-01-01T00:00:00Z')",[]).unwrap();
        tx.execute("INSERT INTO attempts VALUES('44444444444444444444444444444444','33333333333333333333333333333333','mac-a','in_progress',1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",[]).unwrap();
        bump_store_revision(&tx).unwrap();
        tx.commit().unwrap();
        let identity = store.identity(Plugin::Practice).unwrap();
        let logical_hash = canonical_logical_hash(Plugin::Practice, &store.connection).unwrap();
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        record_sync_receipt(
            &tx,
            Plugin::Practice,
            "sync-session-exact",
            identity.revision,
            identity.revision,
            &logical_hash,
            "ready",
        )
        .unwrap();
        tx.commit().unwrap();
        let value = takeover_attempt(
            &mut store,
            AttemptTakeover {
                entity_id: "44444444444444444444444444444444".into(),
                old_owner: "mac-a".into(),
                new_owner: "mac-b".into(),
                if_revision: 1,
                sync_session_id: "sync-session-exact".into(),
                request_id: "takeover-exact".into(),
            },
        )
        .unwrap();
        assert_eq!(value["result"]["attempt"]["owner"], "mac-b");
        assert_eq!(value["result"]["attempt"]["revision"], 2);
        let stale = takeover_attempt(
            &mut store,
            AttemptTakeover {
                entity_id: "44444444444444444444444444444444".into(),
                old_owner: "mac-b".into(),
                new_owner: "mac-c".into(),
                if_revision: 2,
                sync_session_id: "sync-session-exact".into(),
                request_id: "takeover-stale-receipt".into(),
            },
        )
        .unwrap_err();
        assert_eq!(stale.code(), "stale_sync_receipt");
    }
}
