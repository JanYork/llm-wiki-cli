use rusqlite::{Connection, params};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn load_graphqlite(conn: &Connection, extension: &Path) -> rusqlite::Result<()> {
    unsafe {
        conn.load_extension_enable()?;
        let result = conn.load_extension(extension, Some("sqlite3_graphqlite_init"));
        conn.load_extension_disable()?;
        result?;
    }
    let status: String = conn.query_row("SELECT graphqlite_test()", [], |row| row.get(0))?;
    assert!(status.to_lowercase().contains("successfully"), "{status}");
    Ok(())
}

fn cypher(conn: &Connection, query: &str, parameters: Value) -> rusqlite::Result<Value> {
    let raw: Option<String> = conn.query_row(
        "SELECT cypher(?1, ?2)",
        params![query, parameters.to_string()],
        |row| row.get(0),
    )?;
    Ok(raw.map_or(Value::Null, |value| {
        serde_json::from_str(&value).unwrap_or(Value::String(value))
    }))
}

#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64")
))]
#[test]
fn pinned_graphqlite_extension_persists_unicode_parameterized_graph_data() {
    let filename = if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "graphqlite-macos-x86_64.dylib"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "graphqlite-macos-aarch64.dylib"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "graphqlite-linux-x86_64.so"
    } else {
        "graphqlite-linux-aarch64.so"
    };
    let extension = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/graphqlite/0.6.0")
        .join(filename);
    let directory = tempdir().unwrap();
    let database = directory.path().join("graph.db");

    {
        let conn = Connection::open(&database).unwrap();
        load_graphqlite(&conn, &extension).unwrap();
        cypher(
            &conn,
            "CREATE (:Node {id: $id, label: $label}), (:Node {id: 'term:rust', label: 'Rust'})",
            serde_json::json!({"id": "term:知识", "label": "知识 🧠"}),
        )
        .unwrap();
        cypher(
            &conn,
            "MATCH (a:Node {id: $from}), (b:Node {id: $to}) CREATE (a)-[:CO_OCCURS {weight: 0.75}]->(b)",
            serde_json::json!({"from": "term:知识", "to": "term:rust"}),
        )
        .unwrap();
    }

    let conn = Connection::open(&database).unwrap();
    load_graphqlite(&conn, &extension).unwrap();
    let rows = cypher(
        &conn,
        "MATCH (a:Node)-[r:CO_OCCURS]->(b:Node) RETURN a.id AS source, b.id AS target, a.label AS label, r.weight AS weight ORDER BY source, target",
        serde_json::json!({}),
    )
    .unwrap();

    assert_eq!(
        rows,
        serde_json::json!([{
            "source": "term:知识",
            "target": "term:rust",
            "label": "知识 🧠",
            "weight": 0.75
        }])
    );
}
