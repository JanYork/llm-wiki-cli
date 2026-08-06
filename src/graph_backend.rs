use crate::error::{AppError, Result as AppResult};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
#[cfg(has_embedded_graphqlite)]
use sha2::{Digest, Sha256};
#[cfg(has_embedded_graphqlite)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

#[cfg(has_embedded_graphqlite)]
const EMBEDDED_GRAPHQLITE: &[u8] = include_bytes!(env!("LWC_GRAPHQLITE_EMBEDDED"));

pub fn embedded_graphqlite_available() -> bool {
    cfg!(has_embedded_graphqlite)
}

#[cfg(has_embedded_graphqlite)]
fn embedded_graphqlite_digest() -> String {
    Sha256::digest(EMBEDDED_GRAPHQLITE)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(has_embedded_graphqlite)]
pub fn materialize_graphqlite_runtime(database: &Path) -> AppResult<PathBuf> {
    let parent = database.parent().ok_or_else(|| {
        AppError::new(
            "graphqlite_runtime_failed",
            "wiki database has no parent directory",
        )
    })?;
    reject_symlink(parent)?;
    let runtime = parent.join("runtime");
    if runtime.exists() {
        reject_symlink(&runtime)?;
    } else {
        fs::create_dir_all(&runtime)?;
    }
    let digest = embedded_graphqlite_digest();
    let filename = env!("LWC_GRAPHQLITE_FILENAME");
    let path = runtime.join(format!("{}-{}", &digest[..16], filename));
    if path.exists() {
        reject_symlink(&path)?;
        let actual = Sha256::digest(fs::read(&path)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if actual != digest {
            return Err(AppError::new(
                "graphqlite_runtime_checksum_mismatch",
                "existing GraphQLite runtime checksum does not match the pinned artifact",
            ));
        }
        return Ok(path);
    }
    let temporary = runtime.join(format!(
        ".graphqlite-{}-{}.tmp",
        std::process::id(),
        &digest[..16]
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o700);
    let mut file = options.open(&temporary)?;
    file.write_all(EMBEDDED_GRAPHQLITE)?;
    file.sync_all()?;
    fs::rename(&temporary, &path)?;
    Ok(path)
}

#[cfg(not(has_embedded_graphqlite))]
pub fn materialize_graphqlite_runtime(_database: &Path) -> AppResult<PathBuf> {
    Err(AppError::new(
        "graphqlite_unavailable",
        "GraphQLite is not packaged for this target",
    ))
}

fn reject_symlink(path: &Path) -> AppResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_graphqlite_path",
            format!("GraphQLite path is a symlink: {}", path.display()),
        ));
    }
    Ok(())
}

pub fn load_graphqlite(conn: &Connection, extension: &Path) -> AppResult<()> {
    unsafe {
        conn.load_extension_enable()?;
        let result = conn.load_extension(extension, Some("sqlite3_graphqlite_init"));
        conn.load_extension_disable()?;
        result?;
    }
    let status: String = conn.query_row("SELECT graphqlite_test()", [], |row| row.get(0))?;
    if !status.to_lowercase().contains("successfully") {
        return Err(AppError::new(
            "graphqlite_self_test_failed",
            "GraphQLite startup self-test failed",
        ));
    }
    validate_graphqlite_storage_schema(conn)?;
    Ok(())
}

fn validate_graphqlite_storage_schema(conn: &Connection) -> AppResult<()> {
    for table in [
        "nodes",
        "node_labels",
        "node_props_text",
        "edges",
        "edge_props_text",
        "edge_props_real",
        "property_keys",
    ] {
        let exists: i64 = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        if exists != 1 {
            return Err(AppError::new(
                "graphqlite_schema_mismatch",
                "pinned GraphQLite storage schema is incompatible",
            ));
        }
    }
    Ok(())
}

pub fn graphqlite_cypher(conn: &Connection, query: &str, parameters: Value) -> AppResult<Value> {
    let raw: Option<String> = conn.query_row(
        "SELECT cypher(?1, ?2)",
        params![query, parameters.to_string()],
        |row| row.get(0),
    )?;
    Ok(raw.map_or(Value::Null, |value| {
        serde_json::from_str(&value).unwrap_or(Value::String(value))
    }))
}

