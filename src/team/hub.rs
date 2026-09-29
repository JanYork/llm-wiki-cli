use super::{
    control,
    server::{HttpError, Shared, database, session},
};
use crate::{
    error::{AppError, Result},
    store::{Store, SyncTransferSummary, TeamCommit},
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use futures_util::{Stream, StreamExt};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path as FilePath, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/spaces/{space}/head", get(head))
        .route(
            "/api/spaces/{space}/recovery",
            post(super::recovery::execute),
        )
        .route("/api/spaces/{space}/query", post(query))
        .route("/api/spaces/{space}/replicas", post(register))
        .route("/api/spaces/{space}/pull", get(pull))
        .route("/api/spaces/{space}/uploads", post(reserve))
        .route(
            "/api/spaces/{space}/uploads/{upload}",
            put(upload).delete(abort_upload),
        )
        .route("/api/spaces/{space}/push", post(push))
        .route(
            "/api/spaces/{space}/receipts/{replica}/{batch}",
            get(receipt),
        )
        .route("/api/spaces/{space}/ack", post(ack))
        .route("/api/spaces/{space}/report", post(report))
}

fn replication_session(headers: &HeaderMap) -> Result<String> {
    if !headers.contains_key(header::AUTHORIZATION) {
        return Err(AppError::new(
            "unauthorized",
            "replication requires a device bearer session",
        ));
    }
    session(headers)
}

fn id(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AppError::new(
            "invalid_id",
            "expected a canonical object ID",
        ));
    }
    Ok(())
}
fn replica(tx: &Transaction<'_>, actor: &str, space: &str, replica: &str) -> Result<()> {
    id(replica)?;
    if let Some(principal) = super::delegation::current(tx)?
        && !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM replicas WHERE id=?1 AND device_key=?2)",
            params![replica, principal.device_id],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(AppError::new(
            "forbidden",
            "replica belongs to another registered device",
        ));
    }
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM replicas WHERE id=?1 AND space_id=?2 AND user_id=?3 AND revoked=0)",params![replica,space,actor],|r|r.get::<_,bool>(0))? {return Err(AppError::new("forbidden","replica is not registered to this account and space"));}
    Ok(())
}

pub(super) async fn space_operation<T: Send + 'static>(
    state: &Shared,
    secret: String,
    space: String,
    role: &'static str,
    operation: impl FnOnce(&Transaction<'_>, &mut Store, &FilePath, &str) -> Result<T> + Send + 'static,
) -> Result<T> {
    id(&space)?;
    let directory = state.config.data.join("spaces").join(&space);
    database(state, move |conn| {
        // ponytail: one control write lock serializes commits and permission changes;
        // introduce per-space authorization leases only if measured throughput requires it.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (actor, delegated) = super::delegation::principal(&tx, &secret, &space, role)?;
        super::delegation::bind(&tx, delegated.as_ref(), &secret)?;
        control::authorize(&tx, &actor, &space, role)?;
        let epoch: String =
            tx.query_row("SELECT epoch FROM spaces WHERE id=?1", [&space], |r| {
                r.get(0)
            })?;
        super::private_directory(&directory)?;
        let (mut store, _) = Store::initialize("team", directory.join("wiki.db"))?;
        store.bind_team_space(&space, &epoch)?;
        fs::create_dir_all(directory.join("snapshots"))?;
        if store.team_head()?["artifact_id"].is_null() {
            let artifact = control::token()?;
            let path = directory.join("snapshots").join(&artifact);
            let exported = store.export_sync_state(&path)?;
            fs::File::open(&path)?.sync_all()?;
            #[cfg(unix)]
            fs::File::open(directory.join("snapshots"))?.sync_all()?;
            store.initialize_team_artifact(&artifact, &exported.state_digest)?;
        }
        let result = operation(&tx, &mut store, &directory, &actor)?;
        tx.commit()?;
        Ok(result)
    })
    .await
}

