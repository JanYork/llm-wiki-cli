use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
    thread,
};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl TestWorld {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let home = temp.path().join("home");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self {
            _temp: temp,
            project,
            home,
        }
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", &self.home)
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

    fn err(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.command(cwd, args);
        assert!(
            !output.status.success(),
            "command {args:?} unexpectedly succeeded\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stderr).unwrap()
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.project.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }
}

fn as_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn run_command(bin: &str, home: &Path, cwd: &Path, args: &[&str]) -> ExitStatus {
    Command::new(bin)
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
}

#[test]
fn chinese_subword_search_hits() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let page = world.write("zh.md", "注意力机制帮助模型聚焦上下文中的关键信号。");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "zh-attention",
            "--title",
            "中文检索",
            "--file",
            as_str(&page),
        ],
    );

    let search = world.ok(&world.project, &["search", "注意力"]);
    assert!(
        search["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|result| result["identifier"] == "zh-attention"),
        "expected Chinese subword search to hit page, got: {search}"
    );
}

#[test]
fn punctuation_queries_do_not_raise_fts_syntax_errors() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let page = world.write(
        "punctuation.md",
        "C++ foo-bar A/B email@example.com 引号\"测试\"",
    );
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "punctuation",
            "--title",
            "Punctuation",
            "--file",
            as_str(&page),
        ],
    );

    for query in ["C++", "foo-bar", "A/B", "email@example.com", "引号\"测试\""] {
        let output = world.command(&world.project, &["search", query]);
        assert!(
            output.status.success(),
            "query {query:?} should not fail\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            json["results"].is_array(),
            "query {query:?} returned non-search JSON: {json}"
        );
    }
}

#[test]
fn all_scope_limit_one_prefers_stronger_global_result() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    world.ok(&world.project, &["--scope", "global", "init"]);

    let project_page = world.write("project-weak.md", "rareterm appears once.");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "project-weak",
            "--title",
            "Project weak",
            "--file",
            as_str(&project_page),
        ],
    );

    let global_page = world.write(
        "global-strong.md",
        "rareterm rareterm rareterm rareterm rareterm rareterm rareterm rareterm",
    );
    world.ok(
        &world.project,
        &[
            "--scope",
            "global",
            "page",
            "put",
            "global-strong",
            "--title",
            "Global strong",
            "--file",
            as_str(&global_page),
        ],
    );

    let combined = world.ok(
        &world.project,
        &["--scope", "all", "search", "rareterm", "--limit", "1"],
    );
    let results = combined["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        1,
        "expected exactly one merged result: {combined}"
    );
    assert_eq!(
        results[0]["scope"], "global",
        "expected stronger global result to survive all-scope limit=1: {combined}"
    );
}

#[test]
fn ascii_token_ranking_does_not_count_substrings_inside_other_words() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let substring_heavy = world.write("substring-heavy.md", "ai paid paid paid paid");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "substring-heavy",
            "--title",
            "Paid rail",
            "--file",
            as_str(&substring_heavy),
        ],
    );
    let exact_tokens = world.write("exact-tokens.md", "ai ai");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "exact-tokens",
            "--title",
            "Exact tokens",
            "--file",
            as_str(&exact_tokens),
        ],
    );

    let search = world.ok(&world.project, &["search", "ai", "--limit", "1"]);
    assert_eq!(
        search["results"][0]["identifier"], "exact-tokens",
        "ranking must count exact indexed tokens, not ai inside paid/rail: {search}"
    );
}

#[test]
fn source_list_omits_full_content_bodies() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let source = world.write(
        "raw/source.md",
        "secret body that should not be listed verbatim",
    );
    world.ok(
        &world.project,
        &["source", "add", as_str(&source), "--title", "Bodyless list"],
    );

    let listed = world.ok(&world.project, &["source", "list"]);
    let sources = listed["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 1, "expected one listed source: {listed}");
    assert!(
        sources[0].get("content").is_none(),
        "source list should omit full content bodies: {listed}"
    );
}

#[test]
fn source_and_page_lists_honor_limit_offset_and_has_more() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    for index in 1..=3 {
        let source = world.write(
            &format!("raw/source-{index}.md"),
            &format!("distinct source {index}"),
        );
        world.ok(&world.project, &["source", "add", as_str(&source)]);
    }
    for slug in ["alpha", "beta", "gamma"] {
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

    let sources = world.ok(
        &world.project,
        &["source", "list", "--limit", "1", "--offset", "1"],
    );
    assert_eq!(sources["sources"].as_array().unwrap().len(), 1);
    assert_eq!(sources["sources"][0]["id"], 2);
    assert_eq!(sources["limit"], 1);
    assert_eq!(sources["offset"], 1);
    assert_eq!(sources["has_more"], true);

    let pages = world.ok(
        &world.project,
        &["page", "list", "--limit", "1", "--offset", "1"],
    );
    assert_eq!(pages["pages"].as_array().unwrap().len(), 1);
    assert_eq!(pages["pages"][0]["slug"], "beta");
    assert_eq!(pages["limit"], 1);
    assert_eq!(pages["offset"], 1);
    assert_eq!(pages["has_more"], true);
}

