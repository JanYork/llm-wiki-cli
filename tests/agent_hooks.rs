use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const CODEGRAPH_VERSION: &str = "v1.5.0-lwc.1";
const DEFAULT_CLAUDE_SESSION: &str = "default-claude-session";
const DEFAULT_CODEX_SESSION: &str = "default-codex-session";
const DEFAULT_CLAUDE_PROMPT_AGENT: &str = "default-claude-prompt-agent";

fn codegraph_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x64",
        ("windows", "aarch64") => "win32-arm64",
        ("windows", "x86_64") => "win32-x64",
        pair => panic!("unsupported test platform {pair:?}"),
    }
}

struct World {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl World {
    fn new(init: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let world = Self {
            _temp: temp,
            project,
            home,
        };
        if init {
            world.ok(&["init"]);
        }
        world
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
        command.current_dir(&self.project).env("HOME", &self.home);
        command
    }

    fn output(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn page(&self, slug: &str, body: &str) {
        let path = self.project.join(format!("{slug}.md"));
        fs::write(&path, body).unwrap();
        self.ok(&[
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--file",
            path.to_str().unwrap(),
            "--provenance",
            "user-provided",
        ]);
    }

    fn enable_tag(&self, tag: &str, page: &str, tag_priority: &str, max_chars: &str) {
        self.ok(&[
            "tag",
            "set",
            tag,
            page,
            "--priority",
            tag_priority,
            "--reason",
            "core fixture",
        ]);
        self.ok(&[
            "tag",
            "autoload",
            tag,
            "--enable",
            "--priority",
            tag_priority,
            "--limit",
            "10",
            "--max-chars",
            max_chars,
            "--reason",
            "session fixture",
        ]);
    }

    fn install_codegraph_runtime_fixture(&self) {
        let runtime = self.codegraph_runtime_fixture_dir();
        let target = codegraph_target();
        let binary = runtime.join(if cfg!(windows) {
            "bin/codegraph.cmd"
        } else {
            "bin/codegraph"
        });
        fs::create_dir_all(runtime.join("home")).unwrap();
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"fixture").unwrap();
        fs::write(
            runtime.join("runtime.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": CODEGRAPH_VERSION,
                "target": target,
                "asset": format!(
                    "codegraph-{target}.{}",
                    if cfg!(windows) { "zip" } else { "tar.gz" }
                ),
                "archive_sha256": "0".repeat(64),
                "binary": binary.strip_prefix(&runtime).unwrap(),
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn codegraph_runtime_fixture_dir(&self) -> PathBuf {
        self.home
            .join(".lwc/runtime/codegraph")
            .join(CODEGRAPH_VERSION)
            .join(codegraph_target())
    }
}

fn context(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_string()
}

fn readiness(context: &str) -> Value {
    let line = context
        .lines()
        .find_map(|line| line.strip_prefix("LWC_READINESS "))
        .expect("boundary context must contain readiness");
    serde_json::from_str(line).unwrap()
}

fn hook_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn signal_batch(value: &Value) -> Value {
    let context =
        hook_context(value).expect("signal output must use the host's context or stop envelope");
    let line = context
        .lines()
        .find_map(|line| line.strip_prefix("LWC_SIGNAL "))
        .expect("hook context must contain an LWC_SIGNAL batch");
    serde_json::from_str(line).unwrap()
}

fn hook_context(value: &Value) -> Option<&str> {
    if let Some(context) = value.as_str() {
        return Some(context);
    }
    value
        .pointer("/hookSpecificOutput/additionalContext")
        .or_else(|| value.get("additionalContext"))
        .or_else(|| value.get("additional_context"))
        .or_else(|| value.get("context"))
        .or_else(|| value.get("reason"))
        .and_then(Value::as_str)
}

fn create_active_plan(world: &World, title: &str, objective: &str, done_when: &str) -> Value {
    let created = world.ok(&[
        "plan",
        "create",
        title,
        "--objective",
        objective,
        "--done-when",
        done_when,
        "--step",
        "implement the current step",
    ]);
    let id = created["plan"]["id"].as_str().unwrap();
    for (target, session) in [
        ("claude", DEFAULT_CLAUDE_SESSION),
        ("codex", DEFAULT_CODEX_SESSION),
    ] {
        let context = agent_context(target, session, "main", "main");
        let _ = world.command().args([
            "plan", "track", id, "--context", &context,
        ]).output();
    }
    let prompt_context = agent_context(
        "claude",
        DEFAULT_CLAUDE_SESSION,
        "subagent",
        DEFAULT_CLAUDE_PROMPT_AGENT,
    );
    let _ = world.command().args([
        "plan", "track", id, "--context", &prompt_context,
    ]).output();
    created
}

fn agent_context(target: &str, session: &str, subject: &str, actor: &str) -> String {
    let bytes = format!("lwc-agent-context/v1\0{target}\0{session}\0{subject}\0{actor}");
    let digest = Sha256::digest(bytes.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("lwcctx-v1-{digest}")
}

#[test]
fn agent_context_isolates_root_and_child_plan_todo_readiness() {
    let world = World::new(true);
    world.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
    let session = "PRIVATE_SESSION_ROOT_A";
    let root = agent_context("codex", session, "main", "main");
    let root_plan = create_active_plan(&world, "ROOT_A_PLAN", "private", "verified");
    let other_plan = create_active_plan(&world, "OTHER_AGENT_PLAN", "private", "verified");
    let root_plan_id = root_plan["plan"]["id"].as_str().unwrap();
    world.ok(&["plan", "track", root_plan_id, "--context", &root]);
    let root_todo = world.ok(&[
        "todo", "add", "ROOT_A_TODO", "--target-at", "2000-01-01T00:00:00Z",
    ]);
    let root_todo_id = root_todo["todo"]["id"].as_str().unwrap();
    world.ok(&["todo", "track", root_todo_id, "--context", &root]);
    world.ok(&[
        "todo", "add", "OTHER_AGENT_TODO", "--target-at", "2000-01-01T00:00:00Z",
    ]);

    let root_value = tool_hook(
        &world,
        "codex",
        "SessionStart",
        &serde_json::json!({"session_id":session,"source":"startup"}),
    );
    let root_text = hook_context(&root_value).unwrap();
    let root_readiness = readiness(root_text);
    assert_eq!(root_readiness["agent_context"]["status"], "bound");
    assert_eq!(root_readiness["agent_context"]["context_id"], root);
    assert_eq!(root_readiness["plan"]["active"], 1);
    assert_eq!(root_readiness["plan"]["tracking"]["id"], root_plan_id);
    assert_eq!(root_readiness["todo"]["open"], 1);
    assert_eq!(root_readiness["todo"]["reminders"][0]["id"], root_todo_id);
    let rendered = serde_json::to_string(&root_value).unwrap();
    assert!(!rendered.contains(other_plan["plan"]["id"].as_str().unwrap()));
    assert!(!rendered.contains("OTHER_AGENT_PLAN"));
    assert!(!rendered.contains("OTHER_AGENT_TODO"));
    assert!(!rendered.contains(session));
    assert!(root_text.contains(
        "Only follow Plan/Todo progress that is bound to this LWC_READINESS.agent_context"
    ));

    let prompt = prompt_hook_for_session(&world, "continue the current plan", session);
    let prompt_rendered = serde_json::to_string(&prompt).unwrap();
    assert!(!prompt_rendered.contains(root_plan_id));
    assert!(!prompt_rendered.contains(other_plan["plan"]["id"].as_str().unwrap()));
    assert!(!prompt_rendered.contains("OTHER_AGENT_PLAN"));
    let unbound_prompt = serde_json::to_string(&prompt_hook_for_session(
        &world,
        "continue the current plan",
        "UNBOUND_SESSION",
    ))
    .unwrap();
    assert!(!unbound_prompt.contains(root_plan_id));
    assert!(!unbound_prompt.contains(other_plan["plan"]["id"].as_str().unwrap()));
    assert!(!unbound_prompt.contains("ROOT_A_PLAN"));
    assert!(!unbound_prompt.contains("OTHER_AGENT_PLAN"));

    let child_id = "PRIVATE_CHILD_B";
    let child_context = agent_context("codex", session, "subagent", child_id);
    let other_plan_id = other_plan["plan"]["id"].as_str().unwrap();
    world.ok(&["plan", "track", other_plan_id, "--context", &child_context]);
    let child_prompt = tool_hook(
        &world,
        "codex",
        "UserPromptSubmit",
        &serde_json::json!({
            "prompt":"continue the current plan",
            "session_id":session,
            "agent_id":child_id,
        }),
    );
    let child_prompt = serde_json::to_string(&child_prompt).unwrap();
    assert!(child_prompt.contains(other_plan_id));
    assert!(!child_prompt.contains(root_plan_id));
    assert!(!child_prompt.contains(child_id));
    let child_value = tool_hook(
        &world,
        "codex",
        "SubagentStart",
        &serde_json::json!({"session_id":session,"agent_id":child_id}),
    );
    let child_readiness = readiness(hook_context(&child_value).unwrap());
    assert_eq!(child_readiness["agent_context"]["status"], "bound");
    assert_eq!(
        child_readiness["agent_context"]["context_id"],
        child_context
    );
    assert_eq!(child_readiness["plan"]["tracking"]["id"], other_plan_id);
    assert_eq!(child_readiness["todo"]["open"], 0);
    assert!(child_readiness["todo"].get("reminders").is_none());

    let unresolved = tool_hook(
        &world,
        "codex",
        "SubagentStart",
        &serde_json::json!({"session_id":session,"agent_type":"worker"}),
    );
    let unresolved_readiness = readiness(hook_context(&unresolved).unwrap());
    assert_eq!(unresolved_readiness["agent_context"]["status"], "unresolved");
    assert_eq!(
        unresolved_readiness["agent_context"]["reason"],
        "missing_child_id"
    );
    assert!(unresolved_readiness["agent_context"].get("context_id").is_none());
    assert!(unresolved_readiness.get("plan").is_none());
    assert!(unresolved_readiness.get("todo").is_none());
}

#[test]
fn agent_context_stop_continues_only_its_tracked_plan() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let session_a = "session-stop-a";
    let session_b = "session-stop-b";
    let context_a = agent_context("codex", session_a, "main", "main");
    let context_b = agent_context("codex", session_b, "main", "main");
    let plan_a = create_active_plan(&world, "PLAN_A", "private", "verified");
    let plan_b = create_active_plan(&world, "PLAN_B", "private", "verified");
    let plan_a_id = plan_a["plan"]["id"].as_str().unwrap();
    let plan_b_id = plan_b["plan"]["id"].as_str().unwrap();
    world.ok(&["plan", "track", plan_a_id, "--context", &context_a]);
    world.ok(&["plan", "track", plan_b_id, "--context", &context_b]);

    let stopped_a = tool_hook(
        &world,
        "codex",
        "Stop",
        &serde_json::json!({"session_id":session_a,"stop_hook_active":false}),
    );
    let signal_a = &signal_batch(&stopped_a)["signals"][0];
    assert_eq!(signal_a["state"]["id"], plan_a_id);
    let rendered = serde_json::to_string(&stopped_a).unwrap();
    assert!(!rendered.contains(plan_b_id));
    assert!(!rendered.contains("PLAN_B"));

    let unbound = tool_hook(
        &world,
        "codex",
        "Stop",
        &serde_json::json!({"session_id":"session-stop-unbound"}),
    );
    assert_eq!(unbound, serde_json::json!({}));

    let child_id = "child-stop-b";
    let child_context = agent_context("codex", session_a, "subagent", child_id);
    world.ok(&["plan", "track", plan_b_id, "--context", &child_context]);
    let child_stop = tool_hook(
        &world,
        "codex",
        "SubagentStop",
        &serde_json::json!({"session_id":session_a,"agent_id":child_id}),
    );
    assert_eq!(signal_batch(&child_stop)["signals"][0]["state"]["id"], plan_b_id);
    let unresolved_child = tool_hook(
        &world,
        "codex",
        "SubagentStop",
        &serde_json::json!({"session_id":session_a,"agent_type":"worker"}),
    );
    assert_eq!(unresolved_child, serde_json::json!({}));
}

#[test]
fn scope_all_hook_merges_only_the_current_contexts_project_and_global_work() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    world.ok(&["--scope", "global", "init"]);
    world.ok(&["--scope", "global", "config", "set", "--plan", "enabled"]);
    let session = "scope-all-session";
    let context_id = agent_context("codex", session, "main", "main");
    let project_plan = create_active_plan(&world, "PROJECT_PLAN", "p", "p");
    let global_plan = world.ok(&[
        "--scope", "global", "plan", "create", "GLOBAL_PLAN", "--objective", "g",
        "--done-when", "g", "--step", "g",
    ]);
    let other_global = world.ok(&[
        "--scope", "global", "plan", "create", "OTHER_GLOBAL_PLAN", "--objective", "x",
        "--done-when", "x", "--step", "x",
    ]);
    let project_id = project_plan["plan"]["id"].as_str().unwrap();
    let global_id = global_plan["plan"]["id"].as_str().unwrap();
    world.ok(&["plan", "track", project_id, "--context", &context_id]);
    world.ok(&[
        "--scope", "global", "plan", "track", global_id, "--context", &context_id,
    ]);

    let start = hook_json(&world.output(
        &["--scope", "all", "agent", "hook", "--agent", "codex", "--event", "SessionStart"],
        &serde_json::json!({"session_id":session,"source":"startup"}).to_string(),
    ));
    let text = hook_context(&start).unwrap();
    let state = readiness(text);
    assert_eq!(state["plan"]["tracking"]["id"], project_id);
    assert_eq!(state["plan"]["additional_trackings"][0]["id"], global_id);
    let rendered = serde_json::to_string(&start).unwrap();
    assert!(!rendered.contains(other_global["plan"]["id"].as_str().unwrap()));
    assert!(!rendered.contains("OTHER_GLOBAL_PLAN"));

    let stop = hook_json(&world.output(
        &["--scope", "all", "agent", "hook", "--agent", "codex", "--event", "Stop"],
        &serde_json::json!({"session_id":session}).to_string(),
    ));
    let stopped = serde_json::to_string(&stop).unwrap();
    assert!(stopped.contains(project_id));
    assert!(stopped.contains(global_id));
    assert!(!stopped.contains(other_global["plan"]["id"].as_str().unwrap()));
}

fn put_global_autoload_page(world: &World, slug: &str, body: &str) {
    let path = world.project.join(format!("{slug}.md"));
    fs::write(&path, body).unwrap();
    world.ok(&[
        "--scope",
        "global",
        "page",
        "put",
        slug,
        "--title",
        slug,
        "--file",
        path.to_str().unwrap(),
        "--provenance",
        "user-provided",
    ]);
    world.ok(&[
        "--scope",
        "global",
        "tag",
        "set",
        "Rules",
        slug,
        "--priority",
        "1",
        "--reason",
        "scope isolation fixture",
    ]);
    world.ok(&[
        "--scope",
        "global",
        "tag",
        "autoload",
        "Rules",
        "--enable",
        "--priority",
        "1",
        "--limit",
        "10",
        "--max-chars",
        "1000",
        "--reason",
        "scope isolation fixture",
    ]);
}

fn write_work_hook_state(world: &World, id: &str, state: &str, sequence: u64) -> PathBuf {
    let directory = world.project.join(".lwc/work").join(id);
    fs::create_dir_all(&directory).unwrap();
    let phase = match state {
        "queued" => "queued",
        "running" => "projecting",
        "succeeded" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        other => panic!("unsupported Work fixture state {other}"),
    };
    let error = (state == "failed").then(|| {
        serde_json::json!({
            "code": "work_failed",
            "message": "PRIVATE_WORK_ERROR_MESSAGE",
        })
    });
    let path = directory.join("state.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "id": id,
            "kind": "graph-project",
            "scope": "project",
            "database": "/PRIVATE_WORK_DATABASE/wiki.db",
            "state": state,
            "phase": phase,
            "completed": sequence,
            "total": 10,
            "percent": 50.0,
            "sequence": sequence,
            "updated_at_unix_ms": sequence,
            "cancel_requested": state == "cancelled",
            "pid": 4242,
            "message": "PRIVATE_WORK_MESSAGE",
            "result": {"body": "PRIVATE_WORK_RESULT"},
            "error": error,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn stage_changeset_page(world: &World, name: &str, private_body: &str) -> PathBuf {
    world.ok(&["changeset", "begin", name]);
    let file = world.project.join(format!("{name}-private.md"));
    fs::write(&file, private_body).unwrap();
    world.ok(&[
        "--changeset",
        name,
        "page",
        "put",
        "boundary-draft-page",
        "--title",
        "PRIVATE_CHANGESET_TITLE",
        "--file",
        file.to_str().unwrap(),
        "--provenance",
        "agent-observed",
    ]);
    world
        .project
        .join(".lwc/changesets")
        .join(format!("{name}.db"))
}

fn make_changeset_conflict(path: &Path) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE meta SET value=?1 WHERE key='store_id'",
            ["f".repeat(64)],
        )
        .unwrap();
}

fn add_ingest_source(world: &World) -> i64 {
    let path = world.project.join("PRIVATE_INGEST_SOURCE.txt");
    fs::write(&path, "PRIVATE_INGEST_SOURCE_BODY").unwrap();
    world.ok(&[
        "source",
        "add",
        path.to_str().unwrap(),
        "--title",
        "PRIVATE_INGEST_TITLE",
    ])["source"]["id"]
        .as_i64()
        .unwrap()
}

fn set_ingest_status(world: &World, source_id: i64, status: &str) {
    rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute(
            "UPDATE ingest_jobs SET status=?1, attempts=3, last_error=?2 WHERE source_id=?3",
            (status, "PRIVATE_INGEST_ERROR", source_id),
        )
        .unwrap();
}

fn set_memory_pressure(world: &World) {
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-bytes",
        "1000",
    ]);
    rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute("UPDATE memory_state SET logical_bytes=850 WHERE id=1", [])
        .unwrap();
}

fn write_sync_hook_state(world: &World, phase: &str, conflicted: bool) -> PathBuf {
    let session_id = "0123456789abcdef0123456789abcdef";
    let directory = world.project.join(".lwc/sync").join(session_id);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("state.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": session_id,
            "mode": "merge",
            "scope": "project",
            "host": "PRIVATE_SYNC_HOST",
            "remote_directory": "/PRIVATE_SYNC_PATH",
            "phase": phase,
            "conflict_count": if conflicted { 2 } else { 0 },
            "conflict_kinds": if conflicted { serde_json::json!(["page"]) } else { serde_json::json!([]) },
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_digest": null,
            "peer_stores": [],
            "private_payload": "PRIVATE_SYNC_PAYLOAD",
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn live_operation_count(world: &World) -> i64 {
    rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap()
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, String>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(path).unwrap();
            snapshot.insert(relative, format!("symlink:{}", target.to_string_lossy()));
        } else if metadata.is_dir() {
            snapshot.insert(relative, "directory".to_owned());
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), snapshot);
            }
        } else if metadata.is_file() {
            let digest = Sha256::digest(fs::read(path).unwrap());
            let digest = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            snapshot.insert(relative, format!("file:{digest}"));
        } else {
            snapshot.insert(relative, "other".to_owned());
        }
    }

    let mut snapshot = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut snapshot);
    }
    snapshot
}

fn boundary_hook(world: &World) -> Value {
    hook_json(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"source": "resume", "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ))
}

fn tool_hook(world: &World, agent: &str, event: &str, payload: &Value) -> Value {
    let mut payload = payload.clone();
    if event == "Stop" && payload.get("session_id").is_none() {
        let session = match agent {
            "claude" => Some(DEFAULT_CLAUDE_SESSION),
            "codex" => Some(DEFAULT_CODEX_SESSION),
            _ => None,
        };
        if let Some(session) = session {
            payload["session_id"] = serde_json::json!(session);
        }
    }
    hook_json(&world.output(
        &["agent", "hook", "--agent", agent, "--event", event],
        &payload.to_string(),
    ))
}

fn prompt_hook(world: &World, prompt: &str) -> Value {
    tool_hook(
        world,
        "claude",
        "UserPromptSubmit",
        &serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt,
            "session_id": DEFAULT_CLAUDE_SESSION,
            "agent_id": DEFAULT_CLAUDE_PROMPT_AGENT,
        }),
    )
}

fn prompt_hook_for_session(world: &World, prompt: &str, session_id: &str) -> Value {
    tool_hook(
        world,
        "codex",
        "UserPromptSubmit",
        &serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": prompt,
            "session_id": session_id,
        }),
    )
}

fn prompt_hook_with_env(
    world: &World,
    agent: &str,
    event: &str,
    input: &str,
    environment: &[(&str, &str)],
) -> Value {
    let mut command = world.command();
    command
        .args(["agent", "hook", "--agent", agent, "--event", event])
        .envs(environment.iter().copied());
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    hook_json(&child.wait_with_output().unwrap())
}

