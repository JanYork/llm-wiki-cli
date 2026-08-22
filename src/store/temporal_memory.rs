fn create_temporal_memory_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(&format!(
        "CREATE TABLE memory_events(
            id TEXT PRIMARY KEY CHECK(TRIM(id) <> ''),
            request_id TEXT CHECK(request_id IS NULL OR TRIM(request_id) <> ''),
            fingerprint TEXT NOT NULL CHECK(LENGTH(fingerprint) = 64),
            event_type TEXT NOT NULL CHECK(TRIM(event_type) <> ''),
            context TEXT NOT NULL CHECK(TRIM(context) <> ''),
            occurred_at TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL}),
            valid_from TEXT,
            valid_until TEXT,
            pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0, 1)),
            logical_bytes INTEGER NOT NULL CHECK(logical_bytes >= 0)
        );
        CREATE UNIQUE INDEX memory_events_request_id
        ON memory_events(request_id) WHERE request_id IS NOT NULL;
        CREATE INDEX memory_events_context
        ON memory_events(event_type, context, occurred_at DESC, id);
        CREATE INDEX memory_events_retention
        ON memory_events(occurred_at, id);

        CREATE TABLE memory_fragments(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK(kind IN (
                'observed', 'decision', 'constraint', 'learned', 'unresolved', 'outcome'
            )),
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            value TEXT NOT NULL CHECK(TRIM(value) <> ''),
            PRIMARY KEY(event_id, kind, ordinal)
        );
        CREATE INDEX memory_fragments_kind
        ON memory_fragments(kind, event_id);

        CREATE TABLE memory_changes(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            subject TEXT NOT NULL CHECK(TRIM(subject) <> ''),
            before_value TEXT,
            after_value TEXT,
            reason TEXT,
            PRIMARY KEY(event_id, ordinal)
        );

        CREATE TABLE memory_evidence(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            reference TEXT NOT NULL CHECK(TRIM(reference) <> ''),
            excerpt TEXT,
            PRIMARY KEY(event_id, ordinal)
        );

        CREATE TABLE memory_relations(
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            relation_type TEXT NOT NULL CHECK(relation_type IN (
                'supersedes', 'contradicts', 'resolves', 'supports', 'related'
            )),
            target_event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            basis TEXT,
            PRIMARY KEY(event_id, ordinal)
        );
        CREATE INDEX memory_relations_target
        ON memory_relations(target_event_id, relation_type, event_id);

        CREATE TABLE memory_feedback(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL REFERENCES memory_events(id) ON DELETE CASCADE,
            signal TEXT NOT NULL CHECK(signal IN ('useful', 'not-useful')),
            reason TEXT NOT NULL CHECK(TRIM(reason) <> ''),
            created_at TEXT NOT NULL DEFAULT ({TIMESTAMP_SQL})
        );
        CREATE INDEX memory_feedback_event
        ON memory_feedback(event_id, created_at, id);

        CREATE TABLE memory_hint_state(
            candidate_key TEXT PRIMARY KEY CHECK(TRIM(candidate_key) <> ''),
            hint_type TEXT NOT NULL CHECK(TRIM(hint_type) <> ''),
            last_emitted_at TEXT NOT NULL,
            next_eligible_at TEXT NOT NULL
        );

        CREATE TABLE memory_state(
            id INTEGER PRIMARY KEY CHECK(id = 1),
            record_attempts INTEGER NOT NULL DEFAULT 0 CHECK(record_attempts >= 0),
            inserted_events INTEGER NOT NULL DEFAULT 0 CHECK(inserted_events >= 0),
            idempotent_replays INTEGER NOT NULL DEFAULT 0 CHECK(idempotent_replays >= 0),
            feedback_useful INTEGER NOT NULL DEFAULT 0 CHECK(feedback_useful >= 0),
            feedback_not_useful INTEGER NOT NULL DEFAULT 0 CHECK(feedback_not_useful >= 0),
            age_evictions INTEGER NOT NULL DEFAULT 0 CHECK(age_evictions >= 0),
            capacity_evictions INTEGER NOT NULL DEFAULT 0 CHECK(capacity_evictions >= 0),
            event_count INTEGER NOT NULL DEFAULT 0 CHECK(event_count >= 0),
            logical_bytes INTEGER NOT NULL DEFAULT 0 CHECK(logical_bytes >= 0)
        );
        INSERT INTO memory_state(id) VALUES (1);

        CREATE VIRTUAL TABLE memory_fts USING fts5(
            event_id UNINDEXED,
            event_type,
            context_terms,
            content_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );"
    ))?;
    Ok(())
}

