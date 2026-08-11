fn normalize_tag_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 128 || value.chars().any(char::is_control) {
        return Err(AppError::new(
            "invalid_tag",
            "tag must be 1..128 characters without control characters",
        ));
    }
    Ok(value.to_string())
}

fn normalize_tag_reason(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 512 || value.chars().any(char::is_control) {
        return Err(AppError::new(
            "invalid_tag_reason",
            "tag reason must be 1..512 characters without control characters",
        ));
    }
    Ok(value.to_string())
}

fn load_tag_snapshot(conn: &Connection, schema: &str, tag: &str) -> Result<SparseTagSnapshot> {
    if !matches!(schema, "main" | "live_base" | "candidate") {
        return Err(AppError::new(
            "changeset_corrupt",
            "unsupported tag snapshot schema",
        ));
    }
    let policy = conn
        .query_row(
            &format!(
                "SELECT name, autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason, updated_at
                 FROM {schema}.tags WHERE name = ?1"
            ),
            [tag],
            |row| {
                Ok(TagPolicySnapshot {
                    name: row.get(0)?,
                    autoload: row.get(1)?,
                    autoload_priority: row.get(2)?,
                    autoload_limit: row.get(3)?,
                    autoload_max_chars: row.get(4)?,
                    reason: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()?;
    let memberships = if policy.is_some() {
        let mut statement = conn.prepare(&format!(
            "SELECT tag_name, page_slug, priority, reason, created_at, updated_at
             FROM {schema}.page_tags WHERE tag_name = ?1 ORDER BY page_slug"
        ))?;
        statement
            .query_map([tag], |row| {
                Ok(PageTagSnapshot {
                    tag_name: row.get(0)?,
                    page_slug: row.get(1)?,
                    priority: row.get(2)?,
                    reason: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Ok(SparseTagSnapshot {
        policy,
        memberships,
    })
}

fn tag_snapshot_fingerprint(snapshot: &SparseTagSnapshot) -> String {
    hash_content(&serde_json::to_string(snapshot).unwrap_or_default())
}

fn restore_tag_snapshot(tx: &Transaction<'_>, snapshot: &SparseTagSnapshot) -> Result<()> {
    let Some(policy) = snapshot.policy.as_ref() else {
        return Ok(());
    };
    tx.execute(
        "INSERT INTO tags(
            name, autoload, autoload_priority, autoload_limit,
            autoload_max_chars, reason, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &policy.name,
            policy.autoload,
            policy.autoload_priority,
            policy.autoload_limit,
            policy.autoload_max_chars,
            &policy.reason,
            &policy.updated_at,
        ],
    )?;
    for member in &snapshot.memberships {
        tx.execute(
            "INSERT INTO page_tags(
                tag_name, page_slug, priority, reason, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &member.tag_name,
                &member.page_slug,
                member.priority,
                &member.reason,
                &member.created_at,
                &member.updated_at,
            ],
        )?;
    }
    Ok(())
}

impl Store {
    pub fn begin_read_snapshot(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN DEFERRED")?;
        Ok(())
    }

    pub fn begin_hook_snapshot(&self) -> Result<()> {
        self.conn.busy_timeout(std::time::Duration::from_millis(250))?;
        self.begin_read_snapshot()
    }

    pub fn tag_autoload_policies(&self) -> Result<(Vec<TagAutoloadPolicy>, bool)> {
        // ponytail: 100 policies cap hook work; raise only with measured demand and budget evidence.
        const LIMIT: i64 = 101;
        let mut statement = self.conn.prepare(
            "SELECT name, autoload_priority, autoload_limit, autoload_max_chars, reason
             FROM tags
             WHERE autoload = 1
             ORDER BY autoload_priority DESC, name ASC
             LIMIT ?1",
        )?;
        let mut policies = statement
            .query_map([LIMIT], |row| {
                Ok(TagAutoloadPolicy {
                    scope: self.scope.clone(),
                    name: row.get(0)?,
                    priority: row.get(1)?,
                    limit: row.get::<_, i64>(2)? as usize,
                    max_chars: row.get::<_, i64>(3)? as usize,
                    reason: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = policies.len() == LIMIT as usize;
        policies.truncate((LIMIT - 1) as usize);
        Ok((policies, has_more))
    }

    pub fn tag_fingerprint(&self, tag: &str) -> Result<String> {
        Ok(tag_snapshot_fingerprint(&load_tag_snapshot(
            &self.conn, "main", tag,
        )?))
    }

    pub fn tag_first_page(&self, tag: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT page_slug FROM page_tags WHERE tag_name = ?1 ORDER BY page_slug LIMIT 1",
                [tag],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn tag_page_identities(&self, tag: &str, limit: usize) -> Result<Vec<TagPageIdentity>> {
        let tag = normalize_tag_name(tag)?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tags WHERE name = ?1)",
            [&tag],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppError::new(
                "tag_not_found",
                format!("tag {tag} does not exist"),
            ));
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut statement = self.conn.prepare(
            "SELECT page_slug, priority, reason
             FROM page_tags INDEXED BY page_tags_lookup
             WHERE tag_name = ?1
             ORDER BY priority DESC, page_slug ASC
             LIMIT ?2",
        )?;
        Ok(statement
            .query_map(params![&tag, limit], |row| {
                Ok(TagPageIdentity {
                    scope: self.scope.clone(),
                    tag: tag.clone(),
                    page_slug: row.get(0)?,
                    priority: row.get(1)?,
                    reason: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tagged_page(&self, identity: TagPageIdentity, ordinal: usize) -> Result<TaggedPage> {
        Ok(TaggedPage {
            scope: identity.scope,
            tag: identity.tag,
            priority: identity.priority,
            reason: identity.reason,
            ordinal,
            page: self.load_page(&identity.page_slug)?,
        })
    }

    pub fn tag_list(&self) -> Result<Value> {
        let mut statement = self.conn.prepare(
            "SELECT t.name, t.autoload, t.autoload_priority, t.autoload_limit,
                    t.autoload_max_chars, t.reason, t.updated_at, COUNT(pt.page_slug)
             FROM tags t LEFT JOIN page_tags pt ON pt.tag_name = t.name
             GROUP BY t.name
             ORDER BY t.autoload DESC, t.autoload_priority DESC, t.name",
        )?;
        let tags = statement
            .query_map([], |row| {
                Ok(json!({
                    "name": row.get::<_, String>(0)?,
                    "autoload": row.get::<_, bool>(1)?,
                    "autoload_priority": row.get::<_, i32>(2)?,
                    "autoload_limit": row.get::<_, i64>(3)?,
                    "autoload_max_chars": row.get::<_, i64>(4)?,
                    "reason": row.get::<_, String>(5)?,
                    "updated_at": row.get::<_, String>(6)?,
                    "pages": row.get::<_, i64>(7)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database_string(),
            "tags": tags,
        }))
    }

    pub fn tag_set(
        &mut self,
        tag: &str,
        page: &str,
        priority: i32,
        reason: &str,
    ) -> Result<Value> {
        let tag = normalize_tag_name(tag)?;
        let reason = normalize_tag_reason(reason)?;
        validate_page_slug(page)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let page_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages WHERE slug = ?1)",
            [page],
            |row| row.get(0),
        )?;
        if !page_exists {
            return Err(AppError::new(
                "page_not_found",
                format!("page {page} does not exist"),
            ));
        }
        let existing = tx
            .query_row(
                "SELECT priority, reason FROM page_tags
                 WHERE tag_name = ?1 AND page_slug = ?2",
                params![&tag, page],
                |row| Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if existing.as_ref() == Some(&(priority, reason.clone())) {
            tx.commit()?;
            return Ok(json!({
                "scope": self.scope,
                "database": self.database_string(),
                "tag": tag,
                "page": page,
                "priority": priority,
                "reason": reason,
                "action": "unchanged",
            }));
        }
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO tags(name, reason, updated_at)
                 VALUES (?1, ?2, {TIMESTAMP_SQL})"
            ),
            params![&tag, &reason],
        )?;
        tx.execute(
            &format!(
                "INSERT INTO page_tags(
                    tag_name, page_slug, priority, reason, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, {TIMESTAMP_SQL}, {TIMESTAMP_SQL})
                 ON CONFLICT(tag_name, page_slug) DO UPDATE SET
                    priority = excluded.priority,
                    reason = excluded.reason,
                    updated_at = excluded.updated_at"
            ),
            params![&tag, page, priority, &reason],
        )?;
        let action = if existing.is_some() { "updated" } else { "created" };
        record_operation(
            &tx,
            "tag_set",
            &format!("{tag}/{page}"),
            &json!({"tag": tag, "page": page, "priority": priority, "reason": reason}),
        )?;
        tx.commit()?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database_string(),
            "tag": tag,
            "page": page,
            "priority": priority,
            "reason": reason,
            "action": action,
        }))
    }

    pub fn tag_remove(&mut self, tag: &str, page: &str) -> Result<Value> {
        let tag = normalize_tag_name(tag)?;
        validate_page_slug(page)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute(
            "DELETE FROM page_tags WHERE tag_name = ?1 AND page_slug = ?2",
            params![&tag, page],
        )?;
        if removed > 0 {
            record_operation(
                &tx,
                "tag_remove",
                &format!("{tag}/{page}"),
                &json!({"tag": tag, "page": page}),
            )?;
        }
        tx.commit()?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database_string(),
            "tag": tag,
            "page": page,
            "action": if removed > 0 { "removed" } else { "unchanged" },
        }))
    }

    pub fn tag_delete(&mut self, tag: &str) -> Result<Value> {
        let tag = normalize_tag_name(tag)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = tx.execute("DELETE FROM tags WHERE name = ?1", [&tag])?;
        if removed > 0 {
            record_operation(&tx, "tag_delete", &tag, &json!({"tag": tag}))?;
        }
        tx.commit()?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database_string(),
            "tag": tag,
            "action": if removed > 0 { "deleted" } else { "unchanged" },
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tag_autoload(
        &mut self,
        tag: &str,
        enabled: bool,
        priority: i32,
        limit: usize,
        max_chars: usize,
        reason: &str,
    ) -> Result<Value> {
        let tag = normalize_tag_name(tag)?;
        let reason = normalize_tag_reason(reason)?;
        if !(1..=100).contains(&limit) || !(1..=100_000).contains(&max_chars) {
            return Err(AppError::new(
                "invalid_tag_policy",
                "tag autoload limit must be 1..100 and max_chars must be 1..100000",
            ));
        }
        let limit = i64::try_from(limit).expect("validated tag limit fits i64");
        let max_chars = i64::try_from(max_chars).expect("validated tag max_chars fits i64");
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = tx
            .query_row(
                "SELECT autoload, autoload_priority, autoload_limit,
                        autoload_max_chars, reason
                 FROM tags WHERE name = ?1",
                [&tag],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, i32>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::new("tag_not_found", format!("tag {tag} does not exist")))?;
        if enabled {
            let members: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM page_tags WHERE tag_name = ?1)",
                [&tag],
                |row| row.get(0),
            )?;
            if !members {
                return Err(AppError::new(
                    "tag_empty",
                    "an empty tag cannot be enabled for autoload",
                ));
            }
        }
        let desired = (enabled, priority, limit, max_chars, reason.clone());
        let action = if existing == desired {
            "unchanged"
        } else {
            tx.execute(
                &format!(
                    "UPDATE tags SET
                        autoload = ?2, autoload_priority = ?3, autoload_limit = ?4,
                        autoload_max_chars = ?5, reason = ?6, updated_at = {TIMESTAMP_SQL}
                     WHERE name = ?1"
                ),
                params![&tag, enabled, priority, limit, max_chars, &reason],
            )?;
            record_operation(
                &tx,
                "tag_autoload",
                &tag,
                &json!({
                    "tag": tag, "autoload": enabled, "priority": priority,
                    "limit": limit, "max_chars": max_chars, "reason": reason,
                }),
            )?;
            "updated"
        };
        tx.commit()?;
        Ok(json!({
            "scope": self.scope,
            "database": self.database_string(),
            "tag": tag,
            "action": action,
            "policy": {
                "autoload": enabled,
                "priority": priority,
                "limit": limit,
                "max_chars": max_chars,
                "reason": reason,
            },
        }))
    }

    #[cfg(test)]
    fn tag_membership_count(&self, tag: &str, page: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM page_tags WHERE tag_name = ?1 AND page_slug = ?2",
            params![tag, page],
            |row| row.get(0),
        )?)
    }
}
