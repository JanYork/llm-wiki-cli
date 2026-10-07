use crate::error::{AppError, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Denial {
    pub kind: String,
    pub key: String,
    pub action: String,
}
impl Denial {
    pub(crate) fn validate(&self) -> Result<()> {
        if !matches!(
            self.kind.as_str(),
            "*" | "meta"
                | "source"
                | "page"
                | "tag"
                | "ingest"
                | "semantic_relation"
                | "retrieval_weight"
                | "retrieval_feedback"
                | "memory"
                | "todo"
                | "plan"
                | "discussion"
                | "source_revision"
                | "memory_audit"
                | "work_audit"
                | "draft_intent"
        ) || self.kind.len() > 128
            || self.key.is_empty()
            || self.key.len() > 2048
            || self.kind.chars().any(char::is_control)
            || self.key.chars().any(|c| c.is_control() && c != '\0')
            || !matches!(
                self.action.as_str(),
                "create" | "update" | "delete" | "compact" | "rollback" | "export" | "*"
            )
        {
            return Err(AppError::new(
                "invalid_policy",
                "invalid resource/action restriction",
            ));
        }
        Ok(())
    }
}
pub(super) fn denials(conn: &Connection, user: &str, space: &str) -> Result<Vec<Denial>> {
    let mut stmt=conn.prepare("SELECT kind,logical_key,action FROM memory_denials WHERE space_id=?1 AND user_id=?2 ORDER BY kind,logical_key,action")?;
    Ok(stmt
        .query_map(params![space, user], |r| {
            Ok(Denial {
                kind: r.get(0)?,
                key: r.get(1)?,
                action: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
/// Check the actual normalized delta under the same lock as grant changes and publication.
/// Client action labels, mutation logs, and local policy files are not authorization evidence.
pub(super) fn authorize_delta(
    conn: &Connection,
    user: &str,
    space: &str,
    before: &Path,
    after: &Path,
) -> Result<()> {
    let rules = denials(conn, user, space)?;
    if rules.is_empty() {
        return Ok(());
    }
    let snapshot = Connection::open_with_flags(after, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    snapshot.execute(
        "ATTACH DATABASE ?1 AS previous",
        [before.to_string_lossy().as_ref()],
    )?;
    let mut stmt=snapshot.prepare("SELECT old.kind,old.logical_key,CASE WHEN new.logical_key IS NULL THEN 'delete' ELSE 'update' END FROM previous.sync_objects old LEFT JOIN main.sync_objects new USING(kind,logical_key) WHERE new.logical_key IS NULL OR old.payload_hash<>new.payload_hash UNION ALL SELECT new.kind,new.logical_key,'create' FROM main.sync_objects new LEFT JOIN previous.sync_objects old USING(kind,logical_key) WHERE old.logical_key IS NULL")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (kind, key, action) = row?;
        if rules.iter().any(|rule| {
            // Protected original sources/events cannot be rewritten or removed by a principal
            // without compaction rights. New summaries remain separate derived objects.
            if rule.action == "compact"
                && matches!(kind.as_str(), "source" | "memory" | "source_revision")
                && action != "create"
                && (rule.kind == "*" || rule.kind == kind)
                && (rule.key == "*" || rule.key == key)
            {
                return true;
            }

            (rule.kind == "*" || rule.kind == kind)
                && (rule.key == "*" || rule.key == key)
                && (rule.action == "*" || rule.action == action)
        }) {
            return Err(AppError::new(
                "forbidden",
                "shared memory delta violates a resource permission",
            )
            .with_details(
                json!({"kind":kind,"key":key,"action":action,"permission_scope":"resource"}),
            ));
        }
    }
    // A new event may supersede a protected original without changing that original's row.
    let mut supersedes=snapshot.prepare("SELECT json_extract(relation.value,'$.target') FROM main.sync_objects n JOIN json_each(n.payload_json,'$.relations') relation WHERE n.kind='memory' AND json_extract(relation.value,'$.type')='supersedes' AND NOT EXISTS(SELECT 1 FROM previous.sync_objects old JOIN json_each(old.payload_json,'$.relations') prior WHERE old.kind=n.kind AND old.logical_key=n.logical_key AND prior.value=relation.value)")?;
    for target in supersedes.query_map([], |r| r.get::<_, String>(0))? {
        let target = target?;
        if rules.iter().any(|rule| {
            matches!(rule.kind.as_str(), "*" | "memory")
                && (rule.key == "*" || rule.key == target)
                && matches!(rule.action.as_str(), "*" | "compact" | "update" | "delete")
        }) {
            return Err(AppError::new(
                "forbidden",
                "superseding a protected memory is not permitted",
            ).with_details(json!({"kind":"memory","key":target,"action":"update","permission_scope":"resource"})));
        }
    }
    Ok(())
}
