use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

fn run(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc-book"));
    command.current_dir(cwd).env("HOME", home).args(args);
    command.output().unwrap()
}

fn ok(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = run(cwd, home, args);
    assert!(
        output.status.success(),
        "{args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn err(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = run(cwd, home, args);
    assert!(!output.status.success(), "{args:?}");
    serde_json::from_slice(&output.stderr).unwrap()
}

fn subject(cwd: &Path, home: &Path) -> String {
    let input = serde_json::json!({
        "name": "分布式系统",
        "request_id": "book-subject-distributed-systems"
    })
    .to_string();
    ok(cwd, home, &["subject", "create", "--json", &input])["result"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn import_book(cwd: &Path, home: &Path, subject_id: &str, path: &str, request: &str) -> Value {
    let input = serde_json::json!({
        "subject_id": subject_id,
        "path": path,
        "title": path,
        "request_id": request
    })
    .to_string();
    ok(cwd, home, &["import", "--json", &input])
}

#[test]
fn import_preserves_exact_bytes_deduplicates_content_and_keeps_explicit_editions() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let original = b"# Chapter 1\r\n\r\nExact original bytes.\r\n";
    fs::write(cwd.join("distributed-systems.txt"), original).unwrap();
    let hash = Sha256::digest(original)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let input = serde_json::json!({
        "subject_id": subject_id,
        "path": "distributed-systems.txt",
        "title": "Distributed Systems",
        "author": "Example Author",
        "request_id": "book-import-v1"
    })
    .to_string();
    let imported = ok(&cwd, &home, &["import", "--json", &input]);
    let book = &imported["result"]["book"];
    let book_id = book["id"].as_str().unwrap();
    assert_eq!(book["state"], "imported");
    assert_eq!(book["format"], "txt");
    assert_eq!(book["original_sha256"], hash);
    assert_eq!(book["original_bytes"], original.len());
    assert!(book["normalized_sha256"].is_null());
    assert_eq!(book["edition_of"], Value::Null);
    assert_eq!(imported["result"]["deduplicated"], false);
    let blob = home
        .join(".lwc/plugins/book/blobs/sha256")
        .join(&hash[..2])
        .join(&hash);
    assert_eq!(fs::read(blob).unwrap(), original);

    fs::write(cwd.join("same-copy.txt"), original).unwrap();
    let duplicate = serde_json::json!({
        "subject_id": subject_id,
        "path": "same-copy.txt",
        "title": "A conflicting display title cannot fork identity",
        "author": "Someone Else",
        "request_id": "book-import-same-bytes"
    })
    .to_string();
    let duplicate = ok(&cwd, &home, &["import", "--json", &duplicate]);
    assert_eq!(duplicate["result"]["book"]["id"], book_id);
    assert_eq!(duplicate["result"]["deduplicated"], true);
    assert_eq!(duplicate["result"]["book"]["title"], "Distributed Systems");

    fs::write(
        cwd.join("distributed-systems-v2.md"),
        b"# Chapter 1\nRevised.\n",
    )
    .unwrap();
    let edition = serde_json::json!({
        "subject_id": subject_id,
        "path": "distributed-systems-v2.md",
        "title": "Distributed Systems",
        "author": "Example Author",
        "edition_of": book_id,
        "request_id": "book-import-v2"
    })
    .to_string();
    let edition = ok(&cwd, &home, &["import", "--json", &edition]);
    assert_ne!(edition["result"]["book"]["id"], book_id);
    assert_eq!(edition["result"]["book"]["edition_of"], book_id);
    assert_eq!(edition["result"]["book"]["format"], "markdown");
}

#[test]
fn unsupported_formats_fail_before_book_or_blob_creation() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    for (name, bytes, request_id) in [
        ("book.html", b"<html>unsupported</html>".as_slice(), "html"),
        ("book.mobi", b"BOOKMOBI".as_slice(), "mobi"),
    ] {
        fs::write(cwd.join(name), bytes).unwrap();
        let input = serde_json::json!({
            "subject_id": subject_id,
            "path": name,
            "title": "Unsupported",
            "request_id": format!("book-import-{request_id}")
        })
        .to_string();
        let error = err(&cwd, &home, &["import", "--json", &input]);
        assert_eq!(error["error"]["code"], "unsupported_book_format");
        assert!(error["error"]["details"]["guidance"].is_string());
    }
    let status = ok(&cwd, &home, &["status"]);
    assert_eq!(status["result"]["books"], 0);
    assert!(!home.join(".lwc/plugins/book/blobs").exists());
}

#[test]
fn prepare_normalizes_indexes_and_proves_gap_free_order_without_advancing_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let source = "\u{feff}# 第一章\r\n\r\n一致性依赖多数派决定。\r\n## 小节\r日志必须持久化。\r\n";
    fs::write(cwd.join("consensus.md"), source.as_bytes()).unwrap();
    let imported = import_book(
        &cwd,
        &home,
        &subject_id,
        "consensus.md",
        "book-prepare-consensus-import",
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"book-prepare-consensus"}).to_string();
    let prepared = ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let book = &prepared["result"]["book"];
    assert_eq!(book["state"], "ready");
    assert_eq!(book["revision"], 2);
    assert!(book["block_count"].as_i64().unwrap() >= 1);
    assert_eq!(book["coverage"]["committed_blocks"], 0);
    assert_eq!(book["coverage"]["percent"], 0.0);

    let normalized = "# 第一章\n\n一致性依赖多数派决定。\n## 小节\n日志必须持久化。\n";
    let normalized_hash = Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(book["normalized_sha256"], normalized_hash);
    let normalized_blob = home
        .join(".lwc/plugins/book/blobs/sha256")
        .join(&normalized_hash[..2])
        .join(&normalized_hash);
    assert_eq!(fs::read(normalized_blob).unwrap(), normalized.as_bytes());

    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT ordinal,byte_start,byte_end,text,text_hash FROM book_blocks
             WHERE book_id=?1 ORDER BY ordinal",
        )
        .unwrap();
    let rows = statement
        .query_map([book_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut cursor = 0_i64;
    let mut rebuilt = String::new();
    for (expected, (ordinal, start, end, text, hash)) in rows.iter().enumerate() {
        assert_eq!(*ordinal, expected as i64);
        assert_eq!(*start, cursor);
        cursor = *end;
        assert_eq!(
            Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            *hash
        );
        rebuilt.push_str(text);
    }
    assert_eq!(cursor as usize, normalized.len());
    assert_eq!(rebuilt, normalized);

    let found = ok(&cwd, &home, &["search", book_id, "一致性", "--limit", "5"]);
    assert_eq!(found["result"]["hits"][0]["book_id"], book_id);
    assert_eq!(found["result"]["hits"][0]["query_only"], true);
    let peeked = ok(
        &cwd,
        &home,
        &["peek", book_id, "--start", "0", "--count", "1"],
    );
    assert_eq!(peeked["result"]["blocks"][0]["ordinal"], 0);
    assert_eq!(peeked["result"]["blocks"][0]["query_only"], true);
    let shown = ok(&cwd, &home, &["show", book_id]);
    assert_eq!(shown["result"]["book"]["coverage"]["committed_blocks"], 0);
}

#[test]
fn invalid_utf8_preparation_records_anomaly_and_never_becomes_ready() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(cwd.join("broken.txt"), [0xff, 0xfe, 0xfd]).unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "broken.txt", "book-broken-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"book-broken-prepare"}).to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &["prepare", book_id, "--if-revision", "1", "--json", &prepare,],
        )["error"]["code"],
        "invalid_book_text"
    );
    let shown = ok(&cwd, &home, &["show", book_id]);
    assert_eq!(shown["result"]["book"]["state"], "imported");
    assert_eq!(shown["result"]["book"]["anomaly_count"], 1);
}

