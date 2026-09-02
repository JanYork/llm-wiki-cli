#[test]
fn changeset_show_is_metadata_only_and_does_not_run_lint() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "metadata-only"]);
    let body = world.write("draft-lint.md", "Real draft link [[missing-target]].");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "metadata-only",
            "page",
            "put",
            "draft-lint",
            "--title",
            "Draft lint",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ],
    );

    let shown = world.ok(&world.project, &["changeset", "show", "metadata-only"]);
    assert_eq!(shown["staged_operation_count"], 1);
    assert!(
        shown.get("lint_issues").is_none(),
        "changeset show must not perform or report implicit lint: {shown}"
    );
}

#[test]
fn point_page_put_does_not_build_a_complete_artifact_snapshot() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let body = world.write(
        "targeted-artifact.md",
        "targeted artifact alpha beta evidence",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .env("LWC_TEST_FORBID_FULL_ARTIFACT_SNAPSHOT", "1")
        .args([
            "page",
            "put",
            "targeted-artifact",
            "--title",
            "Targeted artifact",
            "--file",
            as_str(&body),
            "--provenance",
            "agent-observed",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "point mutation invoked complete artifact snapshot: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn graph_config_rejects_symlink_all_scope_and_changeset_mutation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let outside = world.home.join("outside-config.json");
    fs::write(&outside, "sentinel").unwrap();
    std::os::unix::fs::symlink(&outside, world.project.join(".lwc/config.json")).unwrap();
    let error = world.err(&world.project, &["config", "set", "--graph", "grafeo"]);
    assert_eq!(error["error"]["code"], "unsafe_config_path");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "sentinel");

    let all = world.err(&world.project, &["--scope", "all", "config", "show"]);
    assert_eq!(all["error"]["code"], "scope_not_supported");

    let second = TestWorld::new();
    second.ok(&second.project, &["init"]);
    second.ok(&second.project, &["changeset", "begin", "config-guard"]);
    let staged = second.err(
        &second.project,
        &[
            "--changeset",
            "config-guard",
            "config",
            "set",
            "--graph",
            "grafeo",
        ],
    );
    assert_eq!(staged["error"]["code"], "changeset_command_not_supported");
}

#[test]
fn canonical_graph_supports_exploration_paths_relations_impact_and_overview() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "beta", "Beta", "Target knowledge.");
    put_page(&world, "alpha", "Alpha", "Source knowledge. [[beta]]");
    let enabled = world.ok(&world.project, &["config", "set", "--graph", "grafeo"]);
    let work_id = enabled["work"]["id"].as_str().unwrap();
    assert_eq!(
        world.ok(&world.project, &["work", "watch", work_id])["work"]["state"],
        "succeeded"
    );
    assert!(!world.project.join(".lwc/work/active").exists());

    let node = world.ok(&world.project, &["graph", "node", "page:alpha"]);
    assert_eq!(node["node"]["label"], "Alpha");
    let explored = world.ok(
        &world.project,
        &["graph", "explore", "page:alpha", "--direction", "outgoing"],
    );
    assert!(
        explored["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["type"] == "LINKS_TO")
    );
    let neighbors = world.ok(
        &world.project,
        &[
            "graph",
            "neighbors",
            "page:alpha",
            "--direction",
            "outgoing",
        ],
    );
    assert_eq!(neighbors["neighbors"][0]["identifier"], "page:beta");
    let path = world.ok(
        &world.project,
        &["graph", "path", "page:alpha", "page:beta"],
    );
    assert_eq!(path["found"], true);
    assert_eq!(path["path"], serde_json::json!(["page:alpha", "page:beta"]));
    let relation = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:alpha",
            "SUPPORTS",
            "page:beta",
            "--provenance",
            "agent-observed",
            "--reason",
            "verified relation",
            "--confidence",
            "0.9",
        ],
    );
    let work_id = relation["work"]["id"].as_str().unwrap();
    assert_eq!(
        world.ok(&world.project, &["work", "watch", work_id])["work"]["state"],
        "succeeded"
    );
    let explored = world.ok(
        &world.project,
        &["graph", "explore", "page:alpha", "--direction", "outgoing"],
    );
    assert!(
        explored["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["type"] == "SUPPORTS")
    );
    let impact = world.ok(&world.project, &["graph", "impact", "page:beta"]);
    assert!(
        impact["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["identifier"] == "page:alpha")
    );
    let overview = world.ok(&world.project, &["graph", "overview"]);
    assert_eq!(overview["documents"], 2);
    assert_eq!(overview["edges"], 2);
    let verified = world.ok(&world.project, &["graph", "verify"]);
    assert_eq!(verified["ok"], true, "{verified}");
}

#[test]
fn semantic_relation_lifecycle_validates_all_types_provenance_sources_and_retraction() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["config", "set", "--graph", "grafeo"]);
    let source = world.write("relation-source.md", "Semantic relation evidence.");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    put_page(&world, "relation-a", "Relation A", "Relation source node.");
    put_page(&world, "relation-b", "Relation B", "Relation target node.");

    for relation_type in [
        "SUPPORTS",
        "CONTRADICTS",
        "REFINES",
        "SUPERSEDES",
        "CAUSES",
        "DEPENDS_ON",
    ] {
        world.ok(
            &world.project,
            &[
                "graph",
                "relation",
                "set",
                "page:relation-a",
                relation_type,
                "page:relation-b",
                "--provenance",
                "agent-observed",
                "--reason",
                "validated semantic relation",
                "--confidence",
                "0.8",
            ],
        );
    }
    let listed = world.ok(&world.project, &["graph", "relation", "list"]);
    assert_eq!(listed["relations"].as_array().unwrap().len(), 6);

    for arguments in [
        vec!["LINKS_TO", "agent-observed", "reason", "0.8"],
        vec!["SUPPORTS", "automatic", "reason", "0.8"],
        vec!["SUPPORTS", "agent-observed", "reason", "1.2"],
        vec!["SUPPORTS", "agent-observed", "", "0.8"],
    ] {
        let error = world.err(
            &world.project,
            &[
                "graph",
                "relation",
                "set",
                "page:relation-a",
                arguments[0],
                "page:relation-b",
                "--provenance",
                arguments[1],
                "--reason",
                arguments[2],
                "--confidence",
                arguments[3],
            ],
        );
        assert!(
            error["error"]["code"]
                .as_str()
                .unwrap()
                .starts_with("invalid_")
        );
    }
    let missing_source = world.err(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:relation-a",
            "SUPPORTS",
            "page:relation-b",
            "--provenance",
            "source-grounded",
            "--reason",
            "requires evidence",
            "--confidence",
            "0.9",
        ],
    );
    assert_eq!(missing_source["error"]["code"], "invalid_semantic_relation");
    let grounded = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            "page:relation-a",
            "SUPPORTS",
            "page:relation-b",
            "--provenance",
            "source-grounded",
            "--reason",
            "grounded evidence",
            "--confidence",
            "0.9",
            "--source",
            &source_id,
        ],
    );
    assert_eq!(
        grounded["relation"]["source_ids"][0],
        source_id.parse::<i64>().unwrap()
    );

    let retracted = world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "retract",
            "page:relation-a",
            "DEPENDS_ON",
            "page:relation-b",
            "--reason",
            "dependency removed",
        ],
    );
    assert_eq!(retracted["retracted"], true);
}

