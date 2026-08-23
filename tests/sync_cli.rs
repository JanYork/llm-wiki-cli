use rusqlite::{
    Connection,
    session::{ConflictAction, Session},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

#[test]
fn sync_help_truthfully_advertises_all_scope() {
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .args(["sync", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Sync accepts all three"), "{help}");
    assert!(
        help.contains("[possible values: project, global, all]"),
        "{help}"
    );
}

struct SyncWorld {
    _temp: TempDir,
    local: PathBuf,
    remote: PathBuf,
    local_home: PathBuf,
    remote_home: PathBuf,
    bin: PathBuf,
}

impl SyncWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let remote = temp.path().join("remote");
        let local_home = temp.path().join("local-home");
        let remote_home = temp.path().join("remote-home");
        let bin = temp.path().join("bin");
        for path in [&local, &remote, &local_home, &remote_home, &bin] {
            fs::create_dir_all(path).unwrap();
        }
        let world = Self {
            _temp: temp,
            local,
            remote,
            local_home,
            remote_home,
            bin,
        };
        world.ok(&world.local, &["init"]);
        world.ok(&world.remote, &["init"]);
        world
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command_home(cwd, &self.local_home, args)
    }

    fn command_home(&self, cwd: &Path, home: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", home)
            .env("LWC_PROJECT_ROOT", cwd)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, cwd: &Path, args: &[&str]) -> Value {
        self.ok_home(cwd, &self.local_home, args)
    }

    fn ok_home(&self, cwd: &Path, home: &Path, args: &[&str]) -> Value {
        let output = self.command_home(cwd, home, args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn has_page(&self, cwd: &Path, slug: &str) -> bool {
        self.command(cwd, &["page", "show", slug]).status.success()
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap()
    }

    fn init_shared_git_history(&self) {
        assert!(
            self.git(&self.local, &["init", "-b", "main"])
                .status
                .success()
        );
        fs::write(self.local.join("tracked.txt"), "base\n").unwrap();
        for cwd in [&self.local, &self.remote] {
            self.git(cwd, &["config", "user.name", "Sync Test"]);
            self.git(cwd, &["config", "user.email", "sync@example.invalid"]);
        }
        assert!(
            self.git(&self.local, &["add", "tracked.txt"])
                .status
                .success()
        );
        assert!(
            self.git(&self.local, &["commit", "-m", "base"])
                .status
                .success()
        );
        assert!(
            self.git(&self.remote, &["init", "-b", "main"])
                .status
                .success()
        );
        assert!(
            self.git(
                &self.remote,
                &["fetch", self.local.to_str().unwrap(), "main"]
            )
            .status
            .success()
        );
        assert!(
            self.git(&self.remote, &["checkout", "-B", "main", "FETCH_HEAD"])
                .status
                .success()
        );
        assert!(
            self.git(
                &self.remote,
                &["config", "receive.denyCurrentBranch", "updateInstead"]
            )
            .status
            .success()
        );
    }

    fn put_page(&self, cwd: &Path, slug: &str, title: &str, body: &str) {
        self.put_page_home(cwd, &self.local_home, false, slug, title, body);
    }

    fn put_draft_page(&self, cwd: &Path, home: &Path, draft: &str, slug: &str) {
        self.ok_home(cwd, home, &["changeset", "begin", draft]);
        let source_path = cwd.join(format!("{slug}.draft-source.md"));
        fs::write(&source_path, "portable detached source bytes").unwrap();
        let source = self.ok_home(
            cwd,
            home,
            &[
                "--changeset",
                draft,
                "source",
                "add",
                source_path.to_str().unwrap(),
                "--title",
                "Detached Source",
            ],
        );
        let source_id = source["source"]["id"].as_i64().unwrap().to_string();
        let body_path = cwd.join(format!("{slug}.draft-sync-test.md"));
        fs::write(&body_path, "detached draft knowledge").unwrap();
        self.ok_home(
            cwd,
            home,
            &[
                "--changeset",
                draft,
                "page",
                "put",
                slug,
                "--title",
                "Detached Draft",
                "--file",
                body_path.to_str().unwrap(),
                "--provenance",
                "agent-observed",
                "--source",
                &source_id,
            ],
        );
        fs::remove_file(body_path).unwrap();
        fs::remove_file(source_path).unwrap();
    }

    fn put_page_home(
        &self,
        cwd: &Path,
        home: &Path,
        global: bool,
        slug: &str,
        title: &str,
        body: &str,
    ) {
        let body_path = cwd.join(format!("{slug}.sync-test.md"));
        fs::write(&body_path, body).unwrap();
        let common = [
            "page",
            "put",
            slug,
            "--title",
            title,
            "--file",
            body_path.to_str().unwrap(),
            "--provenance",
            "agent-observed",
        ];
        if global {
            let mut args = vec!["--scope", "global"];
            args.extend(common);
            self.ok_home(cwd, home, &args);
        } else {
            self.ok_home(
                cwd,
                home,
                &[
                    "page",
                    "put",
                    slug,
                    "--title",
                    title,
                    "--file",
                    body_path.to_str().unwrap(),
                    "--provenance",
                    "agent-observed",
                ],
            );
        }
        fs::remove_file(body_path).unwrap();
    }

    #[cfg(unix)]
    fn install_fake_ssh(&self) {
        use std::os::unix::fs::PermissionsExt;

        let path = self.bin.join("ssh");
        fs::write(
            &path,
            "#!/bin/sh\n[ \"$1\" = \"-G\" ] && exit 0\nexport HOME=\"$LWC_REMOTE_HOME\"\nlast=\"\"\nfor arg in \"$@\"; do last=$arg; done\ncase \"$last\" in\n  git-upload-pack*|git-receive-pack*) exec /bin/sh -c \"$last\" ;;\n  *) sleep \"${LWC_TEST_SSH_DELAY:-0}\"\n     shift\n     shift\n     shift\n     exec \"$LWC_TEST_BINARY\" \"$@\" ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    fn sync(&self, args: &[&str]) -> Output {
        self.sync_command(args).output().unwrap()
    }

    #[cfg(unix)]
    fn sync_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lwc"));
        command
            .current_dir(&self.local)
            .env("HOME", &self.local_home)
            .env("LWC_PROJECT_ROOT", &self.local)
            .env("LWC_TEST_BINARY", env!("CARGO_BIN_EXE_lwc"))
            .env("LWC_REMOTE_HOME", &self.remote_home)
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
        command
    }
}

fn write_sync_work_fixture(root: &Path, database: &Path, id: &str, state: &str) {
    let directory = root.join(".lwc/work").join(id);
    fs::create_dir_all(&directory).unwrap();
    let terminal = matches!(state, "succeeded" | "failed" | "cancelled");
    let payload = serde_json::json!({
        "id": id,
        "kind": "graph-project",
        "scope": "project",
        "database": database,
        "state": state,
        "phase": if terminal { "complete" } else { state },
        "completed": if terminal { 3 } else { 0 },
        "total": 3,
        "percent": if terminal { 100.0 } else { 0.0 },
        "sequence": 7,
        "updated_at_unix_ms": 1_777_777_777_777_u128,
        "started_at_unix_ms": 1_777_777_777_000_u128,
        "items_per_second": 2.5,
        "eta_seconds": if terminal { 0 } else { 120 },
        "cancel_requested": false,
        "pid": 424242,
        "message": format!("private {state} message at {}", database.display()),
        "result": if terminal { Some(serde_json::json!({"private_path": database})) } else { None },
        "error": Value::Null,
    });
    fs::write(
        directory.join("state.json"),
        serde_json::to_vec_pretty(&payload).unwrap(),
    )
    .unwrap();
}

fn create_sync_schema(connection: &Connection, schema: &str) {
    connection
        .execute_batch(&format!(
            "CREATE TABLE {schema}.sync_objects(
                 kind TEXT NOT NULL,
                 logical_key TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 payload_hash TEXT NOT NULL,
                 PRIMARY KEY(kind, logical_key)
             );
             CREATE TABLE {schema}.sync_blobs(
                 content_hash TEXT PRIMARY KEY NOT NULL,
                 content BLOB NOT NULL
             );
             CREATE VIRTUAL TABLE {schema}.derived_fts USING fts5(body);"
        ))
        .unwrap();
}

#[test]
fn session_delta_round_trips_normalized_tables() {
    let current = Connection::open_in_memory().unwrap();
    current
        .execute_batch("ATTACH DATABASE ':memory:' AS baseline;")
        .unwrap();
    create_sync_schema(&current, "main");
    create_sync_schema(&current, "baseline");

    current
        .execute_batch(
            "INSERT INTO baseline.sync_objects VALUES
                 ('page', 'guide', '{\"body\":\"old\"}', 'old');
             INSERT INTO baseline.sync_blobs VALUES ('blob-a', X'61');
             INSERT INTO baseline.derived_fts VALUES ('old derived text');

             INSERT INTO main.sync_objects VALUES
                 ('page', 'guide', '{\"body\":\"new\"}', 'new'),
                 ('todo', 'todo-1', '{\"title\":\"ship\"}', 'todo');
             INSERT INTO main.sync_blobs VALUES
                 ('blob-a', X'61'),
                 ('blob-b', X'62');
             INSERT INTO main.derived_fts VALUES ('new derived text');",
        )
        .unwrap();

    let mut session = Session::new(&current).unwrap();
    session
        .diff::<&str, &str>("baseline", "sync_objects")
        .unwrap();
    session
        .diff::<&str, &str>("baseline", "sync_blobs")
        .unwrap();
    let mut changeset = Vec::new();
    session.changeset_strm(&mut changeset).unwrap();
    assert!(!changeset.is_empty());

    let target = Connection::open_in_memory().unwrap();
    create_sync_schema(&target, "main");
    target
        .execute_batch(
            "INSERT INTO sync_objects VALUES
                 ('page', 'guide', '{\"body\":\"old\"}', 'old');
             INSERT INTO sync_blobs VALUES ('blob-a', X'61');
             INSERT INTO derived_fts VALUES ('old derived text');",
        )
        .unwrap();
    target
        .apply_strm(
            &mut changeset.as_slice(),
            None::<fn(&str) -> bool>,
            |_kind, _item| ConflictAction::SQLITE_CHANGESET_ABORT,
        )
        .unwrap();

    assert_eq!(
        target
            .query_row(
                "SELECT payload_hash FROM sync_objects
                 WHERE kind = 'page' AND logical_key = 'guide'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "new"
    );
    assert_eq!(
        target
            .query_row("SELECT COUNT(*) FROM sync_objects", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        target
            .query_row("SELECT COUNT(*) FROM sync_blobs", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        target
            .query_row("SELECT body FROM derived_fts", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "old derived text"
    );
}

#[cfg(unix)]
#[test]
fn sync_cli_persists_resumes_and_rejects_abort_after_publication() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    let remote = world.remote.to_str().unwrap();

    let output = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        output.status.success(),
        "sync failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let created: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(created["action"], "completed");
    assert_eq!(created["mode"], "merge");
    assert_eq!(created["scope"], "project");
    let session = created["session_id"].as_str().unwrap();
    assert_eq!(session.len(), 32);
    let state = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    assert!(state.is_file());

    let resumed = world.sync(&["sync", "peer", remote, "--resume", session]);
    assert!(resumed.status.success());
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["session_id"], session);
    assert_eq!(resumed["action"], "completed");
    assert_eq!(resumed["idempotent"], true);
    assert_eq!(resumed["stores"].as_array().unwrap().len(), 1);
    assert_eq!(resumed["stores"][0]["published_local"], true);
    assert!(resumed["stores"][0]["ending_local"].is_object());
    assert!(resumed["stores"][0]["publication_local"]["checkpoint"].is_string());

    let aborted = world.sync(&["sync", "peer", remote, "--abort", session]);
    assert!(!aborted.status.success());
    let aborted: Value = serde_json::from_slice(&aborted.stderr).unwrap();
    assert_eq!(aborted["error"]["code"], "sync_partially_applied");
    let saved: Value = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
    assert_eq!(saved["phase"], "completed");
}

#[cfg(unix)]
#[test]
fn sync_resume_and_abort_require_the_original_remote_directory() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    let initial = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let value: Value = serde_json::from_slice(&initial.stdout).unwrap();
    let session = value["session_id"].as_str().unwrap();
    for continuation in ["--resume", "--abort"] {
        let output = world.sync(&[
            "sync",
            "peer",
            world.local.to_str().unwrap(),
            "--mode",
            "merge",
            continuation,
            session,
        ]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("sync_session_mismatch"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn sync_merge_transfers_and_publishes_non_overlapping_project_state_to_both_sides() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local knowledge");
    world.put_page(&world.remote, "remote-note", "Remote", "remote knowledge");
    let remote = world.remote.to_str().unwrap();

    let output = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        output.status.success(),
        "sync failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "completed", "{result:#}");
    assert_eq!(result["stores"][0]["conflict_count"], 0);
    for project in [&world.local, &world.remote] {
        assert_eq!(
            world.ok(project, &["page", "show", "local-note"])["page"]["title"],
            "Local"
        );
        assert_eq!(
            world.ok(project, &["page", "show", "remote-note"])["page"]["title"],
            "Remote"
        );
        let lint = world.ok(project, &["lint"]);
        let issue_codes = lint["issues"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|issue| issue["code"].as_str())
            .collect::<Vec<_>>();
        assert!(
            !issue_codes
                .iter()
                .any(|code| code.contains("index") || code.contains("foreign"))
        );
    }
}

#[cfg(unix)]
#[test]
fn sync_pull_and_push_publish_only_the_selected_destination_without_overwriting_unique_data() {
    for mode in ["pull", "push"] {
        let world = SyncWorld::new();
        world.install_fake_ssh();
        world.put_page(&world.local, "local-note", "Local", "local knowledge");
        world.put_page(&world.remote, "remote-note", "Remote", "remote knowledge");
        let output = world.sync(&[
            "sync",
            "peer",
            world.remote.to_str().unwrap(),
            "--mode",
            mode,
        ]);
        assert!(
            output.status.success(),
            "{mode} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["action"], "completed");
        if mode == "pull" {
            assert!(world.has_page(&world.local, "local-note"));
            assert!(world.has_page(&world.local, "remote-note"));
            assert!(!world.has_page(&world.remote, "local-note"));
            assert!(world.has_page(&world.remote, "remote-note"));
        } else {
            assert!(world.has_page(&world.local, "local-note"));
            assert!(!world.has_page(&world.local, "remote-note"));
            assert!(world.has_page(&world.remote, "local-note"));
            assert!(world.has_page(&world.remote, "remote-note"));
        }
    }
}

#[cfg(unix)]
#[test]
fn sync_conflicts_pause_with_semantic_packet_and_agent_resolution_resumes_publication() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "guide", "Local title", "Local body");
    world.put_page(&world.remote, "guide", "Remote title", "Remote body");
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["action"], "conflicts");
    let conflicts = first["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["kind"], "page");
    assert_eq!(conflicts[0]["logical_key"], "guide");
    assert!(
        serde_json::to_string(conflicts)
            .unwrap()
            .contains("Remote title")
    );
    assert!(
        !serde_json::to_string(conflicts)
            .unwrap()
            .contains("sync_objects")
    );

    let decisions = conflicts
        .iter()
        .flat_map(|conflict| {
            conflict["fields"].as_array().unwrap().iter().map(|field| {
                let candidates = field["candidates"].as_array().unwrap();
                let candidate = candidates
                    .iter()
                    .position(|value| {
                        value
                            .as_str()
                            .is_some_and(|text| text.starts_with("Remote"))
                    })
                    .unwrap_or(0);
                serde_json::json!({
                    "conflict_id": conflict["conflict_id"],
                    "kind": conflict["kind"],
                    "logical_key": conflict["logical_key"],
                    "path": field["path"],
                    "candidate": candidate,
                })
            })
        })
        .collect::<Vec<_>>();
    let resolution = world.local.join("resolution.json");
    fs::write(
        &resolution,
        serde_json::to_vec(&serde_json::json!({"version": 1, "decisions": decisions})).unwrap(),
    )
    .unwrap();
    let session = first["session_id"].as_str().unwrap();
    let resumed = world.sync(&[
        "sync",
        "peer",
        remote,
        "--mode",
        "merge",
        "--resume",
        session,
        "--resolve",
        resolution.to_str().unwrap(),
    ]);
    assert!(
        resumed.status.success(),
        "resume failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    for project in [&world.local, &world.remote] {
        let page = world.ok(project, &["page", "show", "guide"]);
        assert_eq!(page["page"]["title"], "Remote title");
        assert_eq!(page["page"]["body"], "Remote body");
    }
}

#[cfg(unix)]
#[test]
fn sync_preserve_both_resolution_resumes_and_keeps_both_pages() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "guide", "Local title", "Local body");
    world.put_page(&world.remote, "guide", "Remote title", "Remote body");
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(first.status.success());
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["action"], "conflicts");
    let conflict = &first["conflicts"][0];
    let resolution = world.local.join("preserve-both-resolution.json");
    fs::write(
        &resolution,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "decisions": [{
                "conflict_id": conflict["conflict_id"],
                "kind": conflict["kind"],
                "logical_key": conflict["logical_key"],
                "strategy": "preserve_both"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let resumed = world.sync(&[
        "sync",
        "peer",
        remote,
        "--mode",
        "merge",
        "--resume",
        first["session_id"].as_str().unwrap(),
        "--resolve",
        resolution.to_str().unwrap(),
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    for project in [&world.local, &world.remote] {
        let slugs = world.ok(project, &["page", "list"])["pages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|page| page["slug"].as_str())
            .filter(|slug| *slug == "guide" || slug.starts_with("guide--sync-"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(slugs.len(), 2, "{slugs:?}");
        assert_eq!(
            slugs
                .iter()
                .map(|slug| {
                    world.ok(project, &["page", "show", slug])["page"]["body"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["Local body".to_string(), "Remote body".to_string()])
        );
    }
}

#[cfg(unix)]
#[test]
fn sync_abort_succeeds_before_publication_and_blocks_resume() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "guide", "Local title", "Local body");
    world.put_page(&world.remote, "guide", "Remote title", "Remote body");
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(first.status.success());
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["action"], "conflicts");
    let session = first["session_id"].as_str().unwrap();

    let aborted = world.sync(&[
        "sync", "peer", remote, "--mode", "merge", "--abort", session,
    ]);
    assert!(
        aborted.status.success(),
        "{}",
        String::from_utf8_lossy(&aborted.stderr)
    );
    let aborted: Value = serde_json::from_slice(&aborted.stdout).unwrap();
    assert_eq!(aborted["action"], "aborted");
    assert_eq!(
        world.ok(&world.local, &["page", "show", "guide"])["page"]["body"],
        "Local body"
    );
    assert_eq!(
        world.ok(&world.remote, &["page", "show", "guide"])["page"]["body"],
        "Remote body"
    );

    let resumed = world.sync(&[
        "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(!resumed.status.success());
    let error: Value = serde_json::from_slice(&resumed.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_session_aborted");
}

#[cfg(unix)]
#[test]
fn sync_conflicts_are_returned_and_resolved_in_bounded_durable_batches() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    for index in 0..21 {
        let slug = format!("conflict-{index:02}");
        world.put_page(&world.local, &slug, "Local", "local");
        world.put_page(&world.remote, &slug, "Remote", "remote");
    }
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let batch = first["conflicts"].as_array().unwrap();
    assert_eq!(batch.len(), 20, "{first}");
    let first_ids = batch
        .iter()
        .map(|conflict| conflict["conflict_id"].as_str().unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let decisions = batch
        .iter()
        .flat_map(|conflict| {
            conflict["fields"].as_array().unwrap().iter().map(|field| {
                serde_json::json!({
                    "conflict_id": conflict["conflict_id"],
                    "kind": conflict["kind"],
                    "logical_key": conflict["logical_key"],
                    "path": field["path"],
                    "candidate": 0,
                })
            })
        })
        .collect::<Vec<_>>();
    let resolution = world.local.join("batch-resolution.json");
    fs::write(
        &resolution,
        serde_json::to_vec(&serde_json::json!({"version":1,"decisions":decisions})).unwrap(),
    )
    .unwrap();
    let session = first["session_id"].as_str().unwrap();
    let resumed = world.sync(&[
        "sync",
        "peer",
        remote,
        "--mode",
        "merge",
        "--resume",
        session,
        "--resolve",
        resolution.to_str().unwrap(),
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "conflicts", "{resumed}");
    let next = resumed["conflicts"].as_array().unwrap();
    assert_eq!(next.len(), 1, "{resumed}");
    assert!(!first_ids.contains(next[0]["conflict_id"].as_str().unwrap()));
}

#[cfg(unix)]
#[test]
fn sync_global_and_all_scope_use_the_matching_remote_stores() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.ok_home(
        &world.local,
        &world.local_home,
        &["--scope", "global", "init"],
    );
    world.ok_home(
        &world.remote,
        &world.remote_home,
        &["--scope", "global", "init"],
    );
    world.put_page_home(
        &world.local,
        &world.local_home,
        true,
        "local-global",
        "Local global",
        "local global memory",
    );
    world.put_page_home(
        &world.remote,
        &world.remote_home,
        true,
        "remote-global",
        "Remote global",
        "remote global memory",
    );
    let global = world.sync(&["--scope", "global", "sync", "peer", "--mode", "merge"]);
    assert!(
        global.status.success(),
        "global sync failed: {}",
        String::from_utf8_lossy(&global.stderr)
    );
    for (cwd, home) in [
        (&world.local, &world.local_home),
        (&world.remote, &world.remote_home),
    ] {
        assert!(
            world
                .command_home(
                    cwd,
                    home,
                    &["--scope", "global", "page", "show", "local-global"]
                )
                .status
                .success()
        );
        assert!(
            world
                .command_home(
                    cwd,
                    home,
                    &["--scope", "global", "page", "show", "remote-global"]
                )
                .status
                .success()
        );
    }

    world.put_page(
        &world.local,
        "local-project",
        "Local project",
        "local project memory",
    );
    world.put_page_home(
        &world.remote,
        &world.remote_home,
        false,
        "remote-project",
        "Remote project",
        "remote project memory",
    );
    let all = world.sync(&[
        "--scope",
        "all",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        all.status.success(),
        "all sync failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&all.stdout),
        String::from_utf8_lossy(&all.stderr)
    );
    let result: Value = serde_json::from_slice(&all.stdout).unwrap();
    assert_eq!(result["stores"].as_array().unwrap().len(), 2);
    for project in [&world.local, &world.remote] {
        assert!(world.has_page(project, "local-project"));
        assert!(world.has_page(project, "remote-project"));
    }
}

#[cfg(unix)]
#[test]
fn sync_git_merges_committed_files_while_preserving_tracked_wip_ignored_and_untracked_files() {
    let world = SyncWorld::new();
    world.init_shared_git_history();
    world.install_fake_ssh();
    fs::write(world.local.join("deleted.txt"), "base deleted\n").unwrap();
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["add", "deleted.txt"]
    ));
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["commit", "-m", "tracked wip fixtures"]
    ));
    assert!(self_or_world_git(
        &world,
        &world.remote,
        &["fetch", world.local.to_str().unwrap(), "main"]
    ));
    assert!(self_or_world_git(
        &world,
        &world.remote,
        &["reset", "--hard", "FETCH_HEAD"]
    ));
    fs::write(world.local.join("local.txt"), "local commit\n").unwrap();
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["add", "local.txt"]
    ));
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["commit", "-m", "local"]
    ));
    fs::write(world.remote.join("remote.txt"), "remote commit\n").unwrap();
    assert!(self_or_world_git(
        &world,
        &world.remote,
        &["add", "remote.txt"]
    ));
    assert!(self_or_world_git(
        &world,
        &world.remote,
        &["commit", "-m", "remote"]
    ));
    fs::write(world.local.join("tracked.txt"), "tracked wip\n").unwrap();
    fs::write(world.local.join("staged.txt"), "staged wip\n").unwrap();
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["add", "staged.txt"]
    ));
    fs::remove_file(world.local.join("deleted.txt")).unwrap();
    fs::write(world.local.join("untracked.keep"), "untracked\n").unwrap();
    fs::write(world.local.join(".gitignore"), "ignored.keep\n").unwrap();
    fs::write(world.local.join("ignored.keep"), "ignored\n").unwrap();
    let cached_before = world.git(&world.local, &["diff", "--cached", "--binary"]);
    let worktree_before = world.git(&world.local, &["diff", "--binary"]);

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        output.status.success(),
        "git sync failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    let session = result["session_id"].as_str().unwrap();
    let remote_ref = format!("refs/lwc-sync/{session}/remote");
    let merged_ref = format!("refs/lwc-sync/{session}/merged");
    assert_eq!(result["action"], "git_pending", "{result}");
    assert_eq!(result["git"]["status"], "pending_local_wip", "{result}");
    assert_eq!(result["git"]["published_remote"], true, "{result}");
    assert_eq!(result["git"]["applied_local"], false, "{result}");
    assert_eq!(result["git"]["tracked_wip_included"], true, "{result}");
    assert_eq!(
        fs::read_to_string(world.local.join("tracked.txt")).unwrap(),
        "tracked wip\n"
    );
    assert_eq!(
        fs::read_to_string(world.local.join("untracked.keep")).unwrap(),
        "untracked\n"
    );
    assert_eq!(
        fs::read_to_string(world.local.join("ignored.keep")).unwrap(),
        "ignored\n"
    );
    assert_eq!(
        fs::read_to_string(world.remote.join("local.txt")).unwrap(),
        "local commit\n"
    );
    assert_eq!(
        fs::read_to_string(world.remote.join("remote.txt")).unwrap(),
        "remote commit\n"
    );
    assert_eq!(
        fs::read_to_string(world.remote.join("tracked.txt")).unwrap(),
        "tracked wip\n"
    );
    assert_eq!(
        fs::read_to_string(world.remote.join("staged.txt")).unwrap(),
        "staged wip\n"
    );
    assert!(!world.remote.join("deleted.txt").exists());
    assert!(!world.remote.join("untracked.keep").exists());
    assert!(!world.remote.join("ignored.keep").exists());
    assert!(!world.remote.join(".gitignore").exists());
    assert_eq!(
        world
            .git(&world.local, &["diff", "--cached", "--binary"])
            .stdout,
        cached_before.stdout
    );
    assert_eq!(
        world.git(&world.local, &["diff", "--binary"]).stdout,
        worktree_before.stdout
    );
    assert!(
        !world
            .git(&world.local, &["status", "--porcelain"])
            .stdout
            .is_empty()
    );
    for retained in [&remote_ref, &merged_ref] {
        assert!(
            world
                .git(&world.local, &["show-ref", "--verify", "--quiet", retained])
                .status
                .success(),
            "pending Sync must retain {retained}"
        );
    }

    assert!(self_or_world_git(&world, &world.local, &["add", "-u"]));
    assert!(self_or_world_git(
        &world,
        &world.local,
        &["commit", "-m", "preserve local wip"]
    ));
    let resumed = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
        "--resume",
        session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed", "{resumed}");
    for cleaned in [&remote_ref, &merged_ref] {
        assert!(
            !world
                .git(&world.local, &["show-ref", "--verify", "--quiet", cleaned])
                .status
                .success(),
            "completed Sync must clean {cleaned}"
        );
    }
    assert!(
        resumed["git_derived"]["local"]["status"].is_string(),
        "{resumed}"
    );
    assert!(
        resumed["git_derived"]["remote"]["status"].is_string(),
        "{resumed}"
    );
    assert_eq!(
        fs::read_to_string(world.local.join("remote.txt")).unwrap(),
        "remote commit\n"
    );
    assert_eq!(
        fs::read_to_string(world.local.join("untracked.keep")).unwrap(),
        "untracked\n"
    );
    assert_eq!(
        fs::read_to_string(world.local.join("ignored.keep")).unwrap(),
        "ignored\n"
    );
}

#[cfg(unix)]
#[test]
fn sync_git_push_rejection_is_resumable_after_wiki_publication() {
    let world = SyncWorld::new();
    world.init_shared_git_history();
    world.install_fake_ssh();
    assert!(
        world
            .git(
                &world.remote,
                &["config", "--unset", "receive.denyCurrentBranch"]
            )
            .status
            .success()
    );
    fs::write(world.local.join("tracked.txt"), "local committed\n").unwrap();
    assert!(
        world
            .git(&world.local, &["add", "tracked.txt"])
            .status
            .success()
    );
    assert!(
        world
            .git(&world.local, &["commit", "-m", "local committed"])
            .status
            .success()
    );
    world.put_page(&world.local, "local-note", "Local", "local wiki");
    let remote = world.remote.to_str().unwrap();

    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["action"], "git_pending", "{first}");
    assert_eq!(first["git"]["status"], "pending_remote_push");
    assert_eq!(first["stores"][0]["published_remote"], true);
    assert!(world.has_page(&world.remote, "local-note"));

    assert!(
        world
            .git(
                &world.remote,
                &["config", "receive.denyCurrentBranch", "updateInstead"]
            )
            .status
            .success()
    );
    let resumed = world.sync(&[
        "sync",
        "peer",
        remote,
        "--mode",
        "merge",
        "--resume",
        first["session_id"].as_str().unwrap(),
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed", "{resumed}");
    assert_eq!(
        fs::read_to_string(world.remote.join("tracked.txt")).unwrap(),
        "local committed\n"
    );
}

#[cfg(unix)]
#[test]
fn sync_git_rejects_prepositioned_temporary_index_symlink() {
    use std::os::unix::fs::symlink;

    let world = SyncWorld::new();
    world.init_shared_git_history();
    world.install_fake_ssh();
    fs::write(world.local.join("tracked.txt"), "dirty\n").unwrap();
    fs::create_dir(world.local.join("sentinel-dir")).unwrap();
    fs::write(world.local.join("sentinel-dir/sentinel"), "safe\n").unwrap();
    symlink(
        world.local.join("sentinel-dir"),
        world.local.join(".git/lwc-sync"),
    )
    .unwrap();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);

    assert!(!output.status.success(), "sync must reject the symlink");
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_git_failed");
    assert_eq!(
        fs::read(world.local.join("sentinel-dir/sentinel")).unwrap(),
        b"safe\n"
    );
    assert_eq!(
        fs::read_dir(world.local.join("sentinel-dir"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
fn self_or_world_git(world: &SyncWorld, cwd: &Path, args: &[&str]) -> bool {
    world.git(cwd, args).status.success()
}

#[test]
fn sync_cli_rejects_host_injection_before_transport() {
    let world = SyncWorld::new();
    let output = world.command(
        &world.local,
        &["sync", "peer;touch-owned", world.remote.to_str().unwrap()],
    );
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_sync_host");

    let output = world.command(&world.local, &["sync", "peer", "relative/project"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "invalid_sync_directory");
}

#[cfg(unix)]
#[test]
fn sync_git_shell_quotes_a_remote_directory_with_shell_syntax() {
    let mut world = SyncWorld::new();
    let hostile = world
        ._temp
        .path()
        .join("remote';touch owned-by-remote-path;echo '");
    fs::rename(&world.remote, &hostile).unwrap();
    world.remote = hostile;
    world.init_shared_git_history();
    world.install_fake_ssh();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!world.local.join("owned-by-remote-path").exists());
}

#[test]
fn sync_peer_rejects_an_incompatible_protocol_before_store_access() {
    let world = SyncWorld::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.remote)
        .env("HOME", &world.remote_home)
        .env("LWC_PROJECT_ROOT", &world.remote)
        .arg("__sync-peer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"{"protocol":2,"action":"handshake","session_id":"0123456789abcdef0123456789abcdef","scope":"project","directory":"/definitely/not/a/store"}"#,
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_protocol_mismatch");
}

#[cfg(unix)]
#[test]
fn sync_peer_does_not_treat_an_unsafe_store_path_as_a_missing_store() {
    use std::os::unix::fs::symlink;

    for symlink_runtime in [false, true] {
        let world = SyncWorld::new();
        let original = world.remote.join(".lwc-original");
        fs::rename(world.remote.join(".lwc"), &original).unwrap();
        if symlink_runtime {
            symlink(&original, world.remote.join(".lwc")).unwrap();
        } else {
            fs::create_dir(world.remote.join(".lwc")).unwrap();
            symlink(original.join("wiki.db"), world.remote.join(".lwc/wiki.db")).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&world.remote)
            .env("HOME", &world.remote_home)
            .env("LWC_PROJECT_ROOT", &world.remote)
            .arg("__sync-peer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(
            child.stdin.take().unwrap(),
            "{}",
            serde_json::json!({
                "protocol": 1,
                "action": "handshake",
                "session_id": "0123456789abcdef0123456789abcdef",
                "scope": "project",
                "directory": world.remote,
            })
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_ne!(error["error"]["code"], "store_not_found");
    }
}

#[cfg(unix)]
#[test]
fn sync_rejects_a_local_write_after_snapshot_instead_of_overwriting_it() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.remote, "remote-note", "Remote", "remote knowledge");

    let mut command = world.sync_command(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
    ]);
    command.env("LWC_TEST_SSH_DELAY", "1");
    let child = command.spawn().unwrap();

    let sync_root = world.local.join(".lwc/sync");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let snapshot_exists = fs::read_dir(&sync_root).is_ok_and(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().join("project/local.db").is_file())
        });
        if snapshot_exists {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "local snapshot was not staged"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    world.put_page(
        &world.local,
        "concurrent-note",
        "Concurrent",
        "must survive",
    );

    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "sync must stop on the CAS mismatch"
    );
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_store_changed");
    assert!(world.has_page(&world.local, "concurrent-note"));
    assert!(!world.has_page(&world.local, "remote-note"));
}

#[cfg(unix)]
#[test]
fn sync_all_stages_every_scope_before_publishing_any_scope() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    for (cwd, home) in [
        (&world.local, &world.local_home),
        (&world.remote, &world.remote_home),
    ] {
        world.ok_home(cwd, home, &["--scope", "global", "init"]);
    }
    world.put_page(&world.local, "local-project", "Local", "local");
    world.put_page(&world.remote, "remote-project", "Remote", "remote");
    world.put_page_home(
        &world.local,
        &world.local_home,
        true,
        "global-conflict",
        "Local",
        "local",
    );
    world.put_page_home(
        &world.remote,
        &world.remote_home,
        true,
        "global-conflict",
        "Remote",
        "remote",
    );

    let output = world.sync(&[
        "--scope",
        "all",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "conflicts");
    assert!(!world.has_page(&world.local, "remote-project"));
    assert!(!world.has_page(&world.remote, "local-project"));

    let session = result["session_id"].as_str().unwrap();
    let state_path = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let merged_path = world
        .local_home
        .join(".lwc/sync")
        .join(session)
        .join("global/merged.db");
    let state_before = fs::read(&state_path).unwrap();
    let merged_before = fs::read(&merged_path).unwrap();
    let invalid = world.local.join("invalid-all-resolution.json");
    fs::write(&invalid, br#"{"version":1,"scopes":{},"extra":true}"#).unwrap();
    let rejected = world.sync(&[
        "--scope",
        "all",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
        "--resume",
        session,
        "--resolve",
        invalid.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_resolution_invalid");
    assert_eq!(fs::read(state_path).unwrap(), state_before);
    assert_eq!(fs::read(merged_path).unwrap(), merged_before);
}

#[cfg(unix)]
#[test]
fn repeated_sync_uses_a_smaller_production_session_delta() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    for index in 0..40 {
        world.put_page(
            &world.remote,
            &format!("page-{index}"),
            &format!("Page {index}"),
            &"production-shaped body ".repeat(80),
        );
    }
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "pull"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    world.put_page(
        &world.remote,
        "page-0",
        "Page 0 changed",
        &"production-shaped body ".repeat(80),
    );
    let second = world.sync(&["sync", "peer", remote, "--mode", "pull"]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let result: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(result["stores"][0]["transfer_kind"], "delta");
    assert!(
        result["stores"][0]["transferred_bytes"].as_u64().unwrap()
            < result["stores"][0]["full_bytes"].as_u64().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn resume_recovers_a_remote_commit_whose_response_was_lost() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let session = first["session_id"].as_str().unwrap();
    let state_path = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["phase"] = Value::String("publishing".to_string());
    state["units"][0]["published_remote"] = Value::Bool(false);
    state["units"][0]["derived_remote"] = Value::Bool(false);
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    let remote_db = world.remote.join(".lwc/wiki.db");
    let before: i64 = Connection::open(&remote_db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    let resumed = world.sync(&[
        "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    assert_eq!(
        resumed["stores"][0]["publication_remote"]["recovered"],
        true
    );
    assert!(
        resumed["stores"][0]["publication_remote"]["checkpoint"]
            .as_str()
            .is_some()
    );
    assert!(resumed["stores"][0]["ending_remote"]["store_id"].is_string());
    let after: i64 = Connection::open(remote_db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after, before, "resume must not replay the canonical merge");
}

#[cfg(unix)]
#[test]
fn resume_rejects_a_stale_remote_commit_receipt_after_remote_changes() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let session = first["session_id"].as_str().unwrap();
    let state_path = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["phase"] = Value::String("publishing".to_string());
    state["units"][0]["published_remote"] = Value::Bool(false);
    state["units"][0]["derived_remote"] = Value::Bool(false);
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    world.put_page(&world.remote, "remote-note-after-sync", "Remote", "later");
    let resumed = world.sync(&[
        "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(!resumed.status.success());
    let error: Value = serde_json::from_slice(&resumed.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_remote_failed");
    assert_eq!(
        error["error"]["details"]["remote"]["error"]["code"],
        "sync_store_changed"
    );
}

#[cfg(unix)]
#[test]
fn resume_recovers_an_existing_local_commit_whose_receipt_write_was_lost() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.remote, "remote-note", "Remote", "remote");
    let remote = world.remote.to_str().unwrap();
    let completed = world.sync(&["sync", "peer", remote, "--mode", "pull"]);
    assert!(completed.status.success());
    let completed: Value = serde_json::from_slice(&completed.stdout).unwrap();
    let session = completed["session_id"].as_str().unwrap();
    let state_path = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["phase"] = Value::String("publishing".to_string());
    state["units"][0]["published_local"] = Value::Bool(false);
    state["units"][0]["ending_local"] = Value::Null;
    state["units"][0]["publication_local"] = Value::Null;
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    let database = world.local.join(".lwc/wiki.db");
    let before: i64 = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();

    let resumed = world.sync(&[
        "sync", "peer", remote, "--mode", "pull", "--resume", session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    assert_eq!(resumed["stores"][0]["publication_local"]["recovered"], true);
    let after: i64 = Connection::open(database)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        after, before,
        "resume must not replay the local publication"
    );
}

#[cfg(unix)]
#[test]
fn committed_peer_replay_still_rejects_bytes_beyond_the_declared_payload() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    let remote = world.remote.to_str().unwrap();
    let completed = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(completed.status.success());
    let completed: Value = serde_json::from_slice(&completed.stdout).unwrap();
    let session = completed["session_id"].as_str().unwrap();
    let session_root = world.local.join(".lwc/sync").join(session);
    let state: Value =
        serde_json::from_slice(&fs::read(session_root.join("state.json")).unwrap()).unwrap();
    let payload = fs::read(session_root.join("project/merged.db")).unwrap();
    let request = serde_json::json!({
        "protocol": 1,
        "action": "publish",
        "session_id": session,
        "scope": "project",
        "directory": world.remote,
        "store_scope": "project",
        "payload_size": payload.len(),
        "state_digest": state["units"][0]["staged_digest"],
        "expected": state["peer_stores"][0]["identity"],
        "requester_store_id": state["units"][0]["local_identity"]["store_id"],
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.remote)
        .env("HOME", &world.remote_home)
        .env("LWC_PROJECT_ROOT", &world.remote)
        .arg("__sync-peer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{request}").unwrap();
    stdin.write_all(&payload).unwrap();
    stdin.write_all(b"x").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_protocol_invalid");
}

#[cfg(unix)]
#[test]
fn pull_safely_creates_a_missing_local_project_store_after_staging() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.remote, "remote-note", "Remote", "remote");
    fs::rename(
        world.local.join(".lwc"),
        world.local.join(".lwc-before-sync"),
    )
    .unwrap();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "completed");
    assert!(world.local.join(".lwc/wiki.db").is_file());
    assert!(world.has_page(&world.local, "remote-note"));

    let session = result["session_id"].as_str().unwrap();
    let state_path = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["phase"] = Value::String("publishing".to_string());
    state["units"][0]["published_local"] = Value::Bool(false);
    state["units"][0]["ending_local"] = Value::Null;
    state["units"][0]["publication_local"] = Value::Null;
    fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    let abort = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
        "--abort",
        session,
    ]);
    assert!(!abort.status.success());
    let error: Value = serde_json::from_slice(&abort.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_partially_applied");

    let resumed = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
        "--resume",
        session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    assert_eq!(resumed["stores"][0]["publication_local"]["recovered"], true);
}

#[cfg(unix)]
#[test]
fn push_from_a_missing_local_store_is_an_explicit_no_op() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.remote, "remote-note", "Remote", "remote");
    fs::rename(
        world.local.join(".lwc"),
        world.local.join(".lwc-before-sync"),
    )
    .unwrap();
    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "push",
    ]);
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["action"], "completed");
    assert_eq!(result["skipped"], "local_store_missing");
    assert!(world.has_page(&world.remote, "remote-note"));
    assert!(!world.local.join(".lwc/wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn push_safely_creates_a_missing_remote_project_store_after_staging() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    fs::rename(
        world.remote.join(".lwc"),
        world.remote.join(".lwc-before-sync"),
    )
    .unwrap();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "push",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.remote.join(".lwc/wiki.db").is_file());
    assert!(world.has_page(&world.remote, "local-note"));
}

#[cfg(unix)]
#[test]
fn pull_from_a_missing_remote_preserves_local_without_creating_remote_canonical_state() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    fs::rename(
        world.remote.join(".lwc"),
        world.remote.join(".lwc-before-sync"),
    )
    .unwrap();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.has_page(&world.local, "local-note"));
    assert!(!world.remote.join(".lwc/wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn merge_safely_creates_a_missing_remote_project_store() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "local-note", "Local", "local");
    fs::rename(
        world.remote.join(".lwc"),
        world.remote.join(".lwc-before-sync"),
    )
    .unwrap();

    let output = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.has_page(&world.remote, "local-note"));

    let repeated = world.sync(&[
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(
        repeated["stores"][0]["transfer_kind"], "delta",
        "{repeated}"
    );
}

#[cfg(unix)]
#[test]
fn merge_safely_creates_a_missing_local_global_store() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.ok_home(
        &world.remote,
        &world.remote_home,
        &["--scope", "global", "init"],
    );
    world.put_page_home(
        &world.remote,
        &world.remote_home,
        true,
        "remote-global",
        "Remote Global",
        "remote global",
    );

    let output = world.sync(&[
        "--scope",
        "global",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "merge",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.local_home.join(".lwc/wiki.db").is_file());
}

#[cfg(unix)]
#[test]
fn push_safely_creates_a_missing_remote_global_store_after_staging() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.ok_home(
        &world.local,
        &world.local_home,
        &["--scope", "global", "init"],
    );
    world.put_page_home(
        &world.local,
        &world.local_home,
        true,
        "global-note",
        "Global",
        "global",
    );

    let output = world.sync(&[
        "--scope",
        "global",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "push",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(world.remote_home.join(".lwc/wiki.db").is_file());
    assert!(
        world
            .command_home(
                &world.remote,
                &world.remote_home,
                &["--scope", "global", "page", "show", "global-note"],
            )
            .status
            .success()
    );
}

#[cfg(unix)]
#[test]
fn all_scope_pull_includes_a_remote_only_global_store() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.ok_home(
        &world.remote,
        &world.remote_home,
        &["--scope", "global", "init"],
    );
    world.put_page_home(
        &world.remote,
        &world.remote_home,
        true,
        "remote-global",
        "Remote Global",
        "remote global",
    );

    let output = world.sync(&[
        "--scope",
        "all",
        "sync",
        "peer",
        world.remote.to_str().unwrap(),
        "--mode",
        "pull",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["stores"].as_array().unwrap().len(), 2);
    assert!(world.local_home.join(".lwc/wiki.db").is_file());
}

#[cfg(unix)]
#[test]
fn all_scope_resume_repairs_a_monotonic_predecessor_receipt() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    for (cwd, home) in [
        (&world.local, &world.local_home),
        (&world.remote, &world.remote_home),
    ] {
        world.ok_home(cwd, home, &["--scope", "global", "init"]);
    }
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["--scope", "all", "sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let session = first["session_id"].as_str().unwrap();
    let global_state = world
        .local_home
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&global_state).unwrap()).unwrap();
    let mut newest = state.clone();
    let revision = state["state_revision"].as_u64().unwrap();
    state["state_revision"] = serde_json::json!(revision - 1);
    state["updated_at_unix_ms"] = serde_json::json!(1);
    state["units"][0]["published_local"] = Value::Bool(false);
    let older_bytes = serde_json::to_vec_pretty(&state).unwrap();
    newest["peer_stores"][0]["identity"]["revision"] = Value::String("f".repeat(64));
    newest["previous_state_digest"] = Value::String(
        Sha256::digest(&older_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    let project_state = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    fs::write(&global_state, older_bytes).unwrap();
    fs::write(&project_state, serde_json::to_vec_pretty(&newest).unwrap()).unwrap();

    let resumed = world.sync(&[
        "--scope", "all", "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    assert_eq!(resumed["idempotent"], true);
    assert_eq!(
        fs::read(&global_state).unwrap(),
        fs::read(&project_state).unwrap()
    );

    let mut tampered: Value = serde_json::from_slice(&fs::read(&global_state).unwrap()).unwrap();
    tampered["state_revision"] =
        serde_json::json!(tampered["state_revision"].as_u64().unwrap() - 1);
    tampered["units"][0]["artifact_digest"] = Value::String("tampered".to_string());
    fs::write(&global_state, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let rejected = world.sync(&[
        "--scope", "all", "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(!rejected.status.success());
    let error: Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "sync_state_conflict");
}

#[cfg(unix)]
#[test]
fn all_scope_resume_recovers_when_one_state_copy_is_missing() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    for (cwd, home) in [
        (&world.local, &world.local_home),
        (&world.remote, &world.remote_home),
    ] {
        world.ok_home(cwd, home, &["--scope", "global", "init"]);
    }
    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["--scope", "all", "sync", "peer", remote, "--mode", "merge"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let session = first["session_id"].as_str().unwrap();
    let global_state = world
        .local_home
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    let project_state = world
        .local
        .join(".lwc/sync")
        .join(session)
        .join("state.json");
    fs::remove_file(&global_state).unwrap();

    let resumed = world.sync(&[
        "--scope", "all", "sync", "peer", remote, "--mode", "merge", "--resume", session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        fs::read(&global_state).unwrap(),
        fs::read(&project_state).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn derived_failure_records_committed_receipt_and_resume_does_not_republish() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.remote, "remote-note", "Remote", "remote");
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(world.local.join(".lwc/.lwc-artifacts-lock"))
        .unwrap();
    lock.lock().unwrap();

    let remote = world.remote.to_str().unwrap();
    let first = world.sync(&["sync", "peer", remote, "--mode", "pull"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["action"], "partially_applied");
    assert_eq!(first["stores"][0]["published_local"], true);
    assert_eq!(first["stores"][0]["derived_local"]["status"], "failed");
    assert!(world.has_page(&world.local, "remote-note"));
    let session = first["session_id"].as_str().unwrap();
    let local_db = world.local.join(".lwc/wiki.db");
    let before: i64 = Connection::open(&local_db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    lock.unlock().unwrap();
    drop(lock);

    let resumed = world.sync(&[
        "sync", "peer", remote, "--mode", "pull", "--resume", session,
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["action"], "completed");
    let after: i64 = Connection::open(local_db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[cfg(unix)]
#[test]
fn directional_baselines_do_not_turn_a_later_one_sided_edit_into_a_conflict() {
    let world = SyncWorld::new();
    world.install_fake_ssh();
    world.put_page(&world.local, "guide", "Base", "base");
    world.put_page(&world.remote, "guide", "Base", "base");
    let remote = world.remote.to_str().unwrap();
    let paired = world.sync(&["sync", "peer", remote, "--mode", "merge"]);
    assert!(
        paired.status.success(),
        "{}",
        String::from_utf8_lossy(&paired.stderr)
    );

    world.put_page(&world.local, "guide", "Local", "local");
    let pulled = world.sync(&["sync", "peer", remote, "--mode", "pull"]);
    assert!(
        pulled.status.success(),
        "{}",
        String::from_utf8_lossy(&pulled.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&pulled.stdout).unwrap()["action"],
        "completed"
    );

    world.put_page(&world.remote, "guide", "Remote", "remote");
    let pushed = world.sync(&["sync", "peer", remote, "--mode", "push"]);
    assert!(
        pushed.status.success(),
        "{}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    let pushed: Value = serde_json::from_slice(&pushed.stdout).unwrap();
    assert_eq!(pushed["action"], "completed", "{pushed}");
    assert_eq!(
        world.ok(&world.remote, &["page", "show", "guide"])["page"]["title"],
        "Remote"
    );
}

#[cfg(unix)]
#[test]
fn sync_modes_transport_detached_drafts_and_terminal_audits_without_active_work() {
    for mode in ["pull", "push", "merge"] {
        let world = SyncWorld::new();
        world.install_fake_ssh();
        let (origin, origin_home, target, target_home) = if mode == "push" {
            (
                &world.local,
                &world.local_home,
                &world.remote,
                &world.remote_home,
            )
        } else {
            (
                &world.remote,
                &world.remote_home,
                &world.local,
                &world.local_home,
            )
        };
        let origin_database = origin.join(".lwc/wiki.db");
        world.put_draft_page(origin, origin_home, "handoff", "detached-page");
        let origin_draft = world.ok_home(origin, origin_home, &["changeset", "show", "handoff"]);

        let terminal_id = "c".repeat(64);
        let queued_id = "d".repeat(64);
        let running_id = "e".repeat(64);
        write_sync_work_fixture(origin, &origin_database, &terminal_id, "succeeded");
        write_sync_work_fixture(origin, &origin_database, &queued_id, "queued");
        write_sync_work_fixture(origin, &origin_database, &running_id, "running");

        let remote = world.remote.to_str().unwrap();
        let output = world.sync(&["sync", "peer", remote, "--mode", mode]);
        assert!(
            output.status.success(),
            "{mode} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["action"], "completed", "{mode}: {result}");
        let continuity_side = if mode == "push" {
            "continuity_remote"
        } else {
            "continuity_local"
        };
        let continuity = &result["stores"][0][continuity_side];
        assert_eq!(continuity["status"], "completed", "{mode}: {continuity}");
        assert_eq!(continuity["terminal_work"], "audited");
        assert_eq!(continuity["active_work"], "local_only");
        let replay_name = continuity["drafts"][0]["name"].as_str().unwrap();
        let replay_id = continuity["drafts"][0]["changeset_id"].as_str().unwrap();
        assert_ne!(replay_id, origin_draft["changeset_id"].as_str().unwrap());

        let shown = world.ok_home(target, target_home, &["changeset", "show", replay_name]);
        assert_eq!(shown["status"], "draft", "{mode}: {shown}");
        assert_eq!(shown["empty"], false);
        let staged = world.ok_home(
            target,
            target_home,
            &["--changeset", replay_name, "page", "show", "detached-page"],
        );
        assert_eq!(staged["page"]["body"], "detached draft knowledge");
        let replay_source_id = staged["page"]["source_ids"][0]
            .as_i64()
            .unwrap()
            .to_string();
        let replay_source = world.ok_home(
            target,
            target_home,
            &[
                "--changeset",
                replay_name,
                "source",
                "show",
                &replay_source_id,
            ],
        );
        assert_eq!(
            replay_source["source"]["content"],
            "portable detached source bytes"
        );
        assert!(
            !world
                .command_home(target, target_home, &["page", "show", "detached-page"])
                .status
                .success(),
            "{mode} must not commit detached draft content to live"
        );

        let target_work = target.join(".lwc/work");
        assert!(
            !target_work.join(&queued_id).exists(),
            "{mode} copied queued Work"
        );
        assert!(
            !target_work.join(&running_id).exists(),
            "{mode} copied running Work"
        );
        assert!(
            !target_work.join(&terminal_id).exists(),
            "{mode} copied raw terminal Work"
        );

        let target_database = target.join(".lwc/wiki.db");
        let audit: String = Connection::open(&target_database)
            .unwrap()
            .query_row(
                "SELECT detail_json FROM operations
                 WHERE action='sync_work_audit'
                   AND json_extract(detail_json,'$.origin_work_id')=?1",
                [&terminal_id],
                |row| row.get(0),
            )
            .unwrap();
        let audit: Value = serde_json::from_str(&audit).unwrap();
        assert_eq!(audit["origin_work_id"], terminal_id);
        assert_eq!(audit["state"], "succeeded");
        for redacted in [
            "database", "scope", "pid", "message", "result", "error", "active",
        ] {
            assert!(
                audit.get(redacted).is_none(),
                "{mode} leaked {redacted}: {audit}"
            );
        }

        if mode == "pull" {
            let session = result["session_id"].as_str().unwrap();
            let state_path = world
                .local
                .join(".lwc/sync")
                .join(session)
                .join("state.json");
            let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
            state["phase"] = Value::String("publishing".into());
            state["units"][0]["continuity_local"] = Value::Bool(false);
            state["units"][0]["derived_local"] = Value::Bool(false);
            fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
            let resumed = world.sync(&[
                "sync", "peer", remote, "--mode", "pull", "--resume", session,
            ]);
            assert!(
                resumed.status.success(),
                "resume failed: {}",
                String::from_utf8_lossy(&resumed.stderr)
            );
            let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
            assert_eq!(resumed["action"], "completed");
            let resumed_draft = &resumed["stores"][0]["continuity_local"]["drafts"][0];
            assert_eq!(resumed_draft["changeset_id"], replay_id);
            let replay_count =
                world.ok_home(target, target_home, &["changeset", "list"])["changesets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|draft| draft["name"] == replay_name)
                    .count();
            assert_eq!(replay_count, 1, "resume duplicated the replay draft");
            let audit_count: i64 = Connection::open(&target_database)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM operations
                     WHERE action='sync_work_audit'
                       AND json_extract(detail_json,'$.origin_work_id')=?1",
                    [&terminal_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(audit_count, 1, "resume duplicated the terminal audit");
        }
    }
}
