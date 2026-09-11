use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct World {
    _root: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}
impl World {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let home = root.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _root: root,
            project,
            home,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env_remove("LWC_PROJECT_ROOT")
            .env_remove("LWC_CODEGRAPH_BINARY")
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn init(&self) {
        self.ok(&["init"]);
        self.ok(&["config", "set", "--plan", "enabled", "--memory", "enabled"]);
    }
}

// BDD: Given a discussion, when an answer is corrected, then raw history survives.
#[test]
fn discussion_preserves_history_and_rejects_conflicts() {
    let w = World::new();
    w.init();
    let call = |v: Value| w.ok(&["discussion", "apply", "--json", &v.to_string()]);
    let start = json!({"id":"d1","context":"lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111","request_id":"r1","if_revision":0,"operations":[{"op":"start","text":"Design","description":"Clarify requirements"}]});
    assert_eq!(call(start.clone())["revision"], 1);
    assert_eq!(call(start)["revision"], 1);
    call(
        json!({"id":"d1","context":"lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111","request_id":"r2","if_revision":1,"operations":[{"op":"question","id":"q1","text":"Which database?"},{"op":"answer","id":"a1","parent":"q1","text":"SQLite"}]}),
    );
    call(
        json!({"id":"d1","context":"lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111","request_id":"r3","if_revision":2,"operations":[{"op":"summary","id":"s1","text":"Use SQLite","refs":["a1"]}]}),
    );
    call(
        json!({"id":"d1","context":"lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111","request_id":"r4","if_revision":3,"operations":[{"op":"revise","id":"a1","text":"SQLite only","reason":"User clarification"}]}),
    );
    let s = w.ok(&[
        "discussion",
        "show",
        "d1",
        "--context",
        "lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111",
    ]);
    assert_eq!(s["items"]["s1"]["stale"], true);
    assert_eq!(s["items"]["a1"]["original"], "SQLite");
    assert_eq!(s["items"]["a1"]["text"], "SQLite only");
    let bad = json!({"id":"d1","context":"lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111","request_id":"bad","if_revision":3,"operations":[{"op":"close"}]});
    assert!(
        !w.run(&["discussion", "apply", "--json", &bad.to_string()])
            .status
            .success()
    );
    assert!(
        !w.run(&[
            "discussion",
            "show",
            "d1",
            "--context",
            "lwcctx-v1-3333333333333333333333333333333333333333333333333333333333333333"
        ])
        .status
        .success()
    );
    let h = w.ok(&[
        "discussion",
        "history",
        "d1",
        "--context",
        "lwcctx-v1-1111111111111111111111111111111111111111111111111111111111111111",
    ]);
    assert_eq!(h["revisions"].as_array().unwrap().len(), 4);
}

// BDD: Given an interrupted question, recovery retains it; invalid batches write nothing.
#[test]
fn discussion_pending_recovery_and_atomic_failure() {
    let w = World::new();
    w.init();
    let input = json!({"id":"d2","context":"lwcctx-v1-2222222222222222222222222222222222222222222222222222222222222222","request_id":"r1","if_revision":0,"operations":[{"op":"start","text":"Recovery"},{"op":"question","id":"q","text":"Ready?"}]});
    w.ok(&["discussion", "apply", "--json", &input.to_string()]);
    let bad = json!({"id":"d2","context":"lwcctx-v1-2222222222222222222222222222222222222222222222222222222222222222","request_id":"r2","if_revision":1,"operations":[{"op":"answer","id":"a","parent":"q","text":"yes"},{"op":"answer","id":"b","parent":"absent","text":"no"}]});
    assert!(
        !w.run(&["discussion", "apply", "--json", &bad.to_string()])
            .status
            .success()
    );
    let s = w.ok(&[
        "discussion",
        "current",
        "d2",
        "--context",
        "lwcctx-v1-2222222222222222222222222222222222222222222222222222222222222222",
    ]);
    assert_eq!(s["revision"], 1);
    assert_eq!(s["pending"], json!(["q"]));
}

// BDD: Closure cannot silently accept pending questions or stale conclusions.
#[test]
fn discussion_close_requires_answer_and_fresh_summary() {
    let w = World::new();
    w.init();
    let call = |v: Value| w.ok(&["discussion", "apply", "--json", &v.to_string()]);
    call(
        json!({"id":"d3","context":"lwcctx-v1-4444444444444444444444444444444444444444444444444444444444444444","request_id":"1","if_revision":0,"operations":[{"op":"start","text":"Closure"},{"op":"question","id":"q","text":"Proceed?"}]}),
    );
    let bad = json!({"id":"d3","context":"lwcctx-v1-4444444444444444444444444444444444444444444444444444444444444444","request_id":"2","if_revision":1,"operations":[{"op":"close"}]});
    assert!(
        !w.run(&["discussion", "apply", "--json", &bad.to_string()])
            .status
            .success()
    );
    call(
        json!({"id":"d3","context":"lwcctx-v1-4444444444444444444444444444444444444444444444444444444444444444","request_id":"3","if_revision":1,"operations":[{"op":"answer","id":"a","parent":"q","text":"Yes"},{"op":"summary","id":"s","text":"Proceed","refs":["a"]},{"op":"close"}]}),
    );
    assert_eq!(
        w.ok(&[
            "discussion",
            "current",
            "d3",
            "--context",
            "lwcctx-v1-4444444444444444444444444444444444444444444444444444444444444444"
        ])["state"],
        "closed"
    );
}

