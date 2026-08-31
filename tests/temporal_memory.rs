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
         DROP TABLE IF EXISTS todo_fts;
         DROP TABLE IF EXISTS todo_fts_data;
         DROP TABLE IF EXISTS todo_fts_idx;
         DROP TABLE IF EXISTS todo_fts_content;
         DROP TABLE IF EXISTS todo_fts_docsize;
         DROP TABLE IF EXISTS todo_fts_config;
         DROP TABLE IF EXISTS todo_tags;
         DROP TABLE IF EXISTS todo_items;
         DROP TABLE IF EXISTS plan_fts;
         DROP TABLE IF EXISTS plan_fts_data;
         DROP TABLE IF EXISTS plan_fts_idx;
         DROP TABLE IF EXISTS plan_fts_content;
         DROP TABLE IF EXISTS plan_fts_docsize;
         DROP TABLE IF EXISTS plan_fts_config;
         DROP TABLE IF EXISTS plan_history;
         DROP TABLE IF EXISTS plan_steps;
         DROP TABLE IF EXISTS plan_constraints;
         DROP TABLE IF EXISTS plan_tags;
         DROP TABLE IF EXISTS plans;
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
    assert_eq!(stored["version"], 7);
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
    assert_eq!((version, format.as_str()), (17, "17"));

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
        "memory_fragments_kind",
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

    let conn = Connection::open(&database).unwrap();
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

#[test]
fn recall_is_bounded_cjk_searchable_and_time_filterable() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let older = remember(
        &world,
        &serde_json::json!({
            "type": "策略变更",
            "context": "支付失败重试策略",
            "occurred_at": "2026-08-01T09:00:00Z",
            "decision": ["支付失败最多重试三次"]
        }),
    );
    let newer = remember(
        &world,
        &serde_json::json!({
            "type": "策略变更",
            "context": "支付失败重试策略",
            "occurred_at": "2026-08-15T09:00:00Z",
            "decision": ["支付失败最多重试两次"]
        }),
    );
    remember(
        &world,
        &serde_json::json!({
            "type": "故障",
            "context": "库存同步延迟",
            "occurred_at": "2026-08-18T09:00:00Z",
            "observed": ["库存同步队列出现积压"]
        }),
    );

    let bounded = world.ok(&["memory", "recall", "支付重试", "--limit", "1"]);
    assert_eq!(bounded["query"], "支付重试");
    assert_eq!(bounded["results"].as_array().unwrap().len(), 1);
    assert_eq!(bounded["results"][0]["event"]["id"], newer["event"]["id"]);

    let windowed = world.ok(&[
        "memory",
        "recall",
        "支付重试",
        "--since",
        "2026-08-10T00:00:00Z",
        "--until",
        "2026-08-20T00:00:00Z",
        "--limit",
        "10",
    ]);
    assert_eq!(windowed["results"].as_array().unwrap().len(), 1);
    assert_eq!(windowed["results"][0]["event"]["id"], newer["event"]["id"]);
    assert_ne!(windowed["results"][0]["event"]["id"], older["event"]["id"]);

    let invalid = world.err(&["memory", "recall", "支付重试", "--limit", "0"]);
    assert_eq!(invalid["error"]["code"], "invalid_limit");
}

