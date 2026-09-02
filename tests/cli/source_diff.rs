#[test]
fn read_commands_migrate_v6_store_to_structured_provenance() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("migration-source.md", "migration evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let body = world.write("migration-page.md", "migration page body");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "migration-page",
            "--title",
            "Migration page",
            "--summary",
            "Migration summary",
            "--file",
            as_str(&body),
            "--source",
            &source_id,
        ],
    );

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         DROP TABLE source_path_revisions;
         DROP TABLE IF EXISTS page_provenance;
         UPDATE meta SET value = '6' WHERE key = 'format_version';
         PRAGMA user_version = 6;",
    )
    .unwrap();
    drop(conn);

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(
        context["stores"][0]["pages"][0]["provenance"],
        serde_json::json!(["source-grounded"])
    );

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 18);
    conn.prepare("SELECT page_slug, provenance FROM page_provenance LIMIT 0")
        .unwrap();
}

#[test]
fn read_commands_migrate_v7_without_guessing_source_path_history() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("legacy-source.md", "legacy evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         DROP TABLE source_path_revisions;
         UPDATE meta SET value = '7' WHERE key = 'format_version';
         PRAGMA user_version = 7;",
    )
    .unwrap();
    drop(conn);

    let listed = world.ok(&world.project, &["source", "list"]);
    assert_eq!(listed["sources"][0]["id"], added["source"]["id"]);

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let tracked: i64 = conn
        .query_row("SELECT COUNT(*) FROM source_path_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 18);
    assert_eq!(tracked, 0, "migration must not guess a legacy path head");

    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let status = world.ok(&world.project, &["source", "status", &source_id]);
    assert!(status["checks"].as_array().unwrap().is_empty());
    assert_eq!(status["untracked_source_ids"], serde_json::json!([1]));
}

#[test]
fn read_commands_atomically_migrate_a_v8_store_to_weighted_retrieval() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "v8-page",
        "V8 page",
        "v8 migration retrieval term\n\n文档目录",
    );

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         UPDATE meta SET value = '8' WHERE key = 'format_version';
         PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);

    let search = world.ok(
        &world.project,
        &["search", "v8 migration retrieval term", "--explain"],
    );
    assert_eq!(search["results"][0]["identifier"], "v8-page");
    assert_eq!(
        search["results"][0]["explanation"]["signals"]["generic_marker"],
        1.0
    );

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 18);
    conn.prepare("SELECT path_terms FROM search_fts LIMIT 0")
        .unwrap();
    conn.prepare("SELECT weight FROM retrieval_weights LIMIT 0")
        .unwrap();
    conn.prepare("SELECT signal FROM retrieval_feedback LIMIT 0")
        .unwrap();
    let structural_navigation: i64 = conn
        .query_row(
            "SELECT structural_navigation FROM pages WHERE slug = 'v8-page'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(structural_navigation, 1);
}

#[test]
fn failed_v8_migration_leaves_version_and_schema_unchanged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         CREATE TABLE retrieval_weights(sentinel TEXT NOT NULL);
         UPDATE meta SET value = '8' WHERE key = 'format_version';
         PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);

    let error = world.err(&world.project, &["context", "--limit", "1"]);
    assert_eq!(error["error"]["code"], "store_migration_failed");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format_version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let feedback_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'retrieval_feedback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 8);
    assert_eq!(format_version, "8");
    assert_eq!(feedback_exists, 0);
    for table in ["sources", "pages"] {
        let columns: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = 'structural_navigation'"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(columns, 0, "failed migration added a column to {table}");
    }
    conn.prepare("SELECT sentinel FROM retrieval_weights LIMIT 0")
        .unwrap();
}

#[test]
fn read_commands_atomically_migrate_a_v9_store_to_changeset_metadata() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "v9-page", "V9 page", "v9 migration content");

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE changesets;
         DELETE FROM meta WHERE key IN ('store_id', 'store_revision');
         UPDATE meta SET value = '9' WHERE key = 'format_version';
         PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);

    let shown = world.ok(&world.project, &["page", "show", "v9-page"]);
    assert_eq!(shown["page"]["body"], "v9 migration content");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 18);
    for key in ["store_id", "store_revision"] {
        let value: String = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value.len(), 64);
    }
    conn.prepare("SELECT id, status, post_revision FROM changesets LIMIT 0")
        .unwrap();
}

