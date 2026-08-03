use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Instant,
};
use tempfile::TempDir;

struct TestWorld {
    _temp: TempDir,
    project: PathBuf,
    home: PathBuf,
    binary: PathBuf,
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
            binary: env::var_os("LWC_BENCH_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_lwc"))),
        }
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(&self.binary)
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
}

#[derive(Debug, Deserialize)]
struct QueryCase {
    query: String,
    expected_paths: Vec<String>,
    #[serde(default)]
    note: String,
}

#[derive(Debug)]
struct QueryMeasurement {
    elapsed_ms: f64,
    reciprocal_rank: f64,
    hit_at_5: bool,
    hit_at_10: bool,
}

#[test]
#[ignore = "local benchmark; requires LWC_BENCH_CORPUS"]
fn search_benchmark_reports_json_for_local_corpus() {
    let corpus = env::var("LWC_BENCH_CORPUS")
        .map(PathBuf::from)
        .expect("set LWC_BENCH_CORPUS to a local text corpus directory");
    assert!(
        corpus.is_dir(),
        "LWC_BENCH_CORPUS must point to a directory: {}",
        corpus.display()
    );

    let query_set = env::var("LWC_BENCH_QUERY_SET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("benchmarks/query-set.example.jsonl"));
    assert!(
        query_set.is_file(),
        "query set file does not exist: {}",
        query_set.display()
    );

    let queries = read_query_set(&query_set);
    assert!(
        !queries.is_empty(),
        "query set must contain at least one JSONL entry"
    );

    let world = TestWorld::new();
    let started = Instant::now();
    let init = world.ok(&world.project, &["init"]);
    let init_ms = started.elapsed().as_secs_f64() * 1000.0;

    let imported_at = Instant::now();
    let imported = world.ok(
        &world.project,
        &[
            "source",
            "add-dir",
            "--allow-external-source",
            corpus.to_str().unwrap(),
        ],
    );
    let import_ms = imported_at.elapsed().as_secs_f64() * 1000.0;

    let source_map = list_all_sources(&world);

    let mut measurements = Vec::with_capacity(queries.len());
    let mut query_reports = Vec::with_capacity(queries.len());
    for case in &queries {
        let measured = run_query(&world, &source_map, case);
        measurements.push(QueryMeasurement {
            elapsed_ms: measured["elapsed_ms"].as_f64().unwrap(),
            reciprocal_rank: measured["reciprocal_rank"].as_f64().unwrap(),
            hit_at_5: measured["hit_at_5"].as_bool().unwrap(),
            hit_at_10: measured["hit_at_10"].as_bool().unwrap(),
        });
        query_reports.push(measured);
    }

    let wiki_dir = world.project.join(".lwc");
    let db_path = wiki_dir.join("wiki.db");
    let wal_path = wiki_dir.join("wiki.db-wal");
    let before_compact = storage_snapshot(&wiki_dir, &db_path, &wal_path);
    let compact_output = world.command(&world.project, &["maintenance", "compact"]);
    let compact = if compact_output.status.success() {
        Some(serde_json::from_slice::<Value>(&compact_output.stdout).unwrap())
    } else {
        None
    };
    let after_compact = storage_snapshot(&wiki_dir, &db_path, &wal_path);
    let report = json!({
        "corpus": {
            "path": corpus,
            "query_set": query_set,
            "source_count": source_map.len(),
        },
        "import": {
            "init_ms": round3(init_ms),
            "source_add_dir_ms": round3(import_ms),
            "total_ms": round3(init_ms + import_ms),
            "created": imported["created"],
            "duplicates": imported["duplicates"],
            "discovered": imported["discovered"],
            "skipped": imported["skipped"],
            "project_database": init["database"],
        },
        "search": {
            "mode": "default_raw_only",
            "query_count": measurements.len(),
            "latency_ms": {
                "p50": percentile_ms(&measurements, 50),
                "p95": percentile_ms(&measurements, 95),
            },
            "quality": {
                "recall_at_5": rate(&measurements, 5),
                "recall_at_10": rate(&measurements, 10),
                "mrr": round3(
                    measurements
                        .iter()
                        .map(|entry| entry.reciprocal_rank)
                        .sum::<f64>()
                        / measurements.len() as f64
                ),
            },
        },
        "storage": {
            "before_compact": before_compact,
            "compact": compact,
            "after_compact": after_compact,
        },
        "queries": query_reports,
    });

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

fn storage_snapshot(wiki_dir: &Path, db_path: &Path, wal_path: &Path) -> Value {
    json!({
        "db_bytes": file_size(db_path),
        "wal_bytes": file_size(wal_path),
        "wiki_dir_bytes": dir_size(wiki_dir),
    })
}

fn read_query_set(path: &Path) -> Vec<QueryCase> {
    let file = fs::File::open(path).unwrap();
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = line.unwrap();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(
                serde_json::from_str::<QueryCase>(trimmed)
                    .unwrap_or_else(|err| panic!("invalid JSONL at line {}: {err}", index + 1)),
            )
        })
        .collect()
}

