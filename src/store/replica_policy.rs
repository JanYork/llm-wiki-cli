// Connection-local triggers cover normal Store writes, including cascades and imports.
// Direct file edits remain untrusted: the server independently checks normalized deltas.
fn install_replica_policy_guards(conn:&Connection)->Result<()> {
    let raw:Option<String>=conn.query_row("SELECT value FROM meta WHERE key='replica_policy'",[],|r|r.get(0)).optional()?;
    let role:Option<String>=conn.query_row("SELECT value FROM meta WHERE key='replica_role'",[],|r|r.get(0)).optional()?;
    if raw.is_none() && role.is_none(){return Ok(());}
    let policy=raw.map(|raw|serde_json::from_str::<crate::team::lease::SignedPolicy>(&raw).map_err(|_|AppError::new("invalid_policy_signature","invalid cached policy"))?.verify()).transpose()?;
    let server:Option<String>=conn.query_row("SELECT value FROM meta WHERE key='replica_server'",[],|r|r.get(0)).optional()?;
    let credential_hash=server.as_deref().and_then(|server|crate::replica::active_credential_hash(server).ok());
    conn.create_scalar_function("lwc_policy_permits",4,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move|ctx| {
        let kind:String=ctx.get(0)?;let key:String=ctx.get(1)?;let action:String=ctx.get(2)?;let revision:i64=ctx.get(3)?;
        let Some(policy)=policy.as_ref() else{return Ok(false);};
        let now=crate::team::lease::now().unwrap_or(u64::MAX);
        if credential_hash.as_deref()!=Some(policy.credential_hash.as_str()) || policy.role=="viewer" || revision!=policy.revision || now>=policy.expires_at || now.saturating_add(60)<policy.issued_at {return Ok(false);}
        if action!="create" && matches!(kind.as_str(),"source"|"memory"|"source_revision") && policy.denials.iter().any(|rule|rule.action=="compact" && (rule.kind=="*" || rule.kind==kind) && (key.is_empty() || rule.key=="*" || rule.key==key)) {return Ok(false);}
        Ok(!policy.denials.iter().any(|rule|(rule.kind=="*" || rule.kind==kind) && (key.is_empty() || rule.key=="*" || rule.key==key) && (rule.action=="*" || rule.action==action)))
    })?;
    let origin:String=conn.query_row("SELECT value FROM meta WHERE key='store_id'",[],|r|r.get(0))?;
    let source_origin=origin.clone();
    conn.create_scalar_function("lwc_revision_key",4,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move|ctx|{
        let path:String=ctx.get(0)?;let revision:i64=ctx.get(1)?;let source:Option<String>=ctx.get(2)?;let observed:String=ctx.get(3)?;
        let Some(source)=source else{return Ok(String::new());};
        Ok(hash_content(&canonical_sync_value(&json!({"origin_store":source_origin,"path_key":hash_content(&path),"revision":revision,"source_hash":source,"observed_at":observed}))))
    })?;
    conn.create_scalar_function("lwc_core_audit",1,rusqlite::functions::FunctionFlags::SQLITE_UTF8,|ctx|{let action:String=ctx.get(0)?;Ok(is_core_memory_action(&action))})?;
    conn.create_scalar_function("lwc_audit_key",5,rusqlite::functions::FunctionFlags::SQLITE_UTF8,move|ctx|{
        let id:i64=ctx.get(0)?;let action:String=ctx.get(1)?;let target:String=ctx.get(2)?;let raw:String=ctx.get(3)?;let at:String=ctx.get(4)?;
        let detail:Value=serde_json::from_str(&raw).map_err(|e|rusqlite::Error::UserFunctionError(Box::new(e)))?;
        let mut value=json!({"origin_store":origin,"operation_id":id,"action":action,"target":target,"detail":detail,"created_at":at});portable_history_value(&mut value);
        Ok(hash_content(&canonical_sync_value(&value)))
    })?;
    conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS lwc_policy_context(remote_apply INTEGER NOT NULL); INSERT INTO lwc_policy_context SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM lwc_policy_context); CREATE TEMP TABLE IF NOT EXISTS lwc_policy_created(kind TEXT,key TEXT,PRIMARY KEY(kind,key));")?;
    // Only trusted static table/column expressions are interpolated. Policy data stays parameters/UDF data.
    let mappings=[
        ("pages","page","{r}.slug",true),("page_sources","page","{r}.page_slug",false),("page_provenance","page","{r}.page_slug",false),("page_tags","page","{r}.page_slug",false),
        ("tags","tag","{r}.name",true),("page_tags","tag","{r}.tag_name",false),
        ("sources","source","{r}.content_hash",true),
        ("source_path_revisions","source_revision","lwc_revision_key({r}.tracked_path,{r}.revision,(SELECT content_hash FROM sources WHERE id={r}.source_id),{r}.observed_at)",true),
        ("ingest_jobs","ingest","(SELECT content_hash FROM sources WHERE id={r}.source_id)",true),
        ("semantic_relations","semantic_relation","{r}.id",true),
        ("memory_events","memory","{r}.id",true),("memory_fragments","memory","{r}.event_id",false),("memory_changes","memory","{r}.event_id",false),("memory_evidence","memory","{r}.event_id",false),("memory_feedback","memory","{r}.event_id",false),
        ("memory_relations","memory","{r}.event_id",false),("memory_relations","memory","{r}.target_event_id",false),
        ("todo_items","todo","{r}.id",true),("todo_tags","todo","{r}.todo_id",false),
        ("plans","plan","{r}.id",true),("plan_tags","plan","{r}.plan_id",false),("plan_steps","plan","{r}.plan_id",false),("plan_constraints","plan","{r}.plan_id",false),("plan_history","plan","{r}.plan_id",false),
        ("discussions","discussion","{r}.id",true),("discussion_revisions","discussion","{r}.discussion_id",false),
        ("meta","meta","{r}.key",true),
        ("retrieval_weights","retrieval_weight","{r}.target_type || char(0) || CASE WHEN {r}.target_type='source' THEN (SELECT content_hash FROM sources WHERE id=CAST({r}.target_identifier AS INTEGER)) ELSE {r}.target_identifier END || char(0) || {r}.provenance",true),
        ("retrieval_feedback","retrieval_feedback","{r}.query_fingerprint || char(0) || {r}.target_type || char(0) || CASE WHEN {r}.target_type='source' THEN (SELECT content_hash FROM sources WHERE id=CAST({r}.target_identifier AS INTEGER)) ELSE {r}.target_identifier END || char(0) || {r}.provenance",true),
    ];
    for (index,(table,kind,key,primary)) in mappings.iter().enumerate() {
        for (sql_action,action,row) in [("INSERT","create","NEW"),("UPDATE","update","NEW"),("DELETE","delete","OLD")] {
            let key=key.replace("{r}",row);
            let action=if *primary {format!("'{action}'")}else{format!("CASE WHEN EXISTS(SELECT 1 FROM lwc_policy_created WHERE kind='{kind}' AND key={key}) THEN 'create' ELSE 'update' END")};
            let filter=if *table=="meta" {format!("AND {row}.key IN ('schema','purpose')")}else{String::new()};
            conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS lwc_guard_{index}_{sql_action} BEFORE {sql_action} ON main.{table} WHEN (SELECT remote_apply FROM lwc_policy_context)=0 {filter} BEGIN SELECT CASE WHEN NOT lwc_policy_permits('{kind}',COALESCE({key},''),{action},CASE WHEN EXISTS(SELECT 1 FROM meta WHERE key='replica_policy_blocked') THEN -1 ELSE COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='replica_policy_revision'),0) END) THEN RAISE(ABORT,'replica_permission_denied') END; END;"))?;
            if sql_action=="UPDATE" {
                let old_key=key.replace("NEW.","OLD.");
                let old_filter=filter.replace("NEW.","OLD.");
                conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS lwc_guard_old_{index} BEFORE UPDATE ON main.{table} WHEN (SELECT remote_apply FROM lwc_policy_context)=0 {old_filter} BEGIN SELECT CASE WHEN NOT lwc_policy_permits('{kind}',COALESCE({old_key},''),'update',CASE WHEN EXISTS(SELECT 1 FROM meta WHERE key='replica_policy_blocked') THEN -1 ELSE COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='replica_policy_revision'),0) END) THEN RAISE(ABORT,'replica_permission_denied') END; END;"))?;
            }
            if *primary && sql_action=="INSERT" {
                let after_filter=filter.replace("AND ","WHEN ");
                conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS lwc_created_{index} AFTER INSERT ON main.{table} {after_filter} BEGIN INSERT OR IGNORE INTO lwc_policy_created VALUES('{kind}',{key}); END;"))?;
            }
        }
    }
    for (event,action,row) in [("INSERT","create","NEW"),("UPDATE","update","NEW"),("UPDATE","update","OLD"),("DELETE","delete","OLD")] {
        for (table,kind,key,filter) in [
            ("operations","'memory_audit'".to_owned(),format!("lwc_audit_key({row}.id,{row}.action,{row}.target,{row}.detail_json,{row}.created_at)"),format!("AND lwc_core_audit({row}.action)")),
            ("operations","'work_audit'".to_owned(),format!("{row}.target"),format!("AND {row}.action='sync_work_audit'")),
            ("replica_history",format!("{row}.kind"),format!("{row}.logical_key"),String::new()),
        ] {
            let name=if kind.contains("memory_audit"){"audit"}else if kind.contains("work_audit"){"work"}else{"inherited"};
            conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS lwc_history_{name}_{event}_{row} AFTER {event} ON main.{table} WHEN (SELECT remote_apply FROM lwc_policy_context)=0 {filter} BEGIN SELECT CASE WHEN NOT lwc_policy_permits({kind},{key},'{action}',CASE WHEN EXISTS(SELECT 1 FROM meta WHERE key='replica_policy_blocked') THEN -1 ELSE COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='replica_policy_revision'),0) END) THEN RAISE(ABORT,'replica_permission_denied') END; END;"))?;
        }
    }
    for event in ["INSERT","UPDATE"] {
        conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS lwc_guard_supersedes_{event} BEFORE {event} ON main.memory_relations WHEN NEW.relation_type='supersedes' AND (SELECT remote_apply FROM lwc_policy_context)=0 BEGIN SELECT CASE WHEN NOT lwc_policy_permits('memory',NEW.target_event_id,'compact',CASE WHEN EXISTS(SELECT 1 FROM meta WHERE key='replica_policy_blocked') THEN -1 ELSE COALESCE((SELECT CAST(value AS INTEGER) FROM meta WHERE key='replica_policy_revision'),0) END) THEN RAISE(ABORT,'replica_permission_denied') END; END;"))?;
    }
    Ok(())
}
impl Store {
    pub(crate) fn suspend_replica_policy(&self)->Result<()> {
        self.conn.execute("INSERT INTO meta(key,value) VALUES('replica_policy_blocked','1') ON CONFLICT(key) DO UPDATE SET value=excluded.value",[])?;Ok(())
    }
    pub(crate) fn require_replica_action(&self,action:&str)->Result<()> {
        let raw:Option<String>=self.conn.query_row("SELECT value FROM meta WHERE key='replica_policy'",[],|r|r.get(0)).optional()?;
        let bound:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='replica_space')",[],|r|r.get(0))?;
        if !bound {return Ok(());}
        if self.conn.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='replica_policy_blocked')",[],|r|r.get::<_,bool>(0))? {return Err(AppError::new("replica_permission_denied","online authorization was withdrawn; local work is preserved"));}
        let denied=||AppError::new("replica_permission_denied","this replica policy does not permit the operation");
        let signed:crate::team::lease::SignedPolicy=serde_json::from_str(&raw.ok_or_else(denied)?).map_err(|_|denied())?;
        let policy=signed.verify()?;
        let server:String=self.conn.query_row("SELECT value FROM meta WHERE key='replica_server'",[],|r|r.get(0))?;
        let now=crate::team::lease::now()?;
        if crate::replica::active_credential_hash(&server)?!=policy.credential_hash || now>=policy.expires_at || now.saturating_add(60)<policy.issued_at
            || (action!="export" && policy.role=="viewer") || policy.denials.iter().any(|rule|rule.action==action || rule.action=="*") {return Err(denied());}
        Ok(())
    }

    pub(crate) fn set_replica_policy(&mut self, envelope:&Value,user:&str,server:&str)->Result<()> {
        self.install_signed_replica_policy(envelope,user,server,false)
    }
    pub(crate) fn recover_replica_policy(&mut self,envelope:&Value,user:&str,server:&str)->Result<()> {
        self.install_signed_replica_policy(envelope,user,server,true)
    }
    fn install_signed_replica_policy(&mut self,envelope:&Value,user:&str,server:&str,recovery:bool)->Result<()> {
        let signed:crate::team::lease::SignedPolicy=serde_json::from_value(envelope.clone()).map_err(|_|AppError::new("invalid_policy_signature","missing signed policy"))?;
        let policy=signed.verify()?;
        let space:String=self.conn.query_row("SELECT value FROM meta WHERE key='replica_space'",[],|r|r.get(0))?;
        let epoch:String=self.conn.query_row("SELECT value FROM meta WHERE key='team_epoch'",[],|r|r.get(0))?;
        let now=crate::team::lease::now()?;
        if policy.user_id!=user || policy.space_id!=space || (!recovery && policy.epoch!=epoch) || policy.expires_at<=now || now.saturating_add(60)<policy.issued_at {return Err(AppError::new("invalid_policy_subject","policy does not authorize this replica"));}
        let pinned:Option<String>=self.conn.query_row("SELECT value FROM meta WHERE key='replica_policy_key'",[],|r|r.get(0)).optional()?;
        if (recovery && pinned.is_none()) || pinned.as_ref().is_some_and(|key|key!=&signed.public_key){return Err(AppError::new("policy_key_changed","server policy key changed; preserve local work"));}
        let previous:Option<i64>=self.conn.query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key='replica_policy_revision'",[],|r|r.get(0)).optional()?;
        if previous.is_some_and(|revision|revision>policy.revision){return Err(AppError::new("policy_regressed","server policy revision regressed"));}
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if recovery {tx.execute("UPDATE meta SET value=?1 WHERE key='team_epoch'",[&policy.epoch])?;}
        for (key,value) in [("replica_server",server.to_owned()),("replica_policy",serde_json::to_string(&signed).map_err(|_|AppError::new("invalid_policy_signature","cannot encode policy"))?),("replica_policy_key",signed.public_key),("replica_policy_revision",policy.revision.to_string()),("replica_role",policy.role)] {
            tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
        }
        tx.execute("DELETE FROM meta WHERE key='replica_policy_blocked'",[])?;
        tx.commit()?;
        install_replica_policy_guards(&self.conn)
    }
    pub(crate) fn publish_replica_state(&mut self,normalized:&Path,expected:&StoreIdentity,session:&str)->Result<SyncPublishSummary> {
        // Called only by the authenticated replica engine after the server policy/head checks.
        let guarded:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE name='lwc_policy_context')",[],|r|r.get(0))?;
        if guarded {self.conn.execute("UPDATE temp.lwc_policy_context SET remote_apply=1",[])?;}
        let result=self.publish_sync_state(normalized,expected,session);
        if guarded {self.conn.execute("UPDATE temp.lwc_policy_context SET remote_apply=0",[])?;}
        result
    }
}
