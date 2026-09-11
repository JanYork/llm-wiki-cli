use serde_json::{Value, json};
#[cfg(unix)]
use std::path::Path;
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

#[test]
fn contract_reports_all_field_paths_and_examples_before_writing() {
    let w = World::new();
    let schema = w.ok(&["contract", "remember"]);
    assert_eq!(
        schema["schema"]["properties"]["evidence"]["items"]["required"],
        json!(["reference"])
    );
    assert!(!w.project.join(".lwc").exists());
    w.init();
    let output=w.run(&["remember","--json",r#"{"type":"decision","context":"test","decision":"bad","evidence":["bad"],"surprise":true}"#]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    let paths: Vec<_> = error["error"]["details"]["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"$.decision")
            && paths.contains(&"$.evidence[0]")
            && paths.contains(&"$.surprise")
    );
    let receipt = w.ok(&["remember", "--json", &schema["example"].to_string()]);
    assert!(receipt["event"]["id"].is_string());
    assert!(receipt["event"]["fingerprint"].is_null());
    assert!(receipt["pressure"].is_null());
    let full = w.ok(&["memory", "show", receipt["event"]["id"].as_str().unwrap()]);
    assert!(full["event"]["decision"].is_array());
}

#[test]
fn plan_revision_preserves_ids_and_distinguishes_waiver_from_completion() {
    let w = World::new();
    w.init();
    let created = w.ok(&[
        "plan",
        "create",
        "delivery",
        "--objective",
        "ship and load test",
        "--done-when",
        "all tests pass",
        "--step",
        "implement",
        "--step",
        "load test",
    ]);
    let id = created["plan"]["id"].as_str().unwrap();
    let first = created["plan"]["steps"][0]["id"].as_str().unwrap();
    let second = created["plan"]["steps"][1]["id"].as_str().unwrap();
    for invalid in [
        json!({"updates":[{"id":first}]}),
        json!({"constraints":vec!["constraint"; 101]}),
    ] {
        assert!(
            !w.run(&[
                "plan",
                "revise",
                id,
                "--if-revision",
                "1",
                "--reason",
                "invalid",
                "--json",
                &invalid.to_string()
            ])
            .status
            .success()
        );
        assert_eq!(w.ok(&["plan", "show", id])["plan"]["revision"], 1);
    }
    let patch = json!({"objective":"ship with focused correctness checks","done_when":"focused checks pass","updates":[{"id":second,"disposition":"waived","basis":"User explicitly removed the load-test requirement"}]});
    let revised = w.ok(&[
        "plan",
        "revise",
        id,
        "--if-revision",
        "1",
        "--reason",
        "scope reduced",
        "--json",
        &patch.to_string(),
    ]);
    assert_eq!(revised["plan"]["revision"], 2);
    assert_eq!(revised["plan"]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(revised["plan"]["steps"][0]["id"], second);
    assert_eq!(revised["plan"]["steps"][0]["disposition"], "waived");
    let full = w.ok(&["plan", "show", id]);
    assert_eq!(full["plan"]["steps"][0]["id"], first);
    assert_eq!(full["plan"]["steps"][0]["status"], "in_progress");
    assert!(
        !w.run(&[
            "plan",
            "revise",
            id,
            "--if-revision",
            "1",
            "--reason",
            "stale",
            "--json",
            &patch.to_string()
        ])
        .status
        .success()
    );
    assert!(
        !w.run(&[
            "plan",
            "complete",
            id,
            "--if-revision",
            "2",
            "--result",
            "done",
            "--evidence",
            "waiver",
            "--done-when-checked"
        ])
        .status
        .success()
    );
    let history = w.ok(&["plan", "history", id]);
    assert_eq!(
        history["history"][1]["result"]["before"]["objective"],
        "ship and load test"
    );
    let archive = w._root.path().join("plan.lwc.zst");
    w.ok(&["compress", archive.to_str().unwrap()]);
    let restored = World::new();
    restored.ok(&["decompress", archive.to_str().unwrap()]);
    restored.ok(&["config", "set", "--plan", "enabled"]);
    let mut restored_plan = restored.ok(&["plan", "show", id])["plan"].clone();
    // Archive materialization rebases step revisions to the restored plan revision.
    for (step, original) in restored_plan["steps"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(full["plan"]["steps"].as_array().unwrap())
    {
        assert_eq!(step["updated_revision"], full["plan"]["revision"]);
        step["updated_revision"] = original["updated_revision"].clone();
    }
    assert_eq!(restored_plan, full["plan"]);
    assert_eq!(
        restored.ok(&["plan", "history", id])["history"],
        history["history"]
    );
    let reconcile = w.ok(&["plan", "reconcile", id]);
    assert_eq!(reconcile["read_only"], true);
    assert_eq!(w.ok(&["plan", "show", id])["plan"]["revision"], 2);
    w.ok(&[
        "plan",
        "advance",
        id,
        "--if-revision",
        "2",
        "--done",
        first,
        "--result",
        "focused checks passed",
    ]);
    assert!(
        !w.run(&[
            "plan",
            "revise",
            id,
            "--if-revision",
            "3",
            "--reason",
            "rewrite",
            "--json",
            &json!({"updates":[{"id":first,"title":"pretend"}]}).to_string()
        ])
        .status
        .success()
    );
}

#[test]
fn latest_known_event_does_not_claim_live_verification() {
    let w = World::new();
    w.init();
    w.ok(&["remember","--json",r#"{"type":"milestone","context":"routing","outcome":["recorded before current checkout changed"]}"#]);
    assert_eq!(
        w.ok(&["memory", "recall", "routing"])["results"][0]["state"],
        "latest_known"
    );
}

#[cfg(unix)]
fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn native_cli_preserves_bytes_stderr_and_exit_code_with_one_owner() {
    let w = World::new();
    let fake = w._root.path().join("native");
    executable(
        &fake,
        "#!/bin/sh\nprintf '\\377raw\\n'\nprintf 'native error\\n' >&2\nexit 23\n",
    );
    let selected = w.ok(&["cg", "configure", "--executable", fake.to_str().unwrap()]);
    assert_eq!(selected["owner"], "independent");
    let output = w.run(&["cg", "query", "Widget"]);
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(output.stdout, b"\xffraw\n");
    assert_eq!(output.stderr, b"native error\n");
    assert_eq!(
        selected["index"],
        w.project
            .canonicalize()
            .unwrap()
            .join(".codegraph")
            .to_string_lossy()
            .as_ref()
    );
    fs::remove_file(fake).unwrap();
    assert_eq!(w.ok(&["cg", "status"])["installed"], false);
    assert!(!w.run(&["cg", "query", "Widget"]).status.success());
    assert!(!w.project.join(".lwc/codegraph").exists());
}

#[test]
fn freshness_checks_dirty_untracked_missing_and_matching_file_content() {
    use sha2::{Digest, Sha256};
    let w = World::new();
    fs::create_dir_all(w.project.join(".lwc/codegraph")).unwrap();
    fs::write(w.project.join("tracked.rs"), b"fn example() {}\n").unwrap();
    let hash = Sha256::digest(b"fn example() {}\n")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let conn = rusqlite::Connection::open(w.project.join(".lwc/codegraph/codegraph.db")).unwrap();
    conn.execute_batch("CREATE TABLE files(path TEXT PRIMARY KEY, content_hash TEXT)")
        .unwrap();
    conn.execute("INSERT INTO files VALUES('tracked.rs',?1)", [hash])
        .unwrap();
    drop(conn);
    assert_eq!(
        w.ok(&["cg", "check", "tracked.rs", "--require-fresh"])["fresh"],
        true
    );
    for absolute in [
        w.project.join("tracked.rs"),
        w.project.join("tracked.rs").canonicalize().unwrap(),
    ] {
        assert_eq!(
            w.ok(&["cg", "check", absolute.to_str().unwrap(), "--require-fresh"])["fresh"],
            true
        );
    }
    #[cfg(windows)]
    assert_eq!(
        w.ok(&[
            "cg",
            "check",
            &w.project
                .join("tracked.rs")
                .to_string_lossy()
                .to_lowercase(),
            "--require-fresh"
        ])["fresh"],
        true
    );
    fs::write(w.project.join("tracked.rs"), b"fn changed() {}\n").unwrap();
    assert!(
        !w.run(&["cg", "check", "tracked.rs", "--require-fresh"])
            .status
            .success()
    );
    fs::write(w.project.join("new.rs"), b"fn new() {}\n").unwrap();
    assert_eq!(
        w.ok(&["cg", "check", "new.rs"])["files"][0]["state"],
        "not_indexed"
    );
    assert!(
        !w.run(&["cg", "--require-fresh", "--file", "new.rs", "query", "new"])
            .status
            .success()
    );
    fs::remove_file(w.project.join("tracked.rs")).unwrap();
    assert_eq!(
        w.ok(&["cg", "check", "tracked.rs"])["files"][0]["state"],
        "missing_file"
    );
    assert!(!w.run(&["cg", "check", "../outside.rs"]).status.success());
}

#[test]
fn doctor_is_read_only_and_does_not_guess_a_binding_or_freshness() {
    let w = World::new();
    fs::write(w.project.join("tracked.txt"), "same content").unwrap();
    for args in [&["init", "-q"][..], &["add", "tracked.txt"][..]] {
        assert!(
            Command::new("git")
                .current_dir(&w.project)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let index = w.project.join(".git/index");
    let before = fs::read(&index).unwrap();
    fs::write(w.project.join("tracked.txt"), "same content").unwrap();
    let result = w.ok(&["doctor"]);
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(result["agent_context"]["status"], "unbound");
    assert_eq!(result["code_graph"]["freshness"], "unknown");
    assert!(!w.project.join(".lwc").exists());
    assert!(result["project"]["checkout"].is_string());
}

#[cfg(unix)]
#[test]
fn affected_stdin_validates_each_path_before_native_execution() {
    use std::io::Write;
    use std::process::Stdio;
    let w = World::new();
    let fake = w._root.path().join("native");
    executable(&fake, "#!/bin/sh\nprintf '%s\\n' \"$*\"\n");
    w.ok(&["cg", "configure", "--executable", fake.to_str().unwrap()]);
    for (input, success) in [
        ("src/a.rs\nsrc/a file.rs\n", true),
        ("../outside.rs\n", false),
        ("--path=/outside\n", false),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&w.project)
            .env("HOME", &w.home)
            .args(["cg", "affected", "--stdin"])
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
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.success(), success);
        if success {
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "affected src/a.rs src/a file.rs\n"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn freshness_never_follows_an_external_index_symlink() {
    use std::os::unix::fs::symlink;
    let w = World::new();
    let outside = w._root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(w.project.join(".lwc")).unwrap();
    fs::write(outside.join("codegraph.db"), b"do not read").unwrap();
    symlink(&outside, w.project.join(".lwc/codegraph")).unwrap();
    fs::write(w.project.join("a.rs"), b"fn a() {}\n").unwrap();
    let result = w.ok(&["cg", "check", "a.rs"]);
    assert_eq!(result["files"][0]["state"], "index_unreadable");
    assert!(
        !w.run(&["cg", "check", "a.rs", "--require-fresh"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(outside.join("codegraph.db")).unwrap(),
        b"do not read"
    );
}

#[test]
fn unknown_index_evidence_never_passes_freshness() {
    let w = World::new();
    fs::write(w.project.join("a.rs"), "fn a() {}").unwrap();
    let db = w.project.join(".lwc/codegraph/codegraph.db");
    for state in ["index_missing", "unsupported_index", "index_unreadable"] {
        match state {
            "unsupported_index" => {
                fs::create_dir_all(db.parent().unwrap()).unwrap();
                rusqlite::Connection::open(&db)
                    .unwrap()
                    .execute_batch("CREATE TABLE unrelated(id TEXT)")
                    .unwrap();
            }
            "index_unreadable" => fs::write(&db, b"not sqlite").unwrap(),
            _ => (),
        }
        assert_eq!(w.ok(&["cg", "check", "a.rs"])["files"][0]["state"], state);
        assert!(
            !w.run(&["cg", "check", "a.rs", "--require-fresh"])
                .status
                .success()
        );
    }
}