#[test]
fn document_removal_preserves_stale_span_diagnostics_when_graph_is_disabled() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "temporary-graph-page",
        "Temporary graph page",
        "RemovalNeedle remains locatable until deletion.",
    );
    let found = world.ok(
        &world.project,
        &["search", "RemovalNeedle", "--granularity", "sentence"],
    );
    let span_id = found["results"][0]["identifier"].as_str().unwrap();

    world.ok(&world.project, &["page", "remove", "temporary-graph-page"]);
    let after = world.ok(
        &world.project,
        &[
            "search",
            "RemovalNeedle",
            "--granularity",
            "all",
            "--group-by",
            "none",
        ],
    );
    assert!(after["results"].as_array().unwrap().is_empty());
    let stale = world.err(&world.project, &["span", "get", span_id]);
    assert_eq!(stale["error"]["code"], "stale_span");
    let missing = world.err(
        &world.project,
        &["graph", "explore", "page:temporary-graph-page"],
    );
    assert_eq!(missing["error"]["code"], "graph_disabled");
}

#[test]
fn every_public_command_exposes_renderable_help() {
    let world = TestWorld::new();
    let commands = [
        "init",
        "changeset",
        "changeset begin",
        "changeset list",
        "changeset show",
        "changeset commit",
        "changeset discard",
        "changeset rollback",
        "schema",
        "schema set",
        "schema show",
        "purpose",
        "purpose set",
        "purpose show",
        "source",
        "source add",
        "source add-dir",
        "source add-manifest",
        "source list",
        "source status",
        "source diff",
        "source show",
        "source refs",
        "source remove",
        "page",
        "page put",
        "page list",
        "page show",
        "page links",
        "page remove",
        "ingest",
        "ingest list",
        "ingest next",
        "ingest claim",
        "ingest analyze",
        "ingest complete",
        "ingest fail",
        "ingest retry",
        "config",
        "config show",
        "config set",
        "config unset",
        "remember",
        "memory",
        "memory recall",
        "memory show",
        "memory feedback",
        "memory status",
        "memory maintain",
        "graph",
        "graph related",
        "graph explore",
        "graph node",
        "graph neighbors",
        "graph path",
        "graph impact",
        "graph overview",
        "graph status",
        "graph verify",
        "graph relation",
        "graph relation set",
        "graph relation list",
        "graph relation retract",
        "span",
        "span get",
        "span expand",
        "maintenance",
        "maintenance materialize",
        "maintenance reindex",
        "maintenance compact",
        "work",
        "work list",
        "work status",
        "work watch",
        "work cancel",
        "work resume",
        "checkpoint",
        "checkpoint create",
        "checkpoint list",
        "checkpoint restore",
        "search",
        "context",
        "lint",
        "log",
    ];

    for command in commands {
        let mut args = command.split_whitespace().collect::<Vec<_>>();
        args.push("--help");
        let output = world.command(&world.project, &args);
        assert!(
            output.status.success(),
            "help command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            help.contains("Usage:") && help.len() >= 120,
            "help command {args:?} is unexpectedly incomplete\n{help}"
        );
    }
}

