#[test]
fn changeset_stages_reads_and_discard_without_touching_live_wiki() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let before_operations: i64 = Connection::open(&database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let before_index = fs::read(world.project.join(".lwc/wiki/index.md")).unwrap();

    let begun = world.ok(&world.project, &["changeset", "begin", "ingest-42"]);
    assert_eq!(begun["status"], "draft");
    assert_eq!(begun["name"], "ingest-42");
    assert!(begun["duration_ms"].is_u64());
    let listed = world.ok(&world.project, &["changeset", "list"]);
    assert_eq!(listed["changesets"][0]["name"], "ingest-42");

    let body = world.write("draft-page.md", "draft-only unique knowledge");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "ingest-42",
            "page",
            "put",
            "draft-only",
            "--title",
            "Draft Only",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ],
    );

    let live_missing = world.err(&world.project, &["page", "show", "draft-only"]);
    assert_eq!(live_missing["error"]["code"], "page_not_found");
    let staged = world.ok(
        &world.project,
        &["--changeset", "ingest-42", "page", "show", "draft-only"],
    );
    assert_eq!(staged["page"]["body"], "draft-only unique knowledge");

    let shown = world.ok(&world.project, &["changeset", "show", "ingest-42"]);
    assert_eq!(shown["status"], "draft");
    assert_eq!(shown["empty"], false);
    assert_eq!(shown["conflict"], false);
    assert!(shown["staged_operation_count"].as_u64().unwrap() >= 1);

    let after_operations: i64 = Connection::open(&database)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after_operations, before_operations);
    assert_eq!(
        fs::read(world.project.join(".lwc/wiki/index.md")).unwrap(),
        before_index
    );

    let discarded = world.ok(&world.project, &["changeset", "discard", "ingest-42"]);
    assert_eq!(discarded["status"], "discarded");
    assert!(!world.project.join(".lwc/changesets/ingest-42.db").exists());
}

#[test]
fn changeset_begin_allocation_is_independent_of_live_wiki_size() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = PathBuf::from(initialized["database"].as_str().unwrap());
    let conn = Connection::open(&database).unwrap();
    conn.execute(
        "INSERT INTO pages(
             slug, title, kind, summary, body, structural_navigation,
             created_at, updated_at
         ) VALUES ('large-live-page', 'Large live page', 'concept', '', ?1, 0,
                   'now', 'now')",
        ["x".repeat(4 * 1024 * 1024)],
    )
    .unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(conn);

    let begun = world.ok(&world.project, &["changeset", "begin", "sparse-begin"]);
    let draft = PathBuf::from(begun["database"].as_str().unwrap());
    let allocated = [
        draft.clone(),
        PathBuf::from(format!("{}-wal", draft.display())),
        PathBuf::from(format!("{}-shm", draft.display())),
    ]
    .into_iter()
    .map(|path| fs::metadata(path).map(|value| value.len()).unwrap_or(0))
    .sum::<u64>();

    assert!(
        allocated <= 1024 * 1024,
        "empty sparse draft allocated {allocated} bytes from unrelated live content"
    );
}

#[test]
fn changeset_commit_preserves_unrelated_live_mutations() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "entity-local"]);
    let staged = world.write("staged-local.md", "staged local alpha beta");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "entity-local",
            "page",
            "put",
            "staged-local",
            "--title",
            "Staged local",
            "--summary",
            "Staged local",
            "--file",
            as_str(&staged),
            "--provenance",
            "agent-observed",
        ],
    );
    put_page(
        &world,
        "unrelated-live",
        "Unrelated live",
        "unrelated live mutation must survive",
    );

    let committed = world.ok(
        &world.project,
        &[
            "changeset",
            "commit",
            "entity-local",
            "--allow-lint-issues",
            "--reason",
            "isolated concurrency regression fixture",
        ],
    );

    assert_eq!(committed["status"], "committed");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "unrelated-live"])["page"]["body"],
        "unrelated live mutation must survive"
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "staged-local"])["page"]["body"],
        "staged local alpha beta"
    );
}