#[test]
fn read_leases_repeat_exactly_and_only_valid_ordered_commits_advance_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let text = format!("# 大书\n{}\n", "顺序阅读内容。".repeat(12_000));
    fs::write(cwd.join("large.md"), text).unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "large.md", "book-read-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"book-read-prepare"}).to_string();
    let prepared = ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let total = prepared["result"]["book"]["block_count"].as_i64().unwrap();
    assert!(total >= 3);

    let next = serde_json::json!({
        "owner": "agent-local",
        "budget": {"unit":"utf8_bytes","value":33000},
        "request_id": "book-read-next-1"
    })
    .to_string();
    let first = ok(
        &cwd,
        &home,
        &[
            "read",
            "next",
            book_id,
            "--if-revision",
            "2",
            "--json",
            &next,
        ],
    );
    let lease = &first["result"]["lease"];
    let lease_id = lease["id"].as_str().unwrap();
    assert_eq!(lease["start_ordinal"], 0);
    assert_eq!(lease["end_ordinal"], 0);
    assert_eq!(lease["owner"], "agent-local");
    assert!(lease["text"].as_str().unwrap().len() <= 33_000);
    assert_eq!(lease["budget"]["requested_unit"], "utf8_bytes");
    assert_eq!(lease["budget"]["source_limit"], 33_000);
    assert_eq!(lease["coverage_before"]["committed_blocks"], 0);

    let repeat = serde_json::json!({
        "owner": "agent-local",
        "budget": {"unit":"utf8_bytes","value":99999},
        "request_id": "book-read-next-repeat"
    })
    .to_string();
    let repeated = ok(
        &cwd,
        &home,
        &[
            "read",
            "next",
            book_id,
            "--if-revision",
            "3",
            "--json",
            &repeat,
        ],
    );
    assert_eq!(repeated["result"]["lease"], *lease);
    assert_eq!(
        ok(&cwd, &home, &["show", book_id])["result"]["book"]["coverage"]["committed_blocks"],
        0
    );

    let report = serde_json::json!({
        "owner": "agent-other-machine",
        "range_hash": lease["range_hash"],
        "summary": "第一窗口摘要",
        "key_points": [{
            "text":"顺序阅读",
            "block_id": lease["blocks"][0]["id"],
            "source_hash": lease["blocks"][0]["text_hash"]
        }],
        "new_concepts": ["顺序覆盖"],
        "prior_links": [],
        "open_threads": ["后续窗口"],
        "anomalies": [],
        "request_id": "book-read-stale-owner"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "read",
                "commit",
                lease_id,
                "--if-revision",
                "1",
                "--json",
                &report,
            ],
        )["error"]["code"],
        "stale_owner"
    );

    let report = serde_json::json!({
        "owner": "agent-local",
        "range_hash": lease["range_hash"],
        "summary": "第一窗口摘要",
        "key_points": [{
            "text":"顺序阅读",
            "block_id": lease["blocks"][0]["id"],
            "source_hash": lease["blocks"][0]["text_hash"]
        }],
        "new_concepts": ["顺序覆盖"],
        "prior_links": [],
        "open_threads": ["后续窗口"],
        "anomalies": [],
        "request_id": "book-read-commit-1"
    })
    .to_string();
    let committed = ok(
        &cwd,
        &home,
        &[
            "read",
            "commit",
            lease_id,
            "--if-revision",
            "1",
            "--json",
            &report,
        ],
    );
    assert_eq!(committed["result"]["coverage"]["committed_blocks"], 1);
    assert_eq!(committed["result"]["lease"]["state"], "committed");
    assert_eq!(
        committed["result"]["lease"]["coverage_before"]["committed_blocks"],
        0
    );

    let next = serde_json::json!({
        "owner": "agent-local",
        "budget": {"unit":"utf8_bytes","value":33000},
        "request_id": "book-read-next-2"
    })
    .to_string();
    let second = ok(
        &cwd,
        &home,
        &[
            "read",
            "next",
            book_id,
            "--if-revision",
            "4",
            "--json",
            &next,
        ],
    );
    assert_eq!(second["result"]["lease"]["start_ordinal"], 1);
    assert_eq!(
        second["result"]["lease"]["coverage_before"]["committed_blocks"],
        1
    );
    let mut lease = second["result"]["lease"].clone();
    let mut expected_start = 1_i64;
    let mut sequence = 2_i64;
    loop {
        assert_eq!(lease["start_ordinal"], expected_start);
        let report = serde_json::json!({
            "owner":"agent-local","range_hash":lease["range_hash"],
            "summary":format!("第 {sequence} 个窗口摘要"),
            "key_points":[{"text":"顺序阅读","block_id":lease["blocks"][0]["id"],
              "source_hash":lease["blocks"][0]["text_hash"]}],
            "new_concepts":["顺序覆盖"],"prior_links":[],"open_threads":[],"anomalies":[],
            "request_id":format!("book-read-commit-{sequence}")
        })
        .to_string();
        let committed = ok(
            &cwd,
            &home,
            &[
                "read",
                "commit",
                lease["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--json",
                &report,
            ],
        );
        let coverage = &committed["result"]["coverage"];
        expected_start = lease["end_ordinal"].as_i64().unwrap() + 1;
        if coverage["committed_blocks"] == total {
            assert_eq!(coverage["percent"], 100.0);
            assert_eq!(
                ok(&cwd, &home, &["show", book_id])["result"]["book"]["state"],
                "covered"
            );
            break;
        }
        sequence += 1;
        let shown = ok(&cwd, &home, &["show", book_id]);
        let revision = shown["result"]["book"]["revision"]
            .as_i64()
            .unwrap()
            .to_string();
        let next = serde_json::json!({
            "owner":"agent-local","budget":{"unit":"utf8_bytes","value":33000},
            "request_id":format!("book-read-next-{sequence}")
        })
        .to_string();
        lease = ok(
            &cwd,
            &home,
            &[
                "read",
                "next",
                book_id,
                "--if-revision",
                &revision,
                "--json",
                &next,
            ],
        )["result"]["lease"]
            .clone();
    }
}