#[test]
fn superseding_event_completes_the_old_pattern_without_merging_other_entities() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let old = remember(
        &world,
        &serde_json::json!({
            "type": "策略变更",
            "context": "支付网关甲版重试三次",
            "occurred_at": "2026-08-01T09:00:00Z",
            "decision": ["旧支付策略"]
        }),
    );
    let old_id = old["event"]["id"].as_str().unwrap();
    let replacement = remember(
        &world,
        &serde_json::json!({
            "type": "策略变更",
            "context": "支付网关乙版重试两次",
            "occurred_at": "2026-08-15T09:00:00Z",
            "decision": ["新支付策略"],
            "relations": [{
                "type": "supersedes",
                "target": old_id,
                "basis": "支付网关限流规则变化"
            }]
        }),
    );
    let inventory = remember(
        &world,
        &serde_json::json!({
            "type": "策略变更",
            "context": "库存失败重试三次",
            "occurred_at": "2026-08-16T09:00:00Z",
            "decision": ["库存策略保持三次"]
        }),
    );

    let current = world.ok(&["memory", "recall", "甲版"]);
    assert_eq!(current["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        current["results"][0]["event"]["id"],
        replacement["event"]["id"]
    );
    assert_eq!(current["results"][0]["state"], "current");
    assert_eq!(
        current["results"][0]["explanation"]["matched_via"],
        "superseded_event"
    );

    let weak_current = remember(
        &world,
        &serde_json::json!({
            "type": "旁支记录",
            "context": "甲版旁支",
            "observed": ["只有一个查询词命中"]
        }),
    );
    let history = world.ok(&["memory", "recall", "甲版 支付网关", "--include-superseded"]);
    let history = history["results"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    assert!(history.iter().any(|result| {
        result["event"]["id"] == old["event"]["id"] && result["state"] == "superseded"
    }));
    assert!(history.iter().any(|result| {
        result["event"]["id"] == replacement["event"]["id"] && result["state"] == "current"
    }));
    let old_position = history
        .iter()
        .position(|result| result["event"]["id"] == old["event"]["id"])
        .unwrap();
    let weak_position = history
        .iter()
        .position(|result| result["event"]["id"] == weak_current["event"]["id"])
        .unwrap();
    assert!(
        history[old_position]["rank"].as_f64().unwrap()
            < history[weak_position]["rank"].as_f64().unwrap()
    );
    assert!(
        old_position < weak_position,
        "a stronger superseded lexical hit must rank before a weaker current hit"
    );

    let inventory_results = world.ok(&["memory", "recall", "库存失败"]);
    assert_eq!(inventory_results["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        inventory_results["results"][0]["event"]["id"],
        inventory["event"]["id"]
    );
}

#[test]
fn scope_all_recall_merges_project_and_global_memories() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["--scope", "global", "init"]);
    let project = remember(
        &world,
        &serde_json::json!({
            "type": "经验",
            "context": "跨作用域召回",
            "learned": ["项目记忆"]
        }),
    );
    let raw = serde_json::to_string(&serde_json::json!({
        "type": "经验",
        "context": "跨作用域召回",
        "learned": ["全局记忆"]
    }))
    .unwrap();
    let global = world.ok(&["--scope", "global", "remember", "--json", &raw]);

    let recalled = world.ok(&[
        "--scope",
        "all",
        "memory",
        "recall",
        "跨作用域召回",
        "--limit",
        "10",
    ]);
    let results = recalled["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|result| {
        result["scope"] == "project" && result["event"]["id"] == project["event"]["id"]
    }));
    assert!(results.iter().any(|result| {
        result["scope"] == "global" && result["event"]["id"] == global["event"]["id"]
    }));
}

#[test]
fn feedback_is_append_only_and_reranks_only_matching_memories() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let useful = remember(
        &world,
        &serde_json::json!({
            "type": "经验",
            "context": "批处理超时恢复",
            "occurred_at": "2026-08-01T09:00:00Z",
            "learned": ["采用小批量策略"]
        }),
    );
    let not_useful = remember(
        &world,
        &serde_json::json!({
            "type": "经验",
            "context": "批处理超时恢复",
            "occurred_at": "2026-08-15T09:00:00Z",
            "learned": ["采用全量重启"]
        }),
    );
    let useful_id = useful["event"]["id"].as_str().unwrap();
    let not_useful_id = not_useful["event"]["id"].as_str().unwrap();

    for reason in ["这次直接帮助定位", "另一次复用仍然有效"] {
        let feedback = world.ok(&[
            "memory", "feedback", useful_id, "--signal", "useful", "--reason", reason,
        ]);
        assert_eq!(feedback["event_id"], useful_id);
        assert_eq!(feedback["signal"], "useful");
    }
    world.ok(&[
        "memory",
        "feedback",
        not_useful_id,
        "--signal",
        "not-useful",
        "--reason",
        "没有帮助解决本次问题",
    ]);

    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_feedback", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT feedback_useful, feedback_not_useful FROM memory_state WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap(),
        (2, 1)
    );

    let recalled = world.ok(&["memory", "recall", "批处理超时恢复"]);
    assert_eq!(recalled["results"][0]["event"]["id"], useful["event"]["id"]);
    assert_eq!(recalled["results"][0]["explanation"]["feedback"], 2);
    let unrelated = world.ok(&["memory", "recall", "办公室门禁"]);
    assert!(unrelated["results"].as_array().unwrap().is_empty());
}