#[test]
fn concurrent_same_entity_writes_return_one_typed_conflict() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let first = world.write("same-entity-first.md", "first concurrent body alpha beta");
    let second = world.write(
        "same-entity-second.md",
        "second concurrent body gamma delta",
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let outputs = std::thread::scope(|scope| {
        [first, second]
            .into_iter()
            .map(|body| {
                let project = world.project.clone();
                let home = world.home.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    Command::new(env!("CARGO_BIN_EXE_lwc"))
                        .current_dir(project)
                        .env("HOME", home)
                        .env("LWC_TEST_PAGE_PUT_PREWRITE_DELAY_MS", "250")
                        .args([
                            "page",
                            "put",
                            "same-entity",
                            "--title",
                            "Same entity",
                            "--file",
                            as_str(&body),
                            "--provenance",
                            "agent-observed",
                        ])
                        .output()
                        .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });

    let succeeded = outputs
        .iter()
        .filter(|output| output.status.success())
        .count();
    let conflicts = outputs
        .iter()
        .filter(|output| !output.status.success())
        .map(stderr_json)
        .filter(|error| error["error"]["code"] == "entity_conflict")
        .count();
    assert_eq!(succeeded, 1, "outputs: {outputs:?}");
    assert_eq!(conflicts, 1, "outputs: {outputs:?}");
}

#[test]
fn concurrent_different_page_writes_complete_without_a_store_wide_conflict() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "parallel-pages"]);
    let outputs = thread::scope(|scope| {
        (0..4)
            .map(|index| {
                let body = world.write(
                    &format!("parallel-{index}.md"),
                    &format!("independent page {index} alpha beta gamma"),
                );
                let project = world.project.clone();
                let home = world.home.clone();
                scope.spawn(move || {
                    Command::new(env!("CARGO_BIN_EXE_lwc"))
                        .current_dir(project)
                        .env("HOME", home)
                        .env("LWC_TEST_PAGE_PUT_PREWRITE_DELAY_MS", "100")
                        .args([
                            "--changeset",
                            "parallel-pages",
                            "page",
                            "put",
                            &format!("parallel-{index}"),
                            "--title",
                            &format!("Parallel {index}"),
                            "--file",
                            as_str(&body),
                            "--provenance",
                            "agent-observed",
                        ])
                        .output()
                        .unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });

    assert!(
        outputs.iter().all(|output| output.status.success()),
        "different entity writes must serialize cleanly: {outputs:?}"
    );
}

#[test]
fn changeset_commit_is_atomic_conflict_aware_and_rollback_guarded() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "publish"]);

    let draft_body = world.write("draft-commit.md", "candidate knowledge [[candidate-index]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "publish",
            "page",
            "put",
            "candidate-page",
            "--title",
            "Candidate Page",
            "--summary",
            "Candidate knowledge",
            "--file",
            as_str(&draft_body),
            "--provenance",
            "agent-observed",
        ],
    );
    let index_body = world.write("draft-index.md", "[[candidate-page]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "publish",
            "page",
            "put",
            "candidate-index",
            "--title",
            "Candidate Index",
            "--summary",
            "Candidate navigation",
            "--file",
            as_str(&index_body),
            "--provenance",
            "agent-observed",
        ],
    );

    let committed = world.ok(&world.project, &["changeset", "commit", "publish"]);
    assert_eq!(committed["status"], "committed");
    assert_eq!(committed["materialized"], true);
    assert_eq!(committed["wal_checkpointed"], true);
    for field in [
        "duration_ms",
        "checkpoint_ms",
        "locked_publish_ms",
        "cleanup_ms",
        "materialization_ms",
    ] {
        assert!(committed[field].is_u64(), "missing commit timing {field}");
    }
    let changeset_id = committed["changeset_id"].as_str().unwrap().to_string();
    let database = world.project.join(".lwc/wiki.db");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "candidate-page"])["page"]["body"],
        "candidate knowledge [[candidate-index]]"
    );
    let (status, checkpoint, post_revision): (String, String, String) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT status, pre_commit_checkpoint, post_revision
                 FROM changesets WHERE id = ?1",
            [&changeset_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "committed");
    assert_eq!(post_revision.len(), 64);
    assert!(
        world
            .project
            .join(".lwc/checkpoints")
            .join(format!("{checkpoint}.db"))
            .is_file()
    );
    assert!(!world.project.join(".lwc/changesets/publish.db").exists());
    let retried = world.err(&world.project, &["changeset", "commit", "publish"]);
    assert_eq!(retried["error"]["code"], "changeset_not_found");

    let rolled_back = world.ok(&world.project, &["changeset", "rollback", &changeset_id]);
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(rolled_back["wal_checkpointed"], true);
    assert!(rolled_back["checkpoint"].as_str().is_some());
    for field in [
        "duration_ms",
        "checkpoint_ms",
        "locked_rollback_ms",
        "materialization_ms",
    ] {
        assert!(
            rolled_back[field].is_u64(),
            "missing rollback timing {field}"
        );
    }
    let rollback_revision = rolled_back["rollback_revision"].clone();
    assert_eq!(
        world.err(&world.project, &["page", "show", "candidate-page"])["error"]["code"],
        "page_not_found"
    );
    put_page(
        &world,
        "later-after-rollback",
        "Later After Rollback",
        "must survive an idempotent rollback retry",
    );
    let repeated = world.ok(&world.project, &["changeset", "rollback", &changeset_id]);
    assert_eq!(repeated["status"], "rolled_back");
    assert_eq!(repeated["changeset_id"], changeset_id);
    assert_eq!(repeated["rollback_revision"], rollback_revision);
    assert_eq!(
        world.ok(&world.project, &["page", "show", "later-after-rollback"])["page"]["title"],
        "Later After Rollback"
    );

    world.ok(&world.project, &["changeset", "begin", "stale"]);
    let stale_body = world.write("stale.md", "stale draft");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "stale",
            "page",
            "put",
            "stale-page",
            "--title",
            "Stale Page",
            "--file",
            as_str(&stale_body),
            "--provenance",
            "agent-observed",
        ],
    );
    put_page(&world, "stale-page", "Concurrent Live", "live wins");
    let conflict = world.err(&world.project, &["changeset", "commit", "stale"]);
    assert_eq!(conflict["error"]["code"], "changeset_conflict");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "stale-page"])["page"]["body"],
        "live wins"
    );
}

