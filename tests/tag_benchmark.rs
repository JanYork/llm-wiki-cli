use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command, time::Instant};

#[test]
#[ignore = "performance evidence; creates 10k pages and 100k tag assignments"]
fn tag_load_uses_covering_identity_index_and_meets_p95_budget() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    let binary = std::env::var_os("LWC_BENCH_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_lwc").into());
    let init = Command::new(&binary)
        .current_dir(&project)
        .env("HOME", &home)
        .arg("init")
        .output()
        .unwrap();
    assert!(init.status.success());
    let init: Value = serde_json::from_slice(&init.stdout).unwrap();
    let database = init["database"].as_str().unwrap();

    let mut conn = Connection::open(database).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
    let tx = conn.transaction().unwrap();
    for tag in 0..10 {
        tx.execute(
            "INSERT INTO tags(name, reason, updated_at) VALUES (?1, 'benchmark', 'now')",
            [format!("tag-{tag}")],
        )
        .unwrap();
    }
    for page in 0..10_000 {
        let slug = format!("page-{page:05}");
        tx.execute(
            "INSERT INTO pages(
                slug, title, body, structural_navigation, created_at, updated_at
             ) VALUES (?1, ?1, ?2, 0, 'now', 'now')",
            params![&slug, format!("complete body {page}")],
        )
        .unwrap();
        for tag in 0..10 {
            tx.execute(
                "INSERT INTO page_tags(
                    tag_name, page_slug, priority, reason, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'benchmark', 'now', 'now')",
                params![format!("tag-{tag}"), &slug, page % 100],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();

    let plan = {
        let mut statement = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT page_slug, priority, reason
                 FROM page_tags INDEXED BY page_tags_lookup
                 WHERE tag_name = 'tag-0'
                 ORDER BY priority DESC, page_slug ASC LIMIT 11",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n")
    };
    assert!(plan.contains("page_tags_lookup"), "{plan}");
    assert!(!plan.contains("SCAN pages"), "{plan}");
    drop(conn);

    let mut samples = Vec::new();
    for _ in 0..30 {
        let started = Instant::now();
        let output = Command::new(&binary)
            .current_dir(&project)
            .env("HOME", &home)
            .args(["load", "tag", "tag-0", "--limit", "10"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let loaded: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(loaded["returned"], 10);
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    let p95_ms = samples[28];
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "pages": 10_000,
            "assignments": 100_000,
            "samples": 30,
            "p95_ms": p95_ms,
            "limit": 10,
            "query_plan": plan,
        }))
        .unwrap()
    );
    assert!(p95_ms <= 250.0, "tag load p95 was {p95_ms:.3}ms");
}
