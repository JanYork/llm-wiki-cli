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
fn sparse_tag_only_changeset_lints_against_live_wiki_and_commits_cleanly() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let schema = world.write("schema.md", "# Test Wiki");
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    for (slug, title, body) in [
        (
            "linked-target",
            "Linked target",
            "target body links back to [[tagged-page]]",
        ),
        (
            "tagged-page",
            "Tagged page",
            "core rules link to [[linked-target]]",
        ),
    ] {
        let file = world.write(&format!("{slug}.md"), body);
        world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                title,
                "--file",
                as_str(&file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
    assert_eq!(world.ok(&world.project, &["lint"])["total"], 0);
    let live_database = world.project.join(".lwc/wiki.db");
    let live_revision: String = Connection::open(&live_database)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'store_revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    world.ok(&world.project, &["changeset", "begin", "tag-only"]);
    world.ok(
        &world.project,
        &[
            "--changeset",
            "tag-only",
            "tag",
            "set",
            "Rules",
            "tagged-page",
            "--reason",
            "required context",
        ],
    );
    let draft = Connection::open(world.project.join(".lwc/changesets/tag-only.db")).unwrap();
    let staged_body: String = draft
        .query_row(
            "SELECT body FROM pages WHERE slug = 'tagged-page'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(staged_body.is_empty(), "tag-only staging copied the page body");
    let staged_dependencies: i64 = draft
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM links) +
                (SELECT COUNT(*) FROM page_sources) +
                (SELECT COUNT(*) FROM page_provenance) +
                (SELECT COUNT(*) FROM search_fts)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(staged_dependencies, 0);

    let lint = world.ok(
        &world.project,
        &["--changeset", "tag-only", "lint"],
    );
    assert_eq!(lint["total"], 0, "{lint}");
    assert_eq!(
        Connection::open(&live_database)
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key = 'store_revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        live_revision,
        "overlay lint mutated the live Wiki",
    );
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "tag-only"],
    );
    assert_eq!(committed["lint_issues"], 0);
    assert_eq!(world.ok(&world.project, &["load", "tag", "Rules"])["returned"], 1);
}

#[test]
fn sparse_changeset_accepts_unchanged_multiple_provenance_fingerprint() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let schema = world.write("schema.md", "# Test Wiki");
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    let anchor = world.write("anchor.md", "anchor links to [[multi-provenance]]");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "anchor",
            "--title",
            "Anchor",
            "--summary",
            "Anchor",
            "--file",
            as_str(&anchor),
            "--provenance",
            "agent-observed",
        ],
    );
    let body = world.write(
        "multi-provenance.md",
        "original body links to [[anchor]]",
    );
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "multi-provenance",
            "--title",
            "Multi provenance",
            "--summary",
            "Multi provenance",
            "--file",
            as_str(&body),
            "--provenance",
            "user-provided",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(world.ok(&world.project, &["lint"])["total"], 0);
    world.ok(&world.project, &["changeset", "begin", "multi-provenance"]);
    let updated = world.write(
        "multi-provenance-updated.md",
        "updated body links to [[anchor]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "multi-provenance",
            "page",
            "put",
            "multi-provenance",
            "--title",
            "Multi provenance",
            "--summary",
            "Multi provenance",
            "--file",
            as_str(&updated),
            "--provenance",
            "user-provided",
            "--provenance",
            "agent-observed",
        ],
    );

    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "multi-provenance"],
    );
    assert_eq!(committed["status"], "committed");
    assert_eq!(committed["lint_issues"], 0);
    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "multi-provenance"])["page"]["body"],
        "original body links to [[anchor]]"
    );
}