fn prompt_hook_without_home(world: &World, prompt: &str) -> Value {
    let mut command = world.command();
    for variable in ["HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH"] {
        command.env_remove(variable);
    }
    let mut child = command
        .args([
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "UserPromptSubmit",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::json!({"prompt": prompt}).to_string().as_bytes())
        .unwrap();
    hook_json(&child.wait_with_output().unwrap())
}

fn work_receipt_stdout(id: &str, state: &str) -> String {
    serde_json::json!({
        "work": {
            "id": id,
            "state": state,
            "phase": if state == "running" { "projecting" } else { state },
            "completed": if state == "succeeded" { 10 } else { 3 },
            "total": 10,
            "sequence": 7,
            "database": "/PRIVATE_TOOL_DATABASE/wiki.db",
            "message": "PRIVATE_TOOL_MESSAGE",
            "result": {"secret": "PRIVATE_TOOL_RESULT"},
            "error": {"message": "PRIVATE_TOOL_ERROR"},
        },
        "prompt": "PRIVATE_TOOL_PROMPT",
    })
    .to_string()
}

fn assert_signal_shape(batch: &Value, event: &str) {
    assert_eq!(batch["schema"], "lwc.signal/v1");
    assert_eq!(batch["event"], event);
    assert!(batch["omitted"].as_u64().is_some());
    for signal in batch["signals"].as_array().unwrap() {
        assert!(signal["kind"].as_str().is_some());
        assert!(matches!(
            signal["priority"].as_u64(),
            Some(100 | 80 | 60 | 20)
        ));
        assert!(signal["why_now"].as_str().is_some());
        assert!(signal["summary"].as_str().is_some());
        assert!(signal["requires_consent"].as_bool().is_some());
        assert!(matches!(
            signal["completion_effect"].as_str(),
            Some("none" | "requires_followup" | "satisfies_followup" | "continue_once")
        ));
    }
}

#[test]
fn signal_prompt_disabled_and_idle_catalog_is_concrete_and_bounded() {
    let world = World::new(true);
    world.ok(&["config", "set", "--memory", "disabled"]);
    fs::create_dir_all(world.project.join("src")).unwrap();
    fs::write(world.project.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();

    let cases = [
        (
            "create a plan",
            "plan.enable",
            20,
            true,
            "lwc --scope project config set --plan enabled",
        ),
        (
            "review my todo list",
            "todo.enable",
            20,
            true,
            "lwc --scope project config set --todo enabled",
        ),
        (
            "show background work",
            "work.review",
            20,
            false,
            "lwc work list",
        ),
        (
            "sync this project",
            "sync.start",
            20,
            true,
            "lwc --scope project sync HOST ABS_DIRECTORY --mode merge",
        ),
        (
            "ingest a source",
            "ingest.start",
            20,
            false,
            "lwc source add PATH",
        ),
        (
            "start a changeset",
            "changeset.start",
            20,
            false,
            "lwc changeset begin NAME",
        ),
        (
            "remember this decision",
            "memory.enable",
            20,
            true,
            "lwc --scope project config set --memory enabled",
        ),
        (
            "search the project wiki",
            "wiki.search",
            20,
            false,
            "lwc search QUERY --limit 20",
        ),
        (
            "show document relationships",
            "graph.document.enable",
            20,
            true,
            "lwc --scope project config set --graph grafeo",
        ),
        (
            "show the code structure",
            "graph.code.enable",
            20,
            true,
            "lwc --scope project cg init",
        ),
        (
            "teach me Rust",
            "tutor.enable",
            20,
            true,
            "lwc --scope global config set --tutor enabled",
        ),
        (
            "start practice questions",
            "practice.enable",
            20,
            true,
            "lwc --scope global config set --practice enabled",
        ),
        (
            "read this book.epub",
            "book.enable",
            20,
            true,
            "lwc --scope global config set --book enabled",
        ),
        (
            "open report.docx",
            "office.enable",
            20,
            true,
            "lwc --scope global config set --office officecli",
        ),
        (
            "convert report.pdf to markdown",
            "trans.configure",
            20,
            true,
            "lwc --scope project config set --trans ENGINE",
        ),
    ];

    for (prompt, kind, priority, consent, next_action) in cases {
        let value = prompt_hook(&world, prompt);
        let batch = signal_batch(&value);
        assert_signal_shape(&batch, "prompt");
        let signals = batch["signals"].as_array().unwrap();
        assert_eq!(signals.len(), 1, "{prompt}: {batch}");
        let signal = &signals[0];
        assert_eq!(signal["kind"], kind, "{prompt}: {batch}");
        assert_eq!(signal["priority"], priority, "{prompt}: {batch}");
        assert_eq!(signal["requires_consent"], consent, "{prompt}: {batch}");
        assert_eq!(signal["completion_effect"], "none", "{prompt}: {batch}");
        assert_eq!(signal["next_action"], next_action, "{prompt}: {batch}");
    }

    let batch = signal_batch(&prompt_hook(
        &world,
        "create a plan, add a todo, and sync this project",
    ));
    assert_eq!(batch["signals"].as_array().unwrap().len(), 1, "{batch}");
    assert_eq!(batch["signals"][0]["kind"], "plan.enable");
    assert_eq!(batch["omitted"], 2);
}

#[test]
fn signal_prompt_maps_active_provider_states_without_mutation_or_private_fields() {
    let idle_plan = World::new(true);
    idle_plan.ok(&["config", "set", "--plan", "enabled"]);
    let signal = &signal_batch(&prompt_hook(&idle_plan, "create a plan"))["signals"][0];
    assert_eq!(signal["kind"], "plan.start");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], false);

    let plan = World::new(true);
    plan.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &plan,
        "PRIVATE_PROMPT_PLAN_TITLE",
        "PRIVATE_PROMPT_PLAN_OBJECTIVE",
        "PRIVATE_PROMPT_PLAN_DONE_WHEN",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let before = plan.ok(&["plan", "show", plan_id]);
    let value = prompt_hook(&plan, "continue the current plan");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "plan.resume");
    assert_eq!(signal["priority"], 80);
    assert_eq!(signal["completion_effect"], "none");
    assert_eq!(signal["next_action"], format!("lwc plan brief {plan_id}"));
    let rendered = serde_json::to_string(&value).unwrap();
    for secret in [
        "PRIVATE_PROMPT_PLAN_OBJECTIVE",
        "PRIVATE_PROMPT_PLAN_DONE_WHEN",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert_eq!(plan.ok(&["plan", "show", plan_id]), before);
    create_active_plan(&plan, "second plan", "second objective", "second done");
    let signal = &signal_batch(&prompt_hook(&plan, "continue the current plan"))["signals"][0];
    assert_eq!(signal["kind"], "plan.resume");
    assert_eq!(signal["priority"], 80);
    assert_eq!(signal["state"]["id"], plan_id);

    let blocked = World::new(true);
    blocked.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(&blocked, "blocked", "wait", "human replies");
    let blocked_id = created["plan"]["id"].as_str().unwrap();
    let step_id = created["plan"]["steps"][0]["id"].as_str().unwrap();
    blocked.ok(&[
        "plan",
        "block",
        blocked_id,
        "--if-revision",
        "1",
        "--step",
        step_id,
        "--reason",
        "PRIVATE_BLOCK_REASON",
    ]);
    let value = prompt_hook(&blocked, "continue the current plan");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "plan.blocked");
    assert_eq!(signal["priority"], 60);
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("PRIVATE_BLOCK_REASON")
    );

    let todo = World::new(true);
    todo.ok(&["config", "set", "--todo", "enabled"]);
    let signal = &signal_batch(&prompt_hook(&todo, "review my todo list"))["signals"][0];
    assert_eq!(signal["kind"], "todo.review");
    assert_eq!(signal["priority"], 20);
    let due = todo.ok(&[
        "todo",
        "add",
        "PRIVATE_DUE_TODO_TITLE",
        "--target-at",
        "2000-01-01T00:00:00Z",
        "--cue",
        "PRIVATE_DUE_TODO_CUE",
    ]);
    let context_id = agent_context(
        "claude",
        DEFAULT_CLAUDE_SESSION,
        "subagent",
        DEFAULT_CLAUDE_PROMPT_AGENT,
    );
    todo.ok(&[
        "todo", "track", due["todo"]["id"].as_str().unwrap(), "--context", &context_id,
    ]);
    let value = prompt_hook(&todo, "review my todo list");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "todo.due");
    assert_eq!(signal["priority"], 60);
    assert!(
        !serde_json::to_string(&signal)
            .unwrap()
            .contains("PRIVATE_DUE_TODO_CUE")
    );

    let work = World::new(true);
    let work_id = "d".repeat(64);
    let work_path = write_work_hook_state(&work, &work_id, "failed", 7);
    let before = fs::read(&work_path).unwrap();
    let value = prompt_hook(&work, "show background work");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "work.failed");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["completion_effect"], "none");
    assert_eq!(signal["next_action"], format!("lwc work status {work_id}"));
    let rendered = serde_json::to_string(&value).unwrap();
    assert!(!rendered.contains("PRIVATE_WORK_DATABASE"));
    assert!(!rendered.contains("PRIVATE_WORK_ERROR_MESSAGE"));
    assert_eq!(fs::read(work_path).unwrap(), before);

    let running_work = World::new(true);
    let running_id = "e".repeat(64);
    write_work_hook_state(&running_work, &running_id, "running", 3);
    let signal = &signal_batch(&prompt_hook(&running_work, "show background work"))["signals"][0];
    assert_eq!(signal["kind"], "work.resume");
    assert_eq!(signal["priority"], 80);

    let sync = World::new(true);
    let sync_path = write_sync_hook_state(&sync, "applying", true);
    let before = fs::read(&sync_path).unwrap();
    let value = prompt_hook(&sync, "resume project sync");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "sync.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["completion_effect"], "none");
    let rendered = serde_json::to_string(&value).unwrap();
    for secret in [
        "PRIVATE_SYNC_HOST",
        "/PRIVATE_SYNC_PATH",
        "PRIVATE_SYNC_PAYLOAD",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert_eq!(fs::read(sync_path).unwrap(), before);
    let pending_sync = World::new(true);
    write_sync_hook_state(&pending_sync, "applying", false);
    let signal = &signal_batch(&prompt_hook(&pending_sync, "resume project sync"))["signals"][0];
    assert_eq!(signal["kind"], "sync.resume");
    assert_eq!(signal["priority"], 80);

    let ingest = World::new(true);
    let source_id = add_ingest_source(&ingest);
    let value = prompt_hook(&ingest, "ingest a source");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "ingest.resume");
    assert_eq!(signal["priority"], 80);
    assert_eq!(
        signal["next_action"],
        format!("lwc ingest claim {source_id}")
    );
    set_ingest_status(&ingest, source_id, "analyzing");
    let signal = &signal_batch(&prompt_hook(&ingest, "resume ingestion"))["signals"][0];
    assert_eq!(signal["kind"], "ingest.resume");
    assert_eq!(signal["priority"], 80);
    set_ingest_status(&ingest, source_id, "failed");
    let value = prompt_hook(&ingest, "resume ingestion");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "ingest.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(
        signal["next_action"],
        format!("lwc ingest retry {source_id}")
    );
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("PRIVATE_INGEST_SOURCE_BODY")
    );

    let changeset = World::new(true);
    let database =
        stage_changeset_page(&changeset, "prompt-draft", "PRIVATE_PROMPT_CHANGESET_BODY");
    let value = prompt_hook(&changeset, "continue the changeset");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "changeset.resume");
    assert_eq!(signal["priority"], 80);
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("PRIVATE_PROMPT_CHANGESET_BODY")
    );
    make_changeset_conflict(&database);
    let signal = &signal_batch(&prompt_hook(&changeset, "continue the changeset"))["signals"][0];
    assert_eq!(signal["kind"], "changeset.recovery");
    assert_eq!(signal["priority"], 100);
}

#[test]
fn signal_ingest_catalog_has_fixed_priority_and_pending_is_continuity() {
    let idle = World::new(true);
    let idle_signal = signal_batch(&prompt_hook(&idle, "ingest a source"))["signals"][0].clone();

    let pending = World::new(true);
    let pending_source = add_ingest_source(&pending);
    let pending_signal =
        signal_batch(&prompt_hook(&pending, "ingest a source"))["signals"][0].clone();

    let active = World::new(true);
    let active_source = add_ingest_source(&active);
    set_ingest_status(&active, active_source, "analyzing");
    let active_signal =
        signal_batch(&prompt_hook(&active, "resume ingestion"))["signals"][0].clone();

    let mut priorities = BTreeMap::<String, u64>::new();
    for signal in [&idle_signal, &pending_signal, &active_signal] {
        let kind = signal["kind"].as_str().unwrap().to_owned();
        let priority = signal["priority"].as_u64().unwrap();
        if let Some(existing) = priorities.insert(kind.clone(), priority) {
            assert_eq!(existing, priority, "kind {kind} changed priority");
        }
    }
    assert_eq!(idle_signal["kind"], "ingest.start");
    assert_eq!(idle_signal["priority"], 20);
    assert_eq!(pending_signal["kind"], "ingest.resume");
    assert_eq!(pending_signal["priority"], 80);
    assert_eq!(pending_signal["completion_effect"], "none");
    assert_eq!(
        pending_signal["state"]["jobs"][0]["source_id"],
        pending_source
    );
    assert_eq!(
        pending_signal["next_action"],
        format!("lwc ingest claim {pending_source}")
    );
    assert_eq!(active_signal["kind"], "ingest.resume");
    assert_eq!(active_signal["priority"], 80);
}

#[test]
fn signal_prompt_memory_learning_book_and_conversion_catalog_obeys_precedence() {
    let memory = World::new(true);
    memory.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-bytes",
        "1000",
    ]);
    for (prompt, kind, next_action) in [
        (
            "remember this decision",
            "memory.record",
            "lwc remember --json '{...}'",
        ),
        (
            "search memory",
            "memory.recall",
            "lwc memory recall QUERY --limit 5",
        ),
        ("memory status", "memory.status", "lwc memory status"),
    ] {
        let signal = &signal_batch(&prompt_hook(&memory, prompt))["signals"][0];
        assert_eq!(signal["kind"], kind, "{prompt}: {signal}");
        assert_eq!(signal["priority"], 20, "{prompt}: {signal}");
        assert_eq!(signal["next_action"], next_action, "{prompt}: {signal}");
        assert_eq!(signal["completion_effect"], "none");
    }
    assert_eq!(
        prompt_hook(&memory, "use durable memory"),
        serde_json::json!({}),
        "ready Memory with no exact subtype stays silent"
    );
    rusqlite::Connection::open(memory.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute("UPDATE memory_state SET logical_bytes=900 WHERE id=1", [])
        .unwrap();
    let value = prompt_hook(&memory, "memory status");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "memory.maintenance");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["next_action"], "lwc memory maintain");
    let rendered = serde_json::to_string(signal).unwrap();
    assert!(!rendered.contains("database"));
    assert!(!rendered.contains("memory_events"));

    let learning = World::new(true);
    learning.ok(&["--scope", "global", "init"]);
    learning.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--tutor",
        "enabled",
        "--practice",
        "enabled",
        "--book",
        "enabled",
    ]);
    for (prompt, kind, next_action) in [
        (
            "teach me Rust",
            "tutor.start",
            "lwc tutor session create --json '{...}'",
        ),
        (
            "start practice questions",
            "practice.start",
            "lwc practice next --json '{...}'",
        ),
        (
            "read this book.epub",
            "book.start",
            "lwc book import --json '{...}'",
        ),
    ] {
        let signal = &signal_batch(&prompt_hook(&learning, prompt))["signals"][0];
        assert_eq!(signal["kind"], kind, "{prompt}: {signal}");
        assert_eq!(signal["priority"], 20, "{prompt}: {signal}");
        assert_eq!(signal["requires_consent"], false);
        assert_eq!(signal["next_action"], next_action);
    }
    let signal = &signal_batch(&prompt_hook(&learning, "read this book.mobi"))["signals"][0];
    assert_eq!(signal["kind"], "book.format_unsupported");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["requires_consent"], false);

    let conversion = World::new(true);
    let signal =
        &signal_batch(&prompt_hook(&conversion, "convert report.docx to markdown"))["signals"][0];
    assert_eq!(signal["kind"], "trans.configure");
    assert_ne!(signal["kind"], "office.enable");
    let signal = &signal_batch(&prompt_hook(&conversion, "open report.docx"))["signals"][0];
    assert_eq!(signal["kind"], "office.enable");
    conversion.ok(&["--scope", "global", "init"]);
    conversion.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--office",
        "officecli",
    ]);
    let signal = &signal_batch(&prompt_hook(&conversion, "open report.docx"))["signals"][0];
    assert_eq!(signal["kind"], "office.use");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], false);
    conversion.ok(&["config", "set", "--trans", "markitdown"]);
    let signal =
        &signal_batch(&prompt_hook(&conversion, "convert report.pdf to markdown"))["signals"][0];
    assert_eq!(signal["kind"], "trans.runtime");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["requires_consent"], false);
}

#[cfg(unix)]
#[test]
fn signal_prompt_uses_a_configured_available_trans_runtime_without_reading_the_file() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new(true);
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let bin = world.home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let executable = bin.join("markitdown");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let private = world.project.join("PRIVATE_REPORT.pdf");
    fs::write(&private, "PRIVATE_REPORT_BODY").unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let payload = serde_json::json!({
        "prompt": format!("convert {} to markdown", private.display()),
        "transcript_path": "/PRIVATE_TRANSCRIPT_PATH",
    })
    .to_string();
    let value = prompt_hook_with_env(
        &world,
        "claude",
        "UserPromptSubmit",
        &payload,
        &[("PATH", path.as_str())],
    );
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "trans.convert");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], false);
    assert_eq!(
        signal["next_action"],
        "lwc --scope project trans INPUT --output OUTPUT.md"
    );
    let rendered = serde_json::to_string(&value).unwrap();
    assert!(!rendered.contains("PRIVATE_REPORT.pdf"));
    assert!(!rendered.contains("PRIVATE_REPORT_BODY"));
    assert!(!rendered.contains("PRIVATE_TRANSCRIPT_PATH"));
    assert_eq!(fs::read_to_string(private).unwrap(), "PRIVATE_REPORT_BODY");
}

