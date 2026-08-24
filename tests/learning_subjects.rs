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
            "--name",
            "商务英语",
        ],
    );
    assert_eq!(renamed["result"]["subject"]["revision"], 2);
    assert_eq!(renamed["result"]["subject"]["name"], "商务英语");
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
                "--name",
                "过时修改",
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