#[test]
fn changeset_work_watch_routes_to_the_draft_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    if let Some(configured_work) = configured["work"]["id"].as_str() {
        world.ok(&world.project, &["work", "watch", configured_work]);
    }
    let source = world.write("draft-work-source.md", "draft graph source");
    let added = world.ok(
        &world.project,
        &["source", "add", as_str(&source)],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let source_work = added["graph"]["work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", source_work]);
    world.ok(&world.project, &["ingest", "next"]);
    let analysis = world.write("draft-work-analysis.md", "draft graph analysis");
    world.ok(
        &world.project,
        &["ingest", "analyze", &source_id, "--file", as_str(&analysis)],
    );
    for (slug, title, kind, contents) in [
        (
            "source-summary",
            "Source summary",
            "source",
            "source summary links [[anchor]]",
        ),
        (
            "anchor",
            "Anchor",
            "concept",
            "anchor links [[source-summary]] and [[draft-work]]",
        ),
        (
            "draft-work",
            "Draft Work",
            "concept",
            "original draft work links [[anchor]]",
        ),
    ] {
        let file = world.write(&format!("{slug}.md"), contents);
        let page = world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                title,
                "--kind",
                kind,
                "--summary",
                title,
                "--file",
                as_str(&file),
                "--source",
                &source_id,
                "--provenance",
                "agent-observed",
            ],
        );
        let work = page["graph"]["work"]["id"].as_str().unwrap();
        world.ok(&world.project, &["work", "watch", work]);
    }
    world.ok(&world.project, &["ingest", "complete", &source_id]);
    world.ok(&world.project, &["changeset", "begin", "draft-work"]);
    let body = world.write("draft-work.md", "draft graph work links [[anchor]]");
    let staged = world.ok(
        &world.project,
        &[
            "--changeset",
            "draft-work",
            "page",
            "put",
            "draft-work",
            "--title",
            "Draft Work",
            "--summary",
            "Draft Work",
            "--file",
            as_str(&body),
            "--source",
            &source_id,
            "--provenance",
            "agent-observed",
        ],
    );
    let draft_work = staged["graph"]["work"]["id"].as_str().unwrap();
    let completed = world.ok(
        &world.project,
        &["--changeset", "draft-work", "work", "watch", draft_work],
    );
    assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
    let verified = world.ok(
        &world.project,
        &["--changeset", "draft-work", "graph", "verify"],
    );
    assert_eq!(verified["ok"], true, "{verified}");

    let committed = world.ok(&world.project, &["changeset", "commit", "draft-work"]);
    let commit_work = committed["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", commit_work]);
    assert_eq!(
        world.ok(&world.project, &["graph", "verify"])["ok"],
        true
    );
    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    let rollback_work = rolled_back["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", rollback_work]);
    assert_eq!(
        world.ok(&world.project, &["graph", "verify"])["ok"],
        true
    );
}

#[test]
fn changeset_work_and_graph_state_are_isolated_per_draft() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    if let Some(work_id) = configured["work"]["id"].as_str() {
        world.ok(&world.project, &["work", "watch", work_id]);
    }

    world.ok(&world.project, &["changeset", "begin", "draft-a"]);
    world.ok(&world.project, &["changeset", "begin", "draft-b"]);
    let page_a = world.write("draft-a-page.md", "draft A unique page");
    let staged_a = world.ok(
        &world.project,
        &[
            "--changeset",
            "draft-a",
            "page",
            "put",
            "draft-a-page",
            "--title",
            "Draft A page",
            "--file",
            as_str(&page_a),
            "--provenance",
            "agent-observed",
        ],
    );
    let work_a = staged_a["graph"]["work"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let cross_status = world.command(
        &world.project,
        &["--changeset", "draft-b", "work", "status", &work_a],
    );
    let cross_watch = world.command(
        &world.project,
        &["--changeset", "draft-b", "work", "watch", &work_a],
    );
    let cross_cancel = world.command(
        &world.project,
        &["--changeset", "draft-b", "work", "cancel", &work_a],
    );
    let draft_b_work_list = world.ok(
        &world.project,
        &["--changeset", "draft-b", "work", "list"],
    );
    let cross_list_exposes_a = draft_b_work_list["works"]
        .as_array()
        .unwrap()
        .iter()
        .any(|work| work["id"] == work_a);
    world.ok(
        &world.project,
        &["--changeset", "draft-a", "work", "watch", &work_a],
    );

    let page_b = world.write("draft-b-page.md", "draft B unique page");
    let staged_b = world.ok(
        &world.project,
        &[
            "--changeset",
            "draft-b",
            "page",
            "put",
            "draft-b-page",
            "--title",
            "Draft B page",
            "--file",
            as_str(&page_b),
            "--provenance",
            "agent-observed",
        ],
    );
    let work_b = staged_b["graph"]["work"]["id"].as_str().unwrap();
    world.ok(
        &world.project,
        &["--changeset", "draft-b", "work", "watch", work_b],
    );
    let verify_a = world.ok(
        &world.project,
        &["--changeset", "draft-a", "graph", "verify"],
    );
    let verify_b = world.ok(
        &world.project,
        &["--changeset", "draft-b", "graph", "verify"],
    );
    world.ok(&world.project, &["changeset", "discard", "draft-a"]);
    world.ok(&world.project, &["changeset", "discard", "draft-b"]);
    let runtime_removed = !world
        .project
        .join(".lwc/changesets/draft-draft-a")
        .exists()
        && !world
            .project
            .join(".lwc/changesets/draft-draft-b")
            .exists();

    assert!(
        !cross_status.status.success()
            && !cross_watch.status.success()
            && !cross_cancel.status.success()
            && !cross_list_exposes_a
            && runtime_removed
            && verify_a["ok"] == true
            && verify_b["ok"] == true,
        "draft isolation failed: cross_status={}, cross_watch={}, cross_cancel={}, \
         cross_list_exposes_a={cross_list_exposes_a}, runtime_removed={runtime_removed}, \
         verify_a={verify_a}, verify_b={verify_b}",
        cross_status.status.success(),
        cross_watch.status.success(),
        cross_cancel.status.success(),
    );
}

