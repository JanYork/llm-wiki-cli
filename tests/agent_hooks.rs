use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const CODEGRAPH_VERSION: &str = "v1.5.0-lwc.1";

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
        let target = codegraph_target();
        let runtime = self
            .home
            .join(".lwc/runtime/codegraph")
            .join(CODEGRAPH_VERSION)
            .join(target);
        let binary = runtime.join(if cfg!(windows) {
            "bin/codegraph.cmd"
        } else {
            "bin/codegraph"
        });
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

#[test]
fn separate_todo_and_plan_readiness_tracks_current_plan_without_sensitive_details() {
    let world = World::new(true);
    world.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
    world.ok(&["todo", "add", "secret todo title"]);
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
        "{}",
    );
    let text = context(&output);
    let value = readiness(&text);
    assert_eq!(value["todo"]["ready"], true);
    assert_eq!(value["todo"]["open"], 1);
    assert_eq!(value["todo"]["list"], "lwc todo list --limit 20");
    assert_eq!(value["plan"]["ready"], true);
    assert_eq!(value["plan"]["active"], 1);
    assert_eq!(value["plan"]["current"], "lwc plan current --limit 20");
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
fn todo_and_plan_hook_prompts_only_enabled_capabilities() {
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
        "{}",
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
        "{}",
    )));
    assert_eq!(todo_only["todo"]["open"], 0);
    assert_eq!(todo_only["todo"]["list"], "lwc todo list --limit 20");
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
        "{}",
    )));
    assert_eq!(plan_only["plan"]["active"], 0);
    assert!(plan_only["plan"].get("tracking").is_none());
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
    for title in ["due 2", "due 3", "due 4", "due 5"] {
        world.ok(&[
            "todo",
            "add",
            title,
            "--parent",
            first_id,
            "--target-at",
            "2000-01-01T00:00:00Z",
        ]);
    }
    world.ok(&[
        "todo",
        "add",
        "future",
        "--target-at",
        "2999-01-01T00:00:00Z",
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
        "{}",
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
    world.ok(&[
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
    let readiness = readiness(&context(&world.output(
        &[
            "agent",
            "hook",
            "--agent",
            "codex",
            "--event",
            "SessionStart",
        ],
        "{}",
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
fn every_hook_adapter_emits_its_official_context_envelope() {
    let world = World::new(true);
    let cases = [
        ("cursor", "session_start", "additional_context"),
        (
            "gemini",
            "SessionStart",
            "hookSpecificOutput.additionalContext",
        ),
        ("hermes", "pre_llm_call", "context"),
        (
            "antigravity",
            "pre_invocation",
            "injectSteps.0.ephemeralMessage",
        ),
        ("copilot-cli", "sessionStart", "additionalContext"),
        (
            "copilot-vscode",
            "SessionStart",
            "hookSpecificOutput.additionalContext",
        ),
        ("pi", "session_start", "additionalContext"),
        ("generic", "SessionStart", "additionalContext"),
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
            assert!(value["hookSpecificOutput"].get("hookEventName").is_none());
        }
        if agent == "copilot-vscode" {
            assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
        }
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
    let context = context(&output);
    assert!(context.contains("fake graph"));
    assert!(!world.project.join(".lwc/wiki.db").exists());

    fs::write(&fake, "#!/bin/sh\nhead -c 21000 /dev/zero | tr '\\0' x\n").unwrap();
    let oversized = prompt_hook_output(&world, &fake, &input);
    assert!(oversized.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&oversized.stdout).unwrap(),
        serde_json::json!({})
    );

    fs::write(&fake, "#!/bin/sh\nsleep 3\nprintf late\n").unwrap();
    let started = Instant::now();
    let timed_out = prompt_hook_output(&world, &fake, &input);
    assert!(started.elapsed() < Duration::from_millis(2800));
    assert!(timed_out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&timed_out.stdout).unwrap(),
        serde_json::json!({})
    );
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
            "session_before_compact",
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
    assert!(elapsed < Duration::from_secs(2), "hook took {elapsed:?}");
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        serde_json::json!({})
    );
    lock.execute_batch("ROLLBACK").unwrap();
}