#[test]
fn project_flow_preserves_sources_and_rolls_back_failed_page_updates() {
    let world = TestWorld::new();

    let initialized = world.ok(&world.project, &["init"]);
    assert_eq!(initialized["scope"], "project");
    assert_eq!(
        initialized["recommendations"]["md_trans"]["default"],
        "disabled"
    );
    assert_eq!(
        initialized["recommendations"]["md_trans"]["config_show"],
        "lwc config show"
    );
    assert_eq!(
        initialized["recommendations"]["md_trans"]["engines"]["anydoc"]["configure"],
        "lwc config set --trans anydoc"
    );
    assert_eq!(
        initialized["recommendations"]["md_trans"]["engines"]["markitdown"]["configure"],
        "lwc config set --trans markitdown"
    );
    assert!(
        !world.project.join(".lwc/config.json").exists(),
        "init guidance must not enable or configure an adapter"
    );
    assert!(world.project.join(".lwc/wiki.db").is_file());

    let schema = world.write(
        "schema.md",
        "# Research Wiki\nEvery factual page cites its raw sources.",
    );
    world.ok(&world.project, &["schema", "set", as_str(&schema)]);
    let shown_schema = world.ok(&world.project, &["schema", "show"]);
    assert_eq!(
        shown_schema["schema"],
        "# Research Wiki\nEvery factual page cites its raw sources."
    );

    let source = world.write(
        "raw/ownership.md",
        "Rust ownership assigns one owner to each value.",
    );
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Ownership source",
        ],
    );
    assert_eq!(added["created"], true);
    assert_eq!(added["source"]["id"], 1);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let duplicate = world.write(
        "elsewhere/same.md",
        "Rust ownership assigns one owner to each value.",
    );
    let reused = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&duplicate),
            "--title",
            "Ignored title",
        ],
    );
    assert_eq!(reused["created"], false);
    assert_eq!(reused["source"]["id"], 1);
    assert_eq!(reused["source"]["title"], "Ownership source");
    assert_eq!(reused["source"]["origin"], as_str(&source));

    let page = world.write(
        "ownership-page.md",
        "Ownership connects to [[borrowing]] and repeats [[borrowing]].",
    );
    let put = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "ownership",
            "--title",
            "Ownership",
            "--kind",
            "concept",
            "--summary",
            "How Rust assigns values",
            "--file",
            as_str(&page),
            "--source",
            &source_id,
            "--source",
            &source_id,
        ],
    );
    assert_eq!(put["created"], true);
    assert_eq!(put["page"]["source_ids"], serde_json::json!([1]));
    assert_eq!(put["page"]["links"], serde_json::json!(["borrowing"]));

    let search = world.ok(&world.project, &["search", "ownership"]);
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["type"] == "page" && result["identifier"] == "ownership")
    );

    let context = world.ok(&world.project, &["context", "--limit", "10"]);
    assert_eq!(context["stores"][0]["scope"], "project");
    assert_eq!(context["stores"][0]["pages"][0]["slug"], "ownership");
    assert!(context["stores"][0]["recent_operations"].is_array());

    let lint = world.ok(&world.project, &["lint"]);
    assert!(
        lint["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "dangling_link" && issue["target"] == "borrowing")
    );

    let before_log = world.ok(&world.project, &["log", "--limit", "100"]);
    let before_page_puts = before_log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["action"] == "page_put")
        .count();

    let replacement = world.write("replacement.md", "Broken replacement [[other]].");
    let failed = world.err(
        &world.project,
        &[
            "page",
            "put",
            "ownership",
            "--title",
            "Replacement",
            "--file",
            as_str(&replacement),
            "--source",
            "999",
        ],
    );
    assert_eq!(failed["error"]["code"], "source_not_found");

    let unchanged = world.ok(&world.project, &["page", "show", "ownership"]);
    assert_eq!(unchanged["page"]["title"], "Ownership");
    assert_eq!(unchanged["page"]["links"], serde_json::json!(["borrowing"]));

    let after_log = world.ok(&world.project, &["log", "--limit", "100"]);
    let after_page_puts = after_log["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["action"] == "page_put")
        .count();
    assert_eq!(after_page_puts, before_page_puts);
}

