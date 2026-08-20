#[test]
fn source_generic_detection_preserves_structural_body_signals() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write(
        "payment-reconciliation-guide.md",
        "payment reconciliation rules\n\n文档目录",
    );
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Payment Reconciliation Guide",
        ],
    );
    let identifier = added["source"]["id"].to_string();
    let search = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation rules",
            "--type",
            "source",
            "--explain",
        ],
    );
    let source = find_result(&search, &identifier);
    assert_eq!(source["explanation"]["signals"]["generic_marker"], 1.0);
}

#[test]
fn document_weights_are_bounded_reversible_precedence_aware_and_candidate_only() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(&world, "alpha", "Alpha", "calibration candidate sharedterm");
    put_page(&world, "beta", "Beta", "calibration candidate sharedterm");
    put_page(&world, "absent", "Absent", "different vocabulary only");

    let mut ranks = Vec::new();
    for value in [-2, -1, 1, 2] {
        let value_text = value.to_string();
        world.ok(
            &world.project,
            &[
                "weight",
                "set",
                "page",
                "alpha",
                "--value",
                &value_text,
                "--reason",
                "monotonic calibration",
                "--provenance",
                "agent-observed",
            ],
        );
        let search = world.ok(
            &world.project,
            &[
                "search",
                "calibration candidate sharedterm",
                "--type",
                "page",
                "--explain",
            ],
        );
        let alpha = find_result(&search, "alpha");
        assert_eq!(
            alpha["explanation"]["signals"]["manual_adjustment"],
            value as f64 / 2.0
        );
        ranks.push(alpha["rank"].as_f64().unwrap());
    }
    assert!(ranks.windows(2).all(|pair| pair[0] > pair[1]), "{ranks:?}");
    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "alpha",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "beta",
            "--value",
            "2",
            "--reason",
            "verified canonical page",
            "--provenance",
            "agent-observed",
        ],
    );
    let boosted = world.ok(
        &world.project,
        &[
            "search",
            "calibration candidate sharedterm",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(boosted["results"][0]["identifier"], "beta");
    assert_eq!(
        find_result(&boosted, "beta")["explanation"]["signals"]["manual_adjustment"],
        1.0
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "beta",
            "--value",
            "-1",
            "--reason",
            "user says this page is secondary",
            "--provenance",
            "user-provided",
        ],
    );
    let listed = world.ok(&world.project, &["weight", "list", "page", "beta"]);
    assert_eq!(listed["adjustments"].as_array().unwrap().len(), 2);
    assert_eq!(listed["effective"]["provenance"], "user-provided");
    assert_eq!(listed["effective"]["weight"], -1);

    for invalid in ["-3", "0", "3", "1.5"] {
        let error = world.err(
            &world.project,
            &[
                "weight",
                "set",
                "page",
                "alpha",
                "--value",
                invalid,
                "--reason",
                "invalid bound",
                "--provenance",
                "agent-observed",
            ],
        );
        assert_eq!(error["error"]["code"], "invalid_weight");
    }
    let empty_reason = world.err(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "alpha",
            "--value",
            "1",
            "--reason",
            " ",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(empty_reason["error"]["code"], "invalid_input");

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "absent",
            "--value",
            "2",
            "--reason",
            "must not create a lexical candidate",
            "--provenance",
            "agent-observed",
        ],
    );
    let search = world.ok(
        &world.project,
        &["search", "calibration candidate sharedterm"],
    );
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["identifier"] != "absent")
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "user-provided",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "agent-observed",
        ],
    );
    let already_cleared = world.ok(
        &world.project,
        &[
            "weight",
            "clear",
            "page",
            "beta",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(already_cleared["removed"], false);
    let cleared = world.ok(&world.project, &["weight", "list", "page", "beta"]);
    assert!(cleared["adjustments"].as_array().unwrap().is_empty());
    assert!(cleared["effective"].is_null());
}

#[test]
fn explicit_feedback_is_query_specific_private_precedence_aware_and_reversible() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    put_page(
        &world,
        "preferred",
        "Preferred",
        "private feedback retrievalterm",
    );
    put_page(&world, "other", "Other", "private feedback retrievalterm");
    let distinctive_query = "retrievalterm, private feedback 7f4a9";

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "relevant",
            "--reason",
            "explicit result judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "irrelevant",
            "--reason",
            "replacement judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "relevant",
            "--reason",
            "final agent judgment",
            "--provenance",
            "agent-observed",
        ],
    );
    let empty_query = world.err(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            "?!",
            "--signal",
            "relevant",
            "--reason",
            "invalid query fixture",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(empty_query["error"]["code"], "invalid_query");
    let equivalent = world.ok(
        &world.project,
        &[
            "search",
            "retrievalterm private feedback 7f4a9!",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(
        find_result(&equivalent, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        1.0
    );
    let paraphrase = world.ok(
        &world.project,
        &[
            "search",
            "private retrieval feedback changed words",
            "--type",
            "page",
            "--explain",
        ],
    );
    assert_eq!(
        find_result(&paraphrase, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        0.0
    );

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--signal",
            "irrelevant",
            "--reason",
            "user overrides agent judgment",
            "--provenance",
            "user-provided",
        ],
    );
    let overridden = world.ok(
        &world.project,
        &["search", distinctive_query, "--type", "page", "--explain"],
    );
    assert_eq!(
        find_result(&overridden, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        -1.0
    );

    let conn = Connection::open(database).unwrap();
    let feedback_dump: String = conn
        .query_row(
            "SELECT GROUP_CONCAT(query_fingerprint || reason, '|') FROM retrieval_feedback",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let operation_dump: String = conn
        .query_row(
            "SELECT GROUP_CONCAT(detail_json, '|') FROM operations WHERE action LIKE 'weight_feedback%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let feedback_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM retrieval_feedback", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(feedback_rows, 2);
    assert!(!feedback_dump.contains(distinctive_query));
    assert!(!operation_dump.contains(distinctive_query));

    world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "user-provided",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "agent-observed",
        ],
    );
    let already_cleared = world.ok(
        &world.project,
        &[
            "weight",
            "feedback-clear",
            "page",
            "preferred",
            "--query",
            distinctive_query,
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(already_cleared["removed"], false);
    let cleared = world.ok(
        &world.project,
        &["search", distinctive_query, "--type", "page", "--explain"],
    );
    assert_eq!(
        find_result(&cleared, "preferred")["explanation"]["signals"]["feedback_adjustment"],
        0.0
    );
}

#[test]
fn retrieval_state_is_deleted_with_its_target_and_all_scope_mutations_are_rejected() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    put_page(&world, "temporary", "Temporary", "cleanupterm");
    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "temporary",
            "--value",
            "1",
            "--reason",
            "temporary",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "weight",
            "feedback",
            "page",
            "temporary",
            "--query",
            "cleanupterm",
            "--signal",
            "relevant",
            "--reason",
            "temporary",
            "--provenance",
            "agent-observed",
        ],
    );

    let rejected = world.err(
        &world.project,
        &[
            "--scope",
            "all",
            "weight",
            "set",
            "page",
            "temporary",
            "--value",
            "1",
            "--reason",
            "must reject",
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(rejected["error"]["code"], "scope_not_supported");

    world.ok(&world.project, &["page", "remove", "temporary"]);
    let conn = Connection::open(database).unwrap();
    for table in ["retrieval_weights", "retrieval_feedback"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "orphaned rows in {table}");
    }
}

#[test]
fn lint_reports_orphaned_retrieval_state() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let database = initialized["database"].as_str().unwrap();
    let conn = Connection::open(database).unwrap();
    conn.execute(
        "INSERT INTO retrieval_weights(
            target_type, target_identifier, provenance, weight, reason
         ) VALUES ('page', 'missing-page', 'agent-observed', 1, 'fixture')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO retrieval_feedback(
            query_fingerprint, target_type, target_identifier, provenance, signal, reason
         ) VALUES (?1, 'source', '999', 'agent-observed', 1, 'fixture')",
        ["a".repeat(64)],
    )
    .unwrap();
    drop(conn);

    let lint = world.ok(&world.project, &["lint"]);
    assert_eq!(lint["counts"]["retrieval_weight_orphan"], 1);
    assert_eq!(lint["counts"]["retrieval_feedback_orphan"], 1);
}

#[test]
fn graph_reranking_is_bounded_query_conditioned_and_does_not_promote_sources() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "seed",
        "Graph calibration seed",
        "graph calibration seedterm [[linked-candidate]]",
    );
    put_page(
        &world,
        "linked-candidate",
        "Linked candidate",
        "graph calibration seedterm",
    );
    put_page(
        &world,
        "unrelated-hub-readme",
        "Unrelated Hub README",
        "graph calibration seedterm [[x]] [[y]] [[z]] [[q]]",
    );
    let raw = world.write("raw.md", "graph calibration seedterm");
    world.ok(
        &world.project,
        &["source", "add", as_str(&raw), "--title", "Raw graph source"],
    );

    let search = world.ok(
        &world.project,
        &[
            "search",
            "graph calibration seedterm",
            "--type",
            "all",
            "--explain",
        ],
    );
    let linked = find_result(&search, "linked-candidate");
    let hub = find_result(&search, "unrelated-hub-readme");
    assert!(
        linked["explanation"]["signals"]["graph_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        linked["explanation"]["signals"]["graph_match"]
            .as_f64()
            .unwrap()
            <= 1.0
    );
    assert_eq!(hub["explanation"]["signals"]["graph_match"], 0.0);
    let hub_penalty = hub["explanation"]["signals"]["graph_hub_penalty"]
        .as_f64()
        .unwrap();
    assert!(hub_penalty > 0.0 && hub_penalty <= 1.0);
    assert!(
        hub["explanation"]["contributions"]["graph"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        linked["explanation"]["contributions"]["graph"]
            .as_f64()
            .unwrap()
            < 0.0
    );
    let raw_result = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["type"] == "source")
        .unwrap();
    assert_eq!(raw_result["explanation"]["signals"]["graph_match"], 0.0);

    put_page(
        &world,
        "common-seed",
        "Common seed",
        "commononlyzeta [[shared-neighbor]]",
    );
    put_page(
        &world,
        "common-sibling",
        "Common sibling",
        "commononlyzeta [[shared-neighbor]]",
    );
    put_page(
        &world,
        "shared-neighbor",
        "Shared neighbor",
        "structural connector only",
    );
    let common = world.ok(
        &world.project,
        &["search", "commononlyzeta", "--type", "page", "--explain"],
    );
    for slug in ["common-seed", "common-sibling"] {
        assert_eq!(
            find_result(&common, slug)["explanation"]["signals"]["graph_match"],
            0.0,
            "common-neighbor-only evidence must not affect search ranking"
        );
    }

    let shared_source = world.write("shared-graph-source.md", "shared graph evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&shared_source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    for slug in ["shared-source-a", "shared-source-b"] {
        let body = world.write(&format!("pages/{slug}.md"), "sharedsourcezeta");
        world.ok(
            &world.project,
            &[
                "page",
                "put",
                slug,
                "--title",
                slug,
                "--file",
                as_str(&body),
                "--source",
                &source_id,
            ],
        );
    }
    let shared = world.ok(
        &world.project,
        &["search", "sharedsourcezeta", "--type", "page", "--explain"],
    );
    for slug in ["shared-source-a", "shared-source-b"] {
        assert!(
            find_result(&shared, slug)["explanation"]["signals"]["graph_match"]
                .as_f64()
                .unwrap()
                > 0.0
        );
    }
}