fn graphqlite_property_keys(tx: &Transaction<'_>) -> AppResult<BTreeMap<&'static str, i64>> {
    let names = [
        "id",
        "node_type",
        "label",
        "document_type",
        "document_identifier",
        "weight",
        "confidence",
        "provenance",
        "reason",
    ];
    for key in names {
        tx.execute(
            "INSERT OR IGNORE INTO property_keys(key) VALUES (?1)",
            [key],
        )?;
    }
    names
        .into_iter()
        .map(|key| {
            tx.query_row(
                "SELECT id FROM property_keys WHERE key = ?1",
                [key],
                |row| row.get::<_, i64>(0),
            )
            .map(|id| (key, id))
            .map_err(Into::into)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn write_projected_node(
    tx: &Transaction<'_>,
    internal_id: i64,
    keys: &BTreeMap<&str, i64>,
    id: &str,
    node_type: &str,
    label: &str,
    document_type: Option<&str>,
    document_identifier: Option<&str>,
) -> AppResult<()> {
    tx.execute("DELETE FROM node_labels WHERE node_id = ?1", [internal_id])?;
    tx.execute(
        "DELETE FROM node_props_text WHERE node_id = ?1",
        [internal_id],
    )?;
    tx.execute(
        "INSERT INTO node_labels(node_id, label) VALUES (?1, 'LwcNode')",
        [internal_id],
    )?;
    for (key, value) in [
        ("id", Some(id)),
        ("node_type", Some(node_type)),
        ("label", Some(label)),
        ("document_type", document_type),
        ("document_identifier", document_identifier),
    ] {
        if let Some(value) = value {
            tx.execute(
                "INSERT INTO node_props_text(node_id, key_id, value) VALUES (?1, ?2, ?3)",
                params![internal_id, keys[key], value],
            )?;
        }
    }
    Ok(())
}

fn project_graphqlite_deltas(
    conn: &mut Connection,
    canonical: &Connection,
    after_generation: i64,
    through_generation: i64,
) -> AppResult<()> {
    let mut statement = canonical.prepare(
        "SELECT entity_type, entity_id FROM graph_deltas
         WHERE generation > ?1 AND generation <= ?2
         ORDER BY id",
    )?;
    let changed = statement
        .query_map(params![after_generation, through_generation], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut changed_nodes = changed
        .iter()
        .filter(|(kind, _)| kind == "node")
        .map(|(_, id)| id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut changed_edges = changed
        .iter()
        .filter(|(kind, _)| kind == "edge")
        .map(|(_, id)| id.clone())
        .collect::<std::collections::BTreeSet<_>>();

    let tx = conn.transaction()?;
    let keys = graphqlite_property_keys(&tx)?;
    let mut node_ids = {
        let mut statement =
            tx.prepare("SELECT value, node_id FROM node_props_text WHERE key_id = ?1")?;
        statement
            .query_map([keys["id"]], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
    };
    let canonical_edge_count: i64 =
        canonical.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?;
    let dense_replacement = changed_edges.len() as i64 > canonical_edge_count / 2;
    if dense_replacement {
        tx.execute("DELETE FROM edges", [])?;
        tx.execute("DELETE FROM nodes", [])?;
        node_ids.clear();
        changed_nodes = {
            let mut statement =
                canonical.prepare("SELECT node_id FROM graph_nodes ORDER BY node_id")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?
        };
        changed_edges = {
            let mut statement =
                canonical.prepare("SELECT edge_id FROM graph_edges ORDER BY edge_id")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?
        };
    } else {
        let mut delete_edge = tx.prepare(
            "DELETE FROM edges WHERE id IN (
                SELECT edge_id FROM edge_props_text WHERE key_id = ?1 AND value = ?2
             )",
        )?;
        for edge_id in &changed_edges {
            delete_edge.execute(params![keys["id"], edge_id])?;
        }
    }
    let mut current_node = canonical.prepare(
        "SELECT node_type, label, document_type, document_identifier
         FROM graph_nodes WHERE node_id = ?1",
    )?;
    for node_id in &changed_nodes {
        let projected = node_ids.get(node_id).copied();
        let current = current_node
            .query_row([node_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .optional()?;
        match (projected, current) {
            (Some(internal_id), None) => {
                tx.execute("DELETE FROM nodes WHERE id = ?1", [internal_id])?;
                node_ids.remove(node_id);
            }
            (Some(internal_id), Some((node_type, label, document_type, document_identifier))) => {
                write_projected_node(
                    &tx,
                    internal_id,
                    &keys,
                    node_id,
                    &node_type,
                    &label,
                    document_type.as_deref(),
                    document_identifier.as_deref(),
                )?;
            }
            (None, Some((node_type, label, document_type, document_identifier))) => {
                tx.execute("INSERT INTO nodes DEFAULT VALUES", [])?;
                let internal_id = tx.last_insert_rowid();
                node_ids.insert(node_id.clone(), internal_id);
                write_projected_node(
                    &tx,
                    internal_id,
                    &keys,
                    node_id,
                    &node_type,
                    &label,
                    document_type.as_deref(),
                    document_identifier.as_deref(),
                )?;
            }
            (None, None) => {}
        }
    }
    if std::env::var("LWC_TEST_GRAPHQLITE_FAIL_AT").as_deref() == Ok("after-nodes") {
        return Err(AppError::new(
            "graphqlite_injected_failure",
            "injected GraphQLite projection failure",
        ));
    }
    {
        let mut current_edge = canonical.prepare(
            "SELECT edge_type, from_node_id, to_node_id,
                    weight, confidence, provenance, reason
             FROM graph_edges WHERE edge_id = ?1",
        )?;
        let mut insert_edge =
            tx.prepare("INSERT INTO edges(source_id, target_id, type) VALUES (?1, ?2, ?3)")?;
        let mut insert_text =
            tx.prepare("INSERT INTO edge_props_text(edge_id, key_id, value) VALUES (?1, ?2, ?3)")?;
        let mut insert_real =
            tx.prepare("INSERT INTO edge_props_real(edge_id, key_id, value) VALUES (?1, ?2, ?3)")?;
        for edge_id in &changed_edges {
            let Some((edge_type, from, to, weight, confidence, provenance, reason)) = current_edge
                .query_row([edge_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })
                .optional()?
            else {
                continue;
            };
            let source_id = node_ids.get(&from).ok_or_else(|| {
                AppError::new(
                    "graphqlite_projection_mismatch",
                    "edge source node is missing",
                )
            })?;
            let target_id = node_ids.get(&to).ok_or_else(|| {
                AppError::new(
                    "graphqlite_projection_mismatch",
                    "edge target node is missing",
                )
            })?;
            insert_edge.execute(params![source_id, target_id, edge_type])?;
            let internal_edge = tx.last_insert_rowid();
            insert_text.execute(params![internal_edge, keys["id"], edge_id])?;
            for (key, value) in [
                ("provenance", provenance.as_deref()),
                ("reason", reason.as_deref()),
            ] {
                if let Some(value) = value {
                    insert_text.execute(params![internal_edge, keys[key], value])?;
                }
            }
            for (key, value) in [("weight", weight), ("confidence", confidence)] {
                if let Some(value) = value {
                    insert_real.execute(params![internal_edge, keys[key], value])?;
                }
            }
        }
    }
    if std::env::var("LWC_TEST_GRAPHQLITE_FAIL_AT").as_deref() == Ok("after-edges") {
        return Err(AppError::new(
            "graphqlite_injected_failure",
            "injected GraphQLite projection failure",
        ));
    }
    tx.commit()?;
    Ok(())
}

fn previous_graphqlite_sidecar(
    canonical: &Connection,
    parent: &Path,
    generation: i64,
) -> AppResult<Option<(i64, PathBuf)>> {
    let mut statement = canonical.prepare(
        "SELECT generation, canonical_digest FROM graph_generations
         WHERE generation < ?1 ORDER BY generation DESC",
    )?;
    for row in statement.query_map([generation], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (candidate_generation, digest) = row?;
        let path = parent.join(format!(
            "graph-graphqlite-g{candidate_generation}-{}.db",
            &digest[..16]
        ));
        if path.is_file() {
            reject_symlink(&path)?;
            return Ok(Some((candidate_generation, path)));
        }
    }
    Ok(None)
}

pub fn project_graphqlite_snapshot(
    canonical: &Connection,
    database: &Path,
    generation: i64,
    digest: &str,
) -> AppResult<PathBuf> {
    if std::env::var("LWC_TEST_GRAPHQLITE_FAIL_AT").as_deref() == Ok("before") {
        return Err(AppError::new(
            "graphqlite_injected_failure",
            "injected GraphQLite projection failure",
        ));
    }
    let extension = materialize_graphqlite_runtime(database)?;
    let parent = database.parent().ok_or_else(|| {
        AppError::new(
            "graphqlite_projection_failed",
            "wiki database has no parent directory",
        )
    })?;
    let sidecar = parent.join(format!(
        "graph-graphqlite-g{generation}-{}.db",
        &digest[..16]
    ));
    let canonical_nodes: i64 =
        canonical.query_row("SELECT COUNT(*) FROM graph_nodes", [], |row| row.get(0))?;
    let canonical_edges: i64 =
        canonical.query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?;
    if sidecar.exists() {
        reject_symlink(&sidecar)?;
        let counts = graphqlite_projection_counts(database, generation, digest)?;
        if counts == (canonical_nodes, canonical_edges) {
            return Ok(sidecar);
        }
        return Err(AppError::new(
            "graphqlite_projection_mismatch",
            "existing GraphQLite projection does not match canonical graph",
        ));
    }
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pending = parent.join(format!(
        ".graph-graphqlite-g{generation}-{}-{}-{suffix}.pending.db",
        &digest[..16],
        std::process::id(),
    ));
    if let Some((previous_generation, previous)) =
        previous_graphqlite_sidecar(canonical, parent, generation)?
    {
        fs::copy(previous, &pending)?;
        #[cfg(unix)]
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o600))?;
        let mut incremental = Connection::open(&pending)?;
        validate_graphqlite_storage_schema(&incremental)?;
        incremental.execute_batch(
            "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;",
        )?;
        project_graphqlite_deltas(&mut incremental, canonical, previous_generation, generation)?;
        let projected_nodes: i64 =
            incremental.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
        let projected_edges: i64 =
            incremental.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
        if projected_nodes != canonical_nodes || projected_edges != canonical_edges {
            return Err(AppError::new(
                "graphqlite_projection_mismatch",
                "incremental GraphQLite projection row counts do not match canonical graph",
            ));
        }
        drop(incremental);
        fs::rename(&pending, &sidecar)?;
        return Ok(sidecar);
    }
    let mut conn = Connection::open(&pending)?;
    #[cfg(unix)]
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o600))?;
    load_graphqlite(&conn, &extension)?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;",
    )?;
    let mut node_statement = canonical.prepare(
        "SELECT node_id, node_type, label, document_type, document_identifier
         FROM graph_nodes ORDER BY node_id",
    )?;
    let nodes = node_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut edge_statement = canonical.prepare(
        "SELECT edge_id, edge_type, from_node_id, to_node_id,
                weight, confidence, provenance, reason
         FROM graph_edges ORDER BY edge_id",
    )?;
    let edges = edge_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let tx = conn.transaction()?;
    let property_names = [
        "id",
        "node_type",
        "label",
        "document_type",
        "document_identifier",
        "weight",
        "confidence",
        "provenance",
        "reason",
    ];
    for key in property_names {
        tx.execute(
            "INSERT OR IGNORE INTO property_keys(key) VALUES (?1)",
            [key],
        )?;
    }
    let property_keys = property_names
        .into_iter()
        .map(|key| {
            tx.query_row(
                "SELECT id FROM property_keys WHERE key = ?1",
                [key],
                |row| row.get::<_, i64>(0),
            )
            .map(|id| (key, id))
        })
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let mut node_ids = BTreeMap::new();
    for (id, node_type, label, document_type, document_identifier) in &nodes {
        tx.execute("INSERT INTO nodes DEFAULT VALUES", [])?;
        let internal_id = tx.last_insert_rowid();
        node_ids.insert(id.clone(), internal_id);
        tx.execute(
            "INSERT INTO node_labels(node_id, label) VALUES (?1, 'LwcNode')",
            [internal_id],
        )?;
        for (key, value) in [
            ("id", Some(id.as_str())),
            ("node_type", Some(node_type.as_str())),
            ("label", Some(label.as_str())),
            ("document_type", document_type.as_deref()),
            ("document_identifier", document_identifier.as_deref()),
        ] {
            if let Some(value) = value {
                tx.execute(
                    "INSERT INTO node_props_text(node_id, key_id, value)
                     VALUES (?1, ?2, ?3)",
                    params![internal_id, property_keys[key], value],
                )?;
            }
        }
    }
    if std::env::var("LWC_TEST_GRAPHQLITE_FAIL_AT").as_deref() == Ok("after-nodes") {
        return Err(AppError::new(
            "graphqlite_injected_failure",
            "injected GraphQLite projection failure",
        ));
    }
    for (id, edge_type, from, to, weight, confidence, provenance, reason) in &edges {
        if !edge_type
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch == '_')
        {
            return Err(AppError::new(
                "invalid_graph_edge_type",
                "unsafe projected edge type",
            ));
        }
        let source_id = node_ids.get(from).ok_or_else(|| {
            AppError::new(
                "graphqlite_projection_mismatch",
                "edge source node is missing",
            )
        })?;
        let target_id = node_ids.get(to).ok_or_else(|| {
            AppError::new(
                "graphqlite_projection_mismatch",
                "edge target node is missing",
            )
        })?;
        tx.execute(
            "INSERT INTO edges(source_id, target_id, type) VALUES (?1, ?2, ?3)",
            params![source_id, target_id, edge_type],
        )?;
        let edge_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO edge_props_text(edge_id, key_id, value) VALUES (?1, ?2, ?3)",
            params![edge_id, property_keys["id"], id],
        )?;
        for (key, value) in [
            ("provenance", provenance.as_deref()),
            ("reason", reason.as_deref()),
        ] {
            if let Some(value) = value {
                tx.execute(
                    "INSERT INTO edge_props_text(edge_id, key_id, value)
                     VALUES (?1, ?2, ?3)",
                    params![edge_id, property_keys[key], value],
                )?;
            }
        }
        for (key, value) in [("weight", *weight), ("confidence", *confidence)] {
            if let Some(value) = value {
                tx.execute(
                    "INSERT INTO edge_props_real(edge_id, key_id, value)
                     VALUES (?1, ?2, ?3)",
                    params![edge_id, property_keys[key], value],
                )?;
            }
        }
    }
    if std::env::var("LWC_TEST_GRAPHQLITE_FAIL_AT").as_deref() == Ok("after-edges") {
        return Err(AppError::new(
            "graphqlite_injected_failure",
            "injected GraphQLite projection failure",
        ));
    }
    tx.commit()?;
    let projected_nodes: i64 =
        conn.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
    let projected_edges: i64 =
        conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
    if projected_nodes != canonical_nodes || projected_edges != canonical_edges {
        return Err(AppError::new(
            "graphqlite_projection_mismatch",
            "GraphQLite projection row counts do not match canonical graph",
        ));
    }
    drop(conn);
    fs::rename(&pending, &sidecar)?;
    Ok(sidecar)
}

fn normalized_positions(positions: &[Range<usize>]) -> Value {
    Value::Array(
        positions
            .iter()
            .map(|position| json!({"byte_start": position.start, "byte_end": position.end}))
            .collect(),
    )
}

fn canonical_projection_edges(canonical: &Connection) -> AppResult<Value> {
    let mut normalized = BTreeMap::new();
    let mut statement = canonical.prepare(
        "SELECT edge_id, edge_type, from_node_id, to_node_id,
                weight, confidence, provenance, reason,
                frequency, positions, first_position
         FROM graph_edges ORDER BY edge_id",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<f64>>(4)?,
            row.get::<_, Option<f64>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<Vec<u8>>>(9)?,
            row.get::<_, Option<i64>>(10)?,
        ))
    })? {
        let (
            id,
            edge_type,
            source,
            target,
            weight,
            confidence,
            provenance,
            reason,
            frequency,
            encoded,
            first_position,
        ) = row?;
        let positions = encoded
            .map(|encoded| decode_positions(&encoded))
            .transpose()
            .map_err(|error| {
                AppError::new(
                    "graph_index_invalid_positions",
                    format!("could not decode canonical term positions: {error:?}"),
                )
            })?;
        normalized.insert(
            id.clone(),
            json!({
                "identifier": id, "type": edge_type,
                "source": source, "target": target,
                "weight": weight, "confidence": confidence,
                "provenance": provenance, "reason": reason,
                "frequency": frequency,
                "positions": positions.as_deref().map(normalized_positions),
                "first_position": first_position,
            }),
        );
    }
    for edge in derived_projection_edges(canonical)? {
        if let Some(identifier) = edge["identifier"].as_str() {
            normalized.insert(identifier.to_string(), edge);
        }
    }
    let mut edges = normalized.into_values().collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        (
            left["type"].as_str(),
            left["source"].as_str(),
            left["target"].as_str(),
            left["identifier"].as_str(),
        )
            .cmp(&(
                right["type"].as_str(),
                right["source"].as_str(),
                right["target"].as_str(),
                right["identifier"].as_str(),
            ))
    });
    Ok(Value::Array(edges))
}

