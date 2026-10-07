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
fn replica(tx: &rusqlite::Connection, actor: &str, space: &str, replica: &str) -> Result<()> {
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

fn space_lock(directory: &FilePath) -> Result<fs::File> {
    super::private_directory(directory)?;
    let path = directory.join("operation.lock");
    if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_private_directory",
            "space lock cannot be a symlink",
        ));
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    lock.lock()?;
    Ok(lock)
}

pub(super) async fn space_operation<T: Send + 'static>(
    state: &Shared,
    secret: String,
    space: String,
    role: &'static str,
    operation: impl FnOnce(&Transaction<'_>, &mut Store, &FilePath, &str) -> Result<T> + Send + 'static,
) -> Result<T> {
    operate(state, secret, space, role, false, operation).await
}

async fn space_metadata<T: Send + 'static>(
    state: &Shared,
    secret: String,
    space: String,
    role: &'static str,
    operation: impl FnOnce(&Transaction<'_>, &mut Store, &FilePath, &str) -> Result<T> + Send + 'static,
) -> Result<T> {
    operate(state, secret, space, role, true, operation).await
}

async fn operate<T: Send + 'static>(
    state: &Shared,
    secret: String,
    space: String,
    role: &'static str,
    metadata_write: bool,
    operation: impl FnOnce(&Transaction<'_>, &mut Store, &FilePath, &str) -> Result<T> + Send + 'static,
) -> Result<T> {
    id(&space)?;
    let directory = state.config.data.join("spaces").join(&space);
    database(state, move |conn| {
        // Authorize before creating any space files; never wait for the space lock
        // while holding a control transaction. All server space writers use this lock.
        let (actor, _) = super::delegation::principal(conn, &secret, &space, role)?;
        control::authorize(conn, &actor, &space, role)?;
        let _lock = space_lock(&directory)?;
        let epoch: String =
            conn.query_row("SELECT epoch FROM spaces WHERE id=?1", [&space], |r| {
                r.get(0)
            })?;
        let (mut store, _) = Store::initialize("team", directory.join("wiki.db"))?;
        store.bind_team_space(&space, &epoch)?;
        fs::create_dir_all(directory.join("snapshots"))?;
        if store.team_head()?["artifact_id"].is_null() {
            let artifact = control::token()?;
            let path = directory.join("snapshots").join(&artifact);
            let exported = store.export_sync_state(&path)?;
            fs::OpenOptions::new().write(true).open(&path)?.sync_all()?;
            #[cfg(unix)]
            fs::File::open(directory.join("snapshots"))?.sync_all()?;
            store.initialize_team_artifact(&artifact, &exported.state_digest)?;
        }
        // Only short metadata mutations use an immediate control transaction.
        let behavior = if metadata_write {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        };
        let tx = conn.transaction_with_behavior(behavior)?;
        let (actor, delegated) = super::delegation::principal(&tx, &secret, &space, role)?;
        super::delegation::bind(&tx, delegated.as_ref(), &secret)?;
        control::authorize(&tx, &actor, &space, role)?;
        let result = operation(&tx, &mut store, &directory, &actor)?;
        tx.commit()?;
        Ok(result)
    })
    .await
}

/// Re-read security state on a fresh connection just before the space COMMIT.
/// A read snapshot from preparation is never sufficient authorization to publish.
pub(super) fn publication_guard(
    directory: &FilePath,
    secret: &str,
    space: &str,
    actor: &str,
    epoch: &str,
    prepared_rules: &str,
    prepared_principal: &Value,
) -> Result<rusqlite::Connection> {
    let conn = control::open(directory.parent().unwrap().parent().unwrap())?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let (current, delegated) = super::delegation::principal(&conn, secret, space, "editor")?;
    super::delegation::bind(&conn, delegated.as_ref(), secret)?;
    control::authorize(&conn, &current, space, "editor")?;
    let current_epoch: String =
        conn.query_row("SELECT epoch FROM spaces WHERE id=?1", [space], |r| {
            r.get(0)
        })?;
    if current != actor || json!(delegated) != *prepared_principal || current_epoch != epoch {
        return Err(AppError::new(
            "forbidden",
            "publication identity or epoch changed during preparation",
        ));
    }
    if policy_rules(&conn, actor, space)? != prepared_rules {
        return Err(AppError::new(
            "head_changed",
            "permissions changed during preparation; retry under current policy",
        ));
    }
    // This read-only write barrier has no durable updates. Dropping the connection
    // rolls it back immediately after canonical commit, before derived rebuilding.
    Ok(conn)
}
pub(super) fn policy_rules(
    conn: &rusqlite::Connection,
    actor: &str,
    space: &str,
) -> Result<String> {
    serde_json::to_string(&super::policy::denials(conn, actor, space)?)
        .map_err(|_| AppError::new("invalid_policy", "invalid current resource permissions"))
}