fn migrate_temporal_memory(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current >= TEMPORAL_MEMORY_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if current != TAGS_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {TEMPORAL_MEMORY_VERSION}"),
        ));
    }
    create_temporal_memory_schema(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{TEMPORAL_MEMORY_VERSION} temporal memory schema: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![TEMPORAL_MEMORY_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", TEMPORAL_MEMORY_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{TEMPORAL_MEMORY_VERSION} temporal memory migration: {error}"),
        )
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEventInput {
    pub request_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub context: String,
    pub occurred_at: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub observed: Vec<String>,
    #[serde(default)]
    pub decision: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub unresolved: Vec<String>,
    #[serde(default)]
    pub outcome: Vec<String>,
    #[serde(default)]
    pub changes: Vec<MemoryChangeInput>,
    #[serde(default)]
    pub evidence: Vec<MemoryEvidenceInput>,
    #[serde(default)]
    pub relations: Vec<MemoryRelationInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryChangeInput {
    pub subject: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEvidenceInput {
    pub reference: String,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRelationInput {
    #[serde(rename = "type")]
    pub relation_type: String,
    pub target: String,
    pub basis: Option<String>,
}

pub fn parse_memory_capsule(raw: &str) -> Result<MemoryEventInput> {
    let mut input: MemoryEventInput = serde_json::from_str(raw)
        .map_err(|error| AppError::new("invalid_memory_capsule", error.to_string()))?;
    input.normalize()?;
    Ok(input)
}

impl MemoryEventInput {
    fn normalize(&mut self) -> Result<()> {
        normalize_required_memory_text("type", &mut self.event_type)?;
        normalize_required_memory_text("context", &mut self.context)?;
        normalize_optional_memory_text("request_id", &mut self.request_id)?;
        for (name, values) in [
            ("observed", &mut self.observed),
            ("decision", &mut self.decision),
            ("constraints", &mut self.constraints),
            ("learned", &mut self.learned),
            ("unresolved", &mut self.unresolved),
            ("outcome", &mut self.outcome),
        ] {
            for value in values {
                normalize_required_memory_text(name, value)?;
            }
        }
        for change in &mut self.changes {
            normalize_required_memory_text("changes.subject", &mut change.subject)?;
            normalize_optional_memory_text("changes.before", &mut change.before)?;
            normalize_optional_memory_text("changes.after", &mut change.after)?;
            normalize_optional_memory_text("changes.reason", &mut change.reason)?;
            if change.before.is_none() && change.after.is_none() {
                return Err(invalid_memory_capsule(
                    "each change requires before or after",
                ));
            }
        }
        for evidence in &mut self.evidence {
            normalize_required_memory_text("evidence.reference", &mut evidence.reference)?;
            normalize_optional_memory_text("evidence.excerpt", &mut evidence.excerpt)?;
        }
        for relation in &mut self.relations {
            normalize_required_memory_text("relations.type", &mut relation.relation_type)?;
            normalize_required_memory_text("relations.target", &mut relation.target)?;
            normalize_optional_memory_text("relations.basis", &mut relation.basis)?;
            if !matches!(
                relation.relation_type.as_str(),
                "supersedes" | "contradicts" | "resolves" | "supports" | "related"
            ) {
                return Err(invalid_memory_capsule(format!(
                    "unsupported relation type '{}'",
                    relation.relation_type
                )));
            }
        }
        if self.observed.is_empty()
            && self.decision.is_empty()
            && self.constraints.is_empty()
            && self.learned.is_empty()
            && self.unresolved.is_empty()
            && self.outcome.is_empty()
            && self.changes.is_empty()
        {
            return Err(invalid_memory_capsule(
                "a memory capsule requires at least one semantic entry",
            ));
        }
        Ok(())
    }
}

fn normalize_required_memory_text(name: &str, value: &mut String) -> Result<()> {
    *value = value.trim().to_owned();
    if value.is_empty() {
        return Err(invalid_memory_capsule(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

fn normalize_optional_memory_text(name: &str, value: &mut Option<String>) -> Result<()> {
    if let Some(value) = value {
        normalize_required_memory_text(name, value)?;
    }
    Ok(())
}

fn invalid_memory_capsule(message: impl Into<String>) -> AppError {
    AppError::new("invalid_memory_capsule", message)
}

impl Store {
    pub fn remember(&mut self, mut input: MemoryEventInput) -> Result<Value> {
        let settings = config::resolve_memory(&self.scope, &self.database)?;
        if settings.setting == config::MemorySetting::Disabled {
            return Err(AppError::new(
                "memory_disabled",
                "temporal memory is disabled for this scope",
            ));
        }
        let scope = self.scope.clone();
        let database = self.database.to_string_lossy().into_owned();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        normalize_memory_timestamps(&tx, &mut input)?;
        let fingerprint = memory_fingerprint(&input)?;

        if let Some(request_id) = input.request_id.as_deref()
            && let Some((event_id, stored_fingerprint)) = tx
                .query_row(
                    "SELECT id, fingerprint FROM memory_events WHERE request_id = ?1",
                    [request_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
        {
            if stored_fingerprint != fingerprint {
                return Err(AppError::new(
                    "memory_request_conflict",
                    format!("request_id '{request_id}' was already used for different content"),
                ));
            }
            tx.execute(
                "UPDATE memory_state
                 SET record_attempts = record_attempts + 1,
                     idempotent_replays = idempotent_replays + 1
                 WHERE id = 1",
                [],
            )?;
            let event = load_memory_event(&tx, &event_id)?;
            let pressure = memory_pressure(&tx, settings.max_bytes)?;
            tx.commit()?;
            return Ok(json!({
                "scope": scope,
                "database": database,
                "created": false,
                "event": event,
                "retention": empty_memory_retention(),
                "pressure": pressure,
                "hints": [],
            }));
        }

        let (event_id, recorded_at): (String, String) = tx.query_row(
            &format!("SELECT LOWER(HEX(RANDOMBLOB(16))), {TIMESTAMP_SQL}"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let occurred_at = match input.occurred_at.as_deref() {
            Some(value) => value.to_owned(),
            None => recorded_at.clone(),
        };
        let logical_bytes = memory_logical_bytes(&input)?;
        tx.execute(
            "INSERT INTO memory_events(
                id, request_id, fingerprint, event_type, context,
                occurred_at, recorded_at, valid_from, valid_until, pinned, logical_bytes
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                &event_id,
                &input.request_id,
                &fingerprint,
                &input.event_type,
                &input.context,
                &occurred_at,
                &recorded_at,
                &input.valid_from,
                &input.valid_to,
                i64::from(input.pinned),
                logical_bytes,
            ],
        )?;
        insert_memory_fragments(&tx, &event_id, &input)?;
        insert_memory_changes(&tx, &event_id, &input.changes)?;
        insert_memory_evidence(&tx, &event_id, &input.evidence)?;
        insert_memory_relations(&tx, &event_id, &input.relations)?;
        index_memory_event(&tx, &event_id, &input)?;
        tx.execute(
            "UPDATE memory_state
             SET record_attempts = record_attempts + 1,
                 inserted_events = inserted_events + 1,
                 event_count = event_count + 1,
                 logical_bytes = logical_bytes + ?1
             WHERE id = 1",
            [logical_bytes],
        )?;
        record_operation(
            &tx,
            "memory_remember",
            &event_id,
            &json!({
                "logical_bytes": logical_bytes,
                "request_id_present": input.request_id.is_some(),
            }),
        )?;
        let event = load_memory_event(&tx, &event_id)?;
        let retention = enforce_memory_retention(
            &tx,
            settings.max_age_days,
            settings.max_bytes,
            Some(&event_id),
            true,
        )?;
        let pressure = memory_pressure(&tx, settings.max_bytes)?;
        let retained: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
            [&event_id],
            |row| row.get(0),
        )?;
        let hints = if retained {
            emit_memory_hints(&tx, &event_id, &input, settings.max_bytes)?
        } else {
            prune_memory_hint_state(&tx, settings.max_bytes, false)?;
            Vec::new()
        };
        tx.commit()?;
        Ok(json!({
            "scope": scope,
            "database": database,
            "created": true,
            "event": event,
            "retention": retention,
            "pressure": pressure,
            "hints": hints,
        }))
    }
}

fn normalize_memory_timestamps(
    conn: &Connection,
    input: &mut MemoryEventInput,
) -> Result<()> {
    for (name, value) in [
        ("occurred_at", &mut input.occurred_at),
        ("valid_from", &mut input.valid_from),
        ("valid_to", &mut input.valid_to),
    ] {
        let Some(raw) = value.as_deref() else {
            continue;
        };
        let normalized = conn
            .query_row(
                "SELECT STRFTIME('%Y-%m-%dT%H:%M:%fZ', ?1)",
                [raw],
                |row| row.get::<_, Option<String>>(0),
            )?
            .ok_or_else(|| invalid_memory_capsule(format!("{name} is not a valid timestamp")))?;
        *value = Some(normalized);
    }
    if let (Some(valid_from), Some(valid_to)) = (&input.valid_from, &input.valid_to)
        && valid_from > valid_to
    {
        return Err(invalid_memory_capsule(
            "valid_from must not be later than valid_to",
        ));
    }
    Ok(())
}

fn memory_fingerprint(input: &MemoryEventInput) -> Result<String> {
    let mut canonical = input.clone();
    canonical.request_id = None;
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| AppError::new("json_error", error.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn memory_logical_bytes(input: &MemoryEventInput) -> Result<i64> {
    let bytes = serde_json::to_vec(input)
        .map_err(|error| AppError::new("json_error", error.to_string()))?
        .len();
    i64::try_from(bytes)
        .map_err(|_| invalid_memory_capsule("memory capsule is too large to account"))
}

fn insert_memory_fragments(
    tx: &Transaction<'_>,
    event_id: &str,
    input: &MemoryEventInput,
) -> Result<()> {
    for (kind, values) in [
        ("observed", &input.observed),
        ("decision", &input.decision),
        ("constraint", &input.constraints),
        ("learned", &input.learned),
        ("unresolved", &input.unresolved),
        ("outcome", &input.outcome),
    ] {
        for (ordinal, value) in values.iter().enumerate() {
            tx.execute(
                "INSERT INTO memory_fragments(event_id, kind, ordinal, value)
                 VALUES (?1, ?2, ?3, ?4)",
                params![event_id, kind, ordinal as i64, value],
            )?;
        }
    }
    Ok(())
}

fn insert_memory_changes(
    tx: &Transaction<'_>,
    event_id: &str,
    changes: &[MemoryChangeInput],
) -> Result<()> {
    for (ordinal, change) in changes.iter().enumerate() {
        tx.execute(
            "INSERT INTO memory_changes(
                event_id, ordinal, subject, before_value, after_value, reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event_id,
                ordinal as i64,
                &change.subject,
                &change.before,
                &change.after,
                &change.reason,
            ],
        )?;
    }
    Ok(())
}

fn insert_memory_evidence(
    tx: &Transaction<'_>,
    event_id: &str,
    evidence: &[MemoryEvidenceInput],
) -> Result<()> {
    for (ordinal, evidence) in evidence.iter().enumerate() {
        tx.execute(
            "INSERT INTO memory_evidence(event_id, ordinal, reference, excerpt)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event_id,
                ordinal as i64,
                &evidence.reference,
                &evidence.excerpt,
            ],
        )?;
    }
    Ok(())
}

fn insert_memory_relations(
    tx: &Transaction<'_>,
    event_id: &str,
    relations: &[MemoryRelationInput],
) -> Result<()> {
    for (ordinal, relation) in relations.iter().enumerate() {
        let target_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
            [&relation.target],
            |row| row.get(0),
        )?;
        if !target_exists {
            return Err(invalid_memory_capsule(format!(
                "relation target '{}' does not exist",
                relation.target
            )));
        }
        tx.execute(
            "INSERT INTO memory_relations(
                event_id, ordinal, relation_type, target_event_id, basis
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event_id,
                ordinal as i64,
                &relation.relation_type,
                &relation.target,
                &relation.basis,
            ],
        )?;
    }
    Ok(())
}

fn index_memory_event(
    tx: &Transaction<'_>,
    event_id: &str,
    input: &MemoryEventInput,
) -> Result<()> {
    let mut content = Vec::new();
    for values in [
        &input.observed,
        &input.decision,
        &input.constraints,
        &input.learned,
        &input.unresolved,
        &input.outcome,
    ] {
        content.extend(values.iter().map(String::as_str));
    }
    for change in &input.changes {
        content.push(&change.subject);
        content.extend(change.before.as_deref());
        content.extend(change.after.as_deref());
        content.extend(change.reason.as_deref());
    }
    for evidence in &input.evidence {
        content.push(&evidence.reference);
        content.extend(evidence.excerpt.as_deref());
    }
    for relation in &input.relations {
        content.extend(relation.basis.as_deref());
    }
    tx.execute(
        "INSERT INTO memory_fts(event_id, event_type, context_terms, content_terms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            event_id,
            joined_index_terms(&input.event_type),
            joined_index_terms(&input.context),
            joined_index_terms(&content.join("\n")),
        ],
    )?;
    Ok(())
}

fn load_memory_event(conn: &Connection, event_id: &str) -> Result<Value> {
    let mut event = conn
        .query_row(
            "SELECT id, request_id, fingerprint, event_type, context,
                    occurred_at, recorded_at, valid_from, valid_until, pinned, logical_bytes
             FROM memory_events WHERE id = ?1",
            [event_id],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "request_id": row.get::<_, Option<String>>(1)?,
                    "fingerprint": row.get::<_, String>(2)?,
                    "type": row.get::<_, String>(3)?,
                    "context": row.get::<_, String>(4)?,
                    "occurred_at": row.get::<_, String>(5)?,
                    "recorded_at": row.get::<_, String>(6)?,
                    "valid_from": row.get::<_, Option<String>>(7)?,
                    "valid_to": row.get::<_, Option<String>>(8)?,
                    "pinned": row.get::<_, bool>(9)?,
                    "logical_bytes": row.get::<_, i64>(10)?,
                    "observed": [],
                    "decision": [],
                    "constraints": [],
                    "learned": [],
                    "unresolved": [],
                    "outcome": [],
                    "changes": [],
                    "evidence": [],
                    "relations": [],
                }))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::new("memory_event_not_found", event_id.to_owned()))?;
    let mut fragments = conn.prepare(
        "SELECT kind, value FROM memory_fragments
         WHERE event_id = ?1 ORDER BY kind, ordinal",
    )?;
    for row in fragments.query_map([event_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (kind, value) = row?;
        let field = if kind == "constraint" {
            "constraints"
        } else {
            kind.as_str()
        };
        event[field].as_array_mut().unwrap().push(Value::String(value));
    }
    event["changes"] = load_memory_changes(conn, event_id)?;
    event["evidence"] = load_memory_evidence(conn, event_id)?;
    event["relations"] = load_memory_relations(conn, event_id)?;
    Ok(event)
}

fn load_memory_changes(conn: &Connection, event_id: &str) -> Result<Value> {
    let mut statement = conn.prepare(
        "SELECT subject, before_value, after_value, reason
         FROM memory_changes WHERE event_id = ?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([event_id], |row| {
            Ok(json!({
                "subject": row.get::<_, String>(0)?,
                "before": row.get::<_, Option<String>>(1)?,
                "after": row.get::<_, Option<String>>(2)?,
                "reason": row.get::<_, Option<String>>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Value::Array(rows))
}

fn load_memory_evidence(conn: &Connection, event_id: &str) -> Result<Value> {
    let mut statement = conn.prepare(
        "SELECT reference, excerpt FROM memory_evidence
         WHERE event_id = ?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([event_id], |row| {
            Ok(json!({
                "reference": row.get::<_, String>(0)?,
                "excerpt": row.get::<_, Option<String>>(1)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Value::Array(rows))
}

fn load_memory_relations(conn: &Connection, event_id: &str) -> Result<Value> {
    let mut statement = conn.prepare(
        "SELECT relation_type, target_event_id, basis FROM memory_relations
         WHERE event_id = ?1 ORDER BY ordinal",
    )?;
    let rows = statement
        .query_map([event_id], |row| {
            Ok(json!({
                "type": row.get::<_, String>(0)?,
                "target": row.get::<_, String>(1)?,
                "basis": row.get::<_, Option<String>>(2)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Value::Array(rows))
}

fn memory_pressure(conn: &Connection, max_bytes: u64) -> Result<Value> {
    let logical_bytes: i64 = conn.query_row(
        "SELECT logical_bytes FROM memory_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    let logical_bytes = u64::try_from(logical_bytes)
        .map_err(|_| AppError::new("corrupt_store", "memory logical byte count is negative"))?;
    Ok(json!({
        "logical_bytes": logical_bytes,
        "max_bytes": max_bytes,
        "ratio": logical_bytes as f64 / max_bytes as f64,
    }))
}

fn empty_memory_retention() -> Value {
    json!({
        "age_evicted": 0,
        "capacity_evicted": 0,
        "logical_bytes_removed": 0,
    })
}

const MEMORY_EVENT_PROTECTED_SQL: &str = "
    e.pinned = 1
    OR EXISTS (
        SELECT 1 FROM memory_fragments unresolved
        WHERE unresolved.event_id = e.id AND unresolved.kind = 'unresolved'
    )
    OR EXISTS (
        SELECT 1 FROM memory_relations contradiction
        WHERE contradiction.relation_type = 'contradicts'
          AND (contradiction.event_id = e.id OR contradiction.target_event_id = e.id)
          AND NOT EXISTS (
              SELECT 1 FROM memory_relations resolution
              WHERE resolution.relation_type = 'resolves'
                AND resolution.target_event_id = contradiction.event_id
          )
    )";

#[derive(Debug, Clone, Serialize)]
pub struct MemoryRecallResult {
    pub scope: String,
    pub event: Value,
    pub state: String,
    pub rank: f64,
    pub explanation: MemoryRecallExplanation,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryRecallExplanation {
    pub lexical_rank: f64,
    pub feedback: i64,
    pub matched_via: String,
}

#[derive(Debug)]
struct MemoryCandidate {
    lexical_rank: f64,
    matched_via: &'static str,
}

impl Store {
    pub fn memory_recall(
        &self,
        query: &str,
        since: Option<&str>,
        until: Option<&str>,
        include_superseded: bool,
        limit: usize,
    ) -> Result<Vec<MemoryRecallResult>> {
        if !(1..=1000).contains(&limit) {
            return Err(AppError::new(
                "invalid_limit",
                "limit must be between 1 and 1000",
            ));
        }
        let tokens = tokenize_for_query(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let since = normalize_memory_query_timestamp(&self.conn, "since", since)?;
        let until = normalize_memory_query_timestamp(&self.conn, "until", until)?;
        if let (Some(since), Some(until)) = (&since, &until)
            && since > until
        {
            return Err(AppError::new(
                "invalid_input",
                "since must not be later than until",
            ));
        }
        let settings = config::resolve_memory(&self.scope, &self.database)?;
        let match_query = tokens
            .iter()
            .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let candidate_limit = limit.saturating_mul(8).clamp(limit, 1000);
        let direct = {
            let sql = format!(
                "SELECT memory_fts.event_id,
                        bm25(memory_fts, 0.0, 2.0, 4.0, 1.0) AS lexical_rank
                 FROM memory_fts
                 JOIN memory_events e ON e.id = memory_fts.event_id
                 WHERE memory_fts MATCH ?1
                   AND (
                       JULIANDAY(e.occurred_at) >= JULIANDAY('now', '-' || ?2 || ' days')
                       OR {MEMORY_EVENT_PROTECTED_SQL}
                   )
                   AND (?3 IS NULL OR JULIANDAY(e.occurred_at) >= JULIANDAY(?3))
                   AND (?4 IS NULL OR JULIANDAY(e.occurred_at) <= JULIANDAY(?4))
                 ORDER BY lexical_rank, e.occurred_at DESC, e.id
                 LIMIT ?5"
            );
            let mut statement = self.conn.prepare(&sql)?;
            statement
                .query_map(
                    params![
                        match_query,
                        i64::from(settings.max_age_days),
                        since.as_deref(),
                        until.as_deref(),
                        candidate_limit as i64,
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let mut candidates = BTreeMap::<String, MemoryCandidate>::new();
        for (event_id, lexical_rank) in direct {
            let current_id = current_visible_memory_id(
                &self.conn,
                &event_id,
                settings.max_age_days,
                since.as_deref(),
                until.as_deref(),
            )?;
            if include_superseded || current_id == event_id {
                keep_memory_candidate(
                    &mut candidates,
                    event_id.clone(),
                    lexical_rank,
                    "direct",
                );
            }
            if current_id != event_id {
                keep_memory_candidate(
                    &mut candidates,
                    current_id,
                    lexical_rank,
                    "superseded_event",
                );
            }
        }

        let mut results = Vec::with_capacity(candidates.len());
        for (event_id, candidate) in candidates {
            let current_id = current_visible_memory_id(
                &self.conn,
                &event_id,
                settings.max_age_days,
                since.as_deref(),
                until.as_deref(),
            )?;
            let feedback = memory_feedback_score(&self.conn, &event_id)?;
            let adjustment = candidate.lexical_rank.abs().max(1e-6)
                * 0.05
                * feedback.clamp(-3, 3) as f64;
            results.push(MemoryRecallResult {
                scope: self.scope.clone(),
                event: load_memory_event(&self.conn, &event_id)?,
                state: if current_id == event_id {
                    "current".to_owned()
                } else {
                    "superseded".to_owned()
                },
                rank: candidate.lexical_rank - adjustment,
                explanation: MemoryRecallExplanation {
                    lexical_rank: candidate.lexical_rank,
                    feedback,
                    matched_via: candidate.matched_via.to_owned(),
                },
            });
        }
        sort_memory_results(&mut results);
        results.truncate(limit);
        Ok(results)
    }

    pub fn memory_show(&self, event_id: &str) -> Result<Value> {
        let event_id = event_id.trim();
        if event_id.is_empty() {
            return Err(AppError::new("invalid_input", "event_id must not be empty"));
        }
        Ok(json!({
            "scope": self.scope,
            "database": self.database.to_string_lossy(),
            "event": load_memory_event(&self.conn, event_id)?,
        }))
    }

    pub fn memory_feedback(
        &mut self,
        event_id: &str,
        signal: &str,
        reason: &str,
    ) -> Result<Value> {
        let event_id = event_id.trim();
        let reason = reason.trim();
        if event_id.is_empty() || reason.is_empty() {
            return Err(AppError::new(
                "invalid_input",
                "event_id and reason must not be empty",
            ));
        }
        if !matches!(signal, "useful" | "not-useful") {
            return Err(AppError::new(
                "invalid_input",
                "signal must be useful or not-useful",
            ));
        }
        let settings = config::resolve_memory(&self.scope, &self.database)?;
        if settings.setting == config::MemorySetting::Disabled {
            return Err(AppError::new(
                "memory_disabled",
                "temporal memory is disabled for this scope",
            ));
        }
        let scope = self.scope.clone();
        let database = self.database.to_string_lossy().into_owned();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
            [event_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppError::new(
                "memory_event_not_found",
                event_id.to_owned(),
            ));
        }
        let created_at: String = tx.query_row(&format!("SELECT {TIMESTAMP_SQL}"), [], |row| {
            row.get(0)
        })?;
        tx.execute(
            "INSERT INTO memory_feedback(event_id, signal, reason, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![event_id, signal, reason, &created_at],
        )?;
        let counter = if signal == "useful" {
            "feedback_useful"
        } else {
            "feedback_not_useful"
        };
        tx.execute(
            &format!("UPDATE memory_state SET {counter} = {counter} + 1 WHERE id = 1"),
            [],
        )?;
        record_operation(
            &tx,
            "memory_feedback",
            event_id,
            &json!({"signal": signal}),
        )?;
        tx.commit()?;
        Ok(json!({
            "scope": scope,
            "database": database,
            "event_id": event_id,
            "signal": signal,
            "reason": reason,
            "created_at": created_at,
        }))
    }
}

fn normalize_memory_query_timestamp(
    conn: &Connection,
    name: &str,
    value: Option<&str>,
) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} must not be empty"),
        ));
    }
    conn.query_row(
        "SELECT STRFTIME('%Y-%m-%dT%H:%M:%fZ', ?1)",
        [value],
        |row| row.get::<_, Option<String>>(0),
    )?
    .map(Some)
    .ok_or_else(|| AppError::new("invalid_input", format!("{name} is not a valid timestamp")))
}

fn current_visible_memory_id(
    conn: &Connection,
    event_id: &str,
    max_age_days: u32,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<String> {
    let sql = format!(
        "SELECT relation.event_id
         FROM memory_relations relation
         JOIN memory_events e ON e.id = relation.event_id
         WHERE relation.relation_type = 'supersedes'
           AND relation.target_event_id = ?1
           AND (
               JULIANDAY(e.occurred_at) >= JULIANDAY('now', '-' || ?2 || ' days')
               OR {MEMORY_EVENT_PROTECTED_SQL}
           )
           AND (?3 IS NULL OR JULIANDAY(e.occurred_at) >= JULIANDAY(?3))
           AND (?4 IS NULL OR JULIANDAY(e.occurred_at) <= JULIANDAY(?4))
         ORDER BY e.occurred_at DESC, e.recorded_at DESC, e.id
         LIMIT 1"
    );
    let mut current = event_id.to_owned();
    loop {
        let next = conn
            .query_row(
                &sql,
                params![&current, i64::from(max_age_days), since, until],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(next) = next else {
            return Ok(current);
        };
        current = next;
    }
}

fn keep_memory_candidate(
    candidates: &mut BTreeMap<String, MemoryCandidate>,
    event_id: String,
    lexical_rank: f64,
    matched_via: &'static str,
) {
    match candidates.entry(event_id) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(MemoryCandidate {
                lexical_rank,
                matched_via,
            });
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let current = entry.get_mut();
            if lexical_rank < current.lexical_rank
                || (lexical_rank == current.lexical_rank && matched_via == "direct")
            {
                current.lexical_rank = lexical_rank;
                current.matched_via = matched_via;
            }
        }
    }
}

fn memory_feedback_score(conn: &Connection, event_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(CASE signal WHEN 'useful' THEN 1 ELSE -1 END), 0)
         FROM memory_feedback WHERE event_id = ?1",
        [event_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn sort_memory_results(results: &mut [MemoryRecallResult]) {
    results.sort_by(|left, right| {
        let state_priority = |state: &str| if state == "current" { 0 } else { 1 };
        left.rank
            .total_cmp(&right.rank)
            .then_with(|| {
                right.explanation.feedback.cmp(&left.explanation.feedback)
            })
            .then_with(|| {
                right.event["occurred_at"]
                    .as_str()
                    .cmp(&left.event["occurred_at"].as_str())
            })
            .then_with(|| {
                state_priority(&left.state).cmp(&state_priority(&right.state))
            })
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| {
                left.event["id"]
                    .as_str()
                    .cmp(&right.event["id"].as_str())
            })
    });
}

#[derive(Default)]
struct MemoryEvictions {
    count: i64,
    logical_bytes: i64,
    event_ids: Vec<String>,
    affected_clusters: BTreeSet<(String, String)>,
}

impl MemoryEvictions {
    fn add(
        &mut self,
        event_id: String,
        logical_bytes: i64,
        event_type: String,
        context: String,
    ) {
        self.count += 1;
        self.logical_bytes += logical_bytes;
        self.affected_clusters.insert((event_type, context));
        if self.event_ids.len() < 100 {
            self.event_ids.push(event_id);
        }
    }
}

fn enforce_memory_retention(
    tx: &Transaction<'_>,
    max_age_days: u32,
    max_bytes: u64,
    retained_event_id: Option<&str>,
    fail_if_over_capacity: bool,
) -> Result<Value> {
    let age_sql = format!(
        "SELECT e.id, e.logical_bytes, e.event_type, e.context
         FROM memory_events e
         WHERE JULIANDAY(e.occurred_at) < JULIANDAY('now', '-' || ?1 || ' days')
           AND NOT ({MEMORY_EVENT_PROTECTED_SQL})
         ORDER BY e.occurred_at, e.recorded_at, e.id
         LIMIT 100"
    );
    let mut age = MemoryEvictions::default();
    loop {
        let batch = {
            let mut statement = tx.prepare(&age_sql)?;
            statement
                .query_map([i64::from(max_age_days)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if batch.is_empty() {
            break;
        }
        for (event_id, logical_bytes, event_type, context) in batch {
            delete_memory_event(tx, &event_id)?;
            age.add(event_id, logical_bytes, event_type, context);
        }
    }
    apply_memory_evictions(tx, "age", &age)?;

    let capacity_sql = format!(
        "SELECT e.id, e.logical_bytes, e.event_type, e.context
         FROM memory_events e
         WHERE (?1 IS NULL OR e.id <> ?1)
           AND NOT ({MEMORY_EVENT_PROTECTED_SQL})
         ORDER BY e.occurred_at, e.recorded_at, e.id
         LIMIT 1"
    );
    let mut capacity = MemoryEvictions::default();
    loop {
        let logical_bytes: i64 = tx.query_row(
            "SELECT logical_bytes FROM memory_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let logical_bytes = u64::try_from(logical_bytes)
            .map_err(|_| AppError::new("corrupt_store", "memory logical bytes are negative"))?;
        if logical_bytes <= max_bytes {
            break;
        }
        let candidate = tx
            .query_row(&capacity_sql, [retained_event_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .optional()?;
        let Some((event_id, event_bytes, event_type, context)) = candidate else {
            if fail_if_over_capacity {
                return Err(AppError::new(
                    "memory_capacity_exceeded",
                    format!(
                        "temporal memory cannot fit within the configured {max_bytes}-byte limit without deleting protected events"
                    ),
                ));
            }
            break;
        };
        delete_memory_event(tx, &event_id)?;
        capacity.add(event_id, event_bytes, event_type, context);
        tx.execute(
            "UPDATE memory_state
             SET capacity_evictions = capacity_evictions + 1,
                 event_count = event_count - 1,
                 logical_bytes = logical_bytes - ?1
             WHERE id = 1",
            [event_bytes],
        )?;
    }
    record_memory_eviction_operation(tx, "capacity", &capacity)?;
    prune_affected_memory_cluster_hints(tx, &age, &capacity)?;

    Ok(json!({
        "age_evicted": age.count,
        "capacity_evicted": capacity.count,
        "logical_bytes_removed": age.logical_bytes + capacity.logical_bytes,
    }))
}

fn prune_affected_memory_cluster_hints(
    tx: &Transaction<'_>,
    age: &MemoryEvictions,
    capacity: &MemoryEvictions,
) -> Result<()> {
    for (event_type, context) in age
        .affected_clusters
        .union(&capacity.affected_clusters)
    {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM memory_events WHERE event_type = ?1 AND context = ?2",
            params![event_type, context],
            |row| row.get(0),
        )?;
        if count < 5 {
            tx.execute(
                "DELETE FROM memory_hint_state
                 WHERE hint_type = 'exact-context-cluster' AND candidate_key = ?1",
                [memory_cluster_candidate_key(event_type, context)],
            )?;
        }
    }
    Ok(())
}

fn delete_memory_event(tx: &Transaction<'_>, event_id: &str) -> Result<()> {
    tx.execute("DELETE FROM memory_fts WHERE event_id = ?1", [event_id])?;
    if tx.execute("DELETE FROM memory_events WHERE id = ?1", [event_id])? != 1 {
        return Err(AppError::new(
            "corrupt_store",
            format!("memory event '{event_id}' disappeared during retention"),
        ));
    }
    Ok(())
}

fn apply_memory_evictions(
    tx: &Transaction<'_>,
    reason: &str,
    evictions: &MemoryEvictions,
) -> Result<()> {
    if evictions.count == 0 {
        return Ok(());
    }
    let counter = if reason == "age" {
        "age_evictions"
    } else {
        "capacity_evictions"
    };
    tx.execute(
        &format!(
            "UPDATE memory_state
             SET {counter} = {counter} + ?1,
                 event_count = event_count - ?1,
                 logical_bytes = logical_bytes - ?2
             WHERE id = 1"
        ),
        params![evictions.count, evictions.logical_bytes],
    )?;
    record_memory_eviction_operation(tx, reason, evictions)
}

fn record_memory_eviction_operation(
    tx: &Transaction<'_>,
    reason: &str,
    evictions: &MemoryEvictions,
) -> Result<()> {
    if evictions.count == 0 {
        return Ok(());
    }
    record_operation(
        tx,
        "memory_retention",
        reason,
        &json!({
            "reason": reason,
            "event_ids": evictions.event_ids,
            "count": evictions.count,
            "logical_bytes_removed": evictions.logical_bytes,
            "identifiers_truncated": evictions.count as usize > evictions.event_ids.len(),
        }),
    )
    .map(|_| ())
}

struct MemoryHintCandidate {
    candidate_key: String,
    hint_type: &'static str,
    reason: &'static str,
    event_ids: Vec<String>,
}

fn emit_memory_hints(
    tx: &Transaction<'_>,
    event_id: &str,
    input: &MemoryEventInput,
    max_bytes: u64,
) -> Result<Vec<Value>> {
    prune_memory_hint_state(tx, max_bytes, false)?;
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();

    let relations = {
        let mut statement = tx.prepare(
            "SELECT relation_type, target_event_id FROM memory_relations
             WHERE event_id = ?1 AND relation_type IN ('contradicts', 'supersedes')
             ORDER BY ordinal",
        )?;
        statement
            .query_map([event_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (relation_type, target) in relations {
        push_memory_hint_candidate(
            &mut candidates,
            &mut seen,
            MemoryHintCandidate {
                candidate_key: format!("relation:{relation_type}:{target}"),
                hint_type: "relation-review",
                reason: "显式冲突或替代链需要复核",
                event_ids: vec![event_id.to_owned(), target],
            },
        );
    }

    let cluster_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM memory_events
         WHERE event_type = ?1 AND context = ?2",
        params![&input.event_type, &input.context],
        |row| row.get(0),
    )?;
    if cluster_count >= 5 {
        let mut statement = tx.prepare(
            "SELECT id FROM memory_events
             WHERE event_type = ?1 AND context = ?2
             ORDER BY occurred_at, recorded_at, id LIMIT 5",
        )?;
        let event_ids = statement
            .query_map(params![&input.event_type, &input.context], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        push_memory_hint_candidate(
            &mut candidates,
            &mut seen,
            MemoryHintCandidate {
                candidate_key: memory_cluster_candidate_key(&input.event_type, &input.context),
                hint_type: "exact-context-cluster",
                reason: "至少五条事件具有完全相同的类型和上下文",
                event_ids,
            },
        );
    }

    let aged_unresolved = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT e.id
             FROM memory_fragments fragment
             JOIN memory_events e ON e.id = fragment.event_id
             WHERE fragment.kind = 'unresolved'
               AND JULIANDAY(e.occurred_at) <= JULIANDAY('now', '-14 days')
             ORDER BY e.occurred_at, e.id LIMIT 100",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for unresolved_id in aged_unresolved {
        push_memory_hint_candidate(
            &mut candidates,
            &mut seen,
            MemoryHintCandidate {
                candidate_key: format!("unresolved:{unresolved_id}"),
                hint_type: "aged-unresolved",
                reason: "未决事件已经超过十四天",
                event_ids: vec![unresolved_id],
            },
        );
    }

    let logical_bytes = memory_logical_bytes_state(tx)?;
    if logical_bytes as f64 / max_bytes as f64 >= 0.8 {
        push_memory_hint_candidate(
            &mut candidates,
            &mut seen,
            MemoryHintCandidate {
                candidate_key: "pressure:store".to_owned(),
                hint_type: "storage-pressure",
                reason: "时序记忆存储压力已经达到百分之八十",
                event_ids: Vec::new(),
            },
        );
    }

    let mut hints = Vec::new();
    for candidate in candidates {
        let cooling_down: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_hint_state WHERE candidate_key = ?1)",
            [&candidate.candidate_key],
            |row| row.get(0),
        )?;
        if cooling_down {
            continue;
        }
        tx.execute(
            &format!(
                "INSERT INTO memory_hint_state(
                    candidate_key, hint_type, last_emitted_at, next_eligible_at
                 ) VALUES (?1, ?2, {TIMESTAMP_SQL}, STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now', '+7 days'))"
            ),
            params![&candidate.candidate_key, candidate.hint_type],
        )?;
        hints.push(json!({
            "candidate_key": candidate.candidate_key,
            "type": candidate.hint_type,
            "reason": candidate.reason,
            "event_ids": candidate.event_ids,
        }));
        if hints.len() == 3 {
            break;
        }
    }
    Ok(hints)
}

fn push_memory_hint_candidate(
    candidates: &mut Vec<MemoryHintCandidate>,
    seen: &mut BTreeSet<String>,
    candidate: MemoryHintCandidate,
) {
    if seen.insert(candidate.candidate_key.clone()) {
        candidates.push(candidate);
    }
}

fn memory_cluster_candidate_key(event_type: &str, context: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(event_type.as_bytes());
    digest.update([0]);
    digest.update(context.as_bytes());
    let hash: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("cluster:{hash}")
}

fn prune_memory_hint_state(
    tx: &Transaction<'_>,
    max_bytes: u64,
    prune_all_clusters: bool,
) -> Result<usize> {
    let mut pruned = tx.execute(
        "DELETE FROM memory_hint_state
         WHERE JULIANDAY(next_eligible_at) <= JULIANDAY('now')",
        [],
    )?;
    if prune_all_clusters {
        let active_clusters = {
            let mut statement = tx.prepare(
                "SELECT event_type, context FROM memory_events
                 GROUP BY event_type, context HAVING COUNT(*) >= 5",
            )?;
            statement
                .query_map([], |row| {
                    Ok(memory_cluster_candidate_key(
                        &row.get::<_, String>(0)?,
                        &row.get::<_, String>(1)?,
                    ))
                })?
                .collect::<rusqlite::Result<BTreeSet<_>>>()?
        };
        let stored_clusters = {
            let mut statement = tx.prepare(
                "SELECT candidate_key FROM memory_hint_state
                 WHERE hint_type = 'exact-context-cluster'",
            )?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for candidate_key in stored_clusters {
            if !active_clusters.contains(&candidate_key) {
                pruned += tx.execute(
                    "DELETE FROM memory_hint_state WHERE candidate_key = ?1",
                    [candidate_key],
                )?;
            }
        }
    }
    pruned += tx.execute(
        "DELETE FROM memory_hint_state
         WHERE hint_type = 'relation-review'
           AND NOT EXISTS (
               SELECT 1 FROM memory_relations relation
               WHERE candidate_key = 'relation:' || relation.relation_type || ':' || relation.target_event_id
                 AND relation.relation_type IN ('contradicts', 'supersedes')
                 AND (
                     relation.relation_type = 'supersedes'
                     OR NOT EXISTS (
                         SELECT 1 FROM memory_relations resolution
                         WHERE resolution.relation_type = 'resolves'
                           AND resolution.target_event_id = relation.event_id
                     )
                 )
           )",
        [],
    )?;
    pruned += tx.execute(
        "DELETE FROM memory_hint_state
         WHERE hint_type = 'aged-unresolved'
           AND NOT EXISTS (
               SELECT 1 FROM memory_events e
               JOIN memory_fragments fragment ON fragment.event_id = e.id
               WHERE e.id = SUBSTR(candidate_key, INSTR(candidate_key, ':') + 1)
                 AND fragment.kind = 'unresolved'
                 AND JULIANDAY(e.occurred_at) <= JULIANDAY('now', '-14 days')
           )",
        [],
    )?;
    if memory_logical_bytes_state(tx)? as f64 / (max_bytes as f64) < 0.8 {
        pruned += tx.execute(
            "DELETE FROM memory_hint_state WHERE hint_type = 'storage-pressure'",
            [],
        )?;
    }
    Ok(pruned)
}

fn memory_logical_bytes_state(conn: &Connection) -> Result<u64> {
    let logical_bytes: i64 = conn.query_row(
        "SELECT logical_bytes FROM memory_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    u64::try_from(logical_bytes)
        .map_err(|_| AppError::new("corrupt_store", "memory logical bytes are negative"))
}

fn memory_pending_hint_count(conn: &Connection, max_bytes: u64) -> Result<i64> {
    let clusters: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT 1 FROM memory_events
             GROUP BY event_type, context HAVING COUNT(*) >= 5
         )",
        [],
        |row| row.get(0),
    )?;
    let relations: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT relation.relation_type, relation.target_event_id
             FROM memory_relations relation
             WHERE relation.relation_type IN ('contradicts', 'supersedes')
               AND (
                   relation.relation_type = 'supersedes'
                   OR NOT EXISTS (
                       SELECT 1 FROM memory_relations resolution
                       WHERE resolution.relation_type = 'resolves'
                         AND resolution.target_event_id = relation.event_id
                   )
               )
             GROUP BY relation.relation_type, relation.target_event_id
         )",
        [],
        |row| row.get(0),
    )?;
    let unresolved: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT e.id)
         FROM memory_events e
         JOIN memory_fragments fragment ON fragment.event_id = e.id
         WHERE fragment.kind = 'unresolved'
           AND JULIANDAY(e.occurred_at) <= JULIANDAY('now', '-14 days')",
        [],
        |row| row.get(0),
    )?;
    let pressure = i64::from(memory_logical_bytes_state(conn)? as f64 / max_bytes as f64 >= 0.8);
    Ok(clusters + relations + unresolved + pressure)
}

impl Store {
    pub fn memory_status(&self) -> Result<Value> {
        let settings = config::resolve_memory(&self.scope, &self.database)?;
        let (
            record_attempts,
            inserted_events,
            idempotent_replays,
            feedback_useful,
            feedback_not_useful,
            age_evictions,
            capacity_evictions,
            event_count,
            logical_bytes,
        ): (i64, i64, i64, i64, i64, i64, i64, i64, i64) = self.conn.query_row(
            "SELECT record_attempts, inserted_events, idempotent_replays,
                    feedback_useful, feedback_not_useful, age_evictions,
                    capacity_evictions, event_count, logical_bytes
             FROM memory_state WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )?;
        let protected: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM memory_events e WHERE {MEMORY_EVENT_PROTECTED_SQL}"),
            [],
            |row| row.get(0),
        )?;
        let superseded: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_events e
             WHERE EXISTS (
                 SELECT 1 FROM memory_relations relation
                 WHERE relation.target_event_id = e.id
                   AND relation.relation_type = 'supersedes'
             )",
            [],
            |row| row.get(0),
        )?;
        let pending_hints = memory_pending_hint_count(&self.conn, settings.max_bytes)?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database.to_string_lossy(),
            "policy": {
                "setting": settings.setting,
                "origin": settings.origin,
                "max_age_days": settings.max_age_days,
                "max_bytes": settings.max_bytes,
            },
            "retained": {
                "events": event_count,
                "protected": protected,
                "superseded": superseded,
                "logical_bytes": logical_bytes,
            },
            "pressure": memory_pressure(&self.conn, settings.max_bytes)?,
            "counters": {
                "record_attempts": record_attempts,
                "inserted_events": inserted_events,
                "idempotent_replays": idempotent_replays,
                "feedback_useful": feedback_useful,
                "feedback_not_useful": feedback_not_useful,
                "age_evictions": age_evictions,
                "capacity_evictions": capacity_evictions,
            },
            "pending_hints": pending_hints,
        }))
    }

    pub fn memory_maintain(&mut self) -> Result<Value> {
        let settings = config::resolve_memory(&self.scope, &self.database)?;
        if settings.setting == config::MemorySetting::Disabled {
            return Err(AppError::new(
                "memory_disabled",
                "temporal memory is disabled for this scope",
            ));
        }
        let scope = self.scope.clone();
        let database = self.database.to_string_lossy().into_owned();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let retention = enforce_memory_retention(
            &tx,
            settings.max_age_days,
            settings.max_bytes,
            None,
            false,
        )?;
        let hints_pruned = prune_memory_hint_state(&tx, settings.max_bytes, true)?;
        let pressure = memory_pressure(&tx, settings.max_bytes)?;
        record_operation(
            &tx,
            "memory_maintain",
            "memory",
            &json!({
                "retention": retention,
                "hint_rows_pruned": hints_pruned,
            }),
        )?;
        tx.commit()?;
        Ok(json!({
            "scope": scope,
            "database": database,
            "retention": retention,
            "hints_pruned": hints_pruned,
            "pressure": pressure,
        }))
    }
}