#[test]
fn lint_is_paginated_but_counts_the_complete_wiki() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    for slug in ["one", "two", "three"] {
        let page = world.write(&format!("{slug}.md"), "body without links");
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

    let lint = world.ok(&world.project, &["lint", "--limit", "1", "--offset", "1"]);
    assert_eq!(lint["issues"].as_array().unwrap().len(), 1);
    assert_eq!(lint["limit"], 1);
    assert_eq!(lint["offset"], 1);
    assert_eq!(lint["has_more"], true);
    assert!(lint["total"].as_u64().unwrap() >= 9);
}

#[test]
fn nested_init_reuses_parent_store_without_creating_shadow_db() {
    let world = TestWorld::new();
    let parent = world.ok(&world.project, &["init"]);
    let parent_db = fs::canonicalize(Path::new(parent["database"].as_str().unwrap())).unwrap();

    let child = world.project.join("nested/child");
    fs::create_dir_all(&child).unwrap();

    let nested = world.ok(&child, &["init"]);
    assert_eq!(
        nested["created"], false,
        "nested init should report reuse: {nested}"
    );
    assert_eq!(
        fs::canonicalize(Path::new(nested["database"].as_str().unwrap())).unwrap(),
        parent_db,
        "nested init should reuse parent database: {nested}"
    );
    assert!(
        !child.join(".lwc/wiki.db").exists(),
        "nested init should not create a shadow database in the child directory"
    );
}

#[test]
fn unsupported_store_version_is_structured() {
    let world = TestWorld::new();
    let initialized = world.ok(&world.project, &["init"]);
    let db_path = Path::new(initialized["database"].as_str().unwrap());

    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch("PRAGMA user_version = 99;").unwrap();

    let error = world.err(&world.project, &["page", "list"]);
    assert_eq!(
        error["error"]["code"], "unsupported_store_version",
        "expected explicit store-version guard: {error}"
    );
}

#[test]
fn concurrent_searches_page_puts_and_duplicate_sources_all_succeed() {
    const WORKERS: usize = 16;

    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);

    let duplicate_source = world.write("raw/duplicate.md", "parallel duplicate source content");
    let mut page_files = Vec::new();
    for index in 0..WORKERS {
        page_files.push(world.write(
            &format!("pages/page-{index}.md"),
            &format!("parallel-term page {index}"),
        ));
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(WORKERS * 3));
    let bin = env!("CARGO_BIN_EXE_lwc").to_string();
    let home = world.home.clone();
    let cwd = world.project.clone();
    thread::scope(|scope| {
        let mut handles = Vec::new();

        for (index, page_path) in page_files.iter().cloned().enumerate() {
            let source_path = duplicate_source.clone();
            let barrier_for_source = barrier.clone();
            let bin_for_source = bin.clone();
            let home_for_source = home.clone();
            let cwd_for_source = cwd.clone();
            handles.push(scope.spawn(move || {
                barrier_for_source.wait();
                run_command(
                    &bin_for_source,
                    &home_for_source,
                    &cwd_for_source,
                    &[
                        "source",
                        "add",
                        as_str(&source_path),
                        "--title",
                        "parallel duplicate",
                    ],
                )
            }));

            let barrier_for_page = barrier.clone();
            let bin_for_page = bin.clone();
            let home_for_page = home.clone();
            let cwd_for_page = cwd.clone();
            handles.push(scope.spawn(move || {
                barrier_for_page.wait();
                let slug = format!("parallel-page-{index}");
                run_command(
                    &bin_for_page,
                    &home_for_page,
                    &cwd_for_page,
                    &[
                        "page",
                        "put",
                        &slug,
                        "--title",
                        &slug,
                        "--file",
                        as_str(&page_path),
                    ],
                )
            }));

            let barrier_for_search = barrier.clone();
            let bin_for_search = bin.clone();
            let home_for_search = home.clone();
            let cwd_for_search = cwd.clone();
            handles.push(scope.spawn(move || {
                barrier_for_search.wait();
                run_command(
                    &bin_for_search,
                    &home_for_search,
                    &cwd_for_search,
                    &["search", "parallel-term"],
                )
            }));
        }

        for handle in handles {
            let status = handle.join().unwrap();
            assert!(
                status.success(),
                "concurrent command failed with status {status}"
            );
        }
    });

    let listed = world.ok(&world.project, &["source", "list"]);
    assert_eq!(
        listed["sources"].as_array().unwrap().len(),
        1,
        "duplicate source ingests should converge to one stored source: {listed}"
    );
    let pages = world.ok(&world.project, &["page", "list"]);
    assert_eq!(pages["pages"].as_array().unwrap().len(), WORKERS);
}

#[test]
fn search_logging_is_explicit_to_avoid_persisting_sensitive_queries() {
    let world = TestWorld::new();
    world.ok(&world.project, &["init"]);
    let page = world.write("privacy.md", "private-query-term");
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            "privacy",
            "--title",
            "Privacy",
            "--file",
            as_str(&page),
        ],
    );

    world.ok(&world.project, &["search", "private-query-term"]);
    let unrecorded = world.ok(&world.project, &["log", "--limit", "20"]);
    assert!(
        unrecorded["operations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|operation| operation["target"] != "private-query-term")
    );

    world.ok(
        &world.project,
        &["search", "private-query-term", "--record"],
    );
    let recorded = world.ok(&world.project, &["log", "--limit", "20"]);
    assert!(
        recorded["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["action"] == "search"
                && operation["target"] == "private-query-term")
    );
    let projected_log = fs::read_to_string(world.project.join(".lwc/wiki/log.md")).unwrap();
    assert!(projected_log.contains("search"));
    assert!(projected_log.contains("private-query-term"));
}