#[test]
fn graph_config_resolves_layers_and_updates_project_atomically() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let defaults = world.ok(&world.project, &["config", "show"]);
    assert_eq!(defaults["graph"]["setting"], "disabled");
    assert_eq!(defaults["graph"]["origin"], "built-in");

    let set = world.ok(&world.project, &["config", "set", "--graph", "grafeo"]);
    assert_eq!(set["graph"]["setting"], "grafeo");
    assert_eq!(set["graph"]["origin"], "project");

    let inherited = world.ok(&world.project, &["config", "unset", "--graph"]);
    assert_eq!(inherited["graph"]["setting"], "disabled");
    assert_eq!(inherited["graph"]["origin"], "built-in");

    world.ok(&world.project, &["--scope", "global", "init"]);
    world.ok(
        &world.project,
        &["--scope", "global", "config", "set", "--graph", "surrealdb"],
    );
    let layered = world.ok(&world.project, &["config", "show"]);
    assert_eq!(layered["graph"]["setting"], "surrealdb");
    assert_eq!(layered["graph"]["origin"], "global");
}

#[test]
fn graph_is_disabled_by_default_and_removed_engines_are_rejected() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let config = world.ok(&world.project, &["config", "show"]);
    assert_eq!(config["graph"]["setting"], "disabled");
    assert_eq!(config["graph"]["origin"], "built-in");

    for removed in ["rslg", "graphqlite", "auto"] {
        let error = world.err(&world.project, &["config", "set", "--graph", removed]);
        assert_eq!(error["error"]["code"], "invalid_graph_engine");
    }
}

