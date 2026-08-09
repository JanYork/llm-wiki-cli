use serde_json::Value;
use std::{fs, path::Path, process::Command, time::Instant};
use tempfile::tempdir;

fn run(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lwc"))
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "performance evidence; run explicitly in release mode"]
fn external_graph_rebuild_and_update_are_document_granular() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    let home = temp.path().join("home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&home).unwrap();
    run(&project, &home, &["init"]);
    for index in 0..100 {
        let path = project.join(format!("page-{index}.md"));
        fs::write(&path, format!("Document {index}. [[page-0]]")).unwrap();
        run(
            &project,
            &home,
            &[
                "page",
                "put",
                &format!("page-{index}"),
                "--title",
                &format!("Page {index}"),
                "--file",
                path.to_str().unwrap(),
                "--provenance",
                "agent-observed",
            ],
        );
    }

    for engine in ["grafeo", "surrealdb"] {
        let started = Instant::now();
        let enabled = run(&project, &home, &["config", "set", "--graph", engine]);
        let work = enabled["work"]["id"].as_str().unwrap();
        let rebuilt = run(&project, &home, &["work", "watch", work]);
        assert_eq!(rebuilt["work"]["completed"], 100);
        assert_eq!(rebuilt["work"]["total"], 100);

        let path = project.join(format!("{engine}-update.md"));
        fs::write(&path, format!("Updated through {engine}.")).unwrap();
        let updated = run(
            &project,
            &home,
            &[
                "page",
                "put",
                "page-99",
                "--title",
                "Page 99",
                "--file",
                path.to_str().unwrap(),
                "--provenance",
                "agent-observed",
            ],
        );
        let work = updated["graph"]["work"]["id"].as_str().unwrap();
        let projected = run(&project, &home, &["work", "watch", work]);
        assert_eq!(projected["work"]["completed"], 1);
        assert_eq!(projected["work"]["total"], 1);

        eprintln!(
            "{engine}: rebuild_ms={} update_ms={}",
            started.elapsed().as_millis(),
            updated["graph"]["queue_duration_ms"]
        );
    }
}
