use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Instant,
};
#[cfg(unix)]
use std::{process::Stdio, thread, time::Duration};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _temp: temp,
            project,
            home,
        }
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn command_in_project_root(&self, cwd: &Path, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("LWC_PROJECT_ROOT", root)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.command(cwd, args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn err(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.command(cwd, args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.project.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn downgrade_to_v10(database: &Path) {
    let conn = Connection::open(database).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE span_fts;
         DROP TABLE IF EXISTS span_fts_data;
         DROP TABLE IF EXISTS span_fts_idx;
         DROP TABLE IF EXISTS span_fts_content;
         DROP TABLE IF EXISTS span_fts_docsize;
         DROP TABLE IF EXISTS span_fts_config;
         DROP TABLE graph_deltas;
         DROP TABLE graph_generations;
         DROP TABLE graph_projection_state;
         DROP TABLE term_pair_totals;
         DROP TABLE term_pair_contributions;
         DROP TABLE graph_occurrences;
         DROP TABLE graph_edges;
         DROP TABLE graph_nodes;
         DROP TABLE document_index_state;
         UPDATE meta SET value = '10' WHERE key = 'format_version';
         PRAGMA user_version = 10;
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

fn prepare_generating_source_with_summary_only(world: &TestWorld, term: &str) -> String {
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/source.md", term);
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let claimed = world.ok(&world.project, &["ingest", "next"]);
    assert_eq!(claimed["job"]["source"]["id"], added["source"]["id"]);

    let analysis = world.write(
        "analysis.md",
        "# Analysis\n\n- Source summary exists.\n- No non-source derived page exists yet.\n",
    );
    world.ok(
        &world.project,
        &["ingest", "analyze", &source_id, "--file", as_str(&analysis)],
    );

    let summary = world.write("source-summary.md", "Source summary body.");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "source-summary",
            "--title",
            "Source summary",
            "--kind",
            "source",
            "--file",
            as_str(&summary),
            "--source",
            &source_id,
        ],
    );

    source_id
}

fn put_page(world: &TestWorld, slug: &str, title: &str, body: &str) {
    let file = world.write(&format!("pages/{slug}.md"), body);
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            slug,
            "--title",
            title,
            "--file",
            as_str(&file),
            "--provenance",
            "agent-observed",
        ],
    );
}

fn stage_clean_linked_pages(world: &TestWorld, changeset: &str, prefix: &str) {
    world.ok(&world.project, &["changeset", "begin", changeset]);
    let first_slug = format!("{prefix}-first");
    let second_slug = format!("{prefix}-second");
    for (slug, title, body) in [
        (
            first_slug.as_str(),
            "First",
            format!("first [[{second_slug}]]"),
        ),
        (
            second_slug.as_str(),
            "Second",
            format!("second [[{first_slug}]]"),
        ),
    ] {
        let file = world.write(&format!("{slug}.md"), &body);
        world.ok(
            &world.project,
            &[
                "--changeset",
                changeset,
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                title,
                "--file",
                as_str(&file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
}

#[test]
fn changeset_stages_reads_and_discard_without_touching_live_wiki() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let before_operations: i64 = Connection::open(&database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let before_index = fs::read(world.project.join(".lwc/wiki/index.md")).unwrap();

    let begun = world.ok(&world.project, &["changeset", "begin", "ingest-42"]);
    assert_eq!(begun["status"], "draft");
    assert_eq!(begun["name"], "ingest-42");
    assert!(begun["duration_ms"].is_u64());
    let listed = world.ok(&world.project, &["changeset", "list"]);
    assert_eq!(listed["changesets"][0]["name"], "ingest-42");

    let body = world.write("draft-page.md", "draft-only unique knowledge");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "ingest-42",
            "page",
            "put",
            "draft-only",
            "--title",
            "Draft Only",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ],
    );

    let live_missing = world.err(&world.project, &["page", "show", "draft-only"]);
    assert_eq!(live_missing["error"]["code"], "page_not_found");
    let staged = world.ok(
        &world.project,
        &["--changeset", "ingest-42", "page", "show", "draft-only"],
    );
    assert_eq!(staged["page"]["body"], "draft-only unique knowledge");

    let shown = world.ok(&world.project, &["changeset", "show", "ingest-42"]);
    assert_eq!(shown["status"], "draft");
    assert_eq!(shown["empty"], false);
    assert_eq!(shown["conflict"], false);
    assert!(shown["staged_operation_count"].as_u64().unwrap() >= 1);

    let after_operations: i64 = Connection::open(&database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after_operations, before_operations);
    assert_eq!(
        fs::read(world.project.join(".lwc/wiki/index.md")).unwrap(),
        before_index
    );

    let discarded = world.ok(&world.project, &["changeset", "discard", "ingest-42"]);
    assert_eq!(discarded["status"], "discarded");
    assert!(!world.project.join(".lwc/changesets/ingest-42.db").exists());
}

#[test]
fn changeset_begin_allocation_is_independent_of_live_wiki_size() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = PathBuf::from(initialized["database"].as_str().unwrap());
    let conn = Connection::open(&database).unwrap();
    conn.execute(
        "INSERT INTO pages(
             slug, title, kind, summary, body, structural_navigation,
             created_at, updated_at
         ) VALUES ('large-live-page', 'Large live page', 'concept', '', ?1, 0,
                   'now', 'now')",
        ["x".repeat(4 * 1024 * 1024)],
    )
    .unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(conn);

    let begun = world.ok(&world.project, &["changeset", "begin", "sparse-begin"]);
    let draft = PathBuf::from(begun["database"].as_str().unwrap());
    let allocated = [
        draft.clone(),
        PathBuf::from(format!("{}-wal", draft.display())),
        PathBuf::from(format!("{}-shm", draft.display())),
    ]
    .into_iter()
    .map(|path| fs::metadata(path).map(|value| value.len()).unwrap_or(0))
    .sum::<u64>();

    assert!(
        allocated <= 1024 * 1024,
        "empty sparse draft allocated {allocated} bytes from unrelated live content"
    );
}

#[test]
fn changeset_commit_preserves_unrelated_live_mutations() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "entity-local"]);
    let staged = world.write("staged-local.md", "staged local alpha beta");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "entity-local",
            "page",
            "put",
            "staged-local",
            "--title",
            "Staged local",
            "--summary",
            "Staged local",
            "--file",
            as_str(&staged),
            "--provenance",
            "agent-observed",
        ],
    );
    put_page(
        &world,
        "unrelated-live",
        "Unrelated live",
        "unrelated live mutation must survive",
    );

    let committed = world.ok(
        &world.project,
        &[
            "changeset",
            "commit",
            "entity-local",
            "--allow-lint-issues",
            "--reason",
            "isolated concurrency regression fixture",
        ],
    );

    assert_eq!(committed["status"], "committed");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "unrelated-live"])["page"]["body"],
        "unrelated live mutation must survive"
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "staged-local"])["page"]["body"],
        "staged local alpha beta"
    );
}

#[test]
fn concurrent_same_entity_writes_return_one_typed_conflict() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let first = world.write("same-entity-first.md", "first concurrent body alpha beta");
    let second = world.write(
        "same-entity-second.md",
        "second concurrent body gamma delta",
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let outputs = std::thread::scope(|scope| {
        [first, second]
            .into_iter()
            .map(|body| {
                let project = world.project.clone();
                let home = world.home.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    Command::new(env!("CARGO_BIN_EXE_lwc"))
                        .current_dir(project)
                        .env("HOME", home)
                        .env("LWC_TEST_PAGE_PUT_PREWRITE_DELAY_MS", "250")
                        .args([
                            "page",
                            "put",
                            "same-entity",
                            "--title",
                            "Same entity",
                            "--file",
                            as_str(&body),
                            "--provenance",
                            "agent-observed",
                        ])
                        .output()
                        .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });

    let succeeded = outputs
        .iter()
        .filter(|output| output.status.success())
        .count();
    let conflicts = outputs
        .iter()
        .filter(|output| !output.status.success())
        .map(stderr_json)
        .filter(|error| error["error"]["code"] == "entity_conflict")
        .count();
    assert_eq!(succeeded, 1, "outputs: {outputs:?}");
    assert_eq!(conflicts, 1, "outputs: {outputs:?}");
}

#[test]
fn changeset_commit_is_atomic_conflict_aware_and_rollback_guarded() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "publish"]);

    let draft_body = world.write("draft-commit.md", "candidate knowledge [[candidate-index]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "publish",
            "page",
            "put",
            "candidate-page",
            "--title",
            "Candidate Page",
            "--summary",
            "Candidate knowledge",
            "--file",
            as_str(&draft_body),
            "--provenance",
            "agent-observed",
        ],
    );
    let index_body = world.write("draft-index.md", "[[candidate-page]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "publish",
            "page",
            "put",
            "candidate-index",
            "--title",
            "Candidate Index",
            "--summary",
            "Candidate navigation",
            "--file",
            as_str(&index_body),
            "--provenance",
            "agent-observed",
        ],
    );

    let committed = world.ok(&world.project, &["changeset", "commit", "publish"]);
    assert_eq!(committed["status"], "committed");
    assert_eq!(committed["materialized"], true);
    assert_eq!(committed["wal_checkpointed"], false);
    for field in [
        "duration_ms",
        "checkpoint_ms",
        "locked_publish_ms",
        "cleanup_ms",
        "materialization_ms",
    ] {
        assert!(committed[field].is_u64(), "missing commit timing {field}");
    }
    let changeset_id = committed["changeset_id"].as_str().unwrap().to_string();
    let database = world.project.join(".lwc/wiki.db");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "candidate-page"])["page"]["body"],
        "candidate knowledge [[candidate-index]]"
    );
    let (status, checkpoint, post_revision): (String, String, String) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT status, pre_commit_checkpoint, post_revision
                 FROM changesets WHERE id = ?1",
            [&changeset_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "committed");
    assert_eq!(post_revision.len(), 64);
    assert!(
        world
            .project
            .join(".lwc/checkpoints")
            .join(format!("{checkpoint}.db"))
            .is_file()
    );
    assert!(!world.project.join(".lwc/changesets/publish.db").exists());
    let retried = world.err(&world.project, &["changeset", "commit", "publish"]);
    assert_eq!(retried["error"]["code"], "changeset_not_found");

    let rolled_back = world.ok(&world.project, &["changeset", "rollback", &changeset_id]);
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(rolled_back["wal_checkpointed"], false);
    assert!(rolled_back["checkpoint"].as_str().is_some());
    for field in [
        "duration_ms",
        "checkpoint_ms",
        "locked_rollback_ms",
        "materialization_ms",
    ] {
        assert!(
            rolled_back[field].is_u64(),
            "missing rollback timing {field}"
        );
    }
    let rollback_revision = rolled_back["rollback_revision"].clone();
    assert_eq!(
        world.err(&world.project, &["page", "show", "candidate-page"])["error"]["code"],
        "page_not_found"
    );
    put_page(
        &world,
        "later-after-rollback",
        "Later After Rollback",
        "must survive an idempotent rollback retry",
    );
    let repeated = world.ok(&world.project, &["changeset", "rollback", &changeset_id]);
    assert_eq!(repeated["status"], "rolled_back");
    assert_eq!(repeated["changeset_id"], changeset_id);
    assert_eq!(repeated["rollback_revision"], rollback_revision);
    assert_eq!(
        world.ok(&world.project, &["page", "show", "later-after-rollback"])["page"]["title"],
        "Later After Rollback"
    );

    world.ok(&world.project, &["changeset", "begin", "stale"]);
    let stale_body = world.write("stale.md", "stale draft");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "stale",
            "page",
            "put",
            "stale-page",
            "--title",
            "Stale Page",
            "--file",
            as_str(&stale_body),
            "--provenance",
            "agent-observed",
        ],
    );
    put_page(&world, "stale-page", "Concurrent Live", "live wins");
    let conflict = world.err(&world.project, &["changeset", "commit", "stale"]);
    assert_eq!(conflict["error"]["code"], "changeset_conflict");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "stale-page"])["page"]["body"],
        "live wins"
    );
}

#[test]
fn changeset_commit_does_not_fall_back_to_an_older_commit_with_a_reused_name() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "reused", "first-version");
    let first = world.ok(&world.project, &["changeset", "commit", "reused"]);

    stage_clean_linked_pages(&world, "reused", "second-version");
    let second = world.ok(&world.project, &["changeset", "commit", "reused"]);
    assert_ne!(first["changeset_id"], second["changeset_id"]);
    world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            second["changeset_id"].as_str().unwrap(),
        ],
    );

    let error = world.err(&world.project, &["changeset", "commit", "reused"]);
    assert_eq!(error["error"]["code"], "changeset_not_found");
}

#[test]
fn changeset_rollback_preserves_an_unrelated_later_live_write() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "rollback-conflict"]);
    let first = world.write("rollback-first.md", "first [[rollback-second]]");
    let second = world.write("rollback-second.md", "second [[rollback-first]]");
    for (slug, title, summary, file) in [
        ("rollback-first", "Rollback First", "First", &first),
        ("rollback-second", "Rollback Second", "Second", &second),
    ] {
        world.ok(
            &world.project,
            &[
                "--changeset",
                "rollback-conflict",
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                summary,
                "--file",
                as_str(file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-conflict"],
    );
    let id = committed["changeset_id"].as_str().unwrap();
    put_page(&world, "later-live", "Later Live", "must survive");

    let rolled_back = world.ok(&world.project, &["changeset", "rollback", id]);
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "later-live"])["page"]["body"],
        "must survive"
    );
    assert_eq!(
        world.err(&world.project, &["page", "show", "rollback-first"])["error"]["code"],
        "page_not_found"
    );
}

#[test]
fn changeset_rollback_conflicts_only_when_a_touched_page_changed_later() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "rollback-entity-conflict", "rollback-entity");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-entity-conflict"],
    );
    let id = committed["changeset_id"].as_str().unwrap();
    put_page(
        &world,
        "rollback-entity-first",
        "Changed Later",
        "the touched entity changed after commit",
    );

    let conflict = world.err(&world.project, &["changeset", "rollback", id]);
    assert_eq!(conflict["error"]["code"], "changeset_rollback_conflict");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "rollback-entity-first"])["page"]["body"],
        "the touched entity changed after commit"
    );
}

