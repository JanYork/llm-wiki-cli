use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
    outside: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        Self {
            _temp: temp,
            project,
            home,
            outside,
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("LWC_PROJECT_ROOT", &self.project)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn err(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        write_file(&self.project.join(relative), content)
    }

    fn write_outside(&self, relative: &str, content: &str) -> PathBuf {
        write_file(&self.outside.join(relative), content)
    }

    fn init(&self) -> Value {
        self.ok(&["init"])
    }

    fn database(&self) -> PathBuf {
        self.project.join(".lwc/wiki.db")
    }
}

fn write_file(path: &Path, content: &str) -> PathBuf {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
    path.to_path_buf()
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn operation_count(database: &Path) -> i64 {
    Connection::open(database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn lint_is_read_only_unless_recording_is_explicit() {
    let world = TestWorld::new();
    world.init();
    let before = operation_count(&world.database());

    let lint = world.ok(&["lint"]);
    assert_eq!(lint["total"], 0);
    assert_eq!(operation_count(&world.database()), before);

    world.ok(&["lint", "--record"]);
    assert_eq!(operation_count(&world.database()), before + 1);
    let log = world.ok(&["log", "--limit", "1"]);
    assert_eq!(log["operations"][0]["action"], "lint");
}

#[test]
fn project_init_adds_a_local_git_exclude_unless_disabled() {
    let world = TestWorld::new();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&world.project)
        .output()
        .unwrap()
        .status;
    assert!(status.success());

    let initialized = world.init();
    assert_eq!(initialized["git_exclude"]["status"], "added");
    let exclude = fs::read_to_string(world.project.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|line| line == "/.lwc/"));

    world.init();
    let exclude = fs::read_to_string(world.project.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.lines().filter(|line| *line == "/.lwc/").count(), 1);

    let disabled = TestWorld::new();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&disabled.project)
        .output()
        .unwrap()
        .status;
    assert!(status.success());
    let initialized = disabled.ok(&["init", "--no-git-exclude"]);
    assert_eq!(initialized["git_exclude"]["status"], "disabled");
    let exclude = fs::read_to_string(disabled.project.join(".git/info/exclude")).unwrap();
    assert!(!exclude.lines().any(|line| line == "/.lwc/"));
}

