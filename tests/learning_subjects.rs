use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn binary(plugin: &str) -> &'static str {
    match plugin {
        "tutor" => env!("CARGO_BIN_EXE_lwc-tutor"),
        "book" => env!("CARGO_BIN_EXE_lwc-book"),
        "practice" => env!("CARGO_BIN_EXE_lwc-practice"),
        _ => panic!("unknown plugin {plugin}"),
    }
}

fn output(plugin: &str, cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(binary(plugin))
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

fn ok(plugin: &str, cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = output(plugin, cwd, home, args);
    assert!(
        output.status.success(),
        "{plugin} {args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn err(plugin: &str, cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = output(plugin, cwd, home, args);
    assert!(
        !output.status.success(),
        "{plugin} {args:?} unexpectedly succeeded"
    );
    serde_json::from_slice(&output.stderr).unwrap()
}

fn database(home: &Path, plugin: &str) -> std::path::PathBuf {
    home.join(format!(".lwc/plugins/{plugin}/data.sqlite3"))
}

#[test]
fn every_plugin_owns_an_independent_idempotent_subject_store() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    let request = serde_json::json!({
        "name": "会计学",
        "parent_id": null,
        "tags": ["专业"],
        "request_id": "subject-create-accounting",
    })
    .to_string();
    let created = ok(
        "tutor",
        &cwd,
        &home,
        &["subject", "create", "--json", &request],
    );
    assert_eq!(created["schema_version"], 1);
    assert_eq!(created["plugin"], "tutor");
    assert_eq!(created["command"], "subject.create");
    assert_eq!(created["request_id"], "subject-create-accounting");
    assert_eq!(created["store"]["revision"], 1);
    assert_eq!(created["store"]["id"].as_str().unwrap().len(), 64);
    assert!(created["committed_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(created["result"]["subject"]["name"], "会计学");
    assert_eq!(created["result"]["subject"]["revision"], 1);
    let id = created["result"]["subject"]["id"].as_str().unwrap();
    assert!(
        id.len() >= 16
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'z').contains(&byte))
    );
    assert_eq!(
        ok(
            "tutor",
            &cwd,
            &home,
            &["subject", "create", "--json", &request]
        ),
        created
    );

    let changed_request = serde_json::json!({
        "name": "会计学修改",
        "tags": [],
        "request_id": "subject-create-accounting",
    })
    .to_string();
    assert_eq!(
        err(
            "tutor",
            &cwd,
            &home,
            &["subject", "create", "--json", &changed_request]
        )["error"]["code"],
        "request_id_reused"
    );

    for plugin in ["book", "practice"] {
        let ensured_request = serde_json::json!({
            "id": id,
            "name": "会计学",
            "parent_id": null,
            "tags": ["专业"],
            "request_id": format!("subject-ensure-{plugin}"),
        })
        .to_string();
        let ensured = ok(
            plugin,
            &cwd,
            &home,
            &["subject", "ensure", "--json", &ensured_request],
        );
        assert_eq!(ensured["result"]["subject"]["id"], id);
        assert_eq!(ensured["result"]["created"], true);
        assert_eq!(ensured["request_id"], format!("subject-ensure-{plugin}"));
        assert_eq!(ensured["store"]["revision"], 1);
        let ensured_again_request = serde_json::json!({
            "id": id,
            "name": "会计学",
            "parent_id": null,
            "tags": ["专业"],
            "request_id": format!("subject-ensure-again-{plugin}"),
        })
        .to_string();
        let ensured_again = ok(
            plugin,
            &cwd,
            &home,
            &["subject", "ensure", "--json", &ensured_again_request],
        );
        assert_eq!(ensured_again["result"]["created"], false);
        assert_eq!(ensured_again["store"]["revision"], 2);
        assert_eq!(
            ok(plugin, &cwd, &home, &["subject", "show", id])["result"]["subject"]["id"],
            id
        );
    }

    for plugin in ["tutor", "book", "practice"] {
        assert!(
            home.join(format!(".lwc/plugins/{plugin}/data.sqlite3"))
                .is_file()
        );
    }
}

#[test]
fn subject_rename_is_cas_guarded_and_ensure_never_title_matches() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let first = serde_json::json!({"name":"英语", "request_id":"english-a"}).to_string();
    let second = serde_json::json!({"name":"英语", "request_id":"english-b"}).to_string();
    let first = ok(
        "tutor",
        &cwd,
        &home,
        &["subject", "create", "--json", &first],
    );
    let second = ok(
        "tutor",
        &cwd,
        &home,
        &["subject", "create", "--json", &second],
    );
    let first_id = first["result"]["subject"]["id"].as_str().unwrap();
    let second_id = second["result"]["subject"]["id"].as_str().unwrap();
    assert_ne!(first_id, second_id, "equal titles must not merge identity");

    let rename = serde_json::json!({
        "name": "商务英语",
        "request_id": "english-rename",
    })
    .to_string();
    let renamed = ok(
        "tutor",
        &cwd,
        &home,
        &[
            "subject",
            "rename",
            first_id,
            "--if-revision",
            "1",
            "--json",
            &rename,
        ],
    );
    assert_eq!(renamed["result"]["subject"]["revision"], 2);
    assert_eq!(renamed["result"]["subject"]["name"], "商务英语");
    assert_eq!(renamed["request_id"], "english-rename");
    assert_eq!(renamed["store"]["revision"], 3);
    assert_eq!(
        ok(
            "tutor",
            &cwd,
            &home,
            &[
                "subject",
                "rename",
                first_id,
                "--if-revision",
                "1",
                "--json",
                &rename,
            ],
        ),
        renamed
    );
    let stale = serde_json::json!({
        "name": "过时修改",
        "request_id": "english-stale",
    })
    .to_string();
    assert_eq!(
        err(
            "tutor",
            &cwd,
            &home,
            &[
                "subject",
                "rename",
                first_id,
                "--if-revision",
                "1",
                "--json",
                &stale,
            ]
        )["error"]["code"],
        "revision_conflict"
    );
}

