#![cfg(unix)]

use rusqlite::{Connection, types::ValueRef};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

const SUBJECT_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const TUTOR_TABLES: &[&str] = &[
    "subjects",
    "subject_name_history",
    "requests",
    "tutor_sessions",
    "tutor_diagnoses",
    "tutor_turns",
    "tutor_turn_owner_history",
    "soul_versions",
    "soul_settings",
    "soul_settings_history",
    "soul_proposals",
    "learner_facts",
    "learner_fact_history",
    "tutor_goals",
    "tutor_goal_criteria",
    "tutor_goal_evidence",
    "tutor_goal_history",
    "tutor_plans",
    "tutor_plan_versions",
    "tutor_plan_steps",
    "tutor_plan_step_history",
];
const BOOK_TABLES: &[&str] = &[
    "subjects",
    "subject_name_history",
    "requests",
    "books",
    "book_blocks",
    "book_anomalies",
    "book_cursors",
    "book_leases",
    "book_lease_owner_history",
    "book_window_reports",
    "book_syntheses",
    "book_summaries",
    "book_mainline",
    "book_relations",
];
const PRACTICE_TABLES: &[&str] = &[
    "subjects",
    "subject_name_history",
    "requests",
    "practice_banks",
    "practice_items",
    "bank_items",
    "practice_sets",
    "set_members",
    "set_member_events",
    "papers",
    "paper_items",
    "attempts",
    "attempt_takeover_history",
    "responses",
    "response_history",
    "grades",
    "grade_history",
    "review_controls",
    "review_events",
    "fsrs_cards",
    "review_debt",
    "review_debt_events",
];

struct World {
    _temp: TempDir,
    local: PathBuf,
    remote: PathBuf,
    local_home: PathBuf,
    remote_home: PathBuf,
    bin: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let world = Self {
            local: temp.path().join("local"),
            remote: temp.path().join("remote"),
            local_home: temp.path().join("local-home"),
            remote_home: temp.path().join("remote-home"),
            bin: temp.path().join("bin"),
            _temp: temp,
        };
        for path in [
            &world.local,
            &world.remote,
            &world.local_home,
            &world.remote_home,
            &world.bin,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        world.lwc_ok(&world.local, &world.local_home, &["init"]);
        world.lwc_ok(&world.remote, &world.remote_home, &["init"]);
        world.install_fake_ssh();
        world
    }

    fn lwc(&self, cwd: &Path, home: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", home)
            .env("LWC_PROJECT_ROOT", cwd)
            .args(args)
            .output()
            .unwrap()
    }