#[test]
fn synthesis_requires_full_coverage_complete_source_links_and_builds_only_private_wiki() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(cwd.join("small.md"), "# 核心章\n提交日志保证恢复。\n").unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "small.md", "synth-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"synth-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let premature = serde_json::json!({
        "summaries": [], "mainline": [], "relations": [], "request_id":"synth-premature"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "synthesis",
                "publish",
                book_id,
                "--if-revision",
                "2",
                "--json",
                &premature,
            ],
        )["error"]["code"],
        "book_not_covered"
    );
    let next = serde_json::json!({
        "owner":"agent-local","budget":{"unit":"utf8_bytes","value":65536},
        "request_id":"synth-next"
    })
    .to_string();
    let lease = ok(
        &cwd,
        &home,
        &[
            "read",
            "next",
            book_id,
            "--if-revision",
            "2",
            "--json",
            &next,
        ],
    )["result"]["lease"]
        .clone();
    let commit = serde_json::json!({
        "owner":"agent-local","range_hash":lease["range_hash"],
        "summary":"本章说明提交日志如何支持恢复。",
        "key_points":[{"text":"提交日志支持恢复","block_id":lease["blocks"][0]["id"],
          "source_hash":lease["blocks"][0]["text_hash"]}],
        "new_concepts":["提交日志"],"prior_links":[],"open_threads":[],"anomalies":[],
        "request_id":"synth-commit"
    })
    .to_string();
    let committed = ok(
        &cwd,
        &home,
        &[
            "read",
            "commit",
            lease["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &commit,
        ],
    );
    assert_eq!(committed["result"]["coverage"]["percent"], 100.0);
    let source_hash = lease["blocks"][0]["text_hash"].clone();
    let complete = serde_json::json!({
        "summaries":[
          {"level":"chapter","title":"核心章","summary":"提交日志保证恢复。",
           "start_ordinal":0,"end_ordinal":0,"source_hashes":[source_hash.clone()]},
          {"level":"book","title":"全书总结","summary":"全书围绕可靠恢复展开。",
           "start_ordinal":0,"end_ordinal":0,"source_hashes":[source_hash.clone()]}
        ],
        "mainline":[{"text":"提交日志 -> 可恢复状态","start_ordinal":0,"end_ordinal":0,
          "source_hashes":[source_hash.clone()]}],
        "relations":[{"from":"提交日志","to":"可恢复状态","kind":"explicit",
          "confidence":1.0,"source_hashes":[source_hash]}],
        "request_id":"synth-publish"
    })
    .to_string();
    let synthesized = ok(
        &cwd,
        &home,
        &[
            "synthesis",
            "publish",
            book_id,
            "--if-revision",
            "4",
            "--json",
            &complete,
        ],
    );
    assert_eq!(synthesized["result"]["book"]["state"], "synthesized");
    let private = home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"));
    let wiki = fs::read_to_string(private).unwrap();
    assert!(wiki.contains("全书围绕可靠恢复展开"));
    assert!(!home.join(".lwc/wiki.db").exists());
    fs::remove_file(home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"))).unwrap();
    ok(&cwd, &home, &["show", book_id]);
    assert!(
        home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"))
            .is_file()
    );
}
