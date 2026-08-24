use crate::learning::*;
use clap::Subcommand;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Subcommand)]
pub(crate) enum PlanCommand {
    Create {
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Show {
        id: String,
    },
    Revise {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
    Rollback {
        id: String,
        #[arg(long)]
        if_revision: i64,
        #[arg(long)]
        to_revision: i64,
        #[arg(long, value_name = "JSON|-|@PATH")]
        json: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanCreate {
    subject_id: String,
    goal_id: String,
    mode: String,
    deadline: String,
    weekly_minutes: i64,
    core_content: Vec<String>,
    order: Vec<String>,
    pace: String,
    method: String,
    exercise_ratio: f64,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanRevise {
    actor: String,
    trigger: String,
    reason: String,
    evidence_refs: Vec<String>,
    #[serde(default)]
    goal_id: Option<String>,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    weekly_minutes: Option<i64>,
    #[serde(default)]
    core_content: Option<Vec<String>>,
    #[serde(default)]
    order: Option<Vec<String>>,
    #[serde(default)]
    pace: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    exercise_ratio: Option<f64>,
    request_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanRollback {
    actor: String,
    reason: String,
    evidence_refs: Vec<String>,
    request_id: String,
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tutor_plans(
           id TEXT PRIMARY KEY,
           subject_id TEXT NOT NULL REFERENCES subjects(id),
           goal_id TEXT NOT NULL REFERENCES tutor_goals(id),
           mode TEXT NOT NULL CHECK(mode IN ('fixed','adaptive','agent-led')),
           status TEXT NOT NULL CHECK(status IN ('active','completed','abandoned')),
           current_revision INTEGER NOT NULL CHECK(current_revision>=1),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS tutor_plans_subject
           ON tutor_plans(subject_id,status,id);
         CREATE TABLE IF NOT EXISTS tutor_plan_versions(
           plan_id TEXT NOT NULL REFERENCES tutor_plans(id),
           revision INTEGER NOT NULL,
           goal_id TEXT NOT NULL REFERENCES tutor_goals(id),
           deadline TEXT NOT NULL,
           weekly_minutes INTEGER NOT NULL CHECK(weekly_minutes>0),
           core_content_json TEXT NOT NULL,
           order_json TEXT NOT NULL,
           pace TEXT NOT NULL CHECK(trim(pace)<>''),
           method TEXT NOT NULL CHECK(trim(method)<>''),
           exercise_ratio REAL NOT NULL CHECK(exercise_ratio>=0 AND exercise_ratio<=1),
           actor TEXT NOT NULL CHECK(actor IN ('learner','agent')),
           trigger TEXT NOT NULL,
           reason TEXT NOT NULL,
           evidence_refs_json TEXT NOT NULL,
           rolled_back_to INTEGER,
           created_at TEXT NOT NULL,
           PRIMARY KEY(plan_id,revision)
         );",
    )?;
    Ok(())
}

pub(crate) fn run(store: &mut Store, command: PlanCommand) -> Result<Value> {
    match command {
        PlanCommand::Create { json } => create(store, read_json(&json)?),
        PlanCommand::Show { id } => Ok(envelope(
            Plugin::Tutor,
            "plan.show",
            json!({"plan": current_plan(&store.connection, &id)?}),
        )),
        PlanCommand::Revise {
            id,
            if_revision,
            json,
        } => revise(store, &id, if_revision, read_json(&json)?),
        PlanCommand::Rollback {
            id,
            if_revision,
            to_revision,
            json,
        } => rollback(store, &id, if_revision, to_revision, read_json(&json)?),
    }
}

fn create(store: &mut Store, input: PlanCreate) -> Result<Value> {
    validate_request_id(&input.request_id)?;
    validate_mode(&input.mode)?;
    validate_snapshot(
        &store.connection,
        &input.deadline,
        input.weekly_minutes,
        &input.core_content,
        &input.order,
        &input.pace,
        &input.method,
        input.exercise_ratio,
    )?;
    require_subject_goal(&store.connection, &input.subject_id, &input.goal_id)?;
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
        "INSERT INTO tutor_plans(
           id,subject_id,goal_id,mode,status,current_revision,created_at,updated_at
         ) VALUES(?1,?2,?3,?4,'active',1,?5,?5)",
        params![id, input.subject_id, input.goal_id, input.mode, timestamp],
    )?;
    insert_version(
        &tx,
        &id,
        1,
        &input.goal_id,
        &input.deadline,
        input.weekly_minutes,
        &input.core_content,
        &input.order,
        &input.pace,
        &input.method,
        input.exercise_ratio,
        "learner",
        "create",
        "initial plan",
        &[],
        None,
        &timestamp,
    )?;
    let plan = current_plan(&tx, &id)?;
    let value = envelope(Plugin::Tutor, "plan.create", json!({"plan": plan}));
    remember(&tx, &input.request_id, &fingerprint, &value)?;
    tx.commit()?;
    Ok(value)
}

fn revise(store: &mut Store, id: &str, if_revision: i64, input: PlanRevise) -> Result<Value> {
    validate_id(id)?;
    validate_revision(if_revision)?;
    validate_change_metadata(
        &input.actor,
        &input.trigger,
        &input.reason,
        &input.evidence_refs,
        &input.request_id,
    )?;
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
    let current = current_plan(&tx, id)?;
    require_current_revision(&current, if_revision)?;
    if current["status"] != "active" {
        return Err(Error::new("plan_not_active", "plan is not active"));
    }
    let changes = requested_changes(&input);
    if changes.is_empty() {
        return Err(Error::new("invalid_input", "plan revision has no changes"));
    }
    authorize_changes(&current, &input.actor, &input.trigger, &changes)?;
    let next = apply_changes(&current, &input)?;
    validate_snapshot_value(&tx, &next)?;
    if let Some(goal_id) = input.goal_id.as_deref() {
        require_subject_goal(&tx, current["subject_id"].as_str().unwrap(), goal_id)?;
    }
    let next_revision = if_revision + 1;
    let timestamp = now(&tx)?;
    insert_snapshot_version(
        &tx,
        id,
        next_revision,
        &next,
        &input.actor,
        &input.trigger,
        &input.reason,
        &input.evidence_refs,
        None,
        &timestamp,
    )?;
    tx.execute(
        "UPDATE tutor_plans SET goal_id=?2,current_revision=?3,updated_at=?4 WHERE id=?1",
        params![
            id,
            next["goal_id"].as_str().unwrap(),
            next_revision,
            timestamp
        ],
    )?;
    let plan = current_plan(&tx, id)?;
    let value = envelope(
        Plugin::Tutor,
        "plan.revise",
        json!({"plan": plan, "diff": diff(&current, &plan)}),
    );
    remember(&tx, &input.request_id, &fingerprint, &value)?;
    tx.commit()?;
    Ok(value)
}

fn rollback(
    store: &mut Store,
    id: &str,
    if_revision: i64,
    to_revision: i64,
    input: PlanRollback,
) -> Result<Value> {
    validate_id(id)?;
    validate_revision(if_revision)?;
    validate_revision(to_revision)?;
    validate_change_metadata(
        &input.actor,
        "rollback",
        &input.reason,
        &input.evidence_refs,
        &input.request_id,
    )?;
    if to_revision >= if_revision {
        return Err(Error::new(
            "invalid_input",
            "rollback target must be an older revision",
        ));
    }
    let fingerprint = fingerprint(&json!({
        "id": id,
        "if_revision": if_revision,
        "to_revision": to_revision,
        "input": &input,
    }))?;
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(value) = replay(&tx, &input.request_id, &fingerprint)? {
        return Ok(value);
    }
    let current = current_plan(&tx, id)?;
    require_current_revision(&current, if_revision)?;
    let target = plan_at(&tx, id, to_revision)?;
    let changes = changed_fields(&current, &target);
    authorize_changes(&current, &input.actor, "rollback", &changes)?;
    let next_revision = if_revision + 1;
    let timestamp = now(&tx)?;
    insert_snapshot_version(
        &tx,
        id,
        next_revision,
        &target,
        &input.actor,
        "rollback",
        &input.reason,
        &input.evidence_refs,
        Some(to_revision),
        &timestamp,
    )?;
    tx.execute(
        "UPDATE tutor_plans SET goal_id=?2,current_revision=?3,updated_at=?4 WHERE id=?1",
        params![
            id,
            target["goal_id"].as_str().unwrap(),
            next_revision,
            timestamp
        ],
    )?;
    let plan = current_plan(&tx, id)?;
    let value = envelope(
        Plugin::Tutor,
        "plan.rollback",
        json!({
            "plan": plan,
            "rolled_back_to": to_revision,
            "diff": diff(&current, &plan),
        }),
    );
    remember(&tx, &input.request_id, &fingerprint, &value)?;
    tx.commit()?;
    Ok(value)
}

fn validate_mode(mode: &str) -> Result<()> {
    if matches!(mode, "fixed" | "adaptive" | "agent-led") {
        Ok(())
    } else {
        Err(Error::new(
            "invalid_input",
            "mode must be fixed, adaptive, or agent-led",
        ))
    }
}

#[allow(clippy::too_many_arguments)] // Mirrors the eight persisted plan fields at one validation boundary.
fn validate_snapshot(
    connection: &Connection,
    deadline: &str,
    weekly_minutes: i64,
    core_content: &[String],
    order: &[String],
    pace: &str,
    method: &str,
    exercise_ratio: f64,
) -> Result<()> {
    let bytes = deadline.as_bytes();
    let explicit_timezone = deadline.ends_with('Z')
        || (bytes.len() >= 6
            && matches!(bytes[bytes.len() - 6], b'+' | b'-')
            && bytes[bytes.len() - 5].is_ascii_digit()
            && bytes[bytes.len() - 4].is_ascii_digit()
            && bytes[bytes.len() - 3] == b':'
            && bytes[bytes.len() - 2].is_ascii_digit()
            && bytes[bytes.len() - 1].is_ascii_digit());
    let valid = connection.query_row("SELECT julianday(?1) IS NOT NULL", [deadline], |row| {
        row.get::<_, bool>(0)
    })?;
    if !explicit_timezone || !valid {
        return Err(Error::new(
            "invalid_input",
            "deadline must be RFC3339 with an explicit timezone",
        ));
    }
    if weekly_minutes <= 0 || weekly_minutes > 7 * 24 * 60 {
        return Err(Error::new(
            "invalid_input",
            "weekly_minutes must be within 1..=10080",
        ));
    }
    validate_list("core_content", core_content)?;
    validate_list("order", order)?;
    validate_string("pace", pace)?;
    validate_string("method", method)?;
    if !exercise_ratio.is_finite() || !(0.0..=1.0).contains(&exercise_ratio) {
        return Err(Error::new(
            "invalid_input",
            "exercise_ratio must be between 0 and 1",
        ));
    }
    Ok(())
}

fn validate_snapshot_value(connection: &Connection, value: &Value) -> Result<()> {
    validate_snapshot(
        connection,
        value["deadline"].as_str().unwrap(),
        value["weekly_minutes"].as_i64().unwrap(),
        &strings(value, "core_content")?,
        &strings(value, "order")?,
        value["pace"].as_str().unwrap(),
        value["method"].as_str().unwrap(),
        value["exercise_ratio"].as_f64().unwrap(),
    )
}

fn validate_list(field: &str, values: &[String]) -> Result<()> {
    if values.is_empty() || values.len() > 256 {
        return Err(Error::new(
            "invalid_input",
            format!("{field} must contain 1..=256 entries"),
        ));
    }
    for value in values {
        validate_string(field, value)?;
    }
    Ok(())
}

fn validate_string(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 16 * 1024 {
        Err(Error::new(
            "invalid_input",
            format!("{field} must contain 1..=16384 UTF-8 bytes"),
        ))
    } else {
        Ok(())
    }
}

fn require_subject_goal(connection: &Connection, subject_id: &str, goal_id: &str) -> Result<()> {
    validate_id(subject_id)?;
    validate_id(goal_id)?;
    let goal_subject = connection
        .query_row(
            "SELECT subject_id FROM tutor_goals WHERE id=?1",
            [goal_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| Error::new("goal_not_found", format!("goal {goal_id} was not found")))?;
    if goal_subject != subject_id {
        return Err(Error::new(
            "goal_subject_mismatch",
            "goal does not belong to the plan subject",
        ));
    }
    Ok(())
}

fn validate_change_metadata(
    actor: &str,
    trigger: &str,
    reason: &str,
    evidence_refs: &[String],
    request_id: &str,
) -> Result<()> {
    validate_request_id(request_id)?;
    if !matches!(actor, "learner" | "agent") {
        return Err(Error::new(
            "invalid_input",
            "actor must be learner or agent",
        ));
    }
    validate_string("trigger", trigger)?;
    validate_string("reason", reason)?;
    normalized_evidence(evidence_refs)?;
    Ok(())
}

fn normalized_evidence(values: &[String]) -> Result<Vec<String>> {
    let mut values = values.to_vec();
    for value in &mut values {
        *value = value.trim().to_owned();
        if value.is_empty() || value.len() > 512 {
            return Err(Error::new(
                "invalid_input",
                "evidence refs must contain 1..=512 UTF-8 bytes",
            ));
        }
    }
    values.sort();
    values.dedup();
    if values.is_empty() || values.len() > 256 {
        return Err(Error::new(
            "invalid_input",
            "plan changes require 1..=256 evidence refs",
        ));
    }
    Ok(values)
}

fn requested_changes(input: &PlanRevise) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if input.goal_id.is_some() {
        fields.push("goal_id");
    }
    if input.deadline.is_some() {
        fields.push("deadline");
    }
    if input.weekly_minutes.is_some() {
        fields.push("weekly_minutes");
    }
    if input.core_content.is_some() {
        fields.push("core_content");
    }
    if input.order.is_some() {
        fields.push("order");
    }
    if input.pace.is_some() {
        fields.push("pace");
    }
    if input.method.is_some() {
        fields.push("method");
    }
    if input.exercise_ratio.is_some() {
        fields.push("exercise_ratio");
    }
    fields
}

fn authorize_changes(current: &Value, actor: &str, trigger: &str, fields: &[&str]) -> Result<()> {
    if actor == "learner" {
        return Ok(());
    }
    if trigger == "single_error" {
        return Err(Error::new(
            "adjustment_not_meaningful",
            "one wrong answer cannot trigger a plan revision",
        ));
    }
    if fields.iter().any(|field| {
        matches!(
            *field,
            "goal_id" | "deadline" | "weekly_minutes" | "core_content"
        )
    }) {
        return Err(Error::new(
            "learner_owned_field",
            "Agent cannot change learner-owned plan fields",
        ));
    }
    if current["mode"] == "fixed" && !fields.is_empty() {
        return Err(Error::new(
            "fixed_plan_requires_learner",
            "fixed-plan execution changes require the learner",
        ));
    }
    Ok(())
}

fn apply_changes(current: &Value, input: &PlanRevise) -> Result<Value> {
    let mut next = current.clone();
    let object = next.as_object_mut().unwrap();
    set_optional(object, "goal_id", input.goal_id.as_ref());
    set_optional(object, "deadline", input.deadline.as_ref());
    if let Some(value) = input.weekly_minutes {
        object.insert("weekly_minutes".to_owned(), json!(value));
    }
    if let Some(value) = &input.core_content {
        object.insert("core_content".to_owned(), json!(value));
    }
    if let Some(value) = &input.order {
        object.insert("order".to_owned(), json!(value));
    }
    set_optional(object, "pace", input.pace.as_ref());
    set_optional(object, "method", input.method.as_ref());
    if let Some(value) = input.exercise_ratio {
        object.insert("exercise_ratio".to_owned(), json!(value));
    }
    Ok(next)
}

fn set_optional(object: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), json!(value));
    }
}

fn changed_fields<'a>(before: &Value, after: &Value) -> Vec<&'a str> {
    const FIELDS: [&str; 8] = [
        "goal_id",
        "deadline",
        "weekly_minutes",
        "core_content",
        "order",
        "pace",
        "method",
        "exercise_ratio",
    ];
    FIELDS
        .into_iter()
        .filter(|field| before[*field] != after[*field])
        .collect()
}

fn diff(before: &Value, after: &Value) -> Value {
    let mut result = Map::new();
    for field in changed_fields(before, after) {
        result.insert(
            field.to_owned(),
            json!({"before": before[field], "after": after[field]}),
        );
    }
    Value::Object(result)
}

#[allow(clippy::too_many_arguments)]
fn insert_version(
    tx: &Transaction<'_>,
    plan_id: &str,
    revision: i64,
    goal_id: &str,
    deadline: &str,
    weekly_minutes: i64,
    core_content: &[String],
    order: &[String],
    pace: &str,
    method: &str,
    exercise_ratio: f64,
    actor: &str,
    trigger: &str,
    reason: &str,
    evidence_refs: &[String],
    rolled_back_to: Option<i64>,
    timestamp: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO tutor_plan_versions(
           plan_id,revision,goal_id,deadline,weekly_minutes,core_content_json,order_json,
           pace,method,exercise_ratio,actor,trigger,reason,evidence_refs_json,
           rolled_back_to,created_at
         ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            plan_id,
            revision,
            goal_id,
            deadline,
            weekly_minutes,
            serde_json::to_string(core_content)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            serde_json::to_string(order)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            pace,
            method,
            exercise_ratio,
            actor,
            trigger,
            reason,
            serde_json::to_string(&normalized_evidence_or_empty(evidence_refs)?)
                .map_err(|error| Error::new("json_error", error.to_string()))?,
            rolled_back_to,
            timestamp,
        ],
    )?;
    Ok(())
}

