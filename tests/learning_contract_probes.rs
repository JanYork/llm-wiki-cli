use rusqlite::{Connection, DatabaseName};
use std::{
    fs::{self, File},
    io::Write,
    time::Instant,
};

const CORPUS_BYTES: usize = 256 * 1024 * 1024;
const CHUNK_BYTES: usize = 1024 * 1024;

#[test]
#[ignore = "TASK-001 capacity evidence; run explicitly on Pro"]
fn external_book_blobs_avoid_putting_canonical_content_in_the_sqlite_wal() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.bin");
    let mut file = File::create(&source).unwrap();
    let chunk = (0..CHUNK_BYTES)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    for _ in 0..(CORPUS_BYTES / CHUNK_BYTES) {
        file.write_all(&chunk).unwrap();
    }
    file.sync_all().unwrap();

    let external = temp.path().join("external.bin");
    let external_started = Instant::now();
    fs::copy(&source, &external).unwrap();
    File::open(&external).unwrap().sync_all().unwrap();
    let external_elapsed = external_started.elapsed();

    let database = temp.path().join("book.sqlite3");
    let mut connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA wal_autocheckpoint=0;
             CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB NOT NULL);",
        )
        .unwrap();
    let sqlite_started = Instant::now();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO blobs(data) VALUES (zeroblob(?1))",
            [CORPUS_BYTES as i64],
        )
        .unwrap();
    let rowid = transaction.last_insert_rowid();
    {
        let mut blob = transaction
            .blob_open(DatabaseName::Main, "blobs", "data", rowid, false)
            .unwrap();
        for _ in 0..(CORPUS_BYTES / CHUNK_BYTES) {
            blob.write_all(&chunk).unwrap();
        }
    }
    transaction.commit().unwrap();
    let sqlite_elapsed = sqlite_started.elapsed();
    let wal_bytes = fs::metadata(database.with_extension("sqlite3-wal"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    eprintln!(
        "book_blob_probe corpus_bytes={CORPUS_BYTES} external_ms={} sqlite_ms={} sqlite_wal_bytes={wal_bytes}",
        external_elapsed.as_millis(),
        sqlite_elapsed.as_millis(),
    );
    assert_eq!(fs::metadata(external).unwrap().len(), CORPUS_BYTES as u64);
    assert_eq!(
        connection
            .query_row("SELECT length(data) FROM blobs", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        CORPUS_BYTES as i64
    );
    assert!(wal_bytes >= CORPUS_BYTES as u64);
}
