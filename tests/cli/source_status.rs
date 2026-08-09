#[test]
fn source_diff_requires_an_exact_path_when_a_source_has_multiple_paths() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let primary = world.write("raw/shared.md", "shared bytes\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&primary)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let alias = world.write("alias/shared.md", "shared bytes\n");
    world.ok(&world.project, &["source", "add", as_str(&alias)]);

    let ambiguous = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(ambiguous["error"]["code"], "source_diff_path_required");
    let message = ambiguous["error"]["message"].as_str().unwrap();
    assert!(message.contains("alias/shared.md, raw/shared.md"));

    fs::write(&alias, "changed bytes\n").unwrap();
    let selected = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--path", "alias/shared.md"],
    );
    assert_eq!(selected["to"]["tracked_path"], "alias/shared.md");
    assert_eq!(selected["changed"], true);

    let missing = world.err(
        &world.project,
        &["source", "diff", &source_id, "--path", "missing.md"],
    );
    assert_eq!(missing["error"]["code"], "source_diff_path_not_found");
}

#[test]
fn source_diff_bounds_unicode_output_and_rejects_invalid_limits() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/unicode.md", "甲乙丙丁\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&source, "甲乙😀丁\n").unwrap();

    let one = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--max-chars", "1"],
    );
    assert_eq!(one["diff"]["text"].as_str().unwrap().chars().count(), 1);
    assert_eq!(one["diff"]["returned_chars"], 1);
    assert!(one["diff"]["total_chars"].as_u64().unwrap() > 1);
    assert_eq!(one["diff"]["truncated"], true);

    assert_eq!(
        world.err(
            &world.project,
            &["source", "diff", &source_id, "--max-chars", "0"],
        )["error"]["code"],
        "invalid_limit"
    );
    assert_eq!(
        world.err(
            &world.project,
            &["source", "diff", &source_id, "--max-chars", "100001"],
        )["error"]["code"],
        "invalid_limit"
    );
}

#[test]
fn source_diff_global_scope_never_falls_back_to_the_project_store() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let project_source = world.write("project.md", "project old\n");
    world.ok(&world.project, &["source", "add", as_str(&project_source)]);

    world.ok(&world.project, &["--scope", "global", "init"]);
    let global_source = world.write("global.md", "global old\n");
    let global_old = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&global_source)],
    );
    let old_id = global_old["source"]["id"].as_i64().unwrap().to_string();
    fs::write(&global_source, "global new\n").unwrap();

    let live = world.ok(
        &world.project,
        &["--scope", "global", "source", "diff", &old_id],
    );
    assert_eq!(live["scope"], "global");
    assert_eq!(
        live["from"]["content_hash"],
        global_old["source"]["content_hash"]
    );
    assert!(
        live["diff"]["text"]
            .as_str()
            .unwrap()
            .contains("-global old\n+global new\n")
    );

    let global_new = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&global_source)],
    );
    let new_id = global_new["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&global_source).unwrap();
    let snapshots = world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "source",
            "diff",
            &old_id,
            "--to-source",
            &new_id,
        ],
    );
    assert_eq!(snapshots["scope"], "global");
    assert_eq!(
        snapshots["to"]["content_hash"],
        global_new["source"]["content_hash"]
    );
}

#[test]
fn source_diff_rejects_missing_and_invalid_utf8_live_files() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/unavailable.md", "valid evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    fs::remove_file(&source).unwrap();
    let missing = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(missing["error"]["code"], "source_diff_unavailable");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing")
    );

    fs::write(&source, [0xff, 0xfe, 0xfd]).unwrap();
    let invalid = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(invalid["error"]["code"], "invalid_utf8");
}

#[test]
fn source_diff_rejects_untracked_and_oversized_inputs_before_rendering() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let untracked = world.write("raw/untracked.md", "evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&untracked)]);
    let untracked_id = added["source"]["id"].as_i64().unwrap().to_string();
    Connection::open(world.project.join(".lwc/wiki.db"))
        .unwrap()
        .execute(
            "DELETE FROM source_path_revisions WHERE source_id = ?1",
            [added["source"]["id"].as_i64().unwrap()],
        )
        .unwrap();
    let error = world.err(&world.project, &["source", "diff", &untracked_id]);
    assert_eq!(error["error"]["code"], "source_diff_untracked");

    let live = world.write("raw/large-live.md", "small\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&live)]);
    let live_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::File::create(&live)
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    let error = world.err(&world.project, &["source", "diff", &live_id]);
    assert_eq!(error["error"]["code"], "source_diff_too_large");

    let too_many_lines = world.write("raw/many-lines.md", &"x\n".repeat(200_001));
    let error = world.err(&world.project, &["source", "add", as_str(&too_many_lines)]);
    assert_eq!(error["error"]["code"], "graph_index_capacity_exceeded");
}

#[cfg(unix)]
#[test]
fn source_diff_blocks_symlink_escape_and_never_blocks_on_a_fifo() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/live.md", "inside evidence\n");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let external = world.project.parent().unwrap().join("outside.md");
    fs::write(&external, "outside evidence\n").unwrap();
    fs::remove_file(&tracked).unwrap();
    symlink(&external, &tracked).unwrap();

    let blocked = world.err(&world.project, &["source", "diff", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "diff", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["changed"], true);

    fs::remove_file(&tracked).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&tracked)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["source", "diff", &source_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("source diff blocked while opening a FIFO");
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "source_diff_unavailable"
    );
}

