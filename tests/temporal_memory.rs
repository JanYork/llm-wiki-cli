use rusqlite::Connection;
use serde_json::Value;
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

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
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
        let path = self.project.join(relative);
        fs::write(&path, content).unwrap();
        path
    }
}

fn database_path(initialized: &Value) -> PathBuf {
    PathBuf::from(initialized["database"].as_str().unwrap())
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn drop_temporal_schema_to_v13(database: &Path) {
    let conn = Connection::open(database).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS memory_fts;
         DROP TABLE IF EXISTS memory_fts_data;
         DROP TABLE IF EXISTS memory_fts_idx;
         DROP TABLE IF EXISTS memory_fts_content;
         DROP TABLE IF EXISTS memory_fts_docsize;
         DROP TABLE IF EXISTS memory_fts_config;
         DROP TABLE IF EXISTS memory_feedback;
         DROP TABLE IF EXISTS memory_relations;
         DROP TABLE IF EXISTS memory_evidence;
         DROP TABLE IF EXISTS memory_changes;
         DROP TABLE IF EXISTS memory_fragments;
         DROP TABLE IF EXISTS memory_hint_state;
         DROP TABLE IF EXISTS memory_state;
         DROP TABLE IF EXISTS memory_events;
         UPDATE meta SET value = '13' WHERE key = 'format_version';
         PRAGMA user_version = 13;
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
}

#[test]
fn memory_config_is_layered_validated_and_unsettable() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["--scope", "global", "init"]);

    let defaults = world.ok(&["config", "show"]);
    assert_eq!(defaults["memory"]["setting"], "enabled");
    assert_eq!(defaults["memory"]["origin"], "built-in");
    assert_eq!(defaults["memory"]["max_age_days"], 365);
    assert_eq!(defaults["memory"]["max_bytes"], 268_435_456_u64);

    world.ok(&[
        "--scope",
        "global",
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "180",
        "--memory-max-bytes",
        "134217728",
    ]);
    let inherited = world.ok(&["config", "show"]);
    assert_eq!(inherited["memory"]["setting"], "enabled");
    assert_eq!(inherited["memory"]["origin"], "global");
    assert_eq!(inherited["memory"]["max_age_days"], 180);
    assert_eq!(inherited["memory"]["max_bytes"], 134_217_728_u64);

    let disabled = world.ok(&["config", "set", "--memory", "disabled"]);
    assert_eq!(disabled["memory"]["setting"], "disabled");
    assert_eq!(disabled["memory"]["origin"], "project");

    let project = world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "30",
        "--memory-max-bytes",
        "1048576",
    ]);
    assert_eq!(project["memory"]["setting"], "enabled");
    assert_eq!(project["memory"]["origin"], "project");
    assert_eq!(project["memory"]["max_age_days"], 30);
    assert_eq!(project["memory"]["max_bytes"], 1_048_576_u64);

    for args in [
        vec!["config", "set", "--memory-max-age-days", "30"],
        vec![
            "config",
            "set",
            "--memory",
            "enabled",
            "--memory-max-age-days",
            "0",
        ],
        vec![
            "config",
            "set",
            "--memory",
            "enabled",
            "--memory-max-bytes",
            "0",
        ],
    ] {
        let error = world.err(&args);
        assert_eq!(error["error"]["code"], "invalid_input");
    }

    let unset = world.ok(&["config", "unset", "--memory"]);
    assert_eq!(unset["memory"]["setting"], "enabled");
    assert_eq!(unset["memory"]["origin"], "global");
    assert_eq!(unset["memory"]["max_age_days"], 180);
    assert_eq!(unset["memory"]["max_bytes"], 134_217_728_u64);

    let config_path = world.project.join(".lwc/config.json");
    fs::write(&config_path, r#"{"version":4,"office":"disabled"}"#).unwrap();
    let legacy = world.ok(&["config", "show"]);
    assert_eq!(legacy["memory"]["setting"], "enabled");
    assert_eq!(legacy["memory"]["origin"], "global");

    world.ok(&["config", "set", "--memory", "enabled"]);
    let stored: Value = serde_json::from_str(&fs::read_to_string(config_path).unwrap()).unwrap();
    assert_eq!(stored["version"], 5);
    assert_eq!(stored["memory"]["setting"], "enabled");
}

#[test]
fn version_13_store_migrates_temporal_tables_transactionally() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    drop_temporal_schema_to_v13(&database);

    let conn = Connection::open(&database).unwrap();
    conn.execute("CREATE TABLE memory_events(broken TEXT)", [])
        .unwrap();
    drop(conn);

    let failed = world.command(&["init"]);
    assert!(!failed.status.success());
    let error: Value = serde_json::from_slice(&failed.stderr).unwrap();
    assert_eq!(error["error"]["code"], "store_migration_failed");

    let conn = Connection::open(&database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 13);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_events') WHERE name = 'broken'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    assert!(conn.prepare("SELECT * FROM memory_fragments").is_err());
    conn.execute("DROP TABLE memory_events", []).unwrap();
    drop(conn);

    world.ok(&["init"]);
    let conn = Connection::open(&database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((version, format.as_str()), (14, "14"));

    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type IN ('table', 'index') AND name LIKE 'memory_%'
             ORDER BY name",
        )
        .unwrap();
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for required in [
        "memory_changes",
        "memory_events",
        "memory_events_request_id",
        "memory_events_retention",
        "memory_evidence",
        "memory_feedback",
        "memory_feedback_event",
        "memory_fragments",
        "memory_fts",
        "memory_hint_state",
        "memory_relations",
        "memory_relations_target",
        "memory_state",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing temporal schema object {required}: {names:?}"
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_state", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn unrelated_sparse_changeset_preserves_live_temporal_rows() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let conn = Connection::open(&database).unwrap();
    conn.execute(
        "INSERT INTO memory_events(
            id, request_id, fingerprint, event_type, context,
            occurred_at, pinned, logical_bytes
         ) VALUES (
            'event-before-draft', 'request-before-draft', ?1, '决策', '保留时序事件',
            '2026-08-20T00:00:00.000Z', 0, 24
         )",
        ["a".repeat(64)],
    )
    .unwrap();
    conn.execute(
        "UPDATE memory_state SET event_count = 1, logical_bytes = 24 WHERE id = 1",
        [],
    )
    .unwrap();
    drop(conn);

    world.ok(&["changeset", "begin", "unrelated-page"]);
    let body = world.write("draft.md", "与时序事件无关的页面内容");
    world.ok(&[
        "--changeset",
        "unrelated-page",
        "page",
        "put",
        "unrelated-page",
        "--title",
        "Unrelated page",
        "--file",
        as_str(&body),
        "--provenance",
        "agent-observed",
    ]);
    world.ok(&[
        "changeset",
        "commit",
        "unrelated-page",
        "--allow-lint-issues",
        "--reason",
        "temporal row preservation regression",
    ]);

    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT context FROM memory_events WHERE id = 'event-before-draft'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "保留时序事件"
    );
    assert_eq!(
        conn.query_row(
            "SELECT event_count, logical_bytes FROM memory_state WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap(),
        (1, 24)
    );
}