#[test]
fn changeset_rollback_rejects_an_invalid_checkpoint_without_any_live_side_effect() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "invalid-checkpoint", "checkpoint-page");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "invalid-checkpoint"],
    );
    let changeset_id = committed["changeset_id"].as_str().unwrap();
    let checkpoint = committed["checkpoint"].as_str().unwrap();
    let checkpoint_directory = world.project.join(".lwc/checkpoints");
    let checkpoint_path = checkpoint_directory.join(format!("{checkpoint}.db"));
    let saved = checkpoint_directory.join(format!("{checkpoint}.saved"));
    fs::rename(&checkpoint_path, &saved).unwrap();
    fs::write(&checkpoint_path, "not a SQLite database").unwrap();

    let database = world.project.join(".lwc/wiki.db");
    let before: (String, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let before_checkpoints = fs::read_dir(&checkpoint_directory).unwrap().count();

    let error = world.err(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(error["error"]["code"], "changeset_corrupt");
    let after: (String, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        fs::read_dir(&checkpoint_directory).unwrap().count(),
        before_checkpoints
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "checkpoint-page-first"])["page"]["title"],
        "First"
    );
}

#[test]
fn changeset_commit_rejects_empty_and_lint_failures_before_checkpointing() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let live_revision = || {
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };

    world.ok(&world.project, &["changeset", "begin", "empty"]);
    let before = live_revision();
    let empty = world.err(&world.project, &["changeset", "commit", "empty"]);
    assert_eq!(empty["error"]["code"], "changeset_empty");
    assert_eq!(live_revision(), before);
    assert!(!world.project.join(".lwc/checkpoints").exists());

    world.ok(&world.project, &["changeset", "begin", "lint-failing"]);
    let bad_body = world.write("lint-failing.md", "uncited and orphaned");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "lint-failing",
            "page",
            "put",
            "lint-failing",
            "--title",
            "Lint failing",
            "--file",
            as_str(&bad_body),
        ],
    );
    let lint = world.err(&world.project, &["changeset", "commit", "lint-failing"]);
    assert_eq!(lint["error"]["code"], "changeset_lint_failed");
    assert_eq!(live_revision(), before);
    assert!(!world.project.join(".lwc/checkpoints").exists());

    let missing_reason = world.err(
        &world.project,
        &["changeset", "commit", "lint-failing", "--allow-lint-issues"],
    );
    assert_eq!(
        missing_reason["error"]["code"],
        "changeset_lint_override_invalid"
    );
    let committed = world.ok(
        &world.project,
        &[
            "changeset",
            "commit",
            "lint-failing",
            "--allow-lint-issues",
            "--reason",
            "reviewed temporary navigation gap",
        ],
    );
    assert_eq!(committed["status"], "committed");
    let detail: String = Connection::open(database)
        .unwrap()
        .query_row(
            "SELECT detail_json FROM operations
             WHERE action = 'changeset_commit'
             ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert!(detail["lint_issues"].as_u64().unwrap() > 0);
    assert_eq!(
        detail["lint_override_reason"],
        "reviewed temporary navigation gap"
    );
}

#[test]
fn changeset_reports_a_committed_materialization_failure_and_repairs_it() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "projection-failure", "projection");

    let wiki = world.project.join(".lwc/wiki");
    let saved = world.project.join(".lwc/wiki-before-failure");
    fs::rename(&wiki, &saved).unwrap();
    fs::write(&wiki, "blocks directory creation").unwrap();
    let error = world.err(
        &world.project,
        &["changeset", "commit", "projection-failure"],
    );
    assert_eq!(
        error["error"]["code"],
        "changeset_committed_materialization_failed"
    );
    assert_eq!(error["error"]["details"]["committed"], true);
    assert_eq!(
        error["error"]["details"]["recovery_command"],
        "lwc maintenance materialize"
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "projection-first"])["page"]["title"],
        "First"
    );

    fs::remove_file(&wiki).unwrap();
    fs::rename(saved, &wiki).unwrap();
    let queued = world.ok(&world.project, &["maintenance", "materialize"]);
    let work_id = queued["work"]["id"].as_str().unwrap().to_owned();
    let finished = world.ok(&world.project, &["work", "watch", &work_id]);
    assert_eq!(finished["work"]["state"], "succeeded", "{finished}");
    assert!(
        String::from_utf8(fs::read(wiki.join("index.md")).unwrap())
            .unwrap()
            .contains("projection-first")
    );
}

#[test]
fn changeset_reports_a_rolled_back_materialization_failure_and_retry_repairs_it() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "rollback-projection-failure", "rollback-projection");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-projection-failure"],
    );
    let changeset_id = committed["changeset_id"].as_str().unwrap();

    let wiki = world.project.join(".lwc/wiki");
    let saved = world.project.join(".lwc/wiki-before-rollback-failure");
    fs::rename(&wiki, &saved).unwrap();
    fs::write(&wiki, "blocks directory creation").unwrap();
    let error = world.err(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(
        error["error"]["code"],
        "changeset_rolled_back_materialization_failed"
    );
    assert_eq!(error["error"]["details"]["rolled_back"], true);
    assert_eq!(
        error["error"]["details"]["recovery_command"],
        "lwc maintenance materialize"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["page", "show", "rollback-projection-first"]
        )["error"]["code"],
        "page_not_found"
    );

    fs::remove_file(&wiki).unwrap();
    fs::rename(saved, &wiki).unwrap();
    let repaired = world.ok(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(repaired["materialized"], true);
    assert!(
        !String::from_utf8(fs::read(wiki.join("index.md")).unwrap())
            .unwrap()
            .contains("rollback-projection-first")
    );
}

#[cfg(unix)]
#[test]
fn changeset_reports_committed_cleanup_failure_and_retry_only_finishes_cleanup() {
    use std::os::unix::fs::PermissionsExt;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "cleanup-failure", "cleanup");
    let directory = world.project.join(".lwc/changesets");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    let error = world.err(&world.project, &["changeset", "commit", "cleanup-failure"]);
    assert_eq!(error["error"]["code"], "changeset_committed_cleanup_failed");
    assert_eq!(error["error"]["details"]["committed"], true);
    let id = error["error"]["details"]["changeset_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        world.ok(&world.project, &["page", "show", "cleanup-first"])["page"]["title"],
        "First"
    );

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let late_body = world.write("cleanup-late.md", "must not disappear during recovery");
    let late_write = world.err(
        &world.project,
        &[
            "--changeset",
            "cleanup-failure",
            "page",
            "put",
            "cleanup-late",
            "--title",
            "Cleanup Late",
            "--file",
            as_str(&late_body),
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(late_write["error"]["code"], "changeset_frozen");
    let retried = world.ok(&world.project, &["changeset", "commit", "cleanup-failure"]);
    assert_eq!(retried["changeset_id"], id);
    assert!(!directory.join("cleanup-failure.db").exists());
    assert_eq!(
        world.err(&world.project, &["page", "show", "cleanup-late"])["error"]["code"],
        "page_not_found"
    );
    let commit_count: i64 = Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations
             WHERE action = 'changeset_commit' AND target = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(commit_count, 1);
}

#[cfg(unix)]
#[test]
fn changeset_locked_recheck_rejects_a_draft_writer_that_finishes_during_commit() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "draft-race", "draft-race");
    let live = world.project.join(".lwc/wiki.db");
    let before_revision: String = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let draft = world.project.join(".lwc/changesets/draft-race.db");
    let writer = Connection::open(&draft).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["changeset", "commit", "draft-race"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let checkpoint_dir = world.project.join(".lwc/checkpoints");
    let deadline = Instant::now() + Duration::from_secs(5);
    while (!checkpoint_dir.is_dir() || fs::read_dir(&checkpoint_dir).unwrap().next().is_none())
        && child.try_wait().unwrap().is_none()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.try_wait().unwrap().is_none());
    writer
        .execute_batch(
            "INSERT INTO operations(action, target, detail_json)
             VALUES ('test_draft_race', 'draft', '{}');
             UPDATE meta SET value = LOWER(HEX(RANDOMBLOB(32)))
             WHERE key = 'store_revision';
             COMMIT;",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(stderr_json(&output)["error"]["code"], "changeset_changed");
    let after_revision: String = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after_revision, before_revision);
    assert_eq!(
        world.err(&world.project, &["page", "show", "draft-race-first"])["error"]["code"],
        "page_not_found"
    );
    assert!(draft.is_file());
}

#[cfg(unix)]
#[test]
fn changeset_locked_recheck_preserves_a_live_writer_that_finishes_during_commit() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "live-race", "live-race");
    let live = world.project.join(".lwc/wiki.db");
    let writer = Connection::open(&live).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["changeset", "commit", "live-race"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let checkpoint_dir = world.project.join(".lwc/checkpoints");
    let deadline = Instant::now() + Duration::from_secs(5);
    while (!checkpoint_dir.is_dir() || fs::read_dir(&checkpoint_dir).unwrap().next().is_none())
        && child.try_wait().unwrap().is_none()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.try_wait().unwrap().is_none());
    writer
        .execute_batch(
            "INSERT INTO operations(action, target, detail_json)
             VALUES ('test_live_race', 'live', '{}');
             UPDATE meta SET value = LOWER(HEX(RANDOMBLOB(32)))
             WHERE key = 'store_revision';
             COMMIT;",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let race_count: i64 = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action = 'test_live_race'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(race_count, 1);
    assert_eq!(
        world.ok(&world.project, &["page", "show", "live-race-first"])["page"]["title"],
        "First"
    );
}

#[test]
fn changeset_rejects_all_scope_and_unsafe_names_before_creating_files() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let all = world.err(
        &world.project,
        &["--scope", "all", "changeset", "begin", "bad"],
    );
    assert_eq!(all["error"]["code"], "scope_not_supported");

    for name in [
        "",
        " ",
        "../escape",
        ".",
        "..",
        "two/segments",
        "two\\segments",
        " leading",
        "trailing ",
        "line\nbreak",
    ] {
        let invalid = world.err(&world.project, &["changeset", "begin", name]);
        assert_eq!(invalid["error"]["code"], "changeset_name_invalid");
    }
    let oversized = "x".repeat(81);
    let invalid = world.err(&world.project, &["changeset", "begin", oversized.as_str()]);
    assert_eq!(invalid["error"]["code"], "changeset_name_invalid");
    assert!(!world.project.join(".lwc/changesets").exists());
}

#[test]
fn changeset_rejects_unsupported_command_families_without_live_mutation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "guarded"]);
    let database = world.project.join(".lwc/wiki.db");
    let before: (String, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    for args in [
        vec!["--changeset", "guarded", "init"],
        vec!["--changeset", "guarded", "maintenance", "materialize"],
        vec!["--changeset", "guarded", "checkpoint", "list"],
        vec!["--changeset", "guarded", "changeset", "list"],
    ] {
        let error = world.err(&world.project, &args);
        assert_eq!(error["error"]["code"], "changeset_command_unsupported");
    }
    let all = world.err(
        &world.project,
        &["--scope", "all", "--changeset", "guarded", "context"],
    );
    assert_eq!(all["error"]["code"], "scope_not_supported");

    let after: (String, i64) = Connection::open(database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn changeset_read_only_preview_stays_empty_but_recorded_reads_are_staged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "preview"]);

    for args in [
        vec!["--changeset", "preview", "context"],
        vec!["--changeset", "preview", "lint"],
        vec!["--changeset", "preview", "log"],
        vec!["--changeset", "preview", "search", "not-present"],
    ] {
        world.ok(&world.project, &args);
    }
    assert_eq!(
        world.ok(&world.project, &["changeset", "show", "preview"])["empty"],
        true
    );

    world.ok(
        &world.project,
        &[
            "--changeset",
            "preview",
            "search",
            "durable question",
            "--record",
        ],
    );
    let shown = world.ok(&world.project, &["changeset", "show", "preview"]);
    assert_eq!(shown["empty"], false);
    assert_eq!(shown["action_counts"]["search"], 1);
}

#[test]
fn changeset_stages_a_complete_ingest_then_discards_every_candidate_row() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "complete-ingest"]);
    let source = world.write("evidence.md", "atomic candidate evidence");
    let added = world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "source",
            "add",
            as_str(&source),
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "claim",
            &source_id,
        ],
    );
    let analysis = world.write("candidate-analysis.md", "candidate analysis");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "analyze",
            &source_id,
            "--file",
            as_str(&analysis),
        ],
    );
    let source_page = world.write(
        "candidate-source.md",
        "candidate source summary. [[candidate-synthesis]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "page",
            "put",
            "source-candidate",
            "--title",
            "Candidate Source",
            "--kind",
            "source",
            "--summary",
            "Immutable candidate evidence",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );
    let synthesis = world.write(
        "candidate-synthesis.md",
        "Atomic candidate synthesis. [[source-candidate]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "page",
            "put",
            "candidate-synthesis",
            "--title",
            "Candidate Synthesis",
            "--kind",
            "synthesis",
            "--summary",
            "Atomic candidate conclusion",
            "--file",
            as_str(&synthesis),
            "--source",
            &source_id,
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "complete",
            &source_id,
        ],
    );
    let search = world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "search",
            "atomic candidate",
        ],
    );
    assert!(find_result(&search, "candidate-synthesis")["rank"].is_number());
    assert_eq!(
        world.ok(&world.project, &["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        world.ok(&world.project, &["--changeset", "complete-ingest", "lint"])["total"],
        0
    );
    world.ok(&world.project, &["changeset", "discard", "complete-ingest"]);
    assert_eq!(
        world.ok(&world.project, &["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn changeset_routes_schema_purpose_graph_weight_context_search_lint_and_log() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let live_schema = world.ok(&world.project, &["schema", "show"])["schema"]
        .as_str()
        .unwrap()
        .to_string();
    world.ok(&world.project, &["changeset", "begin", "command-matrix"]);
    let schema = world.write("matrix-schema.md", "# Draft Schema\n\nMatrix only.");
    let purpose = world.write("matrix-purpose.md", "# Draft Purpose\n\nMatrix only.");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "schema",
            "set",
            as_str(&schema),
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "purpose",
            "set",
            as_str(&purpose),
        ],
    );
    let first = world.write("matrix-first.md", "matrix token [[matrix-second]]");
    let second = world.write("matrix-second.md", "matrix token [[matrix-first]]");
    for (slug, title, file) in [
        ("matrix-first", "Matrix First", &first),
        ("matrix-second", "Matrix Second", &second),
    ] {
        world.ok(
            &world.project,
            &[
                "--changeset",
                "command-matrix",
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                title,
                "--file",
                as_str(file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "weight",
            "set",
            "page",
            "matrix-first",
            "--value",
            "2",
            "--reason",
            "draft canonical page",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "weight",
            "feedback",
            "page",
            "matrix-first",
            "--query",
            "matrix token",
            "--signal",
            "relevant",
            "--reason",
            "draft result verified",
            "--provenance",
            "agent-observed",
        ],
    );
    let related = world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "graph",
            "related",
            "matrix-first",
        ],
    );
    assert_eq!(related["related"][0]["slug"], "matrix-second");
    let context = world.ok(
        &world.project,
        &["--changeset", "command-matrix", "context"],
    );
    assert!(
        context["stores"][0]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| page["slug"] == "matrix-first")
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "search",
            "matrix token",
            "--record",
        ],
    );
    assert_eq!(
        world.ok(&world.project, &["--changeset", "command-matrix", "lint"])["total"],
        0
    );
    assert!(
        world.ok(
            &world.project,
            &["--changeset", "command-matrix", "log", "--limit", "20"]
        )["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["action"] == "weight_set")
    );
    assert_eq!(
        world.ok(&world.project, &["schema", "show"])["schema"],
        live_schema
    );
    world.ok(&world.project, &["changeset", "discard", "command-matrix"]);
}

#[test]
fn same_changeset_name_is_isolated_between_project_and_global_scopes() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);
    world.ok(&world.project, &["changeset", "begin", "same-name"]);
    world.ok(
        &world.project,
        &["--scope", "global", "changeset", "begin", "same-name"],
    );
    let project_body = world.write("project-scope.md", "project candidate");
    let global_body = world.write("global-scope.md", "global candidate");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "same-name",
            "page",
            "put",
            "scope-page",
            "--title",
            "Project Candidate",
            "--file",
            as_str(&project_body),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "--changeset",
            "same-name",
            "page",
            "put",
            "scope-page",
            "--title",
            "Global Candidate",
            "--file",
            as_str(&global_body),
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(
        world.ok(
            &world.project,
            &["--changeset", "same-name", "page", "show", "scope-page"]
        )["page"]["title"],
        "Project Candidate"
    );
    assert_eq!(
        world.ok(
            &world.project,
            &[
                "--scope",
                "global",
                "--changeset",
                "same-name",
                "page",
                "show",
                "scope-page",
            ]
        )["page"]["title"],
        "Global Candidate"
    );
    world.ok(&world.project, &["changeset", "discard", "same-name"]);
    assert_eq!(
        world.ok(
            &world.project,
            &["--scope", "global", "changeset", "show", "same-name",]
        )["status"],
        "draft"
    );
}