#[test]
fn changeset_commit_does_not_fall_back_to_an_older_commit_with_a_reused_name() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "reused", "first-version");
    let first = world.ok(&world.project, &["changeset", "commit", "reused"]);

    stage_clean_linked_pages(&world, "reused", "second-version");
    let second = world.ok(&world.project, &["changeset", "commit", "reused"]);
    assert_ne!(first["changeset_id"], second["changeset_id"]);
    world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            second["changeset_id"].as_str().unwrap(),
        ],
    );

    let error = world.err(&world.project, &["changeset", "commit", "reused"]);
    assert_eq!(error["error"]["code"], "changeset_not_found");
}

#[test]
fn changeset_rollback_preserves_an_unrelated_later_live_write() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "rollback-conflict"]);
    let first = world.write("rollback-first.md", "first [[rollback-second]]");
    let second = world.write("rollback-second.md", "second [[rollback-first]]");
    for (slug, title, summary, file) in [
        ("rollback-first", "Rollback First", "First", &first),
        ("rollback-second", "Rollback Second", "Second", &second),
    ] {
        world.ok(
            &world.project,
            &[
                "--changeset",
                "rollback-conflict",
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                summary,
                "--file",
                as_str(file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-conflict"],
    );
    let id = committed["changeset_id"].as_str().unwrap();
    put_page(&world, "later-live", "Later Live", "must survive");

    let rolled_back = world.ok(&world.project, &["changeset", "rollback", id]);
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "later-live"])["page"]["body"],
        "must survive"
    );
    assert_eq!(
        world.err(&world.project, &["page", "show", "rollback-first"])["error"]["code"],
        "page_not_found"
    );
}

#[test]
fn changeset_rollback_conflicts_only_when_a_touched_page_changed_later() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "rollback-entity-conflict", "rollback-entity");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-entity-conflict"],
    );
    let id = committed["changeset_id"].as_str().unwrap();
    put_page(
        &world,
        "rollback-entity-first",
        "Changed Later",
        "the touched entity changed after commit",
    );

    let conflict = world.err(&world.project, &["changeset", "rollback", id]);
    assert_eq!(conflict["error"]["code"], "changeset_rollback_conflict");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "rollback-entity-first"])["page"]["body"],
        "the touched entity changed after commit"
    );
}

#[test]
fn changeset_meta_updates_commit_and_rollback_without_touching_unrelated_pages() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let original_schema = world.ok(&world.project, &["schema", "show"])["schema"].clone();
    let original_purpose = world.ok(&world.project, &["purpose", "show"])["purpose"].clone();
    world.ok(&world.project, &["changeset", "begin", "meta-patch"]);
    let schema = world.write(
        "sparse-schema.md",
        "# Sparse schema\n\nOnly changed metadata.",
    );
    let purpose = world.write("sparse-purpose.md", "# Sparse purpose\n\nMinimal publish.");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "meta-patch",
            "schema",
            "set",
            as_str(&schema),
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "meta-patch",
            "purpose",
            "set",
            as_str(&purpose),
        ],
    );
    stage_pages_in_existing_changeset(&world, "meta-patch", "meta-patch-page");
    let committed = world.ok(&world.project, &["changeset", "commit", "meta-patch"]);
    assert_eq!(
        world.ok(&world.project, &["schema", "show"])["schema"],
        "# Sparse schema\n\nOnly changed metadata."
    );
    assert_eq!(
        world.ok(&world.project, &["purpose", "show"])["purpose"],
        "# Sparse purpose\n\nMinimal publish."
    );
    put_page(
        &world,
        "meta-unrelated",
        "Unrelated",
        "must survive meta rollback",
    );
    world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        world.ok(&world.project, &["schema", "show"])["schema"],
        original_schema
    );
    assert_eq!(
        world.ok(&world.project, &["purpose", "show"])["purpose"],
        original_purpose
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "meta-unrelated"])["page"]["body"],
        "must survive meta rollback"
    );
}