async fn space_query<T: Send + 'static>(
    state: &Shared,
    secret: String,
    space: String,
    operation: impl FnOnce(&Transaction<'_>, &Store, &FilePath, &str) -> Result<T> + Send + 'static,
) -> Result<T> {
    id(&space)?;
    let directory = state.config.data.join("spaces").join(&space);
    let ready = Store::open_for_read("team", directory.join("wiki.db")).is_ok_and(|store| {
        store
            .team_head()
            .is_ok_and(|head| head["artifact_id"].is_string())
    });
    if !ready {
        space_operation(
            state,
            secret.clone(),
            space.clone(),
            "viewer",
            |_, _, _, _| Ok(()),
        )
        .await?;
    }
    // Pure reads use a consistent WAL snapshot, so the current space remains
    // readable while a writer prepares its next version. No write/space lock.
    database(state, move |conn| {
        let tx = conn.transaction()?;
        let (actor, delegated) = super::delegation::principal(&tx, &secret, &space, "viewer")?;
        super::delegation::bind(&tx, delegated.as_ref(), &secret)?;
        control::authorize(&tx, &actor, &space, "viewer")?;
        let store = Store::open_for_read("team", directory.join("wiki.db"))?;
        let result = store.read_team_snapshot(|store| operation(&tx, store, &directory, &actor));
        let recover = store.team_indexes_pending().unwrap_or(false);
        tx.commit()?;
        if recover {
            schedule_index_recovery(&directory);
        }
        result
    })
    .await
}

// The durable derived receipt is the job queue. Acquire before spawning, so
// request fan-out cannot accumulate blocked workers or interfere with writers.
fn schedule_index_recovery(directory: &FilePath) {
    let path = directory.join("operation.lock");
    if fs::symlink_metadata(&path).map_or(true, |meta| {
        !meta.is_file() || meta.file_type().is_symlink()
    }) {
        return;
    }
    let Ok(lock) = fs::OpenOptions::new().read(true).write(true).open(path) else {
        return;
    };
    if lock.try_lock().is_err() {
        return;
    }
    let directory = directory.to_owned();
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        let result = Store::initialize("team", directory.join("wiki.db"))
            .and_then(|(mut store, _)| store.resume_team_indexes());
        if let Err(error) = result {
            eprintln!("team index recovery: {}", error.code);
        }
    });
}