#[test]
fn trans_is_disabled_by_default_and_resolves_global_then_project_profiles() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let defaults = world.ok(&world.project, &["config", "show"]);
    assert_eq!(defaults["trans"]["setting"], "disabled");
    assert_eq!(defaults["trans"]["origin"], "built-in");
    assert_eq!(defaults["trans"]["timeout_seconds"], 120);
    assert_eq!(defaults["trans"]["anydoc_args"], serde_json::json!([]));
    assert_eq!(defaults["trans"]["markitdown_args"], serde_json::json!([]));

    world.ok(&world.project, &["--scope", "global", "init"]);
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-timeout",
            "45",
            "--trans-arg",
            "--fast",
            "--trans-arg",
            "pdf",
        ],
    );
    let layered = world.ok(&world.project, &["config", "show"]);
    assert_eq!(layered["trans"]["setting"], "markitdown");
    assert_eq!(layered["trans"]["origin"], "global");
    assert_eq!(layered["trans"]["timeout_seconds"], 45);
    assert_eq!(layered["trans"]["anydoc_args"], serde_json::json!([]));
    assert_eq!(
        layered["trans"]["markitdown_args"],
        serde_json::json!(["--fast", "pdf"])
    );

    let project = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-timeout",
            "30",
            "--trans-arg",
            "ocr",
            "--trans-arg",
            "html",
        ],
    );
    assert_eq!(project["trans"]["setting"], "anydoc");
    assert_eq!(project["trans"]["origin"], "project");
    assert_eq!(project["trans"]["timeout_seconds"], 30);
    assert_eq!(project["trans"]["anydoc_args"], serde_json::json!(["ocr", "html"]));
    assert_eq!(project["trans"]["markitdown_args"], serde_json::json!([]));
}