#[test]
fn changeset_rollback_rejects_an_invalid_checkpoint_without_any_live_side_effect() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "invalid-checkpoint", "checkpoint-page");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "invalid-checkpoint"],
    );
    let changeset_id = committed["changeset_id"].as_str().unwrap();
    let checkpoint = committed["checkpoint"].as_str().unwrap();
    let checkpoint_directory = world.project.join(".lwc/checkpoints");
    let checkpoint_path = checkpoint_directory.join(format!("{checkpoint}.db"));
    let saved = checkpoint_directory.join(format!("{checkpoint}.saved"));
    fs::rename(&checkpoint_path, &saved).unwrap();
    fs::write(&checkpoint_path, "not a SQLite database").unwrap();

    let database = world.project.join(".lwc/wiki.db");
    let before: (String, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let before_checkpoints = fs::read_dir(&checkpoint_directory).unwrap().count();

    let error = world.err(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(error["error"]["code"], "changeset_corrupt");
    let after: (String, i64) = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT
                (SELECT value FROM meta WHERE key = 'store_revision'),
                (SELECT COUNT(*) FROM operations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        fs::read_dir(&checkpoint_directory).unwrap().count(),
        before_checkpoints
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "checkpoint-page-first"])["page"]["title"],
        "First"
    );
}

#[test]
fn changeset_commit_rejects_empty_and_lint_failures_before_checkpointing() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let live_revision = || {
        Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };

    world.ok(&world.project, &["changeset", "begin", "empty"]);
    let before = live_revision();
    let empty = world.err(&world.project, &["changeset", "commit", "empty"]);
    assert_eq!(empty["error"]["code"], "changeset_empty");
    assert_eq!(live_revision(), before);
    assert!(!world.project.join(".lwc/checkpoints").exists());

    world.ok(&world.project, &["changeset", "begin", "lint-failing"]);
    let bad_body = world.write("lint-failing.md", "uncited and orphaned");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "lint-failing",
            "page",
            "put",
            "lint-failing",
            "--title",
            "Lint failing",
            "--file",
            as_str(&bad_body),
        ],
    );
    let lint = world.err(&world.project, &["changeset", "commit", "lint-failing"]);
    assert_eq!(lint["error"]["code"], "changeset_lint_failed");
    assert_eq!(live_revision(), before);
    assert!(!world.project.join(".lwc/checkpoints").exists());

    let missing_reason = world.err(
        &world.project,
        &["changeset", "commit", "lint-failing", "--allow-lint-issues"],
    );
    assert_eq!(
        missing_reason["error"]["code"],
        "changeset_lint_override_invalid"
    );
    let committed = world.ok(
        &world.project,
        &[
            "changeset",
            "commit",
            "lint-failing",
            "--allow-lint-issues",
            "--reason",
            "reviewed temporary navigation gap",
        ],
    );
    assert_eq!(committed["status"], "committed");
    let detail: String = Connection::open(database)
        .unwrap()
        .query_row(
            "SELECT detail_json FROM operations
             WHERE action = 'changeset_commit'
             ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert!(detail["lint_issues"].as_u64().unwrap() > 0);
    assert_eq!(
        detail["lint_override_reason"],
        "reviewed temporary navigation gap"
    );
}

#[test]
fn changeset_reports_a_committed_materialization_failure_and_repairs_it() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "projection-failure", "projection");

    let wiki = world.project.join(".lwc/wiki");
    let saved = world.project.join(".lwc/wiki-before-failure");
    fs::rename(&wiki, &saved).unwrap();
    fs::write(&wiki, "blocks directory creation").unwrap();
    let error = world.err(
        &world.project,
        &["changeset", "commit", "projection-failure"],
    );
    assert_eq!(
        error["error"]["code"],
        "changeset_committed_materialization_failed"
    );
    assert_eq!(error["error"]["details"]["committed"], true);
    assert_eq!(
        error["error"]["details"]["recovery_command"],
        "lwc maintenance materialize"
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "projection-first"])["page"]["title"],
        "First"
    );

    fs::remove_file(&wiki).unwrap();
    fs::rename(saved, &wiki).unwrap();
    let queued = world.ok(&world.project, &["maintenance", "materialize"]);
    let work_id = queued["work"]["id"].as_str().unwrap().to_owned();
    let finished = world.ok(&world.project, &["work", "watch", &work_id]);
    assert_eq!(finished["work"]["state"], "succeeded", "{finished}");
    assert!(
        String::from_utf8(fs::read(wiki.join("index.md")).unwrap())
            .unwrap()
            .contains("projection-first")
    );
}

