#[cfg(unix)]
#[test]
fn changeset_locked_recheck_preserves_a_live_writer_that_finishes_during_commit() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    stage_clean_linked_pages(&world, "live-race", "live-race");
    let live = world.project.join(".lwc/wiki.db");
    let writer = Connection::open(&live).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["changeset", "commit", "live-race"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let checkpoint_dir = world.project.join(".lwc/checkpoints");
    let deadline = Instant::now() + Duration::from_secs(5);
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
             VALUES ('test_live_race', 'live', '{}');
             UPDATE meta SET value = LOWER(HEX(RANDOMBLOB(32)))
             WHERE key = 'store_revision';
             COMMIT;",
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let race_count: i64 = Connection::open(&live)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM operations WHERE action = 'test_live_race'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(race_count, 1);
    assert_eq!(
        world.ok(&world.project, &["page", "show", "live-race-first"])["page"]["title"],
        "First"
    );
}

#[test]
fn changeset_rejects_all_scope_and_unsafe_names_before_creating_files() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let all = world.err(
        &world.project,
        &["--scope", "all", "changeset", "begin", "bad"],
    );
    assert_eq!(all["error"]["code"], "scope_not_supported");

    for name in [
        "",
        " ",
        "../escape",
        ".",
        "..",
        "two/segments",
        "two\\segments",
        " leading",
        "trailing ",
        "line\nbreak",
    ] {
        let invalid = world.err(&world.project, &["changeset", "begin", name]);
        assert_eq!(invalid["error"]["code"], "changeset_name_invalid");
    }
    let oversized = "x".repeat(81);
    let invalid = world.err(&world.project, &["changeset", "begin", oversized.as_str()]);
    assert_eq!(invalid["error"]["code"], "changeset_name_invalid");
    assert!(!world.project.join(".lwc/changesets").exists());
}

#[test]
fn changeset_rejects_unsupported_command_families_without_live_mutation() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "guarded"]);
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

    for args in [
        vec!["--changeset", "guarded", "init"],
        vec!["--changeset", "guarded", "maintenance", "materialize"],
        vec!["--changeset", "guarded", "checkpoint", "list"],
        vec!["--changeset", "guarded", "changeset", "list"],
    ] {
        let error = world.err(&world.project, &args);
        assert_eq!(error["error"]["code"], "changeset_command_unsupported");
    }
    let all = world.err(
        &world.project,
        &["--scope", "all", "--changeset", "guarded", "context"],
    );
    assert_eq!(all["error"]["code"], "scope_not_supported");

    let after: (String, i64) = Connection::open(database)
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
}

#[test]
fn changeset_read_only_preview_stays_empty_but_recorded_reads_are_staged() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "preview"]);

    for args in [
        vec!["--changeset", "preview", "context"],
        vec!["--changeset", "preview", "lint"],
        vec!["--changeset", "preview", "log"],
        vec!["--changeset", "preview", "search", "not-present"],
    ] {
        world.ok(&world.project, &args);
    }
    assert_eq!(
        world.ok(&world.project, &["changeset", "show", "preview"])["empty"],
        true
    );

    world.ok(
        &world.project,
        &[
            "--changeset",
            "preview",
            "search",
            "durable question",
            "--record",
        ],
    );
    let shown = world.ok(&world.project, &["changeset", "show", "preview"]);
    assert_eq!(shown["empty"], false);
    assert_eq!(shown["action_counts"]["search"], 1);
}