fn normalized_evidence_or_empty(values: &[String]) -> Result<Vec<String>> {
    if values.is_empty() {
        Ok(Vec::new())
    } else {
        normalized_evidence(values)
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_snapshot_version(
    tx: &Transaction<'_>,
    plan_id: &str,
    revision: i64,
    snapshot: &Value,
    actor: &str,
    trigger: &str,
    reason: &str,
    evidence_refs: &[String],
    rolled_back_to: Option<i64>,
    timestamp: &str,
) -> Result<()> {
    insert_version(
        tx,
        plan_id,
        revision,
        snapshot["goal_id"].as_str().unwrap(),
        snapshot["deadline"].as_str().unwrap(),
        snapshot["weekly_minutes"].as_i64().unwrap(),
        &strings(snapshot, "core_content")?,
        &strings(snapshot, "order")?,
        snapshot["pace"].as_str().unwrap(),
        snapshot["method"].as_str().unwrap(),
        snapshot["exercise_ratio"].as_f64().unwrap(),
        actor,
        trigger,
        reason,
        evidence_refs,
        rolled_back_to,
        timestamp,
    )
}

fn current_plan(connection: &Connection, id: &str) -> Result<Value> {
    validate_id(id)?;
    let base = connection
        .query_row(
            "SELECT subject_id,mode,status,current_revision,created_at,updated_at
             FROM tutor_plans WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| Error::new("plan_not_found", format!("plan {id} was not found")))?;
    let mut plan = plan_at(connection, id, base.3)?;
    let object = plan.as_object_mut().unwrap();
    object.insert("id".to_owned(), json!(id));
    object.insert("subject_id".to_owned(), json!(base.0));
    object.insert("mode".to_owned(), json!(base.1));
    object.insert("status".to_owned(), json!(base.2));
    object.insert("created_at".to_owned(), json!(base.4));
    object.insert("updated_at".to_owned(), json!(base.5));
    Ok(plan)
}

fn plan_at(connection: &Connection, id: &str, revision: i64) -> Result<Value> {
    connection
        .query_row(
            "SELECT goal_id,deadline,weekly_minutes,core_content_json,order_json,
                    pace,method,exercise_ratio,actor,trigger,reason,evidence_refs_json,
                    rolled_back_to,created_at
             FROM tutor_plan_versions WHERE plan_id=?1 AND revision=?2",
            params![id, revision],
            |row| {
                let core: String = row.get(3)?;
                let order: String = row.get(4)?;
                let evidence: String = row.get(11)?;
                let parse = |column, value: &str| {
                    serde_json::from_str::<Value>(value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            column,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                };
                Ok(json!({
                    "revision": revision,
                    "goal_id": row.get::<_, String>(0)?,
                    "deadline": row.get::<_, String>(1)?,
                    "weekly_minutes": row.get::<_, i64>(2)?,
                    "core_content": parse(3, &core)?,
                    "order": parse(4, &order)?,
                    "pace": row.get::<_, String>(5)?,
                    "method": row.get::<_, String>(6)?,
                    "exercise_ratio": row.get::<_, f64>(7)?,
                    "revision_actor": row.get::<_, String>(8)?,
                    "revision_trigger": row.get::<_, String>(9)?,
                    "revision_reason": row.get::<_, String>(10)?,
                    "revision_evidence_refs": parse(11, &evidence)?,
                    "rolled_back_to": row.get::<_, Option<i64>>(12)?,
                    "revision_created_at": row.get::<_, String>(13)?,
                }))
            },
        )
        .optional()?
        .ok_or_else(|| {
            Error::new(
                "plan_revision_not_found",
                format!("plan {id} revision {revision} was not found"),
            )
        })
}

fn strings(value: &Value, field: &str) -> Result<Vec<String>> {
    value[field]
        .as_array()
        .ok_or_else(|| Error::new("corrupt_store", format!("{field} is not an array")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::new("corrupt_store", format!("{field} contains non-text")))
        })
        .collect()
}

fn validate_revision(revision: i64) -> Result<()> {
    if revision > 0 {
        Ok(())
    } else {
        Err(Error::new("invalid_input", "revision must be positive"))
    }
}

fn require_current_revision(plan: &Value, expected: i64) -> Result<()> {
    let current = plan["revision"].as_i64().unwrap();
    if current == expected {
        Ok(())
    } else {
        Err(Error::new("revision_conflict", "plan revision is stale")
            .details(json!({"expected": expected, "current": current})))
    }
}
