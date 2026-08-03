use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
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

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else if !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("wiki.db")
            {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn knowledge_counts(database: &Path) -> Vec<i64> {
    let connection =
        Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    [
        "sources",
        "source_path_revisions",
        "pages",
        "page_sources",
        "operations",
    ]
    .into_iter()
    .map(|table| {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    })
    .collect()
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
    assert_eq!(
        Connection::open(world.database())
            .unwrap()
            .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
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
fn source_remove_never_rolls_a_tracked_path_back_to_stale_content() {
    let world = TestWorld::new();
    world.init();

    let head_path = world.write("head.md", "head-a");
    let head_a = world.ok(&["source", "add", as_str(&head_path)])["source"]["id"]
        .as_i64()
        .unwrap();
    fs::write(&head_path, "head-b").unwrap();
    let head_b = world.ok(&["source", "add", as_str(&head_path)])["source"]["id"]
        .as_i64()
        .unwrap();

    let history_path = world.write("history.md", "history-a");
    let history_a = world.ok(&["source", "add", as_str(&history_path)])["source"]["id"]
        .as_i64()
        .unwrap();
    fs::write(&history_path, "history-b").unwrap();
    let history_b = world.ok(&["source", "add", as_str(&history_path)])["source"]["id"]
        .as_i64()
        .unwrap();

    let historical = world.ok(&["source", "remove", &history_a.to_string()]);
    assert_eq!(historical["removed_path_revisions"], 1);
    assert!(historical["untracked_paths"].as_array().unwrap().is_empty());
    let still_current = world.ok(&["source", "status", &history_b.to_string()]);
    assert_eq!(still_current["checks"][0]["head_revision"], 2);
    assert_eq!(still_current["checks"][0]["filesystem_state"], "current");

    let current_head = world.ok(&["source", "remove", &head_b.to_string()]);
    assert_eq!(current_head["removed_path_revisions"], 2);
    assert_eq!(current_head["untracked_paths"], json!(["head.md"]));
    let previous = world.ok(&["source", "status", &head_a.to_string()]);
    assert!(previous["checks"].as_array().unwrap().is_empty());
    assert_eq!(previous["untracked_source_ids"], json!([head_a]));

    let conn = Connection::open(world.database()).unwrap();
    let head_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM source_path_revisions WHERE tracked_path = 'head.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        head_rows, 0,
        "deleting a head must not reveal an older head"
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
fn source_diff_requires_both_external_and_sensitive_acknowledgements() {
    let world = TestWorld::new();
    world.init();
    let external = world.write_outside("review.md", "public evidence\n");
    let added = world.ok(&[
        "source",
        "add",
        as_str(&external),
        "--allow-external-source",
    ]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let sensitive = "-----BEGIN PRIVATE KEY-----\nnot-a-real-key\n-----END PRIVATE KEY-----\n";
    fs::write(&external, sensitive).unwrap();
    let before = operation_count(&world.database());

    let no_flags = world.err(&["source", "diff", &source_id]);
    assert_eq!(
        no_flags["error"]["code"],
        "external_source_requires_acknowledgement"
    );

    let external_only = world.err(&["source", "diff", &source_id, "--allow-external-source"]);
    assert_eq!(external_only["error"]["code"], "possible_secret_detected");
    assert!(!external_only.to_string().contains("not-a-real-key"));

    let sensitive_only = world.err(&[
        "source",
        "diff",
        &source_id,
        "--acknowledge-sensitive-source",
    ]);
    assert_eq!(
        sensitive_only["error"]["code"],
        "external_source_requires_acknowledgement"
    );

    let allowed = world.ok(&[
        "source",
        "diff",
        &source_id,
        "--allow-external-source",
        "--acknowledge-sensitive-source",
    ]);
    assert_eq!(allowed["changed"], true);
    assert_eq!(
        allowed["to"]["tracked_path"],
        fs::canonicalize(&external)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    );
    assert!(
        allowed["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("not-a-real-key")
    );
    assert_eq!(operation_count(&world.database()), before);
}

#[test]
fn source_status_diff_and_refs_leave_current_wiki_state_unchanged() {
    let world = TestWorld::new();
    world.init();
    let source = world.write("evidence.md", "limit=3\n");
    let added = world.ok(&["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let page = world.write("page.md", "The configured limit is three.");
    world.ok(&[
        "page",
        "put",
        "limit",
        "--title",
        "Limit",
        "--file",
        as_str(&page),
        "--source",
        &source_id,
    ]);
    fs::write(&source, "limit=4\n").unwrap();

    let database = world.database();
    let wal = PathBuf::from(format!("{}-wal", database.display()));
    let before_counts = knowledge_counts(&database);
    let before_files = snapshot_files(&world.project.join(".lwc"));
    let before_database = fs::read(&database).unwrap();
    let before_wal = fs::read(&wal).ok();

    assert_eq!(
        world.ok(&["source", "status", &source_id])["checks"][0]["filesystem_state"],
        "modified"
    );
    assert_eq!(world.ok(&["source", "diff", &source_id])["changed"], true);
    assert_eq!(
        world.ok(&["source", "refs", &source_id, "--limit", "1000"])["pages"][0]["slug"],
        "limit"
    );

    let after_database = fs::read(&database).unwrap();
    let after_wal = fs::read(&wal).ok();
    assert!(
        after_database == before_database,
        "database bytes changed: {} -> {}",
        before_database.len(),
        after_database.len()
    );
    assert!(
        after_wal == before_wal,
        "WAL bytes changed: {:?} -> {:?}",
        before_wal.as_ref().map(Vec::len),
        after_wal.as_ref().map(Vec::len)
    );
    assert_eq!(knowledge_counts(&database), before_counts);
    assert_eq!(snapshot_files(&world.project.join(".lwc")), before_files);
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
    let revision_count: i64 = Connection::open(world.database())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revision_count, 0, "failed preflight must not track paths");

    let added = world.ok(&[
        "source",
        "add-manifest",
        as_str(&manifest),
        "--acknowledge-sensitive-source",
    ]);
    assert_eq!(added["created"], 2);
    assert_eq!(added["duplicates"], 0);
    assert_eq!(added["sources"].as_array().unwrap().len(), 2);
    let status = world.ok(&["source", "status", "--all"]);
    assert_eq!(status["checks"].as_array().unwrap().len(), 2);
}

#[test]
fn directory_sensitive_preflight_fails_before_sources_or_paths_are_committed() {
    let world = TestWorld::new();
    world.init();
    world.write("batch/a-safe.md", "safe evidence");
    world.write(
        "batch/z-sensitive.md",
        "-----BEGIN PRIVATE KEY-----\nnot-a-real-key\n-----END PRIVATE KEY-----",
    );

    let error = world.err(&["source", "add-dir", as_str(&world.project.join("batch"))]);
    assert_eq!(error["error"]["code"], "possible_secret_detected");
    assert!(
        world.ok(&["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let revisions: i64 = Connection::open(world.database())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revisions, 0);
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