#[cfg(unix)]
#[test]
fn changeset_keeps_live_source_authority_and_rejects_symlinked_draft_storage() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "bounded"]);
    let outside_source = world.project.parent().unwrap().join("outside.md");
    fs::write(&outside_source, "outside evidence").unwrap();
    let error = world.err(
        &world.project,
        &[
            "--changeset",
            "bounded",
            "source",
            "add",
            as_str(&outside_source),
        ],
    );
    assert_eq!(
        error["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let protected = world.project.parent().unwrap().join("protected-wal");
    fs::write(&protected, "must survive").unwrap();
    let wal = world.project.join(".lwc/changesets/bounded.db-wal");
    if wal.exists() {
        fs::remove_file(&wal).unwrap();
    }
    symlink(&protected, &wal).unwrap();
    let invalid = world.err(&world.project, &["changeset", "discard", "bounded"]);
    assert_eq!(invalid["error"]["code"], "changeset_path_invalid");
    assert_eq!(fs::read_to_string(&protected).unwrap(), "must survive");
    fs::remove_file(wal).unwrap();
    world.ok(&world.project, &["changeset", "discard", "bounded"]);

    fs::remove_dir(world.project.join(".lwc/changesets")).unwrap();
    let outside_directory = world.project.parent().unwrap().join("outside-drafts");
    fs::create_dir(&outside_directory).unwrap();
    symlink(&outside_directory, world.project.join(".lwc/changesets")).unwrap();
    let invalid = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(invalid["error"]["code"], "changeset_path_invalid");
    assert!(fs::read_dir(outside_directory).unwrap().next().is_none());
}

fn find_result<'a>(search: &'a Value, identifier: &str) -> &'a Value {
    search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["identifier"] == identifier)
        .unwrap_or_else(|| panic!("missing result {identifier}: {search}"))
}

#[test]
fn help_documents_the_agent_workflow_and_command_side_effects() {
    let world = TestWorld::new();

    for (args, expected) in [
        (
            vec!["--help"],
            vec![
                "Agent operating contract:",
                "Persistent workflow:",
                "~/.lwc/wiki.db",
                "LWC_PROJECT_ROOT",
                "JSON",
                "Do not edit .lwc/wiki.db",
                "Atomic multi-command changes:",
                "lwc changeset begin",
            ],
        ),
        (
            vec!["source", "--help"],
            vec!["When to use:", "Next action:", "pending ingest job"],
        ),
        (
            vec!["ingest", "--help"],
            vec![
                "pending -> analyzing -> generating -> completed",
                "Required Agent loop:",
                "kind=source",
            ],
        ),
        (
            vec!["page", "--help"],
            vec!["When to use:", "Decision rule:", "kind=query"],
        ),
        (
            vec!["source", "add-dir", "--help"],
            vec!["UTF-8", "partial_import", "idempotent"],
        ),
        (
            vec!["source", "add-manifest", "--help"],
            vec!["JSON", "relative", "--acknowledge-sensitive-source"],
        ),
        (
            vec!["source", "status", "--help"],
            vec!["Read-only", "streaming SHA-256", "--allow-external-source"],
        ),
        (
            vec!["source", "diff", "--help"],
            vec![
                "Read-only",
                "--to-source",
                "--max-chars",
                "--acknowledge-sensitive-source",
            ],
        ),
        (
            vec!["page", "put", "--help"],
            vec![
                "kind=source",
                "[[slug]]",
                "--source <SOURCE_IDS>",
                "--provenance <PROVENANCE>",
            ],
        ),
        (
            vec!["ingest", "complete", "--help"],
            vec![
                "kind=source summary",
                "non-source Wiki page",
                "--no-derived-pages-reason",
                "completed",
            ],
        ),
        (
            vec!["search", "--help"],
            vec![
                "--type auto",
                "--type source",
                "--kind",
                "does not persist the query",
                "--scope all",
                "--record",
                "each selected store",
            ],
        ),
        (
            vec!["maintenance", "materialize", "--help"],
            vec!["SQLite is authoritative", ".lwc/wiki"],
        ),
        (
            vec!["maintenance", "compact", "--help"],
            vec!["WAL TRUNCATE", "busy=true", "active reader"],
        ),
        (
            vec!["source", "show", "--help"],
            vec![
                "--offset-chars",
                "--max-chars",
                "window.has_more",
                "window.next_offset_chars",
            ],
        ),
        (
            vec!["checkpoint", "restore", "--help"],
            vec!["Restore", "checkpoint"],
        ),
        (
            vec!["lint", "--help"],
            vec!["--record", "Read-only by default"],
        ),
    ] {
        let output = world.command(&world.project, &args);
        assert!(
            output.status.success(),
            "help command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        for text in expected {
            assert!(
                help.contains(text),
                "help command {args:?} should contain {text:?}\n{help}"
            );
        }
    }
}

#[test]
fn explain_reports_exact_bounded_lexical_and_specificity_arithmetic() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "payment-overview-readme",
        "Payment Overview README",
        "payment reconciliation matching rules evidence",
    );
    put_page(
        &world,
        "payment-reconciliation-matching-rules",
        "Payment Reconciliation Matching Rules",
        "payment reconciliation matching rules evidence",
    );
    put_page(
        &world,
        "payment-catalog",
        "Payment Configuration",
        "payment reconciliation matching rules\n\n## 使用导航\n总览文档只保留模块边界",
    );
    put_page(
        &world,
        "payment-procedure",
        "Payment Procedure",
        "payment reconciliation matching rules\n\n## 使用导航\n按步骤完成具体业务操作",
    );

    let default_search = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation matching rules",
            "--type",
            "page",
        ],
    );
    assert!(default_search["results"][0].get("explanation").is_none());
    assert_eq!(
        default_search["results"][0]["identifier"],
        "payment-reconciliation-matching-rules"
    );

    let explained = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation matching rules",
            "--type",
            "page",
            "--explain",
        ],
    );
    let direct = find_result(&explained, "payment-reconciliation-matching-rules");
    let overview = find_result(&explained, "payment-overview-readme");
    let catalog = find_result(&explained, "payment-catalog");
    let procedure = find_result(&explained, "payment-procedure");
    assert!(
        direct["explanation"]["signals"]["title_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        direct["explanation"]["signals"]["path_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(overview["explanation"]["signals"]["generic_marker"], 1.0);
    assert_eq!(catalog["explanation"]["signals"]["generic_marker"], 1.0);
    assert_eq!(procedure["explanation"]["signals"]["generic_marker"], 0.0);
    for result in explained["results"].as_array().unwrap() {
        let explanation = &result["explanation"];
        let reconstructed = explanation["base_rank"].as_f64().unwrap()
            + explanation["contributions"]
                .as_object()
                .unwrap()
                .values()
                .map(|value| value.as_f64().unwrap())
                .sum::<f64>();
        let final_rank = explanation["final_rank"].as_f64().unwrap();
        assert!((reconstructed - final_rank).abs() < 1e-9, "{explanation}");
        assert!((result["rank"].as_f64().unwrap() - final_rank).abs() < 1e-9);
    }

    let explicit_overview = world.ok(
        &world.project,
        &[
            "search",
            "payment overview README",
            "--type",
            "page",
            "--explain",
        ],
    );
    let overview = find_result(&explicit_overview, "payment-overview-readme");
    assert_eq!(overview["explanation"]["signals"]["generic_marker"], 0.0);
}

#[test]
fn source_generic_detection_preserves_structural_body_signals() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write(
        "payment-reconciliation-guide.md",
        "payment reconciliation rules\n\n文档目录",
    );
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Payment Reconciliation Guide",
        ],
    );
    let identifier = added["source"]["id"].to_string();
    let search = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation rules",
            "--type",
            "source",
            "--explain",
        ],
    );
    let source = find_result(&search, &identifier);
    assert_eq!(source["explanation"]["signals"]["generic_marker"], 1.0);
}

#[test]
fn document_weights_are_bounded_reversible_precedence_aware_and_candidate_only() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "alpha", "Alpha", "calibration candidate sharedterm");
    put_page(&world, "beta", "Beta", "calibration candidate sharedterm");
    put_page(&world, "absent", "Absent", "different vocabulary only");

    let mut ranks = Vec::new();
    for value in [-2, -1, 1, 2] {
        let value_text = value.to_string();
        world.ok(
            &world.project,
            &[
                "weight",
                "set",
                "page",
                "alpha",
                "--value",
                &value_text,
                "--reason",
                "monotonic calibration",
                "--provenance",
                "agent-observed",
            ],
        );
        let search = world.ok(
            &world.project,
            &[
                "search",
                "calibration candidate sharedterm",
                "--type",
                "page",
                "--explain",
            ],
        );
        let alpha = find_result(&search, "alpha");
        assert_eq!(
            alpha["explanation"]["signals"]["manual_adjustment"],
            value as f64 / 2.0
        );
        ranks.push(alpha["rank"].as_f64().unwrap());
    }
    assert!(ranks.windows(2).all(|pair| pair[0] > pair[1]), "{ranks:?}");
    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "alpha",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "beta",
            "--value",
            "2",
            "--reason",
            "verified canonical page",
            "--provenance",
            "agent-observed",
        ],
    );
    let boosted = world.ok(
        &world.project,
        &[
            "search",
            "calibration candidate sharedterm",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(boosted["results"][0]["identifier"], "beta");
    assert_eq!(
        find_result(&boosted, "beta")["explanation"]["signals"]["manual_adjustment"],
        1.0
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "beta",
            "--value",
            "-1",
            "--reason",
            "user says this page is secondary",
            "--provenance",
            "user-provided",
        ],
    );
    let listed = world.ok(&world.project, &["weight", "list", "page", "beta"]);
    assert_eq!(listed["adjustments"].as_array().unwrap().len(), 2);
    assert_eq!(listed["effective"]["provenance"], "user-provided");
    assert_eq!(listed["effective"]["weight"], -1);

    for invalid in ["-3", "0", "3", "1.5"] {
        let error = world.err(
            &world.project,
            &[
                "weight",
                "set",
                "page",
                "alpha",
                "--value",
                invalid,
                "--reason",
                "invalid bound",
                "--provenance",
                "agent-observed",
            ],
        );
        assert_eq!(error["error"]["code"], "invalid_weight");
    }
    let empty_reason = world.err(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "alpha",
            "--value",
            "1",
            "--reason",
            " ",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(empty_reason["error"]["code"], "invalid_input");

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "absent",
            "--value",
            "2",
            "--reason",
            "must not create a lexical candidate",
            "--provenance",
            "agent-observed",
        ],
    );
    let search = world.ok(
        &world.project,
        &["search", "calibration candidate sharedterm"],
    );
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["identifier"] != "absent")
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "user-provided",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "agent-observed",
        ],
    );
    let already_cleared = world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(already_cleared["removed"], false);
    let cleared = world.ok(&world.project, &["weight", "list", "page", "beta"]);
    assert!(cleared["adjustments"].as_array().unwrap().is_empty());
    assert!(cleared["effective"].is_null());
}

