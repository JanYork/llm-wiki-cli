// Discussion records are opt-in, separate from searchable Wiki knowledge.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscussionInput {
    pub id: String,
    pub context: String,
    pub request_id: String,
    pub if_revision: i64,
    pub operations: Vec<DiscussionOperation>,
    pub from_context: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiscussionOperation {
    pub op: String,
    pub id: Option<String>,
    pub parent: Option<String>,
    pub text: Option<String>,
    pub description: Option<String>,
    pub reason: Option<String>,
    #[serde(default)]
    pub refs: Vec<String>,
    pub ordinal: Option<i64>,
    pub confirmed: Option<bool>,
    pub message_id: Option<String>,
}
fn discussion_error(message: &str) -> AppError {
    AppError::new("invalid_discussion", message)
}
fn discussion_text(value: Option<&str>) -> Result<&str> {
    value
        .filter(|s| !s.trim().is_empty() && s.len() <= 65536)
        .ok_or_else(|| discussion_error("nonempty text up to 65536 bytes required"))
}
fn create_discussion_schema(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS discussions(id TEXT PRIMARY KEY, context TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS discussion_revisions(discussion_id TEXT NOT NULL REFERENCES discussions(id), revision INTEGER NOT NULL, request_id TEXT NOT NULL, input TEXT NOT NULL, body TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), PRIMARY KEY(discussion_id,revision), UNIQUE(discussion_id,request_id));
      CREATE TABLE IF NOT EXISTS discussion_bindings(context TEXT PRIMARY KEY, discussion_id TEXT NOT NULL REFERENCES discussions(id));")?;
    // Legacy prototype snapshots duplicated every preceding answer. Inputs retain history.
    tx.execute(
        "UPDATE discussion_revisions SET body='{}' WHERE body<>'{}'",
        [],
    )?;
    Ok(())
}
fn discussion_active(item: &Value) -> bool {
    item["withdrawn"] != true
}
fn invalidate_discussion(items: &mut serde_json::Map<String, Value>, seeds: &[String]) {
    let mut changed: BTreeSet<String> = seeds.iter().cloned().collect();
    for (id, item) in items.iter() {
        if item["parent"]
            .as_str()
            .is_some_and(|parent| changed.contains(parent))
        {
            changed.insert(id.clone());
        }
    }
    loop {
        let before = changed.len();
        for (id, item) in items.iter_mut() {
            if item["kind"] == "summary"
                && !seeds.contains(id)
                && item["refs"].as_array().is_some_and(|rs| {
                    rs.iter()
                        .any(|r| r.as_str().is_some_and(|s| changed.contains(s)))
                })
            {
                item["stale"] = json!(true);
                item["confirmed"] = json!(false);
                changed.insert(id.clone());
            }
        }
        if changed.len() == before {
            break;
        }
    }
}
fn validate_discussion_body(body: &Value) -> Result<()> {
    discussion_text(body["title"].as_str())?;
    if !matches!(body["state"].as_str(), Some("active" | "paused" | "closed")) {
        return Err(discussion_error("invalid discussion state"));
    }
    let items = body["items"]
        .as_object()
        .ok_or_else(|| discussion_error("items must be an object"))?;
    if items.len() > 10000 {
        return Err(discussion_error(
            "discussion item limit exceeded; start a linked discussion",
        ));
    }
    for (id, item) in items {
        discussion_text(item["text"].as_str())?;
        discussion_text(item["original"].as_str())?;
        if !matches!(
            item["kind"].as_str(),
            Some("question" | "answer" | "option" | "reply" | "gap" | "summary")
        ) {
            return Err(discussion_error("invalid item kind"));
        }
        if id.len() > 256 || item["ordinal"].as_i64().is_none_or(|v| v < 0) {
            return Err(discussion_error("invalid item order or ID"));
        }
        if item["id"].as_str() != Some(id) {
            return Err(discussion_error("item ID mismatch"));
        }
        if let Some(parent) = item["parent"].as_str()
            && !items.get(parent).is_some_and(|v| v["kind"] == "question")
        {
            return Err(discussion_error("parent question missing"));
        }
        if item["kind"] == "summary" {
            let refs = item["refs"]
                .as_array()
                .ok_or_else(|| discussion_error("summary references missing"))?;
            if refs.is_empty() || refs.iter().any(|v| v.as_str().is_none()) {
                return Err(discussion_error("summary references required"));
            }
            let mut stack = refs.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            let mut visited = BTreeSet::new();
            while let Some(target) = stack.pop() {
                if target == id {
                    return Err(discussion_error("cyclic summary reference"));
                }
                if !visited.insert(target) {
                    continue;
                }
                let linked = items
                    .get(target)
                    .ok_or_else(|| discussion_error("unknown reference"))?;
                if linked["kind"] == "summary" {
                    stack.extend(
                        linked["refs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str),
                    );
                }
            }
        }
    }
    Ok(())
}
impl Store {
    pub fn discussion_apply(&mut self, input: DiscussionInput) -> Result<Value> {
        normalize_agent_context(&input.context)?;
        for value in [&input.id, &input.context, &input.request_id] {
            if value.trim().is_empty() || value.len() > 256 {
                return Err(discussion_error("IDs must be 1..256 bytes"));
            }
        }
        if input.operations.is_empty() || input.operations.len() > 64 {
            return Err(discussion_error("expected 1..64 operations"));
        }
        if input.if_revision < 0 || input.if_revision == i64::MAX {
            return Err(discussion_error("invalid revision"));
        }
        let encoded =
            serde_json::to_string(&input).map_err(|e| discussion_error(&e.to_string()))?;
        if encoded.len() > 1024 * 1024 {
            return Err(discussion_error("batch exceeds 1 MiB"));
        }
        if !crate::secret_scan::detect_possible_secret_reasons(
            Path::new("discussion.txt"),
            &encoded,
        )
        .is_empty()
        {
            return Err(discussion_error(
                "possible secret excluded; submit a gap marker without sensitive text",
            ));
        }
        self.conn.pragma_update(None, "synchronous", "FULL")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let frozen: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='changeset_frozen')",
            [],
            |r| r.get(0),
        )?;
        if frozen {
            return Err(discussion_error("store is frozen for publication"));
        }
        let existing: Option<(String, i64, String)> = tx
            .query_row(
                "SELECT context,revision,body FROM discussions WHERE id=?1",
                [&input.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((context, _, _)) = &existing
            && context != &input.context
            && !(input.from_context.as_ref() == Some(context)
                && input.operations.first().is_some_and(|op| op.op == "resume"))
        {
            return Err(discussion_error("context mismatch"));
        }
        let retry:Option<(String,i64)>=tx.query_row("SELECT input,revision FROM discussion_revisions WHERE discussion_id=?1 AND request_id=?2",params![input.id,input.request_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((previous, revision)) = retry {
            let previous: DiscussionInput =
                serde_json::from_str(&previous).map_err(|e| discussion_error(&e.to_string()))?;
            let previous =
                serde_json::to_string(&previous).map_err(|e| discussion_error(&e.to_string()))?;
            if previous != encoded {
                return Err(discussion_error("request ID reused with different input"));
            }
            return Ok(json!({"id":input.id,"revision":revision,"replayed":true}));
        }
        let revision = existing.as_ref().map(|v| v.1).unwrap_or(0);
        if revision != input.if_revision {
            return Err(discussion_error("revision conflict"));
        }
        // ponytail: bounded 10000-item current document; normalize items if large-discussion profiling warrants it.
        let mut body = if let Some((_, _, raw)) = existing {
            serde_json::from_str::<Value>(&raw).map_err(|e| discussion_error(&e.to_string()))?
        } else {
            json!({"id":input.id,"state":"active","items":{}})
        };
        if body["state"] != "active" && input.operations.first().is_none_or(|op| op.op != "resume")
        {
            return Err(discussion_error(
                "resume the discussion explicitly before editing",
            ));
        }
        let bound: Option<String> = tx
            .query_row(
                "SELECT discussion_id FROM discussion_bindings WHERE context=?1",
                [&input.context],
                |r| r.get(0),
            )
            .optional()?;
        if bound.as_ref().is_some_and(|id| id != &input.id) {
            return Err(discussion_error(
                "pause current discussion before switching",
            ));
        }
        let mut started = revision > 0;
        for op in &input.operations {
            if op.op == "start" {
                if started {
                    return Err(discussion_error("already started"));
                }
                body["title"] = json!(discussion_text(op.text.as_deref())?);
                body["description"] = json!(op.description.as_deref().unwrap_or(""));
                started = true;
                continue;
            }
            if !started {
                return Err(discussion_error("start must be first"));
            }
            if op.op == "resume" {
                discussion_text(op.reason.as_deref())?;
                body["state"] = json!("active");
                continue;
            }
            if body["state"] != "active" {
                return Err(discussion_error("operation after terminal transition"));
            }
            if op.op == "pause" {
                discussion_text(op.reason.as_deref())?;
                body["state"] = json!("paused");
                continue;
            }
            if op.op == "metadata" {
                discussion_text(op.reason.as_deref())?;
                if let Some(text) = &op.text {
                    body["title"] = json!(discussion_text(Some(text))?);
                }
                if let Some(description) = &op.description {
                    body["description"] = json!(description);
                }
                continue;
            }
            if op.op == "close" {
                let items = body["items"].as_object().unwrap();
                if !items
                    .values()
                    .any(|v| v["kind"] == "summary" && discussion_active(v))
                    || items.values().any(|v| {
                        discussion_active(v) && v["kind"] == "summary" && v["stale"] == true
                    })
                {
                    return Err(discussion_error("current summary required"));
                }
                if items.iter().any(|(id, v)| {
                    v["kind"] == "question"
                        && discussion_active(v)
                        && !items.values().any(|a| {
                            discussion_active(a)
                                && a["kind"] == "answer"
                                && a["parent"].as_str() == Some(id)
                        })
                }) {
                    return Err(discussion_error("unanswered question"));
                }
                if items
                    .values()
                    .any(|v| discussion_active(v) && v["kind"] == "reply")
                {
                    return Err(discussion_error(
                        "associate or withdraw unassigned replies before closing",
                    ));
                }
                let mut covered = BTreeSet::new();
                for summary in items
                    .values()
                    .filter(|v| discussion_active(v) && v["kind"] == "summary")
                {
                    for reference in summary["refs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        covered.insert(reference.to_string());
                        if let Some(parent) = items[reference]["parent"].as_str() {
                            covered.insert(parent.to_string());
                        }
                    }
                }
                if items.iter().any(|(id, v)| {
                    discussion_active(v)
                        && matches!(v["kind"].as_str(), Some("question" | "gap"))
                        && !covered.contains(id)
                }) {
                    return Err(discussion_error(
                        "summary must cover each active question and capture gap",
                    ));
                }
                body["state"] = json!("closed");
                continue;
            }
            if body["state"] != "active" {
                return Err(discussion_error("operation after close"));
            }
            let id = discussion_text(op.id.as_deref())?;
            let text = op.text.as_deref().unwrap_or("");
            let items = body["items"].as_object_mut().unwrap();
            match op.op.as_str() {
                "question" | "answer" | "option" | "summary" | "gap" | "reply" => {
                    discussion_text(Some(text))?;
                    if id.len() > 256 {
                        return Err(discussion_error("item ID too long"));
                    }
                    if let Some(message) = &op.message_id
                        && items
                            .values()
                            .any(|v| v["message_id"].as_str() == Some(message))
                    {
                        return Err(discussion_error(
                            "message already recorded; recover current state",
                        ));
                    }
                    if items.contains_key(id) {
                        return Err(discussion_error("item already exists"));
                    }
                    if matches!(op.op.as_str(), "answer" | "option")
                        && op
                            .parent
                            .as_ref()
                            .and_then(|p| items.get(p))
                            .is_none_or(|p| p["kind"] != "question")
                    {
                        return Err(discussion_error("parent question required"));
                    }
                    if op.op == "summary"
                        && (op.refs.is_empty()
                            || op.refs.iter().any(|r| {
                                !items
                                    .get(r)
                                    .is_some_and(|v| discussion_active(v) && v["stale"] != true)
                            }))
                    {
                        return Err(discussion_error("valid summary references required"));
                    }
                    if op.op != "summary" {
                        let seeds = op.parent.iter().cloned().collect::<Vec<_>>();
                        invalidate_discussion(items, &seeds);
                    }
                    let ordinal = items.len();
                    items.insert(id.into(),json!({"id":id,"kind":op.op,"original":text,"text":text,"parent":op.parent,"refs":op.refs,"stale":false,"ordinal":ordinal,"revision":revision+1,"confirmed":false,"withdrawn":false,"message_id":op.message_id,"delivery":"prepared"}));
                }
                "withdraw" | "restore" | "move" | "confirm" | "delivery" => {
                    discussion_text(op.reason.as_deref())?;
                    if let Some(parent) = &op.parent
                        && !items
                            .get(parent)
                            .is_some_and(|v| v["kind"] == "question" && discussion_active(v))
                    {
                        return Err(discussion_error("active parent question required"));
                    }
                    let item = items
                        .get_mut(id)
                        .ok_or_else(|| discussion_error("item missing"))?;
                    let old_parent = item["parent"].as_str().map(str::to_string);
                    match op.op.as_str() {
                        "withdraw" => item["withdrawn"] = json!(true),
                        "restore" => item["withdrawn"] = json!(false),
                        "move" => {
                            if let Some(parent) = &op.parent {
                                if !matches!(
                                    item["kind"].as_str(),
                                    Some("answer" | "option" | "reply")
                                ) {
                                    return Err(discussion_error(
                                        "only answers/options have parent questions",
                                    ));
                                }
                                item["parent"] = json!(parent);
                                if item["kind"] == "reply" {
                                    item["kind"] = json!("answer");
                                }
                            }
                            if let Some(ordinal) = op.ordinal {
                                if ordinal < 0 {
                                    return Err(discussion_error("negative ordinal"));
                                }
                                item["ordinal"] = json!(ordinal);
                            }
                        }
                        "confirm" => {
                            if item["kind"] != "summary" || item["stale"] == true {
                                return Err(discussion_error(
                                    "only current summaries can be confirmed",
                                ));
                            }
                            item["confirmed"] =
                                json!(op.confirmed.ok_or_else(|| discussion_error(
                                    "confirmed boolean required"
                                ))?);
                        }
                        "delivery" => {
                            if item["kind"] != "question" || op.message_id.is_none() {
                                return Err(discussion_error(
                                    "question and host message evidence required",
                                ));
                            }
                            item["delivery"] = json!("reported_delivered");
                            item["message_id"] = json!(op.message_id);
                        }
                        _ => unreachable!(),
                    }
                    item["revision"] = json!(revision + 1);
                    if !matches!(op.op.as_str(), "confirm" | "delivery") {
                        let mut seeds = vec![id.to_string()];
                        seeds.extend(old_parent);
                        seeds.extend(op.parent.clone());
                        invalidate_discussion(items, &seeds);
                    }
                }
                "revise" => {
                    discussion_text(Some(text))?;
                    discussion_text(op.reason.as_deref())?;
                    let item = items
                        .get_mut(id)
                        .ok_or_else(|| discussion_error("item missing"))?;
                    item["text"] = json!(text);
                    item["confirmed"] = json!(false);
                    item["revision"] = json!(revision + 1);
                    if item["kind"] == "summary" {
                        if op.refs.is_empty() {
                            return Err(discussion_error("summary revision requires references"));
                        }
                        item["refs"] = json!(op.refs);
                        item["stale"] = json!(false);
                    }
                    if op.refs.iter().any(|r| {
                        !items
                            .get(r)
                            .is_some_and(|v| discussion_active(v) && v["stale"] != true)
                    }) {
                        return Err(discussion_error("unknown reference"));
                    }
                    let mut seeds = vec![id.to_string()];
                    seeds.extend(items[id]["parent"].as_str().map(str::to_string));
                    invalidate_discussion(items, &seeds);
                }
                _ => return Err(discussion_error("unsupported operation")),
            }
        }
        validate_discussion_body(&body)?;
        body["revision"] = json!(revision + 1);
        let raw = serde_json::to_string(&body).map_err(|e| discussion_error(&e.to_string()))?;
        if raw.len() > 16 * 1024 * 1024 {
            return Err(discussion_error(
                "discussion exceeds 16 MiB; continue in a linked discussion",
            ));
        }
        tx.execute("INSERT INTO discussions VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET context=excluded.context,revision=excluded.revision,body=excluded.body",params![input.id,input.context,revision+1,raw])?;
        tx.execute("INSERT INTO discussion_revisions(discussion_id,revision,request_id,input,body) VALUES(?1,?2,?3,?4,?5)",params![input.id,revision+1,input.request_id,encoded,"{}"])?;
        if let Some(previous) = &input.from_context {
            tx.execute(
                "DELETE FROM discussion_bindings WHERE context=?1 AND discussion_id=?2",
                params![previous, input.id],
            )?;
        }
        if body["state"] == "active" {
            tx.execute("INSERT INTO discussion_bindings VALUES(?1,?2) ON CONFLICT(context) DO UPDATE SET discussion_id=excluded.discussion_id",params![input.context,input.id])?;
        } else {
            tx.execute(
                "DELETE FROM discussion_bindings WHERE context=?1 AND discussion_id=?2",
                params![input.context, input.id],
            )?;
        }
        tx.execute(
            "INSERT INTO operations(action,target,detail_json) VALUES('discussion.apply',?1,?2)",
            params![
                input.id,
                json!({"revision":revision+1,"operation_count":input.operations.len()}).to_string()
            ],
        )?;
        tx.commit()?;
        Ok(json!({"id":input.id,"revision":revision+1}))
    }
    pub fn discussion_read(
        &self,
        id: &str,
        context: &str,
        mode: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        normalize_agent_context(context)?;
        if !(1..=100).contains(&limit) || offset > i64::MAX as usize {
            return Err(discussion_error("limit must be 1..100"));
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='discussions')",
            [],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(discussion_error("discussion not found"));
        }
        let bound_id: Option<String> = if id.is_empty() {
            self.conn
                .query_row(
                    "SELECT discussion_id FROM discussion_bindings WHERE context=?1",
                    [context],
                    |r| r.get(0),
                )
                .optional()?
        } else {
            Some(id.to_string())
        };
        let Some(id) = bound_id.as_deref() else {
            return Ok(json!({"discussion":null}));
        };
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT context,body FROM discussions WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (owner, raw) = row.ok_or_else(|| discussion_error("discussion not found"))?;
        if !owner.is_empty() && owner != context {
            return Err(discussion_error("context mismatch"));
        }
        let mut body: Value =
            serde_json::from_str(&raw).map_err(|e| discussion_error(&e.to_string()))?;
        if mode == "current" {
            let items = body["items"].as_object().unwrap();
            let pending: Vec<_> = items
                .iter()
                .filter(|(id, v)| {
                    v["kind"] == "question"
                        && discussion_active(v)
                        && !items.values().any(|a| {
                            discussion_active(a)
                                && a["kind"] == "answer"
                                && a["parent"].as_str() == Some(id)
                        })
                })
                .map(|(id, _)| id)
                .collect();
            return Ok(
                json!({"id":id,"title":bounded_hook_text(body["title"].as_str().unwrap_or("").into()),"revision":body["revision"],"state":body["state"],"pending":pending.iter().take(20).collect::<Vec<_>>(),"pending_total":pending.len(),"pending_questions":pending.iter().take(3).map(|id|json!({"id":id,"text":bounded_hook_text(items[*id]["text"].as_str().unwrap_or("").into())})).collect::<Vec<_>>(),"last_item":items.values().max_by_key(|v|v["revision"].as_i64().unwrap_or(0)).map(|v|json!({"id":v["id"],"kind":v["kind"],"text":bounded_hook_text(v["text"].as_str().unwrap_or("").into())})),"description":bounded_hook_text(body["description"].as_str().unwrap_or("").into()),"unassigned":items.values().filter(|v|v["kind"]=="reply" && discussion_active(v)).count(),"capture":"agent_submitted_with_optional_host_reply"}),
            );
        }
        if mode == "history" {
            let total: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM discussion_revisions WHERE discussion_id=?1",
                [id],
                |r| r.get(0),
            )?;
            let mut statement=self.conn.prepare("SELECT revision,input,created_at FROM discussion_revisions WHERE discussion_id=?1 ORDER BY revision ASC LIMIT ?2 OFFSET ?3")?;
            let rows=statement.query_map(params![id,limit as i64,offset as i64],|r|Ok(json!({"revision":r.get::<_,i64>(0)?,"input":r.get::<_,String>(1)?,"created_at":r.get::<_,String>(2)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
            return Ok(
                json!({"id":id,"revisions":rows,"limit":limit,"offset":offset,"total":total}),
            );
        }
        if mode != "export" {
            let items = body["items"].as_object().unwrap();
            let total = items.len();
            let mut ordered = items.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|(id, v)| (v["ordinal"].as_i64().unwrap_or(0), *id));
            let selected = ordered
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|(id, v)| (id.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>();
            body["items"] = json!(selected);
            body["total"] = json!(total);
            body["offset"] = json!(offset);
            body["limit"] = json!(limit);
        }
        Ok(body)
    }
}

impl Store {
    pub(crate) fn export_sync_discussions(&self, output: &Connection) -> Result<()> {
        let mut query = self
            .conn
            .prepare("SELECT id,body FROM discussions ORDER BY id")?;
        for row in query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, raw) = row?;
            let mut body: Value =
                serde_json::from_str(&raw).map_err(|e| discussion_error(&e.to_string()))?;
            let mut h=self.conn.prepare("SELECT revision,request_id,input,created_at FROM discussion_revisions WHERE discussion_id=?1 ORDER BY revision")?;
            let mut history = Vec::new();
            for row in h.query_map([&id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })? {
                let (revision, request_id, raw, at) = row?;
                let mut input: Value =
                    serde_json::from_str(&raw).map_err(|e| discussion_error(&e.to_string()))?;
                input
                    .as_object_mut()
                    .ok_or_else(|| discussion_error("invalid stored input"))?
                    .remove("context");
                input.as_object_mut().unwrap().remove("from_context");
                history.push(json!({"revision":revision,"request_id":request_id,"input":input,"created_at":at}));
            }
            body["revision"] = history
                .last()
                .map(|v| v["revision"].clone())
                .unwrap_or(json!(0));
            insert_sync_object(
                output,
                "discussion",
                &id,
                &json!({"id":id,"body":body,"history":history}),
            )?;
        }
        Ok(())
    }
}

fn import_sync_discussions(tx: &Transaction<'_>, state: &PreparedSyncState) -> Result<()> {
    let mut old = tx.prepare("SELECT id,context,revision FROM discussions")?;
    let owners = old
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, i64>(2)?),
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    drop(old);
    // Bindings stay local; imported discussions require an explicit resume/transfer.
    let mut binding_query = tx.prepare("SELECT context,discussion_id FROM discussion_bindings")?;
    let bindings = binding_query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(binding_query);
    tx.execute_batch("DELETE FROM discussion_bindings; DELETE FROM discussion_revisions; DELETE FROM discussions;")?;
    for (id, payload) in objects_of_kind(state, "discussion") {
        ensure_payload_key(payload, "id", id, "discussion")?;
        let mut body = payload["body"].clone();
        if body["id"].as_str() != Some(id) {
            return Err(discussion_error("imported discussion ID mismatch"));
        }
        validate_discussion_body(&body)?;
        let history = payload["history"]
            .as_array()
            .ok_or_else(|| discussion_error("imported history missing"))?;
        let remote = body["revision"]
            .as_i64()
            .filter(|v| *v > 0 && *v < i64::MAX - 1)
            .ok_or_else(|| discussion_error("invalid imported revision"))?;
        let owner = owners.get(id).map(|v| v.0.as_str()).unwrap_or("");
        let local = owners.get(id).map(|v| v.1).unwrap_or(0);
        let revision = remote
            .max(local)
            .checked_add(1)
            .ok_or_else(|| discussion_error("revision overflow"))?;
        body["revision"] = json!(revision);
        let raw = serde_json::to_string(&body).map_err(|e| discussion_error(&e.to_string()))?;
        tx.execute(
            "INSERT INTO discussions VALUES(?1,?2,?3,?4)",
            params![id, owner, revision, raw],
        )?;
        let mut previous = 0;
        for event in history {
            let rev = event["revision"]
                .as_i64()
                .filter(|r| *r > previous && *r <= remote)
                .ok_or_else(|| discussion_error("invalid history order"))?;
            previous = rev;
            let req = discussion_text(event["request_id"].as_str())?;
            let at = discussion_text(event["created_at"].as_str())?;
            let mut input = event["input"].clone();
            let map = input
                .as_object_mut()
                .ok_or_else(|| discussion_error("invalid history input"))?;
            map.insert("context".into(), json!(owner));
            let raw =
                serde_json::to_string(&input).map_err(|e| discussion_error(&e.to_string()))?;
            tx.execute("INSERT INTO discussion_revisions(discussion_id,revision,request_id,input,body,created_at) VALUES(?1,?2,?3,?4,'{}',?5)",params![id,rev,req,raw,at])?;
        }
        if previous != remote {
            return Err(discussion_error("incomplete imported history"));
        }
    }
    for (context, id) in bindings {
        tx.execute("INSERT INTO discussion_bindings SELECT ?1,id FROM discussions WHERE id=?2 AND context=?1 AND json_extract(body,'$.state')='active'",params![context,id])?;
    }
    Ok(())
}

impl Store {
    pub(crate) fn open_for_discussion_capture(database: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(Duration::from_millis(100))?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        prepare_store_read_only(&conn)?;
        Ok(Self {
            scope: "project".into(),
            database: database.to_path_buf(),
            conn,
        })
    }
    pub(crate) fn capture_discussion_reply(
        &mut self,
        context: &str,
        message_id: &str,
        text: &str,
    ) -> Result<Option<Value>> {
        let current = self.discussion_read("", context, "current", 0, 20)?;
        let Some(id) = current["id"].as_str() else {
            return Ok(None);
        };
        let body = self.discussion_read(id, context, "export", 0, 100)?;
        if let Some(existing) = body["items"]
            .as_object()
            .unwrap()
            .values()
            .find(|v| v["message_id"].as_str() == Some(message_id))
        {
            if existing["original"].as_str() != Some(text) {
                return Err(discussion_error(
                    "host message changed; explicit correction required",
                ));
            }
            return Ok(Some(json!({"recorded":true,"replayed":true})));
        }
        let key = format!(
            "host-{}",
            &hash_content(&format!("{id}\0{message_id}"))[..32]
        );
        let input:DiscussionInput=serde_json::from_value(json!({"id":id,"context":context,"request_id":key,"if_revision":current["revision"],"operations":[{"op":"reply","id":key,"text":text,"message_id":message_id}]})).map_err(|e|discussion_error(&e.to_string()))?;
        self.discussion_apply(input).map(Some)
    }
}

impl Store {
    pub fn discussion_list(&self, context: &str, offset: usize, limit: usize) -> Result<Value> {
        normalize_agent_context(context)?;
        if !(1..=100).contains(&limit) || offset > i64::MAX as usize {
            return Err(discussion_error("invalid pagination"));
        }
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM discussions WHERE context=?1 OR context=''",
            [context],
            |r| r.get(0),
        )?;
        let mut q=self.conn.prepare("SELECT id,context,revision,json_extract(body,'$.title'),json_extract(body,'$.state') FROM discussions WHERE context=?1 OR context='' ORDER BY id LIMIT ?2 OFFSET ?3")?;
        let rows=q.query_map(params![context,limit as i64,offset as i64],|r|Ok(json!({"id":r.get::<_,String>(0)?,"unbound":r.get::<_,String>(1)?.is_empty(),"revision":r.get::<_,i64>(2)?,"title":bounded_hook_text(r.get::<_,String>(3)?),"state":r.get::<_,String>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({"discussions":rows,"total":total,"offset":offset,"limit":limit}))
    }
    pub fn discussion_item(&self, id: &str, context: &str, item: &str) -> Result<Value> {
        let body = self.discussion_read(id, context, "export", 0, 100)?;
        let value = body["items"]
            .get(item)
            .ok_or_else(|| discussion_error("item missing"))?;
        Ok(json!({"id":id,"revision":body["revision"],"item":value}))
    }
}