#[test]
fn discussion_local_changes_preserve_unrelated_summaries_and_history_pages() {
    let w = World::new();
    w.init();
    let c = format!("lwcctx-v1-{}", "5".repeat(64));
    let call = |rev: i64, ops: Value| {
        w.ok(&["discussion","apply","--json",&json!({"id":"local","context":c,"request_id":format!("r{rev}"),"if_revision":rev,"operations":ops}).to_string()])
    };
    call(
        0,
        json!([{"op":"start","text":"Local changes"},{"op":"question","id":"q1","text":"Storage?"},{"op":"answer","id":"a1","parent":"q1","text":"SQLite"},{"op":"question","id":"q2","text":"Export?"},{"op":"answer","id":"a2","parent":"q2","text":"JSON"},{"op":"summary","id":"s1","text":"SQLite","refs":["a1"]},{"op":"summary","id":"s2","text":"JSON","refs":["a2"]}]),
    );
    call(
        1,
        json!([{"op":"revise","id":"a1","text":"SQLite only","reason":"User correction"},{"op":"metadata","text":"Updated title","reason":"Clarified scope"}]),
    );
    let state = w.ok(&["discussion", "show", "local", "--context", &c]);
    assert_eq!(state["items"]["s1"]["stale"], true);
    assert_eq!(state["items"]["s2"]["stale"], false);
    call(
        2,
        json!([{"op":"withdraw","id":"a2","reason":"User withdrawal"},{"op":"pause","reason":"User interruption"}]),
    );
    assert!(w.ok(&["discussion", "current", "--context", &c])["discussion"].is_null());
    call(
        3,
        json!([{"op":"resume","reason":"User resumed"},{"op":"restore","id":"a2","reason":"User restored"}]),
    );
    let history = w.ok(&[
        "discussion",
        "history",
        "local",
        "--context",
        &c,
        "--offset",
        "1",
        "--limit",
        "1",
    ]);
    assert_eq!(history["revisions"].as_array().unwrap().len(), 1);
    assert_eq!(history["revisions"][0]["revision"], 2);
    let page = w.ok(&[
        "discussion",
        "show",
        "local",
        "--context",
        &c,
        "--offset",
        "2",
        "--limit",
        "1",
    ]);
    assert_eq!(page["items"].as_object().unwrap().len(), 1);
}

#[test]
fn discussion_migrates_prototype_and_rejects_secret_or_cyclic_summary() {
    let w = World::new();
    w.init();
    let database = w.project.join(".lwc/wiki.db");
    let conn = rusqlite::Connection::open(&database).unwrap();
    conn.execute_batch("PRAGMA user_version=18; UPDATE meta SET value='18' WHERE key='format_version'; DROP TABLE discussion_bindings; DROP TABLE discussion_revisions; DROP TABLE discussions;").unwrap();
    drop(conn);
    let c = format!("lwcctx-v1-{}", "6".repeat(64));
    let input = json!({"id":"migration","context":c,"request_id":"1","if_revision":0,"operations":[{"op":"start","text":"Migration"},{"op":"question","id":"q","text":"Choice?"},{"op":"answer","id":"a","parent":"q","text":"A"},{"op":"summary","id":"s","text":"A","refs":["a"]}]});
    w.ok(&["discussion", "apply", "--json", &input.to_string()]);
    let conn = rusqlite::Connection::open(&database).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        19
    );
    let cycle = json!({"id":"migration","context":c,"request_id":"2","if_revision":1,"operations":[{"op":"revise","id":"s","text":"Cycle","reason":"Bad","refs":["s"]}]});
    assert!(
        !w.run(&["discussion", "apply", "--json", &cycle.to_string()])
            .status
            .success()
    );
    let secret = json!({"id":"migration","context":c,"request_id":"3","if_revision":1,"operations":[{"op":"reply","id":"private","text":"-----BEGIN PRIVATE KEY-----"}]});
    assert!(
        !w.run(&["discussion", "apply", "--json", &secret.to_string()])
            .status
            .success()
    );
    assert_eq!(
        w.ok(&["discussion", "current", "--context", &c])["revision"],
        1
    );
}