#[test]
fn signal_prompt_graph_routing_handles_dual_pending_ready_and_recovery_states() {
    let dual = World::new(true);
    fs::create_dir_all(dual.project.join("src")).unwrap();
    fs::write(dual.project.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
    let batch = signal_batch(&prompt_hook(
        &dual,
        "show both document relationships and code structure",
    ));
    assert_eq!(batch["signals"].as_array().unwrap().len(), 1, "{batch}");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "graph.enable");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], true);
    assert_eq!(signal["next_action"], "Reply with 1-4.");

    let pending = World::new(true);
    fs::write(
        pending.project.join(".lwc/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {"setting": "grafeo"},
            "trans": {
                "setting": "inherit",
                "timeout_seconds": 120,
                "anydoc_args": [],
                "markitdown_args": []
            }
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        prompt_hook(&pending, "show document relationships"),
        serde_json::json!({}),
        "an enabled graph with pending projection Work stays silent"
    );

    let ready = World::new(true);
    ready.page("graph-ready", "bounded graph fixture");
    let enabled = ready.ok(&["config", "set", "--graph", "grafeo"]);
    if let Some(work_id) = enabled.pointer("/work/id").and_then(Value::as_str) {
        ready.ok(&["work", "watch", work_id]);
    }
    let signal = &signal_batch(&prompt_hook(&ready, "show document relationships"))["signals"][0];
    assert_eq!(signal["kind"], "graph.document.explore");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], false);
    assert_eq!(signal["next_action"], "lwc --scope project graph explore");

    let document_error = World::new(true);
    fs::write(
        document_error.project.join(".lwc/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {"setting": "grafeo"},
            "trans": {
                "setting": "inherit",
                "timeout_seconds": 120,
                "anydoc_args": [],
                "markitdown_args": []
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(document_error.project.join(".lwc/graph-grafeo"), b"broken").unwrap();
    let signal =
        &signal_batch(&prompt_hook(&document_error, "show document relationships"))["signals"][0];
    assert_eq!(signal["kind"], "graph.document.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["requires_consent"], false);

    let code_recovery = World::new(true);
    fs::create_dir_all(code_recovery.project.join("src")).unwrap();
    fs::write(
        code_recovery.project.join("src/lib.rs"),
        "pub fn marker() {}\n",
    )
    .unwrap();
    fs::create_dir_all(code_recovery.project.join(".lwc/codegraph")).unwrap();
    fs::write(
        code_recovery.project.join(".lwc/codegraph/codegraph.db"),
        b"fixture",
    )
    .unwrap();
    let signal =
        &signal_batch(&prompt_hook(&code_recovery, "show the code structure"))["signals"][0];
    assert_eq!(signal["kind"], "graph.code.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["requires_consent"], false);
    assert_eq!(signal["next_action"], "lwc --scope project cg status");

    code_recovery.install_codegraph_runtime_fixture();
    let signal =
        &signal_batch(&prompt_hook(&code_recovery, "show the code structure"))["signals"][0];
    assert_eq!(signal["kind"], "graph.code.explore");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], false);
    assert_eq!(signal["next_action"], "lwc --scope project cg search QUERY");
}

#[test]
fn external_graph_hook_status_is_bounded_and_never_mutates_enabled_sidecars() {
    for engine in ["grafeo", "surrealdb"] {
        let world = World::new(true);
        world.page(
            "hook-graph-fixture",
            "PRIVATE_GRAPH_BODY_MUST_NOT_LEAK_OR_CHANGE",
        );
        let enabled = world.ok(&["config", "set", "--graph", engine]);
        if let Some(work_id) = enabled.pointer("/work/id").and_then(Value::as_str) {
            let completed = world.ok(&["work", "watch", work_id]);
            assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
        }
        let sidecar = world.project.join(".lwc").join(format!("graph-{engine}"));
        assert!(sidecar.exists(), "missing {engine} sidecar");
        if engine == "grafeo" {
            if sidecar.is_dir() {
                fs::remove_dir_all(&sidecar).unwrap();
            } else {
                fs::remove_file(&sidecar).unwrap();
            }
            let staging = world.project.join(".lwc/hook-fixture.grafeo");
            let graph = grafeo::GrafeoDB::open(&staging).unwrap();
            graph.close().unwrap();
            fs::rename(staging, &sidecar).unwrap();
        }
        let before = snapshot_tree(&sidecar);

        let started = Instant::now();
        let boundary = boundary_hook(&world);
        assert!(started.elapsed() < Duration::from_millis(1_900));
        let projection =
            &readiness(hook_context(&boundary).unwrap())["document_graph"]["projection"];
        assert_eq!(projection["engine"], engine, "{projection}");
        if engine == "grafeo" {
            assert_eq!(projection["status"], "ready", "{projection}");
            assert_eq!(projection["documents"], 0, "{projection}");
        } else {
            assert_eq!(projection["status"], "unverified", "{projection}");
            assert!(projection.get("documents").is_none(), "{projection}");
        }

        let started = Instant::now();
        let prompt = prompt_hook(&world, "show document relationships");
        assert!(started.elapsed() < Duration::from_millis(1_900));
        let signal = &signal_batch(&prompt)["signals"][0];
        assert_eq!(
            signal["kind"],
            if engine == "grafeo" {
                "graph.document.explore"
            } else {
                "graph.document.recovery"
            },
            "{signal}"
        );
        assert_eq!(
            signal["priority"],
            if engine == "grafeo" { 20 } else { 100 },
            "{signal}"
        );
        assert!(
            !serde_json::to_string(&prompt)
                .unwrap()
                .contains("PRIVATE_GRAPH_BODY_MUST_NOT_LEAK_OR_CHANGE")
        );
        assert_eq!(
            snapshot_tree(&sidecar),
            before,
            "{engine} Hook probes changed project state"
        );
    }
}

#[test]
fn external_graph_hook_errors_are_isolated_from_plan_signals_without_writes() {
    let world = World::new(true);
    world.page("graph-error-fixture", "bounded fixture");
    let enabled = world.ok(&["config", "set", "--graph", "grafeo"]);
    if let Some(work_id) = enabled.pointer("/work/id").and_then(Value::as_str) {
        world.ok(&["work", "watch", work_id]);
    }
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&world, "graph probe isolation", "continue", "verified");

    let sidecar = world.project.join(".lwc/graph-grafeo");
    if sidecar.is_dir() {
        fs::remove_dir_all(&sidecar).unwrap();
    } else if sidecar.exists() {
        fs::remove_file(&sidecar).unwrap();
    }
    fs::write(
        &sidecar,
        b"PRIVATE_LEGACY_GRAPH_PAYLOAD PRIVATE_LEGACY_GRAPH_BODY",
    )
    .unwrap();
    let before = snapshot_tree(&sidecar);

    let started = Instant::now();
    let boundary = boundary_hook(&world);
    assert!(started.elapsed() < Duration::from_millis(1_900));
    let context = hook_context(&boundary).unwrap();
    let readiness = readiness(context);
    assert_eq!(readiness["document_graph"]["projection"]["status"], "error");
    assert_eq!(
        readiness["document_graph"]["projection"]["error_code"],
        "graph_hook_unavailable"
    );
    let boundary_batch = signal_batch(&boundary);
    let kinds = boundary_batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|signal| signal["kind"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(kinds.contains("plan.resume"), "{boundary}");

    let started = Instant::now();
    let prompt = prompt_hook(
        &world,
        "continue the current plan and show document relationships",
    );
    assert!(started.elapsed() < Duration::from_millis(1_900));
    let prompt_batch = signal_batch(&prompt);
    let kinds = prompt_batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|signal| signal["kind"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(kinds.contains("plan.resume"), "{prompt}");
    assert!(kinds.contains("graph.document.recovery"), "{prompt}");
    let recovery = prompt_batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["kind"] == "graph.document.recovery")
        .unwrap();
    assert_eq!(recovery["priority"], 100, "{recovery}");
    let rendered = serde_json::to_string(&prompt).unwrap();
    assert!(!rendered.contains("PRIVATE_LEGACY_GRAPH_PAYLOAD"));
    assert!(!rendered.contains("PRIVATE_LEGACY_GRAPH_BODY"));
    assert_eq!(snapshot_tree(&sidecar), before);
}

#[cfg(unix)]
#[test]
fn symlinked_graph_hook_sidecar_is_not_followed_and_does_not_hide_plan() {
    use std::os::unix::fs::symlink;

    let world = World::new(true);
    let enabled = world.ok(&["config", "set", "--graph", "grafeo"]);
    if let Some(work_id) = enabled.pointer("/work/id").and_then(Value::as_str) {
        world.ok(&["work", "watch", work_id]);
    }
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&world, "symlink graph isolation", "continue", "verified");

    let sidecar = world.project.join(".lwc/graph-grafeo");
    if sidecar.is_dir() {
        fs::remove_dir_all(&sidecar).unwrap();
    } else if sidecar.exists() {
        fs::remove_file(&sidecar).unwrap();
    }
    let outside = world.project.join("PRIVATE_OUTSIDE_GRAPH.grafeo");
    let graph = grafeo::GrafeoDB::open(&outside).unwrap();
    graph.close().unwrap();
    let outside_before = snapshot_tree(&outside);
    symlink(&outside, &sidecar).unwrap();

    let started = Instant::now();
    let boundary = boundary_hook(&world);
    assert!(started.elapsed() < Duration::from_millis(1_900));
    let context = hook_context(&boundary).unwrap();
    let readiness = readiness(context);
    assert_eq!(
        readiness["document_graph"]["projection"],
        serde_json::json!({
            "status": "error",
            "error_code": "graph_hook_unavailable",
        })
    );
    assert_eq!(signal_batch(&boundary)["signals"][0]["kind"], "plan.resume");

    let started = Instant::now();
    let prompt = prompt_hook(
        &world,
        "continue the current plan and show document relationships",
    );
    assert!(started.elapsed() < Duration::from_millis(1_900));
    let batch = signal_batch(&prompt);
    let recovery = batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signal| signal["kind"] == "graph.document.recovery")
        .unwrap();
    assert_eq!(recovery["priority"], 100, "{recovery}");
    assert!(
        batch["signals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|signal| signal["kind"] == "plan.resume"),
        "{batch}"
    );
    let rendered = serde_json::to_string(&prompt).unwrap();
    assert!(!rendered.contains("PRIVATE_OUTSIDE_GRAPH"));
    assert_eq!(snapshot_tree(&outside), outside_before);
}

#[test]
fn signal_prompt_graph_mixed_states_prioritize_only_the_missing_consent() {
    let document_missing = World::new(true);
    fs::create_dir_all(document_missing.project.join("src")).unwrap();
    fs::write(
        document_missing.project.join("src/lib.rs"),
        "pub fn marker() {}\n",
    )
    .unwrap();
    fs::create_dir_all(document_missing.project.join(".lwc/codegraph")).unwrap();
    fs::write(
        document_missing.project.join(".lwc/codegraph/codegraph.db"),
        b"fixture",
    )
    .unwrap();
    document_missing.install_codegraph_runtime_fixture();
    let batch = signal_batch(&prompt_hook(
        &document_missing,
        "show both document relationships and code structure",
    ));
    assert_eq!(batch["signals"].as_array().unwrap().len(), 1, "{batch}");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "graph.document.enable", "{batch}");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], true);

    let code_missing = World::new(true);
    fs::create_dir_all(code_missing.project.join("src")).unwrap();
    fs::write(
        code_missing.project.join("src/lib.rs"),
        "pub fn marker() {}\n",
    )
    .unwrap();
    code_missing.page("graph-ready", "bounded graph fixture");
    let enabled = code_missing.ok(&["config", "set", "--graph", "grafeo"]);
    if let Some(work_id) = enabled.pointer("/work/id").and_then(Value::as_str) {
        code_missing.ok(&["work", "watch", work_id]);
    }
    let batch = signal_batch(&prompt_hook(
        &code_missing,
        "show both document relationships and code structure",
    ));
    assert_eq!(batch["signals"].as_array().unwrap().len(), 1, "{batch}");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "graph.code.enable", "{batch}");
    assert_eq!(signal["priority"], 20);
    assert_eq!(signal["requires_consent"], true);
}

#[test]
fn signal_prompt_uses_only_exact_current_input_and_context_capable_hosts() {
    let world = World::new(true);
    let prompt = "create a plan; PRIVATE_PROMPT_PATH=/private/secret.md";
    let cases = [
        (
            "claude",
            "UserPromptSubmit",
            serde_json::json!({"prompt": prompt}),
            "prompt",
        ),
        (
            "codex",
            "UserPromptSubmit",
            serde_json::json!({"prompt": prompt}),
            "prompt",
        ),
        (
            "gemini",
            "BeforeAgent",
            serde_json::json!({"prompt": prompt}),
            "turn_start",
        ),
        (
            "hermes",
            "pre_llm_call",
            serde_json::json!({"extra": {"user_message": prompt}}),
            "prompt",
        ),
        (
            "pi",
            "before_agent_start",
            serde_json::json!({"prompt": prompt}),
            "turn_start",
        ),
    ];
    for (agent, event, payload, semantic) in cases {
        let value = tool_hook(&world, agent, event, &payload);
        if agent == "gemini" {
            assert_eq!(value["hookSpecificOutput"]["hookEventName"], "BeforeAgent");
            let output = value["hookSpecificOutput"].as_object().unwrap();
            assert_eq!(output.len(), 2, "unexpected Gemini schema: {value}");
            assert!(output["additionalContext"].as_str().is_some());
        }
        let batch = signal_batch(&value);
        assert_signal_shape(&batch, semantic);
        assert_eq!(batch["signals"][0]["kind"], "plan.enable", "{agent}");
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(
            !rendered.contains("PRIVATE_PROMPT_PATH"),
            "{agent}: {rendered}"
        );
        assert!(
            !rendered.contains("/private/secret.md"),
            "{agent}: {rendered}"
        );
        assert!(!rendered.contains("LWC_READINESS "), "{agent}: {rendered}");
    }

    let kiro = prompt_hook_with_env(
        &world,
        "kiro",
        "UserPromptSubmit",
        "",
        &[("USER_PROMPT", prompt)],
    );
    let batch = signal_batch(&kiro);
    assert_eq!(batch["event"], "prompt");
    assert_eq!(batch["signals"][0]["kind"], "plan.enable");

    let transcript = world.project.join("private-transcript.jsonl");
    fs::write(&transcript, "create a plan PRIVATE_TRANSCRIPT_BODY").unwrap();
    let rejected = [
        (
            "claude",
            "UserPromptSubmit",
            serde_json::json!({"messages": [{"content": "create a plan"}]}),
        ),
        (
            "claude",
            "UserPromptSubmit",
            serde_json::json!({"transcript_path": transcript}),
        ),
        (
            "claude",
            "UserPromptSubmit",
            serde_json::json!({"transformedPrompt": "create a plan"}),
        ),
        (
            "cursor",
            "beforeSubmitPrompt",
            serde_json::json!({"prompt": "create a plan"}),
        ),
        (
            "copilot-vscode",
            "UserPromptSubmit",
            serde_json::json!({"prompt": "create a plan"}),
        ),
        (
            "copilot-cli",
            "userPromptSubmitted",
            serde_json::json!({"prompt": "create a plan"}),
        ),
        (
            "copilot-cli",
            "userPromptTransformed",
            serde_json::json!({"prompt": "create a plan"}),
        ),
        (
            "antigravity",
            "PreInvocation",
            serde_json::json!({"prompt": "create a plan"}),
        ),
    ];
    for (agent, event, payload) in rejected {
        assert_eq!(
            tool_hook(&world, agent, event, &payload),
            serde_json::json!({}),
            "{agent}/{event} must not infer current input"
        );
    }
    assert_eq!(
        prompt_hook_with_env(
            &world,
            "kiro",
            "UserPromptSubmit",
            "{}",
            &[("USER_PROMPT", "create a plan")],
        ),
        serde_json::json!({}),
        "Kiro environment fallback is valid only when stdin is empty"
    );
    let long_environment = format!("{}create a plan", "x".repeat(4_096));
    assert_eq!(
        prompt_hook_with_env(
            &world,
            "kiro",
            "UserPromptSubmit",
            "",
            &[("USER_PROMPT", long_environment.as_str())],
        ),
        serde_json::json!({}),
        "Kiro environment classification is bounded to 4096 chars"
    );
    assert_eq!(
        prompt_hook(&world, &format!("{}create a plan", "x".repeat(4_096))),
        serde_json::json!({}),
        "classification never reads after 4096 chars"
    );
    assert_eq!(prompt_hook(&world, "say hello"), serde_json::json!({}));
    assert_eq!(
        fs::read_to_string(transcript).unwrap(),
        "create a plan PRIVATE_TRANSCRIPT_BODY"
    );
}

#[test]
fn signal_prompt_provider_errors_are_isolated_and_prompt_hooks_stay_read_only() {
    let world = World::new(true);
    world.page("strong-secret", "PRIVATE_STRONG_PAGE_BODY");
    world.enable_tag("Rules", "strong-secret", "10", "1000");
    let database = world.project.join(".lwc/wiki.db");
    let lock = rusqlite::Connection::open(&database).unwrap();
    lock.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    let before_config = fs::read(world.project.join(".lwc/config.json")).ok();
    let started = Instant::now();
    let value = prompt_hook(&world, "create a plan and ingest a source");
    assert!(started.elapsed() < Duration::from_secs(2));
    let batch = signal_batch(&value);
    assert_eq!(batch["signals"][0]["kind"], "plan.enable");
    let rendered = serde_json::to_string(&value).unwrap();
    assert!(!rendered.contains("PRIVATE_STRONG_PAGE_BODY"));
    assert!(!rendered.contains("LWC_READINESS "));
    assert_eq!(
        fs::read(world.project.join(".lwc/config.json")).ok(),
        before_config
    );
    lock.execute_batch("ROLLBACK").unwrap();

    let missing = World::new(false);
    for prompt in [
        "ingest a source",
        "start a changeset",
        "search the project wiki",
    ] {
        let signal = &signal_batch(&prompt_hook(&missing, prompt))["signals"][0];
        assert_eq!(signal["kind"], "wiki.setup", "{prompt}: {signal}");
        assert_eq!(signal["priority"], 20);
        assert_eq!(signal["requires_consent"], true);
        assert_eq!(signal["next_action"], "lwc --scope project init");
    }
    assert!(!missing.project.join(".lwc/wiki.db").exists());
}