    fn lwc_ok(&self, cwd: &Path, home: &Path, args: &[&str]) -> Value {
        let output = self.lwc(cwd, home, args);
        assert!(
            output.status.success(),
            "lwc {args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn plugin(&self, plugin: &str, cwd: &Path, home: &Path, args: &[&str]) -> Output {
        let binary = match plugin {
            "tutor" => env!("CARGO_BIN_EXE_lwc-tutor"),
            "book" => env!("CARGO_BIN_EXE_lwc-book"),
            "practice" => env!("CARGO_BIN_EXE_lwc-practice"),
            _ => panic!("unknown plugin"),
        };
        Command::new(binary)
            .current_dir(cwd)
            .env("HOME", home)
            .args(args)
            .output()
            .unwrap()
    }

    fn plugin_ok(&self, plugin: &str, cwd: &Path, home: &Path, args: &[&str]) -> Value {
        let output = self.plugin(plugin, cwd, home, args);
        assert!(
            output.status.success(),
            "{plugin} {args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn install_fake_ssh(&self) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.bin.join("ssh");
        fs::write(
            &path,
            r#"#!/bin/sh
[ "$1" = "-G" ] && exit 0
export HOME="$LWC_REMOTE_HOME"
last=""
for arg in "$@"; do last=$arg; done
case "$last" in
  git-upload-pack*|git-receive-pack*) exec /bin/sh -c "$last" ;;
esac
shift
shift
shift
first=""
IFS= read -r first || exit 91
case "$first" in
  *'"action":"plugin-publish"'*'"plugin_id":"'"$LWC_FAIL_PLUGIN"'"'*) exit 97 ;;
esac
{ printf '%s\n' "$first"; cat; } | exec "$LWC_TEST_BINARY" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn sync(&self, mode: &str, fail_plugin: Option<&str>, resume: Option<&str>) -> Output {
        let mut args = vec![
            "sync",
            "peer",
            self.remote.to_str().unwrap(),
            "--mode",
            mode,
        ];
        if let Some(session) = resume {
            args.extend(["--resume", session]);
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
        command
            .current_dir(&self.local)
            .env("HOME", &self.local_home)
            .env("LWC_PROJECT_ROOT", &self.local)
            .env("LWC_TEST_BINARY", env!("CARGO_BIN_EXE_lwc"))
            .env("LWC_REMOTE_HOME", &self.remote_home)
            .env("LWC_FAIL_PLUGIN", fail_plugin.unwrap_or(""))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.output().unwrap()
    }

    fn install_remote_runtimes(&self) {
        self.install_runtimes(&self.remote, &self.remote_home);
    }

    fn install_runtimes(&self, cwd: &Path, home: &Path) {
        for plugin in ["tutor", "book", "practice"] {
            install_runtime(
                home,
                plugin,
                match plugin {
                    "tutor" => Path::new(env!("CARGO_BIN_EXE_lwc-tutor")),
                    "book" => Path::new(env!("CARGO_BIN_EXE_lwc-book")),
                    _ => Path::new(env!("CARGO_BIN_EXE_lwc-practice")),
                },
            );
        }
        self.lwc_ok(cwd, home, &["--scope", "global", "init"]);
        for plugin in ["tutor", "book", "practice"] {
            let flag = format!("--{plugin}");
            self.lwc_ok(
                cwd,
                home,
                &["--scope", "global", "config", "set", &flag, "enabled"],
            );
        }
    }
}

fn install_runtime(home: &Path, plugin: &str, binary: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let target = format!("{}-apple-darwin", std::env::consts::ARCH);
    let version = env!("CARGO_PKG_VERSION");
    let root = home
        .join(".lwc/runtime")
        .join(plugin)
        .join(version)
        .join(&target);
    fs::create_dir_all(&root).unwrap();
    let name = format!("lwc-{plugin}");
    let installed = root.join(&name);
    fs::copy(binary, &installed).unwrap();
    fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
    let sha = hex(Sha256::digest(fs::read(&installed).unwrap()).as_slice());
    fs::write(
        root.join("runtime.json"),
        serde_json::to_vec_pretty(&json!({
            "plugin":plugin, "version":version, "target":target,
            "asset":format!("lwc-{plugin}-{version}-{target}.tar.gz"),
            "sha256":sha, "binary":name
        }))
        .unwrap(),
    )
    .unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn ensure_subject(world: &World, home: &Path, cwd: &Path, plugin: &str, suffix: &str) {
    let input = json!({
        "id":SUBJECT_ID, "name":"Sync subject", "request_id":format!("{suffix}-{plugin}-subject")
    })
    .to_string();
    world.plugin_ok(plugin, cwd, home, &["subject", "ensure", "--json", &input]);
}

struct ActiveIds {
    tutor_turn: String,
    book_lease: String,
    practice_attempt: String,
}

fn seed_small(world: &World) -> ActiveIds {
    for plugin in ["tutor", "book", "practice"] {
        ensure_subject(world, &world.local_home, &world.local, plugin, "small");
    }
    let session = world.plugin_ok(
        "tutor",
        &world.local,
        &world.local_home,
        &[
            "session",
            "create",
            "--json",
            &json!({"subject_id":SUBJECT_ID,"mode":"learning","request_id":"small-session"})
                .to_string(),
        ],
    );
    let pending = world.plugin_ok(
        "tutor",
        &world.local,
        &world.local_home,
        &[
            "turn",
            "begin",
            "--json",
            &json!({
                "session_id":session["result"]["session"]["id"], "owner":"old-tutor",
                "input":"pending takeover canary", "request_id":"small-pending-turn"
            })
            .to_string(),
        ],
    );
    let evidence = world.plugin_ok(
        "tutor",
        &world.local,
        &world.local_home,
        &[
            "turn",
            "begin",
            "--json",
            &json!({
                "session_id":session["result"]["session"]["id"], "owner":"evidence-owner",
                "input":"committed evidence canary", "request_id":"small-evidence-turn"
            })
            .to_string(),
        ],
    );
    let evidence_id = evidence["result"]["turn"]["id"].as_str().unwrap();
    world.plugin_ok(
        "tutor",
        &world.local,
        &world.local_home,
        &[
            "turn",
            "commit",
            evidence_id,
            "--if-revision",
            "1",
            "--json",
            &json!({
                "owner":"evidence-owner","reply":"exact committed evidence",
                "checkpoint":{"kind":"progress","blocked_by":null,"hint_level":0},
                "request_id":"small-evidence-commit"
            })
            .to_string(),
        ],
    );
    fs::write(
        world.local.join("small-sync.md"),
        "# First\n\nSmall canonical blob.\n",
    )
    .unwrap();
    let imported = world.plugin_ok(
        "book",
        &world.local,
        &world.local_home,
        &[
            "import",
            "--json",
            &json!({
                "subject_id":SUBJECT_ID,"path":"small-sync.md","title":"Small Sync",
                "request_id":"small-book-import"
            })
            .to_string(),
        ],
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    world.plugin_ok(
        "book",
        &world.local,
        &world.local_home,
        &[
            "prepare",
            book_id,
            "--if-revision",
            "1",
            "--json",
            &json!({"request_id":"small-book-prepare"}).to_string(),
        ],
    );
    let lease = world.plugin_ok(
        "book",
        &world.local,
        &world.local_home,
        &[
            "read",
            "next",
            book_id,
            "--if-revision",
            "2",
            "--json",
            &json!({"owner":"old-book","request_id":"small-book-lease"}).to_string(),
        ],
    );
    let bank = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &[
            "bank",
            "create",
            "--json",
            &json!({
                "subject_id":SUBJECT_ID,"key":format!("subject:{SUBJECT_ID}"),"title":"Mixed",
                "source":{"kind":"subject","id":SUBJECT_ID,"revision_or_hash":"1","subject_id":SUBJECT_ID},
                "request_id":"small-practice-bank"
            })
            .to_string(),
        ],
    );
    let item_input = json!({
        "subject_id":SUBJECT_ID,"item_type":"choice","grading_kind":"objective",
        "prompt":"Evidence canary?","answer":"A","rubric":null,"max_points":1.0,
        "estimated_minutes":1,"difficulty":0.5,"topic":"sync",
        "source":{"kind":"tutor_turn","id":evidence_id,"revision_or_hash":"2","subject_id":SUBJECT_ID},
        "request_id":"small-practice-item"
    });
    let item = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &["item", "create", "--json", &item_input.to_string()],
    );
    let item_id = item["result"]["item"]["id"].as_str().unwrap();
    let verified = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &[
            "item",
            "verify",
            item_id,
            "--if-revision",
            "1",
            "--json",
            &json!({
                "prompt":item_input["prompt"],"answer":item_input["answer"],"rubric":null,
                "source":item_input["source"],"request_id":"small-practice-verify"
            })
            .to_string(),
        ],
    );
    let bank = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &[
            "bank",
            "add",
            bank["result"]["bank"]["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &json!({
                "item_id":item_id,"item_revision":verified["result"]["item"]["revision"],
                "request_id":"small-practice-bank-add"
            })
            .to_string(),
        ],
    );
    let paper = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &[
            "paper",
            "create",
            "--json",
            &json!({
                "bank_id":bank["result"]["bank"]["id"],"count":1,"duration_minutes":2,
                "total_points":1.0,"request_id":"small-practice-paper"
            })
            .to_string(),
        ],
    );
    let attempt = world.plugin_ok(
        "practice",
        &world.local,
        &world.local_home,
        &[
            "attempt",
            "create",
            "--json",
            &json!({
                "paper_id":paper["result"]["paper"]["id"],"owner":"old-practice",
                "request_id":"small-practice-attempt"
            })
            .to_string(),
        ],
    );
    ActiveIds {
        tutor_turn: pending["result"]["turn"]["id"].as_str().unwrap().to_owned(),
        book_lease: lease["result"]["lease"]["id"].as_str().unwrap().to_owned(),
        practice_attempt: attempt["result"]["attempt"]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
    }
}