#[cfg(unix)]
#[test]
fn changeset_graph_rejects_a_symlinked_draft_runtime() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    world.ok(&world.project, &["changeset", "begin", "unsafe-runtime"]);
    let runtime = world
        .project
        .join(".lwc/changesets/draft-unsafe-runtime");
    let outside = world.project.join("outside-runtime");
    fs::remove_dir(&runtime).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, &runtime).unwrap();

    let page = world.write("unsafe-runtime-page.md", "unsafe runtime page");
    let error = world.err(
        &world.project,
        &[
            "--changeset",
            "unsafe-runtime",
            "page",
            "put",
            "unsafe-runtime-page",
            "--title",
            "Unsafe runtime page",
            "--file",
            as_str(&page),
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(error["error"]["code"], "changeset_path_invalid");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_file(&runtime).unwrap();
    fs::create_dir(&runtime).unwrap();
    assert_eq!(
        world.err(
            &world.project,
            &[
                "--changeset",
                "unsafe-runtime",
                "page",
                "show",
                "unsafe-runtime-page",
            ],
        )["error"]["code"],
        "page_not_found"
    );
    fs::remove_dir(&runtime).unwrap();
    symlink(&outside, &runtime).unwrap();
    let relation = world.err(
        &world.project,
        &[
            "--changeset",
            "unsafe-runtime",
            "graph",
            "relation",
            "set",
            "page:index",
            "SUPPORTS",
            "page:index",
            "--provenance",
            "agent-observed",
            "--reason",
            "must not partially persist",
            "--confidence",
            "0.8",
        ],
    );
    assert_eq!(relation["error"]["code"], "changeset_path_invalid");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_file(&runtime).unwrap();
    fs::create_dir(&runtime).unwrap();
    assert_eq!(
        world.ok(
            &world.project,
            &[
                "--changeset",
                "unsafe-runtime",
                "graph",
                "relation",
                "list",
            ],
        )["relations"],
        serde_json::json!([])
    );
}

#[test]
fn changeset_safely_recreates_a_legacy_missing_draft_runtime() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    world.ok(&world.project, &["changeset", "begin", "legacy-runtime"]);
    let runtime = world
        .project
        .join(".lwc/changesets/draft-legacy-runtime");
    fs::remove_dir(&runtime).unwrap();

    let page = world.write("legacy-runtime-page.md", "legacy runtime page");
    let staged = world.ok(
        &world.project,
        &[
            "--changeset",
            "legacy-runtime",
            "page",
            "put",
            "legacy-runtime-page",
            "--title",
            "Legacy runtime page",
            "--file",
            as_str(&page),
            "--provenance",
            "agent-observed",
        ],
    );
    let work = staged["graph"]["work"]["id"].as_str().unwrap();
    world.ok(
        &world.project,
        &["--changeset", "legacy-runtime", "work", "watch", work],
    );
    assert_eq!(
        world.ok(
            &world.project,
            &["--changeset", "legacy-runtime", "graph", "verify"],
        )["ok"],
        true
    );
}

#[test]
fn changeset_runtime_names_do_not_overlap_legacy_shared_roots() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    for name in ["work", "graph-grafeo", "graph-surrealdb"] {
        let legacy = world.project.join(".lwc/changesets").join(name);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("legacy-marker"), "legacy derived state").unwrap();
        world.ok(&world.project, &["changeset", "begin", name]);
        world.ok(&world.project, &["changeset", "discard", name]);
        assert_eq!(
            fs::read_to_string(legacy.join("legacy-marker")).unwrap(),
            "legacy derived state"
        );
    }
}