#[test]
fn signal_prompt_shared_readiness_probe_failures_do_not_hide_other_providers() {
    let no_home = World::new(true);
    fs::create_dir_all(no_home.project.join("src")).unwrap();
    fs::write(no_home.project.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
    let started = Instant::now();
    let value = prompt_hook_without_home(&no_home, "create a plan and inspect the code structure");
    assert!(started.elapsed() < Duration::from_secs(2));
    let batch = signal_batch(&value);
    let kinds = batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|signal| signal["kind"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(kinds.contains("plan.enable"), "{batch}");
    assert!(!kinds.contains("graph.code.enable"), "{batch}");

    let corrupt_global = World::new(true);
    let work_id = "9".repeat(64);
    let work_path = write_work_hook_state(&corrupt_global, &work_id, "running", 4);
    let before = fs::read(&work_path).unwrap();
    fs::create_dir_all(corrupt_global.home.join(".lwc")).unwrap();
    let config = corrupt_global.home.join(".lwc/config.json");
    fs::write(&config, b"not json").unwrap();
    let started = Instant::now();
    let value = prompt_hook(&corrupt_global, "show background work and teach me Rust");
    assert!(started.elapsed() < Duration::from_secs(2));
    let batch = signal_batch(&value);
    let kinds = batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|signal| signal["kind"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(kinds.contains("work.resume"), "{batch}");
    assert!(!kinds.contains("tutor.enable"), "{batch}");
    assert_eq!(fs::read(work_path).unwrap(), before);
    assert_eq!(fs::read(config).unwrap(), b"not json");
}

#[test]
fn signal_semantic_sources_keep_real_envelopes_and_single_plan_continuity() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "private continuity title",
        "PRIVATE_OBJECTIVE_MUST_NOT_LEAK",
        "PRIVATE_DONE_WHEN_MUST_NOT_LEAK",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();
    let cases = [
        ("SessionStart", "startup", "SessionStart", "session_start"),
        ("SessionStart", "resume", "SessionStart", "session_resume"),
        ("SessionStart", "clear", "SessionStart", "session_clear"),
        ("SessionStart", "compact", "SessionStart", "compact_after"),
    ];

    for (event, source, envelope_event, semantic_event) in cases {
        let output = world.output(
            &["agent", "hook", "--agent", "claude", "--event", event],
            &serde_json::json!({
                "hook_event_name": event,
                "source": source,
                "prompt": "PROMPT_SECRET_MUST_NOT_LEAK",
                "transcript_path": "/private/transcript.jsonl",
                "session_id": DEFAULT_CLAUDE_SESSION,
            })
            .to_string(),
        );
        let value = hook_json(&output);
        assert_eq!(
            value["hookSpecificOutput"]["hookEventName"], envelope_event,
            "wrong host envelope for {event}/{source}: {value}"
        );
        let batch = signal_batch(&value);
        assert_signal_shape(&batch, semantic_event);
        let signals = batch["signals"].as_array().unwrap();
        assert_eq!(signals.len(), 1, "{event}/{source}: {batch}");
        let signal = &signals[0];
        assert_eq!(signal["kind"], "plan.resume");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["why_now"], "active_plan_at_session_boundary");
        assert_eq!(signal["requires_consent"], false);
        assert_eq!(signal["completion_effect"], "none");
        assert_eq!(signal["state"]["id"], plan_id);
        assert_eq!(signal["state"]["revision"], 1);
        assert_eq!(signal["next_action"], format!("lwc plan brief {plan_id}"));
        let rendered = serde_json::to_string(&batch).unwrap();
        for secret in [
            "PRIVATE_OBJECTIVE_MUST_NOT_LEAK",
            "PRIVATE_DONE_WHEN_MUST_NOT_LEAK",
            "PROMPT_SECRET_MUST_NOT_LEAK",
            "/private/transcript.jsonl",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn scope_all_boundary_omits_stale_global_store_without_losing_project_plan() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "SCOPE_ISOLATION_PROJECT_PLAN",
        "PRIVATE_SCOPE_ISOLATION_OBJECTIVE",
        "PRIVATE_SCOPE_ISOLATION_DONE_WHEN",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let scoped_context = agent_context("codex", "thr_scope_isolation_project", "main", "main");
    world.ok(&["plan", "track", plan_id, "--context", &scoped_context]);
    world.page("project-scope-rule", "PROJECT_SCOPE_CONTEXT");
    world.enable_tag("Rules", "project-scope-rule", "1", "1000");
    world.ok(&["--scope", "global", "init"]);
    put_global_autoload_page(&world, "stale-global-rule", "PRIVATE_STALE_GLOBAL_CONTEXT");
    let global_database = world.home.join(".lwc/wiki.db");
    rusqlite::Connection::open(&global_database)
        .unwrap()
        .pragma_update(None, "user_version", 16)
        .unwrap();

    let project_before = snapshot_tree(&world.project);
    let home_before = snapshot_tree(&world.home);
    let value = hook_json(
        &world.output(
            &[
                "--scope",
                "all",
                "agent",
                "hook",
                "--agent",
                "codex",
                "--event",
                "SessionStart",
            ],
            &serde_json::json!({
                "session_id": "thr_scope_isolation_project",
                "transcript_path": null,
                "cwd": world.project,
                "hook_event_name": "SessionStart",
                "model": "gpt-5.6-sol",
                "permission_mode": "default",
                "source": "startup",
            })
            .to_string(),
        ),
    );

    let context = hook_context(&value).expect("valid project context must survive stale global");
    assert!(context.contains("PROJECT_SCOPE_CONTEXT"), "{value}");
    let signal = &signal_batch(&value)["signals"][0];
    assert_eq!(signal["kind"], "plan.resume");
    assert_eq!(signal["state"]["id"], plan_id);
    assert!(!context.contains("PRIVATE_STALE_GLOBAL_CONTEXT"), "{value}");
    assert!(!context.contains("store_hook_unavailable"), "{value}");
    assert!(!context.contains("unsupported_store_version"), "{value}");
    assert!(
        !context.contains(global_database.to_str().unwrap()),
        "{value}"
    );
    assert_eq!(snapshot_tree(&world.project), project_before);
    assert_eq!(snapshot_tree(&world.home), home_before);
}

#[test]
fn scope_all_boundary_omits_stale_project_store_without_losing_global_context() {
    let world = World::new(true);
    world.page("stale-project-rule", "PRIVATE_STALE_PROJECT_CONTEXT");
    world.enable_tag("Rules", "stale-project-rule", "1", "1000");
    world.ok(&["--scope", "global", "init"]);
    put_global_autoload_page(&world, "global-scope-rule", "GLOBAL_SCOPE_CONTEXT");
    let project_database = world.project.join(".lwc/wiki.db");
    rusqlite::Connection::open(&project_database)
        .unwrap()
        .pragma_update(None, "user_version", 16)
        .unwrap();

    let project_before = snapshot_tree(&world.project);
    let home_before = snapshot_tree(&world.home);
    let value = hook_json(
        &world.output(
            &[
                "--scope",
                "all",
                "agent",
                "hook",
                "--agent",
                "codex",
                "--event",
                "SessionStart",
            ],
            &serde_json::json!({
                "session_id": "thr_scope_isolation_global",
                "transcript_path": null,
                "cwd": world.project,
                "hook_event_name": "SessionStart",
                "model": "gpt-5.6-sol",
                "permission_mode": "default",
                "source": "startup",
            })
            .to_string(),
        ),
    );

    let context = hook_context(&value).expect("valid global context must survive stale project");
    assert!(context.contains("GLOBAL_SCOPE_CONTEXT"), "{value}");
    assert!(
        !context.contains("PRIVATE_STALE_PROJECT_CONTEXT"),
        "{value}"
    );
    assert!(!context.contains("store_hook_unavailable"), "{value}");
    assert!(!context.contains("unsupported_store_version"), "{value}");
    assert!(
        !context.contains(project_database.to_str().unwrap()),
        "{value}"
    );
    assert_eq!(snapshot_tree(&world.project), project_before);
    assert_eq!(snapshot_tree(&world.home), home_before);
}

#[test]
fn signal_subagent_start_does_not_reuse_root_boundary_candidates() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(
        &world,
        "PRIVATE_CHILD_PLAN_TITLE",
        "PRIVATE_CHILD_PLAN_OBJECTIVE",
        "PRIVATE_CHILD_PLAN_DONE_WHEN",
    );
    world.ok(&["config", "set", "--todo", "enabled"]);
    world.ok(&[
        "todo",
        "add",
        "PRIVATE_CHILD_TODO_TITLE",
        "--target-at",
        "2000-01-01T00:00:00Z",
        "--cue",
        "PRIVATE_CHILD_TODO_CUE",
    ]);
    let work_id = "7".repeat(64);
    let work_state = write_work_hook_state(&world, &work_id, "failed", 1);
    let source_id = add_ingest_source(&world);
    set_ingest_status(&world, source_id, "failed");
    let sync_state = write_sync_hook_state(&world, "conflicts", true);
    let changeset = stage_changeset_page(&world, "child-boundary", "PRIVATE_CHILD_CHANGESET_BODY");
    make_changeset_conflict(&changeset);

    assert!(
        hook_context(&boundary_hook(&world))
            .unwrap()
            .contains("LWC_SIGNAL "),
        "the root boundary fixture must contain actionable root state"
    );
    let before_work = fs::read(&work_state).unwrap();
    let before_sync = fs::read(&sync_state).unwrap();
    let before_changeset = fs::read(&changeset).unwrap();
    let value = tool_hook(
        &world,
        "claude",
        "SubagentStart",
        &serde_json::json!({
            "hook_event_name": "SubagentStart",
            "source": "subagent",
            "prompt": "PRIVATE_CHILD_PROMPT",
        }),
    );
    let context = hook_context(&value).expect("child boundary keeps bounded readiness context");
    assert!(context.contains("LWC_READINESS "), "{value}");
    assert!(!context.contains("LWC_SIGNAL "), "{value}");
    for secret in [
        "PRIVATE_CHILD_PLAN_OBJECTIVE",
        "PRIVATE_CHILD_PLAN_DONE_WHEN",
        "PRIVATE_CHILD_TODO_CUE",
        "PRIVATE_CHILD_CHANGESET_BODY",
        "PRIVATE_CHILD_PROMPT",
    ] {
        assert!(!context.contains(secret), "leaked {secret}: {context}");
    }
    assert_eq!(fs::read(work_state).unwrap(), before_work);
    assert_eq!(fs::read(sync_state).unwrap(), before_sync);
    assert_eq!(fs::read(changeset).unwrap(), before_changeset);
}

#[test]
fn signal_plan_candidate_over_three_kib_utf8_is_dropped_deterministically() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let title = "🧠".repeat(600);
    let current = "🚀".repeat(600);
    let next = "📚".repeat(600);
    let created = world.ok(&[
        "plan",
        "create",
        &title,
        "--objective",
        "bounded candidate",
        "--done-when",
        "verified",
        "--step",
        &current,
        "--step",
        &next,
    ]);
    let context_id = agent_context("claude", DEFAULT_CLAUDE_SESSION, "main", "main");
    world.ok(&[
        "plan", "track", created["plan"]["id"].as_str().unwrap(), "--context", &context_id,
    ]);

    let first = boundary_hook(&world);
    let second = boundary_hook(&world);
    assert_eq!(first, second, "oversized candidate handling must be stable");
    let context = hook_context(&first).unwrap();
    let tracking = &readiness(context)["plan"]["tracking"];
    assert_eq!(tracking["title"].as_str().unwrap().chars().count(), 500);
    assert_eq!(
        tracking["current_step"]["title"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        500
    );
    assert_eq!(
        tracking["next_step"]["title"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        500
    );
    assert!(!context.contains("LWC_SIGNAL "), "{first}");
}

#[test]
fn signal_empty_state_and_irrelevant_events_are_default_noops() {
    let world = World::new(true);
    for (event, input) in [
        ("SessionStart", serde_json::json!({"source": "startup"})),
        ("SessionStart", serde_json::json!({"source": "resume"})),
        ("SessionStart", serde_json::json!({"source": "clear"})),
        ("SessionStart", serde_json::json!({"source": "compact"})),
        ("SubagentStart", serde_json::json!({})),
    ] {
        let value = hook_json(&world.output(
            &["agent", "hook", "--agent", "codex", "--event", event],
            &input.to_string(),
        ));
        let context = hook_context(&value).expect("low-frequency boundary keeps readiness");
        assert!(context.contains("LWC_READINESS "), "{event}: {value}");
        assert!(!context.contains("LWC_SIGNAL "), "{event}: {value}");
    }

    for (event, input) in [
        (
            "UserPromptSubmit",
            serde_json::json!({"prompt": "say hello"}),
        ),
        ("PreToolUse", serde_json::json!({"tool_name": "Read"})),
        ("PostToolUse", serde_json::json!({"tool_name": "Read"})),
        (
            "PostToolUseFailure",
            serde_json::json!({"tool_name": "Read"}),
        ),
        ("Stop", serde_json::json!({"stop_hook_active": false})),
        ("SessionEnd", serde_json::json!({})),
    ] {
        let output = world.output(
            &["agent", "hook", "--agent", "codex", "--event", event],
            &input.to_string(),
        );
        assert_eq!(hook_json(&output), serde_json::json!({}), "{event}");
    }
}

#[test]
fn signal_opencode_context_is_the_only_model_visible_event() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "OpenCode continuity",
        "PRIVATE_OPENCODE_OBJECTIVE",
        "PRIVATE_OPENCODE_DONE_WHEN",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let context_id = agent_context("opencode", "opencode-session", "main", "main");
    world.ok(&["plan", "track", plan_id, "--context", &context_id]);

    let value = tool_hook(
        &world,
        "opencode",
        "context",
        &serde_json::json!({"sessionID":"opencode-session"}),
    );
    let context = value["additionalContext"]
        .as_str()
        .expect("OpenCode plugin reads additionalContext");
    assert!(context.contains("LWC_READINESS "));
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "session_start");
    assert_eq!(batch["signals"][0]["kind"], "plan.resume");
    assert_eq!(
        batch["signals"][0]["next_action"],
        format!("lwc plan brief {plan_id}")
    );
    assert!(!context.contains("PRIVATE_OPENCODE_OBJECTIVE"));
    assert!(!context.contains("PRIVATE_OPENCODE_DONE_WHEN"));

    for event in ["prompt", "execute.after", "Stop"] {
        assert_eq!(
            tool_hook(&world, "opencode", event, &serde_json::json!({})),
            serde_json::json!({}),
            "unsupported OpenCode event {event} must fail open"
        );
    }
}

#[test]
fn signal_multiple_active_plans_only_exposes_the_contexts_tracked_plan() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let first = create_active_plan(
        &world,
        "FIRST_PRIVATE_PLAN",
        "FIRST_PRIVATE_OBJECTIVE",
        "FIRST_PRIVATE_DONE_WHEN",
    );
    let second = create_active_plan(
        &world,
        "SECOND_PRIVATE_PLAN",
        "SECOND_PRIVATE_OBJECTIVE",
        "SECOND_PRIVATE_DONE_WHEN",
    );
    let cases = [
        ("SessionStart", "startup", "session_start"),
        ("SessionStart", "resume", "session_resume"),
        ("SessionStart", "compact", "compact_after"),
    ];

    for (event, source, semantic_event) in cases {
        let value = hook_json(&world.output(
            &["agent", "hook", "--agent", "claude", "--event", event],
            &serde_json::json!({"source": source, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
        ));
        let batch = signal_batch(&value);
        assert_signal_shape(&batch, semantic_event);
        let signals = batch["signals"].as_array().unwrap();
        assert_eq!(signals.len(), 1, "{batch}");
        let signal = &signals[0];
        assert_eq!(signal["kind"], "plan.resume");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["state"]["id"], first["plan"]["id"]);
        assert_eq!(signal["completion_effect"], "none");
        let rendered = serde_json::to_string(&value).unwrap();
        for secret in [
            second["plan"]["id"].as_str().unwrap(),
            "FIRST_PRIVATE_OBJECTIVE",
            "FIRST_PRIVATE_DONE_WHEN",
            "SECOND_PRIVATE_PLAN",
            "SECOND_PRIVATE_OBJECTIVE",
            "SECOND_PRIVATE_DONE_WHEN",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }

    let prompt = prompt_hook(&world, "continue the current plan");
    assert_eq!(
        signal_batch(&prompt)["signals"][0]["kind"],
        "plan.resume"
    );
    let prompt_rendered = serde_json::to_string(&prompt).unwrap();
    for secret in [
        second["plan"]["id"].as_str().unwrap(),
        "SECOND_PRIVATE_PLAN",
    ] {
        assert!(
            !prompt_rendered.contains(secret),
            "prompt leaked {secret}: {prompt_rendered}"
        );
    }

    rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute(
            "UPDATE plan_steps SET title=?1 WHERE plan_id=?2 AND ordinal=1",
            (vec![0xff_u8], second["plan"]["id"].as_str().unwrap()),
        )
        .unwrap();
    let boundary = boundary_hook(&world);
    assert_eq!(
        signal_batch(&boundary)["signals"][0]["kind"],
        "plan.resume",
        "untracked malformed Plan must not affect this context: {boundary}"
    );
    let plan = &readiness(hook_context(&boundary).unwrap())["plan"];
    assert_eq!(plan["active"], 1);
    assert_eq!(plan["tracking"]["id"], first["plan"]["id"]);
    let prompt = prompt_hook(&world, "continue the current plan");
    assert_eq!(
        signal_batch(&prompt)["signals"][0]["kind"],
        "plan.resume",
        "untracked malformed Plan must not affect prompt continuity: {prompt}"
    );
}

#[test]
fn signal_stop_continues_only_the_contexts_tracked_plan_among_multiple_active() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let first = create_active_plan(
        &world,
        "FIRST_STOP_PRIVATE_PLAN",
        "FIRST_STOP_PRIVATE_OBJECTIVE",
        "FIRST_STOP_PRIVATE_DONE_WHEN",
    );
    let second = create_active_plan(
        &world,
        "SECOND_STOP_PRIVATE_PLAN",
        "SECOND_STOP_PRIVATE_OBJECTIVE",
        "SECOND_STOP_PRIVATE_DONE_WHEN",
    );
    let value = hook_json(&world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": false, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    assert_eq!(value["decision"], "block", "{value}");
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "stop");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.continue");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["why_now"], "executable_plan_at_stop");
    assert_eq!(signal["state"]["id"], first["plan"]["id"]);
    assert_eq!(signal["completion_effect"], "continue_once");
    let rendered = serde_json::to_string(&batch).unwrap();
    for secret in [
        second["plan"]["id"].as_str().unwrap(),
        "FIRST_STOP_PRIVATE_OBJECTIVE",
        "FIRST_STOP_PRIVATE_DONE_WHEN",
        "SECOND_STOP_PRIVATE_PLAN",
        "SECOND_STOP_PRIVATE_OBJECTIVE",
        "SECOND_STOP_PRIVATE_DONE_WHEN",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
}

#[test]
fn signal_plan_lifecycle_distinguishes_blocked_and_completion_pending() {
    let blocked = World::new(true);
    blocked.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &blocked,
        "blocked continuity",
        "BLOCKED_PRIVATE_OBJECTIVE",
        "BLOCKED_PRIVATE_DONE_WHEN",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let step_id = created["plan"]["steps"][0]["id"].as_str().unwrap();
    blocked.ok(&[
        "plan",
        "block",
        plan_id,
        "--if-revision",
        "1",
        "--step",
        step_id,
        "--reason",
        "BLOCKED_PRIVATE_REASON",
    ]);
    let value = hook_json(&blocked.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"source": "resume", "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    let batch = signal_batch(&value);
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.blocked");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["why_now"], "plan_blocked_waiting_input");
    assert_eq!(signal["completion_effect"], "none");
    let rendered = serde_json::to_string(&batch).unwrap();
    for secret in [
        "BLOCKED_PRIVATE_OBJECTIVE",
        "BLOCKED_PRIVATE_DONE_WHEN",
        "BLOCKED_PRIVATE_REASON",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }

    let terminal = World::new(true);
    terminal.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &terminal,
        "terminal continuity",
        "TERMINAL_PRIVATE_OBJECTIVE",
        "TERMINAL_PRIVATE_DONE_WHEN",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let step_id = created["plan"]["steps"][0]["id"].as_str().unwrap();
    terminal.ok(&[
        "plan",
        "advance",
        plan_id,
        "--if-revision",
        "1",
        "--done",
        step_id,
        "--result",
        "TERMINAL_PRIVATE_RESULT",
    ]);
    let value = hook_json(&terminal.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"source": "resume", "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    let batch = signal_batch(&value);
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.complete");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["why_now"], "active_plan_at_session_boundary");
    assert_eq!(signal["completion_effect"], "none");
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(rendered.contains("lwc plan complete"));
    assert!(rendered.contains("--done-when-checked"));
    for secret in [
        "TERMINAL_PRIVATE_OBJECTIVE",
        "TERMINAL_PRIVATE_DONE_WHEN",
        "TERMINAL_PRIVATE_RESULT",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
}

#[test]
fn signal_executable_active_plan_blocks_stop_once_without_mutation() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(&world, "stop guard", "finish work", "tests pass");
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();

    let value = hook_json(&world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": false, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    assert_eq!(value["decision"], "block", "{value}");
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "stop");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.continue");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["why_now"], "executable_plan_at_stop");
    assert_eq!(signal["completion_effect"], "continue_once");
    let guidance = serde_json::to_string(signal).unwrap();
    assert!(guidance.contains(&format!("lwc plan brief {plan_id}")));
    assert!(guidance.contains("lwc plan advance"));
    assert!(guidance.contains("--if-revision"));
    assert!(guidance.contains("verify") || guidance.contains("验证"));
    assert!(guidance.contains("continue") || guidance.contains("继续"));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);

    let looped = world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": true}).to_string(),
    );
    assert_eq!(hook_json(&looped), serde_json::json!({}));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_subagent_stop_never_blocks_an_active_parent_plan() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "parent plan",
        "keep parent work active",
        "parent work verified",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();

    for (agent, event) in [
        ("claude", "SubagentStop"),
        ("codex", "SubagentStop"),
        ("cursor", "subagentStop"),
        ("hermes", "subagent_stop"),
        ("copilot-cli", "subagentStop"),
        ("copilot-vscode", "SubagentStop"),
    ] {
        let output = world.output(
            &["agent", "hook", "--agent", agent, "--event", event],
            &serde_json::json!({"stop_hook_active": false}).to_string(),
        );
        assert_eq!(hook_json(&output), serde_json::json!({}), "{agent}/{event}");
    }

    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_stop_loop_guard_short_circuits_locked_or_corrupt_wiki() {
    let locked = World::new(true);
    locked.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&locked, "locked stop", "finish", "verified");
    let database = locked.project.join(".lwc/wiki.db");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    let locked_before = snapshot_tree(&locked.project);
    let started = Instant::now();
    let output = locked.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": true}).to_string(),
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(hook_json(&output), serde_json::json!({}));
    assert_eq!(snapshot_tree(&locked.project), locked_before);
    connection.execute_batch("ROLLBACK").unwrap();

    let corrupt = World::new(true);
    fs::write(
        corrupt.project.join(".lwc/wiki.db"),
        b"not a sqlite database",
    )
    .unwrap();
    let corrupt_before = snapshot_tree(&corrupt.project);
    let started = Instant::now();
    let output = corrupt.output(
        &["agent", "hook", "--agent", "codex", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": true}).to_string(),
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(hook_json(&output), serde_json::json!({}));
    assert_eq!(snapshot_tree(&corrupt.project), corrupt_before);
}

#[test]
fn signal_blocked_active_plan_does_not_force_stop() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(&world, "blocked plan", "wait", "human approves");
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let step_id = created["plan"]["steps"][0]["id"].as_str().unwrap();
    world.ok(&[
        "plan",
        "block",
        plan_id,
        "--if-revision",
        "1",
        "--step",
        step_id,
        "--reason",
        "WAITING_FOR_HUMAN_SECRET",
    ]);
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();
    let value = hook_json(&world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": false, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));

    assert_eq!(value, serde_json::json!({}));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_terminal_steps_in_active_plan_continue_to_completion_check() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "completion guard",
        "ship",
        "DONE_WHEN_SECRET_MUST_NOT_LEAK",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let step_id = created["plan"]["steps"][0]["id"].as_str().unwrap();
    world.ok(&[
        "plan",
        "advance",
        plan_id,
        "--if-revision",
        "1",
        "--done",
        step_id,
        "--result",
        "PRIVATE_STEP_RESULT",
    ]);
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();
    assert_eq!(before["state"], "active");
    assert!(
        before["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|step| { matches!(step["status"].as_str(), Some("completed" | "skipped")) })
    );

    let value = hook_json(&world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": false, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    assert_eq!(value["decision"], "block", "{value}");
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "stop");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.complete");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["why_now"], "active_plan_terminal_steps_at_stop");
    assert_eq!(signal["completion_effect"], "continue_once");
    let guidance = serde_json::to_string(signal).unwrap();
    assert!(guidance.contains("lwc plan complete"));
    assert!(guidance.contains("--done-when-checked"));
    assert!(!guidance.contains("DONE_WHEN_SECRET_MUST_NOT_LEAK"));
    assert!(!guidance.contains("PRIVATE_STEP_RESULT"));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_stop_recovers_an_active_plan_without_a_focal_step() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(
        &world,
        "malformed focal",
        "recover safely",
        "tracking repaired",
    );
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let database = world.project.join(".lwc/wiki.db");
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute(
            "UPDATE plan_steps SET status='pending' WHERE plan_id=?1",
            [plan_id],
        )
        .unwrap();
    drop(connection);
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();

    let value = hook_json(&world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Stop"],
        &serde_json::json!({"stop_hook_active": false, "session_id": DEFAULT_CLAUDE_SESSION}).to_string(),
    ));
    assert_eq!(value["decision"], "block", "{value}");
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "stop");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "plan.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["why_now"], "active_plan_invalid_transition_state");
    assert_eq!(signal["state"], serde_json::json!({"id": plan_id}));
    assert_eq!(signal["next_action"], format!("lwc plan brief {plan_id}"));
    assert_eq!(signal["completion_effect"], "continue_once");
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(!rendered.contains("lwc plan advance"));
    assert!(!rendered.contains("--if-revision"));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_stop_state_machine_is_exhaustive_for_claude_and_codex() {
    let states = [
        ("zero", None),
        ("multiple", Some(("plan.continue", "executable_plan_at_stop"))),
        (
            "in_progress",
            Some(("plan.continue", "executable_plan_at_stop")),
        ),
        ("blocked", None),
        (
            "terminal",
            Some(("plan.complete", "active_plan_terminal_steps_at_stop")),
        ),
        (
            "malformed",
            Some(("plan.recovery", "active_plan_invalid_transition_state")),
        ),
    ];

    for (state, expected) in states {
        let world = World::new(true);
        world.ok(&["config", "set", "--plan", "enabled"]);
        match state {
            "zero" => {}
            "multiple" => {
                create_active_plan(&world, "first stop table plan", "continue", "verified");
                create_active_plan(&world, "second stop table plan", "continue", "verified");
            }
            "in_progress" => {
                create_active_plan(&world, "executable stop table plan", "continue", "verified");
            }
            "blocked" => {
                let created =
                    create_active_plan(&world, "blocked stop table plan", "wait", "human input");
                world.ok(&[
                    "plan",
                    "block",
                    created["plan"]["id"].as_str().unwrap(),
                    "--if-revision",
                    "1",
                    "--step",
                    created["plan"]["steps"][0]["id"].as_str().unwrap(),
                    "--reason",
                    "PRIVATE_STOP_TABLE_HUMAN_WAIT",
                ]);
            }
            "terminal" => {
                let created = create_active_plan(
                    &world,
                    "terminal stop table plan",
                    "finish",
                    "done when checked",
                );
                world.ok(&[
                    "plan",
                    "advance",
                    created["plan"]["id"].as_str().unwrap(),
                    "--if-revision",
                    "1",
                    "--done",
                    created["plan"]["steps"][0]["id"].as_str().unwrap(),
                    "--result",
                    "PRIVATE_STOP_TABLE_RESULT",
                ]);
            }
            "malformed" => {
                let created = create_active_plan(
                    &world,
                    "malformed stop table plan",
                    "recover",
                    "tracking valid",
                );
                rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
                    .unwrap()
                    .execute(
                        "UPDATE plan_steps SET status='pending' WHERE plan_id=?1",
                        [created["plan"]["id"].as_str().unwrap()],
                    )
                    .unwrap();
            }
            other => panic!("unsupported Stop table state {other}"),
        }

        let operations = live_operation_count(&world);
        let before = snapshot_tree(&world.project);
        for agent in ["claude", "codex"] {
            let first = tool_hook(
                &world,
                agent,
                "Stop",
                &serde_json::json!({"stop_hook_active": false}),
            );
            if let Some((kind, why_now)) = expected {
                assert_eq!(first["decision"], "block", "{agent}/{state}: {first}");
                let signal = &signal_batch(&first)["signals"][0];
                assert_eq!(signal["kind"], kind, "{agent}/{state}: {signal}");
                assert_eq!(signal["priority"], 100, "{agent}/{state}: {signal}");
                assert_eq!(signal["why_now"], why_now, "{agent}/{state}: {signal}");
                assert_eq!(
                    signal["completion_effect"], "continue_once",
                    "{agent}/{state}: {signal}"
                );
            } else {
                assert_eq!(first, serde_json::json!({}), "{agent}/{state}: {first}");
            }
            assert_eq!(
                tool_hook(
                    &world,
                    agent,
                    "Stop",
                    &serde_json::json!({"stop_hook_active": true}),
                ),
                serde_json::json!({}),
                "{agent}/{state} repeat must short-circuit"
            );
        }
        assert_eq!(
            snapshot_tree(&world.project),
            before,
            "Stop mutated {state}"
        );
        assert_eq!(live_operation_count(&world), operations, "{state}");
    }
}

#[test]
fn signal_stop_loop_guard_is_stable_across_fifty_first_repeat_pairs() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&world, "repeat stress plan", "continue", "verified");
    let operations = live_operation_count(&world);
    let before = snapshot_tree(&world.project);

    for index in 0..50 {
        let agent = if index % 2 == 0 { "claude" } else { "codex" };
        let first = tool_hook(
            &world,
            agent,
            "Stop",
            &serde_json::json!({"stop_hook_active": false}),
        );
        assert_eq!(first["decision"], "block", "pair {index}: {first}");
        let signal = &signal_batch(&first)["signals"][0];
        assert_eq!(signal["kind"], "plan.continue", "pair {index}: {signal}");
        assert_eq!(signal["completion_effect"], "continue_once");
        assert_eq!(
            tool_hook(
                &world,
                agent,
                "Stop",
                &serde_json::json!({"stop_hook_active": true}),
            ),
            serde_json::json!({}),
            "pair {index} repeat"
        );
    }

    assert_eq!(snapshot_tree(&world.project), before);
    assert_eq!(live_operation_count(&world), operations);
}

#[test]
fn signal_observer_stop_hosts_and_nonplan_state_never_block() {
    let active = World::new(true);
    active.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&active, "observer stop plan", "continue", "verified");
    let before = snapshot_tree(&active.project);
    for (agent, event) in [
        ("cursor", "stop"),
        ("gemini", "AfterAgent"),
        ("hermes", "post_llm_call"),
        ("antigravity", "PostInvocation"),
        ("antigravity", "Stop"),
        ("kiro", "Stop"),
        ("copilot-cli", "agentStop"),
        ("copilot-vscode", "Stop"),
        ("pi", "agent_settled"),
        ("opencode", "Stop"),
        ("generic", "Stop"),
    ] {
        assert_eq!(
            tool_hook(
                &active,
                agent,
                event,
                &serde_json::json!({"stop_hook_active": false}),
            ),
            serde_json::json!({}),
            "{agent}/{event} must not impersonate a guarded root Stop"
        );
    }
    assert_eq!(snapshot_tree(&active.project), before);

    let providers = World::new(true);
    write_work_hook_state(&providers, &"a".repeat(64), "running", 1);
    write_sync_hook_state(&providers, "transferring", false);
    stage_changeset_page(
        &providers,
        "stop-provider-draft",
        "PRIVATE_STOP_PROVIDER_BODY",
    );
    let before = snapshot_tree(&providers.project);
    for agent in ["claude", "codex"] {
        assert_eq!(
            tool_hook(
                &providers,
                agent,
                "Stop",
                &serde_json::json!({"stop_hook_active": false}),
            ),
            serde_json::json!({}),
            "{agent} must not block on Work/Sync/Changeset state"
        );
    }
    assert_eq!(snapshot_tree(&providers.project), before);
}

#[test]
fn signal_unknown_event_fails_open_without_touching_plan() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let created = create_active_plan(&world, "unknown event", "stay safe", "unchanged");
    let plan_id = created["plan"]["id"].as_str().unwrap();
    let before = world.ok(&["plan", "show", plan_id])["plan"].clone();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "UnexpectedLifecycleEvent",
        ],
        &serde_json::json!({"prompt": "PRIVATE_UNKNOWN_EVENT_PROMPT"}).to_string(),
    );
    assert_eq!(hook_json(&output), serde_json::json!({}));
    assert_eq!(world.ok(&["plan", "show", plan_id])["plan"], before);
}