fn database(home: &Path, plugin: &str) -> PathBuf {
    home.join(".lwc/plugins").join(plugin).join("data.sqlite3")
}

fn canonical_snapshot(database: &Path, tables: &[&str]) -> BTreeMap<String, Vec<String>> {
    let connection =
        Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    tables
        .iter()
        .map(|table| {
            let columns = connection
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let mut primary = columns
                .into_iter()
                .filter(|(_, ordinal)| *ordinal > 0)
                .collect::<Vec<_>>();
            primary.sort_by_key(|(_, ordinal)| *ordinal);
            assert!(!primary.is_empty(), "{table} lacks a primary key");
            let selected = primary
                .iter()
                .map(|(name, _)| format!("\"{}\"", name.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = connection
                .prepare(&format!(
                    "SELECT {selected} FROM \"{table}\" ORDER BY {selected}"
                ))
                .unwrap();
            let column_count = primary.len();
            let keys = statement
                .query_map([], |row| {
                    let mut key = Vec::new();
                    for index in 0..column_count {
                        let value = match row.get_ref(index)? {
                            ValueRef::Null => "null".to_owned(),
                            ValueRef::Integer(value) => format!("i:{value}"),
                            ValueRef::Real(value) => format!("r:{:016x}", value.to_bits()),
                            ValueRef::Text(value) => format!("t:{}", hex(value)),
                            ValueRef::Blob(value) => format!("b:{}", hex(value)),
                        };
                        key.push(value);
                    }
                    Ok(key.join("|"))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            ((*table).to_owned(), keys)
        })
        .collect()
}

fn plugin_tables(plugin: &str) -> &'static [&'static str] {
    match plugin {
        "tutor" => TUTOR_TABLES,
        "book" => BOOK_TABLES,
        "practice" => PRACTICE_TABLES,
        _ => unreachable!(),
    }
}

#[test]
fn small_three_plugin_sync_preserves_every_category_blob_hash_and_ready_receipt() {
    let world = World::new();
    seed_small(&world);
    let before = ["tutor", "book", "practice"]
        .into_iter()
        .map(|plugin| {
            (
                plugin,
                canonical_snapshot(&database(&world.local_home, plugin), plugin_tables(plugin)),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let first = world.sync("merge", None, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    world.install_remote_runtimes();
    let second = world.sync("merge", None, None);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let result: Value = serde_json::from_slice(&second.stdout).unwrap();
    let session = result["session_id"].as_str().unwrap();

    for plugin in ["tutor", "book", "practice"] {
        let remote_db = database(&world.remote_home, plugin);
        assert_eq!(
            canonical_snapshot(&remote_db, plugin_tables(plugin)),
            before[plugin],
            "{plugin} canonical category IDs changed"
        );
        let receipt = Connection::open(&remote_db)
            .unwrap()
            .query_row(
                "SELECT logical_hash,runtime_state,state FROM sync_receipts WHERE session_id=?1",
                [session],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        let manifest: Value = serde_json::from_slice(
            &fs::read(preserved_manifest(&world.remote_home, plugin)).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.0, manifest["logical_hash"]);
        assert_eq!(
            (receipt.1.as_str(), receipt.2.as_str()),
            ("ready", "completed")
        );
        let report = result["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|unit| unit["plugin"] == plugin)
            .unwrap();
        assert!(
            report["record_counts"].is_object(),
            "{plugin} lacks per-category receipt: {report}"
        );
        assert!(
            report["blob_hashes"].is_array(),
            "{plugin} lacks blob receipt: {report}"
        );
        assert!(report["publication"].is_object() && report["rebuild"].is_object());
    }
    let source_blobs = blob_hashes(&world.local_home.join(".lwc/plugins/book/blobs"));
    let remote_blobs = blob_hashes(&world.remote_home.join(".lwc/plugins/book/blobs"));
    assert_eq!(remote_blobs, source_blobs);
}

fn preserved_manifest(home: &Path, plugin: &str) -> PathBuf {
    let preserved = home.join(".lwc/plugins").join(plugin).join("preserved");
    for store in fs::read_dir(preserved).unwrap() {
        for revision in fs::read_dir(store.unwrap().path()).unwrap() {
            let manifest = revision.unwrap().path().join("manifest.json");
            if manifest.is_file() {
                return manifest;
            }
        }
    }
    panic!("{plugin} preserved manifest missing")
}

#[test]
fn latest_ready_receipt_gates_one_takeover_per_plugin_and_rejects_old_owner() {
    let world = World::new();
    let ids = seed_small(&world);
    let first = world.sync("merge", None, None);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    world.install_remote_runtimes();
    let second = world.sync("merge", None, None);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let receipt: Value = serde_json::from_slice(&second.stdout).unwrap();
    let session = receipt["session_id"].as_str().unwrap();

    for (plugin, command, entity, old_owner, new_owner, request, stale_code) in [
        (
            "tutor",
            &["turn", "takeover"][..],
            ids.tutor_turn.as_str(),
            "old-tutor",
            "new-tutor",
            "takeover-tutor",
            "stale_sync_receipt",
        ),
        (
            "book",
            &["read", "takeover"][..],
            ids.book_lease.as_str(),
            "old-book",
            "new-book",
            "takeover-book",
            "stale_sync_receipt",
        ),
        (
            "practice",
            &["attempt", "takeover"][..],
            ids.practice_attempt.as_str(),
            "old-practice",
            "new-practice",
            "takeover-practice",
            "stale_owner",
        ),
    ] {
        let body = json!({
            "entity_id":entity,"old_owner":old_owner,"new_owner":new_owner,
            "if_revision":1,"sync_session_id":session,"request_id":request
        })
        .to_string();
        let mut args = command.to_vec();
        args.extend(["--json", &body]);
        world.plugin_ok(plugin, &world.remote, &world.remote_home, &args);
        let stale = json!({
            "entity_id":entity,"old_owner":old_owner,"new_owner":"another-owner",
            "if_revision":2,"sync_session_id":session,"request_id":format!("{request}-stale")
        })
        .to_string();
        let mut stale_args = command.to_vec();
        stale_args.extend(["--json", &stale]);
        let rejected = world.plugin(plugin, &world.remote, &world.remote_home, &stale_args);
        assert!(
            !rejected.status.success(),
            "{plugin} old owner took over twice"
        );
        let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
        assert_eq!(error["error"]["code"], stale_code, "{plugin}: {error}");
    }
}

fn blob_hashes(root: &Path) -> Vec<String> {
    fn visit(root: &Path, output: &mut Vec<String>) {
        if !root.exists() {
            return;
        }
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(&entry.path(), output);
            } else {
                output.push(hex(
                    Sha256::digest(fs::read(entry.path()).unwrap()).as_slice()
                ));
            }
        }
    }
    let mut output = Vec::new();
    visit(root, &mut output);
    output.sort();
    output
}

#[test]
fn common_plugin_baseline_merges_non_conflicting_changes_on_both_sides() {
    let world = World::new();
    for plugin in ["tutor", "book", "practice"] {
        ensure_subject(&world, &world.local_home, &world.local, plugin, "base");
    }
    world.install_runtimes(&world.local, &world.local_home);
    world.install_runtimes(&world.remote, &world.remote_home);
    copy_tree(
        &world.local_home.join(".lwc/plugins"),
        &world.remote_home.join(".lwc/plugins"),
    );
    for plugin in ["tutor", "book", "practice"] {
        world.plugin_ok(plugin, &world.local, &world.local_home, &["status"]);
        world.plugin_ok(plugin, &world.remote, &world.remote_home, &["status"]);
    }
    let paired = world.sync("merge", None, None);
    assert!(
        paired.status.success(),
        "failed to establish the shared plugin baseline: {}",
        String::from_utf8_lossy(&paired.stderr)
    );
    for plugin in ["tutor", "book", "practice"] {
        let local = json!({
            "id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","name":"Local only",
            "request_id":format!("local-{plugin}-only")
        })
        .to_string();
        world.plugin_ok(
            plugin,
            &world.local,
            &world.local_home,
            &["subject", "ensure", "--json", &local],
        );
        let remote = json!({
            "id":"cccccccccccccccccccccccccccccccc","name":"Remote only",
            "request_id":format!("remote-{plugin}-only")
        })
        .to_string();
        world.plugin_ok(
            plugin,
            &world.remote,
            &world.remote_home,
            &["subject", "ensure", "--json", &remote],
        );
    }
    let merged = world.sync("merge", None, None);
    assert!(
        merged.status.success(),
        "common-baseline non-conflicting plugin edits must merge: {}",
        String::from_utf8_lossy(&merged.stderr)
    );
    for plugin in ["tutor", "book", "practice"] {
        for (cwd, home) in [
            (&world.local, &world.local_home),
            (&world.remote, &world.remote_home),
        ] {
            for id in [
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "cccccccccccccccccccccccccccccccc",
            ] {
                world.plugin_ok(plugin, cwd, home, &["subject", "show", id]);
            }
        }
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn partial_plugin_publication_cannot_be_aborted_and_resume_is_idempotent() {
    let world = World::new();
    seed_small(&world);
    let failed = world.sync("merge", Some("book"), None);
    assert!(
        !failed.status.success(),
        "fault injection did not stop Book publication"
    );
    let sync_root = world.local.join(".lwc/sync");
    let session = fs::read_dir(&sync_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .find(|entry| entry.path().join("state.json").is_file())
        .unwrap()
        .file_name()
        .to_string_lossy()
        .into_owned();
    let aborted = world.lwc(
        &world.local,
        &world.local_home,
        &[
            "sync",
            "peer",
            world.remote.to_str().unwrap(),
            "--mode",
            "merge",
            "--abort",
            &session,
        ],
    );
    assert!(
        !aborted.status.success(),
        "abort hid a published Tutor unit"
    );
    let error: Value = serde_json::from_slice(&aborted.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_partially_applied");
    let resumed = world.sync("merge", None, Some(&session));
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
}

#[test]
#[ignore = "large Pro acceptance: requires LWC_LARGE_SYNC_FIXTURE produced by the public CLI generator"]
fn large_fixture_manifest_has_frozen_capacity_and_no_loss_inventory() {
    let root = PathBuf::from(
        std::env::var_os("LWC_LARGE_SYNC_FIXTURE")
            .expect("set LWC_LARGE_SYNC_FIXTURE to the generated two-host fixture"),
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["tutor_turns"].as_u64().unwrap() >= 10_000);
    assert!(manifest["book_normalized_bytes"].as_u64().unwrap() >= 256 * 1024 * 1024);
    assert!(manifest["book_blocks"].as_u64().unwrap() >= 100_000);
    assert!(manifest["practice_items"].as_u64().unwrap() >= 50_000);
    assert!(manifest["responses"].as_u64().unwrap() >= 10_000);
    for key in [
        "category_counts",
        "category_ids_sha256",
        "logical_hashes",
        "blob_hashes",
        "canaries",
        "wall_ms",
        "peak_rss_bytes",
        "transferred_bytes",
        "rebuilt_bytes",
    ] {
        assert!(
            !manifest[key].is_null(),
            "large fixture manifest lacks {key}"
        );
    }
}
