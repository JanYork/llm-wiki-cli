use rusqlite::{Connection, params};
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

fn database_path(initialized: &Value) -> PathBuf {
    PathBuf::from(initialized["database"].as_str().unwrap())
}

fn wal_path(database: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", database.display()))
}

#[test]
fn new_store_uses_contentless_search_fts_and_keeps_identifiers_readable() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);

    let source_file = world.write("source.md", "evidence for the page");
    let source = world.ok(&["source", "add", as_str(&source_file), "--title", "Evidence"]);
    let source_id = source["source"]["id"].as_i64().unwrap().to_string();

    let page_file = world.write("page.md", "contentless unique term");
    world.ok(&[
        "page",
        "put",
        "contentless-page",
        "--title",
        "Contentless Page",
        "--summary",
        "search row stays readable",
        "--file",
        as_str(&page_file),
        "--source",
        &source_id,
    ]);

    let conn = Connection::open(&database).unwrap();
    let create_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'search_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let normalized = create_sql.to_ascii_lowercase().replace(' ', "");
    assert!(
        normalized.contains("content=''"),
        "search_fts should be contentless\n{create_sql}"
    );
    assert!(
        normalized.contains("contentless_delete=1"),
        "search_fts should support contentless-delete updates\n{create_sql}"
    );
    assert!(
        normalized.contains("contentless_unindexed=1"),
        "search_fts should keep UNINDEXED identifiers readable\n{create_sql}"
    );
    assert!(
        normalized.contains("path_terms"),
        "search_fts should index canonical paths separately\n{create_sql}"
    );

    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    for table in ["retrieval_weights", "retrieval_feedback", "changesets"] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing retrieval state table {table}");
    }
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
        assert_eq!(columns, 1, "missing derived retrieval feature on {table}");
    }

    let rows = {
        let mut statement = conn
            .prepare(
                "SELECT doc_type, identifier
                 FROM search_fts
                 ORDER BY doc_type, identifier",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert!(
        rows.contains(&("page".to_string(), "contentless-page".to_string())),
        "page identifier should stay readable from search_fts: {rows:?}"
    );
    assert!(
        rows.contains(&("source".to_string(), source_id)),
        "source identifier should stay readable from search_fts: {rows:?}"
    );
}

#[test]
fn store_identity_is_stable_and_recorded_mutations_advance_revision() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);

    let metadata = |key: &str| -> String {
        Connection::open(&database)
            .unwrap()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .unwrap()
    };
    let store_id = metadata("store_id");
    let initial_revision = metadata("store_revision");
    assert_eq!(store_id.len(), 64);
    assert_eq!(initial_revision.len(), 64);
    assert!(store_id.chars().all(|value| value.is_ascii_hexdigit()));
    assert!(
        initial_revision
            .chars()
            .all(|value| value.is_ascii_hexdigit())
    );

    let shown = world.ok(&["schema", "show"]);
    assert!(!shown["schema"].as_str().unwrap().is_empty());
    assert_eq!(metadata("store_id"), store_id);
    assert_eq!(metadata("store_revision"), initial_revision);

    let page = world.write("revision.md", "revision contract");
    world.ok(&[
        "page",
        "put",
        "revision-contract",
        "--title",
        "Revision Contract",
        "--file",
        as_str(&page),
        "--provenance",
        "agent-observed",
    ]);
    let mutated_revision = metadata("store_revision");
    assert_ne!(mutated_revision, initial_revision);
    assert_eq!(mutated_revision.len(), 64);

    world.ok(&["search", "revision contract"]);
    assert_eq!(metadata("store_revision"), mutated_revision);
}

#[test]
fn source_origin_and_page_slug_are_rebuilt_into_path_terms() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);

    let source_file = world.write(
        "docs/payment/reconciliation-rules.md",
        "shared evidence term",
    );
    let source = world.ok(&[
        "source",
        "add",
        as_str(&source_file),
        "--title",
        "Unrelated explicit source title",
    ]);
    let source_id = source["source"]["id"].as_i64().unwrap().to_string();
    let page_file = world.write("page.md", "shared evidence term");
    world.ok(&[
        "page",
        "put",
        "payment-reconciliation-guide",
        "--title",
        "Unrelated explicit page title",
        "--file",
        as_str(&page_file),
    ]);

    let before = world.ok(&["search", "reconciliation", "--type", "all"]);
    assert!(
        before["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row["type"] == "source" && row["identifier"] == source_id })
    );
    assert!(before["results"].as_array().unwrap().iter().any(|row| {
        row["type"] == "page" && row["identifier"] == "payment-reconciliation-guide"
    }));

    let queued = world.ok(&["maintenance", "reindex"]);
    let work_id = queued["work"]["id"].as_str().unwrap().to_owned();
    let finished = world.ok(&["work", "watch", &work_id]);
    assert_eq!(finished["work"]["state"], "succeeded", "{finished}");
    let after = world.ok(&["search", "reconciliation", "--type", "all"]);
    assert_eq!(before["results"], after["results"]);

    let conn = Connection::open(database).unwrap();
    conn.execute_batch("CREATE VIRTUAL TABLE search_vocab USING fts5vocab(search_fts, col);")
        .unwrap();
    let non_empty: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT term) FROM search_vocab WHERE col = 'path_terms'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(non_empty > 0, "expected indexed path vocabulary");
    conn.execute_batch("DROP TABLE search_vocab;").unwrap();
}

