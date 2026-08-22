use rusqlite::Connection;
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

#[test]
fn version_14_store_migrates_todo_and_plan_schemas_through_v16() {
    let w = World::new();
    let db = w.project.join(".lwc/wiki.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE IF EXISTS todo_fts; DROP TABLE IF EXISTS todo_fts_data; DROP TABLE IF EXISTS todo_fts_idx; DROP TABLE IF EXISTS todo_fts_content; DROP TABLE IF EXISTS todo_fts_docsize; DROP TABLE IF EXISTS todo_fts_config; DROP TABLE IF EXISTS todo_tags; DROP TABLE IF EXISTS todo_items; DROP TABLE IF EXISTS plan_fts; DROP TABLE IF EXISTS plan_fts_data; DROP TABLE IF EXISTS plan_fts_idx; DROP TABLE IF EXISTS plan_fts_content; DROP TABLE IF EXISTS plan_fts_docsize; DROP TABLE IF EXISTS plan_fts_config; DROP TABLE IF EXISTS plan_history; DROP TABLE IF EXISTS plan_steps; DROP TABLE IF EXISTS plan_constraints; DROP TABLE IF EXISTS plan_tags; DROP TABLE IF EXISTS plans; UPDATE meta SET value='14' WHERE key='format_version'; PRAGMA user_version=14; PRAGMA foreign_keys=ON;").unwrap();
    drop(conn);
    assert_eq!(w.ok(&["todo", "list"])["returned"], 0);
    let conn = Connection::open(db).unwrap();
    assert_eq!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        16
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN ('todo_items','plans','plan_steps')",[],|r|r.get::<_,i64>(0)).unwrap(),3);
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_table_info('todo_items') WHERE name IN ('parent_id','target_at')",[],|r|r.get::<_,i64>(0)).unwrap(),2);
}

#[test]
fn failed_v15_migration_keeps_v14_and_can_retry() {
    let w = World::new();
    let db = w.project.join(".lwc/wiki.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE todo_fts; DROP TABLE IF EXISTS todo_fts_data; DROP TABLE IF EXISTS todo_fts_idx; DROP TABLE IF EXISTS todo_fts_content; DROP TABLE IF EXISTS todo_fts_docsize; DROP TABLE IF EXISTS todo_fts_config; DROP TABLE todo_tags; DROP TABLE todo_items; DROP TABLE plan_fts; DROP TABLE IF EXISTS plan_fts_data; DROP TABLE IF EXISTS plan_fts_idx; DROP TABLE IF EXISTS plan_fts_content; DROP TABLE IF EXISTS plan_fts_docsize; DROP TABLE IF EXISTS plan_fts_config; DROP TABLE plan_history; DROP TABLE plan_steps; DROP TABLE plan_constraints; DROP TABLE plan_tags; DROP TABLE plans; CREATE TABLE plans(broken TEXT); UPDATE meta SET value='14' WHERE key='format_version'; PRAGMA user_version=14; PRAGMA foreign_keys=ON;").unwrap();
    drop(conn);
    assert_eq!(
        w.error(&["todo", "list"])["error"]["code"],
        "store_migration_failed"
    );
    let conn = Connection::open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        14
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('plans') WHERE name='broken'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    conn.execute("DROP TABLE plans", []).unwrap();
    drop(conn);
    assert_eq!(w.ok(&["todo", "list"])["returned"], 0);
}