#[test]
fn signal_unknown_session_source_and_unsupported_agent_events_are_noops() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&world, "capability gate", "private", "private");
    for (agent, event, input) in [
        (
            "claude",
            "SessionStart",
            serde_json::json!({"source": "future-source"}),
        ),
        ("claude", "PreCompact", serde_json::json!({})),
        (
            "cursor",
            "stop",
            serde_json::json!({"stop_hook_active": false}),
        ),
        ("gemini", "PostCompact", serde_json::json!({})),
        ("codex", "PostToolUseFailure", serde_json::json!({})),
    ] {
        let output = world.output(
            &["agent", "hook", "--agent", agent, "--event", event],
            &input.to_string(),
        );
        assert_eq!(hook_json(&output), serde_json::json!({}), "{agent}/{event}");
    }
}

#[test]
fn signal_selector_caps_batch_and_opportunities_without_leaking_inputs() {
    let world = World::new(true);
    world.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
    create_active_plan(
        &world,
        "budget plan",
        "budget objective",
        "budget done when",
    );
    world.ok(&[
        "todo",
        "add",
        "overdue signal",
        "--target-at",
        "2000-01-01T00:00:00Z",
        "--cue",
        "PRIVATE_TODO_CUE",
    ]);
    let session_id = "abcdefabcdefabcdefabcdefabcdefab";
    let sync = world.project.join(".lwc/sync").join(session_id);
    fs::create_dir_all(&sync).unwrap();
    fs::write(
        sync.join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": session_id,
            "mode": "merge",
            "scope": "project",
            "host": "PRIVATE_SYNC_HOST",
            "remote_directory": "/private/sync/path",
            "phase": "conflicts",
            "conflict_count": 2,
            "conflict_kinds": ["page", "plan"],
            "payload": {"secret": "PRIVATE_SYNC_PAYLOAD"},
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_stores": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let transcript = world.project.join("private-transcript.jsonl");
    fs::write(&transcript, "PRIVATE_TRANSCRIPT_BODY").unwrap();

    let value = hook_json(
        &world.output(
            &[
                "--scope",
                "all",
                "agent",
                "hook",
                "--agent",
                "claude",
                "--event",
                "SessionStart",
            ],
            &serde_json::json!({
                "source": "resume",
                "prompt": "PRIVATE_PROMPT_BODY",
                "transcript_path": transcript,
            })
            .to_string(),
        ),
    );
    let batch = signal_batch(&value);
    assert_signal_shape(&batch, "session_resume");
    let signals = batch["signals"].as_array().unwrap();
    assert!(
        signals.len() <= 3,
        "selector exceeded the fixed cap: {batch}"
    );
    assert!(
        signals
            .iter()
            .filter(|signal| signal["priority"] == 20)
            .count()
            <= 1,
        "{batch}"
    );
    let sync = signals
        .iter()
        .find(|signal| signal["kind"] == "sync.recovery")
        .expect("conflicted Sync must remain visible at a lifecycle boundary");
    assert_eq!(sync["priority"], 100);
    assert_eq!(sync["completion_effect"], "none");
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(rendered.chars().count() <= 8_192, "{batch}");
    for secret in [
        "budget objective",
        "budget done when",
        "PRIVATE_TODO_CUE",
        "PRIVATE_SYNC_HOST",
        "/private/sync/path",
        "PRIVATE_SYNC_PAYLOAD",
        "PRIVATE_PROMPT_BODY",
        "PRIVATE_TRANSCRIPT_BODY",
        "private-transcript.jsonl",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
}

#[test]
fn signal_sync_boundary_resumes_nonconflicted_progress_without_followup_effect() {
    let world = World::new(true);
    let session_id = "abcdefabcdefabcdefabcdefabcdefab";
    let sync = world.project.join(".lwc/sync").join(session_id);
    fs::create_dir_all(&sync).unwrap();
    fs::write(
        sync.join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": session_id,
            "mode": "merge",
            "scope": "project",
            "host": "PRIVATE_SYNC_HOST",
            "remote_directory": "/private/sync/path",
            "phase": "handshake_complete",
            "conflict_count": 0,
            "conflict_kinds": [],
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_stores": [],
        }))
        .unwrap(),
    )
    .unwrap();

    let batch = signal_batch(&boundary_hook(&world));
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "sync.resume");
    assert_eq!(signal["priority"], 80);
    assert_eq!(signal["completion_effect"], "none");
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(!rendered.contains("PRIVATE_SYNC_HOST"));
    assert!(!rendered.contains("/private/sync/path"));
}

#[test]
fn signal_work_boundary_maps_active_failure_and_success_silence() {
    let world = World::new(true);
    let id = "a".repeat(64);

    for (sequence, state) in [(1, "queued"), (2, "running")] {
        let path = write_work_hook_state(&world, &id, state, sequence);
        let before = fs::read(&path).unwrap();
        let batch = signal_batch(&boundary_hook(&world));
        assert_signal_shape(&batch, "session_resume");
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "work.resume");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["completion_effect"], "none");
        assert_eq!(signal["next_action"], format!("lwc work status {id}"));
        let rendered = serde_json::to_string(&batch).unwrap();
        for secret in [
            "PRIVATE_WORK_DATABASE",
            "PRIVATE_WORK_MESSAGE",
            "PRIVATE_WORK_RESULT",
            "PRIVATE_WORK_ERROR_MESSAGE",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
        assert_eq!(fs::read(path).unwrap(), before);
    }

    for (sequence, state) in [(3, "failed"), (4, "cancelled")] {
        let path = write_work_hook_state(&world, &id, state, sequence);
        let before = fs::read(&path).unwrap();
        let batch = signal_batch(&boundary_hook(&world));
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "work.failed");
        assert_eq!(signal["priority"], 60);
        assert_eq!(signal["completion_effect"], "none");
        assert_eq!(signal["next_action"], format!("lwc work status {id}"));
        let rendered = serde_json::to_string(&batch).unwrap();
        assert!(!rendered.contains("PRIVATE_WORK_ERROR_MESSAGE"));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    write_work_hook_state(&world, &id, "succeeded", 5);
    assert!(
        !hook_context(&boundary_hook(&world))
            .unwrap()
            .contains("LWC_SIGNAL ")
    );
}

#[test]
fn signal_changeset_boundary_maps_nonempty_conflict_and_empty_silence() {
    let empty = World::new(true);
    empty.ok(&["changeset", "begin", "empty-boundary"]);
    assert!(
        !hook_context(&boundary_hook(&empty))
            .unwrap()
            .contains("LWC_SIGNAL ")
    );

    let world = World::new(true);
    let draft = stage_changeset_page(
        &world,
        "boundary-draft",
        "PRIVATE_CHANGESET_BODY_MUST_NOT_LEAK",
    );
    let operations = live_operation_count(&world);
    let before = fs::read(&draft).unwrap();
    let batch = signal_batch(&boundary_hook(&world));
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "changeset.resume");
    assert_eq!(signal["priority"], 80);
    assert_eq!(signal["completion_effect"], "none");
    assert_eq!(
        signal["next_action"],
        "lwc changeset show boundary-draft --limit 20"
    );
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(!rendered.contains("PRIVATE_CHANGESET_BODY_MUST_NOT_LEAK"));
    assert!(!rendered.contains("PRIVATE_CHANGESET_TITLE"));
    assert_eq!(fs::read(&draft).unwrap(), before);
    assert_eq!(live_operation_count(&world), operations);

    make_changeset_conflict(&draft);
    let before = fs::read(&draft).unwrap();
    let batch = signal_batch(&boundary_hook(&world));
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "changeset.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["completion_effect"], "none");
    assert_eq!(fs::read(&draft).unwrap(), before);
    assert_eq!(live_operation_count(&world), operations);
}

#[test]
fn signal_ingest_boundary_maps_active_failure_and_terminal_silence() {
    let world = World::new(true);
    let source_id = add_ingest_source(&world);
    assert!(
        !hook_context(&boundary_hook(&world))
            .unwrap()
            .contains("LWC_SIGNAL ")
    );

    for status in ["analyzing", "generating"] {
        set_ingest_status(&world, source_id, status);
        let before = rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
            .unwrap()
            .query_row(
                "SELECT status, attempts, last_error FROM ingest_jobs WHERE source_id=?1",
                [source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        let batch = signal_batch(&boundary_hook(&world));
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "ingest.resume");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["completion_effect"], "none");
        assert_eq!(signal["next_action"], "lwc ingest list --limit 20");
        let rendered = serde_json::to_string(&batch).unwrap();
        for secret in [
            "PRIVATE_INGEST_SOURCE.txt",
            "PRIVATE_INGEST_SOURCE_BODY",
            "PRIVATE_INGEST_TITLE",
            "PRIVATE_INGEST_ERROR",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
        let after = rusqlite::Connection::open(world.project.join(".lwc/wiki.db"))
            .unwrap()
            .query_row(
                "SELECT status, attempts, last_error FROM ingest_jobs WHERE source_id=?1",
                [source_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(after, before);
    }

    set_ingest_status(&world, source_id, "failed");
    let batch = signal_batch(&boundary_hook(&world));
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "ingest.recovery");
    assert_eq!(signal["priority"], 100);
    assert_eq!(signal["completion_effect"], "none");
    assert_eq!(
        signal["next_action"],
        format!("lwc ingest retry {source_id}")
    );
    assert!(
        !serde_json::to_string(&batch)
            .unwrap()
            .contains("PRIVATE_INGEST_ERROR")
    );

    set_ingest_status(&world, source_id, "completed");
    assert!(
        !hook_context(&boundary_hook(&world))
            .unwrap()
            .contains("LWC_SIGNAL ")
    );
}

#[test]
fn signal_memory_boundary_stays_silent_without_bound_current_work() {
    let world = World::new(true);
    set_memory_pressure(&world);
    let database = world.project.join(".lwc/wiki.db");
    let before: (i64, i64) = rusqlite::Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT event_count, logical_bytes FROM memory_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    let value = boundary_hook(&world);
    let context = hook_context(&value).unwrap();
    assert!(!context.contains("LWC_SIGNAL "));
    let after: (i64, i64) = rusqlite::Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT event_count, logical_bytes FROM memory_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);

    let hints = World::new(true);
    hints.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-bytes",
        "1000000000",
    ]);
    for index in 0..5 {
        let capsule = serde_json::json!({
            "type": "boundary-cluster",
            "context": "PRIVATE_MEMORY_CONTEXT",
            "observed": [format!("PRIVATE_MEMORY_OBSERVATION_{index}")],
        })
        .to_string();
        hints.ok(&["remember", "--json", &capsule]);
    }
    let value = boundary_hook(&hints);
    let context = hook_context(&value).unwrap();
    assert!(!context.contains("LWC_SIGNAL "));
    assert!(!context.contains("PRIVATE_MEMORY_CONTEXT"));
    assert!(!context.contains("PRIVATE_MEMORY_OBSERVATION"));
}

#[cfg(unix)]
#[test]
fn signal_broken_or_symlinked_provider_does_not_hide_the_active_plan() {
    use std::os::unix::fs::symlink;

    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(&world, "provider isolation", "continue", "verified");
    let source_id = add_ingest_source(&world);
    set_ingest_status(&world, source_id, "analyzing");

    let outside = world.project.join("outside-work");
    let linked_id = "b".repeat(64);
    let linked_state = write_work_hook_state(&world, &linked_id, "failed", 1);
    let work_root = linked_state.parent().unwrap().parent().unwrap();
    fs::rename(work_root, &outside).unwrap();
    symlink(&outside, work_root).unwrap();

    let changesets = world.project.join(".lwc/changesets");
    fs::create_dir_all(&changesets).unwrap();
    fs::write(changesets.join("broken.db"), b"not a sqlite database").unwrap();

    let batch = signal_batch(&boundary_hook(&world));
    let signals = batch["signals"].as_array().unwrap();
    assert_eq!(signals.len(), 2, "{batch}");
    let kinds = signals
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(kinds, BTreeSet::from(["ingest.resume", "plan.resume"]));
    assert!(
        !serde_json::to_string(&batch)
            .unwrap()
            .contains("work.failed")
    );
}

#[test]
fn signal_hook_deadline_omits_locked_changesets_without_hiding_plan_context() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(
        &world,
        "deadline continuity",
        "PRIVATE_DEADLINE_OBJECTIVE",
        "PRIVATE_DEADLINE_DONE_WHEN",
    );
    let mut drafts = Vec::new();
    let mut locks = Vec::new();
    for index in 0..10 {
        let path = stage_changeset_page(
            &world,
            &format!("deadline-{index:02}"),
            "PRIVATE_DEADLINE_CHANGESET_BODY",
        );
        drafts.push((path.clone(), fs::read(&path).unwrap()));
        let lock = rusqlite::Connection::open(path).unwrap();
        lock.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")
            .unwrap();
        locks.push(lock);
    }

    for value in [
        {
            let started = Instant::now();
            let value = boundary_hook(&world);
            assert!(
                started.elapsed() < Duration::from_millis(1_900),
                "lifecycle hook exceeded its wall budget"
            );
            value
        },
        {
            let started = Instant::now();
            let value = prompt_hook(&world, "continue the current plan and changeset");
            assert!(
                started.elapsed() < Duration::from_millis(1_900),
                "prompt hook exceeded its wall budget"
            );
            value
        },
    ] {
        let batch = signal_batch(&value);
        let kinds = batch["signals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|signal| signal["kind"].as_str())
            .collect::<BTreeSet<_>>();
        assert!(kinds.contains("plan.resume"), "{value}");
        assert!(
            !kinds.iter().any(|kind| kind.starts_with("changeset.")),
            "{value}"
        );
        let rendered = serde_json::to_string(&value).unwrap();
        assert!(!rendered.contains("PRIVATE_DEADLINE_CHANGESET_BODY"));
        assert!(!rendered.contains("PRIVATE_DEADLINE_OBJECTIVE"));
        assert!(!rendered.contains("PRIVATE_DEADLINE_DONE_WHEN"));
    }
    for (path, before) in drafts {
        assert_eq!(fs::read(path).unwrap(), before);
    }
    for lock in locks {
        lock.execute_batch("ROLLBACK").unwrap();
    }
}