#[test]
fn subject_json_is_strict_utf8_bounded_and_path_relative_to_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(
        cwd.join("subject.json"),
        r#"{"name":"经济学","request_id":"economics","unknown":true}"#,
    )
    .unwrap();
    assert_eq!(
        err(
            "book",
            &cwd,
            &home,
            &["subject", "create", "--json", "@subject.json"]
        )["error"]["code"],
        "invalid_input"
    );

    fs::write(
        cwd.join("subject.json"),
        r#"{"name":"经济学","request_id":"economics"}"#,
    )
    .unwrap();
    assert_eq!(
        ok(
            "book",
            &cwd,
            &home,
            &["subject", "create", "--json", "@subject.json"]
        )["result"]["subject"]["name"],
        "经济学"
    );

    let oversized = cwd.join("oversized.json");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        err(
            "book",
            &cwd,
            &home,
            &["subject", "create", "--json", "@oversized.json"]
        )["error"]["code"],
        "input_too_large"
    );
}

#[test]
fn plugin_store_migrates_v1_and_rejects_future_or_corrupt_schema() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    ok("tutor", &cwd, &home, &["status"]);
    let tutor = database(&home, "tutor");
    let connection = Connection::open(&tutor).unwrap();
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS plugin_meta;
         DROP TABLE IF EXISTS sync_receipts;
         PRAGMA user_version=1;",
        )
        .unwrap();
    drop(connection);
    ok("tutor", &cwd, &home, &["status"]);
    let connection = Connection::open(&tutor).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    for table in ["plugin_meta", "sync_receipts"] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "missing migrated {table}"
        );
    }
    drop(connection);

    ok("book", &cwd, &home, &["status"]);
    let book = database(&home, "book");
    Connection::open(&book)
        .unwrap()
        .execute_batch("PRAGMA user_version=99;")
        .unwrap();
    assert_eq!(
        err("book", &cwd, &home, &["status"])["error"]["code"],
        "unsupported_plugin_schema"
    );
    assert_eq!(
        Connection::open(&book)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        99
    );

    ok("practice", &cwd, &home, &["status"]);
    let practice = database(&home, "practice");
    let connection = Connection::open(&practice).unwrap();
    connection
        .execute_batch(
            "DROP TABLE requests;
             DROP TABLE plugin_meta;
             DROP TABLE sync_receipts;
             PRAGMA user_version=1;",
        )
        .unwrap();
    let objects_before = connection
        .prepare(
            "SELECT type,name,sql FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    drop(connection);
    assert_eq!(
        err("practice", &cwd, &home, &["status"])["error"]["code"],
        "corrupt_store"
    );
    let connection = Connection::open(&practice).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let objects_after = connection
        .prepare(
            "SELECT type,name,sql FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(objects_after, objects_before);
}

#[test]
fn plugin_store_identity_is_independent_revisioned_and_receipt_ready() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let mut identities = std::collections::BTreeSet::new();

    for plugin in ["tutor", "book", "practice"] {
        ok(plugin, &cwd, &home, &["status"]);
        // The second open validates that every table created by the domain is in
        // the fixed canonical or exact derived-table inventory.
        ok(plugin, &cwd, &home, &["status"]);
        let connection = Connection::open(database(&home, plugin)).unwrap();
        let store_id: String = connection
            .query_row(
                "SELECT value FROM plugin_meta WHERE key='store_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(store_id.len(), 64);
        assert!(
            store_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert!(
            identities.insert(store_id),
            "plugin stores shared an identity"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM plugin_meta WHERE key='revision'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "0"
        );
        let columns = connection
            .prepare("PRAGMA table_info(sync_receipts)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            columns,
            [
                "session_id",
                "plugin_id",
                "store_id",
                "source_revision",
                "resolved_revision",
                "logical_hash",
                "completed_at",
                "runtime_state",
                "state",
                "receipt_hash",
            ]
        );
    }

    let request = serde_json::json!({"name":"数学", "request_id":"math-create"}).to_string();
    ok(
        "tutor",
        &cwd,
        &home,
        &["subject", "create", "--json", &request],
    );
    let connection = Connection::open(database(&home, "tutor")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM plugin_meta WHERE key='revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "1"
    );

    for plugin in ["tutor", "book", "practice"] {
        let connection = Connection::open(database(&home, plugin)).unwrap();
        connection
            .execute_batch("CREATE TABLE unregistered_projection(value TEXT);")
            .unwrap();
        assert_eq!(
            err(plugin, &cwd, &home, &["status"])["error"]["code"],
            "corrupt_store",
            "{plugin} silently accepted a table outside its fixed inventory"
        );
        let canonical = match plugin {
            "tutor" => "tutor_diagnoses",
            "book" => "book_anomalies",
            "practice" => "requests",
            _ => unreachable!(),
        };
        connection
            .execute_batch(&format!(
                "DROP TABLE unregistered_projection; DROP TABLE {canonical};"
            ))
            .unwrap();
        drop(connection);
        assert_eq!(
            err(plugin, &cwd, &home, &["status"])["error"]["code"],
            "corrupt_store",
            "{plugin} silently skipped missing canonical table {canonical}"
        );
    }
}

#[cfg(unix)]
#[test]
fn fresh_plugin_store_uses_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    ok("tutor", &cwd, &home, &["status"]);
    let root = home.join(".lwc/plugins/tutor");
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(database(&home, "tutor"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