#[test]
fn explicit_feedback_is_query_specific_private_precedence_aware_and_reversible() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    put_page(
        &world,
        "preferred",
        "Preferred",
        "private feedback retrievalterm",
    );
    put_page(&world, "other", "Other", "private feedback retrievalterm");
    let distinctive_query = "retrievalterm, private feedback 7f4a9";

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "relevant",
            "--reason",
            "explicit result judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "irrelevant",
            "--reason",
            "replacement judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "relevant",
            "--reason",
            "final agent judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    let empty_query = world.err(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            "?!",
            "--signal",
            "relevant",
            "--reason",
            "invalid query fixture",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(empty_query["error"]["code"], "invalid_query");
    let equivalent = world.ok(
        &world.project,
        &[
            "search",
            "retrievalterm private feedback 7f4a9!",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(
        find_result(&equivalent, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        1.0
    );
    let paraphrase = world.ok(
        &world.project,
        &[
            "search",
            "private retrieval feedback changed words",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(
        find_result(&paraphrase, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        0.0
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "irrelevant",
            "--reason",
            "user overrides agent judgment",
            "--provenance",
            "user-provided",
        ],
    );
    let overridden = world.ok(
        &world.project,
        &["search", distinctive_query, "--type", "page", "--explain"],
    );
    assert_eq!(
        find_result(&overridden, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        -1.0
    );

    let conn = Connection::open(database).unwrap();
    let feedback_dump: String = conn
        .query_row(
            "SELECT GROUP_CONCAT(query_fingerprint || reason, '|') FROM retrieval_feedback",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let operation_dump: String = conn
        .query_row(
            "SELECT GROUP_CONCAT(detail_json, '|') FROM operations WHERE action LIKE 'weight_feedback%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let feedback_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM retrieval_feedback", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(feedback_rows, 2);
    assert!(!feedback_dump.contains(distinctive_query));
    assert!(!operation_dump.contains(distinctive_query));

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "user-provided",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "agent-observed",
        ],
    );
    let already_cleared = world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(already_cleared["removed"], false);
    let cleared = world.ok(
        &world.project,
        &["search", distinctive_query, "--type", "page", "--explain"],
    );
    assert_eq!(
        find_result(&cleared, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        0.0
    );
}

#[test]
fn retrieval_state_is_deleted_with_its_target_and_all_scope_mutations_are_rejected() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    put_page(&world, "temporary", "Temporary", "cleanupterm");
    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "temporary",
            "--value",
            "1",
            "--reason",
            "temporary",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "temporary",
            "--query",
            "cleanupterm",
            "--signal",
            "relevant",
            "--reason",
            "temporary",
            "--provenance",
            "agent-observed",
        ],
    );

    let rejected = world.err(
        &world.project,
        &[
            "--scope",
            "all",
            "weight",
            "set",
            "page",
            "temporary",
            "--value",
            "1",
            "--reason",
            "must reject",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(rejected["error"]["code"], "scope_not_supported");

    world.ok(&world.project, &["page", "remove", "temporary"]);
    let conn = Connection::open(database).unwrap();
    for table in ["retrieval_weights", "retrieval_feedback"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "orphaned rows in {table}");
    }
}

#[test]
fn lint_reports_orphaned_retrieval_state() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    let conn = Connection::open(database).unwrap();
    conn.execute(
        "INSERT INTO retrieval_weights(
            target_type, target_identifier, provenance, weight, reason
         ) VALUES ('page', 'missing-page', 'agent-observed', 1, 'fixture')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO retrieval_feedback(
            query_fingerprint, target_type, target_identifier, provenance, signal, reason
         ) VALUES (?1, 'source', '999', 'agent-observed', 1, 'fixture')",
        ["a".repeat(64)],
    )
    .unwrap();
    drop(conn);

    let lint = world.ok(&world.project, &["lint"]);
    assert_eq!(lint["counts"]["retrieval_weight_orphan"], 1);
    assert_eq!(lint["counts"]["retrieval_feedback_orphan"], 1);
}

#[test]
fn graph_reranking_is_bounded_query_conditioned_and_does_not_promote_sources() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "seed",
        "Graph calibration seed",
        "graph calibration seedterm [[linked-candidate]]",
    );
    put_page(
        &world,
        "linked-candidate",
        "Linked candidate",
        "graph calibration seedterm",
    );
    put_page(
        &world,
        "unrelated-hub-readme",
        "Unrelated Hub README",
        "graph calibration seedterm [[x]] [[y]] [[z]] [[q]]",
    );
    let raw = world.write("raw.md", "graph calibration seedterm");
    world.ok(
        &world.project,
        &["source", "add", as_str(&raw), "--title", "Raw graph source"],
    );

    let search = world.ok(
        &world.project,
        &[
            "search",
            "graph calibration seedterm",
            "--type",
            "all",
            "--explain",
        ],
    );
    let linked = find_result(&search, "linked-candidate");
    let hub = find_result(&search, "unrelated-hub-readme");
    assert!(
        linked["explanation"]["signals"]["graph_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        linked["explanation"]["signals"]["graph_match"]
            .as_f64()
            .unwrap()
            <= 1.0
    );
    assert_eq!(hub["explanation"]["signals"]["graph_match"], 0.0);
    let hub_penalty = hub["explanation"]["signals"]["graph_hub_penalty"]
        .as_f64()
        .unwrap();
    assert!(hub_penalty > 0.0 && hub_penalty <= 1.0);
    assert!(
        hub["explanation"]["contributions"]["graph"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        linked["explanation"]["contributions"]["graph"]
            .as_f64()
            .unwrap()
            < 0.0
    );
    let raw_result = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["type"] == "source")
        .unwrap();
    assert_eq!(raw_result["explanation"]["signals"]["graph_match"], 0.0);

    put_page(
        &world,
        "common-seed",
        "Common seed",
        "commononlyzeta [[shared-neighbor]]",
    );
    put_page(
        &world,
        "common-sibling",
        "Common sibling",
        "commononlyzeta [[shared-neighbor]]",
    );
    put_page(
        &world,
        "shared-neighbor",
        "Shared neighbor",
        "structural connector only",
    );
    let common = world.ok(
        &world.project,
        &["search", "commononlyzeta", "--type", "page", "--explain"],
    );
    for slug in ["common-seed", "common-sibling"] {
        assert_eq!(
            find_result(&common, slug)["explanation"]["signals"]["graph_match"],
            0.0,
            "common-neighbor-only evidence must not affect search ranking"
        );
    }

    let shared_source = world.write("shared-graph-source.md", "shared graph evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&shared_source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    for slug in ["shared-source-a", "shared-source-b"] {
        let body = world.write(&format!("pages/{slug}.md"), "sharedsourcezeta");
        world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                slug,
                "--file",
                as_str(&body),
                "--source",
                &source_id,
            ],
        );
    }
    let shared = world.ok(
        &world.project,
        &["search", "sharedsourcezeta", "--type", "page", "--explain"],
    );
    for slug in ["shared-source-a", "shared-source-b"] {
        assert!(
            find_result(&shared, slug)["explanation"]["signals"]["graph_match"]
                .as_f64()
                .unwrap()
                > 0.0
        );
    }
}

#[test]
fn graph_config_resolves_layers_and_updates_project_atomically() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let defaults = world.ok(&world.project, &["config", "show"]);
    assert_eq!(defaults["graph"]["physical"], "disabled");
    assert_eq!(defaults["graph"]["engine"], "auto");
    assert_eq!(defaults["graph"]["physical_origin"], "built-in");
    assert_eq!(defaults["graph"]["engine_origin"], "built-in");
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    assert_eq!(defaults["graph"]["resolved_engine"], "graphqlite");

    let set = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--physical",
            "disabled",
            "--engine",
            "rslg",
        ],
    );
    assert_eq!(set["graph"]["physical"], "disabled");
    assert_eq!(set["graph"]["resolved_engine"], "rslg");
    assert_eq!(set["graph"]["physical_origin"], "project");
    assert_eq!(set["graph"]["engine_origin"], "project");

    let inherited = world.ok(
        &world.project,
        &["config", "unset", "--physical", "--engine"],
    );
    assert_eq!(inherited["graph"]["physical"], "disabled");
    assert_eq!(inherited["graph"]["engine"], "auto");

    world.ok(&world.project, &["--scope", "global", "init"]);
    world.ok(
        &world.project,
        &["--scope", "global", "config", "set", "--engine", "rslg"],
    );
    let layered = world.ok(&world.project, &["config", "show"]);
    assert_eq!(layered["graph"]["engine"], "rslg");
    assert_eq!(layered["graph"]["engine_origin"], "global");
    assert_eq!(layered["graph"]["resolved_engine"], "rslg");
}

#[test]
fn physical_projection_is_disabled_by_default() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let config = world.ok(&world.project, &["config", "show"]);

    assert_eq!(config["graph"]["physical"], "disabled");
    assert_eq!(config["graph"]["physical_origin"], "built-in");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn explicit_graphqlite_projection_uses_one_stable_sidecar() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--physical",
            "enabled",
            "--engine",
            "graphqlite",
        ],
    );
    for index in 0..3 {
        put_page(
            &world,
            &format!("stable-sidecar-{index}"),
            &format!("Stable sidecar {index}"),
            &format!("generation {index} alpha beta gamma evidence"),
        );
    }

    let sidecars = fs::read_dir(world.project.join(".lwc"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("graph-graphqlite") && name.ends_with(".db"))
        .collect::<Vec<_>>();

    assert_eq!(
        sidecars.len(),
        1,
        "sidecars must not grow by generation: {sidecars:?}"
    );
}

#[test]
fn point_page_put_does_not_build_a_complete_artifact_snapshot() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let body = world.write(
        "targeted-artifact.md",
        "targeted artifact alpha beta evidence",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .env("LWC_TEST_FORBID_FULL_ARTIFACT_SNAPSHOT", "1")
        .args([
            "page",
            "put",
            "targeted-artifact",
            "--title",
            "Targeted artifact",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "point mutation invoked complete artifact snapshot: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn graph_config_rejects_symlink_all_scope_and_changeset_mutation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let outside = world.home.join("outside-config.json");
    fs::write(&outside, "sentinel").unwrap();
    std::os::unix::fs::symlink(&outside, world.project.join(".lwc/config.json")).unwrap();
    let error = world.err(&world.project, &["config", "set", "--engine", "rslg"]);
    assert_eq!(error["error"]["code"], "unsafe_config_path");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "sentinel");

    let all = world.err(&world.project, &["--scope", "all", "config", "show"]);
    assert_eq!(all["error"]["code"], "scope_not_supported");

    let second = TestWorld::new();
    second.ok(&second.project, &["init"]);
    second.ok(&second.project, &["changeset", "begin", "config-guard"]);
    let staged = second.err(
        &second.project,
        &[
            "--changeset",
            "config-guard",
            "config",
            "set",
            "--engine",
            "rslg",
        ],
    );
    assert_eq!(staged["error"]["code"], "changeset_command_not_supported");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn graphqlite_projection_failure_commits_canonical_fails_closed_and_recovers() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--physical",
            "enabled",
            "--engine",
            "graphqlite",
        ],
    );
    let body = world.write("projection.md", "Committed projection evidence.");
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .env("LWC_TEST_GRAPHQLITE_FAIL_AT", "before")
        .args([
            "page",
            "put",
            "projection-page",
            "--title",
            "Projection page",
            "--file",
            as_str(&body),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = stderr_json(&output);
    assert_eq!(error["error"]["code"], "graph_projection_failed");
    assert_eq!(error["error"]["details"]["canonical_committed"], true);

    let status = world.ok(&world.project, &["graph", "status"]);
    assert_eq!(status["projection"]["status"], "stale");
    let stale = world.err(
        &world.project,
        &["graph", "explore", "page:projection-page"],
    );
    assert_eq!(stale["error"]["code"], "graph_projection_stale");

    world.ok(&world.project, &["config", "set", "--engine", "graphqlite"]);
    let status = world.ok(&world.project, &["graph", "status"]);
    assert_eq!(status["projection"]["status"], "fresh");
    let page = world.ok(&world.project, &["page", "show", "projection-page"]);
    assert_eq!(page["page"]["title"], "Projection page");
}

#[test]
fn canonical_graph_supports_exploration_paths_relations_impact_and_overview() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "beta", "Beta", "SharedTerm target knowledge.");
    put_page(
        &world,
        "alpha",
        "Alpha",
        "SharedTerm source knowledge. [[beta]]",
    );

    let explored = world.ok(
        &world.project,
        &[
            "graph",
            "explore",
            "term:sharedterm",
            "--depth",
            "2",
            "--limit",
            "100",
        ],
    );
    assert_eq!(explored["start"]["identifier"], "term:sharedterm");
    assert!(explored["nodes"].as_array().unwrap().len() >= 3);
    let occurrence = explored["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["type"] == "OCCURS_IN")
        .unwrap();
    assert!(occurrence["frequency"].as_u64().unwrap() > 0);
    assert_eq!(
        occurrence["positions"].as_array().unwrap().len() as u64,
        occurrence["frequency"].as_u64().unwrap()
    );
    assert_eq!(
        occurrence["positions"][0]["byte_start"],
        occurrence["first_position"]
    );
    world.ok(&world.project, &["config", "set", "--engine", "rslg"]);
    let rslg_explored = world.ok(
        &world.project,
        &[
            "graph",
            "explore",
            "term:sharedterm",
            "--depth",
            "2",
            "--limit",
            "100",
        ],
    );
    assert_eq!(rslg_explored["nodes"], explored["nodes"]);
    assert_eq!(rslg_explored["edges"], explored["edges"]);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    world.ok(&world.project, &["config", "set", "--engine", "graphqlite"]);
    let node = world.ok(&world.project, &["graph", "node", "page:alpha"]);
    assert_eq!(node["node"]["label"], "Alpha");
    let neighbors = world.ok(
        &world.project,
        &[
            "graph",
            "neighbors",
            "page:alpha",
            "--direction",
            "outgoing",
        ],
    );
    assert!(
        neighbors["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["type"] == "LINKS_TO")
    );
    let keyword_free = world.ok(&world.project, &["graph", "explore"]);
    assert_eq!(keyword_free["keyword_free"], true);
    assert!(
        !keyword_free["representatives"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let path = world.ok(
        &world.project,
        &["graph", "path", "page:alpha", "page:beta"],
    );
    assert_eq!(path["found"], true);
    assert_eq!(path["edges"][0]["type"], "LINKS_TO");

    let relation = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:alpha",
            "SUPPORTS",
            "page:beta",
            "--provenance",
            "agent-observed",
            "--reason",
            "Alpha explicitly supports beta",
            "--confidence",
            "0.9",
        ],
    );
    assert_eq!(relation["relation"]["type"], "SUPPORTS");
    assert_eq!(relation["relation"]["provenance"], "agent-observed");

    let impact = world.ok(&world.project, &["graph", "impact", "page:beta"]);
    assert!(
        impact["review"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["identifier"] == "page:alpha")
    );

    let status = world.ok(&world.project, &["graph", "status"]);
    assert_eq!(status["projection"]["status"], "disabled");
    assert_eq!(
        status["projection"]["canonical_generation"],
        status["projection"]["projected_generation"]
    );
    let verified = world.ok(&world.project, &["graph", "verify"]);
    assert_eq!(verified["valid"], true, "{verified}");

    let overview = world.ok(&world.project, &["graph", "overview"]);
    assert!(overview["node_counts"]["term"].as_u64().unwrap() > 0);
    assert!(overview["edge_counts"]["SUPPORTS"].as_u64().unwrap() > 0);
    assert_eq!(overview["projection"]["status"], "disabled");

    let listed = world.ok(
        &world.project,
        &["graph", "relation", "list", "--from", "page:alpha"],
    );
    assert_eq!(listed["relations"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed["relations"][0]["reason"],
        "Alpha explicitly supports beta"
    );

    let retracted = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "retract",
            "page:alpha",
            "SUPPORTS",
            "page:beta",
            "--reason",
            "Evidence was superseded",
        ],
    );
    assert_eq!(retracted["retracted"], true);
    let listed = world.ok(&world.project, &["graph", "relation", "list"]);
    assert!(listed["relations"].as_array().unwrap().is_empty());
}