#[test]
fn memory_show_returns_the_complete_capsule_and_relations() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let prior = remember(&world, &minimal_capsule("旧部署步骤", Some("show-prior")));
    let prior_id = prior["event"]["id"].as_str().unwrap();
    let full = remember(
        &world,
        &serde_json::json!({
            "type": "复盘",
            "context": "部署超时复盘",
            "observed": ["健康检查等待过短"],
            "decision": ["延长健康检查窗口"],
            "constraints": ["不能中断现有请求"],
            "learned": ["先观测再切流"],
            "unresolved": ["仍需观察峰值流量"],
            "outcome": ["灰度发布成功"],
            "changes": [{
                "subject": "健康检查窗口",
                "before": "30 秒",
                "after": "90 秒",
                "reason": "冷启动需要更久"
            }],
            "evidence": [{
                "reference": "运维记录 2026-08-20",
                "excerpt": "90 秒后实例稳定"
            }],
            "relations": [{
                "type": "supersedes",
                "target": prior_id,
                "basis": "新步骤已经验证"
            }]
        }),
    );
    let event_id = full["event"]["id"].as_str().unwrap();

    let shown = world.ok(&["memory", "show", event_id]);
    assert_eq!(shown["scope"], "project");
    assert_eq!(shown["event"], full["event"]);
}

