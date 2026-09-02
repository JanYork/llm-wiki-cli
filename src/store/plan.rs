#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStepInput {
    pub title: String,
    pub verify: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCreateInput {
    pub title: String,
    pub objective: String,
    pub done_when: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub steps: Vec<PlanStepInput>,
    pub request_id: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanReviseInput {
    pub steps: Vec<PlanStepInput>,
    pub focal: Option<usize>,
}

fn plan_fingerprint(input: &PlanCreateInput) -> Result<String> {
    let mut c = input.clone();
    c.request_id = None;
    c.tags = normalize_labels(c.tags)?;
    Ok(Sha256::digest(
        serde_json::to_vec(&c).map_err(|e| AppError::new("json_error", e.to_string()))?,
    )
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect())
}
fn plan_revision_error(expected: i64, current: i64) -> AppError {
    AppError::new(
        "plan_revision_conflict",
        format!("expected revision {expected}, current revision is {current}"),
    )
    .with_details(json!({"current_revision":current}))
}
fn load_plan(conn: &Connection, id: &str, scope: &str) -> Result<Value> {
    let mut p=conn.query_row("SELECT id,request_id,title,objective,done_when,state,result,completion_evidence,done_when_checked,abandoned_reason,revision,created_at,updated_at,closed_at FROM plans WHERE id=?1",[id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"request_id":r.get::<_,Option<String>>(1)?,"scope":scope,"title":r.get::<_,String>(2)?,"objective":r.get::<_,String>(3)?,"done_when":r.get::<_,String>(4)?,"state":r.get::<_,String>(5)?,"result":r.get::<_,Option<String>>(6)?,"completion_evidence":r.get::<_,Option<String>>(7)?,"done_when_checked":r.get::<_,bool>(8)?,"abandoned_reason":r.get::<_,Option<String>>(9)?,"revision":r.get::<_,i64>(10)?,"created_at":r.get::<_,String>(11)?,"updated_at":r.get::<_,String>(12)?,"closed_at":r.get::<_,Option<String>>(13)?,"tags":[],"constraints":[],"steps":[]}))).optional()?.ok_or_else(||AppError::new("plan_not_found",format!("Plan '{id}' was not found")))?;
    let mut st =
        conn.prepare("SELECT tag_name FROM plan_tags WHERE plan_id=?1 ORDER BY tag_name")?;
    p["tags"] = json!(
        st.query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    );
    let mut st =
        conn.prepare("SELECT value FROM plan_constraints WHERE plan_id=?1 ORDER BY ordinal")?;
    p["constraints"] = json!(
        st.query_map([id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    );
    let mut st=conn.prepare("SELECT step_id,ordinal,title,status,verify,result,blocker,created_revision,updated_revision,created_at,updated_at FROM plan_steps WHERE plan_id=?1 ORDER BY ordinal")?;
    p["steps"]=json!(st.query_map([id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"ordinal":r.get::<_,i64>(1)?,"title":r.get::<_,String>(2)?,"status":r.get::<_,String>(3)?,"verify":r.get::<_,Option<String>>(4)?,"result":r.get::<_,Option<String>>(5)?,"blocker":r.get::<_,Option<String>>(6)?,"created_revision":r.get::<_,i64>(7)?,"updated_revision":r.get::<_,i64>(8)?,"created_at":r.get::<_,String>(9)?,"updated_at":r.get::<_,String>(10)?})))?.collect::<std::result::Result<Vec<_>,_>>()?);
    Ok(p)
}
fn index_plan(tx: &Transaction<'_>, id: &str, p: &Value) -> Result<()> {
    tx.execute("DELETE FROM plan_fts WHERE plan_id=?1", [id])?;
    let tags = p["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let cons = p["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let steps = p["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["title"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    tx.execute("INSERT INTO plan_fts(plan_id,title_terms,tag_terms,objective_terms,constraint_terms,step_terms)VALUES(?1,?2,?3,?4,?5,?6)",params![id,joined_index_terms(p["title"].as_str().unwrap()),joined_index_terms(&tags),joined_index_terms(p["objective"].as_str().unwrap()),joined_index_terms(&cons),joined_index_terms(&steps)])?;
    Ok(())
}
fn require_active(plan: &Value) -> Result<()> {
    if plan["state"] != "active" {
        return Err(AppError::new(
            "invalid_plan_transition",
            "Plan is not active",
        ));
    }
    Ok(())
}

fn bounded_hook_text(value: String) -> String {
    const LIMIT: usize = 500;
    if value.chars().count() <= LIMIT {
        value
    } else {
        value.chars().take(LIMIT - 1).chain(['…']).collect()
    }
}

impl Store {
    pub fn active_plan_count(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM plans WHERE state='active'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn plan_tracking(&self) -> Result<Option<Value>> {
        let plan = self
            .conn
            .query_row(
                "SELECT id,title,revision FROM plans WHERE state='active' ORDER BY updated_at DESC,id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, title, revision)) = plan else {
            return Ok(None);
        };
        self.plan_tracking_value(id, title, revision).map(Some)
    }

    pub fn plan_tracking_for_context(&self, context: &str) -> Result<Option<Value>> {
        let context = normalize_agent_context(context)?;
        let plan = self
            .conn
            .query_row(
                "SELECT p.id,p.title,p.revision FROM agent_plan_tracks a JOIN plans p ON p.id=a.plan_id WHERE a.context_id=?1 AND p.state='active'",
                [&context],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
            )
            .optional()?;
        let Some((id, title, revision)) = plan else {
            return Ok(None);
        };
        self.plan_tracking_value(id, title, revision).map(Some)
    }

    fn plan_tracking_value(&self, id: String, title: String, revision: i64) -> Result<Value> {
        let (completed_steps, terminal_steps, total_steps) = self.conn.query_row(
            "SELECT
                SUM(CASE WHEN status='completed' THEN 1 ELSE 0 END),
                SUM(CASE WHEN status IN ('completed','skipped') THEN 1 ELSE 0 END),
                COUNT(*)
             FROM plan_steps WHERE plan_id=?1",
            [&id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )?;
        let load_step = |sql: &str| -> Result<Option<Value>> {
            Ok(self
                .conn
                .query_row(sql, [&id], |row| {
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "title": bounded_hook_text(row.get::<_, String>(1)?),
                        "status": row.get::<_, String>(2)?,
                    }))
                })
                .optional()?)
        };
        Ok(json!({
            "id": id,
            "title": bounded_hook_text(title),
            "revision": revision,
            "progress": {
                "completed_steps": completed_steps,
                "terminal_steps": terminal_steps,
                "total_steps": total_steps,
            },
            "current_step": load_step(
                "SELECT step_id,title,status FROM plan_steps
                 WHERE plan_id=?1 AND status IN ('in_progress','blocked')
                 ORDER BY ordinal LIMIT 1",
            )?,
            "next_step": load_step(
                "SELECT step_id,title,status FROM plan_steps
                 WHERE plan_id=?1 AND status='pending' ORDER BY ordinal LIMIT 1",
            )?,
            "brief": format!("lwc plan brief {id}"),
        }))
    }

    pub fn plan_track(&mut self, id: &str, context: &str) -> Result<Value> {
        let id = normalize_todo_id("plan_id", id)?;
        let context = normalize_agent_context(context)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = tx
            .query_row("SELECT state FROM plans WHERE id=?1", [&id], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .ok_or_else(|| AppError::new("plan_not_found", format!("Plan '{id}' was not found")))?;
        if state != "active" {
            return Err(AppError::new(
                "invalid_plan_transition",
                "only active Plans can be tracked",
            ));
        }
        if let Some(existing) = tx
            .query_row(
                "SELECT plan_id FROM agent_plan_tracks WHERE context_id=?1",
                [&context],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if existing == id {
                tx.commit()?;
                return Ok(json!({"action":"unchanged","context":context,"plan_id":id}));
            }
            return Err(AppError::new(
                "plan_tracking_conflict",
                "the context already tracks another Plan",
            )
            .with_details(json!({"plan_id":existing})));
        }
        tx.execute(
            "INSERT INTO agent_plan_tracks(context_id,plan_id) VALUES(?1,?2)",
            params![context, id],
        )?;
        tx.commit()?;
        Ok(json!({"action":"tracked","context":context,"plan_id":id}))
    }
    pub fn plan_untrack(&mut self, id: &str, context: &str) -> Result<Value> {
        let id = normalize_todo_id("plan_id", id)?;
        let context = normalize_agent_context(context)?;
        let changed = self.conn.execute(
            "DELETE FROM agent_plan_tracks WHERE context_id=?1 AND plan_id=?2",
            params![context, id],
        )?;
        if changed == 0 {
            return Err(AppError::new(
                "plan_tracking_not_found",
                "the context does not track that Plan",
            ));
        }
        Ok(json!({"action":"untracked","context":context,"plan_id":id}))
    }
    pub fn tracked_plan(&self, context: &str) -> Result<Value> {
        let context = normalize_agent_context(context)?;
        let id = self
            .conn
            .query_row(
                "SELECT p.id FROM agent_plan_tracks a JOIN plans p ON p.id=a.plan_id WHERE a.context_id=?1 AND p.state='active'",
                [&context],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let plan = id
            .as_deref()
            .map(|id| load_plan(&self.conn, id, &self.scope))
            .transpose()?;
        Ok(json!({"scope":self.scope,"database":self.database,"context":context,"plan":plan}))
    }

    pub fn plan_create(&mut self, mut input: PlanCreateInput) -> Result<Value> {
        input.title = normalize_todo_text("title", &input.title)?;
        input.objective = normalize_todo_text("objective", &input.objective)?;
        input.done_when = normalize_todo_text("done_when", &input.done_when)?;
        input.tags = normalize_labels(input.tags)?;
        input.constraints = input
            .constraints
            .into_iter()
            .map(|v| normalize_todo_text("constraint", &v))
            .collect::<Result<Vec<_>>>()?;
        if input.constraints.len() > 100 {
            return Err(AppError::new(
                "invalid_input",
                "at most 100 constraints are allowed",
            ));
        }
        if input.steps.is_empty() || input.steps.len() > 100 {
            return Err(AppError::new(
                "invalid_input",
                "steps must contain 1 to 100 items",
            ));
        }
        for s in &mut input.steps {
            s.title = normalize_todo_text("step title", &s.title)?;
            if let Some(v) = s.verify.as_mut() {
                *v = normalize_todo_text("verify", v)?
            }
        }
        if let Some(v) = input.request_id.as_mut() {
            *v = normalize_todo_text("request_id", v)?
        }
        let fingerprint = plan_fingerprint(&input)?;
        let scope = self.scope.clone();
        let database = self.database.to_string_lossy().into_owned();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(req) = input.request_id.as_deref()
            && let Some((id, stored)) = tx
                .query_row(
                    "SELECT id,fingerprint FROM plans WHERE request_id=?1",
                    [req],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()?
        {
            if stored != fingerprint {
                return Err(AppError::new(
                    "plan_request_conflict",
                    format!("request_id '{req}' was already used for different content"),
                ));
            }
            let plan = load_plan(&tx, &id, &scope)?;
            tx.commit()?;
            return Ok(
                json!({"scope":scope,"database":database,"action":"unchanged","created":false,"plan":plan}),
            );
        }
        let (id, now): (String, String) = tx.query_row(
            &format!("SELECT LOWER(HEX(RANDOMBLOB(16))),{TIMESTAMP_SQL}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.execute("INSERT INTO plans(id,request_id,fingerprint,title,objective,done_when,state,revision,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,'active',1,?7,?7)",params![id,input.request_id,fingerprint,input.title,input.objective,input.done_when,now])?;
        for tag in &input.tags {
            tx.execute("INSERT INTO plan_tags VALUES(?1,?2)", params![id, tag])?;
        }
        for (i, v) in input.constraints.iter().enumerate() {
            tx.execute(
                "INSERT INTO plan_constraints VALUES(?1,?2,?3)",
                params![id, i as i64, v],
            )?;
        }
        for (i, s) in input.steps.iter().enumerate() {
            let sid: String =
                tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(16)))", [], |r| r.get(0))?;
            tx.execute("INSERT INTO plan_steps(plan_id,step_id,ordinal,title,status,verify,created_revision,updated_revision,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,1,1,?7,?7)",params![id,sid,i as i64,s.title,if i==0{"in_progress"}else{"pending"},s.verify,now])?;
        }
        tx.execute(
            "INSERT INTO plan_history(plan_id,revision,action)VALUES(?1,1,'created')",
            [&id],
        )?;
        let plan = load_plan(&tx, &id, &scope)?;
        index_plan(&tx, &id, &plan)?;
        record_operation(
            &tx,
            "plan_create",
            &id,
            &json!({"request_id_present":input.request_id.is_some()}),
        )?;
        tx.commit()?;
        Ok(json!({"scope":scope,"database":database,"action":"created","created":true,"plan":plan}))
    }
    pub fn plan_show(&self, id: &str) -> Result<Value> {
        Ok(
            json!({"scope":self.scope,"database":self.database,"plan":load_plan(&self.conn,id,&self.scope)?}),
        )
    }
    pub fn plan_query(
        &self,
        query: Option<&str>,
        state: Option<&str>,
        tag: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Value>> {
        let mut st = self
            .conn
            .prepare("SELECT id FROM plans ORDER BY updated_at DESC,id")?;
        let ids = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let q = query.map(tokenize_for_query).unwrap_or_default();
        let mut out = Vec::new();
        for id in ids {
            let p = load_plan(&self.conn, &id, &self.scope)?;
            if state.is_some_and(|s| p["state"] != s) {
                continue;
            }
            if tag.is_some_and(|t| !p["tags"].as_array().unwrap().iter().any(|v| v == t)) {
                continue;
            }
            if !q.is_empty() {
                let hay = format!(
                    "{} {} {} {}",
                    p["title"].as_str().unwrap(),
                    p["objective"].as_str().unwrap(),
                    p["constraints"],
                    p["steps"]
                );
                let terms = joined_index_terms(&hay);
                if !q.iter().all(|t| terms.contains(t)) {
                    continue;
                }
            }
            out.push(p)
        }
        Ok(out.into_iter().skip(offset).take(limit).collect())
    }
    pub fn plan_brief(&self, id: &str) -> Result<Value> {
        let mut p = load_plan(&self.conn, id, &self.scope)?;
        let steps = p["steps"].as_array().unwrap();
        let terminal = steps
            .iter()
            .filter(|s| matches!(s["status"].as_str(), Some("completed" | "skipped")))
            .rev()
            .take(20)
            .cloned()
            .collect::<Vec<_>>();
        let focal = steps
            .iter()
            .find(|s| matches!(s["status"].as_str(), Some("in_progress" | "blocked")))
            .cloned();
        let next = steps.iter().find(|s| s["status"] == "pending").cloned();
        let omitted = steps
            .iter()
            .filter(|s| matches!(s["status"].as_str(), Some("completed" | "skipped")))
            .count()
            .saturating_sub(terminal.len());
        p.as_object_mut()
            .expect("Plan is an object")
            .remove("steps");
        Ok(
            json!({"scope":self.scope,"database":self.database,"plan":p,"terminal_steps":terminal,"focal":focal,"next":next,"omitted_terminal_steps":omitted}),
        )
    }
    pub fn plan_advance(
        &mut self,
        id: &str,
        expected: i64,
        done: &str,
        result: &str,
        next: Option<&str>,
    ) -> Result<Value> {
        let result = normalize_todo_text("result", result)?;
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = load_plan(&tx, id, &scope)?;
        require_active(&p)?;
        let current = p["revision"].as_i64().unwrap();
        if current != expected {
            return Err(plan_revision_error(expected, current));
        }
        let focal = p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["status"] == "in_progress")
            .ok_or_else(|| AppError::new("invalid_plan_transition", "Plan has no focal step"))?;
        if focal["id"] != done {
            return Err(AppError::new(
                "invalid_plan_transition",
                "--done must identify the focal step",
            ));
        }
        let pending = p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["status"] == "pending")
            .collect::<Vec<_>>();
        if pending.is_empty() && next.is_some() || !pending.is_empty() && next.is_none() {
            return Err(AppError::new(
                "invalid_plan_transition",
                "--next is required exactly when pending steps remain",
            ));
        }
        if let Some(n) = next
            && !pending.iter().any(|s| s["id"] == n)
        {
            return Err(AppError::new(
                "invalid_plan_transition",
                "--next must identify a pending step",
            ));
        }
        let rev = current + 1;
        tx.execute(&format!("UPDATE plan_steps SET status='completed',result=?3,blocker=NULL,updated_revision=?4,updated_at={TIMESTAMP_SQL} WHERE plan_id=?1 AND step_id=?2"),params![id,done,result,rev])?;
        if let Some(n) = next {
            tx.execute(&format!("UPDATE plan_steps SET status='in_progress',updated_revision=?3,updated_at={TIMESTAMP_SQL} WHERE plan_id=?1 AND step_id=?2"),params![id,n,rev])?;
        }
        tx.execute(
            &format!("UPDATE plans SET revision=?2,updated_at={TIMESTAMP_SQL} WHERE id=?1"),
            params![id, rev],
        )?;
        tx.execute("INSERT INTO plan_history(plan_id,revision,action,step_id,result)VALUES(?1,?2,'advanced',?3,?4)",params![id,rev,done,result])?;
        record_operation(&tx, "plan_advance", id, &json!({"step_id":done}))?;
        let p = load_plan(&tx, id, &scope)?;
        index_plan(&tx, id, &p)?;
        tx.commit()?;
        Ok(json!({"action":"updated","plan":p}))
    }
    pub fn plan_block(
        &mut self,
        id: &str,
        expected: i64,
        step: &str,
        reason: &str,
    ) -> Result<Value> {
        let reason = normalize_todo_text("reason", reason)?;
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = load_plan(&tx, id, &scope)?;
        require_active(&p)?;
        let current = p["revision"].as_i64().unwrap();
        if current != expected {
            return Err(plan_revision_error(expected, current));
        }
        if !p["steps"].as_array().unwrap().iter().any(|s| {
            s["id"] == step && matches!(s["status"].as_str(), Some("in_progress" | "blocked"))
        }) {
            return Err(AppError::new(
                "invalid_plan_transition",
                "--step must identify the focal step",
            ));
        }
        if p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == step && s["status"] == "blocked" && s["blocker"] == reason)
        {
            tx.commit()?;
            return Ok(json!({"action":"unchanged","plan":p}));
        }
        let rev = current + 1;
        tx.execute(&format!("UPDATE plan_steps SET status='blocked',blocker=?3,updated_revision=?4,updated_at={TIMESTAMP_SQL} WHERE plan_id=?1 AND step_id=?2"),params![id,step,reason,rev])?;
        tx.execute(
            &format!("UPDATE plans SET revision=?2,updated_at={TIMESTAMP_SQL} WHERE id=?1"),
            params![id, rev],
        )?;
        tx.execute("INSERT INTO plan_history(plan_id,revision,action,reason,step_id)VALUES(?1,?2,'blocked',?3,?4)",params![id,rev,reason,step])?;
        record_operation(&tx, "plan_block", id, &json!({"step_id":step}))?;
        let p = load_plan(&tx, id, &scope)?;
        tx.commit()?;
        Ok(json!({"action":"updated","plan":p}))
    }
    pub fn plan_revise(
        &mut self,
        id: &str,
        expected: i64,
        reason: &str,
        mut input: PlanReviseInput,
    ) -> Result<Value> {
        let reason = normalize_todo_text("reason", reason)?;
        if input.steps.is_empty() || input.steps.len() > 100 {
            return Err(AppError::new(
                "invalid_input",
                "revision steps must contain 1 to 100 items",
            ));
        }
        let focal = input.focal.unwrap_or(0);
        if focal >= input.steps.len() {
            return Err(AppError::new(
                "invalid_input",
                "focal index is out of range",
            ));
        }
        for step in &mut input.steps {
            step.title = normalize_todo_text("step title", &step.title)?;
            if let Some(verify) = step.verify.as_mut() {
                *verify = normalize_todo_text("verify", verify)?;
            }
        }
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = load_plan(&tx, id, &scope)?;
        require_active(&p)?;
        let current = p["revision"].as_i64().unwrap();
        if current != expected {
            return Err(plan_revision_error(expected, current));
        }
        let rev = current + 1;
        tx.execute(&format!("UPDATE plan_steps SET status='skipped',blocker=NULL,updated_revision=?2,updated_at={TIMESTAMP_SQL} WHERE plan_id=?1 AND status IN ('pending','in_progress','blocked')"),params![id,rev])?;
        let max: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal),-1) FROM plan_steps WHERE plan_id=?1",
            [id],
            |r| r.get(0),
        )?;
        for (i, s) in input.steps.iter().enumerate() {
            let sid: String =
                tx.query_row("SELECT LOWER(HEX(RANDOMBLOB(16)))", [], |r| r.get(0))?;
            tx.execute(&format!("INSERT INTO plan_steps(plan_id,step_id,ordinal,title,status,verify,created_revision,updated_revision,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,?6,?7,?7,{TIMESTAMP_SQL},{TIMESTAMP_SQL})"),params![id,sid,max+1+i as i64,s.title,if i==focal{"in_progress"}else{"pending"},s.verify,rev])?;
        }
        tx.execute(
            &format!("UPDATE plans SET revision=?2,updated_at={TIMESTAMP_SQL} WHERE id=?1"),
            params![id, rev],
        )?;
        tx.execute(
            "INSERT INTO plan_history(plan_id,revision,action,reason)VALUES(?1,?2,'revised',?3)",
            params![id, rev, reason],
        )?;
        record_operation(&tx, "plan_revise", id, &json!({"reason":reason}))?;
        let p = load_plan(&tx, id, &scope)?;
        index_plan(&tx, id, &p)?;
        tx.commit()?;
        Ok(json!({"action":"updated","plan":p}))
    }
    #[allow(clippy::too_many_arguments, reason = "explicit completion gate boundary")]
    pub fn plan_finish(
        &mut self,
        id: &str,
        expected: i64,
        complete: bool,
        result: Option<&str>,
        evidence: Option<&str>,
        checked: bool,
        reason: Option<&str>,
    ) -> Result<Value> {
        let scope = self.scope.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = load_plan(&tx, id, &scope)?;
        require_active(&p)?;
        let current = p["revision"].as_i64().unwrap();
        if current != expected {
            return Err(plan_revision_error(expected, current));
        }
        let (state, action);
        if complete {
            let result = normalize_todo_text("result", result.unwrap_or(""))?;
            let evidence = normalize_todo_text("evidence", evidence.unwrap_or(""))?;
            if !checked
                || p["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| !matches!(s["status"].as_str(), Some("completed" | "skipped")))
            {
                return Err(AppError::new(
                    "plan_completion_incomplete",
                    "all steps must be terminal and done-when must be checked",
                ));
            }
            state = "completed";
            action = "completed";
            tx.execute(&format!("UPDATE plans SET state=?2,result=?3,completion_evidence=?4,done_when_checked=1,revision=revision+1,updated_at={TIMESTAMP_SQL},closed_at={TIMESTAMP_SQL} WHERE id=?1"),params![id,state,result,evidence])?;
        } else {
            let reason = normalize_todo_text("reason", reason.unwrap_or(""))?;
            state = "abandoned";
            action = "abandoned";
            tx.execute(&format!("UPDATE plans SET state=?2,abandoned_reason=?3,revision=revision+1,updated_at={TIMESTAMP_SQL},closed_at={TIMESTAMP_SQL} WHERE id=?1"),params![id,state,reason])?;
        }
        tx.execute(
            "INSERT INTO plan_history(plan_id,revision,action,reason,result)VALUES(?1,?2,?3,?4,?5)",
            params![id, current + 1, action, reason, result],
        )?;
        record_operation(&tx, &format!("plan_{action}"), id, &json!({}))?;
        let p = load_plan(&tx, id, &scope)?;
        tx.commit()?;
        Ok(json!({"action":"updated","plan":p}))
    }
}
