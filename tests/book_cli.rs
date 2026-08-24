use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::Path, process::Command};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn run(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc-book"));
    command.current_dir(cwd).env("HOME", home).args(args);
    let trans_config = home.join("book-trans-config.json");
    if trans_config.is_file() {
        command.env(
            "LWC_BOOK_TRANS_CONFIG",
            fs::read_to_string(trans_config).unwrap(),
        );
        command.env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        );
    }
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
fn existing_book_schema_corruption_fails_closed_instead_of_being_repaired() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    ok(&cwd, &home, &["status"]);
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    connection
        .execute("DROP INDEX book_blocks_order", [])
        .unwrap();
    drop(connection);

    let error = err(&cwd, &home, &["status"]);
    assert_eq!(error["error"]["code"], "corrupt_store");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("book_blocks_order")
    );
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name='book_blocks_order'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
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
            "SELECT ordinal,byte_start,byte_end,text_hash FROM book_blocks
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
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut cursor = 0_i64;
    let mut rebuilt = String::new();
    for (expected, (ordinal, start, end, hash)) in rows.iter().enumerate() {
        assert_eq!(*ordinal, expected as i64);
        assert_eq!(*start, cursor);
        cursor = *end;
        assert_eq!(
            Sha256::digest(&normalized.as_bytes()[*start as usize..*end as usize])
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            *hash
        );
        rebuilt.push_str(&normalized[*start as usize..*end as usize]);
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
fn prepared_blocks_keep_large_text_only_in_the_content_addressed_blob() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let paragraph = "正文只在 blob 中。".repeat(2048);
    let source = format!(
        "# 大型样本\n\n{}",
        (0..256)
            .map(|_| format!("{paragraph}\n\n"))
            .collect::<String>()
    );
    assert!(source.len() >= 8 * 1024 * 1024);
    fs::write(cwd.join("blob-only.md"), source.as_bytes()).unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "blob-only.md", "blob-only-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"blob-only-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let database = home.join(".lwc/plugins/book/data.sqlite3");
    let connection = Connection::open(&database).unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(book_blocks)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(!columns.iter().any(|column| column == "text"));
    let fts_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='book_blocks_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(fts_sql.contains("content=''"));
    connection
        .execute(
            "INSERT INTO book_blocks_fts(book_blocks_fts) VALUES('delete-all')",
            [],
        )
        .unwrap();
    drop(connection);
    let database_bytes = fs::metadata(&database).unwrap().len()
        + fs::metadata(database.with_extension("sqlite3-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    assert!(
        database_bytes < source.len() as u64 * 2,
        "database+WAL {database_bytes} duplicated {} source bytes",
        source.len()
    );
    let hit = ok(
        &cwd,
        &home,
        &["search", book_id, "正文只在", "--limit", "1"],
    );
    assert!(
        hit["result"]["hits"][0]["text"]
            .as_str()
            .unwrap()
            .contains("正文只在 blob 中")
    );
}

#[test]
#[ignore = "capacity acceptance: creates and prepares a real 256 MiB source"]
fn prepares_256_mib_without_canonical_full_text_duplication() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let source_path = cwd.join("capacity.md");
    let mut source = fs::File::create(&source_path).unwrap();
    let chunk = "容量样本正文只存 blob。".repeat(4096);
    while source.metadata().unwrap().len() < 256 * 1024 * 1024 {
        writeln!(source, "{chunk}\n").unwrap();
    }
    source.sync_all().unwrap();
    let source_bytes = source.metadata().unwrap().len();
    drop(source);
    let imported = import_book(&cwd, &home, &subject_id, "capacity.md", "capacity-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"capacity-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let database = home.join(".lwc/plugins/book/data.sqlite3");
    let database_bytes = fs::metadata(&database).unwrap().len()
        + fs::metadata(database.with_extension("sqlite3-wal"))
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    assert!(database_bytes < source_bytes * 2);
}

#[test]
#[ignore = "capacity acceptance: builds and reconstructs a 100k-row derived FTS index"]
fn reconstructs_100k_block_fts_without_loading_all_text() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let mut source = fs::File::create(cwd.join("many-blocks.md")).unwrap();
    writeln!(source, "# Scale\n").unwrap();
    for ordinal in 0..100_000 {
        writeln!(source, "block-{ordinal} searchable-marker\n").unwrap();
    }
    source.sync_all().unwrap();
    drop(source);
    let imported = import_book(
        &cwd,
        &home,
        &subject_id,
        "many-blocks.md",
        "many-blocks-import",
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"many-blocks-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let database = home.join(".lwc/plugins/book/data.sqlite3");
    let connection = Connection::open(database).unwrap();
    let block_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM book_blocks WHERE book_id=?1",
            [book_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(block_count >= 100_000);
    connection
        .execute(
            "INSERT INTO book_blocks_fts(book_blocks_fts) VALUES('delete-all')",
            [],
        )
        .unwrap();
    drop(connection);
    let hit = ok(
        &cwd,
        &home,
        &["search", book_id, "block-99999", "--limit", "1"],
    );
    assert_eq!(hit["result"]["hits"][0]["ordinal"], block_count - 1);
}

#[test]
fn deduplicated_import_rejects_same_size_corruption_of_an_existing_blob() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let source = "# Hash\n\nSame sized corruption must not pass.\n";
    fs::write(cwd.join("hash.md"), source).unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "hash.md", "hash-import");
    let book = &imported["result"]["book"];
    let book_id = book["id"].as_str().unwrap();
    let hash = book["original_sha256"].as_str().unwrap();
    let blob = home
        .join(".lwc/plugins/book/blobs/sha256")
        .join(&hash[..2])
        .join(hash);
    fs::write(&blob, vec![b'x'; source.len()]).unwrap();
    let duplicate = serde_json::json!({
        "subject_id":subject_id,"title":"Hash duplicate","path":"hash.md",
        "request_id":"hash-import-duplicate"
    })
    .to_string();
    assert_eq!(
        err(&cwd, &home, &["import", "--json", &duplicate],)["error"]["code"],
        "corrupt_blob"
    );
    let connection = Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    let state: String = connection
        .query_row("SELECT state FROM books WHERE id=?1", [book_id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(state, "imported");
}

#[test]
fn oversized_paragraph_keeps_one_logical_block_with_stable_sublocators() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let paragraph = "连续段落。".repeat(12_000);
    fs::write(cwd.join("oversized.md"), format!("# 大章\n\n{paragraph}\n")).unwrap();
    let imported = import_book(
        &cwd,
        &home,
        &subject_id,
        "oversized.md",
        "book-oversized-import",
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"book-oversized-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );

    let peeked = ok(
        &cwd,
        &home,
        &["peek", book_id, "--start", "0", "--count", "10"],
    );
    let blocks = peeked["result"]["blocks"].as_array().unwrap();
    let paragraph_parts = blocks
        .iter()
        .filter(|block| block["text"].as_str().unwrap().contains("连续段落"))
        .collect::<Vec<_>>();
    assert!(paragraph_parts.len() >= 2);
    let logical_id = paragraph_parts[0]["locator"]["logical_block_id"]
        .as_str()
        .unwrap();
    for (subordinal, block) in paragraph_parts.iter().enumerate() {
        assert_eq!(block["locator"]["book_id"], book_id);
        assert_eq!(block["locator"]["logical_block_id"], logical_id);
        assert_eq!(block["locator"]["subordinal"], subordinal as i64);
        assert_eq!(block["locator"]["source_hash"], block["text_hash"]);
    }
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

#[cfg(unix)]
#[test]
fn epub_and_text_pdf_use_the_shared_configured_converter_without_nested_lwc() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    let bin = home.join("bin");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let adapter = bin.join("markitdown");
    fs::write(
        &adapter,
        "#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf '# Converted\\n\\nOrdered text from adapter.\\n' > \"$2\"\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();
    fs::write(
        home.join("book-trans-config.json"),
        serde_json::json!({
            "engine":"markitdown", "args":[], "timeout_seconds":5
        })
        .to_string(),
    )
    .unwrap();
    let subject_id = subject(&cwd, &home);

    for (name, original, request) in [
        (
            "book.epub",
            &[0x50, 0x4b, 0x03, 0x04, b'e', b'p', b'u', b'b'][..],
            "epub",
        ),
        ("book.pdf", b"%PDF-1.7 synthetic text pdf".as_slice(), "pdf"),
    ] {
        fs::write(cwd.join(name), original).unwrap();
        let imported = import_book(
            &cwd,
            &home,
            &subject_id,
            name,
            &format!("converter-{request}-import"),
        );
        let book_id = imported["result"]["book"]["id"].as_str().unwrap();
        let prepare = serde_json::json!({
            "request_id":format!("converter-{request}-prepare")
        })
        .to_string();
        let prepared = ok(
            &cwd,
            &home,
            &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
        );
        assert_eq!(prepared["result"]["book"]["state"], "ready");
        assert_eq!(prepared["result"]["book"]["converter"], "markitdown");
        assert_eq!(prepared["result"]["book"]["original_bytes"], original.len());
        assert_eq!(
            ok(&cwd, &home, &["peek", book_id, "--start", "0"])["result"]["blocks"][0]["text"],
            "# Converted\n\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn converter_failures_are_bounded_fail_closed_and_leave_no_stderr_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    let bin = home.join("bin");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(cwd.join("broken.pdf"), b"%PDF-1.7 synthetic").unwrap();
    let imported = import_book(
        &cwd,
        &home,
        &subject_id,
        "broken.pdf",
        "converter-failure-import",
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();

    let prepare = |request: &str| serde_json::json!({"request_id":request}).to_string();
    fs::write(
        home.join("book-trans-config.json"),
        r#"{"engine":"disabled","args":[],"timeout_seconds":1}"#,
    )
    .unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-disabled"),
            ],
        )["error"]["code"],
        "trans_disabled"
    );

    fs::write(
        home.join("book-trans-config.json"),
        r#"{"engine":"markitdown","args":["--output","escape"],"timeout_seconds":1}"#,
    )
    .unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-malicious-args"),
            ],
        )["error"]["code"],
        "trans_unsafe_args"
    );
    assert!(!cwd.join("escape").exists());

    fs::write(
        home.join("book-trans-config.json"),
        r#"{"engine":"markitdown","args":[],"timeout_seconds":1}"#,
    )
    .unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-missing"),
            ],
        )["error"]["code"],
        "trans_executable_missing"
    );
    let revision_after_first_missing = ok(&cwd, &home, &["status"])["result"]["store"]["revision"]
        .as_i64()
        .unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-missing-repeat"),
            ],
        )["error"]["code"],
        "trans_executable_missing"
    );
    assert_eq!(
        ok(&cwd, &home, &["status"])["result"]["store"]["revision"],
        revision_after_first_missing
    );
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM book_anomalies
                 WHERE book_id=?1 AND kind='converter_error' AND details='trans_executable_missing'",
                [book_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    drop(connection);
    assert_eq!(
        fs::read_dir(home.join(".lwc/plugins/book"))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("stderr"))
            .count(),
        0
    );

    let adapter = bin.join("markitdown");
    fs::write(&adapter, "#!/bin/sh\nsleep 5\n").unwrap();
    let mut permissions = fs::metadata(&adapter).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&adapter, permissions).unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-timeout"),
            ],
        )["error"]["code"],
        "trans_timeout"
    );

    fs::write(
        &adapter,
        "#!/bin/sh\nwhile [ \"$1\" != \"-o\" ]; do shift; done\n: > \"$2\"\n",
    )
    .unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "prepare",
                book_id,
                "--if-revision",
                "1",
                "--json",
                &prepare("converter-empty"),
            ],
        )["error"]["code"],
        "trans_empty_output"
    );
    let shown = ok(&cwd, &home, &["show", book_id]);
    assert_eq!(shown["result"]["book"]["state"], "imported");
    assert!(shown["result"]["book"]["anomaly_count"].as_i64().unwrap() >= 5);
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
fn lease_expiry_and_renewal_never_advance_or_change_the_source_range() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(
        cwd.join("lease.md"),
        "# Lease\n\nExpiry never advances coverage.\n",
    )
    .unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "lease.md", "lease-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"lease-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let next = serde_json::json!({
        "owner":"agent-a", "lease_seconds":60, "request_id":"lease-next"
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
    assert!(lease["expires_at"].is_string());
    let revision_before_repeat = ok(&cwd, &home, &["status"])["result"]["store"]["revision"]
        .as_i64()
        .unwrap();
    let repeated_next = serde_json::json!({
        "owner":"agent-a", "lease_seconds":60, "request_id":"lease-next-repeat"
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
            &repeated_next,
        ],
    )["result"]["lease"]
        .clone();
    assert_eq!(repeated["id"], lease["id"]);
    assert_eq!(
        ok(&cwd, &home, &["status"])["result"]["store"]["revision"],
        revision_before_repeat
    );

    let renew = serde_json::json!({
        "owner":"agent-a", "lease_seconds":120, "request_id":"lease-renew"
    })
    .to_string();
    let renewed = ok(
        &cwd,
        &home,
        &[
            "read",
            "renew",
            lease["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &renew,
        ],
    )["result"]["lease"]
        .clone();
    assert_eq!(renewed["revision"], 2);
    assert_eq!(renewed["range_hash"], lease["range_hash"]);
    assert_eq!(renewed["blocks"], lease["blocks"]);

    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE book_leases SET expires_at='2000-01-01T00:00:00.000Z' WHERE id=?1",
            [lease["id"].as_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE plugin_meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
            [],
        )
        .unwrap();
    let report = serde_json::json!({
        "owner":"agent-a", "range_hash":lease["range_hash"], "summary":"expired",
        "key_points":[{"text":"expired","block_id":lease["blocks"][0]["id"],
          "source_hash":lease["blocks"][0]["text_hash"]}],
        "new_concepts":[], "prior_links":[], "open_threads":[], "anomalies":[],
        "request_id":"lease-expired-commit"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "read",
                "commit",
                lease["id"].as_str().unwrap(),
                "--if-revision",
                "2",
                "--json",
                &report,
            ],
        )["error"]["code"],
        "lease_expired"
    );
    let shown = ok(&cwd, &home, &["show", book_id]);
    assert_eq!(shown["result"]["book"]["coverage"]["committed_blocks"], 0);
}

#[test]
fn renewed_lease_commit_increments_revision_with_exact_cas() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(
        cwd.join("renew-commit.md"),
        "# Renew\n\nCommit after renew.\n",
    )
    .unwrap();
    let imported = import_book(
        &cwd,
        &home,
        &subject_id,
        "renew-commit.md",
        "renew-commit-import",
    );
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"renew-commit-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let next = serde_json::json!({"owner":"agent","request_id":"renew-commit-next"}).to_string();
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
    let renew = serde_json::json!({
        "owner":"agent","lease_seconds":120,"request_id":"renew-commit-renew"
    })
    .to_string();
    let renewed = ok(
        &cwd,
        &home,
        &[
            "read",
            "renew",
            lease["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &renew,
        ],
    )["result"]["lease"]
        .clone();
    let report = serde_json::json!({
        "owner":"agent","range_hash":renewed["range_hash"],"summary":"renewed",
        "key_points":[{"text":"renewed","block_id":renewed["blocks"][0]["id"],
          "source_hash":renewed["blocks"][0]["text_hash"]}],
        "new_concepts":[],"prior_links":[],"open_threads":[],"anomalies":[],
        "request_id":"renew-commit-report"
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
            "2",
            "--json",
            &report,
        ],
    );
    assert_eq!(committed["result"]["lease"]["revision"], 3);
    assert_eq!(committed["result"]["lease"]["state"], "committed");
}

#[test]
fn lease_takeover_requires_a_latest_sync_receipt_and_rejects_the_stale_owner() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(cwd.join("takeover.md"), "# Takeover\n\nOne exact range.\n").unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "takeover.md", "takeover-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"takeover-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let next = serde_json::json!({
        "owner":"agent-old", "request_id":"takeover-next"
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

    let takeover = serde_json::json!({
        "entity_id":lease["id"], "old_owner":"agent-old", "new_owner":"agent-new",
        "if_revision":1, "sync_session_id":"sync-book-takeover",
        "request_id":"takeover-owner-change"
    })
    .to_string();
    assert_eq!(
        err(&cwd, &home, &["read", "takeover", "--json", &takeover])["error"]["code"],
        "sync_receipt_missing"
    );
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
    let anomaly_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO book_anomalies(id,book_id,kind,details,status,created_at)
             VALUES(?1,?2,'hierarchy_gap','fixture','open','2026-08-24T00:00:00.000Z')",
            [anomaly_id, book_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE plugin_meta SET value=CAST(value AS INTEGER)+1 WHERE key='revision'",
            [],
        )
        .unwrap();
    drop(connection);
    let source_hash = lease["blocks"][0]["text_hash"].clone();
    let complete = serde_json::json!({
        "anomaly_dispositions":[{"id":anomaly_id,"status":"accepted"}],
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
    let private_books = home.join(".lwc/plugins/book/wiki/books");
    fs::create_dir_all(&private_books).unwrap();
    let blocked_projection = private_books.join(format!("{book_id}.md"));
    let symlink_target = home.join("wiki-target");
    fs::write(&symlink_target, "must remain unchanged").unwrap();
    std::os::unix::fs::symlink(&symlink_target, &blocked_projection).unwrap();
    let pending = err(
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
    assert_eq!(pending["error"]["code"], "book_materialization_pending");
    assert_eq!(pending["error"]["details"]["committed"], true);
    assert_eq!(pending["error"]["details"]["retryable"], true);
    let committed_state: String = Connection::open(home.join(".lwc/plugins/book/data.sqlite3"))
        .unwrap()
        .query_row("SELECT state FROM books WHERE id=?1", [book_id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(committed_state, "synthesized");
    assert_eq!(
        fs::read_to_string(&symlink_target).unwrap(),
        "must remain unchanged"
    );
    fs::remove_file(&blocked_projection).unwrap();
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
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM book_anomalies WHERE id=?1",
                [anomaly_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "accepted"
    );
    drop(connection);
    let private = home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"));
    let wiki = fs::read_to_string(&private).unwrap();
    assert!(wiki.contains("全书围绕可靠恢复展开"));
    assert!(!home.join(".lwc/wiki.db").exists());
    fs::remove_file(home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"))).unwrap();
    ok(&cwd, &home, &["show", book_id]);
    assert!(
        home.join(format!(".lwc/plugins/book/wiki/books/{book_id}.md"))
            .is_file()
    );

    let correction = serde_json::json!({
        "supersedes_revision":1,
        "summaries":[
          {"level":"chapter","title":"核心章","summary":"修正后的章节摘要。",
           "start_ordinal":0,"end_ordinal":0,"source_hashes":[source_hash.clone()]},
          {"level":"book","title":"全书总结","summary":"修正后的全书摘要。",
           "start_ordinal":0,"end_ordinal":0,"source_hashes":[source_hash.clone()]}
        ],
        "mainline":[{"text":"提交日志 -> 可恢复状态","start_ordinal":0,"end_ordinal":0,
          "source_hashes":[source_hash.clone()]}],
        "relations":[{"from":"提交日志","to":"可恢复状态","kind":"explicit",
          "confidence":1.0,"source_hashes":[source_hash]}],
        "request_id":"synth-correction"
    })
    .to_string();
    let corrected = ok(
        &cwd,
        &home,
        &[
            "synthesis",
            "publish",
            book_id,
            "--if-revision",
            "5",
            "--json",
            &correction,
        ],
    );
    assert_eq!(corrected["result"]["synthesis_revision"], 2);
    assert_eq!(corrected["result"]["supersedes_revision"], 1);
    let connection =
        rusqlite::Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(DISTINCT synthesis_revision) FROM book_relations WHERE book_id=?1",
                [book_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
    assert!(
        fs::read_to_string(private)
            .unwrap()
            .contains("修正后的全书摘要")
    );
}

#[test]
fn synthesis_requires_every_detected_chapter_span() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(
        cwd.join("chapters.md"),
        "# 第一章\n\n甲。\n\n# 第二章\n\n乙。\n",
    )
    .unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "chapters.md", "chapters-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"chapters-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let next = serde_json::json!({"owner":"agent","request_id":"chapters-next"}).to_string();
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
    let blocks = lease["blocks"].as_array().unwrap();
    let report = serde_json::json!({
        "owner":"agent","range_hash":lease["range_hash"],"summary":"全书窗口",
        "key_points":[{"text":"甲乙","block_id":blocks[0]["id"],
          "source_hash":blocks[0]["text_hash"]}],
        "new_concepts":[],"prior_links":[],"open_threads":[],"anomalies":[],
        "request_id":"chapters-commit"
    })
    .to_string();
    ok(
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
    let hashes = blocks
        .iter()
        .map(|block| block["text_hash"].clone())
        .collect::<Vec<_>>();
    let first_end = blocks
        .iter()
        .rposition(|block| block["heading_path"][0] == "第一章")
        .unwrap();
    let incomplete = serde_json::json!({
        "summaries":[
          {"level":"chapter","title":"第一章","summary":"甲。","start_ordinal":0,
           "end_ordinal":first_end,"source_hashes":hashes[..=first_end]},
          {"level":"book","title":"全书","summary":"甲乙。","start_ordinal":0,
           "end_ordinal":blocks.len()-1,"source_hashes":hashes}
        ],
        "mainline":[{"text":"甲到乙","start_ordinal":0,"end_ordinal":blocks.len()-1,
          "source_hashes":hashes}],
        "relations":[{"from":"甲","to":"乙","kind":"explicit","confidence":1.0,
          "source_hashes":[hashes[0].clone()]}],
        "request_id":"chapters-incomplete"
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
                "4",
                "--json",
                &incomplete,
            ],
        )["error"]["code"],
        "incomplete_synthesis"
    );
}

#[test]
fn synthesis_maps_real_part_chapter_section_depths_and_requires_full_mainline() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    fs::write(
        cwd.join("hierarchy.md"),
        "# 第一部\n\n部引言。\n\n## 第一章\n\n章引言。\n\n### 第一节\n\n小节正文。\n",
    )
    .unwrap();
    let imported = import_book(&cwd, &home, &subject_id, "hierarchy.md", "hierarchy-import");
    let book_id = imported["result"]["book"]["id"].as_str().unwrap();
    let prepare = serde_json::json!({"request_id":"hierarchy-prepare"}).to_string();
    ok(
        &cwd,
        &home,
        &["prepare", book_id, "--if-revision", "1", "--json", &prepare],
    );
    let next = serde_json::json!({"owner":"agent","request_id":"hierarchy-next"}).to_string();
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
    let blocks = lease["blocks"].as_array().unwrap();
    let report = serde_json::json!({
        "owner":"agent","range_hash":lease["range_hash"],"summary":"完整层级",
        "key_points":[{"text":"层级","block_id":blocks[0]["id"],
          "source_hash":blocks[0]["text_hash"]}],
        "new_concepts":[],"prior_links":[],"open_threads":[],"anomalies":[],
        "request_id":"hierarchy-commit"
    })
    .to_string();
    ok(
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
    let hashes = blocks
        .iter()
        .map(|block| block["text_hash"].clone())
        .collect::<Vec<_>>();
    let span = |depth: usize| {
        let start = blocks
            .iter()
            .position(|block| block["heading_path"].as_array().unwrap().len() >= depth)
            .unwrap();
        (start, blocks.len() - 1)
    };
    let (chapter_start, chapter_end) = span(2);
    let (section_start, section_end) = span(3);
    let summaries = serde_json::json!([
        {"level":"part","title":"第一部","summary":"部摘要。","start_ordinal":0,
         "end_ordinal":blocks.len()-1,"source_hashes":hashes},
        {"level":"chapter","title":"第一部 / 第一章","summary":"章摘要。",
         "start_ordinal":chapter_start,"end_ordinal":chapter_end,
         "source_hashes":hashes[chapter_start..=chapter_end]},
        {"level":"section","title":"第一部 / 第一章 / 第一节","summary":"节摘要。",
         "start_ordinal":section_start,"end_ordinal":section_end,
         "source_hashes":hashes[section_start..=section_end]},
        {"level":"book","title":"全书","summary":"全书摘要。","start_ordinal":0,
         "end_ordinal":blocks.len()-1,"source_hashes":hashes}
    ]);
    let partial = serde_json::json!({
        "summaries":summaries,
        "mainline":[{"text":"只有开头","start_ordinal":0,"end_ordinal":0,
          "source_hashes":[hashes[0].clone()]}],
        "relations":[{"from":"部","to":"节","kind":"explicit","confidence":1.0,
          "source_hashes":[hashes[0].clone()]}],
        "request_id":"hierarchy-partial"
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
                "4",
                "--json",
                &partial
            ],
        )["error"]["code"],
        "incomplete_synthesis"
    );
    let complete = serde_json::json!({
        "summaries":summaries,
        "mainline":[{"text":"部到节","start_ordinal":0,"end_ordinal":blocks.len()-1,
          "source_hashes":hashes}],
        "relations":[{"from":"部","to":"节","kind":"explicit","confidence":1.0,
          "source_hashes":[hashes[0].clone()]}],
        "request_id":"hierarchy-complete"
    })
    .to_string();
    ok(
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
    let connection = Connection::open(home.join(".lwc/plugins/book/data.sqlite3")).unwrap();
    let levels = connection
        .prepare("SELECT level FROM book_summaries WHERE book_id=?1 ORDER BY level")
        .unwrap()
        .query_map([book_id], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(levels, vec!["book", "chapter", "part", "section"]);
}