#[test]
fn changeset_stages_a_complete_ingest_then_discards_every_candidate_row() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "complete-ingest"]);
    let source = world.write("evidence.md", "atomic candidate evidence");
    let added = world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "source",
            "add",
            as_str(&source),
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "claim",
            &source_id,
        ],
    );
    let analysis = world.write("candidate-analysis.md", "candidate analysis");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "analyze",
            &source_id,
            "--file",
            as_str(&analysis),
        ],
    );
    let source_page = world.write(
        "candidate-source.md",
        "candidate source summary. [[candidate-synthesis]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "page",
            "put",
            "source-candidate",
            "--title",
            "Candidate Source",
            "--kind",
            "source",
            "--summary",
            "Immutable candidate evidence",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );
    let synthesis = world.write(
        "candidate-synthesis.md",
        "Atomic candidate synthesis. [[source-candidate]]",
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "page",
            "put",
            "candidate-synthesis",
            "--title",
            "Candidate Synthesis",
            "--kind",
            "synthesis",
            "--summary",
            "Atomic candidate conclusion",
            "--file",
            as_str(&synthesis),
            "--source",
            &source_id,
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "ingest",
            "complete",
            &source_id,
        ],
    );
    let search = world.ok(
        &world.project,
        &[
            "--changeset",
            "complete-ingest",
            "search",
            "atomic candidate",
        ],
    );
    assert!(find_result(&search, "candidate-synthesis")["rank"].is_number());
    assert_eq!(
        world.ok(&world.project, &["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        world.ok(&world.project, &["--changeset", "complete-ingest", "lint"])["total"],
        0
    );
    world.ok(&world.project, &["changeset", "discard", "complete-ingest"]);
    assert_eq!(
        world.ok(&world.project, &["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn changeset_commits_and_rolls_back_only_its_new_source_ingest_and_pages() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "source-patch"]);
    let source = world.write("source-patch.md", "bounded source patch evidence");
    let added = world.ok(
        &world.project,
        &[
            "--changeset",
            "source-patch",
            "source",
            "add",
            as_str(&source),
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    world.ok(
        &world.project,
        &["--changeset", "source-patch", "ingest", "claim", &source_id],
    );
    let analysis = world.write("source-patch-analysis.md", "bounded analysis");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "source-patch",
            "ingest",
            "analyze",
            &source_id,
            "--file",
            as_str(&analysis),
        ],
    );
    for (slug, title, kind, body) in [
        (
            "source-patch-summary",
            "Source Patch Summary",
            "source",
            "source summary [[source-patch-synthesis]]",
        ),
        (
            "source-patch-synthesis",
            "Source Patch Synthesis",
            "synthesis",
            "derived knowledge [[source-patch-summary]]",
        ),
    ] {
        let file = world.write(&format!("{slug}.md"), body);
        world.ok(
            &world.project,
            &[
                "--changeset",
                "source-patch",
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
            ],
        );
    }
    world.ok(
        &world.project,
        &[
            "--changeset",
            "source-patch",
            "ingest",
            "complete",
            &source_id,
        ],
    );
    let committed = world.ok(&world.project, &["changeset", "commit", "source-patch"]);
    assert_eq!(
        world.ok(&world.project, &["ingest", "list", "--status", "completed"])["jobs"][0]["source_id"],
        source_id.parse::<i64>().unwrap()
    );
    put_page(
        &world,
        "source-unrelated",
        "Unrelated",
        "survives source rollback",
    );
    world.ok(
        &world.project,
        &[
            "changeset",
            "rollback",
            committed["changeset_id"].as_str().unwrap(),
        ],
    );
    assert!(
        world.ok(&world.project, &["source", "list"])["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        world.ok(&world.project, &["page", "show", "source-unrelated"])["page"]["body"],
        "survives source rollback"
    );
}

#[test]
fn changeset_routes_schema_purpose_graph_weight_context_search_lint_and_log() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let live_schema = world.ok(&world.project, &["schema", "show"])["schema"]
        .as_str()
        .unwrap()
        .to_string();
    world.ok(&world.project, &["changeset", "begin", "command-matrix"]);
    let schema = world.write("matrix-schema.md", "# Draft Schema\n\nMatrix only.");
    let purpose = world.write("matrix-purpose.md", "# Draft Purpose\n\nMatrix only.");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "schema",
            "set",
            as_str(&schema),
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "purpose",
            "set",
            as_str(&purpose),
        ],
    );
    let first = world.write("matrix-first.md", "matrix token [[matrix-second]]");
    let second = world.write("matrix-second.md", "matrix token [[matrix-first]]");
    for (slug, title, file) in [
        ("matrix-first", "Matrix First", &first),
        ("matrix-second", "Matrix Second", &second),
    ] {
        world.ok(
            &world.project,
            &[
                "--changeset",
                "command-matrix",
                "page",
                "put",
                slug,
                "--title",
                title,
                "--summary",
                title,
                "--file",
                as_str(file),
                "--provenance",
                "agent-observed",
            ],
        );
    }
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "weight",
            "set",
            "page",
            "matrix-first",
            "--value",
            "2",
            "--reason",
            "draft canonical page",
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "weight",
            "feedback",
            "page",
            "matrix-first",
            "--query",
            "matrix token",
            "--signal",
            "relevant",
            "--reason",
            "draft result verified",
            "--provenance",
            "agent-observed",
        ],
    );
    let related = world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "graph",
            "related",
            "matrix-first",
        ],
    );
    assert_eq!(related["related"][0]["slug"], "matrix-second");
    let context = world.ok(
        &world.project,
        &["--changeset", "command-matrix", "context"],
    );
    assert!(
        context["stores"][0]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| page["slug"] == "matrix-first")
    );
    world.ok(
        &world.project,
        &[
            "--changeset",
            "command-matrix",
            "search",
            "matrix token",
            "--record",
        ],
    );
    assert_eq!(
        world.ok(&world.project, &["--changeset", "command-matrix", "lint"])["total"],
        0
    );
    assert!(
        world.ok(
            &world.project,
            &["--changeset", "command-matrix", "log", "--limit", "20"]
        )["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["action"] == "weight_set")
    );
    assert_eq!(
        world.ok(&world.project, &["schema", "show"])["schema"],
        live_schema
    );
    let unsupported = world.err(
        &world.project,
        &[
            "changeset",
            "commit",
            "command-matrix",
            "--allow-lint-issues",
            "--reason",
            "sparse semantic mutation guard",
        ],
    );
    assert_eq!(unsupported["error"]["code"], "changeset_sparse_unsupported");
    world.ok(&world.project, &["changeset", "discard", "command-matrix"]);
}

