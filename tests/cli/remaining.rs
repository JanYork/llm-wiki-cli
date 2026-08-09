#[test]
fn source_windows_keep_large_utf8_documents_bounded_and_resumable() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/large.md", "甲乙丙丁戊己庚辛壬癸");
    world.ok(&world.project, &["source", "add", as_str(&source)]);

    let shown = world.ok(
        &world.project,
        &[
            "source",
            "show",
            "1",
            "--offset-chars",
            "3",
            "--max-chars",
            "4",
        ],
    );
    assert_eq!(shown["source"]["content"], "丁戊己庚");
    assert_eq!(shown["window"]["offset_chars"], 3);
    assert_eq!(shown["window"]["returned_chars"], 4);
    assert_eq!(shown["window"]["total_chars"], 10);
    assert_eq!(shown["window"]["next_offset_chars"], 7);
    assert_eq!(shown["window"]["has_more"], true);

    let packet = world.ok(
        &world.project,
        &["ingest", "next", "--source-max-chars", "4"],
    );
    assert_eq!(packet["job"]["source"]["content"], "甲乙丙丁");
    assert_eq!(packet["job"]["source_window"]["next_offset_chars"], 4);
    assert_eq!(packet["job"]["source_window"]["has_more"], true);
}

#[test]
fn search_directly_returns_sentence_and_passage_locators() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    put_page(
        &world,
        "span-page",
        "Span page",
        "First context. UniqueNeedle appears here. Final context.",
    );

    let sentence = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "sentence",
        ],
    );
    assert_eq!(sentence["results"][0]["type"], "sentence");
    assert_eq!(sentence["results"][0]["document"]["type"], "page");
    assert_eq!(
        sentence["results"][0]["document"]["identifier"],
        "span-page"
    );
    assert_eq!(
        sentence["results"][0]["snippet"],
        "UniqueNeedle appears here."
    );
    assert_eq!(sentence["results"][0]["span"]["segmenter_version"], 1);
    assert!(
        sentence["results"][0]["span"]["byte_end"].as_u64().unwrap()
            > sentence["results"][0]["span"]["byte_start"]
                .as_u64()
                .unwrap()
    );

    let passage = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "passage",
        ],
    );
    assert_eq!(passage["results"][0]["type"], "passage");
    assert_eq!(
        passage["results"][0]["snippet"],
        "First context. UniqueNeedle appears here. Final context."
    );

    let grouped = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "all",
            "--group-by",
            "document",
        ],
    );
    assert_eq!(grouped["results"].as_array().unwrap().len(), 1);
    assert_eq!(grouped["results"][0]["type"], "page");
    assert_eq!(grouped["results"][0]["identifier"], "span-page");
    assert_eq!(
        grouped["results"][0]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|result| result["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["sentence", "passage", "page"]
    );
    let matches = grouped["results"][0]["matches"].as_array().unwrap();
    for (actual, expected) in [
        (matches[0]["fused_score"].as_f64().unwrap(), 1.15 / 61.0),
        (matches[1]["fused_score"].as_f64().unwrap(), 1.05 / 61.0),
        (matches[2]["fused_score"].as_f64().unwrap(), 1.0 / 61.0),
    ] {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    world.ok(
        &world.project,
        &[
            "weight",
            "set",
            "page",
            "span-page",
            "--value",
            "2",
            "--reason",
            "canonical span owner",
            "--provenance",
            "agent-observed",
        ],
    );
    let adjusted = world.ok(
        &world.project,
        &[
            "search",
            "UniqueNeedle",
            "--type",
            "page",
            "--granularity",
            "sentence",
            "--explain",
        ],
    );
    assert_eq!(
        adjusted["results"][0]["explanation"]["signals"]["manual_adjustment"],
        1.0
    );
    assert_eq!(
        adjusted["results"][0]["explanation"]["contributions"]["manual"],
        -2.0
    );

    let sentence_id = sentence["results"][0]["identifier"].as_str().unwrap();
    let shown = world.ok(&world.project, &["span", "get", sentence_id]);
    assert_eq!(shown["span"]["text"], "UniqueNeedle appears here.");
    assert_eq!(shown["span"]["document"]["identifier"], "span-page");
    let expanded = world.ok(
        &world.project,
        &[
            "span",
            "expand",
            sentence_id,
            "--before",
            "1",
            "--after",
            "1",
        ],
    );
    assert_eq!(
        expanded["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| span["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "First context.",
            "UniqueNeedle appears here.",
            "Final context."
        ]
    );
    assert_eq!(
        expanded["parent"]["text"],
        "First context. UniqueNeedle appears here. Final context."
    );

    world.ok(
        &world.project,
        &[
            "graph",
            "relation",
            "set",
            sentence_id,
            "REFINES",
            "page:span-page",
            "--provenance",
            "agent-observed",
            "--reason",
            "span-scoped interpretation",
            "--confidence",
            "0.7",
        ],
    );
    let replacement = world.write(
        "span-replacement.md",
        "Replacement content has a different fingerprint.",
    );
    let replaced = world.ok(
        &world.project,
        &[
            "page",
            "put",
            "span-page",
            "--title",
            "Span page",
            "--file",
            as_str(&replacement),
        ],
    );
    assert_eq!(replaced["graph"]["invalidated_semantic_relations"], 1);
    let stale = world.err(&world.project, &["span", "get", sentence_id]);
    assert_eq!(stale["error"]["code"], "stale_span", "{stale}");
    assert_eq!(stale["error"]["details"]["prior"]["segmenter_version"], 1);
    assert_eq!(stale["error"]["details"]["current"]["segmenter_version"], 1);
    assert_ne!(
        stale["error"]["details"]["prior"]["content_fingerprint"],
        stale["error"]["details"]["current"]["content_fingerprint"]
    );
}

#[test]
fn search_supports_type_and_kind_filters_and_exposes_page_kind() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/evidence.md", "sharedterm raw evidence");
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Evidence source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let query_page = world.write("query.md", "sharedterm durable answer");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "durable-answer",
            "--title",
            "Durable answer",
            "--kind",
            "query",
            "--file",
            as_str(&query_page),
            "--source",
            &source_id,
        ],
    );

    let source_page = world.write("source-page.md", "sharedterm curated source summary");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "source-summary",
            "--title",
            "Source summary",
            "--kind",
            "source",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );

    let filtered = world.command(
        &world.project,
        &[
            "search",
            "sharedterm",
            "--type",
            "page",
            "--kind",
            "query",
            "--kind",
            "source",
        ],
    );
    assert!(
        filtered.status.success(),
        "page/kind filtered search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&filtered.stdout),
        String::from_utf8_lossy(&filtered.stderr)
    );
    let filtered = stdout_json(&filtered);
    let results = filtered["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "filtered page search should return page results"
    );
    assert!(results.iter().all(|result| result["type"] == "page"));
    assert!(
        results
            .iter()
            .all(|result| { matches!(result["kind"].as_str(), Some("query" | "source")) })
    );
    assert!(results.iter().any(|result| result["kind"] == "query"));
    assert!(results.iter().any(|result| result["kind"] == "source"));

    let all_filtered = world.ok(
        &world.project,
        &["search", "sharedterm", "--type", "all", "--kind", "query"],
    );
    let all_filtered_results = all_filtered["results"].as_array().unwrap();
    assert!(
        all_filtered_results
            .iter()
            .any(|result| result["type"] == "source"),
        "--kind should restrict page kinds without removing raw sources from --type all: {all_filtered}"
    );
    assert!(
        all_filtered_results.iter().all(|result| {
            result["type"] == "source" || (result["type"] == "page" && result["kind"] == "query")
        }),
        "--type all --kind query should keep sources plus query pages only: {all_filtered}"
    );

    let invalid = world.command(
        &world.project,
        &[
            "search",
            "sharedterm",
            "--type",
            "source",
            "--kind",
            "source",
        ],
    );
    assert!(
        !invalid.status.success(),
        "source search with kind filter should fail"
    );
    let invalid = stderr_json(&invalid);
    assert_eq!(invalid["error"]["code"], "invalid_input");
    assert!(
        invalid["error"]["message"]
            .as_str()
            .unwrap()
            .contains("kind"),
        "source search kind error should mention kind: {invalid}"
    );
}