#[test]
fn failed_v9_changeset_migration_leaves_version_and_schema_unchanged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE changesets;
         CREATE TABLE changesets(sentinel TEXT NOT NULL);
         DELETE FROM meta WHERE key IN ('store_id', 'store_revision');
         UPDATE meta SET value = '9' WHERE key = 'format_version';
         PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);

    let error = world.err(&world.project, &["context", "--limit", "1"]);
    assert_eq!(error["error"]["code"], "store_migration_failed");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let format_version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let identity_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key IN ('store_id', 'store_revision')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 9);
    assert_eq!(format_version, "9");
    assert_eq!(identity_count, 0);
    conn.prepare("SELECT sentinel FROM changesets LIMIT 0")
        .unwrap();
}

#[test]
fn project_and_global_stores_are_isolated_and_combined_deterministically() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);

    let project_page = world.write("project.md", "sharedterm project knowledge");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "project-page",
            "--title",
            "Project page",
            "--summary",
            "Project result",
            "--file",
            as_str(&project_page),
        ],
    );

    let global_page = world.write("global.md", "sharedterm global knowledge");
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "page",
            "put",
            "global-page",
            "--title",
            "Global page",
            "--summary",
            "Global result",
            "--file",
            as_str(&global_page),
        ],
    );

    let project_only = world.ok(&world.project, &["search", "sharedterm"]);
    assert!(
        project_only["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["scope"] == "project")
    );

    let combined = world.ok(&world.project, &["--scope", "all", "search", "sharedterm"]);
    let scopes: Vec<_> = combined["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["scope"].as_str().unwrap())
        .collect();
    assert_eq!(scopes, vec!["project", "global"]);

    let second_project_page = world.write("project-two.md", "sharedterm second project page");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "project-page-two",
            "--title",
            "Second project page",
            "--file",
            as_str(&second_project_page),
        ],
    );
    let limited = world.ok(
        &world.project,
        &["--scope", "all", "search", "sharedterm", "--limit", "1"],
    );
    assert_eq!(limited["results"].as_array().unwrap().len(), 1);
    assert_eq!(limited["results"][0]["scope"], "project");

    let context = world.ok(&world.project, &["--scope", "all", "context"]);
    assert_eq!(context["stores"][0]["scope"], "project");
    assert_eq!(context["stores"][1]["scope"], "global");

    let unsupported = world.err(&world.project, &["--scope", "all", "schema", "show"]);
    assert_eq!(unsupported["error"]["code"], "scope_not_supported");
}

#[test]
fn retrieval_state_for_the_same_identifier_never_crosses_scopes() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);
    let body = world.write("same-scope.md", "scope-isolation-query");
    for scope in [None, Some("global")] {
        let mut args = Vec::new();
        if let Some(scope) = scope {
            args.extend(["--scope", scope]);
        }
        args.extend([
            "page",
            "put",
            "same-page",
            "--title",
            "Same page",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ]);
        world.ok(&world.project, &args);
    }

    for (scope, value, signal) in [
        (None, "2", "relevant"),
        (Some("global"), "-2", "irrelevant"),
    ] {
        let mut prefix = Vec::new();
        if let Some(scope) = scope {
            prefix.extend(["--scope", scope]);
        }
        let mut weight = prefix.clone();
        weight.extend([
            "weight",
            "set",
            "page",
            "same-page",
            "--value",
            value,
            "--reason",
            "scope fixture",
            "--provenance",
            "user-provided",
        ]);
        world.ok(&world.project, &weight);
        let mut feedback = prefix;
        feedback.extend([
            "weight",
            "feedback",
            "page",
            "same-page",
            "--query",
            "scope-isolation-query",
            "--signal",
            signal,
            "--reason",
            "scope fixture",
            "--provenance",
            "user-provided",
        ]);
        world.ok(&world.project, &feedback);
    }

    for (scope, expected) in [(None, 1.0), (Some("global"), -1.0)] {
        let mut args = Vec::new();
        if let Some(scope) = scope {
            args.extend(["--scope", scope]);
        }
        args.extend(["search", "scope-isolation-query", "--explain"]);
        let result = world.ok(&world.project, &args);
        let explanation = &find_result(&result, "same-page")["explanation"];
        assert_eq!(explanation["signals"]["manual_adjustment"], expected);
        assert_eq!(explanation["signals"]["feedback_adjustment"], expected);
    }

    let combined = world.ok(
        &world.project,
        &[
            "--scope",
            "all",
            "search",
            "scope-isolation-query",
            "--explain",
        ],
    );
    let rows = combined["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["identifier"] == "same-page")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_ne!(
        rows[0]["explanation"]["signals"]["manual_adjustment"],
        rows[1]["explanation"]["signals"]["manual_adjustment"]
    );
}

