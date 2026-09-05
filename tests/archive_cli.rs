use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct World {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn project(&self, name: &str, initialize: bool) -> PathBuf {
        let project = self.root.join(name);
        fs::create_dir_all(&project).unwrap();
        if initialize {
            self.ok(&project, &["init"]);
        }
        project
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command_with_env(cwd, args, &[])
    }

    fn command_with_env(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("LWC_PROJECT_ROOT", cwd)
            .envs(env.iter().copied())
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

    fn error(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.command(cwd, args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn put_page(&self, project: &Path, slug: &str, title: &str, body: &str) {
        let input = project.join(format!("{slug}.input.md"));
        fs::write(&input, body).unwrap();
        self.ok(
            project,
            &[
                "page",
                "put",
                slug,
                "--title",
                title,
                "--file",
                input.to_str().unwrap(),
                "--provenance",
                "agent-observed",
            ],
        );
        fs::remove_file(input).unwrap();
    }

    fn has_page(&self, project: &Path, slug: &str) -> bool {
        self.command(project, &["page", "show", slug])
            .status
            .success()
    }

    fn compress(&self, project: &Path, output: &Path) -> Value {
        self.ok(project, &["compress", output.to_str().unwrap()])
    }
}

fn resolution_packet(conflicts: &[Value]) -> Value {
    serde_json::json!({
        "version":1,
        "decisions":conflicts.iter().map(|conflict| serde_json::json!({
            "conflict_id":conflict["conflict_id"],
            "kind":conflict["kind"],
            "logical_key":conflict["logical_key"],
            "strategy":"preserve_both",
        })).collect::<Vec<_>>()
    })
}

fn archive_plan(world: &World, source: &Path) -> Value {
    world.ok(source, &["config", "set", "--plan", "enabled"]);
    world.ok(
        source,
        &[
            "plan",
            "create",
            "Release history",
            "--objective",
            "Preserve history",
            "--done-when",
            "Verified",
            "--step",
            "Publish",
            "--step",
            "Verify",
        ],
    )["plan"]
        .clone()
}

#[test]
fn archive_round_trip_and_merge_preserve_abandoned_plan_history() {
    for blocked in [false, true] {
        let world = World::new();
        let source = world.project("source", true);
        let plan = archive_plan(&world, &source);
        let id = plan["id"].as_str().unwrap();
        if blocked {
            world.ok(
                &source,
                &[
                    "plan",
                    "block",
                    id,
                    "--if-revision",
                    "1",
                    "--step",
                    plan["steps"][0]["id"].as_str().unwrap(),
                    "--reason",
                    "Failed tag",
                ],
            );
        }
        world.ok(
            &source,
            &[
                "plan",
                "abandon",
                id,
                "--if-revision",
                if blocked { "2" } else { "1" },
                "--reason",
                "Keep immutable failed release history",
            ],
        );
        let archive = world.root.join("history.lwc.zst");
        let original = world.compress(&source, &archive);
        let target = world.project("imported", false);
        assert_eq!(
            world.ok(&target, &["decompress", archive.to_str().unwrap()])["committed"],
            true
        );
        let exported = world.compress(&target, &world.root.join("roundtrip.lwc.zst"));
        assert_eq!(original["state_digest"], exported["state_digest"]);

        let merged = world.project("merged", true);
        world.put_page(&merged, "local-note", "Local", "Retain local memory");
        assert_eq!(
            world.ok(&merged, &["merge", archive.to_str().unwrap()])["committed"],
            true
        );
        assert!(world.has_page(&merged, "local-note"));
        for project in [&source, &target, &merged] {
            let db = Connection::open(project.join(".lwc/wiki.db")).unwrap();
            let preserved: (String, String, String) = db.query_row(
                "SELECT p.state,p.abandoned_reason,s.status FROM plans p JOIN plan_steps s ON s.plan_id=p.id WHERE p.id=?1 AND s.ordinal=0",
                [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).unwrap();
            assert_eq!(
                preserved,
                (
                    "abandoned".into(),
                    "Keep immutable failed release history".into(),
                    if blocked { "blocked" } else { "in_progress" }.into(),
                )
            );
        }
    }
}

#[test]
fn archive_round_trip_accepts_active_plan_awaiting_completion() {
    let world = World::new();
    let source = world.project("source", true);
    let plan = archive_plan(&world, &source);
    let id = plan["id"].as_str().unwrap();
    world.ok(
        &source,
        &[
            "plan",
            "advance",
            id,
            "--if-revision",
            "1",
            "--done",
            plan["steps"][0]["id"].as_str().unwrap(),
            "--next",
            plan["steps"][1]["id"].as_str().unwrap(),
            "--result",
            "Published",
        ],
    );
    world.ok(
        &source,
        &[
            "plan",
            "advance",
            id,
            "--if-revision",
            "2",
            "--done",
            plan["steps"][1]["id"].as_str().unwrap(),
            "--result",
            "Verified",
        ],
    );
    let archive = world.root.join("awaiting-completion.lwc.zst");
    let original = world.compress(&source, &archive);
    let target = world.project("target", false);
    world.ok(&target, &["decompress", archive.to_str().unwrap()]);
    assert_eq!(
        original["state_digest"],
        world.compress(&target, &world.root.join("again.lwc.zst"))["state_digest"]
    );

    world.ok(
        &source,
        &[
            "plan",
            "complete",
            id,
            "--if-revision",
            "3",
            "--result",
            "Verified",
            "--evidence",
            "Both steps finished",
            "--done-when-checked",
        ],
    );
    let completed_archive = world.root.join("completed.lwc.zst");
    let completed = world.compress(&source, &completed_archive);
    let completed_target = world.project("completed-target", false);
    world.ok(
        &completed_target,
        &["decompress", completed_archive.to_str().unwrap()],
    );
    assert_eq!(
        completed["state_digest"],
        world.compress(
            &completed_target,
            &world.root.join("completed-again.lwc.zst")
        )["state_digest"]
    );
}

#[test]
fn archive_rejects_invalid_plan_steps_without_publishing() {
    for (state, first, second) in [
        ("active", "pending", "pending"),
        ("active", "in_progress", "blocked"),
        ("abandoned", "in_progress", "blocked"),
        ("completed", "in_progress", "completed"),
        ("completed", "completed", "pending"),
        ("active", "", ""),
        ("abandoned", "", ""),
        ("completed", "", ""),
    ] {
        let world = World::new();
        let source = world.project("source", true);
        archive_plan(&world, &source);
        let db = Connection::open(source.join(".lwc/wiki.db")).unwrap();
        db.execute("UPDATE plans SET state=?1", [state]).unwrap();
        if first.is_empty() {
            db.execute("DELETE FROM plan_steps", []).unwrap();
        } else {
            db.execute(
                "UPDATE plan_steps SET status=CASE ordinal WHEN 0 THEN ?1 ELSE ?2 END",
                [first, second],
            )
            .unwrap();
        }
        drop(db);
        let archive = world.root.join("invalid.lwc.zst");
        world.compress(&source, &archive);
        let target = world.project("target", false);
        let error = world.error(&target, &["decompress", archive.to_str().unwrap()]);
        assert_eq!(error["error"]["code"], "sync_state_invalid");
        assert!(!target.join(".lwc/wiki.db").exists());
    }
}

#[test]
fn compress_help_default_and_custom_round_trip_preserve_memory_without_source_paths() {
    let world = World::new();
    for command in ["compress", "decompress", "merge"] {
        let help = world.command(&world.root, &[command, "--help"]);
        assert!(
            help.status.success(),
            "missing `{command}` command: {}",
            String::from_utf8_lossy(&help.stderr)
        );
    }

    let source = world.project("source", true);
    let external = world.root.join("private-source-路径.md");
    fs::write(&external, "portable source 正文\n").unwrap();
    world.ok(
        &source,
        &[
            "source",
            "add",
            external.to_str().unwrap(),
            "--title",
            "Portable source",
            "--allow-external-source",
        ],
    );
    world.put_page(&source, "portable-note", "可移植记忆", "正文与 emoji 🧠");

    let default = world.ok(&source, &["compress"]);
    let default_path = source.join(".lwc/memory.lwc.zst");
    assert!(default_path.is_file());
    assert_eq!(default["action"], "compressed");
    assert_eq!(default["scope"], "project");
    assert!(default["state_digest"].as_str().is_some());
    assert!(default["payload_sha256"].as_str().is_some());
    let warning = default["warning"].as_str().unwrap();
    assert!(warning.contains("plaintext") && warning.contains("trusted"));

    let custom = world.root.join("portable-custom.lwc.zst");
    world.compress(&source, &custom);
    assert!(custom.is_file());

    let target = world.project("missing-target", false);
    let imported = world.ok(&target, &["decompress", custom.to_str().unwrap()]);
    assert_eq!(imported["action"], "completed");
    assert_eq!(imported["scope"], "project");
    assert_eq!(imported["committed"], true);
    let session = imported["session_id"].as_str().unwrap();
    let session_directory = target.join(".lwc/imports").join(session);
    assert!(session_directory.join("merged.db").is_file());
    assert!(!session_directory.join("incoming.db").exists());
    assert!(target.join(".lwc/wiki.db").is_file());
    let page = world.ok(&target, &["page", "show", "portable-note"]);
    assert_eq!(page["page"]["title"], "可移植记忆");
    assert_eq!(page["page"]["body"], "正文与 emoji 🧠");

    let database = Connection::open(target.join(".lwc/wiki.db")).unwrap();
    let source_content: String = database
        .query_row(
            "SELECT content FROM sources WHERE title='Portable source'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_content, "portable source 正文\n");
    let serialized_paths: String = database
        .query_row(
            "SELECT COALESCE(GROUP_CONCAT(origin, '\n'), '') FROM sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!serialized_paths.contains(external.to_str().unwrap()));
    let tracked_paths: i64 = database
        .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(tracked_paths, 0);

    let all = world.error(&source, &["--scope", "all", "compress"]);
    assert_eq!(all["error"]["code"], "archive_scope_unsupported");
}

#[test]
fn global_compress_and_missing_global_decompress_use_the_global_runtime() {
    let source = World::new();
    source.ok(&source.root, &["--scope", "global", "init"]);
    let input = source.root.join("global-note.md");
    fs::write(&input, "global portable memory").unwrap();
    source.ok(
        &source.root,
        &[
            "--scope",
            "global",
            "page",
            "put",
            "global-note",
            "--title",
            "Global note",
            "--file",
            input.to_str().unwrap(),
            "--provenance",
            "agent-observed",
        ],
    );
    let compressed = source.ok(&source.root, &["--scope", "global", "compress"]);
    let archive = source.home.join(".lwc/memory.lwc.zst");
    assert_eq!(compressed["scope"], "global");
    assert!(archive.is_file());

    let target = World::new();
    let imported = target.ok(
        &target.root,
        &["--scope", "global", "decompress", archive.to_str().unwrap()],
    );
    assert_eq!(imported["action"], "completed");
    assert_eq!(imported["scope"], "global");
    assert!(target.home.join(".lwc/wiki.db").is_file());
    let page = target.ok(
        &target.root,
        &["--scope", "global", "page", "show", "global-note"],
    );
    assert_eq!(page["page"]["body"], "global portable memory");
}

#[test]
fn decompress_existing_different_store_only_stages_and_returns_exact_resume() {
    let world = World::new();
    let source = world.project("source", true);
    world.put_page(&source, "incoming-only", "Incoming", "incoming body");
    let archive = world.root.join("incoming.lwc.zst");
    world.compress(&source, &archive);

    let target = world.project("target", true);
    world.put_page(&target, "local-only", "Local", "local body");
    let before = fs::read(target.join(".lwc/wiki.db")).unwrap();
    let staged = world.ok(&target, &["decompress", archive.to_str().unwrap()]);
    assert_eq!(staged["action"], "staged");
    let session = staged["session_id"].as_str().unwrap();
    assert_eq!(staged["committed"], false);
    assert_eq!(
        staged["next_action"],
        format!("lwc merge --resume {session}")
    );
    assert!(target.join(".lwc/imports").join(session).is_dir());
    assert_eq!(fs::read(target.join(".lwc/wiki.db")).unwrap(), before);
    assert!(world.has_page(&target, "local-only"));
    assert!(!world.has_page(&target, "incoming-only"));
}

#[cfg(unix)]
#[test]
fn archive_decode_rejects_wrong_scope_corruption_and_symlinks_without_mutation() {
    use std::os::unix::fs::symlink;

    let world = World::new();
    let source = world.project("source", true);
    world.put_page(&source, "incoming", "Incoming", "incoming");
    let archive = world.root.join("valid.lwc.zst");
    world.compress(&source, &archive);

    world.ok(&source, &["--scope", "global", "init"]);
    let wrong_scope = world.error(
        &source,
        &["--scope", "global", "decompress", archive.to_str().unwrap()],
    );
    assert_eq!(wrong_scope["error"]["code"], "archive_scope_mismatch");

    let target = world.project("target", true);
    world.put_page(&target, "sentinel", "Sentinel", "must survive");
    let before = fs::read(target.join(".lwc/wiki.db")).unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    bytes[0] ^= 0xff;
    let corrupt = world.root.join("corrupt.lwc.zst");
    fs::write(&corrupt, bytes).unwrap();
    let invalid = world.error(&target, &["decompress", corrupt.to_str().unwrap()]);
    assert_eq!(invalid["error"]["code"], "archive_invalid");

    let linked_input = world.root.join("linked-input.lwc.zst");
    symlink(&archive, &linked_input).unwrap();
    let unsafe_input = world.error(&target, &["decompress", linked_input.to_str().unwrap()]);
    assert_eq!(unsafe_input["error"]["code"], "archive_unsafe_path");

    let protected = world.root.join("protected.txt");
    fs::write(&protected, "keep").unwrap();
    let linked_output = world.root.join("linked-output.lwc.zst");
    symlink(&protected, &linked_output).unwrap();
    let unsafe_output = world.error(&source, &["compress", linked_output.to_str().unwrap()]);
    assert_eq!(unsafe_output["error"]["code"], "archive_unsafe_path");
    assert_eq!(fs::read_to_string(protected).unwrap(), "keep");
    assert_eq!(fs::read(target.join(".lwc/wiki.db")).unwrap(), before);
    assert!(fs::read_dir(target.join(".lwc/imports")).is_err());
}

#[cfg(unix)]
#[test]
fn archive_rejects_input_and_output_reached_through_an_ancestor_symlink() {
    use std::os::unix::fs::symlink;

    let world = World::new();
    let source = world.project("source", true);
    world.put_page(&source, "incoming", "Incoming", "incoming");

    let real_input = world.root.join("real-input");
    fs::create_dir(&real_input).unwrap();
    let archive = real_input.join("memory.lwc.zst");
    world.compress(&source, &archive);
    let linked_input_directory = world.root.join("linked-input-directory");
    symlink(&real_input, &linked_input_directory).unwrap();

    let target = world.project("target", true);
    world.put_page(&target, "sentinel", "Sentinel", "must survive");
    let before = fs::read(target.join(".lwc/wiki.db")).unwrap();
    let unsafe_input = world.error(
        &target,
        &[
            "decompress",
            linked_input_directory
                .join("memory.lwc.zst")
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(unsafe_input["error"]["code"], "archive_unsafe_path");
    assert_eq!(fs::read(target.join(".lwc/wiki.db")).unwrap(), before);

    let real_output = world.root.join("real-output");
    fs::create_dir(&real_output).unwrap();
    let protected = real_output.join("memory.lwc.zst");
    fs::write(&protected, "protected output").unwrap();
    let linked_output_directory = world.root.join("linked-output-directory");
    symlink(&real_output, &linked_output_directory).unwrap();
    let unsafe_output = world.error(
        &source,
        &[
            "compress",
            linked_output_directory
                .join("memory.lwc.zst")
                .to_str()
                .unwrap(),
        ],
    );
    assert_eq!(unsafe_output["error"]["code"], "archive_unsafe_path");
    assert_eq!(fs::read_to_string(protected).unwrap(), "protected output");
}

#[test]
fn merge_preserves_unique_objects_and_resumes_bounded_conflicts() {
    let world = World::new();
    let incoming = world.project("incoming", true);
    world.put_page(
        &incoming,
        "incoming-only",
        "Incoming only",
        "incoming unique",
    );
    for index in 0..21 {
        world.put_page(
            &incoming,
            &format!("conflict-{index:02}"),
            "Incoming",
            &format!("incoming {index}"),
        );
    }
    let archive = world.root.join("merge.lwc.zst");
    world.compress(&incoming, &archive);

    let target = world.project("target", true);
    world.put_page(&target, "local-only", "Local only", "local unique");
    for index in 0..21 {
        world.put_page(
            &target,
            &format!("conflict-{index:02}"),
            "Local",
            &format!("local {index}"),
        );
    }

    let first = world.ok(&target, &["merge", archive.to_str().unwrap()]);
    assert_eq!(first["action"], "conflicts");
    let session = first["session_id"].as_str().unwrap();
    assert!(
        target
            .join(".lwc/imports")
            .join(session)
            .join("incoming.db")
            .is_file()
    );
    let first_batch = first["conflicts"].as_array().unwrap();
    assert_eq!(first_batch.len(), 20);
    assert_eq!(
        first["next_action"],
        format!("lwc merge --resume {session} --resolve PACKET")
    );
    assert!(world.has_page(&target, "local-only"));
    assert!(!world.has_page(&target, "incoming-only"));

    let first_packet = target.join("resolve-1.json");
    fs::write(&first_packet, resolution_packet(first_batch).to_string()).unwrap();
    let second = world.ok(
        &target,
        &[
            "merge",
            "--resume",
            session,
            "--resolve",
            first_packet.to_str().unwrap(),
        ],
    );
    assert_eq!(second["action"], "conflicts");
    let second_batch = second["conflicts"].as_array().unwrap();
    assert_eq!(second_batch.len(), 1);

    let second_packet = target.join("resolve-2.json");
    fs::write(&second_packet, resolution_packet(second_batch).to_string()).unwrap();
    let completed = world.ok(
        &target,
        &[
            "merge",
            "--resume",
            session,
            "--resolve",
            second_packet.to_str().unwrap(),
        ],
    );
    assert_eq!(completed["action"], "completed");
    assert_eq!(completed["committed"], true);
    assert!(world.has_page(&target, "local-only"));
    assert!(world.has_page(&target, "incoming-only"));
}

#[test]
fn overwrite_requires_bound_confirmation_and_rejects_stale_target_identity() {
    let world = World::new();
    let incoming = world.project("incoming", true);
    world.put_page(&incoming, "incoming", "Incoming", "incoming");
    let archive = world.root.join("overwrite.lwc.zst");
    world.compress(&incoming, &archive);

    let target = world.project("target", true);
    world.put_page(&target, "local", "Local", "local");
    let consent = world.ok(
        &target,
        &["decompress", archive.to_str().unwrap(), "--overwrite"],
    );
    assert_eq!(consent["action"], "confirmation_required");
    assert_eq!(consent["requires_consent"], true);
    let stale_token = consent["confirmation_token"].as_str().unwrap();
    assert!(world.has_page(&target, "local"));
    assert!(!world.has_page(&target, "incoming"));

    world.put_page(&target, "concurrent", "Concurrent", "new local write");
    let stale = world.error(
        &target,
        &[
            "decompress",
            archive.to_str().unwrap(),
            "--overwrite",
            "--confirm-overwrite",
            stale_token,
        ],
    );
    assert_eq!(stale["error"]["code"], "archive_confirmation_stale");
    assert!(world.has_page(&target, "concurrent"));
    assert!(!world.has_page(&target, "incoming"));

    let refreshed = world.ok(
        &target,
        &["decompress", archive.to_str().unwrap(), "--overwrite"],
    );
    let token = refreshed["confirmation_token"].as_str().unwrap();
    let completed = world.ok(
        &target,
        &[
            "decompress",
            archive.to_str().unwrap(),
            "--overwrite",
            "--confirm-overwrite",
            token,
        ],
    );
    assert_eq!(completed["action"], "completed");
    assert_eq!(completed["committed"], true);
    let session = completed["session_id"].as_str().unwrap();
    let session_directory = target.join(".lwc/imports").join(session);
    assert!(session_directory.join("merged.db").is_file());
    assert!(!session_directory.join("incoming.db").exists());
    assert!(world.has_page(&target, "incoming"));
    assert!(!world.has_page(&target, "local"));
}

#[test]
fn merge_rejects_a_target_write_after_staging_instead_of_overwriting_it() {
    let world = World::new();
    let incoming = world.project("incoming", true);
    world.put_page(&incoming, "incoming", "Incoming", "incoming");
    let archive = world.root.join("cas.lwc.zst");
    world.compress(&incoming, &archive);

    let target = world.project("target", true);
    world.put_page(&target, "local", "Local", "local");
    let staged = world.ok(&target, &["decompress", archive.to_str().unwrap()]);
    assert_eq!(staged["action"], "staged");
    let session = staged["session_id"].as_str().unwrap();
    world.put_page(&target, "concurrent", "Concurrent", "must survive");

    let error = world.error(&target, &["merge", "--resume", session]);
    assert_eq!(error["error"]["code"], "archive_store_changed");
    assert!(world.has_page(&target, "concurrent"));
    assert!(!world.has_page(&target, "incoming"));
}

#[test]
#[cfg(debug_assertions)]
fn committed_merge_resumes_rebuild_without_republishing() {
    let world = World::new();
    let incoming = world.project("incoming", true);
    world.put_page(&incoming, "incoming", "Incoming", "incoming");
    let archive = world.root.join("recover.lwc.zst");
    world.compress(&incoming, &archive);

    let target = world.project("target", true);
    let output = world.command_with_env(
        &target,
        &["merge", archive.to_str().unwrap()],
        &[("LWC_TEST_ARCHIVE_FAIL_AFTER_COMMIT", "1")],
    );
    assert!(!output.status.success());
    let failure: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(failure["error"]["details"]["committed"], true);
    let session = failure["error"]["details"]["session_id"].as_str().unwrap();
    assert_eq!(
        failure["error"]["details"]["next_action"],
        format!("lwc merge --resume {session}")
    );
    assert!(world.has_page(&target, "incoming"));

    let database_path = target.join(".lwc/wiki.db");
    {
        let database = Connection::open(&database_path).unwrap();
        let encoded: String = database
            .query_row(
                "SELECT detail_json FROM operations WHERE action='sync_merge' AND target=?1",
                [session],
                |row| row.get(0),
            )
            .unwrap();
        let mut receipt: Value = serde_json::from_str(&encoded).unwrap();
        receipt["derived"] = serde_json::json!({
            "status":"failed",
            "error":"injected_fts_failure",
            "committed":true,
            "next_action":"resume_derived_rebuild",
        });
        database
            .execute(
                "UPDATE operations SET detail_json=?1 WHERE action='sync_merge' AND target=?2",
                [receipt.to_string(), session.to_owned()],
            )
            .unwrap();
    }
    world.put_page(&target, "concurrent", "Concurrent", "must survive recovery");

    let resumed = world.ok(&target, &["merge", "--resume", session]);
    assert_eq!(resumed["action"], "completed");
    assert_eq!(resumed["recovered"], true);
    assert_ne!(resumed["derived"]["fts"]["status"], "failed");
    assert!(target.join(".lwc/wiki/concepts/incoming.md").is_file());
    assert!(world.has_page(&target, "incoming"));
    assert!(world.has_page(&target, "concurrent"));
    let database = Connection::open(database_path).unwrap();
    let publications: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(publications, 1);
}