#[test]
fn search_auto_hides_raw_sources_behind_matching_source_pages_and_all_keeps_both() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/evidence.md", "sharedterm raw evidence");
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&source),
            "--title",
            "Evidence source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let source_page = world.write("source-page.md", "sharedterm curated source summary");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "source-summary",
            "--title",
            "Source summary",
            "--kind",
            "source",
            "--file",
            as_str(&source_page),
            "--source",
            &source_id,
        ],
    );

    let query_page = world.write("query-page.md", "sharedterm durable answer");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "durable-answer",
            "--title",
            "Durable answer",
            "--kind",
            "query",
            "--file",
            as_str(&query_page),
            "--source",
            &source_id,
        ],
    );

    let auto = world.command(&world.project, &["search", "sharedterm", "--type", "auto"]);
    assert!(
        auto.status.success(),
        "auto search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&auto.stdout),
        String::from_utf8_lossy(&auto.stderr)
    );
    let auto = stdout_json(&auto);
    let auto_results = auto["results"].as_array().unwrap();
    assert!(
        !auto_results.is_empty(),
        "auto search should return page results"
    );
    assert!(
        auto_results.iter().all(|result| result["type"] == "page"),
        "auto search should hide raw sources when a matching kind=source page cites the source: {auto}"
    );

    let implicit_auto = world.ok(&world.project, &["search", "sharedterm"]);
    assert_eq!(
        implicit_auto["results"], auto["results"],
        "plain search should be identical to explicit --type auto"
    );

    let all = world.command(&world.project, &["search", "sharedterm", "--type", "all"]);
    assert!(
        all.status.success(),
        "all search should succeed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&all.stdout),
        String::from_utf8_lossy(&all.stderr)
    );
    let all = stdout_json(&all);
    let all_results = all["results"].as_array().unwrap();
    assert!(
        all_results.iter().any(|result| result["type"] == "page"),
        "all search should keep page results"
    );
    assert!(
        all_results.iter().any(|result| result["type"] == "source"),
        "all search should retain raw source results"
    );
    let first_source = all_results
        .iter()
        .position(|result| result["type"] == "source")
        .expect("all search should include a raw source result");
    assert!(
        all_results[..first_source]
            .iter()
            .all(|result| result["type"] == "page"),
        "all page results should be grouped before raw sources: {all}"
    );
}