#[test]
fn changeset_rollback_restores_a_dangling_link_that_predated_a_new_page() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let schema = world.write("schema.md", "# Test Wiki");
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    let anchor = world.write("preexisting-anchor.md", "anchor links [[new-page]]");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "anchor",
            "--title",
            "Anchor",
            "--summary",
            "Anchor",
            "--file",
            as_str(&anchor),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(&world.project, &["changeset", "begin", "new-page"]);
    let page = world.write("new-page.md", "new page links [[anchor]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "new-page",
            "page",
            "put",
            "new-page",
            "--title",
            "New page",
            "--summary",
            "New page",
            "--file",
            as_str(&page),
            "--provenance",
            "agent-observed",
        ],
    );
    let committed = world.ok(&world.project, &["changeset", "commit", "new-page"]);
    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.err(&world.project, &["page", "show", "new-page"])["error"]["code"],
        "page_not_found"
    );
    assert_eq!(
        world.ok(&world.project, &["page", "links", "anchor"])["missing"],
        serde_json::json!(["new-page"])
    );
}

#[test]
fn changeset_rollback_restores_a_touched_page_with_a_preexisting_dangling_link() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let schema = world.write("schema-touched-link.md", "# Test Wiki");
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    let original = world.write("anchor-original.md", "anchor links [[a-new-page]]");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "z-anchor",
            "--title",
            "Anchor",
            "--summary",
            "Anchor",
            "--file",
            as_str(&original),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(&world.project, &["changeset", "begin", "touched-link"]);
    let updated = world.write(
        "anchor-updated.md",
        "updated anchor still links [[a-new-page]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "touched-link",
            "page",
            "put",
            "z-anchor",
            "--title",
            "Anchor",
            "--summary",
            "Anchor",
            "--file",
            as_str(&updated),
            "--provenance",
            "agent-observed",
        ],
    );
    let page = world.write("touched-new-page.md", "new page links [[z-anchor]]");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "touched-link",
            "page",
            "put",
            "a-new-page",
            "--title",
            "New page",
            "--summary",
            "New page",
            "--file",
            as_str(&page),
            "--provenance",
            "agent-observed",
        ],
    );
    let committed = world.ok(&world.project, &["changeset", "commit", "touched-link"]);
    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.ok(&world.project, &["page", "show", "z-anchor"])["page"]["body"],
        "anchor links [[a-new-page]]"
    );
    assert_eq!(
        world.err(&world.project, &["page", "show", "a-new-page"])["error"]["code"],
        "page_not_found"
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
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    let configured_work = configured["work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", configured_work]);
    assert_eq!(world.ok(&world.project, &["graph", "verify"])["ok"], true);

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
        format!("lwc changeset rollback {changeset_id}")
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
    let graph_work = repaired["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", graph_work]);
    assert_eq!(world.ok(&world.project, &["graph", "verify"])["ok"], true);
    assert!(
        !String::from_utf8(fs::read(wiki.join("index.md")).unwrap())
            .unwrap()
            .contains("rollback-projection-first")
    );
}
#[cfg(unix)]
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
#[test]
fn changeset_source_head_rollback_reprojects_the_previous_tracked_revision() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    if let Some(work_id) = configured["work"]["id"].as_str() {
        world.ok(&world.project, &["work", "watch", work_id]);
    }

    let tracked = world.write("tracked.md", "tracked revision v1");
    let first = world.ok(
        &world.project,
        &["source", "add", as_str(&tracked)],
    );
    let first_id = first["source"]["id"].as_i64().unwrap().to_string();
    let first_work = first["graph"]["work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", first_work]);

    fs::write(&tracked, "tracked revision v2").unwrap();
    world.ok(&world.project, &["changeset", "begin", "tracked-source"]);
    let second = world.ok(
        &world.project,
        &[
            "--changeset",
            "tracked-source",
            "source",
            "add",
            as_str(&tracked),
        ],
    );
    let second_id = second["source"]["id"].as_i64().unwrap().to_string();
    let second_work = second["graph"]["work"]["id"].as_str().unwrap();
    world.ok(
        &world.project,
        &[
            "--changeset",
            "tracked-source",
            "work",
            "watch",
            second_work,
        ],
    );

    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "tracked-source"],
    );
    let commit_work = committed["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", commit_work]);
    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    let rollback_work = rolled_back["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", rollback_work]);

    let verified = world.ok(&world.project, &["graph", "verify"]);
    assert_eq!(verified["ok"], true, "{verified}");
    world.ok(
        &world.project,
        &["graph", "node", &format!("source:{first_id}")],
    );
    assert_eq!(
        world.err(
            &world.project,
            &["graph", "node", &format!("source:{second_id}")],
        )["error"]["code"],
        "graph_node_not_found"
    );
}

