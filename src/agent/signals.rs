use super::{
    intent::{BookFormatEvidence, IntentSet, MemoryIntent},
    tool_protocol::{Receipt, RecognizedInvocation},
};
use crate::{
    changeset,
    config::{self, CapabilitySetting},
    error::{AppError, Result},
    scope::{Scope, StorePath, init_store_path, resolve_read_store_paths},
    store::Store,
    work,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, path::Path, time::Instant};

const MAX_SIGNALS: usize = 3;
const MAX_OPPORTUNITIES: usize = 1;
const MAX_SIGNAL_BYTES: usize = 3 * 1_024;
const MAX_SIGNAL_CHARS: usize = 8_192;
const OPPORTUNITY_PRIORITY: u8 = 20;
const SIGNAL_CATALOG: &[(&str, u8)] = &[
    ("book.enable", 20),
    ("book.format_unsupported", 60),
    ("book.start", 20),
    ("changeset.closed", 60),
    ("changeset.recovery", 100),
    ("changeset.resume", 80),
    ("changeset.start", 20),
    ("graph.code.enable", 20),
    ("graph.code.explore", 20),
    ("graph.code.recovery", 100),
    ("graph.document.enable", 20),
    ("graph.document.explore", 20),
    ("graph.document.recovery", 100),
    ("graph.enable", 20),
    ("graph.recovery", 100),
    ("ingest.completed", 60),
    ("ingest.recovery", 100),
    ("ingest.resume", 80),
    ("ingest.start", 20),
    ("memory.enable", 20),
    ("memory.maintenance", 60),
    ("memory.recall", 20),
    ("memory.record", 20),
    ("memory.status", 20),
    ("office.enable", 20),
    ("office.use", 20),
    ("plan.blocked", 60),
    ("plan.closed", 60),
    ("plan.complete", 100),
    ("plan.continue", 100),
    ("plan.disambiguate", 100),
    ("plan.enable", 20),
    ("plan.recovery", 100),
    ("plan.resume", 80),
    ("plan.start", 20),
    ("practice.enable", 20),
    ("practice.start", 20),
    ("sync.completed", 60),
    ("sync.recovery", 100),
    ("sync.resume", 80),
    ("sync.start", 20),
    ("todo.due", 60),
    ("todo.enable", 20),
    ("todo.review", 20),
    ("trans.configure", 20),
    ("trans.convert", 20),
    ("trans.runtime", 60),
    ("tutor.enable", 20),
    ("tutor.start", 20),
    ("wiki.search", 20),
    ("wiki.setup", 20),
    ("work.completed", 60),
    ("work.failed", 60),
    ("work.recovery", 100),
    ("work.resume", 80),
    ("work.review", 20),
];

fn catalog_priority(kind: &str) -> Option<u8> {
    SIGNAL_CATALOG
        .iter()
        .find_map(|(candidate, priority)| (*candidate == kind).then_some(*priority))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    SessionStart,
    SessionResume,
    SessionClear,
    CompactBefore,
    CompactAfter,
    SubagentStart,
    Prompt,
    TurnStart,
    ToolBefore,
    ToolAfter,
    ToolFailure,
    Stop,
    SubagentStop,
    SessionEnd,
}

impl EventKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::SessionResume => "session_resume",
            Self::SessionClear => "session_clear",
            Self::CompactBefore => "compact_before",
            Self::CompactAfter => "compact_after",
            Self::SubagentStart => "subagent_start",
            Self::Prompt => "prompt",
            Self::TurnStart => "turn_start",
            Self::ToolBefore => "tool_before",
            Self::ToolAfter => "tool_after",
            Self::ToolFailure => "tool_failure",
            Self::Stop => "stop",
            Self::SubagentStop => "subagent_stop",
            Self::SessionEnd => "session_end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemanticEvent {
    pub(crate) kind: EventKind,
    pub(crate) native: String,
}

pub(crate) fn parse_event(native: &str, semantic: &str, payload: &Value) -> Result<SemanticEvent> {
    let kind = match semantic {
        "session_start" => session_start_kind(payload)?,
        "session_resume" => EventKind::SessionResume,
        "session_clear" => EventKind::SessionClear,
        "compact_before" => EventKind::CompactBefore,
        "compact_after" => EventKind::CompactAfter,
        "subagent_start" => EventKind::SubagentStart,
        "prompt" => EventKind::Prompt,
        "turn_start" => EventKind::TurnStart,
        "tool_before" => EventKind::ToolBefore,
        "tool_after" => EventKind::ToolAfter,
        "tool_failure" => EventKind::ToolFailure,
        "stop" => EventKind::Stop,
        "subagent_stop" => EventKind::SubagentStop,
        "session_end" => EventKind::SessionEnd,
        _ => {
            return Err(AppError::new(
                "unsupported_hook_event",
                "unsupported Agent hook event",
            ));
        }
    };
    Ok(SemanticEvent {
        kind,
        native: native.to_owned(),
    })
}