#[test]
fn configured_project_root_blocks_ancestor_project_reuse() {
    let world = TestWorld::new();
    let outer = world.home.join("work");
    let project = outer.join("project");
    let nested = project.join("src");
    fs::create_dir_all(&nested).unwrap();
    world.ok(&outer, &["init"]);

    let output = world.command_in_project_root(&nested, &project, &["init"]);
    assert!(
        output.status.success(),
        "bounded init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let initialized = stdout_json(&output);
    let expected_database = fs::canonicalize(&project).unwrap().join(".lwc/wiki.db");

    assert_eq!(
        initialized["database"],
        expected_database.to_string_lossy().as_ref()
    );
    assert!(project.join(".lwc/wiki.db").is_file());
}

#[test]
fn configured_project_root_rejects_a_cwd_outside_it() {
    let world = TestWorld::new();
    let outside = world.home.join("outside");
    fs::create_dir_all(&outside).unwrap();

    let output = world.command_in_project_root(&outside, &world.project, &["init"]);
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "project_root_mismatch"
    );
    assert!(!outside.join(".lwc/wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn configured_project_root_rejects_a_symlinked_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside = world.home.join("outside-store");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, world.project.join(".lwc")).unwrap();

    let output = world.command_in_project_root(&world.project, &world.project, &["init"]);
    assert!(!output.status.success());
    assert_eq!(stderr_json(&output)["error"]["code"], "project_root_escape");
    assert!(!outside.join("wiki.db").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_project_scope_rejects_a_symlinked_live_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside_project = world.home.join("outside-project");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let outside_store = outside_project.join(".lwc");
    symlink(&outside_store, world.project.join(".lwc")).unwrap();

    let error = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(error["error"]["code"], "project_root_escape");
    assert!(!outside_store.join("changesets").exists());
}

#[cfg(unix)]
#[test]
fn unconfigured_project_scope_rejects_a_symlinked_live_database() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let outside_project = world.home.join("outside-project");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let project_database = world.project.join(".lwc/wiki.db");
    fs::remove_file(&project_database).unwrap();
    symlink(outside_project.join(".lwc/wiki.db"), &project_database).unwrap();

    let error = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(error["error"]["code"], "project_root_escape");
    assert!(!world.project.join(".lwc/changesets").exists());
}

#[cfg(unix)]
#[test]
fn global_scope_rejects_a_symlinked_live_store_directory() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    let outside_project = world.project.join("outside-global");
    fs::create_dir_all(&outside_project).unwrap();
    world.ok(&outside_project, &["init"]);
    let outside_store = outside_project.join(".lwc");
    symlink(&outside_store, world.home.join(".lwc")).unwrap();

    let error = world.err(
        &world.project,
        &["--scope", "global", "changeset", "begin", "escape"],
    );
    assert_eq!(error["error"]["code"], "store_path_invalid");
    assert!(!outside_store.join("changesets").exists());
}

#[test]
fn configured_project_root_rejects_multiple_ancestor_stores() {
    let world = TestWorld::new();
    let nested = world.project.join("nested");
    fs::create_dir_all(&nested).unwrap();
    world.ok(&world.project, &["init"]);

    let nested_init = world.command_in_project_root(&nested, &nested, &["init"]);
    assert!(nested_init.status.success());

    let output = world.command_in_project_root(&nested, &world.project, &["context"]);
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "project_scope_conflict"
    );
}