#[test]
fn changeset_reports_a_rolled_back_materialization_failure_and_retry_repairs_it() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "rollback-projection-failure", "rollback-projection");
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "rollback-projection-failure"],
    );
    let changeset_id = committed["changeset_id"].as_str().unwrap();

    let wiki = world.project.join(".lwc/wiki");
    let saved = world.project.join(".lwc/wiki-before-rollback-failure");
    fs::rename(&wiki, &saved).unwrap();
    fs::write(&wiki, "blocks directory creation").unwrap();
    let error = world.err(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(
        error["error"]["code"],
        "changeset_rolled_back_materialization_failed"
    );
    assert_eq!(error["error"]["details"]["rolled_back"], true);
    assert_eq!(
        error["error"]["details"]["recovery_command"],
        "lwc maintenance materialize"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["page", "show", "rollback-projection-first"]
        )["error"]["code"],
        "page_not_found"
    );

    fs::remove_file(&wiki).unwrap();
    fs::rename(saved, &wiki).unwrap();
    let repaired = world.ok(&world.project, &["changeset", "rollback", changeset_id]);
    assert_eq!(repaired["materialized"], true);
    assert!(
        !String::from_utf8(fs::read(wiki.join("index.md")).unwrap())
            .unwrap()
            .contains("rollback-projection-first")
    );
}
#[test]
fn changeset_reports_committed_cleanup_failure_and_retry_only_finishes_cleanup() {
    use std::os::unix::fs::PermissionsExt;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "cleanup-failure", "cleanup");
    let directory = world.project.join(".lwc/changesets");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    let error = world.err(&world.project, &["changeset", "commit", "cleanup-failure"]);
    assert_eq!(error["error"]["code"], "changeset_committed_cleanup_failed");
    assert_eq!(error["error"]["details"]["committed"], true);
    let id = error["error"]["details"]["changeset_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        world.ok(&world.project, &["page", "show", "cleanup-first"])["page"]["title"],
        "First"
    );

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let late_body = world.write("cleanup-late.md", "must not disappear during recovery");
    let late_write = world.err(
        &world.project,
        &[
            "--changeset",
            "cleanup-failure",
            "page",
            "put",
            "cleanup-late",
            "--title",
            "Cleanup Late",
            "--file",
            as_str(&late_body),
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(late_write["error"]["code"], "changeset_frozen");
    let retried = world.ok(&world.project, &["changeset", "commit", "cleanup-failure"]);
    assert_eq!(retried["changeset_id"], id);
    assert!(!directory.join("cleanup-failure.db").exists());
    assert_eq!(
        world.err(&world.project, &["page", "show", "cleanup-late"])["error"]["code"],
        "page_not_found"
    );
    let commit_count: i64 = Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations
             WHERE action = 'changeset_commit' AND target = ?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(commit_count, 1);
}

#[cfg(unix)]
#[test]
fn changeset_locked_recheck_rejects_a_draft_writer_that_finishes_during_commit() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "draft-race", "draft-race");
    let live = world.project.join(".lwc/wiki.db");
    let before_revision: String = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let draft = world.project.join(".lwc/changesets/draft-race.db");
    let writer = Connection::open(&draft).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["changeset", "commit", "draft-race"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let checkpoint_dir = world.project.join(".lwc/checkpoints");
    let deadline = Instant::now() + Duration::from_millis(500);
    while (!checkpoint_dir.is_dir() || fs::read_dir(&checkpoint_dir).unwrap().next().is_none())
        && child.try_wait().unwrap().is_none()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(child.try_wait().unwrap().is_none());
    writer
        .execute_batch(
            "INSERT INTO operations(action, target, detail_json)
             VALUES ('test_draft_race', 'draft', '{}');
             UPDATE meta SET value = LOWER(HEX(RANDOMBLOB(32)))
             WHERE key = 'store_revision';
             COMMIT;",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(stderr_json(&output)["error"]["code"], "changeset_changed");
    let after_revision: String = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after_revision, before_revision);
    assert_eq!(
        world.err(&world.project, &["page", "show", "draft-race-first"])["error"]["code"],
        "page_not_found"
    );
    assert!(draft.is_file());
}