#[test]
fn trans_set_replaces_selected_engine_args_and_unset_restores_inherit() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-timeout",
            "240",
            "--trans-arg",
            "first",
            "--trans-arg",
            "second",
        ],
    );
    let replaced = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-arg",
            "only",
        ],
    );
    assert_eq!(replaced["trans"]["setting"], "anydoc");
    assert_eq!(replaced["trans"]["origin"], "project");
    assert_eq!(replaced["trans"]["timeout_seconds"], 240);
    assert_eq!(replaced["trans"]["anydoc_args"], serde_json::json!(["only"]));

    let switched = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-arg",
            "md",
        ],
    );
    assert_eq!(switched["trans"]["setting"], "markitdown");
    assert_eq!(switched["trans"]["origin"], "project");
    assert_eq!(switched["trans"]["anydoc_args"], serde_json::json!(["only"]));
    assert_eq!(switched["trans"]["markitdown_args"], serde_json::json!(["md"]));

    let stored: Value =
        serde_json::from_str(&fs::read_to_string(world.project.join(".lwc/config.json")).unwrap())
            .unwrap();
    assert_eq!(stored["version"], 5);
    assert_eq!(stored["trans"]["setting"], "markitdown");
    assert_eq!(stored["trans"]["timeout_seconds"], 240);
    assert_eq!(stored["trans"]["anydoc_args"], serde_json::json!(["only"]));
    assert_eq!(stored["trans"]["markitdown_args"], serde_json::json!(["md"]));

    let inherited = world.ok(&world.project, &["config", "unset", "--trans"]);
    assert_eq!(inherited["trans"]["setting"], "disabled");
    assert_eq!(inherited["trans"]["origin"], "built-in");
    assert_eq!(inherited["trans"]["timeout_seconds"], 120);
    assert_eq!(inherited["trans"]["anydoc_args"], serde_json::json!([]));
    assert_eq!(inherited["trans"]["markitdown_args"], serde_json::json!([]));

    let stored: Value =
        serde_json::from_str(&fs::read_to_string(world.project.join(".lwc/config.json")).unwrap())
            .unwrap();
    assert_eq!(stored["version"], 5);
    assert_eq!(stored["trans"]["setting"], "inherit");
    assert_eq!(stored["trans"]["timeout_seconds"], 120);
    assert_eq!(stored["trans"]["anydoc_args"], serde_json::json!([]));
    assert_eq!(stored["trans"]["markitdown_args"], serde_json::json!([]));
}