#[test]
fn page_provenance_is_structured_on_every_read_surface_and_replaced() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("evidence.md", "immutable evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let page = world.write("mixed.md", "provenanceterm mixed knowledge");
    let expected = serde_json::json!([
        "source-grounded",
        "user-provided",
        "agent-observed",
        "hypothesis"
    ]);

    let put = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "mixed-provenance",
            "--title",
            "Mixed provenance",
            "--summary",
            "Structured provenance test",
            "--file",
            as_str(&page),
            "--source",
            &source_id,
            "--provenance",
            "hypothesis",
            "--provenance",
            "agent-observed",
            "--provenance",
            "user-provided",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(put["page"]["provenance"], expected);

    let shown = world.ok(&world.project, &["page", "show", "mixed-provenance"]);
    assert_eq!(shown["page"]["provenance"], expected);

    let listed = world.ok(&world.project, &["page", "list"]);
    assert_eq!(listed["pages"][0]["provenance"], expected);

    let context = world.ok(&world.project, &["context", "--limit", "10"]);
    assert_eq!(context["stores"][0]["pages"][0]["provenance"], expected);

    let search = world.ok(&world.project, &["search", "provenanceterm"]);
    let page_hit = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["type"] == "page")
        .unwrap();
    assert_eq!(page_hit["provenance"], expected);

    let refs = world.ok(&world.project, &["source", "refs", &source_id]);
    assert_eq!(refs["pages"][0]["provenance"], expected);

    let replacement = world.write("replacement.md", "replacement body");
    let replaced = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "mixed-provenance",
            "--title",
            "Mixed provenance",
            "--summary",
            "Structured provenance test",
            "--file",
            as_str(&replacement),
            "--source",
            &source_id,
        ],
    );
    assert_eq!(
        replaced["page"]["provenance"],
        serde_json::json!(["source-grounded"])
    );
}

