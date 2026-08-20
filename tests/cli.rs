use rusqlite::Connection;
use serde_json::Value;
#[cfg(unix)]
use std::process::Stdio;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
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

    fn command_in_project_root(&self, cwd: &Path, root: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("LWC_PROJECT_ROOT", root)
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
            "command {args:?} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
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

fn drop_temporal_schema(conn: &Connection) {
    conn.execute_batch(
        "DROP TABLE IF EXISTS memory_fts;
         DROP TABLE IF EXISTS memory_fts_data;
         DROP TABLE IF EXISTS memory_fts_idx;
         DROP TABLE IF EXISTS memory_fts_content;
         DROP TABLE IF EXISTS memory_fts_docsize;
         DROP TABLE IF EXISTS memory_fts_config;
         DROP TABLE IF EXISTS memory_feedback;
         DROP TABLE IF EXISTS memory_relations;
         DROP TABLE IF EXISTS memory_evidence;
         DROP TABLE IF EXISTS memory_changes;
         DROP TABLE IF EXISTS memory_fragments;
         DROP TABLE IF EXISTS memory_hint_state;
         DROP TABLE IF EXISTS memory_state;
         DROP TABLE IF EXISTS memory_events;",
    )
    .unwrap();
}

fn downgrade_to_v10(database: &Path) {
    let conn = Connection::open(database).unwrap();
    drop_temporal_schema(&conn);
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE IF EXISTS span_fts;
         DROP TABLE IF EXISTS span_fts_data;
         DROP TABLE IF EXISTS span_fts_idx;
         DROP TABLE IF EXISTS span_fts_content;
         DROP TABLE IF EXISTS span_fts_docsize;
         DROP TABLE IF EXISTS span_fts_config;
         DROP TABLE IF EXISTS graph_deltas;
         DROP TABLE IF EXISTS graph_generations;
         DROP TABLE IF EXISTS graph_projection_state;
         DROP TABLE IF EXISTS term_pair_totals;
         DROP TABLE IF EXISTS term_pair_contributions;
         DROP TABLE IF EXISTS graph_occurrences;
         DROP TABLE IF EXISTS graph_edges;
         DROP TABLE IF EXISTS graph_nodes;
         DROP TABLE IF EXISTS document_index_state;
         DROP TABLE IF EXISTS semantic_relations;
         DROP TABLE IF EXISTS page_tags;
         DROP TABLE IF EXISTS tags;
         UPDATE meta SET value = '10' WHERE key = 'format_version';
         PRAGMA user_version = 10;
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

fn prepare_generating_source_with_summary_only(world: &TestWorld, term: &str) -> String {
    world.ok(&world.project, &["init"]);

    let source = world.write("raw/source.md", term);
    let added = world.ok(&world.project, &["source", "add", as_str(&source)]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let claimed = world.ok(&world.project, &["ingest", "next"]);
    assert_eq!(claimed["job"]["source"]["id"], added["source"]["id"]);

    let analysis = world.write(
        "analysis.md",
        "# Analysis\n\n- Source summary exists.\n- No non-source derived page exists yet.\n",
    );
    world.ok(
        &world.project,
        &["ingest", "analyze", &source_id, "--file", as_str(&analysis)],
    );

    let summary = world.write("source-summary.md", "Source summary body.");
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
            as_str(&summary),
            "--source",
            &source_id,
        ],
    );

    source_id
}

fn put_page(world: &TestWorld, slug: &str, title: &str, body: &str) {
    let file = world.write(&format!("pages/{slug}.md"), body);
    world.ok(
        &world.project,
        &[
            "page",
            "put",
            slug,
            "--title",
            title,
            "--file",
            as_str(&file),
            "--provenance",
            "agent-observed",
        ],
    );
}

fn stage_clean_linked_pages(world: &TestWorld, changeset: &str, prefix: &str) {
    world.ok(&world.project, &["changeset", "begin", changeset]);
    stage_pages_in_existing_changeset(world, changeset, prefix);
}

fn stage_pages_in_existing_changeset(world: &TestWorld, changeset: &str, prefix: &str) {
    let first_slug = format!("{prefix}-first");
    let second_slug = format!("{prefix}-second");
    for (slug, title, body) in [
        (
            first_slug.as_str(),
            "First",
            format!("first [[{second_slug}]]"),
        ),
        (
            second_slug.as_str(),
            "Second",
            format!("second [[{first_slug}]]"),
        ),
    ] {
        let file = world.write(&format!("{slug}.md"), &body);
        world.ok(
            &world.project,
            &[
                "--changeset",
                changeset,
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
}

include!("cli/changesets.rs");
include!("cli/retrieval.rs");
include!("cli/graph.rs");
include!("cli/core.rs");
include!("cli/source_diff.rs");
include!("cli/source_status.rs");
include!("cli/remaining.rs");