#[test]
fn changeset_rollback_accepts_a_legacy_sparse_inverse_without_new_optional_fields() {
    use sha2::{Digest, Sha256};

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "legacy-inverse"]);
    let body = world.write("legacy-inverse.md", "legacy inverse payload");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "legacy-inverse",
            "page",
            "put",
            "legacy-inverse",
            "--title",
            "Legacy inverse",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ],
    );
    let committed = world.ok(
        &world.project,
        &[
            "changeset",
            "commit",
            "legacy-inverse",
            "--allow-lint-issues",
            "--reason",
            "legacy sparse inverse compatibility fixture",
        ],
    );
    let checkpoint = committed["checkpoint"].as_str().unwrap();
    let inverse_path = world
        .project
        .join(".lwc/checkpoints")
        .join(format!("{checkpoint}.db"));
    let encoded = fs::read_to_string(inverse_path).unwrap();
    let checksum_marker = ",\"checksum\":";
    let marker = encoded.rfind(checksum_marker).unwrap();
    let raw_payload = encoded[..marker].strip_prefix("{\"payload\":").unwrap();
    let envelope: Value = serde_json::from_str(&encoded).unwrap();

    assert!(envelope["payload"].get("source_paths").is_none());
    assert!(envelope["payload"]["pages"][0]
        .get("inbound_links")
        .is_none());
    assert_eq!(
        envelope["checksum"].as_str().unwrap(),
        Sha256::digest(raw_payload.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );

    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(
        world.err(&world.project, &["page", "show", "legacy-inverse"])["error"]["code"],
        "page_not_found"
    );
}
#[test]
fn changeset_remaps_a_source_id_taken_by_an_unrelated_live_path() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    if let Some(work_id) = configured["work"]["id"].as_str() {
        world.ok(&world.project, &["work", "watch", work_id]);
    }

    let path_a = world.write("a.md", "a v1");
    let first_a = world.ok(&world.project, &["source", "add", as_str(&path_a)]);
    let first_a_id = first_a["source"]["id"].as_i64().unwrap();
    world.ok(
        &world.project,
        &["work", "watch", first_a["graph"]["work"]["id"].as_str().unwrap()],
    );
    let path_b = world.write("b.md", "b v1");
    let first_b = world.ok(&world.project, &["source", "add", as_str(&path_b)]);
    world.ok(
        &world.project,
        &["work", "watch", first_b["graph"]["work"]["id"].as_str().unwrap()],
    );

    fs::write(&path_a, "a v2").unwrap();
    world.ok(&world.project, &["changeset", "begin", "source-remap"]);
    let staged_a = world.ok(
        &world.project,
        &[
            "--changeset",
            "source-remap",
            "source",
            "add",
            as_str(&path_a),
        ],
    );
    let staged_id = staged_a["source"]["id"].as_i64().unwrap();
    world.ok(
        &world.project,
        &[
            "--changeset",
            "source-remap",
            "work",
            "watch",
            staged_a["graph"]["work"]["id"].as_str().unwrap(),
        ],
    );
    let page = world.write(
        "remapped-source-page.md",
        "remapped source evidence links [[remapped-source-page]]",
    );
    let staged_page = world.ok(
        &world.project,
        &[
            "--changeset",
            "source-remap",
            "page",
            "put",
            "remapped-source-page",
            "--title",
            "Remapped source page",
            "--summary",
            "Cites the remapped draft source",
            "--file",
            as_str(&page),
            "--source",
            &staged_id.to_string(),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "source-remap",
            "work",
            "watch",
            staged_page["graph"]["work"]["id"].as_str().unwrap(),
        ],
    );

    fs::write(&path_b, "b v2").unwrap();
    let second_b = world.ok(&world.project, &["source", "add", as_str(&path_b)]);
    let second_b_id = second_b["source"]["id"].as_i64().unwrap();
    assert_eq!(second_b_id, staged_id, "test did not force an ID collision");
    world.ok(
        &world.project,
        &["work", "watch", second_b["graph"]["work"]["id"].as_str().unwrap()],
    );

    let committed = world.ok(&world.project, &["changeset", "commit", "source-remap"]);
    world.ok(
        &world.project,
        &["work", "watch", committed["graph_work"]["id"].as_str().unwrap()],
    );
    let current = world.ok(&world.project, &["source", "status", "--all"]);
    let checks = current["checks"].as_array().unwrap();
    let remapped_a_id = checks
        .iter()
        .find(|check| check["tracked_path"] == "a.md")
        .unwrap()["head_source_id"]
        .as_i64()
        .unwrap();
    let current_b_id = checks
        .iter()
        .find(|check| check["tracked_path"] == "b.md")
        .unwrap()["head_source_id"]
        .as_i64()
        .unwrap();
    assert_ne!(remapped_a_id, staged_id);
    assert_eq!(current_b_id, second_b_id);
    assert_eq!(
        world.ok(
            &world.project,
            &["source", "show", &remapped_a_id.to_string(), "--max-chars", "100"],
        )["source"]["content"],
        "a v2"
    );
    assert_eq!(
        world.ok(
            &world.project,
            &["source", "show", &second_b_id.to_string(), "--max-chars", "100"],
        )["source"]["content"],
        "b v2"
    );
    let committed_page = world.ok(
        &world.project,
        &["page", "show", "remapped-source-page"],
    );
    assert_eq!(
        committed_page["page"]["source_ids"],
        serde_json::json!([remapped_a_id])
    );
    assert_eq!(world.ok(&world.project, &["graph", "verify"])["ok"], true);
    let cited = world.ok(
        &world.project,
        &[
            "graph",
            "neighbors",
            "page:remapped-source-page",
            "--direction",
            "outgoing",
            "--edge-type",
            "CITES",
        ],
    );
    assert!(
        cited["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["identifier"] == format!("source:{remapped_a_id}")),
        "{cited}"
    );

    let rolled_back = world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    world.ok(
        &world.project,
        &["work", "watch", rolled_back["graph_work"]["id"].as_str().unwrap()],
    );
    let restored = world.ok(&world.project, &["source", "status", "--all"]);
    let checks = restored["checks"].as_array().unwrap();
    assert_eq!(
        checks
            .iter()
            .find(|check| check["tracked_path"] == "a.md")
            .unwrap()["head_source_id"],
        first_a_id
    );
    assert_eq!(
        checks
            .iter()
            .find(|check| check["tracked_path"] == "b.md")
            .unwrap()["head_source_id"],
        second_b_id
    );
    assert_eq!(world.ok(&world.project, &["graph", "verify"])["ok"], true);
    assert_eq!(
        world.err(
            &world.project,
            &["page", "show", "remapped-source-page"],
        )["error"]["code"],
        "page_not_found"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["graph", "node", "page:remapped-source-page"],
        )["error"]["code"],
        "graph_node_not_found"
    );
    world.ok(
        &world.project,
        &["graph", "node", &format!("source:{first_a_id}")],
    );
    world.ok(
        &world.project,
        &["graph", "node", &format!("source:{second_b_id}")],
    );
    assert_eq!(
        world.err(
            &world.project,
            &["graph", "node", &format!("source:{remapped_a_id}")],
        )["error"]["code"],
        "graph_node_not_found"
    );
}