#[test]
fn same_changeset_name_is_isolated_between_project_and_global_scopes() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);
    world.ok(&world.project, &["changeset", "begin", "same-name"]);
    world.ok(
        &world.project,
        &["--scope", "global", "changeset", "begin", "same-name"],
    );
    let project_body = world.write("project-scope.md", "project candidate");
    let global_body = world.write("global-scope.md", "global candidate");
    world.ok(
        &world.project,
        &[
            "--changeset",
            "same-name",
            "page",
            "put",
            "scope-page",
            "--title",
            "Project Candidate",
            "--file",
            as_str(&project_body),
            "--provenance",
            "agent-observed",
        ],
    );
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "--changeset",
            "same-name",
            "page",
            "put",
            "scope-page",
            "--title",
            "Global Candidate",
            "--file",
            as_str(&global_body),
            "--provenance",
            "agent-observed",
        ],
    );
    assert_eq!(
        world.ok(
            &world.project,
            &["--changeset", "same-name", "page", "show", "scope-page"]
        )["page"]["title"],
        "Project Candidate"
    );
    assert_eq!(
        world.ok(
            &world.project,
            &[
                "--scope",
                "global",
                "--changeset",
                "same-name",
                "page",
                "show",
                "scope-page",
            ]
        )["page"]["title"],
        "Global Candidate"
    );
    world.ok(&world.project, &["changeset", "discard", "same-name"]);
    assert_eq!(
        world.ok(
            &world.project,
            &["--scope", "global", "changeset", "show", "same-name",]
        )["status"],
        "draft"
    );
}

