use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct World {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
}
impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let w = Self {
            _temp: temp,
            project,
            home,
        };
        w.ok(&["init"]);
        w.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
        w
    }
    fn output(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.output(args);
        assert!(
            o.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn error(&self, args: &[&str]) -> Value {
        let o = self.output(args);
        assert!(!o.status.success(), "expected failure: {args:?}");
        serde_json::from_slice(&o.stderr).unwrap()
    }
}

#[test]
fn plan_create_advance_block_brief_and_complete() {
    let w = World::new();
    let created = w.ok(&[
        "plan",
        "create",
        "Todo/Plan delivery",
        "--objective",
        "ship usable commands",
        "--done-when",
        "acceptance passes",
        "--constraint",
        "no new deps",
        "--tag",
        "release",
        "--step",
        "schema",
        "--step",
        "cli",
    ]);
    assert_eq!(created["action"], "created");
    let plan = &created["plan"];
    assert_eq!(plan["state"], "active");
    assert_eq!(plan["revision"], 1);
    assert_eq!(plan["id"].as_str().unwrap().len(), 32);
    assert_eq!(plan["steps"][0]["status"], "in_progress");
    assert_eq!(plan["steps"][1]["status"], "pending");
    let id = plan["id"].as_str().unwrap();
    let first = plan["steps"][0]["id"].as_str().unwrap();
    let second = plan["steps"][1]["id"].as_str().unwrap();
    let advanced = w.ok(&[
        "plan",
        "advance",
        id,
        "--if-revision",
        "1",
        "--done",
        first,
        "--result",
        "schema ready",
        "--next",
        second,
    ]);
    assert_eq!(advanced["plan"]["revision"], 2);
    assert_eq!(advanced["plan"]["steps"][1]["status"], "in_progress");
    assert_eq!(
        w.error(&[
            "plan",
            "block",
            id,
            "--if-revision",
            "1",
            "--step",
            second,
            "--reason",
            "bad"
        ])["error"]["code"],
        "plan_revision_conflict"
    );
    assert_eq!(
        w.ok(&[
            "plan",
            "block",
            id,
            "--if-revision",
            "2",
            "--step",
            second,
            "--reason",
            "waiting"
        ])["plan"]["steps"][1]["status"],
        "blocked"
    );
    let brief = w.ok(&["plan", "brief", id]);
    assert_eq!(brief["plan"]["id"], id);
    assert!(brief["terminal_steps"].as_array().unwrap().len() <= 20);
    assert_eq!(
        w.error(&[
            "plan",
            "complete",
            id,
            "--if-revision",
            "3",
            "--result",
            "done",
            "--evidence",
            "tests",
            "--done-when-checked"
        ])["error"]["code"],
        "plan_completion_incomplete"
    );
}

#[test]
fn plan_tracking_is_exact_and_isolated_by_agent_context() {
    let w = World::new();
    let first = w.ok(&[
        "plan", "create", "first", "--objective", "o", "--done-when", "d", "--step",
        "only",
    ]);
    let second = w.ok(&[
        "plan", "create", "second", "--objective", "o", "--done-when", "d", "--step",
        "only",
    ]);
    let first_id = first["plan"]["id"].as_str().unwrap();
    let second_id = second["plan"]["id"].as_str().unwrap();
    let context_a = format!("lwcctx-v1-{}", "a".repeat(64));
    let context_b = format!("lwcctx-v1-{}", "b".repeat(64));

    assert_eq!(
        w.ok(&["plan", "track", first_id, "--context", &context_a])["action"],
        "tracked"
    );
    assert_eq!(
        w.ok(&["plan", "track", first_id, "--context", &context_a])["action"],
        "unchanged"
    );
    assert_eq!(
        w.ok(&["plan", "current", "--context", &context_a])["plan"]["id"],
        first_id
    );
    assert!(w.ok(&["plan", "current", "--context", &context_b])["plan"].is_null());
    assert_eq!(
        w.error(&["plan", "track", second_id, "--context", &context_a])["error"]["code"],
        "plan_tracking_conflict"
    );
    assert_eq!(
        w.ok(&["plan", "untrack", second_id, "--context", &context_a])["action"],
        "unchanged"
    );
    assert_eq!(
        w.ok(&["plan", "untrack", first_id, "--context", &context_a])["action"],
        "untracked"
    );
    assert_eq!(
        w.ok(&["plan", "untrack", first_id, "--context", &context_a])["action"],
        "unchanged"
    );
    assert_eq!(
        w.error(&["plan", "current", "--context", "bad"])["error"]["code"],
        "invalid_agent_context"
    );
    assert_eq!(
        w.error(&[
            "--scope", "all", "plan", "track", first_id, "--context", &context_a,
        ])["error"]["code"],
        "scope_not_supported"
    );
}

#[test]
fn plan_can_finish_and_is_idempotent_by_request() {
    let w = World::new();
    let a = w.ok(&[
        "plan",
        "create",
        "one",
        "--objective",
        "o",
        "--done-when",
        "d",
        "--step",
        "only",
        "--request-id",
        "p-1",
    ]);
    assert_eq!(
        w.ok(&[
            "plan",
            "create",
            "one",
            "--objective",
            "o",
            "--done-when",
            "d",
            "--step",
            "only",
            "--request-id",
            "p-1"
        ])["action"],
        "unchanged"
    );
    assert_eq!(
        w.error(&[
            "plan",
            "create",
            "two",
            "--objective",
            "o",
            "--done-when",
            "d",
            "--step",
            "only",
            "--request-id",
            "p-1"
        ])["error"]["code"],
        "plan_request_conflict"
    );
    let id = a["plan"]["id"].as_str().unwrap();
    let step = a["plan"]["steps"][0]["id"].as_str().unwrap();
    w.ok(&[
        "plan",
        "advance",
        id,
        "--if-revision",
        "1",
        "--done",
        step,
        "--result",
        "ok",
    ]);
    let completed = w.ok(&[
        "plan",
        "complete",
        id,
        "--if-revision",
        "2",
        "--result",
        "shipped",
        "--evidence",
        "gate green",
        "--done-when-checked",
    ]);
    assert_eq!(completed["plan"]["state"], "completed");
    assert_eq!(completed["plan"]["revision"], 3);
    assert_eq!(w.ok(&["plan", "current"])["returned"], 0);
    assert_eq!(
        w.error(&["--changeset", "x", "plan", "list"])["error"]["code"],
        "changeset_command_unsupported"
    );
}

#[test]
fn plan_json_revision_search_and_abandon_work() {
    let w = World::new();
    let raw = r#"{"title":"重构","objective":"减少复杂度","done_when":"测试通过","tags":["工程"],"constraints":["不加依赖"],"steps":[{"title":"旧步骤","verify":null}],"request_id":null}"#;
    let created = w.ok(&["plan", "create", "--json", raw]);
    let id = created["plan"]["id"].as_str().unwrap();
    let revised = w.ok(&[
        "plan",
        "revise",
        id,
        "--if-revision",
        "1",
        "--reason",
        "发现更短路径",
        "--json",
        r#"{"steps":[{"title":"新步骤","verify":"unit"}],"focal":0}"#,
    ]);
    assert_eq!(revised["plan"]["revision"], 2);
    assert_eq!(revised["plan"]["steps"][0]["status"], "skipped");
    assert_eq!(revised["plan"]["steps"][1]["status"], "in_progress");
    assert_eq!(
        w.ok(&["plan", "search", "复杂度", "--tag", "工程"])["returned"],
        1
    );
    assert_eq!(
        w.ok(&[
            "plan",
            "abandon",
            id,
            "--if-revision",
            "2",
            "--reason",
            "superseded"
        ])["plan"]["state"],
        "abandoned"
    );
}