#[test]
fn changeset_rollback_rejects_a_page_repointed_to_the_source_id_collision() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let path_a = world.write("conflict-a.md", "a v1");
    world.ok(&world.project, &["source", "add", as_str(&path_a)]);
    let path_b = world.write("conflict-b.md", "b v1");
    world.ok(&world.project, &["source", "add", as_str(&path_b)]);

    fs::write(&path_a, "a v2").unwrap();
    world.ok(
        &world.project,
        &["changeset", "begin", "page-source-remap-conflict"],
    );
    let staged = world.ok(
        &world.project,
        &[
            "--changeset",
            "page-source-remap-conflict",
            "source",
            "add",
            as_str(&path_a),
        ],
    );
    let collided_id = staged["source"]["id"].as_i64().unwrap().to_string();
    let body = world.write(
        "page-source-remap-conflict.md",
        "same content [[page-source-remap-conflict]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "page-source-remap-conflict",
            "page",
            "put",
            "page-source-remap-conflict",
            "--title",
            "Page source remap conflict",
            "--summary",
            "Same summary",
            "--file",
            as_str(&body),
            "--source",
            &collided_id,
            "--provenance",
            "agent-observed",
        ],
    );

    fs::write(&path_b, "b v2").unwrap();
    let collision = world.ok(&world.project, &["source", "add", as_str(&path_b)]);
    assert_eq!(collision["source"]["id"].to_string(), collided_id);
    let committed = world.ok(
        &world.project,
        &["changeset", "commit", "page-source-remap-conflict"],
    );
    let committed_source_id = world
        .ok(
            &world.project,
            &["page", "show", "page-source-remap-conflict"],
        )["page"]["source_ids"][0]
        .as_i64()
        .unwrap()
        .to_string();
    assert_ne!(committed_source_id, collided_id);

    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "page-source-remap-conflict",
            "--title",
            "Page source remap conflict",
            "--summary",
            "Same summary",
            "--file",
            as_str(&body),
            "--source",
            &collided_id,
            "--provenance",
            "agent-observed",
        ],
    );
    let error = world.err(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert_eq!(error["error"]["code"], "changeset_rollback_conflict");
    assert_eq!(
        world
            .ok(
                &world.project,
                &["page", "show", "page-source-remap-conflict"],
            )["page"]["source_ids"],
        serde_json::json!([collided_id.parse::<i64>().unwrap()])
    );
}

