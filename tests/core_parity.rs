use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
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

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "command {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn err(&self, args: &[&str]) -> Value {
        let output = self.command(args);
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

#[test]
fn directory_ingest_is_recursive_filtered_and_idempotent() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let corpus = world.project.join("corpus");
    fs::create_dir_all(corpus.join("nested")).unwrap();
    fs::create_dir_all(corpus.join(".obsidian/plugins")).unwrap();
    fs::write(corpus.join("one.md"), "# One\n中文知识").unwrap();
    fs::write(corpus.join("nested/two.sql"), "select 2;").unwrap();
    fs::write(corpus.join("nested/view.base"), "filters: []").unwrap();
    fs::write(corpus.join("ignored.png"), [0xff, 0xd8, 0xff]).unwrap();
    fs::write(corpus.join(".obsidian/plugins/main.js"), "secret plugin").unwrap();

    let first = world.ok(&["source", "add-dir", as_str(&corpus)]);
    assert_eq!(first["discovered"], 3);
    assert_eq!(first["created"], 3);
    assert_eq!(first["duplicates"], 0);
    assert_eq!(first["skipped"], 0);

    let second = world.ok(&["source", "add-dir", as_str(&corpus)]);
    assert_eq!(second["discovered"], 3);
    assert_eq!(second["created"], 0);
    assert_eq!(second["duplicates"], 3);

    let sources = world.ok(&["source", "list", "--limit", "10"]);
    assert_eq!(sources["sources"].as_array().unwrap().len(), 3);
    assert!(
        sources["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["title"] == "nested/two.sql")
    );
}

#[test]
fn directory_ingest_reports_partial_import_as_an_error() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let corpus = world.project.join("partial-corpus");
    fs::create_dir_all(&corpus).unwrap();
    fs::write(corpus.join("good.md"), "valid source").unwrap();
    fs::write(corpus.join("bad.txt"), [0xff, 0xfe]).unwrap();

    let error = world.err(&["source", "add-dir", as_str(&corpus)]);
    assert_eq!(error["error"]["code"], "partial_import");

    let sources = world.ok(&["source", "list"]);
    assert_eq!(sources["sources"].as_array().unwrap().len(), 1);
}