fn memory_read_snapshot(database: &Path) -> (i64, (i64, i64, i64, i64, i64)) {
    let conn = Connection::open(database).unwrap();
    let operations = conn
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let state = conn
        .query_row(
            "SELECT record_attempts, inserted_events, idempotent_replays,
                    feedback_useful, feedback_not_useful
             FROM memory_state WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    (operations, state)
}

#[test]
fn recall_scope_all_is_read_only_and_memory_rejects_changesets() {
    let world = TestWorld::new();
    let project = world.ok(&["init"]);
    let global = world.ok(&["--scope", "global", "init"]);
    let event = remember(&world, &minimal_capsule("只读跨作用域召回", None));
    let raw = serde_json::to_string(&minimal_capsule("只读跨作用域召回", None)).unwrap();
    world.ok(&["--scope", "global", "remember", "--json", &raw]);
    let project_database = database_path(&project);
    let global_database = database_path(&global);
    let before = (
        memory_read_snapshot(&project_database),
        memory_read_snapshot(&global_database),
    );

    let recalled = world.ok(&["--scope", "all", "memory", "recall", "只读跨作用域"]);
    assert_eq!(recalled["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        before,
        (
            memory_read_snapshot(&project_database),
            memory_read_snapshot(&global_database)
        )
    );

    let event_id = event["event"]["id"].as_str().unwrap();
    let all_feedback = world.err(&[
        "--scope",
        "all",
        "memory",
        "feedback",
        event_id,
        "--signal",
        "useful",
        "--reason",
        "不允许跨库写入",
    ]);
    assert_eq!(all_feedback["error"]["code"], "scope_not_supported");

    world.ok(&["changeset", "begin", "memory-read"]);
    let staged_recall = world.err(&[
        "--changeset",
        "memory-read",
        "memory",
        "recall",
        "只读跨作用域",
    ]);
    assert_eq!(
        staged_recall["error"]["code"],
        "changeset_command_unsupported"
    );
    let staged_feedback = world.err(&[
        "--changeset",
        "memory-read",
        "memory",
        "feedback",
        event_id,
        "--signal",
        "useful",
        "--reason",
        "不允许写入草稿库",
    ]);
    assert_eq!(
        staged_feedback["error"]["code"],
        "changeset_command_unsupported"
    );
}

#[test]
fn age_retention_evicts_only_expired_unprotected_events() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "1000000",
    ]);
    let pinned = remember(
        &world,
        &serde_json::json!({
            "type": "历史事件",
            "context": "过期但固定的记忆",
            "occurred_at": "2000-01-01T00:00:00Z",
            "pinned": true,
            "observed": ["固定事件必须保留"]
        }),
    );
    let ordinary = remember(
        &world,
        &serde_json::json!({
            "type": "历史事件",
            "context": "应该淡忘的普通记忆",
            "occurred_at": "2000-01-02T00:00:00Z",
            "observed": ["普通事件已经过期"]
        }),
    );
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "1",
        "--memory-max-bytes",
        "1000000",
    ]);
    let triggered = remember(&world, &minimal_capsule("触发年龄维护", None));
    assert_eq!(triggered["retention"]["age_evicted"], 1);
    assert_eq!(triggered["retention"]["capacity_evicted"], 0);
    assert!(
        triggered["retention"]["logical_bytes_removed"]
            .as_u64()
            .unwrap()
            > 0
    );

    let conn = Connection::open(&database).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
            [pinned["event"]["id"].as_str().unwrap()],
            |row| row.get::<_, bool>(0),
        )
        .unwrap()
    );
    assert!(
        !conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [ordinary["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert_eq!(
        conn.query_row(
            "SELECT age_evictions, capacity_evictions, event_count
             FROM memory_state WHERE id = 1",
            [],
            |row| Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?
            )),
        )
        .unwrap(),
        (1, 0, 2)
    );
    let retention_log: String = conn
        .query_row(
            "SELECT detail_json FROM operations
             WHERE action = 'memory_retention' ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!retention_log.contains("应该淡忘的普通记忆"));
    drop(conn);

    let immediately_expired = remember(
        &world,
        &serde_json::json!({
            "type": "历史事件",
            "context": "写入时已经过期",
            "occurred_at": "1999-01-01T00:00:00Z",
            "observed": ["同一事务内淘汰"]
        }),
    );
    assert_eq!(immediately_expired["retention"]["age_evicted"], 1);
    assert!(
        !Connection::open(database)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [immediately_expired["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
}

#[test]
fn recall_filters_expired_events_before_physical_maintenance() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "1000000",
    ]);
    let expired = remember(
        &world,
        &serde_json::json!({
            "type": "历史事件",
            "context": "尚未物理清理的过期记忆",
            "occurred_at": "2000-01-01T00:00:00Z",
            "observed": ["读取时必须先隐藏"]
        }),
    );
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "1",
        "--memory-max-bytes",
        "1000000",
    ]);

    let recalled = world.ok(&["memory", "recall", "尚未物理清理"]);
    assert!(recalled["results"].as_array().unwrap().is_empty());
    assert!(
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [expired["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );

    let maintained = world.ok(&["memory", "maintain"]);
    assert_eq!(maintained["retention"]["age_evicted"], 1);
    assert_eq!(
        Connection::open(database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

fn complete_memory_snapshot(database: &Path) -> (i64, i64, i64, i64, i64, i64) {
    let conn = Connection::open(database).unwrap();
    conn.query_row(
        "SELECT record_attempts, inserted_events, age_evictions,
                    capacity_evictions, event_count, logical_bytes
             FROM memory_state WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )
    .unwrap()
}

#[test]
fn byte_budget_evicts_oldest_unprotected_and_rolls_back_when_blocked() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let oldest = remember(
        &world,
        &serde_json::json!({
            "type": "容量事件",
            "context": "最旧普通事件",
            "occurred_at": "2026-08-01T00:00:00Z",
            "observed": ["容量样本甲"]
        }),
    );
    remember(
        &world,
        &serde_json::json!({
            "type": "容量事件",
            "context": "固定容量事件",
            "occurred_at": "2026-08-02T00:00:00Z",
            "pinned": true,
            "observed": ["固定容量样本"]
        }),
    );
    let newer = remember(
        &world,
        &serde_json::json!({
            "type": "容量事件",
            "context": "较新普通事件",
            "occurred_at": "2026-08-03T00:00:00Z",
            "observed": ["容量样本乙"]
        }),
    );
    let incoming = serde_json::json!({
        "type": "容量事件",
        "context": "最新普通事件",
        "occurred_at": "2026-08-04T00:00:00Z",
        "observed": ["容量样本丙"]
    });
    let sizing = TestWorld::new();
    sizing.ok(&["init"]);
    let incoming_bytes = remember(&sizing, &incoming)["event"]["logical_bytes"]
        .as_u64()
        .unwrap();
    let current_bytes = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT logical_bytes FROM memory_state WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let current_bytes = u64::try_from(current_bytes).unwrap();
    let max_bytes = (current_bytes + incoming_bytes - 1).to_string();
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "365",
        "--memory-max-bytes",
        &max_bytes,
    ]);
    let inserted = remember(&world, &incoming);
    assert_eq!(inserted["retention"]["capacity_evicted"], 1);
    let conn = Connection::open(&database).unwrap();
    assert!(
        !conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [oldest["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert!(
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
            [newer["event"]["id"].as_str().unwrap()],
            |row| row.get::<_, bool>(0),
        )
        .unwrap()
    );
    assert_eq!(
        conn.query_row(
            "SELECT event_count FROM memory_state WHERE id = 1",
            [],
            |row| { row.get::<_, i64>(0) }
        )
        .unwrap(),
        3
    );

    let blocked = TestWorld::new();
    let initialized = blocked.ok(&["init"]);
    let blocked_database = database_path(&initialized);
    let protected = remember(
        &blocked,
        &serde_json::json!({
            "type": "容量事件",
            "context": "唯一固定事件",
            "pinned": true,
            "observed": ["不能被容量策略删除"]
        }),
    );
    let max_bytes = (protected["event"]["logical_bytes"].as_u64().unwrap() + 1).to_string();
    blocked.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "365",
        "--memory-max-bytes",
        &max_bytes,
    ]);
    let before = complete_memory_snapshot(&blocked_database);
    let operations_before: i64 = Connection::open(&blocked_database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let raw = serde_json::to_string(&minimal_capsule("无法容纳的新事件", None)).unwrap();
    let error = blocked.err(&["remember", "--json", &raw]);
    assert_eq!(error["error"]["code"], "memory_capacity_exceeded");
    assert_eq!(complete_memory_snapshot(&blocked_database), before);
    assert_eq!(
        Connection::open(&blocked_database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        operations_before
    );
}

#[test]
fn unresolved_pinned_and_open_contradiction_events_are_protected() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "1000000",
    ]);
    let contradicted = remember(
        &world,
        &serde_json::json!({
            "type": "判断",
            "context": "旧容量判断",
            "occurred_at": "2000-01-01T00:00:00Z",
            "decision": ["容量足够"]
        }),
    );
    let contradiction = remember(
        &world,
        &serde_json::json!({
            "type": "判断",
            "context": "容量判断冲突",
            "occurred_at": "2000-01-02T00:00:00Z",
            "decision": ["容量不足"],
            "relations": [{
                "type": "contradicts",
                "target": contradicted["event"]["id"],
                "basis": "两次测量不一致"
            }]
        }),
    );
    let unresolved = remember(
        &world,
        &serde_json::json!({
            "type": "待办",
            "context": "长期未决问题",
            "occurred_at": "2000-01-03T00:00:00Z",
            "unresolved": ["仍需确认容量来源"]
        }),
    );
    let pinned = remember(
        &world,
        &serde_json::json!({
            "type": "证据",
            "context": "固定历史证据",
            "occurred_at": "2000-01-04T00:00:00Z",
            "pinned": true,
            "observed": ["人工确认必须保留"]
        }),
    );
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "1",
        "--memory-max-bytes",
        "1000000",
    ]);
    remember(&world, &minimal_capsule("触发保护检查", None));

    let conn = Connection::open(&database).unwrap();
    for event in [&contradicted, &contradiction, &unresolved, &pinned] {
        assert!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [event["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        );
    }
    drop(conn);

    let resolved = remember(
        &world,
        &serde_json::json!({
            "type": "结论",
            "context": "容量冲突已解决",
            "outcome": ["复测确认容量不足"],
            "relations": [{
                "type": "resolves",
                "target": contradiction["event"]["id"],
                "basis": "复测结果一致"
            }]
        }),
    );
    assert_eq!(resolved["retention"]["age_evicted"], 2);
    let conn = Connection::open(database).unwrap();
    for event in [&contradicted, &contradiction] {
        assert!(
            !conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                    [event["event"]["id"].as_str().unwrap()],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        );
    }
    for event in [&unresolved, &pinned, &resolved] {
        assert!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [event["event"]["id"].as_str().unwrap()],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
        );
    }
}