#[test]
fn semantic_relation_lifecycle_validates_all_types_provenance_sources_and_retraction() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["config", "set", "--engine", "rslg"]);
    let source = world.write("relation-source.md", "Semantic relation evidence.");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    put_page(&world, "relation-a", "Relation A", "Relation source node.");
    put_page(&world, "relation-b", "Relation B", "Relation target node.");

    for relation_type in [
        "SUPPORTS",
        "CONTRADICTS",
        "REFINES",
        "SUPERSEDES",
        "CAUSES",
        "DEPENDS_ON",
    ] {
        world.ok(
            &world.project,
            &[
                "graph",
                "relation",
                "set",
                "page:relation-a",
                relation_type,
                "page:relation-b",
                "--provenance",
                "agent-observed",
                "--reason",
                "validated semantic relation",
                "--confidence",
                "0.8",
            ],
        );
    }
    let listed = world.ok(&world.project, &["graph", "relation", "list"]);
    assert_eq!(listed["relations"].as_array().unwrap().len(), 6);

    for arguments in [
        vec!["LINKS_TO", "agent-observed", "reason", "0.8"],
        vec!["SUPPORTS", "automatic", "reason", "0.8"],
        vec!["SUPPORTS", "agent-observed", "reason", "1.2"],
        vec!["SUPPORTS", "agent-observed", "", "0.8"],
    ] {
        let error = world.err(
            &world.project,
            &[
                "graph",
                "relation",
                "set",
                "page:relation-a",
                arguments[0],
                "page:relation-b",
                "--provenance",
                arguments[1],
                "--reason",
                arguments[2],
                "--confidence",
                arguments[3],
            ],
        );
        assert!(
            error["error"]["code"]
                .as_str()
                .unwrap()
                .starts_with("invalid_")
        );
    }
    let missing_source = world.err(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:relation-a",
            "SUPPORTS",
            "page:relation-b",
            "--provenance",
            "source-grounded",
            "--reason",
            "requires evidence",
            "--confidence",
            "0.9",
        ],
    );
    assert_eq!(missing_source["error"]["code"], "invalid_semantic_relation");
    let grounded = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:relation-a",
            "SUPPORTS",
            "page:relation-b",
            "--provenance",
            "source-grounded",
            "--reason",
            "grounded evidence",
            "--confidence",
            "0.9",
            "--source",
            &source_id,
        ],
    );
    assert_eq!(
        grounded["relation"]["source_ids"][0],
        source_id.parse::<i64>().unwrap()
    );

    let retracted = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "retract",
            "page:relation-a",
            "DEPENDS_ON",
            "page:relation-b",
            "--reason",
            "dependency removed",
        ],
    );
    assert_eq!(retracted["retracted"], true);
}

#[test]
fn document_removal_synchronously_removes_hierarchy_and_preserves_stale_span_diagnostics() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "temporary-graph-page",
        "Temporary graph page",
        "RemovalNeedle remains locatable until deletion.",
    );
    let found = world.ok(
        &world.project,
        &["search", "RemovalNeedle", "--granularity", "sentence"],
    );
    let span_id = found["results"][0]["identifier"].as_str().unwrap();

    world.ok(&world.project, &["page", "remove", "temporary-graph-page"]);
    let after = world.ok(
        &world.project,
        &[
            "search",
            "RemovalNeedle",
            "--granularity",
            "all",
            "--group-by",
            "none",
        ],
    );
    assert!(after["results"].as_array().unwrap().is_empty());
    let stale = world.err(&world.project, &["span", "get", span_id]);
    assert_eq!(stale["error"]["code"], "stale_span");
    let missing = world.err(
        &world.project,
        &["graph", "explore", "page:temporary-graph-page"],
    );
    assert_eq!(missing["error"]["code"], "graph_node_not_found");
}

#[test]
fn every_public_command_exposes_renderable_help() {
    let world = TestWorld::new();
    let commands = [
        "init",
        "changeset",
        "changeset begin",
        "changeset list",
        "changeset show",
        "changeset commit",
        "changeset discard",
        "changeset rollback",
        "schema",
        "schema set",
        "schema show",
        "purpose",
        "purpose set",
        "purpose show",
        "source",
        "source add",
        "source add-dir",
        "source add-manifest",
        "source list",
        "source status",
        "source diff",
        "source show",
        "source refs",
        "source remove",
        "page",
        "page put",
        "page list",
        "page show",
        "page links",
        "page remove",
        "ingest",
        "ingest list",
        "ingest next",
        "ingest claim",
        "ingest analyze",
        "ingest complete",
        "ingest fail",
        "ingest retry",
        "config",
        "config show",
        "config set",
        "config unset",
        "graph",
        "graph related",
        "graph explore",
        "graph node",
        "graph neighbors",
        "graph path",
        "graph impact",
        "graph overview",
        "graph status",
        "graph verify",
        "graph relation",
        "graph relation set",
        "graph relation list",
        "graph relation retract",
        "span",
        "span get",
        "span expand",
        "maintenance",
        "maintenance materialize",
        "maintenance reindex",
        "maintenance compact",
        "work",
        "work list",
        "work status",
        "work watch",
        "work cancel",
        "work resume",
        "checkpoint",
        "checkpoint create",
        "checkpoint list",
        "checkpoint restore",
        "search",
        "context",
        "lint",
        "log",
    ];

    for command in commands {
        let mut args = command.split_whitespace().collect::<Vec<_>>();
        args.push("--help");
        let output = world.command(&world.project, &args);
        assert!(
            output.status.success(),
            "help command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            help.contains("Usage:") && help.len() >= 120,
            "help command {args:?} is unexpectedly incomplete\n{help}"
        );
    }
}

#[test]
fn project_flow_preserves_sources_and_rolls_back_failed_page_updates() {
    let world = TestWorld::new();

    let initialized = world.ok(&world.project, &["init"]);
    assert_eq!(initialized["scope"], "project");
    assert!(world.project.join(".lwc/wiki.db").is_file());

    let schema = world.write(
        "schema.md",
        "# Research Wiki\nEvery factual page cites its raw sources.",
    );
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    let shown_schema = world.ok(&world.project, &["schema", "show"]);
    assert_eq!(
        shown_schema["schema"],
        "# Research Wiki\nEvery factual page cites its raw sources."
    );

    let source = world.write(
        "raw/ownership.md",
        "Rust ownership assigns one owner to each value.",
    );
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Ownership source",
        ],
    );
    assert_eq!(added["created"], true);
    assert_eq!(added["source"]["id"], 1);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let duplicate = world.write(
        "elsewhere/same.md",
        "Rust ownership assigns one owner to each value.",
    );
    let reused = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&duplicate),
            "--title",
            "Ignored title",
        ],
    );
    assert_eq!(reused["created"], false);
    assert_eq!(reused["source"]["id"], 1);
    assert_eq!(reused["source"]["title"], "Ownership source");
    assert_eq!(reused["source"]["origin"], as_str(&source));

    let page = world.write(
        "ownership-page.md",
        "Ownership connects to [[borrowing]] and repeats [[borrowing]].",
    );
    let put = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "ownership",
            "--title",
            "Ownership",
            "--kind",
            "concept",
            "--summary",
            "How Rust assigns values",
            "--file",
            as_str(&page),
            "--source",
            &source_id,
            "--source",
            &source_id,
        ],
    );
    assert_eq!(put["created"], true);
    assert_eq!(put["page"]["source_ids"], serde_json::json!([1]));
    assert_eq!(put["page"]["links"], serde_json::json!(["borrowing"]));

    let search = world.ok(&world.project, &["search", "ownership"]);
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["type"] == "page" && result["identifier"] == "ownership")
    );

    let context = world.ok(&world.project, &["context", "--limit", "10"]);
    assert_eq!(context["stores"][0]["scope"], "project");
    assert_eq!(context["stores"][0]["pages"][0]["slug"], "ownership");
    assert!(context["stores"][0]["recent_operations"].is_array());

    let lint = world.ok(&world.project, &["lint"]);
    assert!(
        lint["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "dangling_link" && issue["target"] == "borrowing")
    );

    let before_log = world.ok(&world.project, &["log", "--limit", "100"]);
    let before_page_puts = before_log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["action"] == "page_put")
        .count();

    let replacement = world.write("replacement.md", "Broken replacement [[other]].");
    let failed = world.err(
        &world.project,
        &[
            "page",
            "put",
            "ownership",
            "--title",
            "Replacement",
            "--file",
            as_str(&replacement),
            "--source",
            "999",
        ],
    );
    assert_eq!(failed["error"]["code"], "source_not_found");

    let unchanged = world.ok(&world.project, &["page", "show", "ownership"]);
    assert_eq!(unchanged["page"]["title"], "Ownership");
    assert_eq!(unchanged["page"]["links"], serde_json::json!(["borrowing"]));

    let after_log = world.ok(&world.project, &["log", "--limit", "100"]);
    let after_page_puts = after_log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["action"] == "page_put")
        .count();
    assert_eq!(after_page_puts, before_page_puts);
}

#[test]
fn page_provenance_is_structured_on_every_read_surface_and_replaced() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("evidence.md", "immutable evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let page = world.write("mixed.md", "provenanceterm mixed knowledge");
    let expected = serde_json::json!([
        "source-grounded",
        "user-provided",
        "agent-observed",
        "hypothesis"
    ]);

    let put = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "mixed-provenance",
            "--title",
            "Mixed provenance",
            "--summary",
            "Structured provenance test",
            "--file",
            as_str(&page),
            "--source",
            &source_id,
            "--provenance",
            "hypothesis",
            "--provenance",
            "agent-observed",
            "--provenance",
            "user-provided",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(put["page"]["provenance"], expected);

    let shown = world.ok(&world.project, &["page", "show", "mixed-provenance"]);
    assert_eq!(shown["page"]["provenance"], expected);

    let listed = world.ok(&world.project, &["page", "list"]);
    assert_eq!(listed["pages"][0]["provenance"], expected);

    let context = world.ok(&world.project, &["context", "--limit", "10"]);
    assert_eq!(context["stores"][0]["pages"][0]["provenance"], expected);

    let search = world.ok(&world.project, &["search", "provenanceterm"]);
    let page_hit = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["type"] == "page")
        .unwrap();
    assert_eq!(page_hit["provenance"], expected);

    let refs = world.ok(&world.project, &["source", "refs", &source_id]);
    assert_eq!(refs["pages"][0]["provenance"], expected);

    let replacement = world.write("replacement.md", "replacement body");
    let replaced = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "mixed-provenance",
            "--title",
            "Mixed provenance",
            "--summary",
            "Structured provenance test",
            "--file",
            as_str(&replacement),
            "--source",
            &source_id,
        ],
    );
    assert_eq!(
        replaced["page"]["provenance"],
        serde_json::json!(["source-grounded"])
    );
}

#[test]
fn lint_accepts_explicit_provenance_but_reports_unclassified_pages() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    for (slug, provenance) in [
        ("classified", Some("agent-observed")),
        ("unclassified", None),
    ] {
        let page = world.write(&format!("{slug}.md"), &format!("{slug} body"));
        let mut args = vec![
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--summary",
            "summary",
            "--file",
            as_str(&page),
        ];
        if let Some(value) = provenance {
            args.extend(["--provenance", value]);
        }
        world.ok(&world.project, &args);
    }

    let lint = world.ok(&world.project, &["lint"]);
    let uncited = lint["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["code"] == "uncited_page")
        .map(|issue| issue["page"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!uncited.contains(&"classified"));
    assert!(uncited.contains(&"unclassified"));
}

#[test]
fn nearest_project_is_discovered_from_nested_directories() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let nested = world.project.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    let listed = world.ok(&nested, &["page", "list"]);
    assert_eq!(listed["scope"], "project");
    assert_eq!(
        fs::canonicalize(Path::new(listed["database"].as_str().unwrap())).unwrap(),
        fs::canonicalize(world.project.join(".lwc/wiki.db")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn readonly_store_allows_unrecorded_search_but_rejects_recording() {
    use std::os::unix::fs::PermissionsExt;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let page = world.write("readonly.md", "readonly-query-term");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "readonly",
            "--title",
            "Readonly",
            "--file",
            as_str(&page),
        ],
    );

    let db_path = world.project.join(".lwc/wiki.db");
    let lwc_dir = world.project.join(".lwc");
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&lwc_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let search = world.ok(
        &world.project,
        &["search", "readonly-query-term", "--explain"],
    );
    assert_eq!(search["results"][0]["identifier"], "readonly");
    assert!(search["results"][0]["explanation"].is_object());

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(context["stores"][0]["pages"][0]["slug"], "readonly");

    let recorded = world.err(
        &world.project,
        &["search", "readonly-query-term", "--record"],
    );
    assert_eq!(recorded["error"]["code"], "database_error");
    assert!(
        recorded["error"]["message"]
            .as_str()
            .unwrap()
            .contains("readonly database")
    );
}

#[test]
fn read_commands_transparently_migrate_a_writable_v5_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE search_fts;
         DROP TABLE IF EXISTS search_fts_data;
         DROP TABLE IF EXISTS search_fts_idx;
         DROP TABLE IF EXISTS search_fts_content;
         DROP TABLE IF EXISTS search_fts_docsize;
         DROP TABLE IF EXISTS search_fts_config;
         DROP TABLE source_path_revisions;
         DROP TABLE page_provenance;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         CREATE VIRTUAL TABLE search_fts USING fts5(
             doc_type UNINDEXED,
             identifier UNINDEXED,
             title_terms,
             summary_terms,
             body_terms
         );
         ALTER TABLE ingest_jobs DROP COLUMN no_derived_pages_reason;
         UPDATE meta SET value = '5' WHERE key = 'format_version';
         PRAGMA user_version = 5;",
    )
    .unwrap();
    drop(conn);

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(context["stores"][0]["scope"], "project");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 11,
        "context should migrate a writable legacy store"
    );
}

