use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
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

    fn command_with_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
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
        child.wait_with_output().unwrap()
    }
}

fn database_path(initialized: &Value) -> PathBuf {
    PathBuf::from(initialized["database"].as_str().unwrap())
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn minimal_capsule(context: &str, request_id: Option<&str>) -> Value {
    let mut value = serde_json::json!({
        "type": "观察",
        "context": context,
        "observed": ["记录了一条需要跨会话保留的事实"]
    });
    if let Some(request_id) = request_id {
        value["request_id"] = Value::String(request_id.to_owned());
    }
    value
}

fn remember(world: &TestWorld, capsule: &Value) -> Value {
    let raw = serde_json::to_string(capsule).unwrap();
    world.ok(&["remember", "--json", &raw])
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

#[test]
fn remember_persists_each_semantic_channel_in_relational_rows() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let prior = remember(
        &world,
        &minimal_capsule("OfficeCLI 已确认读取命令范围", Some("req-prior")),
    );
    let prior_id = prior["event"]["id"].as_str().unwrap();
    let full = serde_json::json!({
        "request_id": "req-full",
        "type": "决策",
        "context": "Office 读取能力采用透传",
        "occurred_at": "2026-08-20T10:30:00+08:00",
        "valid_from": "2026-08-20T00:00:00Z",
        "valid_to": null,
        "pinned": true,
        "observed": ["OfficeCLI 已覆盖 view/get/query", "默认环境未安装运行时"],
        "decision": ["lwc office 原样透传读取命令"],
        "constraints": ["默认不安装", "只允许读取"],
        "learned": ["透传避免维护兼容命令白名单"],
        "unresolved": ["需要观察不同平台的安装行为"],
        "outcome": ["命令接口达成共识"],
        "changes": [{
            "subject": "Office 调用方式",
            "before": "直接调用 office",
            "after": "通过 lwc office 调用",
            "reason": "统一入口"
        }],
        "evidence": [{
            "reference": "github:iOfficeAI/OfficeCLI",
            "excerpt": "读取命令的上游实现"
        }],
        "relations": [{
            "type": "supports",
            "target": prior_id,
            "basis": "补充同一能力的落地决策"
        }]
    });

    let recorded = remember(&world, &full);
    assert_eq!(recorded["created"], true);
    assert_eq!(recorded["event"]["type"], "决策");
    assert_eq!(recorded["event"]["occurred_at"], "2026-08-20T02:30:00.000Z");
    assert_eq!(recorded["event"]["valid_from"], "2026-08-20T00:00:00.000Z");
    assert_eq!(recorded["event"]["valid_to"], Value::Null);
    assert_eq!(recorded["event"]["pinned"], true);
    let event_id = recorded["event"]["id"].as_str().unwrap();

    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_events') WHERE name = 'payload_json'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    let fragments = {
        let mut statement = conn
            .prepare(
                "SELECT kind, ordinal, value FROM memory_fragments
                 WHERE event_id = ?1
                 ORDER BY CASE kind
                    WHEN 'observed' THEN 1 WHEN 'decision' THEN 2
                    WHEN 'constraint' THEN 3 WHEN 'learned' THEN 4
                    WHEN 'unresolved' THEN 5 ELSE 6 END, ordinal",
            )
            .unwrap();
        statement
            .query_map([event_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(
        fragments,
        [
            ("observed", 0, "OfficeCLI 已覆盖 view/get/query"),
            ("observed", 1, "默认环境未安装运行时"),
            ("decision", 0, "lwc office 原样透传读取命令"),
            ("constraint", 0, "默认不安装"),
            ("constraint", 1, "只允许读取"),
            ("learned", 0, "透传避免维护兼容命令白名单"),
            ("unresolved", 0, "需要观察不同平台的安装行为"),
            ("outcome", 0, "命令接口达成共识"),
        ]
        .map(|(kind, ordinal, value)| (kind.to_owned(), ordinal, value.to_owned()))
    );
    assert_eq!(
        conn.query_row(
            "SELECT subject, before_value, after_value, reason
             FROM memory_changes WHERE event_id = ?1",
            [event_id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?
            )),
        )
        .unwrap(),
        (
            "Office 调用方式".to_owned(),
            "直接调用 office".to_owned(),
            "通过 lwc office 调用".to_owned(),
            "统一入口".to_owned(),
        )
    );
    assert_eq!(
        conn.query_row(
            "SELECT reference, excerpt FROM memory_evidence WHERE event_id = ?1",
            [event_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap(),
        (
            "github:iOfficeAI/OfficeCLI".to_owned(),
            "读取命令的上游实现".to_owned(),
        )
    );
    assert_eq!(
        conn.query_row(
            "SELECT relation_type, target_event_id, basis
             FROM memory_relations WHERE event_id = ?1",
            [event_id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            )),
        )
        .unwrap(),
        (
            "supports".to_owned(),
            prior_id.to_owned(),
            "补充同一能力的落地决策".to_owned(),
        )
    );
}

#[test]
fn remember_accepts_inline_stdin_and_scoped_at_file_json() {
    let world = TestWorld::new();
    world.ok(&["init"]);

    let inline = remember(&world, &minimal_capsule("行内 JSON", Some("req-inline")));
    assert_eq!(inline["created"], true);

    let stdin_raw = serde_json::to_string(&minimal_capsule("标准输入", Some("req-stdin"))).unwrap();
    let stdin = world.command_with_stdin(&["remember", "--json", "-"], &stdin_raw);
    assert!(
        stdin.status.success(),
        "{}",
        String::from_utf8_lossy(&stdin.stderr)
    );
    let stdin: Value = serde_json::from_slice(&stdin.stdout).unwrap();
    assert_eq!(stdin["event"]["context"], "标准输入");

    let file_raw = serde_json::to_string(&minimal_capsule("项目文件", Some("req-file"))).unwrap();
    let file = world.write("event.json", &file_raw);
    let selector = format!("@{}", file.display());
    let from_file = world.ok(&["remember", "--json", &selector]);
    assert_eq!(from_file["event"]["context"], "项目文件");

    let outside = world.home.join("outside.json");
    fs::write(
        &outside,
        serde_json::to_vec(&minimal_capsule("越界文件", Some("req-outside"))).unwrap(),
    )
    .unwrap();
    let outside = format!("@{}", outside.display());
    let rejected = world.err(&["remember", "--json", &outside]);
    assert_eq!(rejected["error"]["code"], "project_root_escape");
}

#[test]
fn same_request_and_payload_is_idempotent_across_processes() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let capsule = minimal_capsule("跨进程重试", Some("req-idempotent"));

    let first = remember(&world, &capsule);
    let replay = remember(&world, &capsule);
    assert_eq!(first["created"], true);
    assert_eq!(replay["created"], false);
    assert_eq!(replay["event"]["id"], first["event"]["id"]);

    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT record_attempts, inserted_events, idempotent_replays
             FROM memory_state WHERE id = 1",
            [],
            |row| Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?
            )),
        )
        .unwrap(),
        (2, 1, 1)
    );
}

