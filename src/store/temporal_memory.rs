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
    if current == USER_VERSION {
        tx.commit()?;
        return Ok(());
    }
    if current != TAGS_VERSION {
        return Err(AppError::new(
            "unsupported_store_version",
            format!("cannot migrate wiki database version {current} to {USER_VERSION}"),
        ));
    }
    create_temporal_memory_schema(&tx).map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to prepare v{USER_VERSION} temporal memory schema: {error}"),
        )
    })?;
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('format_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![USER_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", USER_VERSION)?;
    tx.commit().map_err(|error| {
        AppError::new(
            "store_migration_failed",
            format!("failed to commit v{USER_VERSION} temporal memory migration: {error}"),
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
        let pressure = memory_pressure(&tx, settings.max_bytes)?;
        tx.commit()?;
        Ok(json!({
            "scope": scope,
            "database": database,
            "created": true,
            "event": event,
            "retention": empty_memory_retention(),
            "pressure": pressure,
            "hints": [],
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