#[test]
fn v10_commands_return_schema_migration_work_without_blocking_inline() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "migration-work",
        "Migration work",
        "migration work evidence",
    );
    let database = world.project.join(".lwc/wiki.db");
    downgrade_to_v10(&database);

    let started = Instant::now();
    let output = world.command(&world.project, &["context", "--limit", "5"]);
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "old-schema preflight failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = stdout_json(&output);
    assert_eq!(response["work"]["kind"], "schema-migrate");
    assert!(
        matches!(
            response["work"]["state"].as_str(),
            Some("queued" | "running")
        ),
        "unexpected Work response: {response}"
    );
    let work_id = response["work"]["id"].as_str().unwrap();
    assert_eq!(work_id.len(), 64);
    assert!(work_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(
        elapsed.as_millis() <= 500,
        "foreground migration preflight took {} ms",
        elapsed.as_millis()
    );

    let watched = world.ok(&world.project, &["work", "watch", work_id]);
    assert_eq!(watched["work"]["state"], "succeeded", "{watched}");
    assert_eq!(watched["work"]["percent"], 100.0);
    let status = world.ok(&world.project, &["work", "status", work_id]);
    assert_eq!(status["work"]["id"], work_id);
    let works = world.ok(&world.project, &["work", "list"]);
    assert!(
        works["works"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["id"] == work_id)
    );
    let resume_error = world.err(&world.project, &["work", "resume", work_id]);
    assert_eq!(resume_error["error"]["code"], "work_not_resumable");
    world.ok(&world.project, &["context", "--limit", "5"]);
    let verified = world.ok(&world.project, &["graph", "verify"]);
    assert_eq!(verified["valid"], true, "{verified}");
    let migrated = Connection::open(database).unwrap();
    assert_eq!(
        migrated
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        11
    );
}

#[test]
fn read_commands_migrate_v6_store_to_structured_provenance() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("migration-source.md", "migration evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let body = world.write("migration-page.md", "migration page body");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "migration-page",
            "--title",
            "Migration page",
            "--summary",
            "Migration summary",
            "--file",
            as_str(&body),
            "--source",
            &source_id,
        ],
    );

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         DROP TABLE source_path_revisions;
         DROP TABLE IF EXISTS page_provenance;
         UPDATE meta SET value = '6' WHERE key = 'format_version';
         PRAGMA user_version = 6;",
    )
    .unwrap();
    drop(conn);

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(
        context["stores"][0]["pages"][0]["provenance"],
        serde_json::json!(["source-grounded"])
    );

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    conn.prepare("SELECT page_slug, provenance FROM page_provenance LIMIT 0")
        .unwrap();
}

#[test]
fn read_commands_migrate_v7_without_guessing_source_path_history() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("legacy-source.md", "legacy evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         DROP TABLE source_path_revisions;
         UPDATE meta SET value = '7' WHERE key = 'format_version';
         PRAGMA user_version = 7;",
    )
    .unwrap();
    drop(conn);

    let listed = world.ok(&world.project, &["source", "list"]);
    assert_eq!(listed["sources"][0]["id"], added["source"]["id"]);

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let tracked: i64 = conn
        .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 11);
    assert_eq!(tracked, 0, "migration must not guess a legacy path head");

    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let status = world.ok(&world.project, &["source", "status", &source_id]);
    assert!(status["checks"].as_array().unwrap().is_empty());
    assert_eq!(status["untracked_source_ids"], serde_json::json!([1]));
}

#[test]
fn read_commands_atomically_migrate_a_v8_store_to_weighted_retrieval() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "v8-page",
        "V8 page",
        "v8 migration retrieval term\n\n文档目录",
    );

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         UPDATE meta SET value = '8' WHERE key = 'format_version';
         PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);

    let search = world.ok(
        &world.project,
        &["search", "v8 migration retrieval term", "--explain"],
    );
    assert_eq!(search["results"][0]["identifier"], "v8-page");
    assert_eq!(
        search["results"][0]["explanation"]["signals"]["generic_marker"],
        1.0
    );

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    conn.prepare("SELECT path_terms FROM search_fts LIMIT 0")
        .unwrap();
    conn.prepare("SELECT weight FROM retrieval_weights LIMIT 0")
        .unwrap();
    conn.prepare("SELECT signal FROM retrieval_feedback LIMIT 0")
        .unwrap();
    let structural_navigation: i64 = conn
        .query_row(
            "SELECT structural_navigation FROM pages WHERE slug = 'v8-page'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(structural_navigation, 1);
}

#[test]
fn failed_v8_migration_leaves_version_and_schema_unchanged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         CREATE TABLE retrieval_weights(sentinel TEXT NOT NULL);
         UPDATE meta SET value = '8' WHERE key = 'format_version';
         PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);

    let error = world.err(&world.project, &["context", "--limit", "1"]);
    assert_eq!(error["error"]["code"], "store_migration_failed");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format_version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let feedback_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'retrieval_feedback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 8);
    assert_eq!(format_version, "8");
    assert_eq!(feedback_exists, 0);
    for table in ["sources", "pages"] {
        let columns: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = 'structural_navigation'"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 0, "failed migration added a column to {table}");
    }
    conn.prepare("SELECT sentinel FROM retrieval_weights LIMIT 0")
        .unwrap();
}

#[test]
fn read_commands_atomically_migrate_a_v9_store_to_changeset_metadata() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "v9-page", "V9 page", "v9 migration content");

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE changesets;
         DELETE FROM meta WHERE key IN ('store_id', 'store_revision');
         UPDATE meta SET value = '9' WHERE key = 'format_version';
         PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);

    let shown = world.ok(&world.project, &["page", "show", "v9-page"]);
    assert_eq!(shown["page"]["body"], "v9 migration content");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    for key in ["store_id", "store_revision"] {
        let value: String = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value.len(), 64);
    }
    conn.prepare("SELECT id, status, post_revision FROM changesets LIMIT 0")
        .unwrap();
}

#[test]
fn failed_v9_changeset_migration_leaves_version_and_schema_unchanged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP TABLE changesets;
         CREATE TABLE changesets(sentinel TEXT NOT NULL);
         DELETE FROM meta WHERE key IN ('store_id', 'store_revision');
         UPDATE meta SET value = '9' WHERE key = 'format_version';
         PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);

    let error = world.err(&world.project, &["context", "--limit", "1"]);
    assert_eq!(error["error"]["code"], "store_migration_failed");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format_version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let identity_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key IN ('store_id', 'store_revision')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 9);
    assert_eq!(format_version, "9");
    assert_eq!(identity_count, 0);
    conn.prepare("SELECT sentinel FROM changesets LIMIT 0")
        .unwrap();
}

#[test]
fn project_and_global_stores_are_isolated_and_combined_deterministically() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);

    let project_page = world.write("project.md", "sharedterm project knowledge");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "project-page",
            "--title",
            "Project page",
            "--summary",
            "Project result",
            "--file",
            as_str(&project_page),
        ],
    );

    let global_page = world.write("global.md", "sharedterm global knowledge");
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "page",
            "put",
            "global-page",
            "--title",
            "Global page",
            "--summary",
            "Global result",
            "--file",
            as_str(&global_page),
        ],
    );

    let project_only = world.ok(&world.project, &["search", "sharedterm"]);
    assert!(
        project_only["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["scope"] == "project")
    );

    let combined = world.ok(&world.project, &["--scope", "all", "search", "sharedterm"]);
    let scopes: Vec<_> = combined["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["scope"].as_str().unwrap())
        .collect();
    assert_eq!(scopes, vec!["project", "global"]);

    let second_project_page = world.write("project-two.md", "sharedterm second project page");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "project-page-two",
            "--title",
            "Second project page",
            "--file",
            as_str(&second_project_page),
        ],
    );
    let limited = world.ok(
        &world.project,
        &["--scope", "all", "search", "sharedterm", "--limit", "1"],
    );
    assert_eq!(limited["results"].as_array().unwrap().len(), 1);
    assert_eq!(limited["results"][0]["scope"], "project");

    let context = world.ok(&world.project, &["--scope", "all", "context"]);
    assert_eq!(context["stores"][0]["scope"], "project");
    assert_eq!(context["stores"][1]["scope"], "global");

    let unsupported = world.err(&world.project, &["--scope", "all", "schema", "show"]);
    assert_eq!(unsupported["error"]["code"], "scope_not_supported");
}

#[test]
fn retrieval_state_for_the_same_identifier_never_crosses_scopes() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);
    let body = world.write("same-scope.md", "scope-isolation-query");
    for scope in [None, Some("global")] {
        let mut args = Vec::new();
        if let Some(scope) = scope {
            args.extend(["--scope", scope]);
        }
        args.extend([
            "page",
            "put",
            "same-page",
            "--title",
            "Same page",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ]);
        world.ok(&world.project, &args);
    }

    for (scope, value, signal) in [
        (None, "2", "relevant"),
        (Some("global"), "-2", "irrelevant"),
    ] {
        let mut prefix = Vec::new();
        if let Some(scope) = scope {
            prefix.extend(["--scope", scope]);
        }
        let mut weight = prefix.clone();
        weight.extend([
            "weight",
            "set",
            "page",
            "same-page",
            "--value",
            value,
            "--reason",
            "scope fixture",
            "--provenance",
            "user-provided",
        ]);
        world.ok(&world.project, &weight);
        let mut feedback = prefix;
        feedback.extend([
            "weight",
            "feedback",
            "page",
            "same-page",
            "--query",
            "scope-isolation-query",
            "--signal",
            signal,
            "--reason",
            "scope fixture",
            "--provenance",
            "user-provided",
        ]);
        world.ok(&world.project, &feedback);
    }

    for (scope, expected) in [(None, 1.0), (Some("global"), -1.0)] {
        let mut args = Vec::new();
        if let Some(scope) = scope {
            args.extend(["--scope", scope]);
        }
        args.extend(["search", "scope-isolation-query", "--explain"]);
        let result = world.ok(&world.project, &args);
        let explanation = &find_result(&result, "same-page")["explanation"];
        assert_eq!(explanation["signals"]["manual_adjustment"], expected);
        assert_eq!(explanation["signals"]["feedback_adjustment"], expected);
    }

    let combined = world.ok(
        &world.project,
        &[
            "--scope",
            "all",
            "search",
            "scope-isolation-query",
            "--explain",
        ],
    );
    let rows = combined["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["identifier"] == "same-page")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_ne!(
        rows[0]["explanation"]["signals"]["manual_adjustment"],
        rows[1]["explanation"]["signals"]["manual_adjustment"]
    );
}

#[test]
fn configured_project_root_blocks_ancestor_project_reuse() {
    let world = TestWorld::new();
    let outer = world.home.join("work");
    let project = outer.join("project");
    let nested = project.join("src");
    fs::create_dir_all(&nested).unwrap();
    world.ok(&outer, &["init"]);

    let output = world.command_in_project_root(&nested, &project, &["init"]);
    assert!(
        output.status.success(),
        "bounded init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let initialized = stdout_json(&output);
    let expected_database = fs::canonicalize(&project).unwrap().join(".lwc/wiki.db");

    assert_eq!(
        initialized["database"],
        expected_database.to_string_lossy().as_ref()
    );
    assert!(project.join(".lwc/wiki.db").is_file());
}

#[test]
fn configured_project_root_rejects_a_cwd_outside_it() {
    let world = TestWorld::new();
    let outside = world.home.join("outside");
    fs::create_dir_all(&outside).unwrap();

    let output = world.command_in_project_root(&outside, &world.project, &["init"]);
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "project_root_mismatch"
    );
    assert!(!outside.join(".lwc/wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn configured_project_root_rejects_a_symlinked_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside = world.home.join("outside-store");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, world.project.join(".lwc")).unwrap();

    let output = world.command_in_project_root(&world.project, &world.project, &["init"]);
    assert!(!output.status.success());
    assert_eq!(stderr_json(&output)["error"]["code"], "project_root_escape");
    assert!(!outside.join("wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_project_scope_rejects_a_symlinked_live_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside_project = world.home.join("outside-project");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let outside_store = outside_project.join(".lwc");
    symlink(&outside_store, world.project.join(".lwc")).unwrap();

    let error = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(error["error"]["code"], "project_root_escape");
    assert!(!outside_store.join("changesets").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_project_scope_rejects_a_symlinked_live_database() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let outside_project = world.home.join("outside-project");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let project_database = world.project.join(".lwc/wiki.db");
    fs::remove_file(&project_database).unwrap();
    symlink(outside_project.join(".lwc/wiki.db"), &project_database).unwrap();

    let error = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(error["error"]["code"], "project_root_escape");
    assert!(!world.project.join(".lwc/changesets").exists());
}

#[cfg(unix)]
#[test]
fn global_scope_rejects_a_symlinked_live_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside_project = world.project.join("outside-global");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let outside_store = outside_project.join(".lwc");
    symlink(&outside_store, world.home.join(".lwc")).unwrap();

    let error = world.err(
        &world.project,
        &["--scope", "global", "changeset", "begin", "escape"],
    );
    assert_eq!(error["error"]["code"], "store_path_invalid");
    assert!(!outside_store.join("changesets").exists());
}

#[test]
fn configured_project_root_rejects_multiple_ancestor_stores() {
    let world = TestWorld::new();
    let nested = world.project.join("nested");
    fs::create_dir_all(&nested).unwrap();
    world.ok(&world.project, &["init"]);

    let nested_init = world.command_in_project_root(&nested, &nested, &["init"]);
    assert!(nested_init.status.success());

    let output = world.command_in_project_root(&nested, &world.project, &["context"]);
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "project_scope_conflict"
    );
}

#[test]
fn failures_are_structured_and_do_not_create_implicit_stores() {
    let world = TestWorld::new();

    let missing = world.err(&world.project, &["page", "list"]);
    assert_eq!(missing["error"]["code"], "store_not_found");
    assert!(!world.project.join(".lwc/wiki.db").exists());

    let no_combined = world.err(&world.project, &["--scope", "all", "search", "anything"]);
    assert_eq!(no_combined["error"]["code"], "store_not_found");
    let no_combined_context = world.err(&world.project, &["--scope", "all", "context"]);
    assert_eq!(no_combined_context["error"]["code"], "store_not_found");

    let clap_error = world.command(&world.project, &["page", "put"]);
    assert!(!clap_error.status.success());
    assert!(serde_json::from_slice::<Value>(&clap_error.stderr).is_err());
    assert!(String::from_utf8_lossy(&clap_error.stderr).contains("Usage:"));

    world.ok(&world.project, &["init"]);
    let invalid_utf8 = world.project.join("raw.bin");
    fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();
    let invalid = world.err(&world.project, &["source", "add", as_str(&invalid_utf8)]);
    assert_eq!(invalid["error"]["code"], "invalid_utf8");

    let invalid_limit = world.err(&world.project, &["context", "--limit", "0"]);
    assert_eq!(invalid_limit["error"]["code"], "invalid_limit");
}

#[test]
fn context_limit_caps_pages_and_operations_per_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    for slug in ["alpha", "beta"] {
        let page = world.write(&format!("{slug}.md"), &format!("{slug} body"));
        world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                slug,
                "--file",
                as_str(&page),
            ],
        );
    }

    let context = world.ok(&world.project, &["context", "--limit", "1"]);
    assert_eq!(context["stores"][0]["pages"].as_array().unwrap().len(), 1);
    assert_eq!(
        context["stores"][0]["recent_operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn source_add_without_title_uses_a_deterministic_origin_based_title() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/ownership.md", "Rust ownership assigns values.");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);

    let title = added["source"]["title"]
        .as_str()
        .expect("source add should synthesize a non-empty title from origin");
    assert!(!title.is_empty(), "fallback title must not be empty");
    assert!(
        title.contains("ownership.md"),
        "fallback title should be origin-based, got {title:?}"
    );

    let shown = world.ok(&world.project, &["source", "show", "1"]);
    assert_eq!(shown["source"]["title"], title);
}