#[test]
fn discussion_host_capture_is_exact_idempotent_and_requires_explicit_association() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::process::Stdio;
    let w = World::new();
    w.init();
    let c = format!(
        "lwcctx-v1-{}",
        Sha256::digest(b"lwc-agent-context/v1\0codex\0discussion-test\0subagent\0agent-1")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let input = json!({"id":"capture","context":c,"request_id":"1","if_revision":0,"operations":[{"op":"start","text":"Capture"},{"op":"question","id":"q","text":"What do you need?"}]});
    w.ok(&["discussion", "apply", "--json", &input.to_string()]);
    let exact = "  原文\n第二行  ";
    let payload = json!({"session_id":"discussion-test","agent_id":"agent-1","message_id":"message-1","prompt":exact});
    for _ in 0..2 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&w.project)
            .env("HOME", &w.home)
            .env_remove("LWC_PROJECT_ROOT")
            .args([
                "agent",
                "hook",
                "--agent",
                "codex",
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
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let state = w.ok(&["discussion", "show", "capture", "--context", &c]);
    let replies = state["items"]
        .as_object()
        .unwrap()
        .values()
        .filter(|v| v["kind"] == "reply")
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["original"], exact);
    assert_eq!(state["revision"], 2);
    let current = w.ok(&["discussion", "current", "--context", &c]);
    assert_eq!(current["pending"], json!(["q"]));
    assert_eq!(current["unassigned"], 1);
}

#[test]
fn discussion_summary_tracks_question_changes_and_group_reassignment() {
    let w = World::new();
    w.init();
    let c = format!("lwcctx-v1-{}", "9".repeat(64));
    let call = |rev: i64, ops: Value| {
        w.ok(&["discussion","apply","--json",&json!({"id":"groups","context":c,"request_id":format!("r{rev}"),"if_revision":rev,"operations":ops}).to_string()])
    };
    call(
        0,
        json!([{"op":"start","text":"Groups"},{"op":"question","id":"q1","text":"Storage?"},{"op":"answer","id":"a1","parent":"q1","text":"SQLite"},{"op":"question","id":"q2","text":"Export?"},{"op":"answer","id":"a2","parent":"q2","text":"JSON"},{"op":"summary","id":"s1","text":"SQLite","refs":["a1"]},{"op":"summary","id":"s2","text":"JSON","refs":["a2"]}]),
    );
    call(
        1,
        json!([{"op":"revise","id":"q1","text":"Storage and history?","reason":"User expands question"}]),
    );
    assert_eq!(
        w.ok(&["discussion", "item", "groups", "s1", "--context", &c])["item"]["stale"],
        true
    );
    assert_eq!(
        w.ok(&["discussion", "item", "groups", "s2", "--context", &c])["item"]["stale"],
        false
    );
    call(
        2,
        json!([{"op":"move","id":"a1","parent":"q2","ordinal":5,"reason":"Correct answer association"}]),
    );
    let item = w.ok(&["discussion", "item", "groups", "a1", "--context", &c]);
    assert_eq!(item["item"]["parent"], "q2");
    assert_eq!(item["item"]["original"], "SQLite");
    assert_eq!(
        w.ok(&["discussion", "item", "groups", "s2", "--context", &c])["item"]["stale"],
        true
    );
}

#[test]
fn discussion_compaction_readiness_recovers_only_bound_context() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::process::Stdio;
    let w = World::new();
    w.init();
    let digest = Sha256::digest(b"lwc-agent-context/v1\0codex\0recover-test\0main\0main");
    let c = format!(
        "lwcctx-v1-{}",
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    w.ok(&["discussion","apply","--json",&json!({"id":"recover","context":c,"request_id":"1","if_revision":0,"operations":[{"op":"start","text":"Recover me"},{"op":"question","id":"q","text":"Original pending question"}]}).to_string()]);
    for (session, expected) in [("recover-test", true), ("different-test", false)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&w.project)
            .env("HOME", &w.home)
            .env_remove("LWC_PROJECT_ROOT")
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
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                json!({"session_id":session,"source":"compact"})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).contains("Recover me"),
            expected,
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn discussion_survives_unrelated_changeset_publication() {
    let w = World::new();
    w.init();
    let c = format!("lwcctx-v1-{}", "a".repeat(64));
    w.ok(&["discussion","apply","--json",&json!({"id":"kept","context":c,"request_id":"1","if_revision":0,"operations":[{"op":"start","text":"Keep this discussion"},{"op":"question","id":"q","text":"Keep original?"}]}).to_string()]);
    let input = w.project.join("note.md");
    fs::write(
        &input,
        "# Separate\nA distinct page.\n\n[[separate-page]]\n",
    )
    .unwrap();
    w.ok(&["changeset", "begin", "separate-page"]);
    w.ok(&[
        "--changeset",
        "separate-page",
        "page",
        "put",
        "separate-page",
        "--title",
        "Separate",
        "--summary",
        "An unrelated page for publication isolation.",
        "--file",
        input.to_str().unwrap(),
        "--provenance",
        "agent-observed",
    ]);
    w.ok(&["changeset", "commit", "separate-page"]);
    assert_eq!(
        w.ok(&["discussion", "item", "kept", "q", "--context", &c])["item"]["original"],
        "Keep original?"
    );
    assert_eq!(
        w.ok(&["discussion", "current", "--context", &c])["revision"],
        1
    );
}