fn list_all_sources(world: &TestWorld) -> Vec<(String, String)> {
    let mut offset = 0usize;
    let mut sources = Vec::new();
    loop {
        let limit = 1000usize;
        let limit_text = limit.to_string();
        let offset_text = offset.to_string();
        let listed = world.ok(
            &world.project,
            &[
                "source",
                "list",
                "--limit",
                &limit_text,
                "--offset",
                &offset_text,
            ],
        );
        let batch = listed["sources"].as_array().unwrap();
        if batch.is_empty() {
            break;
        }
        sources.extend(batch.iter().map(|source| {
            (
                source["id"].to_string().trim_matches('"').to_string(),
                source["origin"].as_str().unwrap().replace('\\', "/"),
            )
        }));
        if batch.len() < limit {
            break;
        }
        offset += batch.len();
    }
    sources
}

fn run_query(world: &TestWorld, source_map: &[(String, String)], case: &QueryCase) -> Value {
    let started = Instant::now();
    let result = world.ok(&world.project, &["search", &case.query, "--limit", "10"]);
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    let expected = case
        .expected_paths
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<Vec<_>>();
    let matches =
        ranked_origin_matches(result["results"].as_array().unwrap(), source_map, &expected);

    let first_hit = matches.iter().position(|hit| *hit).map(|index| index + 1);
    json!({
        "query": case.query,
        "note": case.note,
        "expected_paths": case.expected_paths,
        "elapsed_ms": round3(elapsed_ms),
        "hit_at_5": matches.iter().take(5).any(|hit| *hit),
        "hit_at_10": matches.iter().take(10).any(|hit| *hit),
        "reciprocal_rank": round3(first_hit.map(|rank| 1.0 / rank as f64).unwrap_or(0.0)),
        "result_types": result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["type"].clone())
            .collect::<Vec<_>>(),
        "result_identifiers": result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["identifier"].clone())
            .collect::<Vec<_>>(),
    })
}

fn ranked_origin_matches(
    results: &[Value],
    source_map: &[(String, String)],
    expected_paths: &[String],
) -> Vec<bool> {
    results
        .iter()
        .map(|entry| {
            if entry["type"] != "source" {
                return false;
            }
            let identifier = entry["identifier"].as_str().unwrap();
            let Some((_, origin)) = source_map.iter().find(|(id, _)| id == identifier) else {
                return false;
            };
            expected_paths
                .iter()
                .any(|expected| origin.ends_with(expected))
        })
        .collect()
}

fn percentile_ms(measurements: &[QueryMeasurement], percentile: usize) -> f64 {
    let mut values = measurements
        .iter()
        .map(|entry| entry.elapsed_ms)
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.total_cmp(right));
    let last = values.len().saturating_sub(1);
    let index = (last * percentile).div_ceil(100);
    round3(values[index.min(last)])
}

fn rate(measurements: &[QueryMeasurement], cutoff: usize) -> f64 {
    let hits = measurements
        .iter()
        .filter(|entry| match cutoff {
            5 => entry.hit_at_5,
            10 => entry.hit_at_10,
            _ => false,
        })
        .count();
    round3(hits as f64 / measurements.len() as f64)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                total += dir_size(&entry_path);
            } else {
                total += file_size(&entry_path);
            }
        }
    }
    total
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