#[test]
fn changeset_commit_retry_reprojects_when_legacy_detail_lacks_graph_documents() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let configured = world.ok(
        &world.project,
        &["config", "set", "--graph", "grafeo"],
    );
    if let Some(work_id) = configured["work"]["id"].as_str() {
        world.ok(&world.project, &["work", "watch", work_id]);
    }

    world.ok(&world.project, &["changeset", "begin", "retry-graph-queue"]);
    let body = world.write(
        "retry-graph-queue.md",
        "retry graph queue [[retry-graph-queue]]",
    );
    let staged = world.ok(
        &world.project,
        &[
            "--changeset",
            "retry-graph-queue",
            "page",
            "put",
            "retry-graph-queue",
            "--title",
            "Retry graph queue",
            "--summary",
            "Retry graph queue",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "retry-graph-queue",
            "work",
            "watch",
            staged["graph"]["work"]["id"].as_str().unwrap(),
        ],
    );

    let work_root = world.project.join(".lwc/work");
    let saved_work_root = world.project.join(".lwc/work-before-queue-failure");
    fs::rename(&work_root, &saved_work_root).unwrap();
    fs::write(&work_root, "blocks graph Work creation").unwrap();
    let failed = world.err(
        &world.project,
        &["changeset", "commit", "retry-graph-queue"],
    );
    assert_eq!(failed["error"]["code"], "graph_projection_failed");
    assert_eq!(failed["error"]["details"]["canonical_committed"], true);
    let changeset_id = failed["error"]["details"]["changeset_id"]
        .as_str()
        .unwrap();
    let database = world.project.join(".lwc/wiki.db");
    let connection = Connection::open(database).unwrap();
    let mut legacy_detail: Value = connection
        .query_row(
            "SELECT detail_json FROM operations
             WHERE action = 'changeset_commit' AND target = ?1",
            [changeset_id],
            |row| row.get::<_, String>(0),
        )
        .map(|detail| serde_json::from_str(&detail).unwrap())
        .unwrap();
    assert!(legacy_detail
        .as_object_mut()
        .unwrap()
        .remove("graph_documents")
        .is_some());
    connection
        .execute(
            "UPDATE operations SET detail_json = ?2
             WHERE action = 'changeset_commit' AND target = ?1",
            rusqlite::params![changeset_id, serde_json::to_string(&legacy_detail).unwrap()],
        )
        .unwrap();

    fs::remove_file(&work_root).unwrap();
    fs::rename(saved_work_root, &work_root).unwrap();
    let retried = world.ok(
        &world.project,
        &["changeset", "commit", "retry-graph-queue"],
    );
    let graph_work = retried["graph_work"]["id"].as_str().unwrap();
    world.ok(&world.project, &["work", "watch", graph_work]);
    let verified = world.ok(&world.project, &["graph", "verify"]);
    assert_eq!(verified["ok"], true, "{verified}");
    world.ok(&world.project, &["graph", "node", "page:retry-graph-queue"]);
}