fn session_start_kind(payload: &Value) -> Result<EventKind> {
    let Some(source) = payload.get("source") else {
        return Ok(EventKind::SessionStart);
    };
    let Some(source) = source.as_str() else {
        return Err(AppError::new(
            "invalid_hook_source",
            "SessionStart source must be a supported string",
        ));
    };
    match normalize(source).as_str() {
        "startup" => Ok(EventKind::SessionStart),
        "resume" => Ok(EventKind::SessionResume),
        "clear" => Ok(EventKind::SessionClear),
        "compact" | "postcompact" => Ok(EventKind::CompactAfter),
        _ => Err(AppError::new(
            "unsupported_hook_source",
            "unsupported SessionStart source",
        )),
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompletionEffect {
    None,
    RequiresFollowup,
    #[allow(dead_code)]
    SatisfiesFollowup,
    ContinueOnce,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Signal {
    kind: &'static str,
    priority: u8,
    why_now: &'static str,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action: Option<String>,
    requires_consent: bool,
    completion_effect: CompletionEffect,
}

impl Signal {
    fn new(
        kind: &'static str,
        priority: u8,
        why_now: &'static str,
        summary: impl Into<String>,
        completion_effect: CompletionEffect,
    ) -> Self {
        let catalog_priority = catalog_priority(kind);
        debug_assert!(
            catalog_priority.is_none() || catalog_priority == Some(priority),
            "signal kind {kind} must use its fixed catalog priority"
        );
        Self {
            kind,
            priority: catalog_priority.unwrap_or(priority),
            why_now,
            summary: summary.into(),
            state: None,
            next_action: None,
            requires_consent: false,
            completion_effect,
        }
    }

    fn state(mut self, state: Value) -> Self {
        self.state = Some(state);
        self
    }

    fn next_action(mut self, next_action: impl Into<String>) -> Self {
        self.next_action = Some(next_action.into());
        self
    }

    fn consent(mut self) -> Self {
        self.requires_consent = true;
        self
    }
}

#[derive(Serialize)]
struct SignalBatch<'a> {
    schema: &'static str,
    event: &'a str,
    signals: &'a [Signal],
    omitted: usize,
}

pub(crate) struct RenderedSignals {
    pub(crate) line: String,
    pub(crate) continues: bool,
}

pub(crate) fn lifecycle(
    event: EventKind,
    cwd: &Path,
    readiness: &Value,
    deadline: Instant,
) -> Result<Option<RenderedSignals>> {
    if event == EventKind::SubagentStart || Instant::now() >= deadline {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    if let Some(signal) = plan_continuity(readiness) {
        candidates.push(signal);
    }
    if let Some(signal) = todo_due(readiness) {
        candidates.push(signal);
    }
    if let Some(signal) = sync_pending(readiness) {
        candidates.push(signal);
    }
    if let Ok(path) = init_store_path(Scope::Project, cwd) {
        if Instant::now() < deadline
            && let Ok(Some(signal)) = work_signal(&path)
        {
            candidates.push(signal);
        }
        if Instant::now() < deadline
            && let Ok(Some(signal)) = changeset_signal(&path, deadline)
        {
            candidates.push(signal);
        }
        if path.path.is_file()
            && Instant::now() < deadline
            && let Some(store) = open_store_until("project", &path.path, deadline)
            && let Ok(Some(signal)) = ingest_signal(&store)
        {
            candidates.push(signal);
        }
    }
    render(event, candidates)
}

pub(crate) fn prompt(
    event: EventKind,
    cwd: &Path,
    readiness: &Value,
    intents: &IntentSet,
    deadline: Instant,
) -> Result<Option<RenderedSignals>> {
    if !matches!(event, EventKind::Prompt | EventKind::TurnStart) || Instant::now() >= deadline {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    let wiki_initialized = readiness
        .pointer("/wiki/initialized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let path = init_store_path(Scope::Project, cwd).ok();
    let needs_store =
        intents.ingest || (intents.memory && intents.memory_intent == MemoryIntent::Status);

    if intents.plan
        && let Some(signal) = plan_prompt_signal(readiness)
    {
        candidates.push(signal);
    }
    if intents.todo
        && let Some(signal) = todo_prompt_signal(readiness)
    {
        candidates.push(signal);
    }
    if intents.work
        && Instant::now() < deadline
        && let Some(path) = path.as_ref()
        && let Ok(Some(signal)) = work_prompt_signal(path)
    {
        candidates.push(signal);
    }
    if intents.sync {
        candidates.push(sync_pending(readiness).unwrap_or_else(sync_start_signal));
    }
    let hook_store = if wiki_initialized && needs_store && Instant::now() < deadline {
        path.as_ref()
            .and_then(|path| open_store_until("project", &path.path, deadline))
    } else {
        None
    };
    if intents.ingest {
        if !wiki_initialized {
            candidates.push(wiki_setup_signal());
        } else if Instant::now() < deadline
            && let Some(store) = hook_store.as_ref()
            && let Ok(Some(signal)) = ingest_prompt_signal(store)
        {
            candidates.push(signal);
        }
    }
    if intents.changeset {
        if !wiki_initialized {
            candidates.push(wiki_setup_signal());
        } else if Instant::now() < deadline
            && let Some(path) = path.as_ref()
            && let Ok(Some(signal)) = changeset_prompt_signal(path, deadline)
        {
            candidates.push(signal);
        }
    }
    if intents.memory
        && Instant::now() < deadline
        && let Some(signal) =
            memory_prompt_signal(hook_store.as_ref(), readiness, intents.memory_intent)
    {
        candidates.push(signal);
    }
    if intents.wiki {
        candidates.push(if wiki_initialized {
            Signal::new(
                "wiki.search",
                20,
                "wiki_query_at_prompt",
                "Search bounded project Wiki evidence for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc search QUERY --limit 20")
        } else {
            wiki_setup_signal()
        });
    }
    candidates.extend(graph_prompt_signals(readiness, intents));
    if intents.tutor
        && let Some(signal) = learning_prompt_signal(readiness, "tutor")
    {
        candidates.push(signal);
    }
    if intents.practice
        && let Some(signal) = learning_prompt_signal(readiness, "practice")
    {
        candidates.push(signal);
    }
    if intents.book
        && let Some(signal) = book_prompt_signal(readiness, intents.book_format)
    {
        candidates.push(signal);
    }
    if intents.trans
        && let Some(signal) = trans_prompt_signal(readiness)
    {
        candidates.push(signal);
    } else if intents.office
        && let Some(signal) = office_prompt_signal(readiness)
    {
        candidates.push(signal);
    }
    render(event, candidates)
}

fn plan_prompt_signal(readiness: &Value) -> Option<Signal> {
    let Some(plan) = readiness.get("plan") else {
        return Some(
            Signal::new(
                "plan.enable",
                20,
                "plan_capability_disabled",
                "Ask before enabling durable Plan tracking for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project config set --plan enabled")
            .consent(),
        );
    };
    if plan.get("ready").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    plan_continuity(readiness).or_else(|| {
        Some(
            Signal::new(
                "plan.start",
                20,
                "plan_enabled_without_active",
                "Start a durable Plan only if this request needs multi-step execution.",
                CompletionEffect::None,
            )
            .next_action(
                "lwc plan create TITLE --objective OBJECTIVE --done-when DONE_WHEN --step STEP",
            ),
        )
    })
}

fn todo_prompt_signal(readiness: &Value) -> Option<Signal> {
    let Some(todo) = readiness.get("todo") else {
        return Some(
            Signal::new(
                "todo.enable",
                20,
                "todo_capability_disabled",
                "Ask before enabling durable Todo tracking for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project config set --todo enabled")
            .consent(),
        );
    };
    if todo.get("ready").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    todo_due(readiness).or_else(|| {
        Some(
            Signal::new(
                "todo.review",
                20,
                "todo_review_at_prompt",
                "Review bounded open Todo state for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc todo list --limit 20"),
        )
    })
}

fn work_prompt_signal(path: &StorePath) -> Result<Option<Signal>> {
    if let Some(signal) = work_signal(path)? {
        return Ok(Some(signal));
    }
    Ok(Some(
        Signal::new(
            "work.review",
            20,
            "work_idle_at_prompt",
            "Review bounded Work state before starting another durable job.",
            CompletionEffect::None,
        )
        .next_action("lwc work list"),
    ))
}

fn sync_start_signal() -> Signal {
    Signal::new(
        "sync.start",
        20,
        "sync_idle_at_prompt",
        "Ask for the target host and mode before starting a Sync session.",
        CompletionEffect::None,
    )
    .next_action("lwc --scope project sync HOST ABS_DIRECTORY --mode merge")
    .consent()
}

fn work_signal(path: &StorePath) -> Result<Option<Signal>> {
    let summary = work::hook_summary(path)?;
    let failures = summary
        .works
        .iter()
        .filter(|item| matches!(item.state.as_str(), "failed" | "cancelled"))
        .collect::<Vec<_>>();
    let active = summary
        .works
        .iter()
        .filter(|item| matches!(item.state.as_str(), "queued" | "running"))
        .collect::<Vec<_>>();
    let (kind, priority, why_now, items) = if !failures.is_empty() {
        ("work.failed", 60, "work_terminal_failure", failures)
    } else if !active.is_empty() {
        ("work.resume", 80, "work_nonterminal", active)
    } else {
        return Ok(None);
    };
    let id = items[0].id.clone();
    Ok(Some(
        Signal::new(
            kind,
            priority,
            why_now,
            format!(
                "Inspect {} relevant Work item(s) before continuing.",
                items.len()
            ),
            CompletionEffect::None,
        )
        .state(json!({
            "works": items,
            "omitted": summary.omitted,
            "has_more": summary.has_more,
        }))
        .next_action(format!("lwc work status {id}")),
    ))
}

fn changeset_signal(path: &StorePath, deadline: Instant) -> Result<Option<Signal>> {
    let summary = changeset::hook_summary(path, deadline)?;
    let conflicts = summary
        .changesets
        .iter()
        .filter(|item| item.conflict)
        .collect::<Vec<_>>();
    let drafts = summary
        .changesets
        .iter()
        .filter(|item| !item.conflict && !item.empty && item.status == "draft")
        .collect::<Vec<_>>();
    let (kind, priority, why_now, items) = if !conflicts.is_empty() {
        ("changeset.recovery", 100, "changeset_conflict", conflicts)
    } else if !drafts.is_empty() {
        ("changeset.resume", 80, "changeset_nonempty_draft", drafts)
    } else {
        return Ok(None);
    };
    let name = items[0].name.clone();
    Ok(Some(
        Signal::new(
            kind,
            priority,
            why_now,
            format!(
                "Inspect {} relevant Changeset draft(s) before continuing.",
                items.len()
            ),
            CompletionEffect::None,
        )
        .state(json!({
            "changesets": items,
            "omitted": summary.omitted,
        }))
        .next_action(format!("lwc changeset show {name} --limit 20")),
    ))
}

fn ingest_signal(store: &Store) -> Result<Option<Signal>> {
    let failed = store.ingest_list(Some("failed"), MAX_SIGNALS, 0)?;
    if !failed.jobs.is_empty() {
        let mut jobs = failed
            .jobs
            .iter()
            .map(|job| {
                json!({
                    "source_id": job.source_id,
                    "status": job.status,
                    "attempts": job.attempts,
                })
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|left, right| left["source_id"].as_i64().cmp(&right["source_id"].as_i64()));
        let source_id = jobs[0]["source_id"].as_i64().ok_or_else(|| {
            AppError::new("ingest_hook_invalid", "failed Ingest source ID is missing")
        })?;
        return Ok(Some(
            Signal::new(
                "ingest.recovery",
                100,
                "ingest_failed",
                format!("Inspect {} failed Ingest job(s).", jobs.len()),
                CompletionEffect::None,
            )
            .state(json!({"jobs": jobs, "has_more": failed.has_more}))
            .next_action(format!("lwc ingest retry {source_id}")),
        ));
    }

    let analyzing = store.ingest_list(Some("analyzing"), MAX_SIGNALS, 0)?;
    let generating = store.ingest_list(Some("generating"), MAX_SIGNALS, 0)?;
    let has_more = analyzing.has_more || generating.has_more;
    let mut jobs = analyzing
        .jobs
        .into_iter()
        .chain(generating.jobs)
        .map(|job| {
            json!({
                "source_id": job.source_id,
                "status": job.status,
                "attempts": job.attempts,
            })
        })
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| {
        left["source_id"]
            .as_i64()
            .cmp(&right["source_id"].as_i64())
            .then_with(|| left["status"].as_str().cmp(&right["status"].as_str()))
    });
    let truncated = jobs.len() > MAX_SIGNALS;
    jobs.truncate(MAX_SIGNALS);
    if jobs.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        Signal::new(
            "ingest.resume",
            80,
            "ingest_nonterminal",
            format!("Inspect {} active Ingest job(s).", jobs.len()),
            CompletionEffect::None,
        )
        .state(json!({"jobs": jobs, "has_more": has_more || truncated}))
        .next_action("lwc ingest list --limit 20"),
    ))
}

fn ingest_prompt_signal(store: &Store) -> Result<Option<Signal>> {
    let failed = store.ingest_list(Some("failed"), MAX_SIGNALS, 0)?;
    if !failed.jobs.is_empty() {
        let jobs = bounded_ingest_jobs(&failed);
        let source_id = jobs[0]["source_id"].as_i64().ok_or_else(|| {
            AppError::new("ingest_hook_invalid", "failed Ingest source ID is missing")
        })?;
        return Ok(Some(
            Signal::new(
                "ingest.recovery",
                100,
                "ingest_failed_at_prompt",
                format!("Inspect {} failed Ingest job(s).", jobs.len()),
                CompletionEffect::None,
            )
            .state(json!({"jobs": jobs, "has_more": failed.has_more}))
            .next_action(format!("lwc ingest retry {source_id}")),
        ));
    }

    let pending = store.ingest_list(Some("pending"), MAX_SIGNALS, 0)?;
    if !pending.jobs.is_empty() {
        let jobs = bounded_ingest_jobs(&pending);
        let source_id = jobs[0]["source_id"].as_i64().ok_or_else(|| {
            AppError::new("ingest_hook_invalid", "pending Ingest source ID is missing")
        })?;
        return Ok(Some(
            Signal::new(
                "ingest.resume",
                80,
                "ingest_pending_claim",
                format!("Claim the first of {} pending Ingest job(s).", jobs.len()),
                CompletionEffect::None,
            )
            .state(json!({"jobs": jobs, "has_more": pending.has_more}))
            .next_action(format!("lwc ingest claim {source_id}")),
        ));
    }

    if let Some(signal) = ingest_signal(store)? {
        return Ok(Some(signal));
    }
    Ok(Some(
        Signal::new(
            "ingest.start",
            20,
            "ingest_idle_at_prompt",
            "Add a reviewed source before starting the Ingest loop.",
            CompletionEffect::None,
        )
        .next_action("lwc source add PATH"),
    ))
}

fn bounded_ingest_jobs(response: &crate::store::IngestListResponse) -> Vec<Value> {
    let mut jobs = response
        .jobs
        .iter()
        .map(|job| {
            json!({
                "source_id": job.source_id,
                "status": job.status,
                "attempts": job.attempts,
            })
        })
        .collect::<Vec<_>>();
    jobs.sort_by(|left, right| left["source_id"].as_i64().cmp(&right["source_id"].as_i64()));
    jobs
}

fn changeset_prompt_signal(path: &StorePath, deadline: Instant) -> Result<Option<Signal>> {
    if let Some(signal) = changeset_signal(path, deadline)? {
        return Ok(Some(signal));
    }
    Ok(Some(
        Signal::new(
            "changeset.start",
            20,
            "changeset_idle_at_prompt",
            "Start a named Changeset only when this request needs atomic Wiki edits.",
            CompletionEffect::None,
        )
        .next_action("lwc changeset begin NAME"),
    ))
}

fn wiki_setup_signal() -> Signal {
    Signal::new(
        "wiki.setup",
        20,
        "wiki_missing_at_prompt",
        "Ask before initializing the project Wiki required by this request.",
        CompletionEffect::None,
    )
    .next_action("lwc --scope project init")
    .consent()
}

fn memory_prompt_signal(
    store: Option<&Store>,
    readiness: &Value,
    memory_intent: MemoryIntent,
) -> Option<Signal> {
    if readiness.pointer("/memory/error_code").is_some() {
        return None;
    }
    let enabled = readiness
        .pointer("/memory/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ready = readiness
        .pointer("/memory/ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled || !ready {
        let next_action = if readiness
            .pointer("/wiki/initialized")
            .and_then(Value::as_bool)
            == Some(false)
        {
            "lwc --scope project init"
        } else {
            "lwc --scope project config set --memory enabled"
        };
        return Some(
            Signal::new(
                "memory.enable",
                20,
                "memory_disabled_or_not_ready",
                "Ask before enabling durable Memory required by this request.",
                CompletionEffect::None,
            )
            .next_action(next_action)
            .consent(),
        );
    }
    match memory_intent {
        MemoryIntent::Record => Some(
            Signal::new(
                "memory.record",
                20,
                "memory_record_at_prompt",
                "Record only normalized durable evidence, never the raw prompt transcript.",
                CompletionEffect::None,
            )
            .next_action("lwc remember --json '{...}'"),
        ),
        MemoryIntent::Recall => Some(
            Signal::new(
                "memory.recall",
                20,
                "memory_recall_at_prompt",
                "Recall bounded durable Memory relevant to this request.",
                CompletionEffect::None,
            )
            .next_action("lwc memory recall QUERY --limit 5"),
        ),
        MemoryIntent::Status => {
            let store = store?;
            let status = store.memory_status().ok()?;
            memory_status_prompt_signal(&status)
        }
        MemoryIntent::None => None,
    }
}

fn memory_status_prompt_signal(status: &Value) -> Option<Signal> {
    let pending_hints = status.get("pending_hints")?.as_u64()?;
    let retained_count = status.pointer("/retained/events")?.as_u64()?;
    let logical_bytes = status.pointer("/pressure/logical_bytes")?.as_u64()?;
    let max_bytes = status.pointer("/pressure/max_bytes")?.as_u64()?;
    let pressure_ratio = status.pointer("/pressure/ratio")?.as_f64()?;
    let maintenance = pending_hints > 0 || pressure_ratio >= 0.8;
    let (kind, priority, why_now, summary, next_action) = if maintenance {
        (
            "memory.maintenance",
            60,
            "memory_maintenance_at_prompt",
            "Review bounded Memory hints or storage pressure.",
            "lwc memory maintain",
        )
    } else {
        (
            "memory.status",
            20,
            "memory_status_at_prompt",
            "Inspect bounded durable Memory status.",
            "lwc memory status",
        )
    };
    Some(
        Signal::new(kind, priority, why_now, summary, CompletionEffect::None)
            .state(json!({
                "pending_hints": pending_hints,
                "retained_count": retained_count,
                "logical_bytes": logical_bytes,
                "max_bytes": max_bytes,
                "pressure_ratio": pressure_ratio,
            }))
            .next_action(next_action),
    )
}

fn graph_prompt_signals(readiness: &Value, intents: &IntentSet) -> Vec<Signal> {
    let document_missing = intents.document_graph
        && readiness
            .pointer("/document_graph/requires_consent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let code_missing = intents.code_graph
        && readiness
            .pointer("/code_graph/requires_consent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if document_missing && code_missing {
        return vec![
            Signal::new(
                "graph.enable",
                20,
                "both_applicable_graphs_need_consent",
                "Ask once: enable both graphs, document graph only, CodeGraph only, or later.",
                CompletionEffect::None,
            )
            .state(json!({
                "choices": [
                    {"id": 1, "capabilities": ["document_graph", "code_graph"]},
                    {"id": 2, "capabilities": ["document_graph"]},
                    {"id": 3, "capabilities": ["code_graph"]},
                    {"id": 4, "capabilities": []},
                ]
            }))
            .next_action("Reply with 1-4.")
            .consent(),
        ];
    }
    if document_missing {
        return document_graph_prompt_signal(readiness, true)
            .into_iter()
            .collect();
    }
    if code_missing {
        return code_graph_prompt_signal(readiness, true)
            .into_iter()
            .collect();
    }
    let mut signals = Vec::new();
    if intents.document_graph
        && let Some(signal) = document_graph_prompt_signal(readiness, document_missing)
    {
        signals.push(signal);
    }
    if intents.code_graph
        && let Some(signal) = code_graph_prompt_signal(readiness, code_missing)
    {
        signals.push(signal);
    }
    signals
}

fn document_graph_prompt_signal(readiness: &Value, missing: bool) -> Option<Signal> {
    if missing {
        return Some(
            Signal::new(
                "graph.document.enable",
                20,
                "document_graph_needs_consent",
                "Ask before enabling the physical document graph.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project config set --graph grafeo")
            .consent(),
        );
    }
    match readiness
        .pointer("/document_graph/projection/status")
        .and_then(Value::as_str)?
    {
        "pending" => None,
        "ready" => Some(
            Signal::new(
                "graph.document.explore",
                20,
                "document_graph_ready_at_prompt",
                "Explore bounded physical document relationships for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project graph explore"),
        ),
        _ => Some(
            Signal::new(
                "graph.document.recovery",
                100,
                "document_graph_not_ready",
                "Inspect the configured document graph before using it.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project graph status"),
        ),
    }
}

fn code_graph_prompt_signal(readiness: &Value, missing: bool) -> Option<Signal> {
    if missing {
        return Some(
            Signal::new(
                "graph.code.enable",
                20,
                "code_graph_needs_consent",
                "Ask before initializing CodeGraph for this project.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project cg init")
            .consent(),
        );
    }
    if readiness
        .pointer("/code_graph/ready")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Some(
            Signal::new(
                "graph.code.explore",
                20,
                "code_graph_ready_at_prompt",
                "Explore bounded CodeGraph evidence for this request.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project cg search QUERY"),
        );
    }
    readiness
        .pointer("/code_graph/initialized")
        .and_then(Value::as_bool)
        .filter(|initialized| *initialized)
        .map(|_| {
            Signal::new(
                "graph.code.recovery",
                100,
                "code_graph_runtime_not_ready",
                "Inspect initialized CodeGraph runtime health before using it.",
                CompletionEffect::None,
            )
            .next_action("lwc --scope project cg status")
        })
}

fn learning_prompt_signal(readiness: &Value, plugin: &'static str) -> Option<Signal> {
    let state = readiness.get(plugin)?;
    if state.get("error_code").is_some() {
        return None;
    }
    let enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (kind, why_now, summary, next_action) = match (plugin, enabled) {
        ("tutor", false) => (
            "tutor.enable",
            "tutor_capability_disabled",
            "Ask before enabling Tutor.",
            "lwc --scope global config set --tutor enabled",
        ),
        ("tutor", true) => (
            "tutor.start",
            "tutor_enabled_at_prompt",
            "Start or resume a teacher-led Tutor session.",
            "lwc tutor session create --json '{...}'",
        ),
        ("practice", false) => (
            "practice.enable",
            "practice_capability_disabled",
            "Ask before enabling Practice.",
            "lwc --scope global config set --practice enabled",
        ),
        ("practice", true) => (
            "practice.start",
            "practice_enabled_at_prompt",
            "Start or resume durable Practice.",
            "lwc practice next --json '{...}'",
        ),
        _ => unreachable!("only Tutor and Practice use this helper"),
    };
    let signal =
        Signal::new(kind, 20, why_now, summary, CompletionEffect::None).next_action(next_action);
    Some(if enabled { signal } else { signal.consent() })
}

fn book_prompt_signal(readiness: &Value, format: BookFormatEvidence) -> Option<Signal> {
    if format == BookFormatEvidence::Unsupported {
        return Some(
            Signal::new(
                "book.format_unsupported",
                60,
                "book_format_unsupported",
                "Use a supported EPUB, PDF, TXT, or Markdown book input.",
                CompletionEffect::None,
            )
            .next_action("lwc book import --json '{...}'"),
        );
    }
    let state = readiness.get("book")?;
    if state.get("error_code").is_some() {
        return None;
    }
    let enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let signal = if enabled {
        Signal::new(
            "book.start",
            20,
            "book_enabled_at_prompt",
            "Import or resume a supported durable Book.",
            CompletionEffect::None,
        )
        .next_action("lwc book import --json '{...}'")
    } else {
        Signal::new(
            "book.enable",
            20,
            "book_capability_disabled",
            "Ask before enabling Book.",
            CompletionEffect::None,
        )
        .next_action("lwc --scope global config set --book enabled")
        .consent()
    };
    Some(signal)
}

fn office_prompt_signal(readiness: &Value) -> Option<Signal> {
    let state = readiness.get("office")?;
    if state.get("error_code").is_some() {
        return None;
    }
    let enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(if enabled {
        Signal::new(
            "office.use",
            20,
            "office_enabled_at_prompt",
            "Use the configured Office capability for the requested file.",
            CompletionEffect::None,
        )
        .next_action("lwc office COMMAND ...")
    } else {
        Signal::new(
            "office.enable",
            20,
            "office_capability_disabled",
            "Ask before enabling the global Office capability.",
            CompletionEffect::None,
        )
        .next_action("lwc --scope global config set --office officecli")
        .consent()
    })
}

fn trans_prompt_signal(readiness: &Value) -> Option<Signal> {
    let state = readiness.get("md_trans")?;
    if state.get("error_code").is_some() {
        return None;
    }
    let enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let available = state
        .get("executable_available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(if !enabled {
        Signal::new(
            "trans.configure",
            20,
            "trans_engine_unselected",
            "Ask which optional conversion engine to configure.",
            CompletionEffect::None,
        )
        .next_action("lwc --scope project config set --trans ENGINE")
        .consent()
    } else if !available {
        Signal::new(
            "trans.runtime",
            60,
            "trans_executable_missing",
            "The configured conversion engine executable is unavailable.",
            CompletionEffect::None,
        )
        .next_action("lwc --scope project config set --trans ENGINE")
    } else {
        Signal::new(
            "trans.convert",
            20,
            "trans_ready_at_prompt",
            "Convert the requested document with the configured engine.",
            CompletionEffect::None,
        )
        .next_action("lwc --scope project trans INPUT --output OUTPUT.md")
    })
}

fn plan_continuity(readiness: &Value) -> Option<Signal> {
    let plan = readiness.get("plan")?;
    let active = plan.get("active")?.as_i64()?;
    if active > 1 {
        return Some(
            Signal::new(
                "plan.disambiguate",
                100,
                "multiple_active_plans_at_session_boundary",
                "Choose the single active Plan before continuing durable work.",
                CompletionEffect::None,
            )
            .state(json!({"active_count": active}))
            .next_action("lwc plan current --limit 20"),
        );
    }
    if active != 1 {
        return None;
    }
    let Some(tracking) = plan.get("tracking").cloned() else {
        return Some(plan_boundary_recovery(None));
    };
    let id = tracking
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| safe_plan_id(id))
        .map(str::to_owned);
    let revision = tracking
        .get("revision")
        .and_then(Value::as_i64)
        .filter(|revision| *revision >= 1);
    let terminal = tracking
        .pointer("/progress/terminal_steps")
        .and_then(Value::as_i64)
        .filter(|terminal| *terminal >= 0);
    let total = tracking
        .pointer("/progress/total_steps")
        .and_then(Value::as_i64)
        .filter(|total| *total >= 0);
    let Some(id) = id else {
        return Some(plan_boundary_recovery(None));
    };
    let Some(revision) = revision else {
        return Some(plan_boundary_recovery(Some(&id)));
    };
    let (Some(terminal), Some(total)) = (terminal, total) else {
        return Some(plan_boundary_recovery(Some(&id)));
    };
    if terminal > total {
        return Some(plan_boundary_recovery(Some(&id)));
    }
    let current = tracking
        .pointer("/current_step/status")
        .and_then(Value::as_str);
    let signal = if total > 0 && terminal == total {
        Signal::new(
            "plan.complete",
            100,
            "active_plan_at_session_boundary",
            format!(
                "Verify done_when, then run `lwc plan complete {id} --if-revision {revision} --result RESULT --evidence EVIDENCE --done-when-checked`."
            ),
            CompletionEffect::None,
        )
    } else if current == Some("blocked") {
        Signal::new(
            "plan.blocked",
            60,
            "plan_blocked_waiting_input",
            "The active Plan is explicitly blocked; review it when the dependency changes.",
            CompletionEffect::None,
        )
    } else if current == Some("in_progress") {
        Signal::new(
            "plan.resume",
            80,
            "active_plan_at_session_boundary",
            "Resume the single active LWC Plan from its current step.",
            CompletionEffect::None,
        )
    } else {
        return Some(plan_boundary_recovery(Some(&id)));
    };
    Some(
        signal
            .state(tracking)
            .next_action(format!("lwc plan brief {id}")),
    )
}

fn plan_boundary_recovery(id: Option<&str>) -> Signal {
    let (state, next_action) = match id {
        Some(id) => (json!({"id": id}), format!("lwc plan brief {id}")),
        None => (
            json!({"active_count": 1}),
            "lwc plan current --limit 20".to_owned(),
        ),
    };
    Signal::new(
        "plan.recovery",
        100,
        "active_plan_invalid_transition_state",
        "Inspect the active Plan's invalid transition state.",
        CompletionEffect::None,
    )
    .state(state)
    .next_action(next_action)
}

fn todo_due(readiness: &Value) -> Option<Signal> {
    let todo = readiness.get("todo")?;
    let reminders = todo.get("reminders")?.as_array()?;
    if reminders.is_empty() {
        return None;
    }
    let omitted = todo
        .get("omitted_reminders")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let due = reminders.len() as u64 + omitted;
    Some(
        Signal::new(
            "todo.due",
            60,
            "due_open_todos",
            format!("Review {due} due open Todo item(s)."),
            CompletionEffect::None,
        )
        .state(json!({"due": reminders, "omitted": omitted}))
        .next_action("lwc todo list --limit 20"),
    )
}

fn sync_pending(readiness: &Value) -> Option<Signal> {
    let sync = readiness.get("sync")?;
    let pending = sync.get("pending")?.as_u64()?;
    let latest = sync.get("latest")?.clone();
    let next_action = latest.get("resume")?.as_str()?.to_owned();
    let phase = latest.get("phase")?.as_str()?;
    let conflicted = phase == "conflicts"
        || latest
            .pointer("/conflicts/count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0)
        || latest
            .pointer("/conflicts/kinds")
            .and_then(Value::as_array)
            .is_some_and(|kinds| !kinds.is_empty());
    let (kind, priority, why_now) = if conflicted {
        ("sync.recovery", 100, "sync_conflict")
    } else {
        ("sync.resume", 80, "sync_nonterminal")
    };
    Some(
        Signal::new(
            kind,
            priority,
            why_now,
            format!("Resume the latest of {pending} nonterminal Sync session(s) at {phase}."),
            CompletionEffect::None,
        )
        .state(json!({"pending": pending, "latest": latest}))
        .next_action(next_action),
    )
}

pub(crate) fn tool_failure(
    event: EventKind,
    invocation: &RecognizedInvocation,
) -> Result<Option<RenderedSignals>> {
    if event != EventKind::ToolFailure {
        return Ok(None);
    }
    let Some((kind, why_now, family, next_action)) = recovery_for_failed_action(&invocation.action)
    else {
        return Ok(None);
    };
    render(
        event,
        vec![
            Signal::new(
                kind,
                100,
                why_now,
                format!("Inspect the failed LWC {family} action before continuing."),
                CompletionEffect::None,
            )
            .state(json!({"action": invocation.action}))
            .next_action(next_action),
        ],
    )
}

pub(crate) fn tool_receipt(
    event: EventKind,
    invocation: &RecognizedInvocation,
    receipt: &Receipt,
) -> Result<Option<RenderedSignals>> {
    if event != EventKind::ToolAfter {
        return Ok(None);
    }
    if let Some(error_code) = receipt.error_code.as_deref()
        && let Some((error_family, kind, why_now, label, inspect)) = recovery_for_error(error_code)
    {
        let recovery_action = exact_recovery_action(receipt);
        if error_evidence_matches(error_family, invocation, receipt) || recovery_action.is_some() {
            let completion_effect = if recovery_action.is_some() {
                CompletionEffect::RequiresFollowup
            } else {
                CompletionEffect::None
            };
            let signal = Signal::new(
                kind,
                100,
                why_now,
                format!("Inspect the verified LWC {label} error before continuing."),
                completion_effect,
            )
            .state(typed_receipt_state(invocation, receipt))
            .next_action(recovery_action.unwrap_or_else(|| inspect.to_owned()));
            return render(event, vec![signal]);
        }
    }

    let mut candidates = Vec::new();
    if let Some(signal) = work_receipt_signal(invocation, receipt) {
        candidates.push(signal);
    }
    if let Some(signal) = plan_receipt_signal(invocation, receipt) {
        candidates.push(signal);
    }
    if let Some(signal) = memory_receipt_signal(invocation, receipt) {
        candidates.push(signal);
    }
    if let Some(signal) = concrete_close_receipt_signal(invocation, receipt) {
        candidates.push(signal);
    }
    render(event, candidates)
}

fn recovery_for_failed_action(
    action: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match action {
        "plan.create" | "plan.advance" | "plan.block" | "plan.revise" | "plan.complete"
        | "plan.abandon" => Some((
            "plan.recovery",
            "plan_tool_failure",
            "Plan",
            "lwc plan current --limit 20",
        )),
        "changeset.begin" | "changeset.commit" | "changeset.discard" | "changeset.rollback" => {
            Some((
                "changeset.recovery",
                "changeset_tool_failure",
                "Changeset",
                "lwc changeset list",
            ))
        }
        "sync.start" | "sync.resume" | "sync.resolve" | "sync.abort" => Some((
            "sync.recovery",
            "sync_tool_failure",
            "Sync",
            "lwc log --limit 20",
        )),
        "work.cancel" | "work.resume" => Some((
            "work.recovery",
            "work_tool_failure",
            "Work",
            "lwc work list",
        )),
        "graph.verify" => Some((
            "graph.recovery",
            "graph_tool_failure",
            "Graph",
            "lwc graph status",
        )),
        _ => None,
    }
}

fn recovery_for_error(
    error_code: &str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    if has_error_prefix(error_code, "plan_revision") {
        Some((
            "plan",
            "plan.recovery",
            "plan_revision_error",
            "Plan",
            "lwc plan current --limit 20",
        ))
    } else if has_error_prefix(error_code, "changeset") {
        Some((
            "changeset",
            "changeset.recovery",
            "changeset_error",
            "Changeset",
            "lwc changeset list",
        ))
    } else if has_error_prefix(error_code, "sync") {
        Some((
            "sync",
            "sync.recovery",
            "sync_error",
            "Sync",
            "lwc log --limit 20",
        ))
    } else if has_error_prefix(error_code, "work") {
        Some((
            "work",
            "work.recovery",
            "work_error",
            "Work",
            "lwc work list",
        ))
    } else if has_error_prefix(error_code, "graph") {
        Some((
            "graph",
            "graph.recovery",
            "graph_error",
            "Graph",
            "lwc graph status",
        ))
    } else {
        None
    }
}

fn has_error_prefix(error_code: &str, prefix: &str) -> bool {
    error_code == prefix
        || error_code.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.starts_with('_') || suffix.starts_with('-') || suffix.starts_with('.')
        })
}

fn error_evidence_matches(
    family: &str,
    invocation: &RecognizedInvocation,
    receipt: &Receipt,
) -> bool {
    invocation
        .action
        .split_once('.')
        .is_some_and(|(prefix, _)| prefix == family)
        || match family {
            "plan" => receipt.identifiers.contains_key("plan_id"),
            "changeset" => receipt.identifiers.contains_key("changeset_id"),
            "sync" => receipt.identifiers.contains_key("session_id"),
            "work" => receipt.identifiers.contains_key("work_id"),
            "graph" => false,
            _ => false,
        }
}

fn work_receipt_signal(invocation: &RecognizedInvocation, receipt: &Receipt) -> Option<Signal> {
    let id = receipt.identifiers.get("work_id")?;
    let state = receipt.state.as_deref()?;
    let recovery_action = work_recovery_action(receipt);
    let (kind, priority, why_now, completion_effect, next_action) = match state {
        "queued" | "running" => (
            "work.resume",
            80,
            "work_receipt_nonterminal",
            CompletionEffect::RequiresFollowup,
            Some(format!("lwc work status {id}")),
        ),
        "failed" | "cancelled" => (
            "work.failed",
            60,
            "work_receipt_terminal_failure",
            if recovery_action.is_some() {
                CompletionEffect::RequiresFollowup
            } else {
                CompletionEffect::None
            },
            recovery_action.or_else(|| Some(format!("lwc work status {id}"))),
        ),
        "succeeded" => (
            "work.completed",
            60,
            "work_receipt_succeeded",
            CompletionEffect::SatisfiesFollowup,
            None,
        ),
        _ => return None,
    };
    let mut signal = Signal::new(
        kind,
        priority,
        why_now,
        format!("Verified Work is now {state}."),
        completion_effect,
    )
    .state(typed_receipt_state(invocation, receipt));
    if let Some(next_action) = next_action {
        signal = signal.next_action(next_action);
    }
    Some(signal)
}

fn work_recovery_action(receipt: &Receipt) -> Option<String> {
    match receipt.recovery_action.as_deref()? {
        "work.resume" => receipt
            .identifiers
            .get("work_id")
            .map(|id| format!("lwc work resume {id}")),
        "work.status" => receipt
            .identifiers
            .get("work_id")
            .map(|id| format!("lwc work status {id}")),
        _ => None,
    }
}

fn plan_receipt_signal(invocation: &RecognizedInvocation, receipt: &Receipt) -> Option<Signal> {
    let (kind, priority, why_now, completion_effect) = match invocation.action.as_str() {
        "plan.create" | "plan.advance" | "plan.revise" => (
            "plan.resume",
            80,
            "plan_receipt_requires_followup",
            CompletionEffect::RequiresFollowup,
        ),
        "plan.block" => (
            "plan.blocked",
            60,
            "plan_receipt_blocked",
            CompletionEffect::SatisfiesFollowup,
        ),
        "plan.complete" | "plan.abandon" => (
            "plan.closed",
            60,
            "plan_receipt_closed",
            CompletionEffect::SatisfiesFollowup,
        ),
        _ => return None,
    };
    let mut signal = Signal::new(
        kind,
        priority,
        why_now,
        "Continue from the verified Plan transition.",
        completion_effect,
    )
    .state(typed_receipt_state(invocation, receipt));
    if completion_effect == CompletionEffect::RequiresFollowup {
        let next_action = receipt
            .identifiers
            .get("plan_id")
            .map(|id| format!("lwc plan brief {id}"))
            .unwrap_or_else(|| "lwc plan current --limit 20".to_owned());
        signal = signal.next_action(next_action);
    }
    Some(signal)
}

fn memory_receipt_signal(invocation: &RecognizedInvocation, receipt: &Receipt) -> Option<Signal> {
    if invocation.action != "memory.status" {
        return None;
    }
    let pending_hints = receipt.counts.get("pending_hints").copied().unwrap_or(0);
    let logical_bytes = receipt.counts.get("logical_bytes").copied();
    let max_bytes = receipt.counts.get("max_bytes").copied();
    let pressure_ratio = logical_bytes
        .zip(max_bytes)
        .filter(|(_, max_bytes)| *max_bytes > 0)
        .map(|(logical_bytes, max_bytes)| logical_bytes as f64 / max_bytes as f64);
    if pending_hints == 0 && pressure_ratio.is_none_or(|ratio| ratio < 0.8) {
        return None;
    }
    let mut state = Map::new();
    state.insert("pending_hints".to_owned(), json!(pending_hints));
    if let Some(retained_count) = receipt.counts.get("retained_count") {
        state.insert("retained_count".to_owned(), json!(retained_count));
    }
    if let Some(logical_bytes) = logical_bytes {
        state.insert("logical_bytes".to_owned(), json!(logical_bytes));
    }
    if let Some(max_bytes) = max_bytes {
        state.insert("max_bytes".to_owned(), json!(max_bytes));
    }
    if let Some(pressure_ratio) = pressure_ratio {
        state.insert("pressure_ratio".to_owned(), json!(pressure_ratio));
    }
    Some(
        Signal::new(
            "memory.maintenance",
            60,
            "memory_receipt_attention_needed",
            "Review verified Memory hints or storage pressure.",
            CompletionEffect::None,
        )
        .state(Value::Object(state))
        .next_action("lwc memory status"),
    )
}

fn concrete_close_receipt_signal(
    invocation: &RecognizedInvocation,
    receipt: &Receipt,
) -> Option<Signal> {
    let (kind, why_now, summary) = match invocation.action.as_str() {
        "changeset.commit" | "changeset.discard" | "changeset.rollback" => (
            "changeset.closed",
            "changeset_receipt_closed",
            "The verified Changeset action closed its follow-up.",
        ),
        "sync.abort" => (
            "sync.completed",
            "sync_receipt_closed",
            "The verified Sync action closed its follow-up.",
        ),
        "ingest.complete" => (
            "ingest.completed",
            "ingest_receipt_completed",
            "The verified Ingest action completed its follow-up.",
        ),
        _ => return None,
    };
    Some(
        Signal::new(
            kind,
            60,
            why_now,
            summary,
            CompletionEffect::SatisfiesFollowup,
        )
        .state(typed_receipt_state(invocation, receipt)),
    )
}

fn exact_recovery_action(receipt: &Receipt) -> Option<String> {
    receipt
        .recovery_action
        .as_deref()
        .and_then(|action| rebuild_action(action, receipt))
}

fn rebuild_action(action: &str, receipt: &Receipt) -> Option<String> {
    match action {
        "memory.status" => Some("lwc memory status".to_owned()),
        "graph.status" => Some("lwc graph status".to_owned()),
        "work.list" => Some("lwc work list".to_owned()),
        "work.status" => receipt
            .identifiers
            .get("work_id")
            .map(|id| format!("lwc work status {id}")),
        "work.resume" => receipt
            .identifiers
            .get("work_id")
            .map(|id| format!("lwc work resume {id}")),
        "plan.current" => Some("lwc plan current --limit 20".to_owned()),
        "plan.brief" => receipt
            .identifiers
            .get("plan_id")
            .map(|id| format!("lwc plan brief {id}")),
        "changeset.list" => Some("lwc changeset list".to_owned()),
        "ingest.list" => Some("lwc ingest list --limit 20".to_owned()),
        "todo.list" => Some("lwc todo list --limit 20".to_owned()),
        _ => None,
    }
}

fn typed_receipt_state(invocation: &RecognizedInvocation, receipt: &Receipt) -> Value {
    let mut state = Map::new();
    state.insert("action".to_owned(), json!(invocation.action));
    if let Some(action) = &receipt.action {
        state.insert("receipt_action".to_owned(), json!(action));
    }
    if !receipt.identifiers.is_empty() {
        state.insert("identifiers".to_owned(), json!(receipt.identifiers));
    }
    if !receipt.revisions.is_empty() {
        state.insert("revisions".to_owned(), json!(receipt.revisions));
    }
    if let Some(value) = &receipt.state {
        state.insert("state".to_owned(), json!(value));
    }
    if let Some(value) = &receipt.status {
        state.insert("status".to_owned(), json!(value));
    }
    if let Some(value) = &receipt.phase {
        state.insert("phase".to_owned(), json!(value));
    }
    if !receipt.counts.is_empty() {
        state.insert("counts".to_owned(), json!(receipt.counts));
    }
    if let Some(progress) = &receipt.progress {
        state.insert(
            "progress".to_owned(),
            json!({
                "completed": progress.completed,
                "total": progress.total,
                "sequence": progress.sequence,
            }),
        );
    }
    if let Some(error_code) = &receipt.error_code {
        state.insert("error_code".to_owned(), json!(error_code));
    }
    Value::Object(state)
}

fn open_store_until(scope: &str, path: &Path, deadline: Instant) -> Option<Store> {
    if Instant::now() >= deadline {
        return None;
    }
    let store = Store::open_for_hook_with_timeout(scope, path, std::time::Duration::ZERO).ok()?;
    if Instant::now() >= deadline {
        return None;
    }
    store
        .begin_hook_snapshot_with_timeout(std::time::Duration::ZERO)
        .ok()?;
    if Instant::now() >= deadline {
        return None;
    }
    Some(store)
}

pub(crate) fn stop_plan(
    scope: Scope,
    cwd: &Path,
    deadline: Instant,
    context: Option<&str>,
) -> Result<Option<RenderedSignals>> {
    let Some(context) = context else {
        return Ok(None);
    };
    let mut signals = Vec::new();
    for path in resolve_read_store_paths(scope, cwd, true)? {
        let scope_name = match path.scope {
            Scope::Project => "project",
            Scope::Global => "global",
            Scope::All => unreachable!(),
        };
        if config::resolve_plan(scope_name, &path.path)?.setting != CapabilitySetting::Enabled
            || !path.path.is_file()
        {
            continue;
        }
        let Some(store) = open_store_until(scope_name, &path.path, deadline) else {
            continue;
        };
        if let Some(tracking) = store.plan_tracking_for_context(context)?
            && let Some(signal) = stop_signal(Some(tracking))
        {
            signals.push(signal);
        }
    }
    render(EventKind::Stop, signals)
}

fn stop_signal(tracking: Option<Value>) -> Option<Signal> {
    let Some(tracking) = tracking else {
        return Some(stop_recovery(None));
    };
    let id = tracking
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| safe_plan_id(id))
        .map(str::to_owned);
    let revision = tracking
        .get("revision")
        .and_then(Value::as_i64)
        .filter(|revision| *revision >= 1);
    let Some(id) = id else {
        return Some(stop_recovery(None));
    };
    let Some(revision) = revision else {
        return Some(stop_recovery(Some(&id)));
    };
    if tracking
        .pointer("/current_step/status")
        .and_then(Value::as_str)
        == Some("blocked")
    {
        return Some(
            Signal::new(
                "plan.blocked",
                60,
                "plan_blocked_waiting_input",
                "The active Plan is blocked and cannot safely continue yet.",
                CompletionEffect::None,
            )
            .state(tracking)
            .next_action(format!("lwc plan brief {id}")),
        );
    }
    let terminal = tracking
        .pointer("/progress/terminal_steps")
        .and_then(Value::as_i64)
        .filter(|terminal| *terminal >= 0);
    let total = tracking
        .pointer("/progress/total_steps")
        .and_then(Value::as_i64)
        .filter(|total| *total >= 0);
    let (Some(terminal), Some(total)) = (terminal, total) else {
        return Some(stop_recovery(Some(&id)));
    };
    if terminal > total {
        return Some(stop_recovery(Some(&id)));
    }
    let signal = if total > 0 && terminal == total {
        Signal::new(
            "plan.complete",
            100,
            "active_plan_terminal_steps_at_stop",
            format!(
                "Run `lwc plan brief {id}`, verify done_when, then `lwc plan complete {id} --if-revision {revision} --result RESULT --evidence EVIDENCE --done-when-checked`, and continue."
            ),
            CompletionEffect::ContinueOnce,
        )
    } else if tracking
        .pointer("/current_step/status")
        .and_then(Value::as_str)
        == Some("in_progress")
    {
        Signal::new(
            "plan.continue",
            100,
            "executable_plan_at_stop",
            format!(
                "Run `lwc plan brief {id}`, verify the current step, CAS with `lwc plan advance {id} --if-revision {revision} ...`, and continue."
            ),
            CompletionEffect::ContinueOnce,
        )
    } else {
        return Some(stop_recovery(Some(&id)));
    };
    Some(
        signal
            .state(tracking)
            .next_action(format!("lwc plan brief {id}")),
    )
}