async fn head(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, HttpError> {
    Ok(Json(
        space_operation(
            &state,
            session(&headers)?,
            space,
            "viewer",
            |tx, store, directory, actor| authorized_head(tx, store, actor, directory),
        )
        .await?,
    ))
}

fn authorized_head(
    tx: &Transaction<'_>,
    store: &Store,
    actor: &str,
    directory: &FilePath,
) -> Result<Value> {
    let mut head = store.team_head()?;
    let mut role: String = tx.query_row(
        "SELECT role FROM space_grants WHERE space_id=?1 AND user_id=?2",
        params![head["space_id"].as_str(), actor],
        |r| r.get(0),
    )?;
    if super::delegation::current(tx)?.is_some_and(|d| !d.can_write) {
        role = "viewer".into();
    }
    head["role"] = json!(role);
    head["manifest"] = json!(super::lease::sign(
        directory.parent().unwrap().parent().unwrap(),
        &json!({"kind":"space-head","head":head})
    )?);
    let space = head["space_id"]
        .as_str()
        .ok_or_else(|| AppError::new("invalid_space", "missing bound space"))?;
    head["denials"] = json!(super::policy::denials(tx, actor, space)?);
    head["policy"] = json!(super::lease::issue(
        tx,
        directory.parent().unwrap().parent().unwrap(),
        actor,
        head["space_id"].as_str().unwrap(),
        &role
    )?);
    Ok(head)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Register {
    device: String,
    device_id: String,
}
async fn register(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Register>,
) -> std::result::Result<Json<Value>, HttpError> {
    if input.device.trim().is_empty()
        || input.device.len() > 160
        || input.device.chars().any(char::is_control)
    {
        return Err(AppError::new("invalid_device", "invalid device name").into());
    }
    id(&input.device_id)?;
    let target = space.clone();
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"viewer",move|tx,store,directory,actor|{
        if super::delegation::current(tx)?.is_some_and(|d|d.device_id!=input.device_id) {return Err(AppError::new("forbidden","delegated credential is bound to another device"));}
        let id=control::token()?;
        tx.execute("INSERT INTO replicas(id,space_id,user_id,device,last_seen,device_key) VALUES(?1,?2,?3,?4,unixepoch(),?5) ON CONFLICT(space_id,user_id,device_key) DO NOTHING",params![id,target,actor,input.device,input.device_id])?;
        let id:String=tx.query_row("SELECT id FROM replicas WHERE space_id=?1 AND user_id=?2 AND device_key=?3 AND revoked=0",params![target,actor,input.device_id],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("forbidden","device registration was revoked"))?;
        let role:String=tx.query_row("SELECT role FROM space_grants WHERE space_id=?1 AND user_id=?2",params![target,actor],|r|r.get(0))?;
        Ok(json!({"replica_id":id,"role":role,"head":authorized_head(tx,store,actor,directory)?}))
    }).await?))
}

