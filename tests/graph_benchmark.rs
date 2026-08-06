use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct BenchWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl BenchWorld {
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

    fn run(&self, args: &[&str]) -> (Value, Duration) {
        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .args(args)
            .output()
            .unwrap();
        let elapsed = started.elapsed();
        assert!(
            output.status.success(),
            "command {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (serde_json::from_slice(&output.stdout).unwrap(), elapsed)
    }
}

fn p95(samples: &mut [Duration]) -> f64 {
    samples.sort();
    let index = ((samples.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples[index].as_secs_f64() * 1000.0
}

fn write_document(path: &Path, index: usize, documents: usize) -> usize {
    let previous = index
        .checked_sub(1)
        .map(|value| format!(" [[bench-{value}]]"))
        .unwrap_or_default();
    let mut body = format!(
        "# Benchmark {index}\n\nDeterministicTerm{index} sharedgraphterm evidence sentence.{previous}\n\n"
    );
    let phrase = format!(
        "Document {index} of {documents} records stable graph evidence deterministic retrieval context and Unicode 知识图谱 semantics "
    )
    .repeat(32);
    let mut section = 0usize;
    while body.len() < 64 * 1024 {
        body.push_str(&format!("{phrase} Section {}.\n", section % 16));
        section += 1;
    }
    fs::write(path, &body).unwrap();
    body.len()
}

fn write_replacement(path: &Path, revision: usize) -> usize {
    let mut body = format!("# 100 KiB replacement revision {revision}\n\n");
    let target = 100 * 1024;
    let mut sentence = 0usize;
    let phrase = "Stable replacement evidence preserves bounded graph projection semantics and deterministic exact context ".repeat(80);
    while body.len() < target {
        body.push_str(&format!(
            "{phrase} Sentence {} revision {revision}.\n",
            sentence % 32
        ));
        sentence += 1;
    }
    body.truncate(target);
    while !body.is_char_boundary(body.len()) {
        body.pop();
    }
    fs::write(path, &body).unwrap();
    body.len()
}

#[test]
#[ignore = "deterministic graph performance benchmark; run in release mode"]
fn graph_benchmark_reports_latency_growth_projection_and_bounds() {
    let documents = env::var("LWC_GRAPH_BENCH_DOCS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(20, 1_000);
    let samples = env::var("LWC_GRAPH_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(10, 200);
    let world = BenchWorld::new();
    world.run(&["init"]);
    world.run(&["config", "set", "--engine", "rslg"]);

    let mut startup_samples = Vec::new();
    for _ in 0..samples {
        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
            .arg("--version")
            .output()
            .unwrap();
        assert!(output.status.success());
        startup_samples.push(started.elapsed());
    }
    let startup_p95_ms = p95(&mut startup_samples);

    let started = Instant::now();
    let mut authoritative_bytes = 0usize;
    let mut document_bytes = Vec::with_capacity(documents);
    for index in 0..documents {
        let path = world.project.join(format!("bench-{index}.md"));
        let bytes = write_document(&path, index, documents);
        authoritative_bytes += bytes;
        document_bytes.push(bytes);
        world.run(&[
            "page",
            "put",
            &format!("bench-{index}"),
            "--title",
            &format!("Benchmark {index}"),
            "--file",
            path.to_str().unwrap(),
        ]);
    }
    let indexing_ms = started.elapsed().as_secs_f64() * 1000.0;
    world.run(&["maintenance", "compact"]);

    let mut sentence_search = Vec::new();
    let mut passage_search = Vec::new();
    let mut expansion = Vec::new();
    let mut neighbors = Vec::new();
    let mut paths = Vec::new();
    let mut impacts = Vec::new();
    let mut overviews = Vec::new();
    let (sentence, _) = world.run(&[
        "search",
        "sharedgraphterm",
        "--granularity",
        "sentence",
        "--limit",
        "20",
    ]);
    let span_id = sentence["results"][0]["identifier"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..samples {
        sentence_search.push(
            world
                .run(&[
                    "search",
                    "sharedgraphterm",
                    "--granularity",
                    "sentence",
                    "--limit",
                    "20",
                ])
                .1,
        );
        passage_search.push(
            world
                .run(&[
                    "search",
                    "sharedgraphterm",
                    "--granularity",
                    "passage",
                    "--limit",
                    "20",
                ])
                .1,
        );
        expansion.push(world.run(&["span", "expand", &span_id]).1);
        neighbors.push(
            world
                .run(&[
                    "graph",
                    "neighbors",
                    &format!("page:bench-{}", documents - 1),
                    "--limit",
                    "200",
                ])
                .1,
        );
        paths.push(
            world
                .run(&[
                    "graph",
                    "path",
                    &format!("page:bench-{}", documents - 1),
                    "page:bench-0",
                    "--max-depth",
                    "10",
                ])
                .1,
        );
        impacts.push(
            world
                .run(&[
                    "graph",
                    "impact",
                    "page:bench-0",
                    "--max-depth",
                    "4",
                    "--limit",
                    "200",
                ])
                .1,
        );
        overviews.push(world.run(&["graph", "overview", "--limit", "20"]).1);
    }

    let database = world.project.join(".lwc/wiki.db");
    let database_bytes = fs::metadata(&database).unwrap().len();
    let conn = Connection::open(&database).unwrap();
    let initial_delta_bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(COALESCE(before_json, '')) + LENGTH(COALESCE(after_json, ''))), 0) FROM graph_deltas",
        [],
        |row| row.get(0),
    ).unwrap();
    let graph_counts = json!({
        "nodes": conn.query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get::<_, i64>(0)).unwrap(),
        "edges": conn.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get::<_, i64>(0)).unwrap(),
        "spans": conn.query_row("SELECT COUNT(*) FROM graph_nodes WHERE node_type IN ('passage','sentence')", [], |row| row.get::<_, i64>(0)).unwrap(),
        "generations": conn.query_row("SELECT COUNT(*) FROM graph_generations", [], |row| row.get::<_, i64>(0)).unwrap(),
        "deltas": conn.query_row("SELECT COUNT(*) FROM graph_deltas", [], |row| row.get::<_, i64>(0)).unwrap(),
    });
    drop(conn);

    let replacement = world.project.join("bench-replacement.md");
    let replacement_bytes = write_replacement(&replacement, 1);
    let final_authoritative_bytes =
        authoritative_bytes - document_bytes[documents - 1] + replacement_bytes;
    let canonical_replacement_started = Instant::now();
    let canonical_response = world.run(&[
        "page",
        "put",
        &format!("bench-{}", documents - 1),
        "--title",
        &format!("Benchmark {}", documents - 1),
        "--file",
        replacement.to_str().unwrap(),
    ]);
    let canonical_replacement_total_ms =
        canonical_replacement_started.elapsed().as_secs_f64() * 1000.0;
    let canonical_replacement_ms = canonical_response.0["graph"]["canonical_duration_ms"]
        .as_u64()
        .unwrap() as f64;

    let graphqlite_started = Instant::now();
    world.run(&["config", "set", "--engine", "graphqlite"]);
    let graphqlite_full_projection_ms = graphqlite_started.elapsed().as_secs_f64() * 1000.0;
    write_replacement(&replacement, 2);
    let replacement_started = Instant::now();
    let graphqlite_response = world.run(&[
        "page",
        "put",
        &format!("bench-{}", documents - 1),
        "--title",
        &format!("Benchmark {}", documents - 1),
        "--file",
        replacement.to_str().unwrap(),
    ]);
    let graphqlite_replacement_total_ms = replacement_started.elapsed().as_secs_f64() * 1000.0;
    let graphqlite_replacement_ms = graphqlite_response.0["graph"]["projection_duration_ms"]
        .as_u64()
        .unwrap() as f64;
    let status = world.run(&["graph", "status"]).0;
    let final_database_bytes = fs::metadata(&database).unwrap().len();
    let final_conn = Connection::open(&database).unwrap();
    let final_delta_bytes: i64 = final_conn.query_row(
        "SELECT COALESCE(SUM(LENGTH(COALESCE(before_json, '')) + LENGTH(COALESCE(after_json, ''))), 0) FROM graph_deltas",
        [],
        |row| row.get(0),
    ).unwrap();
    let canonical_page_bytes: i64 = final_conn
        .query_row(
            "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name NOT LIKE 'graph_deltas%'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(-1);
    let mut page_statement = final_conn
        .prepare(
            "SELECT name, SUM(pgsize) AS bytes FROM dbstat
             WHERE name NOT LIKE 'graph_deltas%'
             GROUP BY name ORDER BY bytes DESC, name LIMIT 20",
        )
        .unwrap();
    let canonical_page_breakdown = page_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .unwrap();
    drop(page_statement);
    drop(final_conn);
    let sidecar_bytes = status["retained_sidecars"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(|name| {
            fs::metadata(world.project.join(".lwc").join(name))
                .unwrap()
                .len()
        })
        .sum::<u64>();

    let raw_sentence_search = p95(&mut sentence_search);
    let raw_passage_search = p95(&mut passage_search);
    let raw_expansion = p95(&mut expansion);
    let raw_neighbors = p95(&mut neighbors);
    let raw_paths = p95(&mut paths);
    let raw_impacts = p95(&mut impacts);
    let raw_overviews = p95(&mut overviews);
    let net = |duration: f64| (duration - startup_p95_ms).max(0.0);
    let report = json!({
        "schema": "lwc-graph-benchmark-v1",
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
        "target": {"os": env::consts::OS, "arch": env::consts::ARCH},
        "corpus": {
            "documents": documents,
            "initial_authoritative_bytes": authoritative_bytes,
            "final_authoritative_bytes": final_authoritative_bytes,
        },
        "indexing_ms": indexing_ms,
        "process_startup_p95_ms": startup_p95_ms,
        "latency_p95_ms": {
            "sentence_search": net(raw_sentence_search),
            "passage_search": net(raw_passage_search),
            "span_expand": net(raw_expansion),
            "neighbors": net(raw_neighbors),
            "path": net(raw_paths),
            "impact": net(raw_impacts),
            "overview": net(raw_overviews),
        },
        "end_to_end_p95_ms": {
            "sentence_search": raw_sentence_search,
            "passage_search": raw_passage_search,
            "span_expand": raw_expansion,
            "neighbors": raw_neighbors,
            "path": raw_paths,
            "impact": raw_impacts,
            "overview": raw_overviews,
        },
        "storage": {
            "initial_database_bytes": database_bytes,
            "final_database_bytes": final_database_bytes,
            "initial_delta_json_bytes": initial_delta_bytes,
            "final_delta_json_bytes": final_delta_bytes,
            "canonical_page_bytes_excluding_deltas": canonical_page_bytes,
            "canonical_page_breakdown": canonical_page_breakdown,
            "graphqlite_retained_sidecar_bytes": sidecar_bytes,
            "canonical_growth_ratio": canonical_page_bytes as f64
                / final_authoritative_bytes as f64,
        },
        "graph_counts": graph_counts,
        "replacement": {
            "bytes": replacement_bytes,
            "canonical_indexing_ms": canonical_replacement_ms,
            "canonical_command_total_ms": canonical_replacement_total_ms,
        },
        "projection_ms": {
            "full_snapshot": graphqlite_full_projection_ms,
            "document_replacement_incremental": graphqlite_replacement_ms,
            "document_replacement_total": graphqlite_replacement_total_ms,
        },
        "projection": status["projection"],
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    assert_eq!(status["projection"]["status"], "fresh");
    assert!(report["graph_counts"]["nodes"].as_i64().unwrap() > documents as i64);
    if !cfg!(debug_assertions) {
        assert!(
            report["latency_p95_ms"]["sentence_search"]
                .as_f64()
                .unwrap()
                <= 150.0
        );
        assert!(report["latency_p95_ms"]["passage_search"].as_f64().unwrap() <= 150.0);
        assert!(report["latency_p95_ms"]["span_expand"].as_f64().unwrap() <= 50.0);
        assert!(report["latency_p95_ms"]["neighbors"].as_f64().unwrap() <= 200.0);
        assert!(report["latency_p95_ms"]["path"].as_f64().unwrap() <= 200.0);
        assert!(report["latency_p95_ms"]["impact"].as_f64().unwrap() <= 500.0);
        assert!(report["latency_p95_ms"]["overview"].as_f64().unwrap() <= 500.0);
        assert!(canonical_replacement_ms <= 1_500.0);
        assert!(graphqlite_replacement_ms <= 750.0);
        if documents >= 100 {
            assert!(
                report["storage"]["canonical_growth_ratio"]
                    .as_f64()
                    .unwrap()
                    <= 4.0
            );
        }
    }
}