#[test]
fn agent_ingest_queue_persists_two_steps_and_requires_a_source_summary() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let source = world.write("paper.md", "注意力机制连接查询、键和值。");
    let added = world.ok(&["source", "add", as_str(&source), "--title", "Attention"]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();

    let pending = world.ok(&["ingest", "list", "--status", "pending"]);
    assert_eq!(pending["jobs"].as_array().unwrap().len(), 1);

    let packet = world.ok(&["ingest", "next", "--context-limit", "5"]);
    assert_eq!(packet["job"]["status"], "analyzing");
    assert_eq!(
        packet["job"]["source"]["content"],
        "注意力机制连接查询、键和值。"
    );
    assert!(packet["schema"].is_string());
    assert!(packet["purpose"].is_string());

    let premature = world.err(&["ingest", "complete", &source_id]);
    assert_eq!(premature["error"]["code"], "invalid_ingest_state");

    let analysis = world.write(
        "analysis.md",
        "# Analysis\nCreate a source summary and an attention concept.",
    );
    let analyzed = world.ok(&["ingest", "analyze", &source_id, "--file", as_str(&analysis)]);
    assert_eq!(analyzed["job"]["status"], "generating");

    let uncited = world.err(&["ingest", "complete", &source_id]);
    assert_eq!(uncited["error"]["code"], "source_summary_missing");

    let summary = world.write("summary.md", "该来源解释了 [[attention-mechanism]]。");
    world.ok(&[
        "page",
        "put",
        "attention-source",
        "--title",
        "Attention source",
        "--kind",
        "source",
        "--summary",
        "注意力机制来源摘要",
        "--file",
        as_str(&summary),
        "--source",
        &source_id,
    ]);
    let completed = world.ok(&["ingest", "complete", &source_id]);
    assert_eq!(completed["job"]["status"], "completed");

    let persisted = world.ok(&["ingest", "list", "--status", "completed"]);
    assert_eq!(persisted["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(persisted["jobs"][0]["source_id"], 1);
}

#[test]
fn purpose_and_markdown_projection_preserve_the_three_layer_architecture() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    assert!(world.project.join(".lwc/schema.md").is_file());
    assert!(world.project.join(".lwc/purpose.md").is_file());
    assert!(world.project.join(".lwc/wiki/index.md").is_file());
    assert!(world.project.join(".lwc/wiki/log.md").is_file());
    assert!(world.project.join(".lwc/wiki/overview.md").is_file());
    assert!(world.project.join(".lwc/raw/sources").is_dir());

    let purpose = world.write(
        "purpose-input.md",
        "# Purpose\nUnderstand the product architecture.",
    );
    world.ok(&["purpose", "set", as_str(&purpose)]);
    let shown = world.ok(&["purpose", "show"]);
    assert_eq!(
        shown["purpose"],
        "# Purpose\nUnderstand the product architecture."
    );

    let source = world.write("source.md", "# Evidence\nSQLite is the index.");
    let added = world.ok(&["source", "add", as_str(&source), "--title", "Evidence"]);
    let source_id = added["source"]["id"].as_i64().unwrap().to_string();
    let page = world.write("page.md", "Body with [[other-page]].");
    world.ok(&[
        "page",
        "put",
        "architecture",
        "--title",
        "Architecture",
        "--kind",
        "concept",
        "--summary",
        "Hybrid storage",
        "--file",
        as_str(&page),
        "--source",
        &source_id,
    ]);
    world.ok(&["maintenance", "materialize"]);

    let projected =
        fs::read_to_string(world.project.join(".lwc/wiki/concepts/architecture.md")).unwrap();
    assert!(projected.starts_with("---\n"));
    assert!(projected.contains("type: \"concept\""));
    assert!(projected.contains("title: \"Architecture\""));
    assert!(projected.contains("sources:\n  - \"raw/sources/"));
    assert!(projected.contains("Body with [[other-page]]."));

    let index = fs::read_to_string(world.project.join(".lwc/wiki/index.md")).unwrap();
    assert!(index.contains("[[concepts/architecture|Architecture]]"));
    let log = fs::read_to_string(world.project.join(".lwc/wiki/log.md")).unwrap();
    assert!(log.contains("source_add"));
    assert!(log.contains("page_put"));
    assert_eq!(
        fs::read_to_string(world.project.join(".lwc/purpose.md")).unwrap(),
        "# Purpose\nUnderstand the product architecture."
    );
    assert_eq!(
        fs::read_dir(world.project.join(".lwc/raw/sources"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn related_graph_exposes_weighted_signals_deterministically() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let evidence = world.write("evidence.md", "shared evidence");
    let source = world.ok(&["source", "add", as_str(&evidence)]);
    let source_id = source["source"]["id"].as_i64().unwrap().to_string();

    for (slug, kind, body, cite) in [
        ("alpha", "entity", "[[beta]] and [[bridge]]", true),
        ("beta", "concept", "[[alpha]] and [[bridge]]", true),
        ("bridge", "concept", "[[alpha]] and [[beta]]", false),
        ("unrelated", "query", "standalone", false),
    ] {
        let file = world.write(&format!("{slug}.md"), body);
        let mut args = vec![
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--kind",
            kind,
            "--file",
            as_str(&file),
        ];
        if cite {
            args.extend(["--source", &source_id]);
        }
        world.ok(&args);
    }

    let graph = world.ok(&["graph", "related", "alpha", "--limit", "3"]);
    let related = graph["related"].as_array().unwrap();
    assert_eq!(related[0]["slug"], "beta");
    assert_eq!(related[0]["direct_links"], 2);
    assert_eq!(related[0]["shared_sources"], 1);
    assert!(related[0]["adamic_adar"].as_f64().unwrap() > 0.0);
    assert!(related[0]["score"].as_f64().unwrap() > related[1]["score"].as_f64().unwrap());
}

#[test]
fn maintenance_reindex_repairs_missing_fts_rows() {
    let world = TestWorld::new();
    let initialized = world.ok(&["init"]);
    let page = world.write("repair.md", "repairable unique phrase");
    world.ok(&[
        "page",
        "put",
        "repair",
        "--title",
        "Repair",
        "--file",
        as_str(&page),
    ]);

    let database = Path::new(initialized["database"].as_str().unwrap());
    let conn = Connection::open(database).unwrap();
    conn.execute(
        "DELETE FROM search_fts WHERE doc_type = 'page' AND identifier = 'repair'",
        [],
    )
    .unwrap();
    drop(conn);

    let before = world.ok(&["lint"]);
    assert_eq!(before["counts"]["search_index_missing"], 1);
    let rebuilt = world.ok(&["maintenance", "reindex"]);
    assert_eq!(rebuilt["pages"], 1);
    assert_eq!(rebuilt["sources"], 0);

    let search = world.ok(&["search", "repairable"]);
    assert_eq!(search["results"][0]["identifier"], "repair");
    let after = world.ok(&["lint"]);
    assert!(after["counts"].get("search_index_missing").is_none());
}

#[test]
fn graph_navigation_exposes_backlinks_missing_links_and_source_refs() {
    let world = TestWorld::new();
    world.ok(&["init"]);
    let evidence = world.write("evidence.md", "traceable evidence");
    let source = world.ok(&["source", "add", as_str(&evidence)]);
    let source_id = source["source"]["id"].as_i64().unwrap().to_string();

    for (slug, body) in [
        ("alpha", "[[beta]] and [[missing-page]]"),
        ("beta", "[[alpha]]"),
    ] {
        let file = world.write(&format!("{slug}.md"), body);
        world.ok(&[
            "page",
            "put",
            slug,
            "--title",
            slug,
            "--file",
            as_str(&file),
            "--source",
            &source_id,
        ]);
    }

    let links = world.ok(&["page", "links", "alpha"]);
    assert_eq!(
        links["outgoing"],
        serde_json::json!(["beta", "missing-page"])
    );
    assert_eq!(links["backlinks"], serde_json::json!(["beta"]));
    assert_eq!(links["missing"], serde_json::json!(["missing-page"]));

    let refs = world.ok(&["source", "refs", &source_id]);
    assert_eq!(refs["pages"].as_array().unwrap().len(), 2);
    assert_eq!(refs["pages"][0]["slug"], "alpha");
    assert_eq!(refs["pages"][1]["slug"], "beta");
}