#[test]
fn lint_accepts_explicit_provenance_but_reports_unclassified_pages() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    for (slug, provenance) in [
        ("classified", Some("agent-observed")),
        ("unclassified", None),
    ] {
        let page = world.write(&format!("{slug}.md"), &format!("{slug} body"));
        let mut args = vec![
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--summary",
            "summary",
            "--file",
            as_str(&page),
        ];
        if let Some(value) = provenance {
            args.extend(["--provenance", value]);
        }
        world.ok(&world.project, &args);
    }

    let lint = world.ok(&world.project, &["lint"]);
    let uncited = lint["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|issue| issue["code"] == "uncited_page")
        .map(|issue| issue["page"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!uncited.contains(&"classified"));
    assert!(uncited.contains(&"unclassified"));
}

#[test]
fn nearest_project_is_discovered_from_nested_directories() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let nested = world.project.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    let listed = world.ok(&nested, &["page", "list"]);
    assert_eq!(listed["scope"], "project");
    assert_eq!(
        fs::canonicalize(Path::new(listed["database"].as_str().unwrap())).unwrap(),
        fs::canonicalize(world.project.join(".lwc/wiki.db")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn readonly_store_allows_unrecorded_search_but_rejects_recording() {
    use std::os::unix::fs::PermissionsExt;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let page = world.write("readonly.md", "readonly-query-term");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "readonly",
            "--title",
            "Readonly",
            "--file",
            as_str(&page),
        ],
    );

    let db_path = world.project.join(".lwc/wiki.db");
    let lwc_dir = world.project.join(".lwc");
    fs::set_permissions(&db_path, fs::Permissions::from_mode(0o444)).unwrap();
    fs::set_permissions(&lwc_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let search = world.ok(
        &world.project,
        &["search", "readonly-query-term", "--explain"],
    );
    assert_eq!(search["results"][0]["identifier"], "readonly");
    assert!(search["results"][0]["explanation"].is_object());

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(context["stores"][0]["pages"][0]["slug"], "readonly");

    let recorded = world.err(
        &world.project,
        &["search", "readonly-query-term", "--record"],
    );
    assert_eq!(recorded["error"]["code"], "database_error");
    assert!(
        recorded["error"]["message"]
            .as_str()
            .unwrap()
            .contains("readonly database")
    );
}

#[test]
fn read_commands_transparently_migrate_a_writable_v5_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let database = world.project.join(".lwc/wiki.db");
    let conn = Connection::open(&database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "DROP TABLE page_tags;
         DROP TABLE tags;
         DROP TABLE search_fts;
         DROP TABLE IF EXISTS search_fts_data;
         DROP TABLE IF EXISTS search_fts_idx;
         DROP TABLE IF EXISTS search_fts_content;
         DROP TABLE IF EXISTS search_fts_docsize;
         DROP TABLE IF EXISTS search_fts_config;
         DROP TABLE source_path_revisions;
         DROP TABLE page_provenance;
         DROP TABLE retrieval_feedback;
         DROP TABLE retrieval_weights;
         ALTER TABLE sources DROP COLUMN structural_navigation;
         ALTER TABLE pages DROP COLUMN structural_navigation;
         CREATE VIRTUAL TABLE search_fts USING fts5(
             doc_type UNINDEXED,
             identifier UNINDEXED,
             title_terms,
             summary_terms,
             body_terms
         );
         ALTER TABLE ingest_jobs DROP COLUMN no_derived_pages_reason;
         UPDATE meta SET value = '5' WHERE key = 'format_version';
         PRAGMA user_version = 5;",
    )
    .unwrap();
    drop(conn);

    let context = world.ok(&world.project, &["context", "--limit", "5"]);
    assert_eq!(context["stores"][0]["scope"], "project");

    let conn = Connection::open(database).unwrap();
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 18,
        "context should migrate a writable legacy store"
    );
}

#[test]
fn v10_commands_migrate_inline_without_building_a_graph() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "migration-work",
        "Migration work",
        "migration work evidence",
    );
    let database = world.project.join(".lwc/wiki.db");
    downgrade_to_v10(&database);

    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .env("LWC_TEST_MIGRATION_DELAY_MS", "5000")
        .args(["context", "--limit", "5"])
        .output()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "old-schema preflight failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = stdout_json(&output);
    assert_eq!(response["stores"][0]["scope"], "project", "{response}");
    assert!(
        elapsed < Duration::from_secs(2),
        "document-only migration took {} ms",
        elapsed.as_millis()
    );
    let migrated = Connection::open(database).unwrap();
    assert_eq!(
        migrated
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        18
    );
    let old_graph_tables: i64 = migrated
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name GLOB 'graph_*'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_graph_tables, 0);
}

#[test]
fn removed_graph_migration_leaves_the_live_writer_available() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "shadow-migration",
        "Shadow migration",
        "alpha beta gamma",
    );
    let database = world.project.join(".lwc/wiki.db");
    downgrade_to_v10(&database);

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["context", "--limit", "5"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let live = Connection::open(&database).unwrap();
    live.busy_timeout(Duration::from_millis(100)).unwrap();
    live.execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
        .expect("removed graph migration must not lock the live Wiki writer");
}