#[test]
fn failures_are_structured_and_do_not_create_implicit_stores() {
    let world = TestWorld::new();

    let missing = world.err(&world.project, &["page", "list"]);
    assert_eq!(missing["error"]["code"], "store_not_found");
    assert!(!world.project.join(".lwc/wiki.db").exists());

    let no_combined = world.err(&world.project, &["--scope", "all", "search", "anything"]);
    assert_eq!(no_combined["error"]["code"], "store_not_found");
    let no_combined_context = world.err(&world.project, &["--scope", "all", "context"]);
    assert_eq!(no_combined_context["error"]["code"], "store_not_found");

    let clap_error = world.command(&world.project, &["page", "put"]);
    assert!(!clap_error.status.success());
    assert!(serde_json::from_slice::<Value>(&clap_error.stderr).is_err());
    assert!(String::from_utf8_lossy(&clap_error.stderr).contains("Usage:"));

    world.ok(&world.project, &["init"]);
    let invalid_utf8 = world.project.join("raw.bin");
    fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();
    let invalid = world.err(&world.project, &["source", "add", as_str(&invalid_utf8)]);
    assert_eq!(invalid["error"]["code"], "invalid_utf8");

    let invalid_limit = world.err(&world.project, &["context", "--limit", "0"]);
    assert_eq!(invalid_limit["error"]["code"], "invalid_limit");
}

#[test]
fn context_limit_caps_pages_and_operations_per_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    for slug in ["alpha", "beta"] {
        let page = world.write(&format!("{slug}.md"), &format!("{slug} body"));
        world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                slug,
                "--file",
                as_str(&page),
            ],
        );
    }

    let context = world.ok(&world.project, &["context", "--limit", "1"]);
    assert_eq!(context["stores"][0]["pages"].as_array().unwrap().len(), 1);
    assert_eq!(
        context["stores"][0]["recent_operations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn source_add_without_title_uses_a_deterministic_origin_based_title() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/ownership.md", "Rust ownership assigns values.");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);

    let title = added["source"]["title"]
        .as_str()
        .expect("source add should synthesize a non-empty title from origin");
    assert!(!title.is_empty(), "fallback title must not be empty");
    assert!(
        title.contains("ownership.md"),
        "fallback title should be origin-based, got {title:?}"
    );

    let shown = world.ok(&world.project, &["source", "show", "1"]);
    assert_eq!(shown["source"]["title"], title);
}

#[test]
fn source_add_tracks_canonical_project_paths_without_breaking_content_deduplication() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    world.write("raw/source.md", "same tracked bytes");
    let nested = world.project.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let first_output = world.command_in_project_root(
        &nested,
        &world.project,
        &["source", "add", "../raw/source.md"],
    );
    assert!(
        first_output.status.success(),
        "source add failed: {}",
        String::from_utf8_lossy(&first_output.stderr)
    );
    let first = stdout_json(&first_output);

    let duplicate = world.write("alias/same.md", "same tracked bytes");
    let second = world.ok(&world.project, &["source", "add", as_str(&duplicate)]);
    assert_eq!(first["source"]["id"], second["source"]["id"]);

    let conn = Connection::open(world.project.join(".lwc/wiki.db")).unwrap();
    let revisions = conn
        .prepare(
            "SELECT tracked_path, revision, source_id
             FROM source_path_revisions
             ORDER BY tracked_path, revision",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        revisions,
        vec![
            ("alias/same.md".to_string(), 1, 1),
            ("raw/source.md".to_string(), 1, 1),
        ]
    );
}

#[test]
fn source_status_detects_same_size_changes_without_recording_an_operation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/status.md", "alpha");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let before = world.ok(&world.project, &["log", "--limit", "100"]);
    let current = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(current["checks"][0]["tracked_path"], "raw/status.md");
    assert_eq!(current["checks"][0]["lineage_state"], "current");
    assert_eq!(current["checks"][0]["filesystem_state"], "current");
    assert_eq!(
        current["checks"][0]["head_content_hash"],
        current["checks"][0]["live_content_hash"]
    );

    fs::write(&source, "bravo").unwrap();
    let modified = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(modified["checks"][0]["lineage_state"], "current");
    assert_eq!(modified["checks"][0]["filesystem_state"], "modified");
    assert_ne!(
        modified["checks"][0]["head_content_hash"],
        modified["checks"][0]["live_content_hash"]
    );

    fs::remove_file(&source).unwrap();
    let missing = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(missing["checks"][0]["filesystem_state"], "missing");
    assert!(missing["checks"][0]["live_content_hash"].is_null());

    let oversized = fs::File::create(&source).unwrap();
    oversized.set_len(64 * 1024 * 1024 + 1).unwrap();
    let oversized = world.ok(&world.project, &["source", "status", &source_id]);
    assert_eq!(oversized["checks"][0]["filesystem_state"], "oversized");
    assert!(oversized["checks"][0]["live_content_hash"].is_null());

    let unknown = world.err(&world.project, &["source", "status", "999"]);
    assert_eq!(unknown["error"]["code"], "source_not_found");

    let after = world.ok(&world.project, &["log", "--limit", "100"]);
    assert_eq!(before["operations"], after["operations"]);
}