#[cfg(unix)]
#[test]
fn changeset_keeps_live_source_authority_and_rejects_symlinked_draft_storage() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["changeset", "begin", "bounded"]);
    let outside_source = world.project.parent().unwrap().join("outside.md");
    fs::write(&outside_source, "outside evidence").unwrap();
    let error = world.err(
        &world.project,
        &[
            "--changeset",
            "bounded",
            "source",
            "add",
            as_str(&outside_source),
        ],
    );
    assert_eq!(
        error["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let protected = world.project.parent().unwrap().join("protected-wal");
    fs::write(&protected, "must survive").unwrap();
    let wal = world.project.join(".lwc/changesets/bounded.db-wal");
    if wal.exists() {
        fs::remove_file(&wal).unwrap();
    }
    symlink(&protected, &wal).unwrap();
    let invalid = world.err(&world.project, &["changeset", "discard", "bounded"]);
    assert_eq!(invalid["error"]["code"], "changeset_path_invalid");
    assert_eq!(fs::read_to_string(&protected).unwrap(), "must survive");
    fs::remove_file(wal).unwrap();
    world.ok(&world.project, &["changeset", "discard", "bounded"]);

    fs::remove_dir(world.project.join(".lwc/changesets")).unwrap();
    let outside_directory = world.project.parent().unwrap().join("outside-drafts");
    fs::create_dir(&outside_directory).unwrap();
    symlink(&outside_directory, world.project.join(".lwc/changesets")).unwrap();
    let invalid = world.err(&world.project, &["changeset", "begin", "escape"]);
    assert_eq!(invalid["error"]["code"], "changeset_path_invalid");
    assert!(fs::read_dir(outside_directory).unwrap().next().is_none());
}

fn find_result<'a>(search: &'a Value, identifier: &str) -> &'a Value {
    search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["identifier"] == identifier)
        .unwrap_or_else(|| panic!("missing result {identifier}: {search}"))
}

#[test]
fn help_documents_the_agent_workflow_and_command_side_effects() {
    let world = TestWorld::new();

    for (args, expected) in [
        (
            vec!["--help"],
            vec![
                "Agent operating contract:",
                "Persistent workflow:",
                "~/.lwc/wiki.db",
                "LWC_PROJECT_ROOT",
                "JSON",
                "Do not edit .lwc/wiki.db",
                "Atomic multi-command changes:",
                "lwc changeset begin",
            ],
        ),
        (
            vec!["source", "--help"],
            vec!["When to use:", "Next action:", "pending ingest job"],
        ),
        (
            vec!["ingest", "--help"],
            vec![
                "pending -> analyzing -> generating -> completed",
                "Required Agent loop:",
                "kind=source",
            ],
        ),
        (
            vec!["page", "--help"],
            vec!["When to use:", "Decision rule:", "kind=query"],
        ),
        (
            vec!["source", "add-dir", "--help"],
            vec!["UTF-8", "partial_import", "idempotent"],
        ),
        (
            vec!["source", "add-manifest", "--help"],
            vec!["JSON", "relative", "--acknowledge-sensitive-source"],
        ),
        (
            vec!["source", "status", "--help"],
            vec!["Read-only", "streaming SHA-256", "--allow-external-source"],
        ),
        (
            vec!["source", "diff", "--help"],
            vec![
                "Read-only",
                "--to-source",
                "--max-chars",
                "--acknowledge-sensitive-source",
            ],
        ),
        (
            vec!["page", "put", "--help"],
            vec![
                "kind=source",
                "[[slug]]",
                "--source <SOURCE_IDS>",
                "--provenance <PROVENANCE>",
            ],
        ),
        (
            vec!["ingest", "complete", "--help"],
            vec![
                "kind=source summary",
                "non-source Wiki page",
                "--no-derived-pages-reason",
                "completed",
            ],
        ),
        (
            vec!["search", "--help"],
            vec![
                "--type auto",
                "--type source",
                "--kind",
                "does not persist the query",
                "--scope all",
                "--record",
                "each selected store",
            ],
        ),
        (
            vec!["maintenance", "materialize", "--help"],
            vec!["SQLite is authoritative", ".lwc/wiki"],
        ),
        (
            vec!["maintenance", "compact", "--help"],
            vec!["WAL TRUNCATE", "busy=true", "active reader"],
        ),
        (
            vec!["source", "show", "--help"],
            vec![
                "--offset-chars",
                "--max-chars",
                "window.has_more",
                "window.next_offset_chars",
            ],
        ),
        (
            vec!["checkpoint", "restore", "--help"],
            vec!["Restore", "checkpoint"],
        ),
        (
            vec!["lint", "--help"],
            vec!["--record", "Read-only by default"],
        ),
        (
            vec!["graph", "--help"],
            vec!["Grafeo", "SurrealDB", "one document before the next"],
        ),
        (
            vec!["changeset", "show", "--help"],
            vec!["without running lint"],
        ),
        (
            vec!["config", "set", "--help"],
            vec!["disabled, grafeo, surrealdb, or inherit"],
        ),
    ] {
        let output = world.command(&world.project, &args);
        assert!(
            output.status.success(),
            "help command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        for text in expected {
            assert!(
                help.contains(text),
                "help command {args:?} should contain {text:?}\n{help}"
            );
        }
    }
}