#[test]
fn source_add_tracks_canonical_project_paths_without_breaking_content_deduplication() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    world.write("raw/source.md", "same tracked bytes");
    let nested = world.project.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let first_output = world.command_in_project_root(
        &nested,
        &world.project,
        &["source", "add", "../raw/source.md"],
    );
    assert!(
        first_output.status.success(),
        "source add failed: {}",
        String::from_utf8_lossy(&first_output.stderr)
    );
    let first = stdout_json(&first_output);

    let duplicate = world.write("alias/same.md", "same tracked bytes");
    let second = world.ok(&world.project, &["source", "add", as_str(&duplicate)]);
    assert_eq!(first["source"]["id"], second["source"]["id"]);

    let conn = Connection::open(world.project.join(".lwc/wiki.db")).unwrap();
    let revisions = conn
        .prepare(
            "SELECT tracked_path, revision, source_id
             FROM source_path_revisions
             ORDER BY tracked_path, revision",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        revisions,
        vec![
            ("alias/same.md".to_string(), 1, 1),
            ("raw/source.md".to_string(), 1, 1),
        ]
    );
}

#[test]
fn source_status_detects_same_size_changes_without_recording_an_operation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/status.md", "alpha");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let before = world.ok(&world.project, &["log", "--limit", "100"]);
    let current = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(current["checks"][0]["tracked_path"], "raw/status.md");
    assert_eq!(current["checks"][0]["lineage_state"], "current");
    assert_eq!(current["checks"][0]["filesystem_state"], "current");
    assert_eq!(
        current["checks"][0]["head_content_hash"],
        current["checks"][0]["live_content_hash"]
    );

    fs::write(&source, "bravo").unwrap();
    let modified = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(modified["checks"][0]["lineage_state"], "current");
    assert_eq!(modified["checks"][0]["filesystem_state"], "modified");
    assert_ne!(
        modified["checks"][0]["head_content_hash"],
        modified["checks"][0]["live_content_hash"]
    );

    fs::remove_file(&source).unwrap();
    let missing = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(missing["checks"][0]["filesystem_state"], "missing");
    assert!(missing["checks"][0]["live_content_hash"].is_null());

    let oversized = fs::File::create(&source).unwrap();
    oversized.set_len(64 * 1024 * 1024 + 1).unwrap();
    let oversized = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(oversized["checks"][0]["filesystem_state"], "oversized");
    assert!(oversized["checks"][0]["live_content_hash"].is_null());

    let unknown = world.err(&world.project, &["source", "status", "999"]);
    assert_eq!(unknown["error"]["code"], "source_not_found");

    let after = world.ok(&world.project, &["log", "--limit", "100"]);
    assert_eq!(before["operations"], after["operations"]);
}

#[test]
fn source_diff_exposes_one_character_live_changes_and_is_read_only() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/diff.md", "first\nport=80\nlast\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let before = world.ok(&world.project, &["log", "--limit", "100"]);

    fs::write(&source, "first\nport=81\nlast\n").unwrap();
    let changed = world.ok(&world.project, &["source", "diff", &source_id]);
    assert_eq!(changed["scope"], "project");
    assert_eq!(changed["from"]["kind"], "source");
    assert_eq!(changed["from"]["source_id"], added["source"]["id"]);
    assert_eq!(changed["from"]["bytes"], 19);
    assert_eq!(changed["to"]["kind"], "live");
    assert_eq!(changed["to"]["tracked_path"], "raw/diff.md");
    assert_eq!(changed["to"]["head_source_id"], added["source"]["id"]);
    assert_eq!(changed["to"]["head_revision"], 1);
    assert_eq!(changed["to"]["bytes"], 19);
    assert_ne!(
        changed["from"]["content_hash"],
        changed["to"]["content_hash"]
    );
    assert_eq!(changed["changed"], true);
    assert_eq!(changed["diff"]["format"], "unified");
    assert_eq!(changed["diff"]["context_lines"], 3);
    let text = changed["diff"]["text"].as_str().unwrap();
    assert!(text.starts_with("--- source:1\n+++ live:raw/diff.md\n"));
    assert!(text.contains("-port=80\n+port=81\n"));
    assert_eq!(
        changed["diff"]["returned_chars"].as_u64().unwrap(),
        text.chars().count() as u64
    );
    assert_eq!(
        changed["diff"]["total_chars"],
        changed["diff"]["returned_chars"]
    );
    assert_eq!(changed["diff"]["truncated"], false);

    fs::write(&source, "first\nport=80\nlast\n").unwrap();
    let current = world.ok(&world.project, &["source", "diff", &source_id]);
    assert_eq!(current["changed"], false);
    assert_eq!(current["diff"]["text"], "");
    assert_eq!(current["diff"]["returned_chars"], 0);
    assert_eq!(current["diff"]["total_chars"], 0);
    assert_eq!(current["diff"]["truncated"], false);

    let after = world.ok(&world.project, &["log", "--limit", "100"]);
    assert_eq!(before["operations"], after["operations"]);
}

#[test]
fn source_diff_compares_immutable_snapshots_without_a_live_file() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/history.md", "setting=old\n");
    let old = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let old_id = old["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&source, "setting=new\n").unwrap();
    let new = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let new_id = new["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&source).unwrap();

    let forward = world.ok(
        &world.project,
        &["source", "diff", &old_id, "--to-source", &new_id],
    );
    assert_eq!(forward["from"]["source_id"], old["source"]["id"]);
    assert_eq!(forward["to"]["kind"], "source");
    assert_eq!(forward["to"]["source_id"], new["source"]["id"]);
    assert!(
        forward["diff"]["text"]
            .as_str()
            .unwrap()
            .starts_with("--- source:1\n+++ source:2\n")
    );
    assert!(
        forward["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-setting=old\n+setting=new\n")
    );

    let reverse = world.ok(
        &world.project,
        &["source", "diff", &new_id, "--to-source", &old_id],
    );
    assert!(
        reverse["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-setting=new\n+setting=old\n")
    );

    let same = world.ok(
        &world.project,
        &["source", "diff", &old_id, "--to-source", &old_id],
    );
    assert_eq!(same["changed"], false);
    assert_eq!(same["diff"]["text"], "");

    for conflicting in [
        "--path",
        "--allow-external-source",
        "--acknowledge-sensitive-source",
    ] {
        let mut args = vec![
            "source",
            "diff",
            &old_id,
            "--to-source",
            &new_id,
            conflicting,
        ];
        if conflicting == "--path" {
            args.push("raw/history.md");
        }
        let output = world.command(&world.project, &args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}

#[test]
fn source_diff_requires_an_exact_path_when_a_source_has_multiple_paths() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let primary = world.write("raw/shared.md", "shared bytes\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&primary)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let alias = world.write("alias/shared.md", "shared bytes\n");
    world.ok(&world.project, &["source", "add", as_str(&alias)]);

    let ambiguous = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(ambiguous["error"]["code"], "source_diff_path_required");
    let message = ambiguous["error"]["message"].as_str().unwrap();
    assert!(message.contains("alias/shared.md, raw/shared.md"));

    fs::write(&alias, "changed bytes\n").unwrap();
    let selected = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--path", "alias/shared.md"],
    );
    assert_eq!(selected["to"]["tracked_path"], "alias/shared.md");
    assert_eq!(selected["changed"], true);

    let missing = world.err(
        &world.project,
        &["source", "diff", &source_id, "--path", "missing.md"],
    );
    assert_eq!(missing["error"]["code"], "source_diff_path_not_found");
}

#[test]
fn source_diff_bounds_unicode_output_and_rejects_invalid_limits() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/unicode.md", "甲乙丙丁\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&source, "甲乙😀丁\n").unwrap();

    let one = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--max-chars", "1"],
    );
    assert_eq!(one["diff"]["text"].as_str().unwrap().chars().count(), 1);
    assert_eq!(one["diff"]["returned_chars"], 1);
    assert!(one["diff"]["total_chars"].as_u64().unwrap() > 1);
    assert_eq!(one["diff"]["truncated"], true);

    assert_eq!(
        world.err(
            &world.project,
            &["source", "diff", &source_id, "--max-chars", "0"],
        )["error"]["code"],
        "invalid_limit"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["source", "diff", &source_id, "--max-chars", "100001"],
        )["error"]["code"],
        "invalid_limit"
    );
}

#[test]
fn source_diff_global_scope_never_falls_back_to_the_project_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let project_source = world.write("project.md", "project old\n");
    world.ok(&world.project, &["source", "add", as_str(&project_source)]);

    world.ok(&world.project, &["--scope", "global", "init"]);
    let global_source = world.write("global.md", "global old\n");
    let global_old = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&global_source)],
    );
    let old_id = global_old["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&global_source, "global new\n").unwrap();

    let live = world.ok(
        &world.project,
        &["--scope", "global", "source", "diff", &old_id],
    );
    assert_eq!(live["scope"], "global");
    assert_eq!(
        live["from"]["content_hash"],
        global_old["source"]["content_hash"]
    );
    assert!(
        live["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-global old\n+global new\n")
    );

    let global_new = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&global_source)],
    );
    let new_id = global_new["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&global_source).unwrap();
    let snapshots = world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "source",
            "diff",
            &old_id,
            "--to-source",
            &new_id,
        ],
    );
    assert_eq!(snapshots["scope"], "global");
    assert_eq!(
        snapshots["to"]["content_hash"],
        global_new["source"]["content_hash"]
    );
}

#[test]
fn source_diff_rejects_missing_and_invalid_utf8_live_files() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/unavailable.md", "valid evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    fs::remove_file(&source).unwrap();
    let missing = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(missing["error"]["code"], "source_diff_unavailable");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing")
    );

    fs::write(&source, [0xff, 0xfe, 0xfd]).unwrap();
    let invalid = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(invalid["error"]["code"], "invalid_utf8");
}

#[test]
fn source_diff_rejects_untracked_and_oversized_inputs_before_rendering() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let untracked = world.write("raw/untracked.md", "evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&untracked)]);
    let untracked_id = added["source"]["id"].as_i64().unwrap().to_string();
    Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute(
            "DELETE FROM source_path_revisions WHERE source_id = ?1",
            [added["source"]["id"].as_i64().unwrap()],
        )
        .unwrap();
    let error = world.err(&world.project, &["source", "diff", &untracked_id]);
    assert_eq!(error["error"]["code"], "source_diff_untracked");

    let live = world.write("raw/large-live.md", "small\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&live)]);
    let live_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::File::create(&live)
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    let error = world.err(&world.project, &["source", "diff", &live_id]);
    assert_eq!(error["error"]["code"], "source_diff_too_large");

    let too_many_lines = world.write("raw/many-lines.md", &"x\n".repeat(200_001));
    let error = world.err(&world.project, &["source", "add", as_str(&too_many_lines)]);
    assert_eq!(error["error"]["code"], "graph_index_capacity_exceeded");
}

#[cfg(unix)]
#[test]
fn source_diff_blocks_symlink_escape_and_never_blocks_on_a_fifo() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/live.md", "inside evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let external = world.project.parent().unwrap().join("outside.md");
    fs::write(&external, "outside evidence\n").unwrap();
    fs::remove_file(&tracked).unwrap();
    symlink(&external, &tracked).unwrap();

    let blocked = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["changed"], true);

    fs::remove_file(&tracked).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&tracked)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["source", "diff", &source_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("source diff blocked while opening a FIFO");
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "source_diff_unavailable"
    );
}

#[test]
fn source_status_reports_a_b_a_lineage_per_path_and_all_current_heads() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/lineage.md", "alpha");
    let alpha = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let alpha_id = alpha["source"]["id"].as_i64().unwrap().to_string();

    fs::write(&source, "bravo").unwrap();
    let bravo = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let bravo_id = bravo["source"]["id"].as_i64().unwrap().to_string();

    fs::write(&source, "alpha").unwrap();
    let alpha_again = world.ok(&world.project, &["source", "add", as_str(&source)]);
    assert_eq!(alpha_again["source"]["id"], alpha["source"]["id"]);
    let alias = world.write("alias/lineage.md", "alpha");
    world.ok(&world.project, &["source", "add", as_str(&alias)]);

    let superseded = world.ok(&world.project, &["source", "status", &bravo_id]);
    assert_eq!(superseded["checks"].as_array().unwrap().len(), 1);
    assert_eq!(superseded["checks"][0]["tracked_path"], "raw/lineage.md");
    assert_eq!(superseded["checks"][0]["head_revision"], 3);
    assert_eq!(
        superseded["checks"][0]["head_source_id"],
        alpha["source"]["id"]
    );
    assert_eq!(superseded["checks"][0]["lineage_state"], "superseded");
    assert_eq!(superseded["checks"][0]["filesystem_state"], "current");

    let current = world.ok(&world.project, &["source", "status", &alpha_id]);
    assert_eq!(current["checks"].as_array().unwrap().len(), 2);
    assert_eq!(current["checks"][0]["tracked_path"], "alias/lineage.md");
    assert_eq!(current["checks"][0]["head_revision"], 1);
    assert_eq!(current["checks"][1]["tracked_path"], "raw/lineage.md");
    assert_eq!(current["checks"][1]["head_revision"], 3);

    let combined = world.ok(&world.project, &["source", "status", &alpha_id, &bravo_id]);
    let raw_path_checks = combined["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["tracked_path"] == "raw/lineage.md")
        .collect::<Vec<_>>();
    assert_eq!(raw_path_checks.len(), 2);
    assert!(
        raw_path_checks
            .iter()
            .all(|check| check["head_source_id"] == alpha["source"]["id"])
    );

    let all = world.ok(&world.project, &["source", "status", "--all"]);
    assert_eq!(all["checks"].as_array().unwrap().len(), 2);
    assert!(all["checks"].as_array().unwrap().iter().all(|check| {
        check["requested_source_id"] == alpha["source"]["id"]
            && check["lineage_state"] == "current"
            && check["filesystem_state"] == "current"
    }));
    assert!(all["untracked_source_ids"].as_array().unwrap().is_empty());
}