#[test]
fn source_status_reports_a_b_a_lineage_per_path_and_all_current_heads() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let source = world.write("raw/lineage.md", "alpha");
    let alpha = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let alpha_id = alpha["source"]["id"].as_i64().unwrap().to_string();

    fs::write(&source, "bravo").unwrap();
    let bravo = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let bravo_id = bravo["source"]["id"].as_i64().unwrap().to_string();

    fs::write(&source, "alpha").unwrap();
    let alpha_again = world.ok(&world.project, &["source", "add", as_str(&source)]);
    assert_eq!(alpha_again["source"]["id"], alpha["source"]["id"]);
    let alias = world.write("alias/lineage.md", "alpha");
    world.ok(&world.project, &["source", "add", as_str(&alias)]);

    let superseded = world.ok(&world.project, &["source", "status", &bravo_id]);
    assert_eq!(superseded["checks"].as_array().unwrap().len(), 1);
    assert_eq!(superseded["checks"][0]["tracked_path"], "raw/lineage.md");
    assert_eq!(superseded["checks"][0]["head_revision"], 3);
    assert_eq!(
        superseded["checks"][0]["head_source_id"],
        alpha["source"]["id"]
    );
    assert_eq!(superseded["checks"][0]["lineage_state"], "superseded");
    assert_eq!(superseded["checks"][0]["filesystem_state"], "current");

    let current = world.ok(&world.project, &["source", "status", &alpha_id]);
    assert_eq!(current["checks"].as_array().unwrap().len(), 2);
    assert_eq!(current["checks"][0]["tracked_path"], "alias/lineage.md");
    assert_eq!(current["checks"][0]["head_revision"], 1);
    assert_eq!(current["checks"][1]["tracked_path"], "raw/lineage.md");
    assert_eq!(current["checks"][1]["head_revision"], 3);

    let combined = world.ok(&world.project, &["source", "status", &alpha_id, &bravo_id]);
    let raw_path_checks = combined["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["tracked_path"] == "raw/lineage.md")
        .collect::<Vec<_>>();
    assert_eq!(raw_path_checks.len(), 2);
    assert!(
        raw_path_checks
            .iter()
            .all(|check| check["head_source_id"] == alpha["source"]["id"])
    );

    let all = world.ok(&world.project, &["source", "status", "--all"]);
    assert_eq!(all["checks"].as_array().unwrap().len(), 2);
    assert!(all["checks"].as_array().unwrap().iter().all(|check| {
        check["requested_source_id"] == alpha["source"]["id"]
            && check["lineage_state"] == "current"
            && check["filesystem_state"] == "current"
    }));
    assert!(all["untracked_source_ids"].as_array().unwrap().is_empty());
}

#[test]
fn source_status_requires_current_external_read_authorization() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let external = world.project.parent().unwrap().join("external-source.md");
    fs::write(&external, "external evidence").unwrap();
    let added = world.ok(
        &world.project,
        &[
            "source",
            "add",
            as_str(&external),
            "--allow-external-source",
        ],
    );
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let blocked = world.err(&world.project, &["source", "status", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "status", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["checks"][0]["filesystem_state"], "current");

    let blocked_all = world.err(&world.project, &["source", "status", "--all"]);
    assert_eq!(
        blocked_all["error"]["code"],
        "external_source_requires_acknowledgement"
    );

    world.ok(&world.project, &["--scope", "global", "init"]);
    let global = world.ok(
        &world.project,
        &["--scope", "global", "source", "add", as_str(&external)],
    );
    let global_id = global["source"]["id"].as_i64().unwrap().to_string();
    let global_status = world.ok(
        &world.project,
        &["--scope", "global", "source", "status", &global_id],
    );
    assert_eq!(global_status["checks"][0]["filesystem_state"], "current");
}

#[cfg(unix)]
#[test]
fn source_status_blocks_a_tracked_path_that_now_escapes_through_a_symlink() {
    use std::os::unix::fs::symlink;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/symlink.md", "inside evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let external = world.project.parent().unwrap().join("symlink-target.md");
    fs::write(&external, "outside evidence").unwrap();
    fs::remove_file(&tracked).unwrap();
    symlink(&external, &tracked).unwrap();

    let blocked = world.err(&world.project, &["source", "status", &source_id]);
    assert_eq!(
        blocked["error"]["code"],
        "external_source_requires_acknowledgement"
    );
    let allowed = world.ok(
        &world.project,
        &["source", "status", &source_id, "--allow-external-source"],
    );
    assert_eq!(allowed["checks"][0]["filesystem_state"], "modified");
}

#[cfg(unix)]
#[test]
fn source_status_rejects_a_fifo_without_blocking() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let tracked = world.write("raw/fifo.md", "regular evidence");
    let added = world.ok(&world.project, &["source", "add", as_str(&tracked)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    fs::remove_file(&tracked).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&tracked)
            .status()
            .unwrap()
            .success()
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(&world.project)
        .env("HOME", &world.home)
        .args(["source", "status", &source_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("source status blocked while opening a FIFO");
    }

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "source status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = stdout_json(&output);
    assert_eq!(status["checks"][0]["filesystem_state"], "unreadable");
}