#[test]
fn explain_reports_exact_bounded_lexical_and_specificity_arithmetic() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "payment-overview-readme",
        "Payment Overview README",
        "payment reconciliation matching rules evidence",
    );
    put_page(
        &world,
        "payment-reconciliation-matching-rules",
        "Payment Reconciliation Matching Rules",
        "payment reconciliation matching rules evidence",
    );
    put_page(
        &world,
        "payment-catalog",
        "Payment Configuration",
        "payment reconciliation matching rules\n\n## 使用导航\n总览文档只保留模块边界",
    );
    put_page(
        &world,
        "payment-procedure",
        "Payment Procedure",
        "payment reconciliation matching rules\n\n## 使用导航\n按步骤完成具体业务操作",
    );

    let default_search = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation matching rules",
            "--type",
            "page",
        ],
    );
    assert!(default_search["results"][0].get("explanation").is_none());
    assert_eq!(
        default_search["results"][0]["identifier"],
        "payment-reconciliation-matching-rules"
    );

    let explained = world.ok(
        &world.project,
        &[
            "search",
            "payment reconciliation matching rules",
            "--type",
            "page",
            "--explain",
        ],
    );
    let direct = find_result(&explained, "payment-reconciliation-matching-rules");
    let overview = find_result(&explained, "payment-overview-readme");
    let catalog = find_result(&explained, "payment-catalog");
    let procedure = find_result(&explained, "payment-procedure");
    assert!(
        direct["explanation"]["signals"]["title_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert!(
        direct["explanation"]["signals"]["path_match"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(overview["explanation"]["signals"]["generic_marker"], 1.0);
    assert_eq!(catalog["explanation"]["signals"]["generic_marker"], 1.0);
    assert_eq!(procedure["explanation"]["signals"]["generic_marker"], 0.0);
    for result in explained["results"].as_array().unwrap() {
        let explanation = &result["explanation"];
        let reconstructed = explanation["base_rank"].as_f64().unwrap()
            + explanation["contributions"]
                .as_object()
                .unwrap()
                .values()
                .map(|value| value.as_f64().unwrap())
                .sum::<f64>();
        let final_rank = explanation["final_rank"].as_f64().unwrap();
        assert!((reconstructed - final_rank).abs() < 1e-9, "{explanation}");
        assert!((result["rank"].as_f64().unwrap() - final_rank).abs() < 1e-9);
    }

    let explicit_overview = world.ok(
        &world.project,
        &[
            "search",
            "payment overview README",
            "--type",
            "page",
            "--explain",
        ],
    );
    let overview = find_result(&explicit_overview, "payment-overview-readme");
    assert_eq!(overview["explanation"]["signals"]["generic_marker"], 0.0);
}