#[derive(Deserialize)]
struct Pull {
    baseline_artifact: Option<String>,
}
struct Download {
    stream: Option<ReaderStream<tokio::fs::File>>,
    path: PathBuf,
}
impl Stream for Download {
    type Item = std::io::Result<Bytes>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(self.get_mut().stream.as_mut().expect("download stream")).poll_next(cx)
    }
}
impl Drop for Download {
    fn drop(&mut self) {
        self.stream.take();
        let _ = fs::remove_file(&self.path);
    }
}
async fn pull(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Query(input): Query<Pull>,
) -> std::result::Result<Response, HttpError> {
    if let Some(baseline) = &input.baseline_artifact {
        id(baseline)?;
    }
    let limit = state.config.max_space_bytes;
    let (path, summary, head) = space_operation(
        &state,
        session(&headers)?,
        space,
        "viewer",
        move |tx, store, directory, actor| {
            let mut head = authorized_head(tx, store, actor, directory)?;
            head.as_object_mut().unwrap().remove("policy");
            head.as_object_mut().unwrap().remove("denials");
            let artifact = head["artifact_id"]
                .as_str()
                .ok_or_else(|| AppError::new("replica_not_ready", "space has no snapshot"))?;
            let current = directory.join("snapshots").join(artifact);
            let baseline = input
                .baseline_artifact
                .map(|id| directory.join("snapshots").join(id))
                .filter(|p| p.is_file());
            quota(directory, fs::metadata(&current)?.len(), limit)?;
            let downloads = directory.join("downloads");
            fs::create_dir_all(&downloads)?;
            for entry in fs::read_dir(&downloads)? {
                let entry = entry?;
                if entry
                    .metadata()?
                    .modified()?
                    .elapsed()
                    .is_ok_and(|age| age > std::time::Duration::from_secs(600))
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
            if fs::read_dir(&downloads)?.take(4).count() >= 4 {
                return Err(AppError::new(
                    "rate_limited",
                    "too many concurrent downloads",
                ));
            }
            let path = downloads.join(control::token()?);
            let summary =
                crate::store::prepare_sync_transfer(baseline.as_deref(), &current, &path)?;
            Ok((path, summary, head))
        },
    )
    .await?;
    let file = tokio::fs::File::open(&path).await.map_err(AppError::from)?;
    let mut response = Body::from_stream(Download {
        stream: Some(ReaderStream::new(file)),
        path,
    })
    .into_response();
    let values = [
        ("content-type", "application/octet-stream".to_owned()),
        ("content-length", summary.size.to_string()),
        ("x-lwc-transfer", summary_json(&summary)?),
        ("x-lwc-head", summary_json(&head)?),
    ];
    for (key, value) in values {
        response.headers_mut().insert(
            key,
            value
                .parse()
                .map_err(|_| AppError::new("invalid_response", "invalid transfer headers"))?,
        );
    }
    Ok(response)
}
fn summary_json(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|_| AppError::new("invalid_transfer", "cannot encode transfer metadata"))
}
pub(super) fn quota(directory: &FilePath, additional: u64, limit: u64) -> Result<()> {
    if additional > limit {
        return Err(AppError::new(
            "space_quota_exceeded",
            "space capacity exceeded; existing memory is preserved",
        ));
    }
    let mut bytes = additional;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.path().symlink_metadata()?;
            if metadata.file_type().is_symlink() {
                return Err(AppError::new(
                    "unsafe_space_path",
                    "unexpected symlink in space data",
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
                    AppError::new("space_quota_exceeded", "space capacity exceeded")
                })?;
            }
            if bytes > limit {
                return Err(AppError::new(
                    "space_quota_exceeded",
                    "space capacity exceeded; existing memory is preserved",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reservation {
    replica_id: String,
    request_id: String,
    transfer: SyncTransferSummary,
}
async fn reserve(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Reservation>,
) -> std::result::Result<Json<Value>, HttpError> {
    if input.transfer.size > state.config.max_artifact_bytes {
        return Err(AppError::new(
            "artifact_too_large",
            "transfer exceeds configured artifact limit",
        )
        .into());
    }
    id(&input.request_id)?;
    id(&input.transfer.state_digest)?;
    if let Some(base) = &input.transfer.baseline_digest {
        id(base)?;
    }
    let target = space.clone();
    let limit = state.config.max_space_bytes;
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"editor",move|tx,_,directory,actor|{
        replica(tx,actor,&target,&input.replica_id)?;quota(directory,input.transfer.size.saturating_mul(2),limit)?;
        let expired=tx.prepare("SELECT id FROM uploads WHERE space_id=?1 AND created_at<unixepoch()-3600")?.query_map([&target],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for old in expired {let _=fs::remove_file(directory.join("uploads").join(&old));tx.execute("DELETE FROM uploads WHERE id=?1",[old])?;}
        let previous:Option<(String,String,String)>=tx.query_row("SELECT id,transfer_json,status FROM uploads WHERE space_id=?1 AND user_id=?2 AND request_key=?3",params![target,actor,input.request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id,raw,status))=previous {if raw!=summary_json(&input.transfer)? {return Err(AppError::new("batch_conflict","upload request already has different metadata"));}return Ok(json!({"artifact_id":id,"status":status,"expires_in":3600}));}
        let count:i64=tx.query_row("SELECT COUNT(*) FROM uploads WHERE space_id=?1 AND status!='committed'",[&target],|r|r.get(0))?;
        if count>=4 {return Err(AppError::new("rate_limited","space already has four pending uploads"));}
        let upload=control::token()?;
        tx.execute("INSERT INTO uploads(id,space_id,replica_id,user_id,transfer_json,status,request_key) VALUES(?1,?2,?3,?4,?5,'reserved',?6)",params![upload,target,input.replica_id,actor,summary_json(&input.transfer)?,input.request_id])?;
        Ok(json!({"artifact_id":upload,"expires_in":3600}))
    }).await?))
}

struct PartialUpload(Option<PathBuf>);
impl Drop for PartialUpload {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
async fn upload(
    State(state): State<Shared>,
    Path((space, upload)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> std::result::Result<Json<Value>, HttpError> {
    id(&upload)?;
    let actor_session = replication_session(&headers)?;
    let upload_id = upload.clone();
    let target = space.clone();
    let (path,size)=space_operation(&state,actor_session.clone(),space.clone(),"editor",move|tx,_,directory,actor|{
        let raw:String=tx.query_row("SELECT transfer_json FROM uploads WHERE id=?1 AND space_id=?2 AND user_id=?3 AND status='reserved' AND created_at>unixepoch()-3600",params![upload_id,target,actor],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("invalid_upload","upload is expired, used, or owned by another account"))?;
        let transfer:SyncTransferSummary=serde_json::from_str(&raw).map_err(|_|AppError::new("invalid_transfer","invalid upload metadata"))?;
        tx.execute("UPDATE uploads SET status='receiving' WHERE id=?1",[&upload_id])?;
        let uploads=directory.join("uploads");fs::create_dir_all(&uploads)?;
        Ok((uploads.join(upload_id),transfer.size))
    }).await?;
    let mut guard = PartialUpload(Some(path.clone()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(AppError::from)?;
    let mut stream = body.into_data_stream();
    let mut bytes = 0_u64;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
    while let Some(chunk) = tokio::time::timeout_at(
        deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(30)),
        stream.next(),
    )
    .await
    .map_err(|_| AppError::new("upload_interrupted", "upload deadline exceeded"))?
    {
        let chunk =
            chunk.map_err(|_| AppError::new("upload_interrupted", "upload body interrupted"))?;
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| AppError::new("artifact_too_large", "upload exceeded size"))?;
        if bytes > size {
            return Err(
                AppError::new("artifact_too_large", "upload exceeded declared size").into(),
            );
        }
        file.write_all(&chunk).await.map_err(AppError::from)?;
    }
    if bytes != size {
        return Err(
            AppError::new("upload_incomplete", "upload size differs from reservation").into(),
        );
    }
    file.sync_all().await.map_err(AppError::from)?;
    drop(file);
    let target = space.clone();
    space_operation(&state,actor_session,space,"editor",move|tx,_,_,actor|{
        if tx.execute("UPDATE uploads SET status='uploaded' WHERE id=?1 AND space_id=?2 AND user_id=?3 AND status='receiving'",params![upload,target,actor])?!=1 {return Err(AppError::new("invalid_upload","upload was invalidated"));}Ok(())
    }).await?;
    guard.0 = None;
    Ok(Json(json!({"uploaded":true,"bytes":bytes})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Push {
    protocol: String,
    share_schema: u32,
    server_epoch: String,
    expected_head: u64,
    replica_id: String,
    batch_id: String,
    artifact_id: String,
    payload_digest: String,
}
async fn push(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Push>,
) -> std::result::Result<Json<Value>, HttpError> {
    if input.protocol != "lwc-team-sync/1" || input.share_schema != 1 {
        return Err(AppError::new("protocol_unsupported", "unknown replication protocol").into());
    }
    for value in [
        &input.replica_id,
        &input.batch_id,
        &input.artifact_id,
        &input.server_epoch,
        &input.payload_digest,
    ] {
        id(value)?;
    }
    let target = space.clone();
    let limit = state.config.max_space_bytes;
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"editor",move|tx,store,directory,actor|{
        replica(tx,actor,&target,&input.replica_id)?;
        if let Some(receipt)=store.team_receipt(&input.replica_id,&input.batch_id)? {
            if receipt["team"]["accepted_digest"]!=input.payload_digest || receipt["team"]["artifact_id"]!=input.artifact_id || receipt["team"]["actor"]!=actor {return Err(AppError::new("batch_conflict","batch was already used for a different submission"));}
            tx.execute("UPDATE uploads SET status='committed' WHERE id=?1",[&input.artifact_id])?;return signed_receipt(directory,receipt);
        }
        let current=store.team_head()?;
        if current["server_epoch"]!=input.server_epoch {return Err(AppError::new("server_epoch_changed","server epoch changed; preserve local work"));}
        if current["head"].as_u64()!=Some(input.expected_head) {return Err(AppError::new("head_changed","head changed; pull and merge"));}
        let raw:String=tx.query_row("SELECT transfer_json FROM uploads WHERE id=?1 AND space_id=?2 AND user_id=?3 AND replica_id=?4 AND status='uploaded'",params![input.artifact_id,target,actor,input.replica_id],|r|r.get(0)).optional()?.ok_or_else(||AppError::new("invalid_upload","completed upload not found"))?;
        let transfer:SyncTransferSummary=serde_json::from_str(&raw).map_err(|_|AppError::new("invalid_transfer","invalid upload metadata"))?;
        if transfer.state_digest!=input.payload_digest {return Err(AppError::new("sync_checksum_mismatch","batch digest differs from reservation"));}
        let baseline=directory.join("snapshots").join(current["artifact_id"].as_str().ok_or_else(||AppError::new("replica_not_ready","missing baseline"))?);
        let normalized=directory.join("snapshots").join(&input.artifact_id);
        if !normalized.exists() {
            quota(directory,fs::metadata(&baseline)?.len().saturating_add(transfer.size),limit)?;
            crate::store::apply_sync_transfer_artifact(Some(&baseline),&directory.join("uploads").join(&input.artifact_id),&transfer,&normalized)?;
            fs::File::open(&normalized)?.sync_all()?;
            #[cfg(unix)] fs::File::open(directory.join("snapshots"))?.sync_all()?;
        } else if crate::store::sync_state_digest(&normalized)?!=input.payload_digest {return Err(AppError::new("sync_checksum_mismatch","previous reconstruction digest differs"));}
        store.reject_revoked_images(&normalized)?;
        super::policy::authorize_delta(tx,actor,&target,&baseline,&normalized)?;
        let commit=TeamCommit{recovery:None,epoch:input.server_epoch,expected_head:input.expected_head,actor:actor.to_owned(),principal:json!(super::delegation::current(tx)?),replica_id:input.replica_id.clone(),batch_id:input.batch_id.clone(),artifact_id:input.artifact_id.clone(),payload_digest:input.payload_digest};
        store.publish_team_state(&normalized,&store.identity()?,&commit)?;
        tx.execute("UPDATE uploads SET status='committed' WHERE id=?1",[&input.artifact_id])?;
        let _=fs::remove_file(directory.join("uploads").join(&input.artifact_id));
        signed_receipt(directory,store.team_receipt(&input.replica_id,&input.batch_id)?.ok_or_else(||AppError::new("sync_receipt_invalid","committed receipt missing"))?)
    }).await?))
}

async fn receipt(
    State(state): State<Shared>,
    Path((space, replica_id, batch)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, HttpError> {
    id(&batch)?;
    let target = space.clone();
    Ok(Json(
        space_operation(
            &state,
            replication_session(&headers)?,
            space,
            "viewer",
            move |tx, store, directory, actor| {
                replica(tx, actor, &target, &replica_id)?;
                Ok(json!({"receipt":store.team_receipt(&replica_id,&batch)?.map(|value|signed_receipt(directory,value)).transpose()?}))
            },
        )
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    replica_id: String,
    server_epoch: String,
    head: u64,
}
async fn ack(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Ack>,
) -> std::result::Result<Json<Value>, HttpError> {
    let target = space.clone();
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"viewer",move|tx,store,_,actor|{
        replica(tx,actor,&target,&input.replica_id)?;let current=store.team_head()?;
        if current["server_epoch"]!=input.server_epoch || current["head"].as_u64().is_none_or(|head|input.head>head) {return Err(AppError::new("invalid_ack","ack does not belong to the current server epoch/head"));}
        tx.execute("UPDATE replicas SET ack_head=MAX(ack_head,?1),last_seen=unixepoch(),sync_status='current',pending_conflicts=0 WHERE id=?2",params![i64::try_from(input.head).map_err(|_|AppError::new("invalid_ack","head exceeds supported range"))?,input.replica_id])?;Ok(json!({"acknowledged":input.head}))
    }).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplicaReport {
    replica_id: String,
    status: String,
    conflict_count: u32,
}
async fn report(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<ReplicaReport>,
) -> std::result::Result<Json<Value>, HttpError> {
    if !matches!(
        input.status.as_str(),
        "conflict" | "retry" | "recovery_required"
    ) {
        return Err(AppError::new("invalid_report", "unsupported replica status").into());
    }
    let target = space.clone();
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"viewer",move|tx,_,_,actor|{
        replica(tx,actor,&target,&input.replica_id)?;
        tx.execute("UPDATE replicas SET sync_status=?1,pending_conflicts=?2,last_seen=unixepoch() WHERE id=?3",params![input.status,input.conflict_count,input.replica_id])?;
        Ok(json!({"reported":true}))
    }).await?))
}

async fn abort_upload(
    State(state): State<Shared>,
    Path((space, upload)): Path<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, HttpError> {
    id(&upload)?;
    let target = space.clone();
    Ok(Json(space_operation(&state,replication_session(&headers)?,space,"editor",move|tx,_,directory,actor|{
        let removed=tx.execute("DELETE FROM uploads WHERE id=?1 AND space_id=?2 AND user_id=?3 AND status!='committed'",params![upload,target,actor])?;
        if removed>0 {let _=fs::remove_file(directory.join("uploads").join(upload));}
        Ok(json!({"aborted":removed>0}))
    }).await?))
}

async fn query(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
    Json(input): Json<super::CloudQuery>,
) -> std::result::Result<Json<Value>, HttpError> {
    input.validate()?;
    let session = super::server::session(&headers)?;
    if !headers.contains_key(header::AUTHORIZATION) {
        super::server::browser_origin(&state, &headers)?;
    }
    Ok(Json(
        space_operation(
            &state,
            session,
            space,
            "viewer",
            move |tx, store, directory, _| {
                let mut result = input.execute(store, directory)?;
                let revision: i64 = tx.query_row(
                    "SELECT revision FROM spaces WHERE id=?1",
                    [result["head"]["space_id"].as_str()],
                    |r| r.get(0),
                )?;
                result["policy_revision"] = json!(revision);
                Ok(result)
            },
        )
        .await?,
    ))
}

pub(super) fn signed_receipt(directory: &FilePath, mut receipt: Value) -> Result<Value> {
    if let Some(recovery) = receipt["team"]["recovery"].as_object_mut()
        && let Some(images) = recovery.remove("rejected_images")
    {
        recovery.insert(
            "rejected_count".into(),
            json!(images.as_array().map_or(0, Vec::len)),
        );
        recovery.insert(
            "rejected_digest".into(),
            json!(control::digest(&images.to_string())),
        );
    }
    receipt["manifest"] = json!(super::lease::sign(
        directory.parent().unwrap().parent().unwrap(),
        &json!({"kind":"commit-receipt","receipt":receipt})
    )?);
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::super::server::{Config, Server};
    use super::*;
    use crate::store::PagePutInput;
    use std::sync::Arc;

    #[test]
    fn team_hub_http_roundtrip_idempotency_and_revocation() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("hub");
        let initialized = control::initialize(&data, "owner@example.com", "Test").unwrap();
        let owner = initialized["user_id"].as_str().unwrap();
        let team = initialized["team_id"].as_str().unwrap();
        let mut conn = control::open(&data).unwrap();
        let space = control::manage(
            &mut conn,
            owner,
            &json!({"action":"space.create","team_id":team,"name":"Shared memory"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (member, secret) = {
            let tx = conn.transaction().unwrap();
            let member = control::identity_user(&tx, "github", "test", "123").unwrap();
            tx.execute(
                "INSERT INTO memberships VALUES(?1,?2,'member')",
                params![team, member],
            )
            .unwrap();
            let secret = control::create_session(&tx, &member).unwrap();
            tx.commit().unwrap();
            (member, secret)
        };
        control::manage(&mut conn,owner,&json!({"action":"space.grant","space_id":space,"user_id":member,"role":"editor","expected_revision":1})).unwrap();
        let (mut a, _) = Store::initialize("project", temp.path().join("a/wiki.db")).unwrap();
        a.page_put(PagePutInput {
            slug: "shared".into(),
            title: "Shared".into(),
            kind: None,
            summary: None,
            body: "Local evidence first".into(),
            source_ids: vec![],
            provenance: vec!["agent-observed".into()],
        })
        .unwrap();
        let normalized = temp.path().join("a-normalized.db");
        a.export_sync_state(&normalized).unwrap();
        let artifact = temp.path().join("transfer.bin");
        let transfer = crate::store::prepare_sync_transfer(None, &normalized, &artifact).unwrap();
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let origin=format!("http://{address}");
            let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().unwrap();
            let state=Arc::new(Server{config:Config{admin_assets:None,data:data.clone(),public_url:origin.clone(),listen:address,smtp:None,github:None,feishu:None,max_artifact_bytes:1024*1024,max_space_bytes:64*1024*1024},client:client.clone()});
            let server=tokio::spawn(async move{axum::serve(listener,routes().with_state(state)).await.unwrap();});
            let root=format!("{origin}/api/spaces/{space}");
            let registration=json!({"device":"A","device_id":"1".repeat(64)});
            let registered:Value=client.post(format!("{root}/replicas")).bearer_auth(&secret).json(&registration).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let repeated:Value=client.post(format!("{root}/replicas")).bearer_auth(&secret).json(&registration).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(registered["replica_id"],repeated["replica_id"]);
            let reservation=json!({"replica_id":registered["replica_id"],"request_id":"2".repeat(64),"transfer":transfer});
            let upload:Value=client.post(format!("{root}/uploads")).bearer_auth(&secret).json(&reservation).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let upload_id=upload["artifact_id"].as_str().unwrap();
            let repeated:Value=client.post(format!("{root}/uploads")).bearer_auth(&secret).json(&reservation).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(repeated["artifact_id"],upload["artifact_id"]);
            client.put(format!("{root}/uploads/{upload_id}")).bearer_auth(&secret).body(fs::read(&artifact).unwrap()).send().await.unwrap().error_for_status().unwrap();
            let batch=json!({"protocol":"lwc-team-sync/1","share_schema":1,"server_epoch":registered["head"]["server_epoch"],"expected_head":0,"replica_id":registered["replica_id"],"batch_id":"3".repeat(64),"artifact_id":upload_id,"payload_digest":transfer.state_digest});
            for _ in 0..2 {let receipt:Value=client.post(format!("{root}/push")).bearer_auth(&secret).json(&batch).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();assert_eq!(receipt["team"]["accepted_head"],1);}
            let downloaded=client.get(format!("{root}/pull")).bearer_auth(&secret).send().await.unwrap().error_for_status().unwrap();
            let transfer:SyncTransferSummary=serde_json::from_str(downloaded.headers()["x-lwc-transfer"].to_str().unwrap()).unwrap();
            let download=temp.path().join("download.bin");fs::write(&download,downloaded.bytes().await.unwrap()).unwrap();
            let b_normalized=temp.path().join("b-normalized.db");crate::store::apply_sync_transfer_artifact(None,&download,&transfer,&b_normalized).unwrap();
            let (mut b,_)=Store::initialize("project",temp.path().join("b/wiki.db")).unwrap();b.publish_sync_state(&b_normalized,&b.identity().unwrap(),"http-replica").unwrap();
            assert_eq!(b.page_show("shared").unwrap().page.body,"Local evidence first");
            let mut stale=batch.clone();stale["batch_id"]=json!("4".repeat(64));
            // This router uses HttpError's status mapping even without the outer server middleware.
            assert_eq!(client.post(format!("{root}/push")).bearer_auth(&secret).json(&stale).send().await.unwrap().status(),409);
            control::manage(&mut conn,owner,&json!({"action":"space.revoke","space_id":space,"user_id":member,"expected_revision":2})).unwrap();
            assert_eq!(client.post(format!("{root}/push")).bearer_auth(&secret).json(&batch).send().await.unwrap().status(),403);
            assert_eq!(client.get(format!("{root}/pull")).bearer_auth(&secret).send().await.unwrap().status(),403);
            server.abort();
        });
    }
}
