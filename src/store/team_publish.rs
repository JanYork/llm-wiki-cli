#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TeamCommit {
    pub(crate) epoch: String,
    pub(crate) expected_head: u64,
    pub(crate) actor: String,
    pub(crate) principal:Value,
    #[serde(default)] pub(crate) recovery:Option<Value>,
    pub(crate) replica_id: String,
    pub(crate) batch_id: String,
    pub(crate) artifact_id: String,
    pub(crate) payload_digest: String,
}

impl Store {
    pub(crate) fn restore_team_epoch(&mut self, epoch:&str)->Result<()> {
        if !is_lower_sync_hex(epoch,64) {return Err(AppError::new("invalid_epoch","invalid recovery epoch"));}
        let integrity:String=self.conn.query_row("PRAGMA integrity_check",[],|r|r.get(0))?;
        if integrity!="ok" {return Err(AppError::new("invalid_backup","memory database integrity check failed"));}
        self.conn.execute("UPDATE meta SET value=?1 WHERE key='team_epoch'",[epoch])?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_replica_role(&mut self, role: &str) -> Result<()> {
        if !matches!(role, "viewer" | "editor" | "manager") {
            return Err(AppError::new("invalid_role", "unknown local replica role"));
        }
        if !self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='replica_space')",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(AppError::new(
                "replica_not_bound",
                "local replica is not bound",
            ));
        }
        self.conn.execute("INSERT INTO meta(key,value) VALUES('replica_role',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[role])?;
        Ok(())
    }
    pub(crate) fn initialize_team_artifact(&mut self, artifact: &str, digest: &str) -> Result<()> {
        if !is_lower_sync_hex(artifact, 64) || !is_lower_sync_hex(digest, 64) {
            return Err(AppError::new(
                "invalid_team_commit",
                "invalid genesis artifact",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head = read_team_head(&tx)?;
        if head["head"] != 0 || !head["artifact_id"].is_null() {
            return Err(AppError::new(
                "head_changed",
                "space genesis was already initialized",
            ));
        }
        for (key, value) in [("team_artifact", artifact), ("team_digest", digest)] {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn bind_team_space(&mut self, space: &str, epoch: &str) -> Result<()> {
        if !is_lower_sync_hex(space, 64) || !is_lower_sync_hex(epoch, 64) {
            return Err(AppError::new(
                "invalid_replica_identity",
                "space and epoch must be canonical IDs",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (key, value) in [("replica_space", space), ("team_epoch", epoch)] {
            let previous: Option<String> = tx
                .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
                .optional()?;
            if previous
                .as_deref()
                .is_some_and(|previous| previous != value)
            {
                return Err(AppError::new(
                    "replica_identity_conflict",
                    "store is already bound to another space or epoch",
                ));
            }
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",
                params![key, value],
            )?;
        }
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('team_head','0') ON CONFLICT(key) DO NOTHING",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn team_head(&self) -> Result<Value> {
        read_team_head(&self.conn)
    }

    pub(crate) fn team_receipt(&self, replica: &str, batch: &str) -> Result<Option<Value>> {
        let key = format!("team:{replica}:{batch}");
        let raw:Option<String>=self.conn.query_row("SELECT detail_json FROM operations WHERE action='sync_merge' AND target=?1 ORDER BY id DESC LIMIT 1",[key],|r|r.get(0)).optional()?;
        raw.map(|raw| {
            serde_json::from_str(&raw).map_err(|_| {
                AppError::new("sync_receipt_invalid", "invalid team publication receipt")
            })
        })
        .transpose()
    }

    pub(crate) fn team_history(&self,limit:usize,offset:usize)->Result<Value> {
        if limit==0 || limit>100 || offset>1_000_000 {return Err(AppError::new("invalid_limit","history limit must be 1..100"));}
        let mut statement=self.conn.prepare("SELECT detail_json,created_at FROM operations WHERE action='sync_merge' AND json_type(detail_json,'$.team')='object' ORDER BY id DESC LIMIT ?1 OFFSET ?2")?;
        let rows=statement.query_map(params![limit as i64,offset as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut history=Vec::new();
        for (raw,created_at) in rows {let detail:Value=serde_json::from_str(&raw).map_err(|_|AppError::new("sync_receipt_invalid","invalid history receipt"))?;let mut team=detail["team"].clone();if let Some(recovery)=team["recovery"].as_object_mut()&& let Some(images)=recovery.remove("rejected_images"){recovery.insert("rejected_count".into(),json!(images.as_array().map_or(0,Vec::len)));recovery.insert("rejected_digest".into(),json!(hash_content(&images.to_string())));}team["created_at"]=json!(created_at);history.push(team);}
        Ok(json!({"history":history,"head":self.team_head()?,"limit":limit,"offset":offset}))
    }
    pub(crate) fn team_commit_at(&self,head:u64)->Result<Value> {
        let head=i64::try_from(head).map_err(|_|AppError::new("invalid_head","head exceeds supported range"))?;
        let raw:String=self.conn.query_row("SELECT detail_json FROM operations WHERE action='sync_merge' AND json_extract(detail_json,'$.team.accepted_head')=?1 ORDER BY id DESC LIMIT 1",[head],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("history_not_found","accepted head is unavailable"))?;
        let detail:Value=serde_json::from_str(&raw).map_err(|_|AppError::new("sync_receipt_invalid","invalid history receipt"))?;
        Ok(detail["team"].clone())
    }
    pub(crate) fn reject_revoked_images(&self,normalized:&Path)->Result<()> {
        // ponytail: scan retained recovery receipts; add an indexed projection if measured
        // recovery volume makes this dominate the already serialized publication path.
        let mut statement=self.conn.prepare("SELECT detail_json FROM operations WHERE action='sync_merge' AND json_type(detail_json,'$.team.recovery.rejected_images')='array'")?;
        let candidate=Connection::open_with_flags(normalized,OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        for raw in statement.query_map([],|r|r.get::<_,String>(0))? {
            let detail:Value=serde_json::from_str(&raw?).map_err(|_|AppError::new("sync_receipt_invalid","invalid recovery receipt"))?;
            for image in detail["team"]["recovery"]["rejected_images"].as_array().into_iter().flatten() {
                let found:bool=candidate.query_row("SELECT EXISTS(SELECT 1 FROM sync_objects WHERE kind=?1 AND logical_key=?2 AND payload_hash=?3)",params![image["kind"].as_str(),image["key"].as_str(),image["hash"].as_str()],|r|r.get(0))?;
                if found{return Err(AppError::new("revoked_memory_version","a recovered memory version cannot be silently resurrected").with_details(json!({"kind":image["kind"],"key":image["key"],"hash":image["hash"],"recovery_head":detail["team"]["accepted_head"]})));}
            }
        }
        Ok(())
    }

    pub(crate) fn publish_team_state(
        &mut self,
        normalized: &Path,
        expected: &StoreIdentity,
        commit: &TeamCommit,
    ) -> Result<SyncPublishSummary> {
        self.publish_sync_state_inner(
            normalized,
            expected,
            &format!("team:{}:{}", commit.replica_id, commit.batch_id),
            Some(commit),
        )
    }
}

fn read_team_head(conn: &Connection) -> Result<Value> {
    let mut statement=conn.prepare("SELECT key,value FROM meta WHERE key IN('replica_space','team_epoch','team_head','team_digest','team_artifact')")?;
    let values = statement
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let head = values
        .get("team_head")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| AppError::new("replica_not_bound", "store has no valid team binding"))?;
    Ok(
        json!({"protocol":"lwc-team-sync/1","share_schema":1,"space_id":values.get("replica_space"),"server_epoch":values.get("team_epoch"),"head":head,"digest":values.get("team_digest"),"artifact_id":values.get("team_artifact")}),
    )
}

fn validate_team_commit(
    tx: &Transaction<'_>,
    commit: &TeamCommit,
    session: &str,
    digest: &str,
) -> Result<()> {
    for value in [
        &commit.epoch,
        &commit.actor,
        &commit.replica_id,
        &commit.batch_id,
        &commit.artifact_id,
        &commit.payload_digest,
    ] {
        if !is_lower_sync_hex(value, 64) {
            return Err(AppError::new(
                "invalid_team_commit",
                "invalid commit identity or digest",
            ));
        }
    }
    if digest != commit.payload_digest {
        return Err(AppError::new(
            "sync_checksum_mismatch",
            "submitted canonical state digest differs from the batch",
        ));
    }
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE action='sync_merge' AND target=?1)",
        [session],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(AppError::new(
            "batch_already_committed",
            "read the existing batch receipt before retrying",
        ));
    }
    let head = read_team_head(tx)?;
    if head["server_epoch"] != commit.epoch {
        return Err(AppError::new(
            "server_epoch_changed",
            "server epoch changed; preserve local work and reconcile",
        ));
    }
    if head["head"].as_u64() != Some(commit.expected_head) {
        return Err(AppError::new(
            "head_changed",
            "team head changed; pull and merge before retrying",
        ));
    }
    if commit.expected_head >= i64::MAX as u64 {
        return Err(AppError::new(
            "head_exhausted",
            "team head counter exhausted",
        ));
    }
    Ok(())
}

fn commit_team_head(tx: &Transaction<'_>, commit: &TeamCommit, digest: &str) -> Result<Value> {
    let parent=read_team_head(tx)?;
    let head = commit.expected_head + 1;
    for (key, value) in [
        ("team_head", head.to_string()),
        ("team_digest", digest.to_owned()),
        ("team_artifact", commit.artifact_id.clone()),
    ] {
        tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
    }
    Ok(
        json!({"server_epoch":commit.epoch,"accepted_head":head,"accepted_digest":digest,"actor":commit.actor,"principal":commit.principal,"replica_id":commit.replica_id,"batch_id":commit.batch_id,"artifact_id":commit.artifact_id,"committed":true,"parent":parent,"recovery":commit.recovery}),
    )
}