#[test]
fn source_status_requires_current_external_read_authorization() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let external = world.project.parent().unwrap().join("external-source.md");
    fs::write(&external, "external evidence").unwrap();
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&external),
            "--allow-external-source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let blocked = world.err(&world.project, &["source", "status", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "status", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["checks"][0]["filesystem_state"], "current");

    let blocked_all = world.err(&world.project, &["source", "status", "--all"]);
    assert_eq!(
        blocked_all["error"]["code"],
        "external_source_requires_acknowledgement"
    );

    world.ok(&world.project, &["--scope", "global", "init"]);
    let global = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&external)],
    );
    let global_id = global["source"]["id"].as_i64().unwrap().to_string();
    let global_status = world.ok(
        &world.project,
        &["--scope", "global", "source", "status", &global_id],
    );
    assert_eq!(global_status["checks"][0]["filesystem_state"], "current");
}

#[cfg(unix)]
#[test]
fn source_status_blocks_a_tracked_path_that_now_escapes_through_a_symlink() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/symlink.md", "inside evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let external = world.project.parent().unwrap().join("symlink-target.md");
    fs::write(&external, "outside evidence").unwrap();
    fs::remove_file(&tracked).unwrap();
    symlink(&external, &tracked).unwrap();

    let blocked = world.err(&world.project, &["source", "status", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "status", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["checks"][0]["filesystem_state"], "modified");
}

#[cfg(unix)]
#[test]
fn source_status_rejects_a_fifo_without_blocking() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/fifo.md", "regular evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&tracked).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&tracked)
            .status()
            .unwrap()
            .success()
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["source", "status", &source_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("source status blocked while opening a FIFO");
    }

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "source status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = stdout_json(&output);
    assert_eq!(status["checks"][0]["filesystem_state"], "unreadable");
}

#[test]
fn source_windows_keep_large_utf8_documents_bounded_and_resumable() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/large.md", "甲乙丙丁戊己庚辛壬癸");
    world.ok(&world.project, &["source", "add", as_str(&source)]);

    let shown = world.ok(
        &world.project,
        &[
            "source",
            "show",
            "1",
            "--offset-chars",
            "3",
            "--max-chars",
            "4",
        ],
    );
    assert_eq!(shown["source"]["content"], "丁戊己庚");
    assert_eq!(shown["window"]["offset_chars"], 3);
    assert_eq!(shown["window"]["returned_chars"], 4);
    assert_eq!(shown["window"]["total_chars"], 10);
    assert_eq!(shown["window"]["next_offset_chars"], 7);
    assert_eq!(shown["window"]["has_more"], true);

    let packet = world.ok(
        &world.project,
        &["ingest", "next", "--source-max-chars", "4"],
    );
    assert_eq!(packet["job"]["source"]["content"], "甲乙丙丁");
    assert_eq!(packet["job"]["source_window"]["next_offset_chars"], 4);
    assert_eq!(packet["job"]["source_window"]["has_more"], true);
}

#[test]
fn search_directly_returns_sentence_and_passage_locators() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "span-page",
        "Span page",
        "First context. UniqueNeedle appears here. Final context.",
    );

    let sentence = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "sentence",
        ],
    );
    assert_eq!(sentence["results"][0]["type"], "sentence");
    assert_eq!(sentence["results"][0]["document"]["type"], "page");
    assert_eq!(
        sentence["results"][0]["document"]["identifier"],
        "span-page"
    );
    assert_eq!(
        sentence["results"][0]["snippet"],
        "UniqueNeedle appears here."
    );
    assert_eq!(sentence["results"][0]["span"]["segmenter_version"], 1);
    assert!(
        sentence["results"][0]["span"]["byte_end"].as_u64().unwrap()
            > sentence["results"][0]["span"]["byte_start"]
                .as_u64()
                .unwrap()
    );

    let passage = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "passage",
        ],
    );
    assert_eq!(passage["results"][0]["type"], "passage");
    assert_eq!(
        passage["results"][0]["snippet"],
        "First context. UniqueNeedle appears here. Final context."
    );

    let grouped = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "all",
            "--group-by",
            "document",
        ],
    );
    assert_eq!(grouped["results"].as_array().unwrap().len(), 1);
    assert_eq!(grouped["results"][0]["type"], "page");
    assert_eq!(grouped["results"][0]["identifier"], "span-page");
    assert_eq!(
        grouped["results"][0]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|result| result["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["sentence", "passage", "page"]
    );
    let matches = grouped["results"][0]["matches"].as_array().unwrap();
    for (actual, expected) in [
        (matches[0]["fused_score"].as_f64().unwrap(), 1.15 / 61.0),
        (matches[1]["fused_score"].as_f64().unwrap(), 1.05 / 61.0),
        (matches[2]["fused_score"].as_f64().unwrap(), 1.0 / 61.0),
    ] {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "span-page",
            "--value",
            "2",
            "--reason",
            "canonical span owner",
            "--provenance",
            "agent-observed",
        ],
    );
    let adjusted = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "sentence",
            "--explain",
        ],
    );
    assert_eq!(
        adjusted["results"][0]["explanation"]["signals"]["manual_adjustment"],
        1.0
    );
    assert_eq!(
        adjusted["results"][0]["explanation"]["contributions"]["manual"],
        -2.0
    );

    let sentence_id = sentence["results"][0]["identifier"].as_str().unwrap();
    let shown = world.ok(&world.project, &["span", "get", sentence_id]);
    assert_eq!(shown["span"]["text"], "UniqueNeedle appears here.");
    assert_eq!(shown["span"]["document"]["identifier"], "span-page");
    let expanded = world.ok(
        &world.project,
        &[
            "span",
            "expand",
            sentence_id,
            "--before",
            "1",
            "--after",
            "1",
        ],
    );
    assert_eq!(
        expanded["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| span["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "First context.",
            "UniqueNeedle appears here.",
            "Final context."
        ]
    );
    assert_eq!(
        expanded["parent"]["text"],
        "First context. UniqueNeedle appears here. Final context."
    );

    world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            sentence_id,
            "REFINES",
            "page:span-page",
            "--provenance",
            "agent-observed",
            "--reason",
            "span-scoped interpretation",
            "--confidence",
            "0.7",
        ],
    );
    let replacement = world.write(
        "span-replacement.md",
        "Replacement content has a different fingerprint.",
    );
    let replaced = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "span-page",
            "--title",
            "Span page",
            "--file",
            as_str(&replacement),
        ],
    );
    assert_eq!(replaced["graph"]["invalidated_semantic_relations"], 1);
    let stale = world.err(&world.project, &["span", "get", sentence_id]);
    assert_eq!(stale["error"]["code"], "stale_span", "{stale}");
    assert_eq!(stale["error"]["details"]["prior"]["segmenter_version"], 1);
    assert_eq!(stale["error"]["details"]["current"]["segmenter_version"], 1);
    assert_ne!(
        stale["error"]["details"]["prior"]["content_fingerprint"],
        stale["error"]["details"]["current"]["content_fingerprint"]
    );
}

#[test]
fn search_supports_type_and_kind_filters_and_exposes_page_kind() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/evidence.md", "sharedterm raw evidence");
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Evidence source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let query_page = world.write("query.md", "sharedterm durable answer");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "durable-answer",
            "--title",
            "Durable answer",
            "--kind",
            "query",
            "--file",
            as_str(&query_page),
            "--source",
            &source_id,
        ],
    );

    let source_page = world.write("source-page.md", "sharedterm curated source summary");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "source-summary",
            "--title",
            "Source summary",
            "--kind",
            "source",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );

    let filtered = world.command(
        &world.project,
        &[
            "search",
            "sharedterm",
            "--type",
            "page",
            "--kind",
            "query",
            "--kind",
            "source",
        ],
    );
    assert!(
        filtered.status.success(),
        "page/kind filtered search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&filtered.stdout),
        String::from_utf8_lossy(&filtered.stderr)
    );
    let filtered = stdout_json(&filtered);
    let results = filtered["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "filtered page search should return page results"
    );
    assert!(results.iter().all(|result| result["type"] == "page"));
    assert!(
        results
            .iter()
            .all(|result| { matches!(result["kind"].as_str(), Some("query" | "source")) })
    );
    assert!(results.iter().any(|result| result["kind"] == "query"));
    assert!(results.iter().any(|result| result["kind"] == "source"));

    let all_filtered = world.ok(
        &world.project,
        &["search", "sharedterm", "--type", "all", "--kind", "query"],
    );
    let all_filtered_results = all_filtered["results"].as_array().unwrap();
    assert!(
        all_filtered_results
            .iter()
            .any(|result| result["type"] == "source"),
        "--kind should restrict page kinds without removing raw sources from --type all: {all_filtered}"
    );
    assert!(
        all_filtered_results.iter().all(|result| {
            result["type"] == "source" || (result["type"] == "page" && result["kind"] == "query")
        }),
        "--type all --kind query should keep sources plus query pages only: {all_filtered}"
    );

    let invalid = world.command(
        &world.project,
        &[
            "search",
            "sharedterm",
            "--type",
            "source",
            "--kind",
            "source",
        ],
    );
    assert!(
        !invalid.status.success(),
        "source search with kind filter should fail"
    );
    let invalid = stderr_json(&invalid);
    assert_eq!(invalid["error"]["code"], "invalid_input");
    assert!(
        invalid["error"]["message"]
            .as_str()
            .unwrap()
            .contains("kind"),
        "source search kind error should mention kind: {invalid}"
    );
}

#[test]
fn search_auto_hides_raw_sources_behind_matching_source_pages_and_all_keeps_both() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/evidence.md", "sharedterm raw evidence");
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Evidence source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let source_page = world.write("source-page.md", "sharedterm curated source summary");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "source-summary",
            "--title",
            "Source summary",
            "--kind",
            "source",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );

    let query_page = world.write("query-page.md", "sharedterm durable answer");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "durable-answer",
            "--title",
            "Durable answer",
            "--kind",
            "query",
            "--file",
            as_str(&query_page),
            "--source",
            &source_id,
        ],
    );

    let auto = world.command(&world.project, &["search", "sharedterm", "--type", "auto"]);
    assert!(
        auto.status.success(),
        "auto search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&auto.stdout),
        String::from_utf8_lossy(&auto.stderr)
    );
    let auto = stdout_json(&auto);
    let auto_results = auto["results"].as_array().unwrap();
    assert!(
        !auto_results.is_empty(),
        "auto search should return page results"
    );
    assert!(
        auto_results.iter().all(|result| result["type"] == "page"),
        "auto search should hide raw sources when a matching kind=source page cites the source: {auto}"
    );

    let implicit_auto = world.ok(&world.project, &["search", "sharedterm"]);
    assert_eq!(
        implicit_auto["results"], auto["results"],
        "plain search should be identical to explicit --type auto"
    );

    let all = world.command(&world.project, &["search", "sharedterm", "--type", "all"]);
    assert!(
        all.status.success(),
        "all search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&all.stdout),
        String::from_utf8_lossy(&all.stderr)
    );
    let all = stdout_json(&all);
    let all_results = all["results"].as_array().unwrap();
    assert!(
        all_results.iter().any(|result| result["type"] == "page"),
        "all search should keep page results"
    );
    assert!(
        all_results.iter().any(|result| result["type"] == "source"),
        "all search should retain raw source results"
    );
    let first_source = all_results
        .iter()
        .position(|result| result["type"] == "source")
        .expect("all search should include a raw source result");
    assert!(
        all_results[..first_source]
            .iter()
            .all(|result| result["type"] == "page"),
        "all page results should be grouped before raw sources: {all}"
    );
}

#[test]
fn ingest_complete_requires_a_non_source_derived_page() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let complete = world.command(&world.project, &["ingest", "complete", &source_id]);
    assert!(
        !complete.status.success(),
        "ingest complete should refuse source-only integration"
    );
    let complete = stderr_json(&complete);
    assert_eq!(complete["error"]["code"], "ingest_integration_required");
}

#[test]
fn ingest_complete_accepts_no_derived_pages_reason_override() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let complete = world.command(
        &world.project,
        &[
            "ingest",
            "complete",
            &source_id,
            "--no-derived-pages-reason",
            "single-source archive import",
        ],
    );
    assert!(
        complete.status.success(),
        "ingest complete should accept an explicit no-derived-pages reason\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&complete.stdout),
        String::from_utf8_lossy(&complete.stderr)
    );
    let complete = stdout_json(&complete);
    assert_eq!(complete["job"]["status"], "completed");
}

#[test]
fn ingest_complete_rejects_no_derived_reason_when_a_derived_page_exists() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let derived = world.write("concept.md", "sharedterm integrated concept");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "shared-concept",
            "--title",
            "Shared concept",
            "--kind",
            "concept",
            "--file",
            as_str(&derived),
            "--source",
            &source_id,
        ],
    );

    let complete = world.command(
        &world.project,
        &[
            "ingest",
            "complete",
            &source_id,
            "--no-derived-pages-reason",
            "should not be accepted",
        ],
    );
    assert!(
        !complete.status.success(),
        "an exception reason should be rejected when a derived page exists"
    );
    assert_eq!(stderr_json(&complete)["error"]["code"], "invalid_input");
}