#[test]
fn v2_config_stays_compatible_and_migrates_to_v5_on_write() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let config_path = world.project.join(".lwc/config.json");
    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "graph": {
                "setting": "surrealdb"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let shown = world.ok(&world.project, &["config", "show"]);
    assert_eq!(shown["graph"]["setting"], "surrealdb");
    assert_eq!(shown["graph"]["origin"], "project");
    assert_eq!(shown["trans"]["setting"], "disabled");
    assert_eq!(shown["trans"]["origin"], "built-in");
    assert_eq!(shown["trans"]["timeout_seconds"], 120);

    let updated = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-timeout",
            "15",
            "--trans-arg",
            "ocr",
        ],
    );
    assert_eq!(updated["graph"]["setting"], "surrealdb");
    assert_eq!(updated["trans"]["setting"], "anydoc");
    assert_eq!(updated["trans"]["origin"], "project");

    let stored: Value = serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(stored["version"], 5);
    assert_eq!(stored["graph"]["setting"], "surrealdb");
    assert_eq!(stored["trans"]["setting"], "anydoc");
    assert_eq!(stored["trans"]["timeout_seconds"], 15);
    assert_eq!(stored["trans"]["anydoc_args"], serde_json::json!(["ocr"]));
    assert_eq!(stored["trans"]["markitdown_args"], serde_json::json!([]));
}

#[test]
fn trans_config_rejects_timeout_bounds_and_unknown_fields_before_write() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let config_path = world.project.join(".lwc/config.json");

    let too_low = world.err(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "anydoc",
            "--trans-timeout",
            "0",
        ],
    );
    assert_eq!(too_low["error"]["code"], "invalid_input");
    assert!(!config_path.exists());

    let too_high = world.err(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-timeout",
            "901",
        ],
    );
    assert_eq!(too_high["error"]["code"], "invalid_input");
    assert!(!config_path.exists());

    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {
                "setting": "inherit"
            },
            "trans": {
                "setting": "anydoc",
                "timeout_seconds": 120,
                "anydoc_args": [],
                "markitdown_args": [],
                "unexpected": true
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let before = fs::read_to_string(&config_path).unwrap();
    let invalid = world.err(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "disabled",
        ],
    );
    assert_eq!(invalid["error"]["code"], "invalid_config");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), before);
}

#[test]
fn trans_config_rejects_invalid_timeout_from_disk_for_show_and_update() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let config_path = world.project.join(".lwc/config.json");

    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {
                "setting": "inherit"
            },
            "trans": {
                "setting": "markitdown",
                "timeout_seconds": 0,
                "anydoc_args": [],
                "markitdown_args": ["--fast"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let before = fs::read_to_string(&config_path).unwrap();

    let show = world.err(&world.project, &["config", "show"]);
    assert_eq!(show["error"]["code"], "invalid_config");

    let update = world.err(&world.project, &["config", "set", "--graph", "disabled"]);
    assert_eq!(update["error"]["code"], "invalid_config");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), before);
}

#[test]
fn trans_config_rejects_secret_args_before_write_and_allows_endpoints() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let config_path = world.project.join(".lwc/config.json");

    let allowed = world.ok(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-arg",
            "--use-cu",
            "--trans-arg",
            "--cu-endpoint",
            "--trans-arg",
            "https://example.invalid/cu",
        ],
    );
    assert_eq!(allowed["trans"]["setting"], "markitdown");
    let stored = fs::read_to_string(&config_path).unwrap();
    assert!(stored.contains("https://example.invalid/cu"));

    fs::remove_file(&config_path).unwrap();
    let secret = "sk-proj-123456789012345678901234";
    let error = world.err(
        &world.project,
        &[
            "config",
            "set",
            "--trans",
            "markitdown",
            "--trans-arg",
            "--api-key",
            "--trans-arg",
            secret,
        ],
    );
    assert_eq!(error["error"]["code"], "possible_secret_detected");
    assert!(!config_path.exists());
}

