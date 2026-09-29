//! Compensating commits reuse the normal merge, policy, receipt and publication path.
use super::{
    control, hub, policy,
    server::{HttpError, Shared, browser_origin, session},
};
use crate::{
    error::{AppError, Result},
    store::{
        TeamCommit, cleanup_sync_conflict_candidates, merge_sync_states_directional,
        next_sync_conflict_batch, read_sync_conflict_candidate, resolve_sync_conflicts,
        sync_state_digest,
    },
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path as FilePath};
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RecoveryQuery {
    History {
        limit: usize,
        offset: usize,
    },
    Preview {
        revert_head: u64,
    },
    Candidate {
        preview_id: String,
        digest: String,
        reference: String,
        offset: u64,
        limit: u64,
    },
    Resolve {
        preview_id: String,
        digest: String,
        resolution: Value,
    },
    Apply {
        preview_id: String,
        digest: String,
        request_id: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    id: String,
    actor: String,
    head: Value,
    revert_head: u64,
    candidate: String,
    digest: String,
    conflicts: Vec<Value>,
    expires_at: u64,
}
fn id(value: &str) -> Result<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_id",
            "expected a canonical recovery ID",
        ))
    }
}
fn save(root: &FilePath, preview: &Preview) -> Result<()> {
    let temporary = root.join(format!(".{}.json", control::token()?));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(
        &serde_json::to_vec(preview)
            .map_err(|_| AppError::new("invalid_recovery", "cannot encode recovery"))?,
    )?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, root.join("preview.json"))?;
    #[cfg(unix)]
    fs::File::open(root)?.sync_all()?;
    Ok(())
}
fn load(
    directory: &FilePath,
    preview_id: &str,
    actor: &str,
    digest: &str,
) -> Result<(std::path::PathBuf, Preview)> {
    id(preview_id)?;
    id(digest)?;
    let root = directory.join("recovery").join(preview_id);
    let preview: Preview = serde_json::from_slice(&fs::read(root.join("preview.json"))?)
        .map_err(|_| AppError::new("invalid_recovery", "invalid saved recovery"))?;
    if preview.id != preview_id || preview.actor != actor || preview.digest != digest {
        return Err(AppError::new(
            "recovery_stale",
            "recovery owner or digest differs",
        ));
    }
    if preview.expires_at <= super::lease::now()? {
        return Err(AppError::new(
            "recovery_expired",
            "prepare a new recovery against current head",
        ));
    }
    id(&preview.candidate)?;
    if sync_state_digest(&root.join(&preview.candidate))? != preview.digest {
        return Err(AppError::new(
            "sync_checksum_mismatch",
            "recovery candidate changed",
        ));
    }
    Ok((root, preview))
}
fn response(preview: &Preview) -> Value {
    json!({"preview_id":preview.id,"digest":preview.digest,"expected_head":preview.head["head"],"revert_head":preview.revert_head,"expires_at":preview.expires_at,"conflict_count":preview.conflicts.len(),"conflicts":next_sync_conflict_batch(&preview.conflicts),"requires_human_approval":false,"next_action":if preview.conflicts.is_empty(){"apply"}else{"resolve"}})
}
fn artifact(directory: &FilePath, head: &Value) -> Result<std::path::PathBuf> {
    let value = head["artifact_id"]
        .as_str()
        .ok_or_else(|| AppError::new("history_not_found", "historical artifact unavailable"))?;
    id(value)?;
    let path = directory.join("snapshots").join(value);
    if sync_state_digest(&path)? != head["digest"].as_str().unwrap_or("") {
        return Err(AppError::new(
            "sync_checksum_mismatch",
            "historical snapshot changed",
        ));
    }
    Ok(path)
}
fn authorize(
    conn: &Connection,
    actor: &str,
    space: &str,
    before: &FilePath,
    after: &FilePath,
) -> Result<()> {
    policy::authorize_delta(conn, actor, space, before, after)?;
    let rules = policy::denials(conn, actor, space)?;
    let candidate = Connection::open_with_flags(after, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    candidate.execute(
        "ATTACH DATABASE ?1 AS previous",
        [before.to_string_lossy().as_ref()],
    )?;
    let mut stmt=candidate.prepare("SELECT kind,logical_key FROM main.sync_objects n WHERE NOT EXISTS(SELECT 1 FROM previous.sync_objects p WHERE p.kind=n.kind AND p.logical_key=n.logical_key AND p.payload_hash=n.payload_hash) UNION SELECT kind,logical_key FROM previous.sync_objects p WHERE NOT EXISTS(SELECT 1 FROM main.sync_objects n WHERE n.kind=p.kind AND n.logical_key=p.logical_key AND n.payload_hash=p.payload_hash)")?;
    for pair in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (kind, key) = pair?;
        if rules.iter().any(|rule| {
            matches!(rule.action.as_str(), "rollback" | "*")
                && (rule.kind == "*" || rule.kind == kind)
                && (rule.key == "*" || rule.key == key)
        }) {
            return Err(AppError::new(
                "forbidden",
                "rollback action is not permitted for an affected object",
            ));
        }
    }
    Ok(())
}
fn rejected_images(before: &FilePath, bad: &FilePath, after: &FilePath) -> Result<Vec<Value>> {
    let conn = Connection::open_with_flags(bad, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute(
        "ATTACH DATABASE ?1 AS previous",
        [before.to_string_lossy().as_ref()],
    )?;
    conn.execute(
        "ATTACH DATABASE ?1 AS recovered",
        [after.to_string_lossy().as_ref()],
    )?;
    let mut stmt=conn.prepare("SELECT kind,logical_key,payload_hash FROM main.sync_objects b WHERE kind NOT IN ('memory_audit','source_revision') AND NOT EXISTS(SELECT 1 FROM previous.sync_objects p WHERE p.kind=b.kind AND p.logical_key=b.logical_key AND p.payload_hash=b.payload_hash) AND NOT EXISTS(SELECT 1 FROM recovered.sync_objects r WHERE r.kind=b.kind AND r.logical_key=b.logical_key AND r.payload_hash=b.payload_hash)")?;
    let images=stmt.query_map([],|r|Ok(json!({"kind":r.get::<_,String>(0)?,"key":r.get::<_,String>(1)?,"hash":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(images)
}
pub(super) async fn execute(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<RecoveryQuery>,
) -> std::result::Result<Json<Value>, HttpError> {
    let secret = session(&headers)?;
    if !headers.contains_key(header::AUTHORIZATION) {
        browser_origin(&state, &headers)?;
    }
    let role = if matches!(input, RecoveryQuery::History { .. }) {
        "viewer"
    } else {
        "editor"
    };
    let target = space.clone();
    let capacity = state.config.max_space_bytes;
    Ok(Json(hub::space_operation(&state,secret,space,role,move|tx,store,directory,actor|{
        match input {
            RecoveryQuery::History{limit,offset}=>store.team_history(limit,offset),
            RecoveryQuery::Preview{revert_head}=>{
                let bad=store.team_commit_at(revert_head)?;
                if bad["parent"].is_null(){return Err(AppError::new("history_not_found","commit predates reversible parent manifests"));}
                let head=store.team_head()?;
                let before=artifact(directory,&bad["parent"])?;
                let bad_file=artifact(directory,&json!({"artifact_id":bad["artifact_id"],"digest":bad["accepted_digest"]}))?;
                let current=artifact(directory,&head)?;
                hub::quota(directory,fs::metadata(&current)?.len().saturating_mul(3),capacity)?;
                let id=control::token()?;let root=directory.join("recovery").join(&id);super::private_directory(&root)?;
                let candidate=control::token()?;
                // Reverse just this commit against current state. Three-way merge preserves
                // later unrelated edits and surfaces overlapping edits to external Agents.
                let result=merge_sync_states_directional(&bad_file,&before,&bad_file,&current,&root.join(&candidate))?;
                let preview=Preview{id,actor:actor.into(),head,revert_head,candidate,digest:result.state_digest,conflicts:result.conflicts,expires_at:super::lease::now()?+900};
                if preview.conflicts.is_empty(){authorize(tx,actor,&target,&current,&root.join(&preview.candidate))?;}
                save(&root,&preview)?;Ok(response(&preview))
            }
            RecoveryQuery::Candidate{preview_id,digest,reference,offset,limit}=>{
                let (root,preview)=load(directory,&preview_id,actor,&digest)?;
                if !preview.conflicts.iter().any(|c|c["candidate_refs"].as_array().is_some_and(|refs|refs.iter().any(|v|v==&reference))){return Err(AppError::new("invalid_candidate_request","candidate is not part of this recovery"));}
                read_sync_conflict_candidate(&root.join(preview.candidate),&reference,offset,limit)
            }
            RecoveryQuery::Resolve{preview_id,digest,resolution}=>{
                let (root,mut preview)=load(directory,&preview_id,actor,&digest)?;
                if store.team_head()?["head"]!=preview.head["head"]{return Err(AppError::new("head_changed","prepare recovery again against the new head"));}
                if preview.conflicts.is_empty(){return Err(AppError::new("recovery_stale","no conflicts remain"));}
                let candidate=control::token()?;let path=root.join(&candidate);fs::copy(root.join(&preview.candidate),&path)?;
                let batch=next_sync_conflict_batch(&preview.conflicts);
                preview.digest=resolve_sync_conflicts(&path,&batch,&resolution)?;
                preview.conflicts.drain(..batch.len());
                if preview.conflicts.is_empty(){preview.digest=cleanup_sync_conflict_candidates(&path)?;authorize(tx,actor,&target,&artifact(directory,&preview.head)?,&path)?;}
                fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
                preview.candidate=candidate;save(&root,&preview)?;Ok(response(&preview))
            }
            RecoveryQuery::Apply{preview_id,digest,request_id}=>{
                id(&preview_id)?;id(&digest)?;id(&request_id)?;
                if let Some(receipt)=store.team_receipt(&preview_id,&request_id)? {
                    if receipt["team"]["actor"]!=actor || receipt["team"]["accepted_digest"]!=digest{return Err(AppError::new("batch_conflict","recovery request was reused"));}
                    return hub::signed_receipt(directory,receipt);
                }
                let (root,preview)=load(directory,&preview_id,actor,&digest)?;
                let head=store.team_head()?;
                if head!=preview.head{return Err(AppError::new("head_changed","prepare recovery again against the new head"));}
                if !preview.conflicts.is_empty(){return Err(AppError::new("recovery_conflicts","Agent must resolve all conflicts before applying"));}
                let candidate=root.join(&preview.candidate);let current=artifact(directory,&head)?;
                authorize(tx,actor,&target,&current,&candidate)?;store.reject_revoked_images(&candidate)?;
                let bad=store.team_commit_at(preview.revert_head)?;
                let rejected=rejected_images(&artifact(directory,&bad["parent"])?,&artifact(directory,&json!({"artifact_id":bad["artifact_id"],"digest":bad["accepted_digest"]}))?,&candidate)?;
                hub::quota(directory,fs::metadata(&candidate)?.len(),capacity)?;
                let artifact_id=control::token()?;let published=directory.join("snapshots").join(&artifact_id);fs::copy(&candidate,&published)?;fs::OpenOptions::new().write(true).open(&published)?.sync_all()?;
                #[cfg(unix)] fs::File::open(directory.join("snapshots"))?.sync_all()?;
                let commit=TeamCommit{epoch:head["server_epoch"].as_str().unwrap().into(),expected_head:head["head"].as_u64().unwrap(),actor:actor.into(),principal:json!(super::delegation::current(tx)?),recovery:Some(json!({"revert_head":preview.revert_head,"preview_id":preview.id,"rejected_images":rejected})),replica_id:preview_id.clone(),batch_id:request_id.clone(),artifact_id,payload_digest:digest};
                store.publish_team_state(&published,&store.identity()?,&commit)?;
                hub::signed_receipt(directory,store.team_receipt(&preview_id,&request_id)?.ok_or_else(||AppError::new("sync_receipt_invalid","recovery receipt unavailable"))?)
            }
        }
    }).await?))
}