#[test]
fn maintenance_compact_truncates_wal_and_reports_metrics() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let wal = wal_path(&database);

    let conn = Connection::open(&database).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    for i in 0..64 {
        conn.execute(
            "INSERT INTO operations(action, target, detail_json) VALUES('test_write', ?1, '{}')",
            params![format!("wal-{i}")],
        )
        .unwrap();
    }

    let before_bytes = fs::metadata(&wal).unwrap().len();
    assert!(
        before_bytes > 0,
        "test setup should leave a non-empty WAL file"
    );

    let output = world.command(&["maintenance", "compact"]);
    assert!(
        output.status.success(),
        "command {:?} failed\nstdout: {}\nstderr: {}",
        ["maintenance", "compact"],
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let queued: Value = serde_json::from_slice(&output.stdout).unwrap();
    let work_id = queued["work"]["id"].as_str().unwrap();
    let finished = world.ok(&["work", "watch", work_id]);
    assert_eq!(finished["work"]["state"], "succeeded", "{finished}");
    let json = &finished["work"]["result"];
    assert_eq!(json["scope"], json!("project"));
    assert_eq!(
        json["database"],
        json!(database.to_string_lossy().to_string())
    );
    assert_eq!(json["before_bytes"], json!(before_bytes));
    assert!(
        json["after_bytes"].is_u64(),
        "compact should report WAL bytes after checkpoint: {json}"
    );
    assert!(
        json["busy"].is_boolean(),
        "compact should report whether checkpoint was busy: {json}"
    );
    assert!(
        json["log_frames"].is_i64() || json["log_frames"].is_u64(),
        "compact should report WAL frame count: {json}"
    );
    assert!(
        json["checkpointed_frames"].is_i64() || json["checkpointed_frames"].is_u64(),
        "compact should report checkpointed frame count: {json}"
    );

    let after_bytes = fs::metadata(&wal).map(|meta| meta.len()).unwrap_or(0);
    assert_eq!(json["after_bytes"], json!(after_bytes));
    if json["busy"] == json!(false) {
        assert_eq!(
            after_bytes, 0,
            "non-busy compact should truncate the WAL file: {json}"
        );
    }
}

#[test]
fn lint_reports_shallow_ingest_when_completed_job_only_has_source_summary() {
    let world = TestWorld::new();
    world.ok(&["init"]);

    let source_file = world.write("paper.md", "one source with no derived concept page");
    let source = world.ok(&["source", "add", as_str(&source_file), "--title", "Paper"]);
    let source_id = source["source"]["id"].as_i64().unwrap().to_string();

    let analysis = world.write(
        "analysis.md",
        "# Analysis\nOnly add the mandatory source page.",
    );
    world.ok(&["ingest", "next"]);
    world.ok(&["ingest", "analyze", &source_id, "--file", as_str(&analysis)]);

    let summary = world.write("summary.md", "Only the required source summary exists.");
    world.ok(&[
        "page",
        "put",
        "paper-source",
        "--title",
        "Paper source",
        "--kind",
        "source",
        "--summary",
        "Mandatory source summary",
        "--file",
        as_str(&summary),
        "--source",
        &source_id,
    ]);
    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(database).unwrap();
    conn.execute(
        "UPDATE ingest_jobs
         SET status = 'completed',
             updated_at = STRFTIME('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE source_id = ?1",
        params![source_id.parse::<i64>().unwrap()],
    )
    .unwrap();

    let lint = world.ok(&["lint"]);
    assert_eq!(
        lint["counts"]["shallow_ingest"],
        json!(1),
        "completed ingest with only a source page should be flagged: {lint}"
    );
    assert!(
        lint["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "shallow_ingest"),
        "lint issues should include shallow_ingest: {lint}"
    );

    conn.execute(
        "UPDATE ingest_jobs
         SET no_derived_pages_reason = 'duplicate evidence'
         WHERE source_id = ?1",
        params![source_id.parse::<i64>().unwrap()],
    )
    .unwrap();
    let lint = world.ok(&["lint"]);
    assert_eq!(
        lint["counts"].get("shallow_ingest"),
        None,
        "a recorded no-derived-pages reason should suppress shallow_ingest: {lint}"
    );
}