#[test]
fn trans_config_rejects_secret_args_from_disk_for_show_and_update() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let config_path = world.project.join(".lwc/config.json");
    let secret = "sk-proj-123456789012345678901234";

    fs::write(
        &config_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 3,
            "graph": {
                "setting": "inherit"
            },
            "trans": {
                "setting": "markitdown",
                "timeout_seconds": 120,
                "anydoc_args": [],
                "markitdown_args": ["--api-key", secret]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let before = fs::read_to_string(&config_path).unwrap();

    let show = world.err(&world.project, &["config", "show"]);
    assert_eq!(show["error"]["code"], "invalid_config");

    let update = world.err(&world.project, &["config", "set", "--graph", "disabled"]);
    assert_eq!(update["error"]["code"], "invalid_config");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), before);
}

#[test]
fn external_graph_engines_build_current_documents_through_observable_work() {
    for engine in ["grafeo", "surrealdb"] {
        let world = TestWorld::new();
        world.ok(&world.project, &["init"]);
        put_page(
            &world,
            "engine-a",
            "Engine A",
            "Current document A links to [[engine-b]].",
        );
        put_page(&world, "engine-b", "Engine B", "Current document B.");
        let configured = world.ok(&world.project, &["config", "set", "--graph", engine]);
        assert_eq!(configured["graph"]["setting"], engine);
        let work_id = configured["work"]["id"]
            .as_str()
            .expect("enabling an external graph engine must return observable Work");
        let completed = world.ok(&world.project, &["work", "watch", work_id]);
        assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
        assert_eq!(completed["work"]["completed"], 2, "{completed}");
        assert_eq!(completed["work"]["total"], 2, "{completed}");

        let status = world.ok(&world.project, &["graph", "status"]);
        assert_eq!(status["engine"], engine, "{status}");
        assert_eq!(status["status"], "ready", "{status}");
        assert_eq!(status["documents"], 2, "{status}");

        let update = world.write(
            &format!("{engine}-update.md"),
            &format!("Updated current document A through {engine}."),
        );
        let updated = world.ok(
            &world.project,
            &[
                "page",
                "put",
                "engine-a",
                "--title",
                "Engine A",
                "--file",
                as_str(&update),
                "--provenance",
                "agent-observed",
            ],
        );
        let work_id = updated["graph"]["work"]["id"]
            .as_str()
            .expect("one changed document must enqueue observable graph Work");
        let completed = world.ok(&world.project, &["work", "watch", work_id]);
        assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
        assert_eq!(completed["work"]["completed"], 1, "{completed}");
        assert_eq!(completed["work"]["total"], 1, "{completed}");
        assert_eq!(
            world.ok(&world.project, &["graph", "node", "page:engine-a"])["node"]["label"],
            "Engine A"
        );

        let removed = world.ok(&world.project, &["page", "remove", "engine-b"]);
        let work_id = removed["graph_work"]["id"].as_str().unwrap();
        let completed = world.ok(&world.project, &["work", "watch", work_id]);
        assert_eq!(completed["work"]["completed"], 1, "{completed}");
        assert_eq!(completed["work"]["total"], 1, "{completed}");
        assert_eq!(
            world.err(&world.project, &["graph", "node", "page:engine-b"])["error"]["code"],
            "graph_node_not_found"
        );
    }
}

