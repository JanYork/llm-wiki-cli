use crate::{
    error::{AppError, Result},
    scope::{Scope, StorePath},
    store::Store,
};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CURRENT_SCHEMA_VERSION: u32 = 12;
const RESUME_STALE_AFTER_MS: u128 = 30_000;
const STATE_REPLACE_RETRIES: usize = 200;
const STATE_REPLACE_RETRY_DELAY: Duration = Duration::from_millis(5);
static WORK_COUNTER: AtomicU64 = AtomicU64::new(0);
const GRAPH_PENDING_FILE: &str = "graph-pending.json";
const GRAPH_PENDING_LOCK: &str = "graph-pending.lock";

#[derive(Clone, Deserialize, Serialize)]
struct WorkRequest {
    id: String,
    kind: String,
    scope: String,
    database: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct WorkState {
    id: String,
    kind: String,
    scope: String,
    database: PathBuf,
    state: String,
    phase: String,
    completed: u64,
    total: Option<u64>,
    percent: Option<f64>,
    sequence: u64,
    updated_at_unix_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at_unix_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    items_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    eta_seconds: Option<u64>,
    cancel_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl WorkState {
    fn queued(request: &WorkRequest) -> Self {
        Self {
            id: request.id.clone(),
            kind: request.kind.clone(),
            scope: request.scope.clone(),
            database: request.database.clone(),
            state: "queued".into(),
            phase: "queued".into(),
            completed: 0,
            total: None,
            percent: None,
            sequence: 1,
            updated_at_unix_ms: now_ms(),
            started_at_unix_ms: None,
            items_per_second: None,
            eta_seconds: None,
            cancel_requested: false,
            pid: None,
            message: format!("{} queued", request.kind),
            result: None,
            error: None,
        }
    }

    fn update(&mut self, state: &str, phase: &str, message: impl Into<String>) {
        self.state = state.into();
        self.phase = phase.into();
        self.message = message.into();
        self.sequence += 1;
        self.updated_at_unix_ms = now_ms();
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WorkHookSummary {
    pub(crate) works: Vec<WorkHookSummaryItem>,
    pub(crate) omitted: usize,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WorkHookSummaryItem {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) phase: String,
    pub(crate) completed: u64,
    pub(crate) total: Option<u64>,
    pub(crate) sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TerminalSyncAudit {
    pub(crate) audit_key: String,
    pub(crate) digest: String,
    pub(crate) origin_store_id: String,
    pub(crate) origin_work_id: String,
    pub(crate) kind: String,
    pub(crate) state: String,
    pub(crate) completed: u64,
    pub(crate) total: Option<u64>,
    pub(crate) updated_at_unix_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error_code: Option<String>,
}