#[test]
fn signal_boundary_selector_omits_lower_priority_candidates_without_leaks_or_writes() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    create_active_plan(
        &world,
        "selector plan",
        "PRIVATE_SELECTOR_OBJECTIVE",
        "PRIVATE_SELECTOR_DONE_WHEN",
    );
    let work_id = "c".repeat(64);
    let work_state = write_work_hook_state(&world, &work_id, "failed", 10);
    let draft = stage_changeset_page(&world, "selector-draft", "PRIVATE_SELECTOR_CHANGESET_BODY");
    make_changeset_conflict(&draft);
    let source_id = add_ingest_source(&world);
    set_ingest_status(&world, source_id, "failed");
    set_memory_pressure(&world);

    let operations = live_operation_count(&world);
    let work_before = fs::read(&work_state).unwrap();
    let draft_before = fs::read(&draft).unwrap();
    let batch = signal_batch(&boundary_hook(&world));
    assert_signal_shape(&batch, "session_resume");
    assert_eq!(batch["signals"].as_array().unwrap().len(), 3, "{batch}");
    assert_eq!(batch["omitted"], 1, "{batch}");
    let kinds = batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        kinds,
        BTreeSet::from(["changeset.recovery", "ingest.recovery", "plan.resume"])
    );
    let rendered = serde_json::to_string(&batch).unwrap();
    assert!(rendered.chars().count() <= 8_192);
    for secret in [
        "PRIVATE_SELECTOR_OBJECTIVE",
        "PRIVATE_SELECTOR_DONE_WHEN",
        "PRIVATE_WORK_DATABASE",
        "PRIVATE_WORK_MESSAGE",
        "PRIVATE_WORK_RESULT",
        "PRIVATE_SELECTOR_CHANGESET_BODY",
        "PRIVATE_CHANGESET_TITLE",
        "PRIVATE_INGEST_SOURCE_BODY",
        "PRIVATE_INGEST_TITLE",
        "PRIVATE_INGEST_ERROR",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert_eq!(live_operation_count(&world), operations);
    assert_eq!(fs::read(work_state).unwrap(), work_before);
    assert_eq!(fs::read(draft).unwrap(), draft_before);
}

#[test]
fn signal_tool_before_asks_only_for_an_exact_typed_consent_candidate() {
    let world = World::new(false);
    let warm = world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Unknown"],
        "{}",
    );
    assert!(warm.status.success());
    let before = snapshot_tree(&world.project);
    let ask = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {
            "command": "lwc --scope project config set --plan enabled",
            "path": "/PRIVATE_TOOL_INPUT_PATH",
        },
        "cwd": "/PRIVATE_TOOL_CWD",
        "prompt": "PRIVATE_TOOL_PROMPT",
        "transcript_path": "/PRIVATE_TOOL_TRANSCRIPT",
    });

    let started = Instant::now();
    let value = tool_hook(&world, "claude", "PreToolUse", &ask);
    assert!(started.elapsed() < Duration::from_millis(1_900));
    assert_ne!(
        value,
        serde_json::json!({}),
        "an exact consent-requiring LWC action must reach the host delivery adapter"
    );
    let rendered = serde_json::to_string(&value).unwrap();
    for secret in [
        "--plan enabled",
        "PRIVATE_TOOL_INPUT_PATH",
        "PRIVATE_TOOL_CWD",
        "PRIVATE_TOOL_PROMPT",
        "PRIVATE_TOOL_TRANSCRIPT",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }

    for payload in [
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "lwc plan status"},
        }),
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "git status"},
        }),
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "lwc config set --plan enabled && touch /PRIVATE_PATH"},
        }),
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "PRIVATE_TOKEN=x lwc config set --plan enabled"},
        }),
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "sh -c 'lwc config set --plan enabled'"},
        }),
        serde_json::json!({
            "tool_name": "Read",
            "tool_input": {"path": "/PRIVATE_PATH"},
        }),
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"cmd": "lwc config set --plan enabled"},
        }),
    ] {
        assert_eq!(
            tool_hook(&world, "claude", "PreToolUse", &payload),
            serde_json::json!({}),
            "non-Ask ToolBefore input must stay an exact no-op: {payload}"
        );
    }
    let oversized = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {
            "command": "lwc config set --plan enabled",
            "padding": "x".repeat(65 * 1024),
        }
    })
    .to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "PreToolUse",
        ],
        &oversized,
    );
    assert!(output.status.success());
    assert_eq!(hook_json(&output), serde_json::json!({}));
    assert_eq!(snapshot_tree(&world.project), before);
    assert!(!world.project.join(".lwc/wiki.db").exists());
}

#[test]
fn signal_tool_before_compiles_exact_ask_and_noop_outputs_for_twelve_hosts() {
    let world = World::new(false);
    let warm = world.output(
        &["agent", "hook", "--agent", "claude", "--event", "Unknown"],
        "{}",
    );
    assert!(warm.status.success());
    let reason = "Set LWC configuration.";
    let cases = vec![
        (
            "claude",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config set --plan enabled"},
                "cwd":"/PRIVATE_CLAUDE_CWD",
            }),
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": reason,
                }
            }),
        ),
        (
            "codex",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config set --plan enabled"},
                "cwd":"/PRIVATE_CODEX_CWD",
            }),
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "additionalContext": reason,
                }
            }),
        ),
        (
            "cursor",
            "beforeShellExecution",
            serde_json::json!({
                "hook_event_name":"beforeShellExecution",
                "command":"lwc config set --plan enabled",
                "cwd":"/PRIVATE_CURSOR_CWD",
                "prompt":"PRIVATE_CURSOR_PROMPT",
            }),
            serde_json::json!({
                "hook_event_name":"beforeShellExecution",
                "command":"lwc plan status",
            }),
            serde_json::json!({
                "permission": "ask",
                "user_message": reason,
                "agent_message": reason,
            }),
        ),
        (
            "hermes",
            "pre_tool_call",
            serde_json::json!({
                "tool_name":"terminal",
                "tool_input":{"command":"lwc config set --plan enabled"},
                "prompt":"PRIVATE_HERMES_PROMPT",
            }),
            serde_json::json!({
                "tool_name":"terminal",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({
                "action": "approve",
                "message": reason,
                "rule_key": "lwc:lwc_config_set",
            }),
        ),
        (
            "gemini",
            "BeforeTool",
            serde_json::json!({
                "tool_name":"run_shell_command",
                "tool_input":{"command":"lwc config set --plan enabled"},
                "cwd":"/PRIVATE_GEMINI_CWD",
            }),
            serde_json::json!({
                "tool_name":"run_shell_command",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
        (
            "antigravity",
            "PreToolUse",
            serde_json::json!({
                "toolCall":{
                    "name":"run_command",
                    "args":{
                        "CommandLine":"lwc config set --plan enabled",
                        "Cwd":"/PRIVATE_ANTIGRAVITY_CWD",
                    }
                }
            }),
            serde_json::json!({
                "toolCall":{
                    "name":"run_command",
                    "args":{"CommandLine":"lwc plan status"}
                }
            }),
            serde_json::json!({"decision":"ask","reason":reason}),
        ),
        (
            "kiro",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"execute_bash",
                "tool_input":{"command":"lwc config set --plan enabled"},
            }),
            serde_json::json!({
                "tool_name":"execute_bash",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
        (
            "pi",
            "tool_call",
            serde_json::json!({
                "tool_name":"bash",
                "args":{"command":"lwc config set --plan enabled"},
            }),
            serde_json::json!({
                "tool_name":"bash",
                "args":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
        (
            "copilot-cli",
            "preToolUse",
            serde_json::json!({
                "toolName":"bash",
                "toolArgs":"{\"command\":\"lwc config set --plan enabled\"}",
            }),
            serde_json::json!({
                "toolName":"bash",
                "toolArgs":"{\"command\":\"lwc plan status\"}",
            }),
            serde_json::json!({}),
        ),
        (
            "copilot-vscode",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config set --plan enabled"},
            }),
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
        (
            "opencode",
            "PreToolUse",
            serde_json::json!({
                "tool":"bash",
                "args":{"command":"lwc config set --plan enabled"},
            }),
            serde_json::json!({
                "tool":"bash",
                "args":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
        (
            "generic",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc config set --plan enabled"},
            }),
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc plan status"},
            }),
            serde_json::json!({}),
        ),
    ];
    let before = snapshot_tree(&world.project);

    for (agent, event, ask, noop, expected) in cases {
        let started = Instant::now();
        let actual = tool_hook(&world, agent, event, &ask);
        assert!(
            started.elapsed() < Duration::from_millis(1_900),
            "{agent}/{event} consent parsing exceeded the Hook budget"
        );
        assert_eq!(actual, expected, "{agent}/{event} Ask");
        assert_eq!(
            tool_hook(&world, agent, event, &noop),
            serde_json::json!({}),
            "{agent}/{event} Noop"
        );
        let rendered = serde_json::to_string(&actual).unwrap();
        for secret in ["config set", "--plan enabled", "PRIVATE_", "/private/"] {
            assert!(
                !rendered.contains(secret),
                "{agent} leaked {secret}: {rendered}"
            );
        }
    }
    assert_eq!(snapshot_tree(&world.project), before);
    assert!(!world.project.join(".lwc/wiki.db").exists());
}

#[test]
fn signal_tool_receipts_use_only_verified_context_hosts_and_envelopes() {
    let world = World::new(true);
    let id = "d".repeat(64);
    let stdout = work_receipt_stdout(&id, "queued");
    let cases = [
        (
            "claude",
            "PostToolUse",
            serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": format!("lwc work status {id}")},
                "tool_response": {"stdout": stdout, "stderr": "PRIVATE_STDERR"},
            }),
        ),
        (
            "codex",
            "PostToolUse",
            serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": format!("lwc work status {id}")},
                "tool_response": {"stdout": stdout, "stderr": "PRIVATE_STDERR"},
            }),
        ),
        (
            "cursor",
            "postToolUse",
            serde_json::json!({
                "tool_name": "Shell",
                "tool_input": {"command": format!("lwc work status {id}")},
                "tool_output": {"stdout": stdout, "stderr": "PRIVATE_STDERR"},
            }),
        ),
        (
            "gemini",
            "AfterTool",
            serde_json::json!({
                "tool_name": "run_shell_command",
                "tool_input": {"command": format!("lwc work status {id}")},
                "tool_response": {"stdout": stdout, "stderr": "PRIVATE_STDERR"},
            }),
        ),
        (
            "pi",
            "tool_result",
            serde_json::json!({
                "tool_name": "bash",
                "args": {"command": format!("lwc work status {id}")},
                "result": {"stdout": stdout, "stderr": "PRIVATE_STDERR"},
            }),
        ),
    ];

    for (agent, event, payload) in cases {
        let value = tool_hook(&world, agent, event, &payload);
        let batch = signal_batch(&value);
        assert_signal_shape(&batch, "tool_after");
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "work.resume", "{agent}: {batch}");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["completion_effect"], "requires_followup");
        assert_eq!(signal["next_action"], format!("lwc work status {id}"));
        let rendered = serde_json::to_string(&batch).unwrap();
        for secret in [
            "PRIVATE_TOOL_DATABASE",
            "PRIVATE_TOOL_MESSAGE",
            "PRIVATE_TOOL_RESULT",
            "PRIVATE_TOOL_ERROR",
            "PRIVATE_TOOL_PROMPT",
            "PRIVATE_STDERR",
        ] {
            assert!(
                !rendered.contains(secret),
                "{agent} leaked {secret}: {rendered}"
            );
        }
    }
}

#[test]
fn signal_tool_receipts_reject_non_lwc_pseudo_stderr_and_unverified_hosts() {
    let world = World::new(true);
    let stdout = work_receipt_stdout(&"e".repeat(64), "running");
    let cases = [
        (
            "claude",
            "PostToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"printf fake"},
                "tool_response":{"stdout":stdout},
            }),
        ),
        (
            "claude",
            "PostToolUse",
            serde_json::json!({
                "toolName":"Bash",
                "toolInput":{"command":"lwc work list"},
                "toolResponse":{"stdout":stdout},
            }),
        ),
        (
            "claude",
            "PostToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc work list"},
                "tool_response":{"stderr":stdout},
            }),
        ),
        (
            "claude",
            "PreToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc work list"},
                "tool_response":{"stdout":stdout},
            }),
        ),
        (
            "kiro",
            "PostToolUse",
            serde_json::json!({
                "tool_name":"execute_bash",
                "tool_input":{"command":"lwc work list"},
                "tool_result":{"stdout":stdout},
            }),
        ),
        (
            "copilot-cli",
            "postToolUse",
            serde_json::json!({
                "toolName":"bash",
                "toolArgs":"{\"command\":\"lwc work list\"}",
                "toolResult":{"resultType":"success","textResultForLlm":stdout},
            }),
        ),
        (
            "copilot-vscode",
            "PostToolUse",
            serde_json::json!({
                "tool_name":"Bash",
                "tool_input":{"command":"lwc work list"},
                "tool_result":{"result_type":"success","text_result_for_llm":stdout},
            }),
        ),
        (
            "hermes",
            "post_tool_call",
            serde_json::json!({
                "tool_name":"terminal",
                "tool_input":{"command":"lwc work list"},
                "tool_result":{"stdout":stdout},
            }),
        ),
        (
            "antigravity",
            "PostToolUse",
            serde_json::json!({
                "toolCall":{"name":"run_command","args":{"CommandLine":"lwc work list"}},
                "toolOutput":{"stdout":stdout},
            }),
        ),
    ];

    for (agent, event, payload) in cases {
        assert_eq!(
            tool_hook(&world, agent, event, &payload),
            serde_json::json!({}),
            "{agent}/{event}"
        );
    }
}

#[test]
fn signal_tool_failure_maps_only_recognized_action_families() {
    let world = World::new(true);
    for (command, expected_kind, next_action) in [
        (
            "lwc plan advance fixture",
            "plan.recovery",
            "lwc plan current --limit 20",
        ),
        (
            "lwc changeset commit safe-name",
            "changeset.recovery",
            "lwc changeset list",
        ),
        (
            "lwc sync fixture --resume safe-session",
            "sync.recovery",
            "lwc log --limit 20",
        ),
        (
            "lwc work resume safe-work",
            "work.recovery",
            "lwc work list",
        ),
        ("lwc graph verify", "graph.recovery", "lwc graph status"),
    ] {
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":command},
            "tool_response":{
                "success":false,
                "exit_code":1,
                "stdout":"{\"error\":{\"code\":\"forged_receipt_error\"}}",
                "stderr":"PRIVATE_FAILURE_STDERR",
            },
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUseFailure", &payload));
        assert_signal_shape(&batch, "tool_failure");
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], expected_kind);
        assert_eq!(signal["priority"], 100);
        assert_eq!(signal["completion_effect"], "none");
        assert_eq!(signal["next_action"], next_action);
        let rendered = serde_json::to_string(&batch).unwrap();
        assert!(!rendered.contains("forged_receipt_error"));
        assert!(!rendered.contains("PRIVATE_FAILURE_STDERR"));
    }

    for command in [
        "lwc config show",
        "lwc work status safe-work",
        "lwc graph status",
    ] {
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":command},
            "tool_response":{
                "success":false,
                "exit_code":1,
                "stdout":"{\"error\":{\"code\":\"work_failed\"}}",
            },
        });
        assert_eq!(
            tool_hook(&world, "claude", "PostToolUseFailure", &payload),
            serde_json::json!({})
        );
    }
}

#[test]
fn signal_tool_success_receipt_maps_only_allowlisted_error_prefixes() {
    let world = World::new(true);
    for (command, error_code, expected_kind) in [
        (
            "lwc plan advance fixture",
            "plan_revision_conflict",
            "plan.recovery",
        ),
        (
            "lwc changeset commit safe-name",
            "changeset_conflict",
            "changeset.recovery",
        ),
        (
            "lwc sync fixture --resume safe-session",
            "sync_transport_failed",
            "sync.recovery",
        ),
        ("lwc work resume safe-work", "work_failed", "work.recovery"),
        (
            "lwc graph verify",
            "graph_projection_failed",
            "graph.recovery",
        ),
    ] {
        let stdout = serde_json::json!({
            "error": {
                "code": error_code,
                "message": "PRIVATE_RECEIPT_ERROR_MESSAGE",
                "details": {"raw": "PRIVATE_RECEIPT_ERROR_DETAILS"},
            },
        })
        .to_string();
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":command},
            "tool_response":{
                "success":true,
                "exit_code":0,
                "stdout":stdout,
                "stderr":"PRIVATE_SUCCESS_STDERR",
            },
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
        assert_signal_shape(&batch, "tool_after");
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], expected_kind);
        assert_eq!(signal["priority"], 100);
        assert_eq!(signal["completion_effect"], "none");
        let rendered = serde_json::to_string(&batch).unwrap();
        assert!(!rendered.contains("PRIVATE_RECEIPT_ERROR_MESSAGE"));
        assert!(!rendered.contains("PRIVATE_RECEIPT_ERROR_DETAILS"));
        assert!(!rendered.contains("PRIVATE_SUCCESS_STDERR"));
    }

    let payload = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc config show"},
        "tool_response":{
            "success":true,
            "exit_code":0,
            "stdout":"{\"error\":{\"code\":\"work_failed\"}}",
        },
    });
    assert_eq!(
        tool_hook(&world, "claude", "PostToolUse", &payload),
        serde_json::json!({})
    );

    let recovery = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc config show"},
        "tool_response":{"stdout":serde_json::json!({
            "error":{"code":"graph_projection_failed"},
            "recovery_command":"lwc memory status",
        }).to_string()},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &recovery));
    assert_eq!(batch["signals"][0]["kind"], "graph.recovery");
    assert_eq!(
        batch["signals"][0]["completion_effect"],
        "requires_followup"
    );
    assert_eq!(batch["signals"][0]["next_action"], "lwc memory status");
}

#[test]
fn signal_tool_plan_and_work_receipts_encode_followup_and_completion() {
    let world = World::new(true);
    let operations = live_operation_count(&world);
    let plan_id = "abcdefabcdefabcdefabcdefabcdefab";
    for action in ["create", "advance", "revise"] {
        let stdout = serde_json::json!({
            "action": "updated",
            "plan": {
                "id": plan_id,
                "revision": 4,
                "state": "active",
                "status": "ready",
                "message": "PRIVATE_PLAN_RECEIPT_MESSAGE",
                "result": "PRIVATE_PLAN_RECEIPT_RESULT",
            },
            "next_action": format!("lwc plan brief {plan_id}"),
        })
        .to_string();
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":format!("lwc plan {action} fixture")},
            "tool_response":{"stdout":stdout},
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "plan.resume", "{action}: {batch}");
        assert_eq!(signal["priority"], 80);
        assert_eq!(signal["completion_effect"], "requires_followup");
        assert_eq!(signal["next_action"], format!("lwc plan brief {plan_id}"));
        let rendered = serde_json::to_string(&batch).unwrap();
        assert!(!rendered.contains("PRIVATE_PLAN_RECEIPT_MESSAGE"));
        assert!(!rendered.contains("PRIVATE_PLAN_RECEIPT_RESULT"));
    }

    let blocked = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":format!("lwc plan block {plan_id}")},
        "tool_response":{"stdout":serde_json::json!({
            "plan":{"id":plan_id,"revision":4,"state":"active","status":"blocked"},
        }).to_string()},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &blocked));
    assert_eq!(batch["signals"][0]["kind"], "plan.blocked");
    assert_eq!(batch["signals"][0]["priority"], 60);
    assert_eq!(
        batch["signals"][0]["completion_effect"],
        "satisfies_followup"
    );

    for action in ["complete", "abandon"] {
        let stdout = serde_json::json!({
            "plan": {"id": plan_id, "revision": 5, "state": action},
        })
        .to_string();
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":format!("lwc plan {action} {plan_id}")},
            "tool_response":{"stdout":stdout},
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], "plan.closed");
        assert_eq!(signal["completion_effect"], "satisfies_followup");
    }

    let work_id = "f".repeat(64);
    for (state, kind, priority, completion) in [
        ("queued", "work.resume", 80, "requires_followup"),
        ("running", "work.resume", 80, "requires_followup"),
        ("failed", "work.failed", 60, "none"),
        ("cancelled", "work.failed", 60, "none"),
        ("succeeded", "work.completed", 60, "satisfies_followup"),
    ] {
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":format!("lwc work status {work_id}")},
            "tool_response":{"stdout":work_receipt_stdout(&work_id, state)},
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
        let signal = &batch["signals"][0];
        assert_eq!(signal["kind"], kind, "{state}: {batch}");
        assert_eq!(signal["priority"], priority);
        assert_eq!(signal["completion_effect"], completion);
        if state != "succeeded" {
            assert_eq!(signal["next_action"], format!("lwc work status {work_id}"));
        }
    }

    let recoverable = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":format!("lwc work status {work_id}")},
        "tool_response":{"stdout":serde_json::json!({
            "work":{"id":work_id,"state":"failed","phase":"failed"},
            "recovery_command":format!("lwc work resume {work_id}"),
        }).to_string()},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &recoverable));
    assert_eq!(batch["signals"][0]["kind"], "work.failed");
    assert_eq!(
        batch["signals"][0]["completion_effect"],
        "requires_followup"
    );
    assert_eq!(
        batch["signals"][0]["next_action"],
        format!("lwc work resume {work_id}")
    );
    assert_eq!(live_operation_count(&world), operations);
}