#[test]
fn ingest_complete_requires_a_non_source_derived_page() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let complete = world.command(&world.project, &["ingest", "complete", &source_id]);
    assert!(
        !complete.status.success(),
        "ingest complete should refuse source-only integration"
    );
    let complete = stderr_json(&complete);
    assert_eq!(complete["error"]["code"], "ingest_integration_required");
}

#[test]
fn ingest_complete_accepts_no_derived_pages_reason_override() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let complete = world.command(
        &world.project,
        &[
            "ingest",
            "complete",
            &source_id,
            "--no-derived-pages-reason",
            "single-source archive import",
        ],
    );
    assert!(
        complete.status.success(),
        "ingest complete should accept an explicit no-derived-pages reason\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&complete.stdout),
        String::from_utf8_lossy(&complete.stderr)
    );
    let complete = stdout_json(&complete);
    assert_eq!(complete["job"]["status"], "completed");
}

#[test]
fn ingest_complete_rejects_no_derived_reason_when_a_derived_page_exists() {
    let world = TestWorld::new();
    let source_id = prepare_generating_source_with_summary_only(&world, "sharedterm source body");

    let derived = world.write("concept.md", "sharedterm integrated concept");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "shared-concept",
            "--title",
            "Shared concept",
            "--kind",
            "concept",
            "--file",
            as_str(&derived),
            "--source",
            &source_id,
        ],
    );

    let complete = world.command(
        &world.project,
        &[
            "ingest",
            "complete",
            &source_id,
            "--no-derived-pages-reason",
            "should not be accepted",
        ],
    );
    assert!(
        !complete.status.success(),
        "an exception reason should be rejected when a derived page exists"
    );
    assert_eq!(stderr_json(&complete)["error"]["code"], "invalid_input");
}