async fn head(
    State(state): State<Shared>,
    Path(space): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, HttpError> {
    Ok(Json(
        space_query(
            &state,
            session(&headers)?,
            space,
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
    Ok(Json(space_metadata(&state,replication_session(&headers)?,space,"viewer",move|tx,store,directory,actor|{
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
    Ok(Json(space_metadata(&state,replication_session(&headers)?,space,"editor",move|tx,_,directory,actor|{
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
    let (path,size)=space_metadata(&state,actor_session.clone(),space.clone(),"editor",move|tx,_,directory,actor|{
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
    space_metadata(&state,actor_session,space,"editor",move|tx,_,_,actor|{
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
    let secret = replication_session(&headers)?;
    let final_secret = secret.clone();
    Ok(Json(space_operation(&state,secret,space,"editor",move|tx,store,directory,actor|{
        replica(tx,actor,&target,&input.replica_id)?;
        if let Some(receipt)=store.team_receipt(&input.replica_id,&input.batch_id)? {
            if receipt["team"]["accepted_digest"]!=input.payload_digest || receipt["team"]["artifact_id"]!=input.artifact_id || receipt["team"]["actor"]!=actor {return Err(AppError::new("batch_conflict","batch was already used for a different submission"));}
            control::open(directory.parent().unwrap().parent().unwrap())?.execute("UPDATE uploads SET status='committed' WHERE id=?1",[&input.artifact_id])?;return signed_receipt(directory,receipt);
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
            fs::OpenOptions::new().write(true).open(&normalized)?.sync_all()?;
            #[cfg(unix)] fs::File::open(directory.join("snapshots"))?.sync_all()?;
        } else if crate::store::sync_state_digest(&normalized)?!=input.payload_digest {return Err(AppError::new("sync_checksum_mismatch","previous reconstruction digest differs"));}
        store.reject_revoked_images(&normalized)?;
        super::policy::authorize_delta(tx,actor,&target,&baseline,&normalized)?;
        let commit=TeamCommit{recovery:None,epoch:input.server_epoch,expected_head:input.expected_head,actor:actor.to_owned(),principal:json!(super::delegation::current(tx)?),replica_id:input.replica_id.clone(),batch_id:input.batch_id.clone(),artifact_id:input.artifact_id.clone(),payload_digest:input.payload_digest};
        let rules = policy_rules(tx, actor, &target)?;
        let prepared_principal = json!(super::delegation::current(tx)?);
        store.publish_team_state_guarded(&normalized,&store.identity()?,&commit, || {
            let guard = publication_guard(directory, &final_secret, &target, actor, &commit.epoch, &rules, &prepared_principal)?;
            replica(&guard, actor, &target, &input.replica_id)?;
            if !guard.query_row("SELECT EXISTS(SELECT 1 FROM uploads WHERE id=?1 AND space_id=?2 AND user_id=?3 AND replica_id=?4 AND status='uploaded' AND transfer_json=?5)", params![input.artifact_id,target,actor,input.replica_id,raw], |r| r.get::<_,bool>(0))? {
                return Err(AppError::new("invalid_upload", "upload changed before publication"));
            }
            Ok(guard)
        })?;
        control::open(directory.parent().unwrap().parent().unwrap())?.execute("UPDATE uploads SET status='committed' WHERE id=?1",[&input.artifact_id])?;
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
        space_query(
            &state,
            replication_session(&headers)?,
            space,
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
    Ok(Json(space_metadata(&state,replication_session(&headers)?,space,"viewer",move|tx,store,_,actor|{
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
    Ok(Json(space_metadata(&state,replication_session(&headers)?,space,"viewer",move|tx,_,_,actor|{
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
    Ok(Json(space_metadata(&state,replication_session(&headers)?,space,"editor",move|tx,_,directory,actor|{
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
        space_query(&state, session, space, move |tx, store, directory, _| {
            let mut result = input.execute(store, directory)?;
            let revision: i64 = tx.query_row(
                "SELECT revision FROM spaces WHERE id=?1",
                [result["head"]["space_id"].as_str()],
                |r| r.get(0),
            )?;
            result["policy_revision"] = json!(revision);
            Ok(result)
        })
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
    fn team_publication_rechecks_revocation_without_holding_control_during_prepare() {
        for (action, expected_error) in [
            ("space.revoke", "forbidden"),
            ("space.delete", "space_deleted"),
            ("team.delete", "team_deleted"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().join("hub");
            let initialized = control::initialize(&data, "owner@example.com", "Test").unwrap();
            let owner = initialized["user_id"].as_str().unwrap();
            let team = initialized["team_id"].as_str().unwrap();
            let mut conn = control::open(&data).unwrap();
            let space = control::manage(
                &mut conn,
                owner,
                &json!({"action":"space.create","team_id":team,"name":"Barrier"}),
            )
            .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let tx = conn.transaction().unwrap();
            let member = control::identity_user(&tx, "github", "test", "barrier").unwrap();
            tx.execute(
                "INSERT INTO memberships VALUES(?1,?2,'member')",
                params![team, member],
            )
            .unwrap();
            let secret = control::create_session(&tx, &member).unwrap();
            tx.commit().unwrap();
            control::manage(&mut conn, owner, &json!({"action":"space.grant","space_id":space,"user_id":member,"role":"editor","expected_revision":1})).unwrap();
            let directory = data.join("spaces").join(&space);
            super::super::private_directory(&directory).unwrap();
            let epoch: String = conn
                .query_row("SELECT epoch FROM spaces WHERE id=?1", [&space], |r| {
                    r.get(0)
                })
                .unwrap();
            let rules = policy_rules(&conn, &member, &space).unwrap();
            let (mut source, _) =
                Store::initialize("project", temp.path().join("source/wiki.db")).unwrap();
            source
                .page_put(PagePutInput {
                    slug: "guarded".into(),
                    title: "Guarded".into(),
                    kind: None,
                    summary: None,
                    body: "Must roll back after revocation".into(),
                    source_ids: vec![],
                    provenance: vec!["agent-observed".into()],
                })
                .unwrap();
            let candidate = temp.path().join("candidate.db");
            let summary = source.export_sync_state(&candidate).unwrap();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            let worker_space = space.clone();
            let worker_member = member.clone();
            let worker_directory = directory.clone();
            let worker = std::thread::spawn(move || {
                let _lock = space_lock(&worker_directory).unwrap();
                let (mut store, _) =
                    Store::initialize("team", worker_directory.join("wiki.db")).unwrap();
                store.bind_team_space(&worker_space, &epoch).unwrap();
                let expected = store.identity().unwrap();
                let commit = TeamCommit {
                    recovery: None,
                    epoch,
                    expected_head: 0,
                    actor: worker_member.clone(),
                    principal: Value::Null,
                    replica_id: "1".repeat(64),
                    batch_id: "2".repeat(64),
                    artifact_id: "3".repeat(64),
                    payload_digest: summary.state_digest,
                };
                let result =
                    store.publish_team_state_guarded(&candidate, &expected, &commit, || {
                        ready_tx.send(()).unwrap();
                        resume_rx
                            .recv_timeout(std::time::Duration::from_secs(15))
                            .unwrap();
                        publication_guard(
                            &worker_directory,
                            &secret,
                            &worker_space,
                            &worker_member,
                            &commit.epoch,
                            &rules,
                            &Value::Null,
                        )
                    });
                assert_eq!(result.unwrap_err().code, expected_error);
                assert_eq!(store.identity().unwrap(), expected);
                assert_eq!(store.team_head().unwrap()["head"], 0);
                assert!(
                    store
                        .team_receipt(&commit.replica_id, &commit.batch_id)
                        .unwrap()
                        .is_none()
                );
                assert!(store.page_show("guarded").is_err());
            });
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(15))
                .unwrap();
            // Complete management and another space operation while the first space
            // remains paused with its canonical transaction still uncommitted.
            let other = control::manage(
                &mut conn,
                owner,
                &json!({"action":"space.create","team_id":team,"name":"Independent"}),
            )
            .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let _other_lock = space_lock(&data.join("spaces").join(other)).unwrap();
            control::manage(&mut conn,owner,&json!({"action":action,"team_id":team,"space_id":space,"user_id":member,"expected_revision":if action=="team.delete"{3}else{2}})).unwrap();
            resume_tx.send(()).unwrap();
            worker.join().unwrap();
        }
    }

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
        // A small second push must remain usable after a realistic first import.
        let mut fixture = rusqlite::Connection::open(temp.path().join("a/wiki.db")).unwrap();
        let seed = fixture.transaction().unwrap();
        for index in 0..65 {
            let content = format!("source {index}");
            seed.execute(
                "INSERT INTO sources(content_hash,title,origin,content,structural_navigation,created_at)
                 VALUES(?1,'source','source.md',?2,0,'2026-01-01T00:00:00.000Z')",
                params![control::digest(&content), content],
            ).unwrap();
        }
        for _ in 0..4140 {
            seed.execute("INSERT INTO operations(action,target,detail_json) VALUES('page_put','fixture','{}')", []).unwrap();
        }
        seed.commit().unwrap();
        drop(fixture);
        for index in 0..147 {
            a.page_put(PagePutInput {
                slug: format!("fixture-{index}"),
                title: format!("Fixture {index}"),
                kind: None,
                summary: None,
                body: "Fixture evidence".into(),
                source_ids: vec![],
                provenance: vec!["agent-observed".into()],
            })
            .unwrap();
        }
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
        let exported = a.export_sync_state(&normalized).unwrap();
        assert!(exported.object_count >= 4500);
        assert_eq!(exported.blob_count, 65);
        let artifact = temp.path().join("transfer.bin");
        let transfer = crate::store::prepare_sync_transfer(None, &normalized, &artifact).unwrap();
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let origin=format!("http://{address}");
            let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().unwrap();
            let state=Arc::new(Server{config:Config{admin_assets:None,data:data.clone(),public_url:origin.clone(),listen:address,smtp:None,github:None,feishu:None,max_artifact_bytes:8*1024*1024,max_space_bytes:64*1024*1024},client:client.clone()});
            let server=tokio::spawn(async move{axum::serve(listener,routes().with_state(state)).await.unwrap();});
            let root=format!("{origin}/api/spaces/{space}");
            let registration=json!({"device":"A","device_id":"1".repeat(64)});
            let response=client.post(format!("{root}/replicas")).bearer_auth(&secret).json(&registration).send().await.unwrap();
            let status=response.status(); let body=response.text().await.unwrap();
            assert!(status.is_success(), "registration returned {status}: {body}");
            let registered:Value=serde_json::from_str(&body).unwrap();
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
            // Simulate interruption after canonical COMMIT but before index rebuilding.
            let persisted=rusqlite::Connection::open(data.join("spaces").join(&space).join("wiki.db")).unwrap();
            persisted.execute_batch("UPDATE operations SET detail_json=json_set(detail_json,'$.derived',json('{\"status\":\"pending\"}')) WHERE action='sync_merge'; DELETE FROM search_fts;").unwrap();
            drop(persisted);
            let index_writer=space_lock(&data.join("spaces").join(&space)).unwrap();
            let readable=tokio::time::timeout(std::time::Duration::from_secs(1),client.get(format!("{root}/head")).bearer_auth(&secret).send()).await;
            let body=tokio::time::timeout(std::time::Duration::from_secs(1),client.post(format!("{root}/query")).bearer_auth(&secret).json(&json!({"action":"get","slug":"shared"})).send()).await.unwrap().unwrap();
            assert_eq!(body.status(),200);
            let receipt_read=tokio::time::timeout(std::time::Duration::from_secs(1),client.get(format!("{root}/receipts/{}/{}",registered["replica_id"].as_str().unwrap(),batch["batch_id"].as_str().unwrap())).bearer_auth(&secret).send()).await.unwrap().unwrap();
            assert_eq!(receipt_read.status(),200);
            let stale:Value=client.post(format!("{root}/query")).bearer_auth(&secret).json(&json!({"action":"search","query":"Local","limit":10})).send().await.unwrap().json().await.unwrap();
            assert!(stale.get("data").is_none(),"pending indexes cannot pretend to be current search results");
            drop(index_writer);
            assert!(readable.is_ok(),"authoritative head reads must not wait for the derived-index writer lock");
            assert_eq!(readable.unwrap().unwrap().status(),200);
            let mut searched = Value::Null;
            for _ in 0..40 {
                let response=client.post(format!("{root}/query")).bearer_auth(&secret).json(&json!({"action":"search","query":"Local evidence","limit":10})).send().await.unwrap();
                let value:Value=response.json().await.unwrap();
                if value.get("data").is_some() { searched=value; break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(searched["data"]["results"].as_array().unwrap().iter().any(|v|v["identifier"]=="shared"));
            assert_eq!(searched["head"]["head"],1);
            let persisted=rusqlite::Connection::open(data.join("spaces").join(&space).join("wiki.db")).unwrap();
            persisted.execute_batch("UPDATE operations SET detail_json=json_set(detail_json,'$.derived',json('{\"status\":\"failed\",\"error\":\"io_error\"}')) WHERE action='sync_merge'; DELETE FROM search_fts;").unwrap();
            drop(persisted);
            let mut recovered = Value::Null;
            for _ in 0..40 {
                let response=client.post(format!("{root}/query")).bearer_auth(&secret).json(&json!({"action":"search","query":"Local","limit":10})).send().await.unwrap();
                let value:Value=response.json().await.unwrap();
                if value.get("data").is_some() { recovered=value; break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(recovered["data"]["results"].as_array().unwrap().iter().any(|v|v["identifier"]=="shared"));
            assert_eq!(recovered["head"]["head"],1);
            // Preparation failures are also persisted and throttled across polls.
            let directory=data.join("spaces").join(&space);
            let persisted=rusqlite::Connection::open(directory.join("wiki.db")).unwrap();
            let artifact:String=persisted.query_row("SELECT value FROM meta WHERE key='team_artifact'",[],|r|r.get(0)).unwrap();
            let snapshot=directory.join("snapshots").join(&artifact);
            let held=directory.join("held-snapshot");
            fs::rename(&snapshot,&held).unwrap();
            persisted.execute_batch("UPDATE operations SET detail_json=json_set(detail_json,'$.derived',json('{\"status\":\"pending\"}')) WHERE action='sync_merge'").unwrap();
            let mut failed=Value::Null;
            for _ in 0..40 {
                assert_eq!(client.get(format!("{root}/head")).bearer_auth(&secret).send().await.unwrap().status(),200);
                let raw:String=persisted.query_row("SELECT detail_json FROM operations WHERE action='sync_merge' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
                let value:Value=serde_json::from_str(&raw).unwrap();
                if value["derived"]["status"]=="failed" {failed=value["derived"].clone();break;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert_eq!(failed["attempts"],1);
            assert!(failed["retry_after"].as_u64().unwrap()>super::super::lease::now().unwrap());
            for _ in 0..5 {assert_eq!(client.get(format!("{root}/head")).bearer_auth(&secret).send().await.unwrap().status(),200);}
            let raw:String=persisted.query_row("SELECT detail_json FROM operations WHERE action='sync_merge' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&raw).unwrap()["derived"],failed);
            fs::rename(&held,&snapshot).unwrap();
            // Expire only the synthetic clock gate, then verify automatic recovery.
            persisted.execute_batch("UPDATE operations SET detail_json=json_set(detail_json,'$.derived.retry_after',0) WHERE action='sync_merge'").unwrap();
            drop(persisted);
            for _ in 0..40 {
                let value:Value=client.post(format!("{root}/query")).bearer_auth(&secret).json(&json!({"action":"search","query":"Local","limit":10})).send().await.unwrap().json().await.unwrap();
                if value.get("data").is_some() {break;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Store::open_for_read("team",directory.join("wiki.db")).unwrap().require_team_indexes().unwrap();
            let downloaded=client.get(format!("{root}/pull")).bearer_auth(&secret).send().await.unwrap().error_for_status().unwrap();
            let transfer:SyncTransferSummary=serde_json::from_str(downloaded.headers()["x-lwc-transfer"].to_str().unwrap()).unwrap();
            let download=temp.path().join("download.bin");fs::write(&download,downloaded.bytes().await.unwrap()).unwrap();
            let b_normalized=temp.path().join("b-normalized.db");crate::store::apply_sync_transfer_artifact(None,&download,&transfer,&b_normalized).unwrap();
            let (mut b,_)=Store::initialize("project",temp.path().join("b/wiki.db")).unwrap();b.publish_sync_state(&b_normalized,&b.identity().unwrap(),"http-replica").unwrap();
            assert_eq!(b.page_show("shared").unwrap().page.body,"Local evidence first");
            a.page_put(PagePutInput{slug:"deployment".into(),title:"Deployment".into(),kind:None,summary:None,body:"One incremental record".into(),source_ids:vec![],provenance:vec!["agent-observed".into()]}).unwrap();
            let next=temp.path().join("a-next.db");a.export_sync_state(&next).unwrap();
            let delta_path=temp.path().join("delta.bin");
            let delta=crate::store::prepare_sync_transfer(Some(&normalized),&next,&delta_path).unwrap();
            let reservation=json!({"replica_id":registered["replica_id"],"request_id":"5".repeat(64),"transfer":delta});
            let upload:Value=client.post(format!("{root}/uploads")).bearer_auth(&secret).json(&reservation).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
            let artifact=upload["artifact_id"].as_str().unwrap();
            client.put(format!("{root}/uploads/{artifact}")).bearer_auth(&secret).body(fs::read(&delta_path).unwrap()).send().await.unwrap().error_for_status().unwrap();
            let incremental=json!({"protocol":"lwc-team-sync/1","share_schema":1,"server_epoch":registered["head"]["server_epoch"],"expected_head":1,"replica_id":registered["replica_id"],"batch_id":"6".repeat(64),"artifact_id":artifact,"payload_digest":delta.state_digest});
            let started=std::time::Instant::now();
            let (pushed,control_write)=tokio::join!(
                client.post(format!("{root}/push")).bearer_auth(&secret).json(&incremental).send(),
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    let data=data.clone();
                    tokio::task::spawn_blocking(move || {
                        // Uses the normal control database's five-second busy timeout.
                        control::open(&data)?.execute_batch("BEGIN IMMEDIATE; COMMIT;")?;
                        Ok::<_,AppError>(())
                    }).await.unwrap()
                }
            );
            control_write.unwrap();
            let receipt:Value=pushed.unwrap().error_for_status().unwrap().json().await.unwrap();
            assert_eq!(receipt["team"]["accepted_head"],2);
            eprintln!("4500-object incremental HTTP push with concurrent control writer: {:?}",started.elapsed());
            let cloud=Store::open_for_read("team",data.join("spaces").join(&space).join("wiki.db")).unwrap();
            assert_eq!(cloud.page_show("deployment").unwrap().page.body,"One incremental record");
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