#[test]
fn signal_tool_receipts_use_concrete_close_kinds_and_drop_generic_noise() {
    let world = World::new(true);
    let followup = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc config show"},
        "tool_response":{"stdout":"{\"next_action\":\"memory.status\"}"},
    });
    assert_eq!(
        tool_hook(&world, "claude", "PostToolUse", &followup),
        serde_json::json!({})
    );

    let closed = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc changeset commit safe-name"},
        "tool_response":{"stdout":"{\"status\":\"committed\"}"},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &closed));
    assert_eq!(batch["signals"][0]["kind"], "changeset.closed");
    assert_eq!(
        batch["signals"][0]["completion_effect"],
        "satisfies_followup"
    );

    for (command, kind) in [
        ("lwc sync fixture --abort safe-session", "sync.completed"),
        ("lwc ingest complete 7", "ingest.completed"),
    ] {
        let payload = serde_json::json!({
            "tool_name":"Bash",
            "tool_input":{"command":command},
            "tool_response":{"stdout":"{\"status\":\"completed\"}"},
        });
        let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
        assert_eq!(batch["signals"][0]["kind"], kind);
        assert_eq!(
            batch["signals"][0]["completion_effect"],
            "satisfies_followup"
        );
    }

    let routine = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc config show"},
        "tool_response":{"stdout":"{\"status\":\"ready\"}"},
    });
    assert_eq!(
        tool_hook(&world, "claude", "PostToolUse", &routine),
        serde_json::json!({})
    );
}

#[test]
fn signal_tool_memory_and_graph_work_use_only_canonical_typed_fields() {
    let world = World::new(true);
    let memory = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc memory status"},
        "tool_response":{"stdout":serde_json::json!({
            "database":"/PRIVATE_MEMORY_DATABASE/wiki.db",
            "pending_hints":3,
            "retained":{"events":9,"logical_bytes":850},
            "pressure":{"logical_bytes":850,"max_bytes":1000,"ratio":0.01},
        }).to_string()},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &memory));
    assert_signal_shape(&batch, "tool_after");
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "memory.maintenance");
    assert_eq!(signal["priority"], 60);
    assert_eq!(signal["state"]["pending_hints"], 3);
    assert_eq!(signal["state"]["retained_count"], 9);
    assert_eq!(signal["state"]["logical_bytes"], 850);
    assert_eq!(signal["state"]["max_bytes"], 1000);
    assert_eq!(signal["state"]["pressure_ratio"], 0.85);
    assert_eq!(signal["next_action"], "lwc memory status");
    assert!(
        !serde_json::to_string(&batch)
            .unwrap()
            .contains("PRIVATE_MEMORY_DATABASE")
    );

    let work_id = "a".repeat(64);
    let graph = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc checkpoint restore safe-checkpoint"},
        "tool_response":{"stdout":serde_json::json!({
            "state":"ready",
            "status":"restored",
            "graph_work":{
                "id":work_id,
                "state":"queued",
                "phase":"queued",
                "completed":0,
                "total":12,
                "sequence":4,
                "message":"PRIVATE_GRAPH_WORK_MESSAGE",
            },
            "next_action":format!("lwc work status {work_id}"),
        }).to_string()},
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &graph));
    let signal = &batch["signals"][0];
    assert_eq!(signal["kind"], "work.resume");
    assert_eq!(signal["state"]["state"], "queued");
    assert_eq!(signal["state"]["phase"], "queued");
    assert_eq!(signal["state"]["progress"]["completed"], 0);
    assert_eq!(signal["state"]["progress"]["total"], 12);
    assert_eq!(signal["state"]["progress"]["sequence"], 4);
    assert_eq!(signal["next_action"], format!("lwc work status {work_id}"));
    assert!(
        !serde_json::to_string(&batch)
            .unwrap()
            .contains("PRIVATE_GRAPH_WORK_MESSAGE")
    );

    let lookalike = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":"lwc checkpoint restore safe-checkpoint"},
        "tool_response":{"stdout":serde_json::json!({
            "result":{"details":{"graph_work":{
                "id":"b".repeat(64),
                "state":"queued",
                "phase":"queued",
            }}},
        }).to_string()},
    });
    assert_eq!(
        tool_hook(&world, "claude", "PostToolUse", &lookalike),
        serde_json::json!({})
    );
}

#[test]
fn signal_tool_selector_uses_only_typed_receipt_state_and_is_read_only() {
    let world = World::new(true);
    let operations = live_operation_count(&world);
    let plan_id = "abcdefabcdefabcdefabcdefabcdefab";
    let work_id = "b".repeat(64);
    let stdout = serde_json::json!({
        "work_id": work_id,
        "state": "failed",
        "phase": "failed",
        "completed": 3,
        "total": 10,
        "sequence": 7,
        "plan": {"id": plan_id, "revision": 4},
        "next_action": format!("lwc plan brief {plan_id}"),
        "database": "/PRIVATE_COMBINED_DATABASE/wiki.db",
        "message": "PRIVATE_COMBINED_MESSAGE",
        "result": {"secret":"PRIVATE_COMBINED_RESULT"},
    })
    .to_string();
    let payload = serde_json::json!({
        "tool_name":"Bash",
        "tool_input":{"command":format!("lwc plan advance {plan_id}")},
        "tool_response":{"stdout":stdout,"stderr":"PRIVATE_COMBINED_STDERR"},
        "transcript_path":"/PRIVATE_TRANSCRIPT_PATH",
    });
    let batch = signal_batch(&tool_hook(&world, "claude", "PostToolUse", &payload));
    assert_signal_shape(&batch, "tool_after");
    assert!(batch["signals"].as_array().unwrap().len() <= 3);
    let kinds = batch["signals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(kinds, BTreeSet::from(["plan.resume", "work.failed"]));
    let rendered = serde_json::to_string(&batch).unwrap();
    for secret in [
        "PRIVATE_COMBINED_DATABASE",
        "PRIVATE_COMBINED_MESSAGE",
        "PRIVATE_COMBINED_RESULT",
        "PRIVATE_COMBINED_STDERR",
        "PRIVATE_TRANSCRIPT_PATH",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert_eq!(live_operation_count(&world), operations);
}

#[test]
fn separate_todo_and_plan_readiness_tracks_current_plan_without_sensitive_details() {
    let world = World::new(true);
    world.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
    let todo = world.ok(&["todo", "add", "secret todo title"]);
    let created = world.ok(&[
        "plan",
        "create",
        "tracked release plan",
        "--objective",
        "private objective",
        "--done-when",
        "verified",
        "--step",
        "prepare artifacts",
        "--step",
        "run acceptance",
        "--step",
        "publish release",
    ]);
    let plan = &created["plan"];
    let plan_id = plan["id"].as_str().unwrap();
    let context_id = agent_context("codex", DEFAULT_CODEX_SESSION, "main", "main");
    world.ok(&[
        "todo", "track", todo["todo"]["id"].as_str().unwrap(), "--context", &context_id,
    ]);
    world.ok(&["plan", "track", plan_id, "--context", &context_id]);
    let first = plan["steps"][0]["id"].as_str().unwrap();
    let second = plan["steps"][1]["id"].as_str().unwrap();
    world.ok(&[
        "plan",
        "advance",
        plan_id,
        "--if-revision",
        "1",
        "--done",
        first,
        "--result",
        "artifacts ready",
        "--next",
        second,
    ]);
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    );
    let text = context(&output);
    let value = readiness(&text);
    assert_eq!(value["todo"]["ready"], true);
    assert_eq!(value["todo"]["open"], 1);
    assert!(value["todo"]["list"].as_str().unwrap().contains(&context_id));
    assert_eq!(value["plan"]["ready"], true);
    assert_eq!(value["plan"]["active"], 1);
    assert!(value["plan"]["current"].as_str().unwrap().contains(&context_id));
    let tracking = &value["plan"]["tracking"];
    assert_eq!(tracking["id"], plan_id);
    assert_eq!(tracking["title"], "tracked release plan");
    assert_eq!(tracking["revision"], 2);
    assert_eq!(tracking["progress"]["completed_steps"], 1);
    assert_eq!(tracking["progress"]["terminal_steps"], 1);
    assert_eq!(tracking["progress"]["total_steps"], 3);
    assert_eq!(tracking["current_step"]["title"], "run acceptance");
    assert_eq!(tracking["current_step"]["status"], "in_progress");
    assert_eq!(tracking["next_step"]["title"], "publish release");
    assert_eq!(tracking["brief"], format!("lwc plan brief {plan_id}"));
    assert!(!text.contains("secret todo title"));
    assert!(!text.contains("private objective"));
    assert!(!text.contains("artifacts ready"));
}

#[test]
fn todo_and_plan_hook_omits_enabled_but_unbound_capabilities() {
    let world = World::new(true);
    let disabled = context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    ));
    assert!(readiness(&disabled).get("todo").is_none());
    assert!(readiness(&disabled).get("plan").is_none());

    world.ok(&["config", "set", "--todo", "enabled"]);
    let todo_only = readiness(&context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    )));
    assert!(todo_only.get("todo").is_none());
    assert!(todo_only.get("plan").is_none());

    world.ok(&["config", "set", "--todo", "disabled", "--plan", "enabled"]);
    let plan_only = readiness(&context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    )));
    assert!(plan_only.get("plan").is_none());
    assert!(plan_only.get("todo").is_none());
}

#[test]
fn todo_hook_reminds_three_oldest_due_open_items_and_counts_omitted() {
    let world = World::new(true);
    world.ok(&["config", "set", "--todo", "enabled"]);
    let closed = world.ok(&[
        "todo",
        "add",
        "closed due",
        "--target-at",
        "2000-01-01T00:00:00Z",
    ]);
    let closed_id = closed["todo"]["id"].as_str().unwrap();
    let context_id = agent_context("codex", DEFAULT_CODEX_SESSION, "main", "main");
    world.ok(&["todo", "track", closed_id, "--context", &context_id]);
    world.ok(&[
        "todo",
        "done",
        closed_id,
        "--if-revision",
        "1",
        "--result",
        "closed",
    ]);
    let first = world.ok(&[
        "todo",
        "add",
        "due 1",
        "--target-at",
        "2000-01-01T00:00:00Z",
        "--cue",
        "private cue",
    ]);
    let first_id = first["todo"]["id"].as_str().unwrap();
    world.ok(&["todo", "track", first_id, "--context", &context_id]);
    for title in ["due 2", "due 3", "due 4", "due 5"] {
        let todo = world.ok(&[
            "todo",
            "add",
            title,
            "--parent",
            first_id,
            "--target-at",
            "2000-01-01T00:00:00Z",
        ]);
        world.ok(&[
            "todo", "track", todo["todo"]["id"].as_str().unwrap(), "--context", &context_id,
        ]);
    }
    let future = world.ok(&[
        "todo",
        "add",
        "future",
        "--target-at",
        "2999-01-01T00:00:00Z",
    ]);
    world.ok(&[
        "todo", "track", future["todo"]["id"].as_str().unwrap(), "--context", &context_id,
    ]);

    let text = context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    ));
    let todo = &readiness(&text)["todo"];
    assert_eq!(todo["reminders"].as_array().unwrap().len(), 3);
    assert_eq!(todo["reminders"][0]["title"], "due 1");
    assert_eq!(todo["reminders"][1]["title"], "due 2");
    assert_eq!(todo["reminders"][2]["title"], "due 3");
    assert_eq!(todo["reminders"][1]["parent_id"], first_id);
    assert_eq!(todo["omitted_reminders"], 2);
    assert!(!text.contains("closed due"));
    assert!(!text.contains("future"));
    assert!(!text.contains("private cue"));
}

#[test]
fn plan_tracking_titles_are_bounded_at_unicode_boundaries() {
    let world = World::new(true);
    world.ok(&["config", "set", "--plan", "enabled"]);
    let long_plan = "计划".repeat(300);
    let long_step = "步骤".repeat(300);
    let created = world.ok(&[
        "plan",
        "create",
        &long_plan,
        "--objective",
        "bounded tracking",
        "--done-when",
        "verified",
        "--step",
        &long_step,
    ]);
    let context_id = agent_context("codex", DEFAULT_CODEX_SESSION, "main", "main");
    world.ok(&[
        "plan", "track", created["plan"]["id"].as_str().unwrap(), "--context", &context_id,
    ]);
    let readiness = readiness(&context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"session_id": DEFAULT_CODEX_SESSION}).to_string(),
    )));
    let tracking = &readiness["plan"]["tracking"];
    assert_eq!(tracking["title"].as_str().unwrap().chars().count(), 500);
    assert_eq!(
        tracking["current_step"]["title"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        500
    );
    assert!(tracking["title"].as_str().unwrap().ends_with('…'));
}

#[test]
fn initialized_codegraph_keeps_consent_when_runtime_is_unavailable() {
    let world = World::new(true);
    let index = world.project.join(".lwc/codegraph");
    fs::create_dir_all(&index).unwrap();
    fs::write(index.join("codegraph.db"), b"fixture").unwrap();

    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"source": "startup", "cwd": world.project}).to_string(),
    );
    let readiness = readiness(&context(&output));
    assert_eq!(readiness["code_graph"]["initialized"], true);
    assert_eq!(readiness["code_graph"]["ready"], false);
    assert_eq!(readiness["code_graph"]["requires_consent"], false);
}

#[cfg(unix)]
fn prompt_hook_output(world: &World, binary: &PathBuf, input: &Value) -> Output {
    let mut command = world.command();
    command.env("LWC_CODEGRAPH_BINARY", binary);
    let mut child = command
        .args([
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "UserPromptSubmit",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn boundary_hook_loads_whole_budgeted_pages_without_reading_transcript() {
    let world = World::new(true);
    world.page("oversized", &"x".repeat(30));
    world.page("shared", "keep me");
    world.enable_tag("Rules", "oversized", "20", "10");
    world.ok(&[
        "tag",
        "set",
        "Rules",
        "shared",
        "--priority",
        "10",
        "--reason",
        "core fixture",
    ]);
    world.enable_tag("Operations", "shared", "10", "100");
    let transcript = world.project.join("transcript.jsonl");
    fs::write(&transcript, "TRANSCRIPT_SECRET").unwrap();
    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "source": "startup",
        "cwd": world.project,
        "transcript_path": transcript,
    });
    let output = world.output(
        &[
            "--scope",
            "all",
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &input.to_string(),
    );
    let context = context(&output);
    assert!(context.contains("keep me"));
    assert!(!context.contains(&"x".repeat(30)));
    assert!(!context.contains("TRANSCRIPT_SECRET"));
    assert_eq!(context.matches("keep me").count(), 1);
    assert!(context.contains("Rules"));
    assert!(context.contains("Operations"));
    assert!(context.contains("omitted"));
    assert!(context.contains("reference data"));
}

#[test]
fn duplicate_pages_count_once_per_tag_budget() {
    let world = World::new(true);
    world.page("shared", "aaaa");
    world.page("secondary", "bbbb");
    world.enable_tag("Primary", "shared", "20", "100");
    world.enable_tag("Secondary", "shared", "10", "8");
    world.ok(&[
        "tag",
        "set",
        "Secondary",
        "secondary",
        "--priority",
        "1",
        "--reason",
        "second page",
    ]);

    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &serde_json::json!({"source": "startup", "cwd": world.project}).to_string(),
    );
    let context = context(&output);
    assert!(context.contains("aaaa"));
    assert!(context.contains("bbbb"));
}

#[test]
fn fresh_init_recommends_both_graphs_without_enabling_them() {
    let world = World::new(false);
    let initialized = world.ok(&["init"]);
    let recommendation = &initialized["recommendations"]["lwc_readiness"];

    assert_eq!(recommendation["wiki"]["initialized"], true);
    assert_eq!(recommendation["document_graph"]["enabled"], false);
    assert_eq!(recommendation["document_graph"]["ready"], false);
    assert_eq!(
        recommendation["document_graph"]["projection"]["status"],
        "disabled"
    );
    assert_eq!(recommendation["code_graph"]["initialized"], false);
    assert_eq!(recommendation["authorization"]["mode"], "plain-text");
    assert_eq!(recommendation["authorization"]["recommended_choice"], "1");
    assert_eq!(recommendation["authorization"]["choices"][0]["id"], "1");
    assert_eq!(
        recommendation["authorization"]["choices"][0]["capabilities"],
        serde_json::json!(["document-graph", "code-graph"])
    );

    assert!(!world.project.join(".lwc/config.json").exists());
    assert!(!world.project.join(".lwc/codegraph").exists());
    assert!(!world.home.join(".lwc/runtime/codegraph").exists());
}

#[test]
fn boundary_hook_reports_portable_graph_authorization_without_mutation() {
    let world = World::new(true);
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let readiness = readiness(&context(&output));

    assert_eq!(readiness["wiki"]["initialized"], true);
    assert_eq!(readiness["document_graph"]["enabled"], false);
    assert_eq!(readiness["document_graph"]["ready"], false);
    assert_eq!(readiness["code_graph"]["initialized"], false);
    assert_eq!(readiness["code_graph"]["ready"], false);
    assert_eq!(readiness["authorization"]["mode"], "plain-text");
    assert_eq!(readiness["authorization"]["recommended_choice"], "1");
    assert_eq!(
        readiness["authorization"]["choices"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(
        readiness["authorization"]["prompt"]
            .as_str()
            .unwrap()
            .contains("Reply with 1-4")
    );

    assert!(!world.project.join(".lwc/config.json").exists());
    assert!(!world.project.join(".lwc/codegraph").exists());
    assert!(!world.home.join(".lwc/runtime/codegraph").exists());
}

#[test]
fn lifecycle_adapters_use_official_envelopes_and_turn_hooks_are_silent() {
    let world = World::new(true);
    let cases = [
        ("cursor", "session_start", "additional_context"),
        (
            "gemini",
            "SessionStart",
            "hookSpecificOutput.additionalContext",
        ),
        ("copilot-cli", "sessionStart", "additionalContext"),
        (
            "copilot-vscode",
            "SessionStart",
            "hookSpecificOutput.additionalContext",
        ),
        ("pi", "session_start", "additionalContext"),
    ];
    for (agent, event, path) in cases {
        let output = world.output(&["agent", "hook", "--agent", agent, "--event", event], "{}");
        assert!(
            output.status.success(),
            "{agent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let context = path.split('.').fold(&value, |value, part| {
            part.parse::<usize>()
                .map_or(&value[part], |index| &value[index])
        });
        assert!(
            context
                .as_str()
                .is_some_and(|text| text.contains("LWC_READINESS")),
            "{agent} did not inject readiness through {path}: {value}"
        );
        if agent == "gemini" {
            assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
        }
        if agent == "copilot-vscode" {
            assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
        }
    }
    for (agent, event) in [
        ("hermes", "pre_llm_call"),
        ("antigravity", "pre_invocation"),
        ("generic", "SessionStart"),
    ] {
        let output = world.output(&["agent", "hook", "--agent", agent, "--event", event], "{}");
        assert_eq!(hook_json(&output), serde_json::json!({}), "{agent}");
    }
}

#[test]
fn lifecycle_event_aliases_echo_the_matched_host_literal() {
    let world = World::new(true);
    for agent in ["claude", "codex", "gemini", "copilot-vscode"] {
        let output = world.output(
            &[
                "agent",
                "hook",
                "--agent",
                agent,
                "--event",
                "session-start",
            ],
            "{}",
        );
        let value = hook_json(&output);
        assert_eq!(
            value["hookSpecificOutput"]["hookEventName"], "SessionStart",
            "{agent}: {value}"
        );
    }
}

#[test]
fn boundary_hook_can_report_a_missing_wiki_without_creating_it() {
    let world = World::new(false);
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let readiness = readiness(&context(&output));
    assert_eq!(readiness["wiki"]["initialized"], false);
    assert_eq!(readiness["md_trans"]["enabled"], false);
    assert_eq!(readiness["md_trans"]["setting"], "disabled");
    assert!(readiness["md_trans"]["available_engines"].is_array());
    assert_eq!(
        readiness["md_trans"]["configure"]["anydoc"],
        "lwc --scope project config set --trans anydoc"
    );
    assert_eq!(
        readiness["md_trans"]["configure"]["markitdown"],
        "lwc --scope project config set --trans markitdown"
    );
    assert_eq!(readiness["office"]["setting"], "disabled");
    assert_eq!(readiness["office"]["enabled"], false);
    assert_eq!(readiness["office"]["runtime_installed"], false);
    assert_eq!(readiness["office"]["ready"], false);
    assert_eq!(readiness["office"]["requires_consent"], true);
    assert_eq!(
        readiness["office"]["configure"],
        "lwc --scope global config set --office officecli"
    );
    assert_eq!(readiness["office"]["command"], "lwc office COMMAND ...");
    assert_eq!(readiness["authorization"]["recommended_choice"], "1");
    assert!(!world.project.join(".lwc").exists());
}

#[test]
fn boundary_hook_reports_a_configured_but_missing_trans_executable() {
    let world = World::new(true);
    world.ok(&["config", "set", "--trans", "markitdown"]);
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let mut child = world
        .command()
        .env("PATH", "")
        .args([
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let readiness = readiness(&context(&output));
    assert_eq!(readiness["md_trans"]["setting"], "markitdown");
    assert_eq!(readiness["md_trans"]["origin"], "project");
    assert_eq!(readiness["md_trans"]["enabled"], true);
    assert_eq!(readiness["md_trans"]["executable_available"], false);
    assert_eq!(
        readiness["md_trans"]["available_engines"],
        serde_json::json!([])
    );
}

#[test]
fn boundary_hook_reports_enabled_office_without_downloading_it() {
    let world = World::new(true);
    world.ok(&["--scope", "global", "init"]);
    world.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--office",
        "officecli",
    ]);
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let readiness = readiness(&context(&output));
    assert_eq!(readiness["office"]["setting"], "officecli");
    assert_eq!(readiness["office"]["origin"], "global");
    assert_eq!(readiness["office"]["enabled"], true);
    assert_eq!(readiness["office"]["runtime_installed"], false);
    assert_eq!(readiness["office"]["ready"], false);
    assert_eq!(readiness["office"]["requires_consent"], false);
    assert!(!world.home.join(".lwc/runtime/officecli").exists());
}

#[test]
fn boundary_hook_reports_learning_readiness_without_installing_or_reading_plugin_data() {
    let world = World::new(true);
    world.ok(&["--scope", "global", "init"]);
    world.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--tutor",
        "enabled",
        "--practice",
        "enabled",
    ]);
    fs::create_dir_all(world.home.join(".lwc/plugins/tutor")).unwrap();
    fs::write(
        world.home.join(".lwc/plugins/tutor/private.txt"),
        "sensitive learner profile",
    )
    .unwrap();
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let readiness = readiness(&context(&output));
    for (plugin, enabled) in [("tutor", true), ("book", false), ("practice", true)] {
        assert_eq!(readiness[plugin]["enabled"], enabled, "{plugin}");
        assert_eq!(readiness[plugin]["runtime_installed"], false, "{plugin}");
        assert_eq!(readiness[plugin]["ready"], false, "{plugin}");
        assert_eq!(
            readiness[plugin]["configure"],
            format!("lwc --scope global config set --{plugin} enabled")
        );
        assert_eq!(
            readiness[plugin]["command"],
            format!("lwc {plugin} COMMAND ...")
        );
    }
    let rendered = serde_json::to_string(&readiness).unwrap();
    assert!(!rendered.contains("sensitive learner profile"));
    assert!(!world.home.join(".lwc/runtime").exists());
    assert_eq!(
        fs::read_to_string(world.home.join(".lwc/plugins/tutor/private.txt")).unwrap(),
        "sensitive learner profile"
    );
}

