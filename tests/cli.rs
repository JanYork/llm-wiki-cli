use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
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
fn every_public_command_exposes_renderable_help() {
    let world = TestWorld::new();
    let commands = [
        "init",
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
        "graph",
        "graph related",
        "maintenance",
        "maintenance materialize",
        "maintenance reindex",
        "maintenance compact",
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

    let search = world.ok(&world.project, &["search", "readonly-query-term"]);
    assert_eq!(search["results"][0]["identifier"], "readonly");

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
    assert_eq!(version, 8, "context should migrate a writable legacy store");
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
        "DROP TABLE source_path_revisions;
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
    assert_eq!(version, 8);
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
        "DROP TABLE source_path_revisions;
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
    assert_eq!(version, 8);
    assert_eq!(tracked, 0, "migration must not guess a legacy path head");

    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let status = world.ok(&world.project, &["source", "status", &source_id]);
    assert!(status["checks"].as_array().unwrap().is_empty());
    assert_eq!(status["untracked_source_ids"], serde_json::json!([1]));
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