#[test]
fn removals_preserve_referenced_sources_and_linked_pages() {
    let world = TestWorld::new();
    world.init();

    let unused = world.write("unused.md", "unused evidence");
    let unused_id = world.ok(&["source", "add", as_str(&unused)])["source"]["id"]
        .as_i64()
        .unwrap()
        .to_string();
    world.ok(&["source", "remove", &unused_id]);
    assert!(
        world.ok(&["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let source = world.write("evidence.md", "referenced evidence");
    let source_id = world.ok(&["source", "add", as_str(&source)])["source"]["id"]
        .as_i64()
        .unwrap()
        .to_string();
    let target = world.write("target.md", "target body");
    world.ok(&[
        "page",
        "put",
        "target",
        "--title",
        "Target",
        "--file",
        as_str(&target),
        "--source",
        &source_id,
    ]);
    let linker = world.write("linker.md", "See [[target]].");
    world.ok(&[
        "page",
        "put",
        "linker",
        "--title",
        "Linker",
        "--file",
        as_str(&linker),
        "--source",
        &source_id,
    ]);

    assert_eq!(
        world.err(&["source", "remove", &source_id])["error"]["code"],
        "source_in_use"
    );
    assert_eq!(
        world.err(&["page", "remove", "target"])["error"]["code"],
        "page_in_use"
    );

    world.ok(&["page", "remove", "linker"]);
    world.ok(&["page", "remove", "target"]);
    world.ok(&["source", "remove", &source_id]);
    assert!(
        world.ok(&["page", "list"])["pages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        world.ok(&["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn checkpoint_restore_recovers_the_database_and_keeps_a_safety_copy() {
    let world = TestWorld::new();
    world.init();
    let original = world.write("original.md", "original page body");
    world.ok(&[
        "page",
        "put",
        "original",
        "--title",
        "Original",
        "--file",
        as_str(&original),
    ]);
    assert_eq!(
        world.err(&["checkpoint", "create", "../escape"])["error"]["code"],
        "checkpoint_name_invalid"
    );
    world.ok(&["checkpoint", "create", "baseline"]);

    let changed = world.write("changed.md", "changed page body");
    world.ok(&[
        "page",
        "put",
        "original",
        "--title",
        "Original",
        "--file",
        as_str(&changed),
    ]);
    let extra = world.write("extra.md", "extra page");
    world.ok(&[
        "page",
        "put",
        "extra",
        "--title",
        "Extra",
        "--file",
        as_str(&extra),
    ]);

    let restored = world.ok(&["checkpoint", "restore", "baseline"]);
    assert_eq!(restored["checkpoint"], "baseline");
    assert!(
        restored["safety_checkpoint"]
            .as_str()
            .unwrap()
            .starts_with("pre-restore-")
    );
    assert_eq!(
        world.ok(&["page", "show", "original"])["page"]["body"],
        "original page body"
    );
    assert_eq!(
        world.err(&["page", "show", "extra"])["error"]["code"],
        "page_not_found"
    );
    let checkpoints = world.ok(&["checkpoint", "list"]);
    assert_eq!(checkpoints["checkpoints"].as_array().unwrap().len(), 2);
}

#[test]
fn external_and_sensitive_sources_require_explicit_acknowledgement() {
    let world = TestWorld::new();
    world.init();

    let external = world.write_outside("external.md", "external evidence");
    assert_eq!(
        world.err(&["source", "add", as_str(&external)])["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    world.ok(&[
        "source",
        "add",
        as_str(&external),
        "--allow-external-source",
    ]);

    let sensitive = world.write(
        "private-key.md",
        "-----BEGIN PRIVATE KEY-----\nnot-a-real-key\n-----END PRIVATE KEY-----",
    );
    assert_eq!(
        world.err(&["source", "add", as_str(&sensitive)])["error"]["code"],
        "possible_secret_detected"
    );
    world.ok(&[
        "source",
        "add",
        as_str(&sensitive),
        "--acknowledge-sensitive-source",
    ]);

    let public_certificate = world.write(
        "public.pem",
        "-----BEGIN CERTIFICATE-----\nnot-a-real-certificate\n-----END CERTIFICATE-----",
    );
    world.ok(&["source", "add", as_str(&public_certificate)]);
}

#[test]
fn manifest_paths_are_relative_and_preflight_is_atomic() {
    let world = TestWorld::new();
    world.init();
    world.write("sources/safe.md", "safe manifest evidence");
    world.write(
        "sources/sensitive.md",
        "-----BEGIN PRIVATE KEY-----\nnot-a-real-key\n-----END PRIVATE KEY-----",
    );
    let manifest = world.write(
        "sources/lwc-sources.json",
        &serde_json::to_string_pretty(&json!({
            "sources": [
                {"path": "safe.md", "title": "Safe"},
                {"path": "sensitive.md", "title": "Sensitive"}
            ]
        }))
        .unwrap(),
    );

    assert_eq!(
        world.err(&["source", "add-manifest", as_str(&manifest)])["error"]["code"],
        "possible_secret_detected"
    );
    assert!(
        world.ok(&["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let added = world.ok(&[
        "source",
        "add-manifest",
        as_str(&manifest),
        "--acknowledge-sensitive-source",
    ]);
    assert_eq!(added["created"], 2);
    assert_eq!(added["duplicates"], 0);
    assert_eq!(added["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn ingest_claim_selects_only_the_requested_pending_source() {
    let world = TestWorld::new();
    world.init();
    let first = world.write("first.md", "first evidence");
    let second = world.write("second.md", "second evidence");
    let first_id = world.ok(&["source", "add", as_str(&first)])["source"]["id"]
        .as_i64()
        .unwrap();
    let second_id = world.ok(&["source", "add", as_str(&second)])["source"]["id"]
        .as_i64()
        .unwrap();

    let claimed = world.ok(&["ingest", "claim", &second_id.to_string()]);
    assert_eq!(claimed["job"]["source"]["id"], second_id);
    let next = world.ok(&["ingest", "next"]);
    assert_eq!(next["job"]["source"]["id"], first_id);
}