#[test]
fn temporal_memory_readiness_is_bounded_and_hook_is_read_only() {
    let world = World::new(true);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "30",
        "--memory-max-bytes",
        "4096",
    ]);
    world.ok(&[
        "remember",
        "--json",
        r#"{"type":"验证","context":"Hook 不应读取事件正文","observed":["敏感事件正文"]}"#,
    ]);
    let database = world.project.join(".lwc/wiki.db");
    let snapshot = || {
        let conn = rusqlite::Connection::open(&database).unwrap();
        let mut values = Vec::new();
        for table in [
            "memory_events",
            "memory_fragments",
            "memory_changes",
            "memory_evidence",
            "memory_relations",
            "memory_feedback",
            "memory_hint_state",
            "memory_fts",
        ] {
            values.push(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            );
        }
        let state = conn
            .query_row(
                "SELECT record_attempts, inserted_events, idempotent_replays,
                        feedback_useful, feedback_not_useful, age_evictions,
                        capacity_evictions, event_count, logical_bytes
                 FROM memory_state WHERE id = 1",
                [],
                |row| {
                    Ok((0..9)
                        .map(|index| row.get::<_, i64>(index).unwrap())
                        .collect::<Vec<_>>())
                },
            )
            .unwrap();
        values.extend(state);
        values
    };
    let before = snapshot();
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let readiness = readiness(&context(&output));
    assert_eq!(
        readiness["memory"],
        serde_json::json!({
            "setting": "enabled",
            "origin": "project",
            "enabled": true,
            "ready": true,
            "max_age_days": 30,
            "max_bytes": 4096,
            "record": "lwc remember --json '{...}'",
            "recall": "lwc memory recall QUERY --limit 5",
            "status": "lwc memory status",
            "maintain": "lwc memory maintain"
        })
    );
    let rendered = serde_json::to_string(&readiness["memory"]).unwrap();
    assert!(!rendered.contains("敏感事件正文"));
    assert!(!rendered.contains("events"));
    assert!(!rendered.contains("hints"));
    assert_eq!(snapshot(), before);
}

#[test]
fn sync_readiness_reports_only_valid_pending_bounded_metadata_and_is_read_only() {
    let world = World::new(true);
    let sync = world.project.join(".lwc/sync");
    let older = "0123456789abcdef0123456789abcdef";
    let latest = "fedcba9876543210fedcba9876543210";
    let terminal = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let invalid = "not-a-session";
    for session in [older, latest, terminal, invalid] {
        fs::create_dir_all(sync.join(session)).unwrap();
    }
    fs::write(
        sync.join(older).join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": older,
            "mode": "pull",
            "scope": "project",
            "host": "sensitive-old-host",
            "remote_directory": "/sensitive/old/path",
            "phase": "handshake_complete",
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_digest": null,
            "peer_stores": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        sync.join(latest).join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": latest,
            "mode": "merge",
            "scope": "all",
            "host": "sensitive-latest-host",
            "remote_directory": "/sensitive/latest/path",
            "phase": "conflicts",
            "conflict_count": 5,
            "conflict_kinds": ["source", "page", "todo", "plan", "page"],
            "payload": {"secret": "object body must stay private"},
            "created_at_unix_ms": 3,
            "updated_at_unix_ms": 4,
            "peer_digest": null,
            "peer_stores": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        sync.join(terminal).join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": terminal,
            "mode": "push",
            "scope": "global",
            "host": "terminal-host",
            "phase": "completed",
            "created_at_unix_ms": 5,
            "updated_at_unix_ms": 6,
            "peer_digest": null,
            "peer_stores": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        sync.join(invalid).join("state.json"),
        br#"{"phase":"conflicts"}"#,
    )
    .unwrap();

    let before = fs::read(sync.join(latest).join("state.json")).unwrap();
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let sync_readiness = readiness(&context(&output))["sync"].clone();
    assert_eq!(
        sync_readiness,
        serde_json::json!({
            "pending": 2,
            "latest": {
                "session_id": latest,
                "phase": "conflicts",
                "conflicts": {"count": 5, "kinds": ["page", "plan", "source"]},
                "resume": format!("lwc --scope all sync HOST ABS_DIRECTORY --mode merge --resume {latest}")
            }
        })
    );
    let rendered = serde_json::to_string(&sync_readiness).unwrap();
    for secret in [
        "sensitive-old-host",
        "sensitive-latest-host",
        "/sensitive/old/path",
        "/sensitive/latest/path",
        "object body must stay private",
        "terminal-host",
    ] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert_eq!(
        fs::read(sync.join(latest).join("state.json")).unwrap(),
        before
    );
}

#[test]
fn sync_readiness_omits_invalid_terminal_and_broken_sessions_without_breaking_hook() {
    let world = World::new(true);
    let sync = world.project.join(".lwc/sync");
    fs::create_dir_all(sync.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")).unwrap();
    fs::write(
        sync.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .join("state.json"),
        b"not json",
    )
    .unwrap();
    fs::create_dir_all(sync.join("cccccccccccccccccccccccccccccccc")).unwrap();
    fs::write(
        sync.join("cccccccccccccccccccccccccccccccc")
            .join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": "cccccccccccccccccccccccccccccccc",
            "mode": "merge",
            "scope": "project",
            "host": "private-host",
            "phase": "failed",
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_digest": null,
            "peer_stores": []
        }))
        .unwrap(),
    )
    .unwrap();
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let value = readiness(&context(&output));
    assert!(value.get("sync").is_none());
    assert!(value.get("wiki").is_some());
}

#[test]
fn sync_readiness_caps_raw_directory_entries_before_parsing_them() {
    let world = World::new(true);
    let sync = world.project.join(".lwc/sync");
    let mut sample = None;
    for index in 0_u64..96 {
        let session_id = format!("{index:032x}");
        let directory = sync.join(&session_id);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "protocol": 1,
                "session_id": session_id,
                "mode": "merge",
                "scope": "project",
                "host": format!("PRIVATE_RAW_HOST_{index}"),
                "remote_directory": format!("/PRIVATE_RAW_PATH/{index}"),
                "phase": "applying",
                "created_at_unix_ms": index + 1,
                "updated_at_unix_ms": index + 1,
                "peer_digest": null,
                "peer_stores": [],
                "private_payload": "PRIVATE_RAW_SYNC_PAYLOAD",
            }))
            .unwrap(),
        )
        .unwrap();
        if index == 95 {
            sample = Some((path.clone(), fs::read(path).unwrap()));
        }
    }

    let started = Instant::now();
    let value = boundary_hook(&world);
    assert!(started.elapsed() < Duration::from_secs(2));
    let readiness = readiness(hook_context(&value).unwrap());
    assert!(readiness.get("sync").is_none(), "{readiness}");
    assert!(!hook_context(&value).unwrap().contains("LWC_SIGNAL "));
    let rendered = serde_json::to_string(&value).unwrap();
    assert!(!rendered.contains("PRIVATE_RAW_HOST"));
    assert!(!rendered.contains("PRIVATE_RAW_PATH"));
    assert!(!rendered.contains("PRIVATE_RAW_SYNC_PAYLOAD"));
    let (path, before) = sample.unwrap();
    assert_eq!(fs::read(path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn sync_readiness_rejects_a_symlinked_sync_root_before_reading_entries() {
    use std::os::unix::fs::symlink;

    let world = World::new(true);
    let outside = world._temp.path().join("outside-sync");
    let session_id = "abcdefabcdefabcdefabcdefabcdefab";
    let directory = outside.join(session_id);
    fs::create_dir_all(&directory).unwrap();
    let state = directory.join("state.json");
    fs::write(
        &state,
        serde_json::to_vec(&serde_json::json!({
            "protocol": 1,
            "session_id": session_id,
            "mode": "merge",
            "scope": "project",
            "host": "PRIVATE_OUTSIDE_SYNC_HOST",
            "remote_directory": "/PRIVATE_OUTSIDE_SYNC_PATH",
            "phase": "conflicts",
            "conflict_count": 3,
            "conflict_kinds": ["page"],
            "created_at_unix_ms": 1,
            "updated_at_unix_ms": 2,
            "peer_digest": null,
            "peer_stores": [],
            "private_payload": "PRIVATE_OUTSIDE_SYNC_PAYLOAD",
        }))
        .unwrap(),
    )
    .unwrap();
    let before = fs::read(&state).unwrap();
    symlink(&outside, world.project.join(".lwc/sync")).unwrap();

    let started = Instant::now();
    let value = boundary_hook(&world);
    assert!(started.elapsed() < Duration::from_secs(2));
    let context = hook_context(&value).unwrap();
    let readiness = readiness(context);
    assert!(readiness.get("sync").is_none(), "{readiness}");
    assert!(!context.contains("LWC_SIGNAL "), "{value}");
    for secret in [
        "PRIVATE_OUTSIDE_SYNC_HOST",
        "/PRIVATE_OUTSIDE_SYNC_PATH",
        "PRIVATE_OUTSIDE_SYNC_PAYLOAD",
    ] {
        assert!(!context.contains(secret), "leaked {secret}: {context}");
    }
    assert_eq!(fs::read(state).unwrap(), before);
}

#[test]
fn boundary_hook_omits_authorization_when_both_graphs_are_configured() {
    let world = World::new(true);
    fs::write(
        world.project.join(".lwc/config.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {"setting": "grafeo"},
            "trans": {
                "setting": "inherit",
                "timeout_seconds": 120,
                "anydoc_args": [],
                "markitdown_args": []
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::create_dir_all(world.project.join(".lwc/codegraph")).unwrap();
    fs::write(
        world.project.join(".lwc/codegraph/codegraph.db"),
        b"fixture",
    )
    .unwrap();
    world.install_codegraph_runtime_fixture();

    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &["agent", "hook", "--agent", "pi", "--event", "session_start"],
        &input,
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let readiness = readiness(value["additionalContext"].as_str().unwrap());
    assert_eq!(readiness["document_graph"]["enabled"], true);
    assert_eq!(readiness["document_graph"]["ready"], false);
    assert_eq!(
        readiness["document_graph"]["projection"]["status"],
        "pending"
    );
    assert_eq!(readiness["code_graph"]["initialized"], true);
    assert_eq!(readiness["code_graph"]["ready"], true);
    assert!(readiness.get("authorization").is_none());
}

#[test]
fn hook_input_is_capped_and_failures_are_empty_successes() {
    let world = World::new(false);
    let oversized = "x".repeat(65 * 1024);
    for args in [
        vec![
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        vec!["agent", "hook", "--agent", "pi", "--event", "session_start"],
    ] {
        let output = world.output(&args, &oversized);
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::json!({})
        );
    }
}

#[cfg(unix)]
#[test]
fn claude_prompt_hook_uses_codegraph_without_opening_or_creating_a_wiki() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new(false);
    world.install_codegraph_runtime_fixture();
    let fake = world.project.join("fake-codegraph");
    fs::write(
        &fake,
        "#!/bin/sh\nread payload\nprintf '<codegraph_context>fake graph</codegraph_context>'\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let input = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "cwd": "/untrusted/path",
        "prompt": "what calls parse_token?",
        "transcript_path": "/must/not/be/read",
    });
    let output = prompt_hook_output(&world, &fake, &input);
    let context_text = context(&output);
    assert!(context_text.contains("fake graph"));
    assert!(!world.project.join(".lwc/wiki.db").exists());

    let plan_input = serde_json::json!({
        "prompt": "create a plan",
        "transcript_path": "/PRIVATE_TRANSCRIPT_PATH",
    });
    let combined = prompt_hook_output(&world, &fake, &plan_input);
    let combined_context = context(&combined);
    assert!(combined_context.contains("fake graph"));
    let combined_value = hook_json(&combined);
    assert_eq!(
        signal_batch(&combined_value)["signals"][0]["kind"],
        "plan.enable"
    );
    assert!(!combined_context.contains("PRIVATE_TRANSCRIPT_PATH"));

    fs::write(&fake, "#!/bin/sh\nhead -c 21000 /dev/zero | tr '\\0' x\n").unwrap();
    let oversized = prompt_hook_output(&world, &fake, &input);
    assert!(oversized.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&oversized.stdout).unwrap(),
        serde_json::json!({})
    );
    let signal_survives = hook_json(&prompt_hook_output(&world, &fake, &plan_input));
    assert_eq!(
        signal_batch(&signal_survives)["signals"][0]["kind"],
        "plan.enable",
        "CodeGraph failure must not erase an independent signal"
    );

    fs::create_dir_all(world.project.join(".lwc")).unwrap();
    fs::write(world.project.join(".lwc/config.json"), b"not json").unwrap();
    fs::write(
        &fake,
        "#!/bin/sh\nread payload\nprintf '<codegraph_context>survives signal failure</codegraph_context>'\n",
    )
    .unwrap();
    let graph_survives = prompt_hook_output(&world, &fake, &plan_input);
    assert!(context(&graph_survives).contains("survives signal failure"));
    assert!(!context(&graph_survives).contains("LWC_SIGNAL "));
    fs::remove_file(world.project.join(".lwc/config.json")).unwrap();

    fs::write(&fake, "#!/bin/sh\nsleep 3\nprintf late\n").unwrap();
    let started = Instant::now();
    let timed_out = prompt_hook_output(&world, &fake, &plan_input);
    assert!(started.elapsed() < Duration::from_millis(1900));
    assert!(timed_out.status.success());
    let timed_out = hook_json(&timed_out);
    assert_eq!(
        signal_batch(&timed_out)["signals"][0]["kind"],
        "plan.enable",
        "a slow CodeGraph probe must not consume the signal budget"
    );
    assert!(!hook_context(&timed_out).unwrap().contains("late"));
}

#[cfg(unix)]
#[test]
fn claude_prompt_hook_rejects_missing_or_symlinked_runtime_home_without_writes() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    for symlinked in [false, true] {
        let world = World::new(false);
        let warm = world.output(
            &["agent", "hook", "--agent", "claude", "--event", "Unknown"],
            "{}",
        );
        assert!(warm.status.success());
        world.install_codegraph_runtime_fixture();
        let fake = world.project.join("fake-codegraph-must-not-run");
        fs::write(
            &fake,
            "#!/bin/sh\nread payload\nprintf '<codegraph_context>PRIVATE_CODEGRAPH_MUST_NOT_RUN</codegraph_context>'\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

        let runtime = world.codegraph_runtime_fixture_dir();
        let home = runtime.join("home");
        fs::remove_dir(&home).unwrap();
        let outside = world.project.join("PRIVATE_OUTSIDE_CODEGRAPH_HOME");
        if symlinked {
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("PRIVATE_OUTSIDE_BODY"), b"must not change").unwrap();
            symlink(&outside, &home).unwrap();
        }
        let runtime_before = snapshot_tree(&world.home.join(".lwc/runtime"));
        let outside_before = symlinked.then(|| snapshot_tree(&outside));

        let input = serde_json::json!({
            "prompt": "create a plan",
            "transcript_path": "/PRIVATE_TRANSCRIPT_PATH",
        });
        let started = Instant::now();
        let output = prompt_hook_output(&world, &fake, &input);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(1_900),
            "symlinked={symlinked} took {elapsed:?}"
        );
        let value = hook_json(&output);
        let context = hook_context(&value).unwrap();
        assert!(!context.contains("PRIVATE_CODEGRAPH_MUST_NOT_RUN"));
        assert!(!context.contains("PRIVATE_TRANSCRIPT_PATH"));
        assert_eq!(signal_batch(&value)["signals"][0]["kind"], "plan.enable");
        assert_eq!(
            snapshot_tree(&world.home.join(".lwc/runtime")),
            runtime_before,
            "invalid runtime home changed the isolated global runtime tree"
        );
        if let Some(outside_before) = outside_before {
            assert_eq!(snapshot_tree(&outside), outside_before);
        }
        assert!(!world.project.join(".lwc/wiki.db").exists());
    }
}

#[test]
fn codex_and_pi_envelopes_keep_the_same_context_semantics() {
    let world = World::new(true);
    world.page("rule", "same body");
    world.enable_tag("Rules", "rule", "1", "100");
    let input = serde_json::json!({"source": "compact", "cwd": world.project}).to_string();

    let codex = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    assert!(context(&codex).contains("same body"));

    let pi = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "pi",
            "--event",
            "session_compact",
        ],
        &input,
    );
    assert!(pi.status.success());
    let value: Value = serde_json::from_slice(&pi.stdout).unwrap();
    assert!(
        value["additionalContext"]
            .as_str()
            .unwrap()
            .contains("same body")
    );
}

#[test]
fn hook_context_never_cuts_a_page_to_fit_the_hard_cap() {
    let world = World::new(true);
    let body = "界".repeat(100_000);
    world.page("too-large", &body);
    world.enable_tag("Rules", "too-large", "1", "100000");
    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let context = context(&output);
    assert!(context.chars().count() <= 100_000);
    assert!(!context.contains(&body));
    assert!(context.contains("omitted_by_global_budget"));
    assert!(context.contains("\"included\":0"));
}

#[test]
fn kiro_hook_prints_context_without_a_foreign_protocol_wrapper() {
    let world = World::new(true);
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "kiro",
            "--event",
            "SessionStart",
            "--raw",
        ],
        "{}",
    );
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("\nLWC_READINESS "), "{text}");
    assert!(!text.starts_with('{'), "{text}");
}

#[test]
fn locked_wiki_fails_open_without_blocking_the_agent() {
    let world = World::new(true);
    world.page("rule", "locked body");
    world.enable_tag("Rules", "rule", "1", "100");
    let database = world.project.join(".lwc/wiki.db");
    let lock = rusqlite::Connection::open(database).unwrap();
    lock.execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();

    let input = serde_json::json!({"source": "startup", "cwd": world.project}).to_string();
    let started = Instant::now();
    let output = world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "claude",
            "--event",
            "SessionStart",
        ],
        &input,
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(1_900),
        "hook took {elapsed:?}"
    );
    assert!(output.status.success());
    let value = hook_json(&output);
    let context = hook_context(&value).expect("safe completed readiness remains visible");
    assert!(context.contains("LWC_READINESS "), "{value}");
    assert!(!context.contains("locked body"), "{value}");
    assert!(!context.contains("LWC_SIGNAL "), "{value}");
    assert!(value.get("decision").is_none(), "{value}");
    lock.execute_batch("ROLLBACK").unwrap();
}