fn derived_projection_edges(canonical: &Connection) -> AppResult<Vec<Value>> {
    let mut edges = Vec::new();
    let mut peers: BTreeMap<(String, String), Vec<(i64, String)>> = BTreeMap::new();
    {
        let mut statement = canonical.prepare(
            "SELECT node_id, parent_node_id, node_type, ordinal
             FROM graph_nodes WHERE parent_node_id IS NOT NULL
             ORDER BY parent_node_id, node_type, ordinal, node_id",
        )?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })? {
            let (node, parent, node_type, ordinal) = row?;
            let edge = crate::graph::automatic_edge("CONTAINS", &parent, &node);
            edges.push(json!({
                "identifier": edge.edge_id, "type": "CONTAINS",
                "source": parent, "target": node,
                "weight": Value::Null, "confidence": Value::Null,
                "provenance": "automatic", "reason": Value::Null,
                "frequency": Value::Null, "positions": Value::Null,
                "first_position": Value::Null,
            }));
            peers
                .entry((edge.from_node_id, node_type))
                .or_default()
                .push((ordinal, edge.to_node_id));
        }
    }
    for siblings in peers.values_mut() {
        siblings.sort();
        for pair in siblings.windows(2) {
            if pair[1].0 != pair[0].0 + 1 {
                continue;
            }
            for (edge_type, from, to) in [
                ("NEXT", pair[0].1.as_str(), pair[1].1.as_str()),
                ("PREVIOUS", pair[1].1.as_str(), pair[0].1.as_str()),
            ] {
                let edge = crate::graph::automatic_edge(edge_type, from, to);
                edges.push(json!({
                    "identifier": edge.edge_id, "type": edge_type,
                    "source": from, "target": to,
                    "weight": Value::Null, "confidence": Value::Null,
                    "provenance": "automatic", "reason": Value::Null,
                    "frequency": Value::Null, "positions": Value::Null,
                    "first_position": Value::Null,
                }));
            }
        }
    }
    let mut occurrence_statement = canonical.prepare(
        "SELECT o.term_node_id, o.document_type, o.document_identifier,
                o.positions, s.document_node_id
         FROM graph_occurrences o
         JOIN document_index_state s
           ON s.document_type = o.document_type
          AND s.document_identifier = o.document_identifier
         ORDER BY o.term_node_id, o.document_type, o.document_identifier",
    )?;
    for row in occurrence_statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })? {
        let (term, document_type, document_identifier, encoded, document_node) = row?;
        let positions = decode_positions(&encoded).map_err(|error| {
            AppError::new(
                "graph_index_invalid_positions",
                format!("could not decode projected term positions: {error:?}"),
            )
        })?;
        let mut targets = vec![(document_node, positions.clone())];
        let mut span_statement = canonical.prepare(
            "SELECT node_id, byte_start, byte_end FROM graph_nodes
             WHERE document_type = ?1 AND document_identifier = ?2
               AND node_type IN ('passage', 'sentence')
             ORDER BY node_type, ordinal, node_id",
        )?;
        for span in
            span_statement.query_map(params![document_type, document_identifier], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as usize,
                    row.get::<_, i64>(2)? as usize,
                ))
            })?
        {
            let (span, start, end) = span?;
            let span_positions = positions
                .iter()
                .filter(|position| position.start >= start && position.end <= end)
                .cloned()
                .collect::<Vec<_>>();
            if !span_positions.is_empty() {
                targets.push((span, span_positions));
            }
        }
        for (target, positions) in targets {
            let edge = crate::graph::automatic_edge("OCCURS_IN", &term, &target);
            edges.push(json!({
                "identifier": edge.edge_id, "type": "OCCURS_IN",
                "source": term, "target": target,
                "weight": Value::Null, "confidence": Value::Null,
                "provenance": "automatic", "reason": Value::Null,
                "frequency": positions.len(),
                "first_position": positions.first().map(|position| position.start),
                "positions": positions.iter().map(|position| json!({
                    "byte_start": position.start, "byte_end": position.end,
                })).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(edges)
}

pub fn graphqlite_projection_edges(
    database: &Path,
    generation: i64,
    digest: &str,
) -> AppResult<Value> {
    let extension = materialize_graphqlite_runtime(database)?;
    let parent = database.parent().ok_or_else(|| {
        AppError::new(
            "graphqlite_projection_failed",
            "wiki database has no parent",
        )
    })?;
    let sidecar = parent.join(format!(
        "graph-graphqlite-g{generation}-{}.db",
        &digest[..16]
    ));
    reject_symlink(&sidecar)?;
    let conn = Connection::open_with_flags(&sidecar, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    load_graphqlite(&conn, &extension)?;
    let projected = graphqlite_cypher(
        &conn,
        "MATCH (a:LwcNode)-[r]->(b:LwcNode) RETURN r.id AS identifier, type(r) AS type, a.id AS source, b.id AS target, r.weight AS weight, r.confidence AS confidence, r.provenance AS provenance, r.reason AS reason ORDER BY type, source, target, identifier",
        json!({}),
    )?;
    let mut normalized = BTreeMap::new();
    for mut edge in projected.as_array().cloned().unwrap_or_default() {
        edge["frequency"] = Value::Null;
        edge["positions"] = Value::Null;
        edge["first_position"] = Value::Null;
        if let Some(identifier) = edge["identifier"].as_str() {
            normalized.insert(identifier.to_string(), edge);
        }
    }
    let canonical = Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    for edge in derived_projection_edges(&canonical)? {
        if let Some(identifier) = edge["identifier"].as_str() {
            normalized.insert(identifier.to_string(), edge);
        }
    }
    let mut edges = normalized.into_values().collect::<Vec<_>>();
    edges.sort_by(|left, right| {
        (
            left["type"].as_str(),
            left["source"].as_str(),
            left["target"].as_str(),
            left["identifier"].as_str(),
        )
            .cmp(&(
                right["type"].as_str(),
                right["source"].as_str(),
                right["target"].as_str(),
                right["identifier"].as_str(),
            ))
    });
    let normalized = Value::Array(edges);
    if normalized != canonical_projection_edges(&canonical)? {
        return Err(AppError::new(
            "graphqlite_projection_mismatch",
            "normalized GraphQLite output does not match the canonical logical graph",
        ));
    }
    Ok(normalized)
}

pub fn graphqlite_projection_counts(
    database: &Path,
    generation: i64,
    digest: &str,
) -> AppResult<(i64, i64)> {
    let parent = database.parent().ok_or_else(|| {
        AppError::new(
            "graphqlite_projection_failed",
            "wiki database has no parent",
        )
    })?;
    let sidecar = parent.join(format!(
        "graph-graphqlite-g{generation}-{}.db",
        &digest[..16]
    ));
    reject_symlink(&sidecar)?;
    let conn = Connection::open_with_flags(&sidecar, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok((
        conn.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?,
        conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PositionEncodingError {
    InvalidRange { start: usize, end: usize },
    Unsorted,
    Malformed,
    Overflow,
}

pub fn encode_positions(positions: &[Range<usize>]) -> Result<Vec<u8>, PositionEncodingError> {
    let mut encoded = Vec::new();
    let mut previous_start = 0usize;
    for position in positions {
        if position.start >= position.end {
            return Err(PositionEncodingError::InvalidRange {
                start: position.start,
                end: position.end,
            });
        }
        if position.start < previous_start {
            return Err(PositionEncodingError::Unsorted);
        }
        push_varint(&mut encoded, position.start - previous_start);
        push_varint(&mut encoded, position.end - position.start);
        previous_start = position.start;
    }
    Ok(encoded)
}

pub fn create_hierarchical_graph_schema(conn: &Connection) -> rusqlite::Result<()> {
    let exists = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_schema
            WHERE type = 'table' AND name = 'document_index_state'
        )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if exists {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE document_index_state(
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL CHECK(TRIM(document_identifier) <> ''),
            document_node_id TEXT NOT NULL UNIQUE,
            content_fingerprint TEXT NOT NULL CHECK(LENGTH(content_fingerprint) = 64),
            segmenter_version INTEGER NOT NULL CHECK(segmenter_version >= 1),
            generation INTEGER NOT NULL CHECK(generation >= 0),
            cooccurrence_truncated INTEGER NOT NULL DEFAULT 0
                CHECK(cooccurrence_truncated >= 0),
            indexed_at TEXT NOT NULL,
            PRIMARY KEY(document_type, document_identifier)
        );

        CREATE TABLE graph_nodes(
            node_id TEXT PRIMARY KEY CHECK(TRIM(node_id) <> ''),
            node_type TEXT NOT NULL
                CHECK(node_type IN ('document', 'passage', 'sentence', 'term')),
            document_type TEXT CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT,
            parent_node_id TEXT,
            ordinal INTEGER CHECK(ordinal IS NULL OR ordinal >= 0),
            byte_start INTEGER CHECK(byte_start IS NULL OR byte_start >= 0),
            byte_end INTEGER CHECK(byte_end IS NULL OR byte_end > byte_start),
            content_fingerprint TEXT
                CHECK(content_fingerprint IS NULL OR LENGTH(content_fingerprint) = 64),
            segmenter_version INTEGER
                CHECK(segmenter_version IS NULL OR segmenter_version >= 1),
            label TEXT NOT NULL,
            properties_json TEXT NOT NULL,
            FOREIGN KEY(parent_node_id) REFERENCES graph_nodes(node_id) ON DELETE CASCADE
        ) WITHOUT ROWID;

        CREATE INDEX graph_nodes_document
        ON graph_nodes(document_type, document_identifier, node_type, ordinal);
        CREATE INDEX graph_nodes_parent
        ON graph_nodes(parent_node_id, node_type, ordinal);
        CREATE INDEX graph_nodes_type_label
        ON graph_nodes(node_type, label, node_id)
        WHERE node_type IN ('document', 'term');

        CREATE TABLE graph_edges(
            edge_id TEXT PRIMARY KEY CHECK(TRIM(edge_id) <> ''),
            edge_type TEXT NOT NULL CHECK(edge_type IN (
                'CONTAINS', 'NEXT', 'PREVIOUS', 'OCCURS_IN', 'LINKS_TO', 'CITES',
                'REVISION_OF', 'CO_OCCURS', 'SUPPORTS', 'CONTRADICTS', 'REFINES',
                'SUPERSEDES', 'CAUSES', 'DEPENDS_ON'
            )),
            from_node_id TEXT NOT NULL,
            to_node_id TEXT NOT NULL,
            owner_type TEXT NOT NULL
                CHECK(owner_type IN ('page', 'source', 'path', 'global', 'manual')),
            owner_identifier TEXT NOT NULL,
            weight REAL,
            confidence REAL CHECK(confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
            provenance TEXT CHECK(provenance IS NULL OR provenance IN (
                'automatic', 'source-grounded', 'user-provided',
                'agent-observed', 'hypothesis'
            )),
            reason TEXT,
            frequency INTEGER CHECK(frequency IS NULL OR frequency > 0),
            positions BLOB,
            first_position INTEGER CHECK(first_position IS NULL OR first_position >= 0),
            properties_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(from_node_id) REFERENCES graph_nodes(node_id) ON DELETE CASCADE,
            FOREIGN KEY(to_node_id) REFERENCES graph_nodes(node_id) ON DELETE CASCADE,
            CHECK(
                (edge_type = 'OCCURS_IN' AND frequency IS NOT NULL
                    AND positions IS NOT NULL AND first_position IS NOT NULL)
                OR
                (edge_type <> 'OCCURS_IN' AND frequency IS NULL
                    AND positions IS NULL AND first_position IS NULL)
            )
        ) WITHOUT ROWID;

        CREATE INDEX graph_edges_forward
        ON graph_edges(from_node_id, edge_type, to_node_id, edge_id);
        CREATE INDEX graph_edges_reverse
        ON graph_edges(to_node_id, edge_type, from_node_id, edge_id)
        WHERE edge_type <> 'CO_OCCURS';

        CREATE TABLE graph_occurrences(
            term_node_id TEXT NOT NULL,
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL,
            frequency INTEGER NOT NULL CHECK(frequency > 0),
            positions BLOB NOT NULL,
            first_position INTEGER NOT NULL CHECK(first_position >= 0),
            PRIMARY KEY(term_node_id, document_type, document_identifier),
            FOREIGN KEY(term_node_id) REFERENCES graph_nodes(node_id) ON DELETE CASCADE
        ) WITHOUT ROWID;

        CREATE INDEX graph_occurrences_document
        ON graph_occurrences(document_type, document_identifier, term_node_id);

        CREATE TABLE term_pair_contributions(
            document_type TEXT NOT NULL CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT NOT NULL,
            contributions BLOB NOT NULL,
            PRIMARY KEY(document_type, document_identifier)
        ) WITHOUT ROWID;

        CREATE TABLE term_pair_totals(
            from_term_id TEXT NOT NULL,
            to_term_id TEXT NOT NULL,
            raw_strength REAL NOT NULL CHECK(raw_strength >= 0),
            witness_count INTEGER NOT NULL CHECK(witness_count > 0),
            PRIMARY KEY(from_term_id, to_term_id),
            CHECK(from_term_id <> to_term_id)
        ) WITHOUT ROWID;

        CREATE VIRTUAL TABLE span_fts USING fts5(
            span_id UNINDEXED,
            span_type UNINDEXED,
            document_type UNINDEXED,
            document_identifier UNINDEXED,
            title_terms,
            path_terms,
            body_terms,
            content='',
            contentless_delete=1,
            contentless_unindexed=1
        );

        CREATE TABLE graph_generations(
            generation INTEGER PRIMARY KEY CHECK(generation >= 1),
            store_revision TEXT NOT NULL CHECK(LENGTH(store_revision) = 64),
            canonical_digest TEXT NOT NULL CHECK(LENGTH(canonical_digest) = 64),
            changed_document_count INTEGER NOT NULL CHECK(changed_document_count >= 0),
            created_at TEXT NOT NULL
        );

        CREATE TABLE graph_deltas(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            generation INTEGER NOT NULL,
            action TEXT NOT NULL CHECK(action IN ('add', 'update', 'remove')),
            entity_type TEXT NOT NULL CHECK(entity_type IN ('node', 'edge')),
            entity_id TEXT NOT NULL,
            document_type TEXT CHECK(document_type IN ('page', 'source')),
            document_identifier TEXT,
            before_json TEXT,
            after_json TEXT,
            created_at TEXT NOT NULL,
            FOREIGN KEY(generation) REFERENCES graph_generations(generation) ON DELETE RESTRICT,
            CHECK(before_json IS NOT NULL OR after_json IS NOT NULL)
        );

        CREATE INDEX graph_deltas_generation
        ON graph_deltas(generation, id);
        CREATE INDEX graph_deltas_entity
        ON graph_deltas(entity_type, entity_id, generation, id);

        CREATE TABLE graph_projection_state(
            projection TEXT PRIMARY KEY CHECK(TRIM(projection) <> ''),
            engine TEXT NOT NULL CHECK(engine IN ('graphlite', 'rslg')),
            schema_version INTEGER NOT NULL CHECK(schema_version >= 1),
            canonical_generation INTEGER NOT NULL CHECK(canonical_generation >= 0),
            projected_generation INTEGER NOT NULL CHECK(projected_generation >= 0),
            status TEXT NOT NULL CHECK(status IN ('disabled', 'fresh', 'pending', 'stale')),
            last_error_code TEXT,
            last_error_message TEXT,
            updated_at TEXT NOT NULL
        );",
    )
}

pub fn decode_positions(encoded: &[u8]) -> Result<Vec<Range<usize>>, PositionEncodingError> {
    let mut positions = Vec::new();
    let mut cursor = 0usize;
    let mut previous_start = 0usize;
    while cursor < encoded.len() {
        let delta = read_varint(encoded, &mut cursor)?;
        let length = read_varint(encoded, &mut cursor)?;
        if length == 0 {
            return Err(PositionEncodingError::Malformed);
        }
        let start = previous_start
            .checked_add(delta)
            .ok_or(PositionEncodingError::Overflow)?;
        let end = start
            .checked_add(length)
            .ok_or(PositionEncodingError::Overflow)?;
        positions.push(start..end);
        previous_start = start;
    }
    Ok(positions)
}

fn push_varint(encoded: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        encoded.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    encoded.push(value as u8);
}

fn read_varint(encoded: &[u8], cursor: &mut usize) -> Result<usize, PositionEncodingError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *encoded
            .get(*cursor)
            .ok_or(PositionEncodingError::Malformed)?;
        *cursor += 1;
        let part = u64::from(byte & 0x7f)
            .checked_shl(shift)
            .ok_or(PositionEncodingError::Overflow)?;
        value = value
            .checked_add(part)
            .ok_or(PositionEncodingError::Overflow)?;
        if byte & 0x80 == 0 {
            return usize::try_from(value).map_err(|_| PositionEncodingError::Overflow);
        }
        shift = shift
            .checked_add(7)
            .filter(|shift| *shift < 64)
            .ok_or(PositionEncodingError::Overflow)?;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_projection_edges, create_hierarchical_graph_schema, decode_positions,
        encode_positions, graphqlite_projection_counts, graphqlite_projection_edges,
        project_graphqlite_snapshot,
    };
    use rusqlite::Connection;

    #[test]
    fn compact_positions_round_trip_overlapping_utf8_ranges() {
        let positions = vec![5..11, 8..14, 300..305, 16_384..16_390];

        let encoded = encode_positions(&positions).unwrap();

        assert_eq!(decode_positions(&encoded).unwrap(), positions);
        assert!(encoded.len() < serde_json::to_vec(&positions_as_pairs()).unwrap().len());
    }

    fn positions_as_pairs() -> Vec<[usize; 2]> {
        vec![[5, 11], [8, 14], [300, 305], [16_384, 16_390]]
    }

    #[test]
    fn hierarchical_graph_schema_has_exact_table_inventory() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        create_hierarchical_graph_schema(&conn).unwrap();

        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            tables,
            vec![
                "document_index_state",
                "graph_deltas",
                "graph_edges",
                "graph_generations",
                "graph_nodes",
                "graph_occurrences",
                "graph_projection_state",
                "span_fts",
                "span_fts_config",
                "span_fts_content",
                "span_fts_data",
                "span_fts_docsize",
                "span_fts_idx",
                "term_pair_contributions",
                "term_pair_totals",
            ]
        );
        assert_eq!(
            conn.pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn position_codec_rejects_invalid_order_ranges_and_bytes() {
        assert_eq!(
            encode_positions(&[8..10, 7..9]),
            Err(super::PositionEncodingError::Unsorted)
        );
        assert_eq!(
            encode_positions(std::slice::from_ref(&(4..4))),
            Err(super::PositionEncodingError::InvalidRange { start: 4, end: 4 })
        );
        assert_eq!(
            decode_positions(&[0x80]),
            Err(super::PositionEncodingError::Malformed)
        );
    }

    #[test]
    fn compact_positions_stay_well_below_json_size_on_realistic_postings() {
        let positions = (0..10_000)
            .map(|index| (index * 7)..(index * 7 + 5))
            .collect::<Vec<_>>();
        let json = serde_json::to_vec(
            &positions
                .iter()
                .map(|position| [position.start, position.end])
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let encoded = encode_positions(&positions).unwrap();

        assert!(
            encoded.len() * 4 < json.len(),
            "{} vs {}",
            encoded.len(),
            json.len()
        );
        assert_eq!(decode_positions(&encoded).unwrap(), positions);
    }

    #[cfg(has_embedded_graphqlite)]
    #[test]
    fn embedded_graphqlite_projects_and_reopens_a_canonical_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let canonical_path = directory.path().join("wiki.db");
        let canonical = Connection::open(&canonical_path).unwrap();
        canonical.pragma_update(None, "foreign_keys", true).unwrap();
        create_hierarchical_graph_schema(&canonical).unwrap();
        for (id, node_type, label) in [
            ("page:alpha", "document", "知识 Alpha"),
            ("page:beta", "document", "Beta 🧠"),
        ] {
            canonical
                .execute(
                    "INSERT INTO graph_nodes(node_id, node_type, label, properties_json)
                 VALUES (?1, ?2, ?3, '{}')",
                    rusqlite::params![id, node_type, label],
                )
                .unwrap();
        }
        canonical
            .execute(
                "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier, provenance, properties_json,
                created_at, updated_at
             ) VALUES ('edge:test', 'SUPPORTS', 'page:alpha', 'page:beta',
                       'manual', 'edge:test', 'agent-observed', '{}', 'now', 'now')",
                [],
            )
            .unwrap();
        let digest = "a".repeat(64);

        let sidecar = project_graphqlite_snapshot(&canonical, &canonical_path, 1, &digest).unwrap();
        assert!(sidecar.is_file());
        assert_eq!(
            graphqlite_projection_counts(&canonical_path, 1, &digest).unwrap(),
            (2, 1)
        );
        let edges = graphqlite_projection_edges(&canonical_path, 1, &digest).unwrap();
        assert_eq!(edges[0]["identifier"], "edge:test");
        assert_eq!(edges[0]["type"], "SUPPORTS");
        assert_eq!(edges[0]["source"], "page:alpha");
        assert_eq!(edges[0]["target"], "page:beta");
    }

    #[cfg(has_embedded_graphqlite)]
    #[test]
    fn graphqlite_adapter_expands_compact_hierarchy_and_occurrence_postings() {
        let directory = tempfile::tempdir().unwrap();
        let canonical_path = directory.path().join("wiki.db");
        let canonical = Connection::open(&canonical_path).unwrap();
        canonical.pragma_update(None, "foreign_keys", true).unwrap();
        create_hierarchical_graph_schema(&canonical).unwrap();
        let fingerprint = "a".repeat(64);
        canonical
            .execute(
                "INSERT INTO graph_nodes(
                node_id, node_type, document_type, document_identifier,
                label, properties_json
             ) VALUES ('page:alpha', 'document', 'page', 'alpha', 'Alpha', '{}')",
                [],
            )
            .unwrap();
        canonical.execute(
            "INSERT INTO graph_nodes(
                node_id, node_type, document_type, document_identifier,
                parent_node_id, ordinal, byte_start, byte_end,
                content_fingerprint, segmenter_version, label, properties_json
             ) VALUES
                ('span:passage', 'passage', 'page', 'alpha', 'page:alpha', 0, 0, 11, ?1, 1, '', '{}'),
                ('span:sentence', 'sentence', 'page', 'alpha', 'span:passage', 0, 0, 11, ?1, 1, '', '{}')",
            [&fingerprint],
        ).unwrap();
        canonical
            .execute(
                "INSERT INTO graph_nodes(node_id, node_type, label, properties_json)
             VALUES ('term:alpha', 'term', 'alpha', '{}')",
                [],
            )
            .unwrap();
        canonical
            .execute(
                "INSERT INTO graph_edges(
                    edge_id, edge_type, from_node_id, to_node_id,
                    owner_type, owner_identifier, confidence, provenance, reason,
                    properties_json, created_at, updated_at
                 ) VALUES (
                    'edge:supports', 'SUPPORTS', 'page:alpha', 'term:alpha',
                    'manual', 'edge:supports', 0.8, 'agent-observed', 'test evidence',
                    '{}', 'now', 'now'
                 )",
                [],
            )
            .unwrap();
        canonical
            .execute(
                "INSERT INTO document_index_state(
                document_type, document_identifier, document_node_id,
                content_fingerprint, segmenter_version, generation,
                cooccurrence_truncated, indexed_at
             ) VALUES ('page', 'alpha', 'page:alpha', ?1, 1, 1, 0, 'now')",
                [&fingerprint],
            )
            .unwrap();
        canonical
            .execute(
                "INSERT INTO graph_occurrences(
                term_node_id, document_type, document_identifier,
                frequency, positions, first_position
             ) VALUES ('term:alpha', 'page', 'alpha', 2, ?1, 0)",
                [encode_positions(&[0..5, 6..11]).unwrap()],
            )
            .unwrap();
        let digest = "b".repeat(64);
        project_graphqlite_snapshot(&canonical, &canonical_path, 1, &digest).unwrap();
        let edges = graphqlite_projection_edges(&canonical_path, 1, &digest).unwrap();
        assert_eq!(edges, canonical_projection_edges(&canonical).unwrap());
        let edges = edges.as_array().unwrap();
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge["type"] == "CONTAINS")
                .count(),
            2
        );
        let occurrences = edges
            .iter()
            .filter(|edge| edge["type"] == "OCCURS_IN")
            .collect::<Vec<_>>();
        assert_eq!(occurrences.len(), 3);
        assert!(occurrences.iter().all(|edge| edge["frequency"] == 2));
        assert!(
            occurrences
                .iter()
                .all(|edge| edge["positions"].as_array().unwrap().len() == 2)
        );
    }

    #[test]
    fn occurrence_edges_require_compact_positions_and_existing_nodes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        create_hierarchical_graph_schema(&conn).unwrap();
        for (id, node_type) in [("term:rust", "term"), ("page:design", "document")] {
            conn.execute(
                "INSERT INTO graph_nodes(
                    node_id, node_type, label, properties_json
                 ) VALUES (?1, ?2, ?1, '{}')",
                rusqlite::params![id, node_type],
            )
            .unwrap();
        }

        assert!(
            conn.execute(
                "INSERT INTO graph_edges(
                    edge_id, edge_type, from_node_id, to_node_id,
                    owner_type, owner_identifier, properties_json, created_at, updated_at
                 ) VALUES (
                    'bad', 'OCCURS_IN', 'term:rust', 'page:design',
                    'page', 'design', '{}', 'now', 'now'
                 )",
                [],
            )
            .is_err()
        );
        let positions = encode_positions(&[5..9, 20..24]).unwrap();
        conn.execute(
            "INSERT INTO graph_edges(
                edge_id, edge_type, from_node_id, to_node_id,
                owner_type, owner_identifier, frequency, positions, first_position,
                properties_json, created_at, updated_at
             ) VALUES (
                'ok', 'OCCURS_IN', 'term:rust', 'page:design',
                'page', 'design', 2, ?1, 5, '{}', 'now', 'now'
             )",
            [positions],
        )
        .unwrap();
        assert!(
            conn.execute(
                "UPDATE graph_edges SET to_node_id = 'missing' WHERE edge_id = 'ok'",
                [],
            )
            .is_err()
        );
    }
}