fn stop_recovery(id: Option<&str>) -> Signal {
    let (state, next_action) = match id {
        Some(id) => (json!({"id": id}), format!("lwc plan brief {id}")),
        None => (
            json!({"active_count": 1}),
            "lwc plan current --limit 20".to_owned(),
        ),
    };
    Signal::new(
        "plan.recovery",
        100,
        "active_plan_invalid_transition_state",
        "Recover the single active Plan's focal step before allowing this run to stop.",
        CompletionEffect::ContinueOnce,
    )
    .state(state)
    .next_action(next_action)
}

fn safe_plan_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn render(event: EventKind, mut candidates: Vec<Signal>) -> Result<Option<RenderedSignals>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    candidates.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.kind.cmp(right.kind))
            .then_with(|| left.why_now.cmp(right.why_now))
    });
    let mut seen = BTreeSet::new();
    let mut opportunities = 0_usize;
    let mut omitted = 0_usize;
    let mut selected = Vec::new();
    for signal in candidates {
        let serialized = serde_json::to_vec(&signal).map_err(|error| {
            AppError::new(
                "hook_output_failed",
                format!("failed to serialize signal candidate: {error}"),
            )
        })?;
        if serialized.len() > MAX_SIGNAL_BYTES {
            omitted += 1;
            continue;
        }
        if !seen.insert(signal.kind) {
            omitted += 1;
            continue;
        }
        if signal.priority == OPPORTUNITY_PRIORITY {
            if opportunities >= MAX_OPPORTUNITIES {
                omitted += 1;
                continue;
            }
            opportunities += 1;
        }
        if selected.len() >= MAX_SIGNALS {
            omitted += 1;
            continue;
        }
        selected.push(signal);
    }
    loop {
        if selected.is_empty() {
            return Ok(None);
        }
        let batch = SignalBatch {
            schema: "lwc.signal/v1",
            event: event.as_str(),
            signals: &selected,
            omitted,
        };
        let json = serde_json::to_string(&batch).map_err(|error| {
            AppError::new(
                "hook_output_failed",
                format!("failed to serialize signal batch: {error}"),
            )
        })?;
        let line = format!("LWC_SIGNAL {json}");
        if line.chars().count() <= MAX_SIGNAL_CHARS {
            return Ok(Some(RenderedSignals {
                line,
                continues: selected
                    .iter()
                    .any(|signal| signal.completion_effect == CompletionEffect::ContinueOnce),
            }));
        }
        if selected.pop().is_none() {
            return Ok(None);
        }
        omitted += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources_without_losing_the_native_event() {
        let event = parse_event(
            "SessionStart",
            "session_start",
            &json!({"source": "resume"}),
        )
        .unwrap();
        assert_eq!(event.kind, EventKind::SessionResume);
        assert_eq!(event.native, "SessionStart");
        assert_eq!(
            parse_event("SubagentStart", "subagent_start", &json!({}))
                .unwrap()
                .kind,
            EventKind::SubagentStart
        );
        assert_eq!(
            parse_event("before_agent", "turn_start", &json!({}))
                .unwrap()
                .kind,
            EventKind::TurnStart
        );
        assert_eq!(
            parse_event("AfterTool", "tool_after", &json!({}))
                .unwrap()
                .kind,
            EventKind::ToolAfter
        );
        assert_eq!(
            parse_event("SubagentStop", "subagent_stop", &json!({}))
                .unwrap()
                .kind,
            EventKind::SubagentStop
        );
        assert!(
            parse_event(
                "SessionStart",
                "session_start",
                &json!({"source": "unknown"}),
            )
            .is_err()
        );
    }

    #[test]
    fn selector_is_bounded_deduplicated_and_allows_one_opportunity() {
        let candidates = vec![
            Signal::new("four", 60, "four", "four", CompletionEffect::None),
            Signal::new("one", 100, "one", "one", CompletionEffect::None),
            Signal::new("two", 20, "two", "two", CompletionEffect::None),
            Signal::new("three", 20, "three", "three", CompletionEffect::None),
            Signal::new("one", 100, "duplicate", "duplicate", CompletionEffect::None),
        ];
        let rendered = render(EventKind::SessionStart, candidates)
            .unwrap()
            .unwrap();
        let batch: Value =
            serde_json::from_str(rendered.line.strip_prefix("LWC_SIGNAL ").unwrap()).unwrap();
        assert_eq!(batch["signals"].as_array().unwrap().len(), 3);
        assert_eq!(
            batch["signals"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|signal| signal["priority"] == 20)
                .count(),
            1
        );
        assert_eq!(batch["omitted"], 2);
        assert!(rendered.line.chars().count() <= MAX_SIGNAL_CHARS);
    }

    #[test]
    fn catalog_assigns_one_fixed_priority_and_opportunity_class_per_kind() {
        let mut seen = BTreeSet::new();
        for &(kind, priority) in SIGNAL_CATALOG {
            assert!(seen.insert(kind), "duplicate signal catalog kind {kind}");
            assert!(matches!(priority, 20 | 60 | 80 | 100), "{kind}");
            let signal = Signal::new(
                kind,
                priority,
                "catalog_test",
                "catalog test",
                CompletionEffect::None,
            );
            assert_eq!(signal.priority, priority, "{kind}");
            assert_eq!(
                signal.priority == OPPORTUNITY_PRIORITY,
                priority == 20,
                "{kind}"
            );
        }
    }

    #[test]
    fn selector_drops_a_signal_that_cannot_fit_as_a_whole_block() {
        let candidate = Signal::new(
            "oversized",
            100,
            "oversized",
            "界".repeat(MAX_SIGNAL_CHARS),
            CompletionEffect::None,
        );
        assert!(
            render(EventKind::SessionStart, vec![candidate])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn selector_drops_an_oversized_utf8_candidate_before_selecting_a_lower_candidate() {
        let oversized = Signal::new(
            "oversized",
            100,
            "oversized",
            "🧠".repeat(800),
            CompletionEffect::None,
        );
        let lower = Signal::new("lower", 60, "lower", "bounded", CompletionEffect::None);
        let first = render(
            EventKind::SessionStart,
            vec![oversized.clone(), lower.clone()],
        )
        .unwrap()
        .unwrap();
        let second = render(EventKind::SessionStart, vec![oversized, lower])
            .unwrap()
            .unwrap();
        assert_eq!(first.line, second.line);
        let batch: Value =
            serde_json::from_str(first.line.strip_prefix("LWC_SIGNAL ").unwrap()).unwrap();
        assert_eq!(batch["signals"].as_array().unwrap().len(), 1);
        assert_eq!(batch["signals"][0]["kind"], "lower");
        assert_eq!(batch["omitted"], 1);
        assert!(serde_json::to_vec(&batch["signals"][0]).unwrap().len() <= 3 * 1_024);
    }

    #[test]
    fn stop_recovery_never_builds_commands_from_missing_identity() {
        let missing = stop_signal(None).unwrap();
        assert_eq!(missing.kind, "plan.recovery");
        assert_eq!(missing.why_now, "active_plan_invalid_transition_state");
        assert_eq!(
            missing.next_action.as_deref(),
            Some("lwc plan current --limit 20")
        );
        assert_eq!(missing.state, Some(json!({"active_count": 1})));
        assert_eq!(missing.completion_effect, CompletionEffect::ContinueOnce);

        let invalid_id = stop_signal(Some(json!({
            "id": "",
            "revision": 4,
            "progress": {"terminal_steps": 0, "total_steps": 1},
            "current_step": {"status": "in_progress"},
        })))
        .unwrap();
        assert_eq!(
            invalid_id.next_action.as_deref(),
            Some("lwc plan current --limit 20")
        );
        assert_eq!(invalid_id.state, Some(json!({"active_count": 1})));

        let id = "abcdefabcdefabcdefabcdefabcdefab";
        let missing_revision = stop_signal(Some(json!({
            "id": id,
            "progress": {"terminal_steps": 0, "total_steps": 1},
            "current_step": {"status": "in_progress"},
        })))
        .unwrap();
        assert_eq!(
            missing_revision.next_action.as_deref(),
            Some("lwc plan brief abcdefabcdefabcdefabcdefabcdefab")
        );
        assert_eq!(missing_revision.state, Some(json!({"id": id})));
    }
}