#[test]
fn changed_request_replay_conflicts_without_mutation() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let original = minimal_capsule("不可变重试", Some("req-conflict"));
    remember(&world, &original);
    let before: (i64, i64, i64, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT record_attempts, inserted_events, idempotent_replays, event_count
             FROM memory_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    let mut changed = original;
    changed["observed"] = serde_json::json!(["同一个 request_id 却改变了内容"]);
    let raw = serde_json::to_string(&changed).unwrap();
    let conflict = world.err(&["remember", "--json", &raw]);
    assert_eq!(conflict["error"]["code"], "memory_request_conflict");

    let conn = Connection::open(database).unwrap();
    let after: (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT record_attempts, inserted_events, idempotent_replays, event_count
             FROM memory_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        conn.query_row(
            "SELECT value FROM memory_fragments WHERE kind = 'observed'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "记录了一条需要跨会话保留的事实"
    );
}

#[test]
fn identical_capsules_without_the_same_request_id_remain_distinct() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let capsule = minimal_capsule("相同内容不得自动合并", None);
    let first = remember(&world, &capsule);
    let second = remember(&world, &capsule);
    let third = remember(
        &world,
        &minimal_capsule("相同内容不得自动合并", Some("req-distinct-a")),
    );
    let fourth = remember(
        &world,
        &minimal_capsule("相同内容不得自动合并", Some("req-distinct-b")),
    );
    let ids =
        [&first, &second, &third, &fourth].map(|value| value["event"]["id"].as_str().unwrap());
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        4
    );

    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(DISTINCT fingerprint) FROM memory_events",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1,
        "fingerprints are retry evidence, never a semantic uniqueness key"
    );
}

#[test]
fn remember_rejects_unknown_empty_or_malformed_capsules() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    for raw in [
        "{",
        r#"{"type":"观察","context":"未知字段","observed":["事实"],"summary":"禁止"}"#,
        r#"{"type":"","context":"空类型","observed":["事实"]}"#,
        r#"{"type":"观察","context":"   ","observed":["事实"]}"#,
        r#"{"type":"观察","context":"没有有效内容"}"#,
        r#"{"type":"观察","context":"空条目","observed":["   "]}"#,
        r#"{"type":"观察","context":"错误时间","observed":["事实"],"occurred_at":"不是时间"}"#,
    ] {
        let error = world.err(&["remember", "--json", raw]);
        assert_eq!(error["error"]["code"], "invalid_memory_capsule", "{raw}");
    }
    assert_eq!(
        Connection::open(&database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );

    world.ok(&["config", "set", "--memory", "disabled"]);
    let raw = serde_json::to_string(&minimal_capsule("禁用时不写入", None)).unwrap();
    let disabled = world.err(&["remember", "--json", &raw]);
    assert_eq!(disabled["error"]["code"], "memory_disabled");
}

#[test]
fn remember_rejects_scope_all_and_changesets() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["--scope", "global", "init"]);
    world.ok(&["changeset", "begin", "memory-write"]);
    let raw = serde_json::to_string(&minimal_capsule("作用域边界", None)).unwrap();

    let all = world.err(&["--scope", "all", "remember", "--json", &raw]);
    assert_eq!(all["error"]["code"], "scope_not_supported");
    let staged = world.err(&["--changeset", "memory-write", "remember", "--json", &raw]);
    assert_eq!(staged["error"]["code"], "changeset_command_unsupported");
}
