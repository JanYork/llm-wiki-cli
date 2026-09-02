#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TodoCreateInput {
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub cue: Option<String>,
    pub detail: Option<String>,
    pub parent_id: Option<String>,
    pub target_at: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct TodoUpdateInput {
    pub title: Option<String>,
    pub cue: Option<Option<String>>,
    pub detail: Option<Option<String>>,
    pub target_at: Option<Option<String>>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
}

fn normalize_todo_id(name: &str, value: &str) -> Result<String> {
    let value = normalize_todo_text(name, value)?;
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} must be a Todo ID"),
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn normalize_agent_context(value: &str) -> Result<String> {
    let Some(digest) = value.strip_prefix("lwcctx-v1-") else {
        return Err(AppError::new(
            "invalid_agent_context",
            "context must be an lwcctx-v1 token",
        ));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::new(
            "invalid_agent_context",
            "context must be an lwcctx-v1 token",
        ));
    }
    Ok(value.to_owned())
}

fn normalize_todo_target_at(conn: &Connection, value: &str) -> Result<String> {
    let value = normalize_todo_text("target_at", value)?;
    let bytes = value.as_bytes();
    let base_ok = bytes.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':');
    let timezone_ok = value.ends_with('Z')
        || (bytes.len() >= 25
            && matches!(bytes.get(bytes.len() - 6), Some(b'+') | Some(b'-'))
            && bytes.get(bytes.len() - 3) == Some(&b':')
            && bytes[bytes.len() - 5..bytes.len() - 3]
                .iter()
                .all(u8::is_ascii_digit)
            && bytes[bytes.len() - 2..].iter().all(u8::is_ascii_digit));
    if !base_ok || !timezone_ok {
        return Err(AppError::new(
            "invalid_todo_target_at",
            "target_at must be an RFC3339 timestamp with an explicit timezone",
        ));
    }
    conn.query_row(
        "SELECT STRFTIME('%Y-%m-%dT%H:%M:%fZ', ?1)",
        [&value],
        |row| row.get::<_, Option<String>>(0),
    )?
    .ok_or_else(|| {
        AppError::new(
            "invalid_todo_target_at",
            "target_at must be a valid RFC3339 timestamp",
        )
    })
}

fn bounded_todo_title(value: &str) -> String {
    value.chars().take(500).collect()
}