#[test]
fn source_diff_exposes_one_character_live_changes_and_is_read_only() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/diff.md", "first\nport=80\nlast\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let before = world.ok(&world.project, &["log", "--limit", "100"]);

    fs::write(&source, "first\nport=81\nlast\n").unwrap();
    let changed = world.ok(&world.project, &["source", "diff", &source_id]);
    assert_eq!(changed["scope"], "project");
    assert_eq!(changed["from"]["kind"], "source");
    assert_eq!(changed["from"]["source_id"], added["source"]["id"]);
    assert_eq!(changed["from"]["bytes"], 19);
    assert_eq!(changed["to"]["kind"], "live");
    assert_eq!(changed["to"]["tracked_path"], "raw/diff.md");
    assert_eq!(changed["to"]["head_source_id"], added["source"]["id"]);
    assert_eq!(changed["to"]["head_revision"], 1);
    assert_eq!(changed["to"]["bytes"], 19);
    assert_ne!(
        changed["from"]["content_hash"],
        changed["to"]["content_hash"]
    );
    assert_eq!(changed["changed"], true);
    assert_eq!(changed["diff"]["format"], "unified");
    assert_eq!(changed["diff"]["context_lines"], 3);
    let text = changed["diff"]["text"].as_str().unwrap();
    assert!(text.starts_with("--- source:1\n+++ live:raw/diff.md\n"));
    assert!(text.contains("-port=80\n+port=81\n"));
    assert_eq!(
        changed["diff"]["returned_chars"].as_u64().unwrap(),
        text.chars().count() as u64
    );
    assert_eq!(
        changed["diff"]["total_chars"],
        changed["diff"]["returned_chars"]
    );
    assert_eq!(changed["diff"]["truncated"], false);

    fs::write(&source, "first\nport=80\nlast\n").unwrap();
    let current = world.ok(&world.project, &["source", "diff", &source_id]);
    assert_eq!(current["changed"], false);
    assert_eq!(current["diff"]["text"], "");
    assert_eq!(current["diff"]["returned_chars"], 0);
    assert_eq!(current["diff"]["total_chars"], 0);
    assert_eq!(current["diff"]["truncated"], false);

    let after = world.ok(&world.project, &["log", "--limit", "100"]);
    assert_eq!(before["operations"], after["operations"]);
}

#[test]
fn source_diff_compares_immutable_snapshots_without_a_live_file() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/history.md", "setting=old\n");
    let old = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let old_id = old["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&source, "setting=new\n").unwrap();
    let new = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let new_id = new["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&source).unwrap();

    let forward = world.ok(
        &world.project,
        &["source", "diff", &old_id, "--to-source", &new_id],
    );
    assert_eq!(forward["from"]["source_id"], old["source"]["id"]);
    assert_eq!(forward["to"]["kind"], "source");
    assert_eq!(forward["to"]["source_id"], new["source"]["id"]);
    assert!(
        forward["diff"]["text"]
            .as_str()
            .unwrap()
            .starts_with("--- source:1\n+++ source:2\n")
    );
    assert!(
        forward["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-setting=old\n+setting=new\n")
    );

    let reverse = world.ok(
        &world.project,
        &["source", "diff", &new_id, "--to-source", &old_id],
    );
    assert!(
        reverse["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-setting=new\n+setting=old\n")
    );

    let same = world.ok(
        &world.project,
        &["source", "diff", &old_id, "--to-source", &old_id],
    );
    assert_eq!(same["changed"], false);
    assert_eq!(same["diff"]["text"], "");

    for conflicting in [
        "--path",
        "--allow-external-source",
        "--acknowledge-sensitive-source",
    ] {
        let mut args = vec![
            "source",
            "diff",
            &old_id,
            "--to-source",
            &new_id,
            conflicting,
        ];
        if conflicting == "--path" {
            args.push("raw/history.md");
        }
        let output = world.command(&world.project, &args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}