#[test]
fn exact_context_cluster_yields_a_candidate_without_merging_events() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let capsule = serde_json::json!({
        "type": "重复故障",
        "context": "支付回调超时",
        "observed": ["回调超过十秒"]
    });
    let mut ids = std::collections::BTreeSet::new();
    for index in 0..5 {
        let recorded = remember(&world, &capsule);
        ids.insert(recorded["event"]["id"].as_str().unwrap().to_owned());
        if index < 4 {
            assert!(recorded["hints"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(recorded["hints"].as_array().unwrap().len(), 1);
            assert_eq!(recorded["hints"][0]["type"], "exact-context-cluster");
        }
    }
    let cooled = remember(&world, &capsule);
    assert!(
        cooled["hints"]
            .as_array()
            .unwrap()
            .iter()
            .all(|hint| hint["type"] != "exact-context-cluster")
    );
    let similar = remember(
        &world,
        &serde_json::json!({
            "type": "重复故障",
            "context": "支付回调偶发超时",
            "observed": ["文字相似但不是同一上下文"]
        }),
    );
    assert!(
        similar["hints"]
            .as_array()
            .unwrap()
            .iter()
            .all(|hint| hint["type"] != "exact-context-cluster")
    );
    assert_eq!(ids.len(), 5);
    assert_eq!(
        Connection::open(database)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM memory_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn hints_are_bounded_cooled_down_and_pruned() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        "1000000",
    ]);
    let cluster = serde_json::json!({
        "type": "候选聚类",
        "context": "同一部署故障",
        "occurred_at": "2000-01-01T00:00:00Z",
        "observed": ["同一确定性模式"]
    });
    let mut target = Value::Null;
    for _ in 0..4 {
        target = remember(&world, &cluster);
    }
    remember(
        &world,
        &serde_json::json!({
            "type": "长期待办",
            "context": "过期未决候选",
            "occurred_at": "2000-01-02T00:00:00Z",
            "unresolved": ["需要长期复核"]
        }),
    );
    let target_id = target["event"]["id"].as_str().unwrap();
    assert!(
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [target_id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    let final_capsule = serde_json::json!({
        "type": "候选聚类",
        "context": "同一部署故障",
        "occurred_at": "2000-01-03T00:00:00Z",
        "observed": ["同一确定性模式"],
        "relations": [{
            "type": "supersedes",
            "target": target_id,
            "basis": "形成显式替代链"
        }]
    });
    let sizer = TestWorld::new();
    sizer.ok(&["init"]);
    sizer.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
    ]);
    let sizing_target = remember(&sizer, &cluster);
    let mut sizing_capsule = final_capsule.clone();
    sizing_capsule["relations"][0]["target"] = sizing_target["event"]["id"].clone();
    let final_bytes = remember(&sizer, &sizing_capsule)["event"]["logical_bytes"]
        .as_u64()
        .unwrap();
    let current_bytes = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT logical_bytes FROM memory_state WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let current_bytes = u64::try_from(current_bytes).unwrap();
    let max_bytes = ((current_bytes + final_bytes) * 100 / 85).to_string();
    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "50000",
        "--memory-max-bytes",
        &max_bytes,
    ]);
    assert!(
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_events WHERE id = ?1)",
                [target_id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    );
    assert_eq!(
        world.ok(&["memory", "show", target_id])["event"]["id"],
        target_id
    );
    let emitted = remember(&world, &final_capsule);
    assert_eq!(emitted["hints"].as_array().unwrap().len(), 3);
    let emitted_types = emitted["hints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hint| hint["type"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(emitted_types.len(), 3);
    let cooled = remember(&world, &cluster);
    assert!(
        cooled["hints"]
            .as_array()
            .unwrap()
            .iter()
            .all(|hint| { !emitted_types.contains(hint["type"].as_str().unwrap()) })
    );

    world.ok(&[
        "config",
        "set",
        "--memory",
        "enabled",
        "--memory-max-age-days",
        "1",
        "--memory-max-bytes",
        "1000000",
    ]);
    remember(&world, &minimal_capsule("触发自动候选清理", None));
    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM memory_hint_state
             WHERE hint_type IN ('exact-context-cluster', 'relation-review', 'storage-pressure')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

#[test]
fn relation_hints_distinguish_relation_types_and_clear_resolved_conflicts() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let target = remember(&world, &minimal_capsule("关系提示目标", None));
    let target_id = target["event"]["id"].as_str().unwrap();
    let superseding = remember(
        &world,
        &serde_json::json!({
            "type": "关系事件",
            "context": "显式替代关系",
            "decision": ["替代旧事件"],
            "relations": [{
                "type": "supersedes",
                "target": target_id,
                "basis": "替代关系提示"
            }]
        }),
    );
    let supersedes_key = superseding["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hint| hint["type"] == "relation-review")
        .unwrap()["candidate_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let contradiction = remember(
        &world,
        &serde_json::json!({
            "type": "关系事件",
            "context": "显式冲突关系",
            "decision": ["反驳旧事件"],
            "relations": [{
                "type": "contradicts",
                "target": target_id,
                "basis": "冲突关系提示"
            }]
        }),
    );
    let contradiction_key = contradiction["hints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hint| hint["type"] == "relation-review")
        .unwrap()["candidate_key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(supersedes_key, contradiction_key);

    remember(
        &world,
        &serde_json::json!({
            "type": "关系事件",
            "context": "冲突已经解决",
            "outcome": ["冲突复核完成"],
            "relations": [{
                "type": "resolves",
                "target": contradiction["event"]["id"],
                "basis": "验证后的最终结论"
            }]
        }),
    );
    world.ok(&["memory", "maintain"]);
    let conn = Connection::open(database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM memory_hint_state WHERE candidate_key = ?1",
            [&contradiction_key],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    assert_eq!(world.ok(&["memory", "status"])["pending_hints"], 1);
}

#[test]
fn memory_status_reports_pressure_outcomes_without_event_text() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let database = database_path(&initialized);
    let prior_capsule = serde_json::json!({
        "request_id": "status-retry",
        "type": "状态样本",
        "context": "状态输出绝不能泄露这段正文",
        "decision": ["旧状态"]
    });
    let prior = remember(&world, &prior_capsule);
    remember(&world, &prior_capsule);
    let replacement = remember(
        &world,
        &serde_json::json!({
            "type": "状态样本",
            "context": "状态统计替代事件",
            "pinned": true,
            "decision": ["新状态"],
            "relations": [{
                "type": "supersedes",
                "target": prior["event"]["id"],
                "basis": "状态统计测试"
            }]
        }),
    );
    world.ok(&[
        "memory",
        "feedback",
        replacement["event"]["id"].as_str().unwrap(),
        "--signal",
        "useful",
        "--reason",
        "验证状态计数",
    ]);
    let before = memory_read_snapshot(&database);
    let status = world.ok(&["memory", "status"]);
    assert_eq!(status["retained"]["events"], 2);
    assert_eq!(status["retained"]["protected"], 1);
    assert_eq!(status["retained"]["superseded"], 1);
    assert!(status["retained"]["logical_bytes"].as_u64().unwrap() > 0);
    assert_eq!(status["policy"]["max_age_days"], 365);
    assert_eq!(status["policy"]["max_bytes"], 268_435_456_u64);
    assert_eq!(status["counters"]["record_attempts"], 3);
    assert_eq!(status["counters"]["inserted_events"], 2);
    assert_eq!(status["counters"]["idempotent_replays"], 1);
    assert_eq!(status["counters"]["feedback_useful"], 1);
    assert_eq!(status["counters"]["feedback_not_useful"], 0);
    assert_eq!(status["counters"]["age_evictions"], 0);
    assert_eq!(status["counters"]["capacity_evictions"], 0);
    assert_eq!(status["pending_hints"], 1);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("状态输出绝不能泄露这段正文")
    );
    assert_eq!(memory_read_snapshot(&database), before);
}

#[test]
fn memory_maintain_rejects_scope_all_and_changesets() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    world.ok(&["--scope", "global", "init"]);
    let all = world.err(&["--scope", "all", "memory", "maintain"]);
    assert_eq!(all["error"]["code"], "scope_not_supported");

    world.ok(&["changeset", "begin", "memory-maintain"]);
    let staged = world.err(&["--changeset", "memory-maintain", "memory", "maintain"]);
    assert_eq!(staged["error"]["code"], "changeset_command_unsupported");
}
