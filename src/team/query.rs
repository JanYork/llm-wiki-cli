//! Bounded remote reads share Store semantics and never register a replica.
use crate::{
    error::{AppError, Result},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CloudQuery {
    Search {
        query: String,
        limit: usize,
    },
    Get {
        slug: String,
    },
    List {
        limit: usize,
        offset: usize,
    },
    Objects {
        kind: String,
        limit: usize,
        offset: usize,
        head: Option<u64>,
        epoch: Option<String>,
    },
    Blob {
        hash: String,
        offset: usize,
        limit: usize,
        head: Option<u64>,
        epoch: Option<String>,
    },
    Object {
        kind: String,
        key: String,
        head: Option<u64>,
        epoch: Option<String>,
    },
}
impl CloudQuery {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Self::Objects { head, epoch, .. }
        | Self::Object { head, epoch, .. }
        | Self::Blob { head, epoch, .. } = self
            && (head.is_some() != epoch.is_some()
                || epoch
                    .as_ref()
                    .is_some_and(|e| e.is_empty() || e.len() > 128))
        {
            return Err(AppError::new(
                "invalid_query",
                "head and epoch must be supplied together",
            ));
        }
        let valid = match self {
            Self::Blob {
                hash,
                offset,
                limit,
                ..
            } => {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && *offset <= i64::MAX as usize - 65536
                    && (1..=65536).contains(limit)
            }
            Self::Search { query, limit } => {
                !query.trim().is_empty() && query.len() <= 2048 && (1..=100).contains(limit)
            }
            Self::Get { slug } => {
                !slug.is_empty() && slug.len() <= 512 && !slug.chars().any(char::is_control)
            }
            Self::Objects {
                kind,
                limit,
                offset,
                ..
            } => kind.len() <= 128 && (1..=100).contains(limit) && *offset <= 1_000_000,
            Self::Object { kind, key, .. } => {
                !kind.is_empty() && kind.len() <= 128 && !key.is_empty() && key.len() <= 2048
            }
            Self::List { limit, offset } => (1..=100).contains(limit) && *offset <= 1_000_000,
        };
        if !valid {
            return Err(AppError::new(
                "invalid_query",
                "invalid cloud query or pagination bounds",
            ));
        }
        Ok(())
    }
    pub(super) fn execute(&self, store: &Store, directory: &std::path::Path) -> Result<Value> {
        self.validate()?;
        // Return domain payloads only; do not expose the server database path.
        if matches!(self, Self::Search { .. }) {
            store.require_team_indexes()?;
        }
        let current = store.team_head()?;
        let data = match self {
            Self::Objects { head, epoch, .. }
            | Self::Object { head, epoch, .. }
            | Self::Blob { head, epoch, .. } => {
                if head.is_some_and(|expected| current["head"].as_u64() != Some(expected))
                    || epoch
                        .as_deref()
                        .is_some_and(|expected| current["server_epoch"].as_str() != Some(expected))
                {
                    return Err(AppError::new(
                        "head_changed",
                        "cloud head changed; restart the bounded read",
                    ));
                }
                let artifact = current["artifact_id"]
                    .as_str()
                    .ok_or_else(|| AppError::new("invalid_snapshot", "missing cloud snapshot"))?;
                if artifact.len() != 64
                    || !artifact
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(AppError::new(
                        "invalid_snapshot",
                        "invalid cloud snapshot ID",
                    ));
                }
                let conn = rusqlite::Connection::open_with_flags(
                    directory.join("snapshots").join(artifact),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?;
                match self {
                    Self::Blob {
                        hash,
                        offset,
                        limit,
                        ..
                    } => {
                        use base64::Engine;
                        use rusqlite::OptionalExtension;
                        let row:Option<(Vec<u8>,i64)>=conn.query_row("SELECT substr(content,?2,?3),length(content) FROM sync_blobs WHERE content_hash=?1",rusqlite::params![hash,*offset as i64+1,*limit as i64],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                        let (bytes, total) = row.ok_or_else(|| {
                            AppError::new("object_not_found", "core blob not found")
                        })?;
                        let next = offset + bytes.len();
                        json!({"hash":hash,"encoding":"base64","content":base64::engine::general_purpose::STANDARD.encode(bytes),"offset":offset,"next_offset":next,"total_bytes":total,"has_more":next<(total as usize)})
                    }
                    Self::Objects {
                        kind,
                        limit,
                        offset,
                        ..
                    } => {
                        let mut stmt=conn.prepare("SELECT kind,logical_key,payload_hash,substr(COALESCE(json_extract(payload_json,'$.title'),json_extract(payload_json,'$.name'),json_extract(payload_json,'$.summary'),logical_key),1,200) FROM sync_objects WHERE (?1='' OR kind=?1) ORDER BY kind,logical_key LIMIT ?2 OFFSET ?3")?;
                        let mut objects=stmt.query_map(rusqlite::params![kind,(limit+1) as i64,*offset as i64],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"key":r.get::<_,String>(1)?,"hash":r.get::<_,String>(2)?,"title":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
                        let has_more = objects.len() > *limit;
                        objects.truncate(*limit);
                        json!({"objects":objects,"offset":offset,"limit":limit,"has_more":has_more})
                    }
                    Self::Object { kind, key, .. } => {
                        use rusqlite::OptionalExtension;
                        let row:Option<(String,String)>=conn.query_row("SELECT payload_json,payload_hash FROM sync_objects WHERE kind=?1 AND logical_key=?2",rusqlite::params![kind,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                        let (raw, hash) = row.ok_or_else(|| {
                            AppError::new("object_not_found", "core memory object not found")
                        })?;
                        if raw.len() > 900 * 1024 {
                            return Err(AppError::new(
                                "query_result_too_large",
                                "object exceeds bounded cloud read size",
                            ));
                        }
                        if super::control::digest(&raw) != hash {
                            return Err(AppError::new(
                                "sync_checksum_mismatch",
                                "core object checksum mismatch",
                            ));
                        }
                        let payload: Value = serde_json::from_str(&raw).map_err(|_| {
                            AppError::new("invalid_snapshot", "invalid core memory payload")
                        })?;
                        json!({"kind":kind,"key":key,"hash":hash,"payload":payload})
                    }
                    _ => unreachable!(),
                }
            }
            Self::Search { query, limit } => json!(store.search_with_options(
                query,
                *limit,
                &crate::store::SearchOptions {
                    mode: crate::store::SearchMode::All,
                    granularity: crate::store::SearchGranularity::Document,
                    grouping: crate::store::SearchGrouping::None,
                    kinds: Vec::new(),
                    explain: false
                }
            )?),
            Self::Get { slug } => json!({"page":store.page_show(slug)?.page}),
            Self::List { limit, offset } => {
                let result = store.page_list(*limit, *offset)?;
                json!({"pages":result.pages,"limit":result.limit,"offset":result.offset,"has_more":result.has_more})
            }
        };
        let result = json!({"mode":"remote-read","head":current,"data":data});
        if serde_json::to_vec(&result)
            .map_err(|_| AppError::new("invalid_response", "cannot encode query result"))?
            .len()
            > 1024 * 1024
        {
            return Err(AppError::new(
                "query_result_too_large",
                "query result exceeds 1 MiB; narrow the query",
            ));
        }
        Ok(result)
    }
}