fn normalize_todo_text(name: &str, value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} must not be empty"),
        ));
    }
    if value.len() > 100_000 {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} is too large"),
        ));
    }
    Ok(value.to_owned())
}
fn normalize_labels(values: Vec<String>) -> Result<Vec<String>> {
    let mut out = BTreeSet::new();
    for value in values {
        let value = normalize_todo_text("tag", &value)?;
        if value.len() > 200 {
            return Err(AppError::new("invalid_input", "tag is too large"));
        }
        out.insert(value);
    }
    if out.len() > 100 {
        return Err(AppError::new(
            "invalid_input",
            "at most 100 tags are allowed",
        ));
    }
    Ok(out.into_iter().collect())
}
fn todo_fingerprint(input: &TodoCreateInput) -> Result<String> {
    let mut canonical = input.clone();
    canonical.request_id = None;
    canonical.title = normalize_todo_text("title", &canonical.title)?;
    canonical.tags = normalize_labels(canonical.tags)?;
    let bytes =
        serde_json::to_vec(&canonical).map_err(|e| AppError::new("json_error", e.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn load_todo(conn: &Connection, id: &str, scope: &str) -> Result<Value> {
    let mut value=conn.query_row("SELECT id,request_id,title,cue,detail,state,result,cancel_reason,revision,created_at,updated_at,closed_at,parent_id,target_at FROM todo_items WHERE id=?1",[id],|r|Ok(json!({
        "id":r.get::<_,String>(0)?,"request_id":r.get::<_,Option<String>>(1)?,"scope":scope,"title":r.get::<_,String>(2)?,"cue":r.get::<_,Option<String>>(3)?,"detail":r.get::<_,Option<String>>(4)?,"state":r.get::<_,String>(5)?,"result":r.get::<_,Option<String>>(6)?,"cancel_reason":r.get::<_,Option<String>>(7)?,"revision":r.get::<_,i64>(8)?,"created_at":r.get::<_,String>(9)?,"updated_at":r.get::<_,String>(10)?,"closed_at":r.get::<_,Option<String>>(11)?,"parent_id":r.get::<_,Option<String>>(12)?,"target_at":r.get::<_,Option<String>>(13)?,"tags":[],"children":[]
    }))).optional()?.ok_or_else(||AppError::new("todo_not_found",format!("Todo '{id}' was not found")))?;
    let mut st =
        conn.prepare("SELECT tag_name FROM todo_tags WHERE todo_id=?1 ORDER BY tag_name")?;
    let tags = st
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    value["tags"] = json!(tags);
    let mut st = conn.prepare("SELECT id,title,state,target_at,created_at,updated_at FROM todo_items WHERE parent_id=?1 ORDER BY created_at,id")?;
    let children = st
        .query_map([id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "state": r.get::<_, String>(2)?,
                "target_at": r.get::<_, Option<String>>(3)?,
                "created_at": r.get::<_, String>(4)?,
                "updated_at": r.get::<_, String>(5)?,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    value["children"] = json!(children);
    Ok(value)
}
fn index_todo(
    tx: &Transaction<'_>,
    id: &str,
    title: &str,
    tags: &[String],
    cue: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    tx.execute("DELETE FROM todo_fts WHERE todo_id=?1", [id])?;
    tx.execute("INSERT INTO todo_fts(todo_id,title_terms,tag_terms,cue_terms,detail_terms) VALUES(?1,?2,?3,?4,?5)",params![id,joined_index_terms(title),joined_index_terms(&tags.join(" ")),cue.map(joined_index_terms),detail.map(joined_index_terms)])?;
    Ok(())
}
fn todo_revision_error(expected: i64, current: i64) -> AppError {
    AppError::new(
        "todo_revision_conflict",
        format!("expected revision {expected}, current revision is {current}"),
    )
    .with_details(json!({"current_revision":current}))
}

impl Store {
    pub fn open_todo_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM todo_items WHERE state='open'",
            [],
            |row| row.get(0),
        )?)
    }
    pub fn due_todo_reminders(&self, limit: usize) -> Result<(Vec<Value>, usize)> {
        let total = self.conn.query_row(
            "SELECT COUNT(*) FROM todo_items WHERE state='open' AND target_at IS NOT NULL AND target_at<=STRFTIME('%Y-%m-%dT%H:%M:%fZ','now')",
            [],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = self.conn.prepare(
            "SELECT id,title,parent_id,target_at FROM todo_items WHERE state='open' AND target_at IS NOT NULL AND target_at<=STRFTIME('%Y-%m-%dT%H:%M:%fZ','now') ORDER BY created_at,rowid LIMIT ?1",
        )?;
        let reminders = statement
            .query_map([limit as i64], |row| {
                let title = row.get::<_, String>(1)?;
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "title": bounded_todo_title(&title),
                    "parent_id": row.get::<_, Option<String>>(2)?,
                    "target_at": row.get::<_, String>(3)?,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let omitted = total.saturating_sub(reminders.len());
        Ok((reminders, omitted))
    }
    pub fn todo_track(&mut self, id: &str, context: &str) -> Result<Value> {
        let id = normalize_todo_id("todo_id", id)?;
        let context = normalize_agent_context(context)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = tx
            .query_row("SELECT state FROM todo_items WHERE id=?1", [&id], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .ok_or_else(|| AppError::new("todo_not_found", format!("Todo '{id}' was not found")))?;
        if state != "open" {
            return Err(AppError::new(
                "invalid_todo_transition",
                "only open Todos can be tracked",
            ));
        }
        let changed = tx.execute(
            "INSERT OR IGNORE INTO agent_todo_tracks(context_id,todo_id) VALUES(?1,?2)",
            params![context, id],
        )?;
        tx.commit()?;
        Ok(json!({"action":if changed == 0 {"unchanged"} else {"tracked"},"context":context,"todo_id":id}))
    }
    pub fn todo_untrack(&mut self, id: &str, context: &str) -> Result<Value> {
        let id = normalize_todo_id("todo_id", id)?;
        let context = normalize_agent_context(context)?;
        let changed = self.conn.execute(
            "DELETE FROM agent_todo_tracks WHERE context_id=?1 AND todo_id=?2",
            params![context, id],
        )?;
        Ok(json!({"action":if changed == 0 {"unchanged"} else {"untracked"},"context":context,"todo_id":id}))
    }
    pub fn tracked_todos(&self, context: &str) -> Result<Value> {
        let context = normalize_agent_context(context)?;
        let mut statement = self.conn.prepare(
            "SELECT t.id FROM agent_todo_tracks a JOIN todo_items t ON t.id=a.todo_id WHERE a.context_id=?1 ORDER BY t.updated_at DESC,t.id",
        )?;
        let ids = statement
            .query_map([&context], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let todos = ids
            .iter()
            .map(|id| load_todo(&self.conn, id, &self.scope))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"scope":self.scope,"database":self.database,"context":context,"returned":todos.len(),"todos":todos}))
    }
    pub fn tracked_open_todo_readiness(
        &self,
        context: &str,
        limit: usize,
    ) -> Result<(i64, Vec<Value>, usize)> {
        let context = normalize_agent_context(context)?;
        let open = self.conn.query_row(
            "SELECT COUNT(*) FROM agent_todo_tracks a JOIN todo_items t ON t.id=a.todo_id WHERE a.context_id=?1 AND t.state='open'",
            [&context],
            |row| row.get::<_, i64>(0),
        )?;
        let total = self.conn.query_row(
            "SELECT COUNT(*) FROM agent_todo_tracks a JOIN todo_items t ON t.id=a.todo_id WHERE a.context_id=?1 AND t.state='open' AND t.target_at IS NOT NULL AND t.target_at<=STRFTIME('%Y-%m-%dT%H:%M:%fZ','now')",
            [&context],
            |row| row.get::<_, i64>(0),
        )? as usize;
        let mut statement = self.conn.prepare(
            "SELECT t.id,t.title,t.parent_id,t.target_at FROM agent_todo_tracks a JOIN todo_items t ON t.id=a.todo_id WHERE a.context_id=?1 AND t.state='open' AND t.target_at IS NOT NULL AND t.target_at<=STRFTIME('%Y-%m-%dT%H:%M:%fZ','now') ORDER BY t.created_at,t.id LIMIT ?2",
        )?;
        let reminders = statement
            .query_map(params![context, limit as i64], |row| {
                let title = row.get::<_, String>(1)?;
                Ok(json!({
                    "id":row.get::<_,String>(0)?,
                    "title":bounded_todo_title(&title),
                    "parent_id":row.get::<_,Option<String>>(2)?,
                    "target_at":row.get::<_,String>(3)?,
                }))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let omitted = total.saturating_sub(reminders.len());
        Ok((open, reminders, omitted))
    }
    pub fn agent_tracking_bound(&self, context: &str) -> Result<bool> {
        let context = normalize_agent_context(context)?;
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_plan_tracks WHERE context_id=?1) OR EXISTS(SELECT 1 FROM agent_todo_tracks WHERE context_id=?1)",
            [&context],
            |row| row.get(0),
        ).map_err(Into::into)
    }
    pub fn todo_add(&mut self, mut input: TodoCreateInput) -> Result<Value> {
        input.title = normalize_todo_text("title", &input.title)?;
        input.tags = normalize_labels(input.tags)?;
        if let Some(v) = input.cue.as_mut() {
            *v = normalize_todo_text("cue", v)?
        }
        if let Some(v) = input.detail.as_mut() {
            *v = normalize_todo_text("detail", v)?
        }
        if let Some(v) = input.parent_id.as_mut() {
            *v = normalize_todo_id("parent_id", v)?
        }
        if let Some(v) = input.target_at.as_mut() {
            *v = normalize_todo_target_at(&self.conn, v)?
        }
        if let Some(v) = input.request_id.as_mut() {
            *v = normalize_todo_text("request_id", v)?
        }
        let fingerprint = todo_fingerprint(&input)?;
        let scope = self.scope.clone();
        let database = self.database.to_string_lossy().into_owned();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent_id) = input.parent_id.as_deref() {
            let exists = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM todo_items WHERE id=?1)",
                [parent_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(AppError::new(
                    "todo_parent_not_found",
                    format!("Parent Todo '{parent_id}' was not found"),
                ));
            }
        }
        if let Some(req) = input.request_id.as_deref()
            && let Some((id, stored)) = tx
                .query_row(
                    "SELECT id,fingerprint FROM todo_items WHERE request_id=?1",
                    [req],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
        {
            if stored != fingerprint {
                return Err(AppError::new(
                    "todo_request_conflict",
                    format!("request_id '{req}' was already used for different content"),
                ));
            }
            let todo = load_todo(&tx, &id, &scope)?;
            tx.commit()?;
            return Ok(
                json!({"scope":scope,"database":database,"action":"unchanged","created":false,"todo":todo}),
            );
        }
        let (id, now): (String, String) = tx.query_row(
            &format!("SELECT LOWER(HEX(RANDOMBLOB(16))),{TIMESTAMP_SQL}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.execute("INSERT INTO todo_items(id,request_id,fingerprint,title,cue,detail,state,revision,created_at,updated_at,parent_id,target_at) VALUES(?1,?2,?3,?4,?5,?6,'open',1,?7,?7,?8,?9)",params![id,input.request_id,fingerprint,input.title,input.cue,input.detail,now,input.parent_id,input.target_at])?;
        for tag in &input.tags {
            tx.execute(
                "INSERT INTO todo_tags(todo_id,tag_name) VALUES(?1,?2)",
                params![id, tag],
            )?;
        }
        index_todo(
            &tx,
            &id,
            &input.title,
            &input.tags,
            input.cue.as_deref(),
            input.detail.as_deref(),
        )?;
        record_operation(
            &tx,
            "todo_add",
            &id,
            &json!({"request_id_present":input.request_id.is_some(),"has_parent":input.parent_id.is_some(),"has_target_at":input.target_at.is_some()}),
        )?;
        let todo = load_todo(&tx, &id, &scope)?;
        tx.commit()?;
        Ok(json!({"scope":scope,"database":database,"action":"created","created":true,"todo":todo}))
    }
    pub fn todo_show(&self, id: &str) -> Result<Value> {
        Ok(
            json!({"scope":self.scope,"database":self.database,"todo":load_todo(&self.conn,id,&self.scope)?}),
        )
    }
    pub fn todo_query(
        &self,
        query: Option<&str>,
        state: Option<&str>,
        tag: Option<&str>,
        parent: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        let parent = parent
            .map(|value| normalize_todo_id("parent", value))
            .transpose()?;
        let mut st = self
            .conn
            .prepare("SELECT id FROM todo_items ORDER BY updated_at DESC,id")?;
        let ids = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let qtokens = query.map(tokenize_for_query).unwrap_or_default();
        let mut out = Vec::new();
        for id in ids {
            let item = load_todo(&self.conn, &id, &self.scope)?;
            if state.is_some_and(|s| item["state"] != s) {
                continue;
            }
            if tag.is_some_and(|t| !item["tags"].as_array().unwrap().iter().any(|v| v == t)) {
                continue;
            }
            if parent
                .as_deref()
                .is_some_and(|value| item["parent_id"] != value)
            {
                continue;
            }
            if !qtokens.is_empty() {
                let hay = format!(
                    "{} {} {} {}",
                    item["title"].as_str().unwrap_or(""),
                    item["cue"].as_str().unwrap_or(""),
                    item["detail"].as_str().unwrap_or(""),
                    item["tags"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                let terms = joined_index_terms(&hay);
                if !qtokens.iter().all(|t| terms.contains(t)) {
                    continue;
                }
            }
            out.push(item);
        }
        Ok(out.into_iter().skip(offset).take(limit).collect())
    }
    pub fn todo_update(
        &mut self,
        id: &str,
        expected: i64,
        mut input: TodoUpdateInput,
    ) -> Result<Value> {
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = load_todo(&tx, id, &scope)?;
        let current = old["revision"].as_i64().unwrap();
        if current != expected {
            return Err(todo_revision_error(expected, current));
        }
        if old["state"] != "open" {
            return Err(AppError::new(
                "invalid_todo_transition",
                "only open Todos can be updated",
            ));
        }
        let title = match input.title.take() {
            Some(v) => normalize_todo_text("title", &v)?,
            None => old["title"].as_str().unwrap().to_owned(),
        };
        let cue = input
            .cue
            .unwrap_or_else(|| old["cue"].as_str().map(str::to_owned));
        let cue = cue
            .map(|value| normalize_todo_text("cue", &value))
            .transpose()?;
        let detail = input
            .detail
            .unwrap_or_else(|| old["detail"].as_str().map(str::to_owned));
        let detail = detail
            .map(|value| normalize_todo_text("detail", &value))
            .transpose()?;
        let target_at = input
            .target_at
            .unwrap_or_else(|| old["target_at"].as_str().map(str::to_owned));
        let target_at = target_at
            .map(|value| normalize_todo_target_at(&tx, &value))
            .transpose()?;
        let mut tags = old["tags"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for t in normalize_labels(input.add_tags)? {
            tags.insert(t);
        }
        for t in normalize_labels(input.remove_tags)? {
            tags.remove(&t);
        }
        let tags = tags.into_iter().collect::<Vec<_>>();
        if title == old["title"]
            && cue.as_deref() == old["cue"].as_str()
            && detail.as_deref() == old["detail"].as_str()
            && target_at.as_deref() == old["target_at"].as_str()
            && json!(tags) == old["tags"]
        {
            tx.commit()?;
            return Ok(json!({"action":"unchanged","todo":old}));
        }
        tx.execute(&format!("UPDATE todo_items SET title=?2,cue=?3,detail=?4,target_at=?5,revision=revision+1,updated_at={TIMESTAMP_SQL} WHERE id=?1"),params![id,title,cue,detail,target_at])?;
        tx.execute("DELETE FROM todo_tags WHERE todo_id=?1", [id])?;
        for tag in &tags {
            tx.execute("INSERT INTO todo_tags VALUES(?1,?2)", params![id, tag])?;
        }
        index_todo(&tx, id, &title, &tags, cue.as_deref(), detail.as_deref())?;
        record_operation(&tx, "todo_update", id, &json!({}))?;
        let todo = load_todo(&tx, id, &scope)?;
        tx.commit()?;
        Ok(json!({"action":"updated","todo":todo}))
    }
    pub fn todo_transition(
        &mut self,
        id: &str,
        expected: i64,
        target: &str,
        payload: Option<&str>,
    ) -> Result<Value> {
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = load_todo(&tx, id, &scope)?;
        let current = old["revision"].as_i64().unwrap();
        let state = old["state"].as_str().unwrap();
        if state == target
            && (expected == current || expected.checked_add(1) == Some(current))
            && ((target == "done" && old["result"].as_str() == payload)
                || (target == "cancelled" && old["cancel_reason"].as_str() == payload))
        {
            tx.commit()?;
            return Ok(json!({"action":"unchanged","todo":old}));
        }
        if current != expected {
            return Err(todo_revision_error(expected, current));
        }
        let valid = matches!(
            (state, target),
            ("open", "done") | ("open", "cancelled") | ("done", "open") | ("cancelled", "open")
        );
        if !valid {
            return Err(AppError::new(
                "invalid_todo_transition",
                format!("cannot transition Todo from {state} to {target}"),
            ));
        }
        if target != "open" {
            normalize_todo_text(
                if target == "done" { "result" } else { "reason" },
                payload.unwrap_or(""),
            )?;
        }
        tx.execute(&format!("UPDATE todo_items SET state=?2,result=?3,cancel_reason=?4,revision=revision+1,updated_at={TIMESTAMP_SQL},closed_at=CASE WHEN ?2='open' THEN NULL ELSE {TIMESTAMP_SQL} END WHERE id=?1"),params![id,target,if target=="done"{payload}else{None},if target=="cancelled"{payload}else{None}])?;
        record_operation(&tx, &format!("todo_{target}"), id, &json!({}))?;
        let todo = load_todo(&tx, id, &scope)?;
        tx.commit()?;
        Ok(json!({"action":"updated","todo":todo}))
    }
}