#[test]
fn external_graph_deletes_removed_pages_and_freezes_superseded_sources() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source_path = world.write("tracked.md", "first immutable version");
    let first = world.ok(&world.project, &["source", "add", as_str(&source_path)]);
    let first_id = first["source"]["id"].as_i64().unwrap().to_string();
    put_page(&world, "temporary", "Temporary", "Temporary current page.");

    let enabled = world.ok(&world.project, &["config", "set", "--graph", "grafeo"]);
    let work_id = enabled["work"]["id"].as_str().unwrap();
    assert_eq!(
        world.ok(&world.project, &["work", "watch", work_id])["work"]["state"],
        "succeeded"
    );

    fs::write(&source_path, "second immutable version").unwrap();
    let second = world.ok(&world.project, &["source", "add", as_str(&source_path)]);
    let second_id = second["source"]["id"].as_i64().unwrap().to_string();
    let work_id = second["graph"]["work"]["id"].as_str().unwrap();
    assert_eq!(
        world.ok(&world.project, &["work", "watch", work_id])["work"]["state"],
        "succeeded"
    );
    assert_eq!(
        world.ok(&world.project, &["source", "show", &first_id])["source"]["content"],
        "first immutable version"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["graph", "node", &format!("source:{first_id}")]
        )["error"]["code"],
        "graph_node_not_found"
    );
    assert_eq!(
        world.ok(
            &world.project,
            &["graph", "node", &format!("source:{second_id}")]
        )["node"]["identifier"],
        format!("source:{second_id}")
    );

    world.ok(&world.project, &["page", "remove", "temporary"]);
    let works = world.ok(&world.project, &["work", "list"]);
    let work_id = works["works"][0]["id"].as_str().unwrap();
    assert_eq!(
        world.ok(&world.project, &["work", "watch", work_id])["work"]["state"],
        "succeeded"
    );
    assert_eq!(
        world.err(&world.project, &["graph", "node", "page:temporary"])["error"]["code"],
        "graph_node_not_found"
    );
}

#[test]
fn graph_work_resumes_only_uncommitted_documents_after_a_document_failure() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    for slug in ["resume-a", "resume-b", "resume-c"] {
        put_page(&world, slug, slug, "document-granular projection");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .env("LWC_TEST_GRAPH_FAIL_AFTER_DOCUMENTS", "1")
        .args(["config", "set", "--graph", "grafeo"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configured: Value = serde_json::from_slice(&output.stdout).unwrap();
    let first_id = configured["work"]["id"].as_str().unwrap();
    let first = world.ok(&world.project, &["work", "watch", first_id]);
    assert_eq!(first["work"]["state"], "failed", "{first}");
    assert_eq!(first["work"]["completed"], 1, "{first}");
    assert_eq!(first["work"]["total"], 3, "{first}");

    thread::sleep(Duration::from_millis(100));
    let works = world.ok(&world.project, &["work", "list"]);
    assert_eq!(
        works["works"].as_array().unwrap().len(),
        1,
        "failed graph Work must stop until explicitly resumed: {works}"
    );

    world.ok(&world.project, &["work", "resume", first_id]);
    let completed = world.ok(&world.project, &["work", "watch", first_id]);
    assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
    assert_eq!(completed["work"]["completed"], 2, "{completed}");
    assert_eq!(completed["work"]["total"], 2, "{completed}");
    assert_eq!(
        world.ok(&world.project, &["graph", "status"])["documents"],
        3
    );
}

#[test]
fn graph_work_cancel_then_resume_reaches_a_verified_terminal_state() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    for index in 0..24 {
        let slug = format!("cancel-resume-{index:02}");
        put_page(
            &world,
            &slug,
            &slug,
            "small document for deterministic cancellation coverage",
        );
    }

    let configured = world.ok(&world.project, &["config", "set", "--graph", "grafeo"]);
    let work_id = configured["work"]["id"].as_str().unwrap();
    let cancellation = world.ok(&world.project, &["work", "cancel", work_id]);
    assert!(cancellation["work"]["cancel_requested"].as_bool().unwrap());

    let cancelled = world.ok(&world.project, &["work", "watch", work_id]);
    assert_eq!(cancelled["work"]["state"], "cancelled", "{cancelled}");
    assert_eq!(
        cancelled["work"]["error"]["code"], "work_cancelled",
        "{cancelled}"
    );
    if let Some(total) = cancelled["work"]["total"].as_u64() {
        assert!(
            cancelled["work"]["completed"].as_u64().unwrap() < total,
            "{cancelled}"
        );
    }

    let resumed = world.ok(&world.project, &["work", "resume", work_id]);
    assert_eq!(resumed["work"]["state"], "queued", "{resumed}");
    assert!(!resumed["work"]["cancel_requested"].as_bool().unwrap());

    let completed = world.ok(&world.project, &["work", "watch", work_id]);
    assert_eq!(completed["work"]["state"], "succeeded", "{completed}");
    assert_eq!(world.ok(&world.project, &["graph", "verify"])["ok"], true);
}