#[test]
fn version_15_store_adds_todo_parent_and_target_columns() {
    let w = World::new();
    let db = w.project.join(".lwc/wiki.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP INDEX todo_items_parent;
         DROP INDEX todo_items_reminder;
         ALTER TABLE todo_items DROP COLUMN parent_id;
         ALTER TABLE todo_items DROP COLUMN target_at;
         UPDATE meta SET value='15' WHERE key='format_version';
         PRAGMA user_version=15;",
    )
    .unwrap();
    drop(conn);

    assert_eq!(w.ok(&["todo", "list"])["returned"], 0);
    let conn = Connection::open(db).unwrap();
    assert_eq!(
        conn.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        16
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('todo_items') WHERE name IN ('parent_id','target_at')",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}

impl World {
    fn new() -> Self {
        let world = Self::new_disabled();
        world.ok(&["config", "set", "--todo", "enabled", "--plan", "enabled"]);
        world
    }
    fn new_disabled() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let world = Self {
            _temp: temp,
            project,
            home,
        };
        world.ok(&["init"]);
        world
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
        let out = self.output(args);
        assert!(
            out.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn error(&self, args: &[&str]) -> Value {
        let out = self.output(args);
        assert!(!out.status.success(), "expected failure for {args:?}");
        serde_json::from_slice(&out.stderr).unwrap()
    }
}

#[test]
fn todo_and_plan_capabilities_are_opt_in_and_layered() {
    let w = World::new_disabled();
    let defaults = w.ok(&["config", "show"]);
    assert_eq!(defaults["todo"]["setting"], "disabled");
    assert_eq!(defaults["todo"]["origin"], "built-in");
    assert_eq!(defaults["plan"]["setting"], "disabled");
    assert_eq!(
        w.error(&["todo", "add", "disabled"])["error"]["code"],
        "todo_disabled"
    );
    assert_eq!(
        w.error(&["plan", "current"])["error"]["code"],
        "plan_disabled"
    );

    w.ok(&["--scope", "global", "init"]);
    w.ok(&[
        "--scope", "global", "config", "set", "--todo", "enabled", "--plan", "enabled",
    ]);
    let inherited = w.ok(&["config", "show"]);
    assert_eq!(inherited["todo"]["setting"], "enabled");
    assert_eq!(inherited["todo"]["origin"], "global");
    assert_eq!(inherited["plan"]["setting"], "enabled");
    assert_eq!(w.ok(&["todo", "add", "inherited"])["action"], "created");
    w.ok(&["--scope", "global", "todo", "add", "global enabled"]);

    w.ok(&["config", "set", "--todo", "disabled"]);
    assert_eq!(w.error(&["todo", "list"])["error"]["code"], "todo_disabled");
    let all = w.ok(&["--scope", "all", "todo", "list"]);
    assert_eq!(all["returned"], 1);
    assert_eq!(all["todos"][0]["title"], "global enabled");
    w.ok(&["config", "set", "--todo", "inherit", "--plan", "inherit"]);
    let explicitly_inherited = w.ok(&["config", "show"]);
    assert_eq!(explicitly_inherited["todo"]["origin"], "global");
    assert_eq!(explicitly_inherited["plan"]["origin"], "global");
    w.ok(&["config", "unset", "--todo", "--plan"]);
    assert_eq!(w.ok(&["todo", "list"])["returned"], 1);
}

#[test]
fn todo_lifecycle_search_and_revision_conflict() {
    let w = World::new();
    let created = w.ok(&[
        "todo",
        "add",
        "发布前验证 crates 包",
        "--tag",
        "release",
        "--cue",
        "准备发布版本时",
        "--detail",
        "运行 dry-run",
    ]);
    assert_eq!(created["action"], "created");
    let todo = &created["todo"];
    assert_eq!(todo["state"], "open");
    assert_eq!(todo["revision"], 1);
    assert_eq!(todo["id"].as_str().unwrap().len(), 32);
    let id = todo["id"].as_str().unwrap();

    let found = w.ok(&["todo", "search", "发布 crates", "--tag", "release"]);
    assert_eq!(found["returned"], 1);
    assert_eq!(found["todos"][0]["id"], id);

    let updated = w.ok(&[
        "todo",
        "update",
        id,
        "--if-revision",
        "1",
        "--title",
        "发布前验证包",
        "--add-tag",
        "验收",
    ]);
    assert_eq!(updated["todo"]["revision"], 2);
    assert_eq!(
        w.error(&["todo", "done", id, "--if-revision", "1", "--result", "完成"])["error"]["code"],
        "todo_revision_conflict"
    );
    let done = w.ok(&["todo", "done", id, "--if-revision", "2", "--result", "完成"]);
    assert_eq!(done["todo"]["state"], "done");
    assert_eq!(done["todo"]["revision"], 3);
    assert_eq!(
        w.ok(&["todo", "done", id, "--if-revision", "2", "--result", "完成"])["action"],
        "unchanged"
    );
    assert_eq!(
        w.error(&["todo", "update", id, "--if-revision", "3", "--title", "no"])["error"]["code"],
        "invalid_todo_transition"
    );
    assert_eq!(
        w.ok(&["todo", "reopen", id, "--if-revision", "3"])["todo"]["state"],
        "open"
    );
}

#[test]
fn todo_request_id_scope_and_changeset_contracts() {
    let w = World::new();
    let first = w.ok(&["todo", "add", "later", "--request-id", "req-1"]);
    assert_eq!(
        w.ok(&["todo", "add", "later", "--request-id", "req-1"])["action"],
        "unchanged"
    );
    assert_eq!(
        w.error(&["todo", "add", "different", "--request-id", "req-1"])["error"]["code"],
        "todo_request_conflict"
    );
    let id = first["todo"]["id"].as_str().unwrap();
    assert_eq!(w.ok(&["todo", "show", id])["todo"]["id"], id);
    assert_eq!(
        w.error(&["--scope", "all", "todo", "show", id])["error"]["code"],
        "scope_not_supported"
    );
    assert_eq!(
        w.error(&["--changeset", "draft", "todo", "list"])["error"]["code"],
        "changeset_command_unsupported"
    );
}

#[test]
fn todo_json_cancel_and_all_scope_reads_work() {
    let w = World::new();
    w.ok(&["--scope", "global", "init"]);
    w.ok(&["--scope", "global", "config", "set", "--todo", "enabled"]);
    let raw = r#"{"title":"全局待办","tags":["跨域"],"cue":"以后","detail":null,"request_id":"global-1"}"#;
    let created = w.ok(&["--scope", "global", "todo", "add", "--json", raw]);
    let id = created["todo"]["id"].as_str().unwrap();
    assert_eq!(
        w.ok(&["--scope", "all", "todo", "search", "全局", "--tag", "跨域"])["returned"],
        1
    );
    let cancelled = w.ok(&[
        "--scope",
        "global",
        "todo",
        "cancel",
        id,
        "--if-revision",
        "1",
        "--reason",
        "obsolete",
    ]);
    assert_eq!(cancelled["todo"]["state"], "cancelled");
    assert_eq!(
        w.ok(&["--scope", "global", "todo", "list", "--state", "cancelled"])["returned"],
        1
    );
}

#[test]
fn todo_children_and_target_time_are_queryable_and_mutable() {
    let w = World::new();
    let parent = w.ok(&["todo", "add", "parent"]);
    let parent_id = parent["todo"]["id"].as_str().unwrap();
    let child = w.ok(&[
        "todo",
        "add",
        "child",
        "--parent",
        parent_id,
        "--target-at",
        "2030-01-02T03:04:05+08:00",
    ]);
    let child_id = child["todo"]["id"].as_str().unwrap();
    assert_eq!(child["todo"]["parent_id"], parent_id);
    assert_eq!(child["todo"]["target_at"], "2030-01-01T19:04:05.000Z");

    let shown = w.ok(&["todo", "show", parent_id]);
    assert_eq!(shown["todo"]["children"][0]["id"], child_id);
    let listed = w.ok(&["todo", "list", "--parent", parent_id]);
    assert_eq!(listed["returned"], 1);
    assert_eq!(listed["todos"][0]["id"], child_id);

    let updated = w.ok(&[
        "todo",
        "update",
        child_id,
        "--if-revision",
        "1",
        "--target-at",
        "2031-01-01T00:00:00Z",
    ]);
    assert_eq!(updated["todo"]["target_at"], "2031-01-01T00:00:00.000Z");
    let cleared = w.ok(&[
        "todo",
        "update",
        child_id,
        "--if-revision",
        "2",
        "--clear-target-at",
    ]);
    assert!(cleared["todo"]["target_at"].is_null());
    assert_eq!(
        w.error(&[
            "todo",
            "add",
            "orphan",
            "--parent",
            "00000000000000000000000000000000",
        ])["error"]["code"],
        "todo_parent_not_found"
    );
    assert_eq!(
        w.error(&["todo", "add", "bad time", "--target-at", "tomorrow"])["error"]["code"],
        "invalid_todo_target_at"
    );
}
