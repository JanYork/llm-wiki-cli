use crate::{
    config::{self, GraphSetting},
    error::{AppError, Result},
};
use grafeo::{Config as GrafeoConfig, GrafeoDB, Value as GrafeoValue};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs,
    path::Path,
    time::Instant,
};
use surrealdb::{Surreal, engine::local::SurrealKv, types::SurrealValue};

#[derive(Clone, Debug, Serialize, SurrealValue)]
struct ProjectionDocument {
    key: String,
    kind: String,
    identifier: String,
    title: String,
    fingerprint: String,
    links: Vec<String>,
    source_ids: Vec<i64>,
    relations: Vec<ProjectionRelation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, SurrealValue)]
struct ProjectionRelation {
    relation_type: String,
    to: String,
}

pub fn projection_keys(scope: &str, database: &Path) -> Result<Vec<(String, String)>> {
    let setting = config::resolve(scope, database)?.setting;
    if setting == GraphSetting::Disabled {
        return Err(AppError::new("graph_disabled", "graph is disabled"));
    }
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut keys = document_keys(&conn)?;
    match setting {
        GraphSetting::Grafeo => {
            let path = sidecar(database, setting)?;
            if path.exists() {
                let graph = GrafeoDB::open(path).map_err(graph_error)?;
                append_stale_keys(&mut keys, load_grafeo_documents(&graph)?);
                graph.close().map_err(graph_error)?;
            }
        }
        GraphSetting::Surrealdb => {
            let path = sidecar(database, setting)?;
            if path.exists() {
                let runtime = tokio::runtime::Runtime::new()
                    .map_err(|error| AppError::new("surrealdb_open_failed", error.to_string()))?;
                let documents = runtime.block_on(async {
                    let graph = open_surreal(database).await?;
                    let mut response = graph
                        .query("SELECT key, kind, identifier, title, fingerprint, links, source_ids, relations FROM document ORDER BY key")
                        .await
                        .map_err(surreal_error)?
                        .check()
                        .map_err(surreal_error)?;
                    response.take(0).map_err(surreal_error)
                })?;
                append_stale_keys(&mut keys, documents);
            }
        }
        GraphSetting::Disabled | GraphSetting::Inherit => unreachable!("effective graph setting"),
    }
    Ok(keys)
}

pub fn project_documents(
    scope: &str,
    database: &Path,
    selected: Option<&[(String, String)]>,
    progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
) -> Result<Value> {
    let setting = config::resolve(scope, database)?.setting;
    if setting == GraphSetting::Disabled {
        return Err(AppError::new("graph_disabled", "graph is disabled"));
    }
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut keys = selected.map_or_else(|| document_keys(&conn), |keys| Ok(keys.to_vec()))?;
    let mut projected = 0;

    match setting {
        GraphSetting::Grafeo => {
            let graph = GrafeoDB::open(sidecar(database, setting)?).map_err(graph_error)?;
            if selected.is_none() {
                append_stale_keys(&mut keys, load_grafeo_documents(&graph)?);
            }
            projected = keys.len();
            progress(0, projected, "projecting-documents")?;
            for (index, (kind, identifier)) in keys.iter().enumerate() {
                let document = load_document(&conn, kind, identifier)?;
                replace_grafeo(&graph, kind, identifier, document.as_ref())?;
                progress(index + 1, projected, "projecting-documents")?;
            }
            graph.close().map_err(graph_error)?;
        }
        GraphSetting::Surrealdb => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| AppError::new("surrealdb_open_failed", error.to_string()))?;
            runtime.block_on(async {
                let graph = open_surreal(database).await?;
                if selected.is_none() {
                    let mut response = graph
                        .query("SELECT key, kind, identifier, title, fingerprint, links, source_ids, relations FROM document ORDER BY key")
                        .await
                        .map_err(surreal_error)?
                        .check()
                        .map_err(surreal_error)?;
                    let documents: Vec<ProjectionDocument> =
                        response.take(0).map_err(surreal_error)?;
                    append_stale_keys(&mut keys, documents);
                }
                projected = keys.len();
                progress(0, projected, "projecting-documents")?;
                for (index, (kind, identifier)) in keys.iter().enumerate() {
                    let document = load_document(&conn, kind, identifier)?;
                    replace_surreal(&graph, kind, identifier, document.as_ref()).await?;
                    progress(index + 1, projected, "projecting-documents")?;
                }
                Ok::<(), AppError>(())
            })?;
        }
        GraphSetting::Disabled | GraphSetting::Inherit => unreachable!("effective graph setting"),
    }
    Ok(json!({
        "engine": setting_name(setting),
        "status": "ready",
        "documents": projected,
    }))
}

fn append_stale_keys(keys: &mut Vec<(String, String)>, documents: Vec<ProjectionDocument>) {
    let current = keys.iter().cloned().collect::<BTreeSet<_>>();
    keys.extend(
        documents
            .into_iter()
            .map(|document| (document.kind, document.identifier))
            .filter(|key| !current.contains(key)),
    );
}

pub fn status(scope: &str, database: &Path) -> Result<Value> {
    let setting = config::resolve(scope, database)?.setting;
    let documents = match setting {
        GraphSetting::Disabled => {
            return Ok(json!({"engine": "disabled", "status": "disabled", "documents": 0}));
        }
        GraphSetting::Grafeo => {
            let path = sidecar(database, setting)?;
            if !path.exists() {
                return Ok(json!({"engine": "grafeo", "status": "pending", "documents": 0}));
            }
            let graph = GrafeoDB::open(path).map_err(graph_error)?;
            let count = graph
                .session()
                .execute("MATCH (d:Document) RETURN count(d)")
                .map_err(graph_error)?
                .scalar::<i64>()
                .map_err(graph_error)?;
            graph.close().map_err(graph_error)?;
            usize::try_from(count).unwrap_or_default()
        }
        GraphSetting::Surrealdb => {
            let path = sidecar(database, setting)?;
            if !path.exists() {
                return Ok(json!({"engine": "surrealdb", "status": "pending", "documents": 0}));
            }
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| AppError::new("surrealdb_open_failed", error.to_string()))?;
            runtime.block_on(async {
                let graph = open_surreal(database).await?;
                let mut response = graph
                    .query("SELECT count() AS count FROM document GROUP ALL")
                    .await
                    .map_err(surreal_error)?
                    .check()
                    .map_err(surreal_error)?;
                let count: Option<CountRow> = response.take(0).map_err(surreal_error)?;
                Ok::<usize, AppError>(count.map_or(0, |row| row.count))
            })?
        }
        GraphSetting::Inherit => unreachable!("effective graph setting"),
    };
    Ok(json!({
        "engine": setting_name(setting),
        "status": "ready",
        "documents": documents,
    }))
}

pub(crate) fn hook_status(scope: &str, database: &Path, deadline: Instant) -> Result<Value> {
    ensure_graph_hook_deadline(deadline)?;
    let setting = config::resolve(scope, database);
    ensure_graph_hook_deadline(deadline)?;
    let setting = setting.map_err(|_| graph_hook_unavailable())?.setting;
    if setting == GraphSetting::Disabled {
        return Ok(json!({"engine": "disabled", "status": "disabled", "documents": 0}));
    }

    let path = sidecar(database, setting);
    ensure_graph_hook_deadline(deadline)?;
    let path = path.map_err(|_| graph_hook_unavailable())?;
    let metadata = fs::symlink_metadata(&path);
    ensure_graph_hook_deadline(deadline)?;
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({
                "engine": setting_name(setting),
                "status": "pending",
                "documents": 0,
            }));
        }
        Err(_) => return Err(graph_hook_unavailable()),
    };
    if metadata.file_type().is_symlink() {
        return Err(graph_hook_unavailable());
    }

    match setting {
        GraphSetting::Grafeo => {
            if metadata.is_dir() {
                return Ok(json!({
                    "engine": "grafeo",
                    "status": "ready",
                    "verification": "passive",
                }));
            }
            if !metadata.is_file() {
                return Err(graph_hook_unavailable());
            }
            let config = grafeo_hook_config(&path, deadline)?;
            let graph = GrafeoDB::with_config(config);
            ensure_graph_hook_deadline(deadline)?;
            let graph = graph.map_err(|_| graph_hook_unavailable())?;
            let count = graph
                .session()
                .execute("MATCH (d:Document) RETURN count(d)")
                .and_then(|result| result.scalar::<i64>());
            ensure_graph_hook_deadline(deadline)?;
            let count = count.map_err(|_| graph_hook_unavailable())?;
            let closed = graph.close();
            ensure_graph_hook_deadline(deadline)?;
            closed.map_err(|_| graph_hook_unavailable())?;
            let documents = usize::try_from(count).map_err(|_| graph_hook_unavailable())?;
            Ok(json!({
                "engine": "grafeo",
                "status": "ready",
                "documents": documents,
            }))
        }
        GraphSetting::Surrealdb => {
            if !metadata.is_dir() {
                return Err(graph_hook_unavailable());
            }
            Ok(json!({"engine": "surrealdb", "status": "unverified"}))
        }
        GraphSetting::Disabled | GraphSetting::Inherit => unreachable!("effective graph setting"),
    }
}

fn grafeo_hook_config(path: &Path, deadline: Instant) -> Result<GrafeoConfig> {
    ensure_graph_hook_deadline(deadline)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(graph_hook_timeout());
    }
    Ok(GrafeoConfig::read_only(path).with_query_timeout(remaining))
}

fn ensure_graph_hook_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() >= deadline {
        Err(graph_hook_timeout())
    } else {
        Ok(())
    }
}

fn graph_hook_timeout() -> AppError {
    AppError::new(
        "graph_hook_timeout",
        "graph Hook status exceeded its fixed deadline",
    )
}

fn graph_hook_unavailable() -> AppError {
    AppError::new(
        "graph_hook_unavailable",
        "graph Hook status could not verify read-only state",
    )
}

pub fn passive_status(scope: &str, database: &Path) -> Result<Value> {
    let setting = config::resolve(scope, database)?.setting;
    if setting == GraphSetting::Disabled {
        return Ok(json!({"engine": "disabled", "status": "disabled"}));
    }
    Ok(json!({
        "engine": setting_name(setting),
        "status": if sidecar(database, setting)?.exists() { "ready" } else { "pending" },
    }))
}

pub fn node(scope: &str, database: &Path, identifier: &str) -> Result<Value> {
    let documents = load_projected_documents(scope, database)?;
    let document = documents
        .iter()
        .find(|document| document.key == identifier)
        .ok_or_else(|| {
            AppError::new(
                "graph_node_not_found",
                format!("graph node {identifier:?} was not found"),
            )
        })?;
    let edges = projected_edges(&documents);
    Ok(json!({
        "scope": scope,
        "node": node_value(document, None),
        "outgoing_degree": edges.iter().filter(|edge| edge.0 == identifier).count(),
        "incoming_degree": edges.iter().filter(|edge| edge.1 == identifier).count(),
    }))
}

pub fn explore(
    scope: &str,
    database: &Path,
    identifier: &str,
    depth: usize,
    limit: usize,
    direction: &str,
    edge_types: &[String],
) -> Result<Value> {
    let documents = load_projected_documents(scope, database)?;
    let by_key = documents
        .iter()
        .map(|document| (document.key.as_str(), document))
        .collect::<BTreeMap<_, _>>();
    if !by_key.contains_key(identifier) {
        return Err(AppError::new(
            "graph_node_not_found",
            format!("graph node {identifier:?} was not found"),
        ));
    }
    let edges = projected_edges(&documents)
        .into_iter()
        .filter(|edge| {
            edge_types.is_empty()
                || edge_types
                    .iter()
                    .any(|edge_type| edge_type.eq_ignore_ascii_case(&edge.2))
        })
        .collect::<Vec<_>>();
    let mut seen = BTreeMap::from([(identifier.to_string(), 0_usize)]);
    let mut queue = VecDeque::from([(identifier.to_string(), 0_usize)]);
    while let Some((current, current_depth)) = queue.pop_front() {
        if current_depth >= depth || seen.len() >= limit {
            continue;
        }
        for (from, to, _) in &edges {
            let next = match direction {
                "outgoing" if from == &current => Some(to),
                "incoming" if to == &current => Some(from),
                "both" if from == &current => Some(to),
                "both" if to == &current => Some(from),
                _ => None,
            };
            if let Some(next) = next
                && seen.len() < limit
                && !seen.contains_key(next)
            {
                seen.insert(next.clone(), current_depth + 1);
                queue.push_back((next.clone(), current_depth + 1));
            }
        }
    }
    let nodes = seen
        .iter()
        .filter_map(|(key, depth)| {
            by_key
                .get(key.as_str())
                .map(|node| node_value(node, Some(*depth)))
        })
        .collect::<Vec<_>>();
    let visible = seen.keys().cloned().collect::<BTreeSet<_>>();
    let edges = edges
        .into_iter()
        .filter(|(from, to, _)| visible.contains(from) && visible.contains(to))
        .map(|(from, to, edge_type)| json!({"from": from, "to": to, "type": edge_type}))
        .collect::<Vec<_>>();
    Ok(json!({
        "scope": scope,
        "start": node_value(by_key[identifier], Some(0)),
        "nodes": nodes,
        "edges": edges,
        "depth": depth,
        "limit": limit,
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn path(
    scope: &str,
    database: &Path,
    from: &str,
    to: &str,
    max_depth: usize,
    limit: usize,
    direction: &str,
    edge_types: &[String],
) -> Result<Value> {
    let explored = explore(
        scope, database, from, max_depth, limit, direction, edge_types,
    )?;
    let edges = explored["edges"].as_array().cloned().unwrap_or_default();
    let mut queue = VecDeque::from([from.to_string()]);
    let mut previous = BTreeMap::<String, String>::new();
    while let Some(current) = queue.pop_front() {
        if current == to {
            break;
        }
        for edge in &edges {
            let edge_from = edge["from"].as_str().unwrap_or_default();
            let edge_to = edge["to"].as_str().unwrap_or_default();
            let next = match direction {
                "outgoing" if edge_from == current => Some(edge_to),
                "incoming" if edge_to == current => Some(edge_from),
                "both" if edge_from == current => Some(edge_to),
                "both" if edge_to == current => Some(edge_from),
                _ => None,
            };
            if let Some(next) = next
                && next != from
                && !previous.contains_key(next)
            {
                previous.insert(next.to_string(), current.clone());
                queue.push_back(next.to_string());
            }
        }
    }
    let mut nodes = Vec::new();
    if from == to || previous.contains_key(to) {
        let mut current = to.to_string();
        nodes.push(current.clone());
        while current != from {
            current = previous[&current].clone();
            nodes.push(current.clone());
        }
        nodes.reverse();
    }
    Ok(json!({"scope": scope, "from": from, "to": to, "found": !nodes.is_empty(), "path": nodes}))
}

pub fn overview(scope: &str, database: &Path, limit: usize) -> Result<Value> {
    let documents = load_projected_documents(scope, database)?;
    let edges = projected_edges(&documents);
    let mut degree = BTreeMap::<String, usize>::new();
    for (from, to, _) in &edges {
        *degree.entry(from.clone()).or_default() += 1;
        *degree.entry(to.clone()).or_default() += 1;
    }
    let mut hubs = degree.into_iter().collect::<Vec<_>>();
    hubs.sort_by_key(|(key, count)| (std::cmp::Reverse(*count), key.clone()));
    hubs.truncate(limit);
    Ok(json!({
        "scope": scope,
        "documents": documents.len(),
        "edges": edges.len(),
        "hubs": hubs.into_iter().map(|(identifier, degree)| json!({"identifier": identifier, "degree": degree})).collect::<Vec<_>>(),
    }))
}

pub fn verify(scope: &str, database: &Path) -> Result<Value> {
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut expected = BTreeMap::new();
    for (kind, identifier) in document_keys(&conn)? {
        if let Some(document) = load_document(&conn, &kind, &identifier)? {
            expected.insert((kind, identifier), document.fingerprint);
        }
    }
    let projected = load_projected_documents(scope, database)?
        .into_iter()
        .map(|document| ((document.kind, document.identifier), document.fingerprint))
        .collect::<BTreeMap<_, _>>();
    let expected_keys = expected.keys().cloned().collect::<BTreeSet<_>>();
    let projected_keys = projected.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected_keys
        .difference(&projected_keys)
        .cloned()
        .collect::<Vec<_>>();
    let stale = projected_keys
        .difference(&expected_keys)
        .cloned()
        .collect::<Vec<_>>();
    let mismatched = expected
        .iter()
        .filter(|(key, fingerprint)| {
            projected
                .get(*key)
                .is_some_and(|value| value != *fingerprint)
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    Ok(json!({
        "scope": scope,
        "ok": missing.is_empty() && stale.is_empty() && mismatched.is_empty(),
        "missing": missing,
        "stale": stale,
        "mismatched": mismatched,
    }))
}

fn node_value(document: &ProjectionDocument, depth: Option<usize>) -> Value {
    let mut value = json!({
        "identifier": document.key,
        "type": document.kind,
        "label": document.title,
        "fingerprint": document.fingerprint,
    });
    if let Some(depth) = depth {
        value["depth"] = json!(depth);
    }
    value
}

fn projected_edges(documents: &[ProjectionDocument]) -> Vec<(String, String, String)> {
    let keys = documents
        .iter()
        .map(|document| document.key.as_str())
        .collect::<BTreeSet<_>>();
    documents
        .iter()
        .flat_map(|document| {
            document
                .links
                .iter()
                .map(|slug| (format!("page:{slug}"), "LINKS_TO".to_string()))
                .chain(
                    document
                        .source_ids
                        .iter()
                        .map(|id| (format!("source:{id}"), "CITES".to_string())),
                )
                .chain(
                    document
                        .relations
                        .iter()
                        .map(|relation| (relation.to.clone(), relation.relation_type.clone())),
                )
                .filter(|(to, _)| keys.contains(to.as_str()))
                .map(|(to, edge_type)| (document.key.clone(), to, edge_type))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn load_projected_documents(scope: &str, database: &Path) -> Result<Vec<ProjectionDocument>> {
    let setting = config::resolve(scope, database)?.setting;
    match setting {
        GraphSetting::Disabled => Err(AppError::new("graph_disabled", "graph is disabled")),
        GraphSetting::Grafeo => {
            let graph = GrafeoDB::open(sidecar(database, setting)?).map_err(graph_error)?;
            let documents = load_grafeo_documents(&graph)?;
            graph.close().map_err(graph_error)?;
            Ok(documents)
        }
        GraphSetting::Surrealdb => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| AppError::new("surrealdb_open_failed", error.to_string()))?;
            runtime.block_on(async {
                let graph = open_surreal(database).await?;
                let mut response = graph
                    .query("SELECT key, kind, identifier, title, fingerprint, links, source_ids, relations FROM document ORDER BY key")
                    .await
                    .map_err(surreal_error)?
                    .check()
                    .map_err(surreal_error)?;
                response.take(0).map_err(surreal_error)
            })
        }
        GraphSetting::Inherit => unreachable!("effective graph setting"),
    }
}

fn load_grafeo_documents(graph: &GrafeoDB) -> Result<Vec<ProjectionDocument>> {
    let result = graph
        .session()
        .execute(
            "MATCH (d:Document) RETURN d.key, d.kind, d.identifier, d.title, d.fingerprint, d.links, d.source_ids, d.relations ORDER BY d.key",
        )
        .map_err(graph_error)?;
    result
        .rows()
        .iter()
        .map(|row| {
            let value = |index: usize| match row.get(index) {
                Some(GrafeoValue::String(value)) => Ok(value.to_string()),
                _ => Err(AppError::new("grafeo_error", "invalid projected document")),
            };
            Ok(ProjectionDocument {
                key: value(0)?,
                kind: value(1)?,
                identifier: value(2)?,
                title: value(3)?,
                fingerprint: value(4)?,
                links: serde_json::from_str(&value(5)?).unwrap_or_default(),
                source_ids: serde_json::from_str(&value(6)?).unwrap_or_default(),
                relations: serde_json::from_str(&value(7)?).unwrap_or_default(),
            })
        })
        .collect()
}

fn document_keys(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut keys = Vec::new();
    {
        let mut statement = conn.prepare(
            "SELECT s.id FROM sources s
             WHERE NOT EXISTS(
                       SELECT 1 FROM source_path_revisions any_revision
                       WHERE any_revision.source_id = s.id
                   )
                OR EXISTS(
                       SELECT 1 FROM source_path_revisions head
                       WHERE head.source_id = s.id
                         AND head.revision = (
                             SELECT MAX(latest.revision)
                             FROM source_path_revisions latest
                             WHERE latest.tracked_path = head.tracked_path
                         )
                   )
             ORDER BY s.id",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
        for row in rows {
            keys.push(("source".to_string(), row?.to_string()));
        }
    }
    {
        let mut statement = conn.prepare("SELECT slug FROM pages ORDER BY slug")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            keys.push(("page".to_string(), row?));
        }
    }
    Ok(keys)
}

fn load_document(
    conn: &Connection,
    kind: &str,
    identifier: &str,
) -> Result<Option<ProjectionDocument>> {
    let (title, body, links, source_ids) = if kind == "page" {
        let document = conn.query_row(
            "SELECT title, body FROM pages WHERE slug = ?1",
            [identifier],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        let (title, body) = match document {
            Ok(document) => document,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut statement =
            conn.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        let links = statement
            .query_map([identifier], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut statement = conn.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        let source_ids = statement
            .query_map([identifier], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (title, body, links, source_ids)
    } else {
        let id = identifier.parse::<i64>().map_err(|_| {
            AppError::new(
                "graph_document_invalid",
                "source identifier is not an integer",
            )
        })?;
        let current: bool = conn.query_row(
            "SELECT NOT EXISTS(
                        SELECT 1 FROM source_path_revisions any_revision
                        WHERE any_revision.source_id = ?1
                    )
                 OR EXISTS(
                        SELECT 1 FROM source_path_revisions head
                        WHERE head.source_id = ?1
                          AND head.revision = (
                              SELECT MAX(latest.revision)
                              FROM source_path_revisions latest
                              WHERE latest.tracked_path = head.tracked_path
                          )
                    )",
            [id],
            |row| row.get(0),
        )?;
        if !current {
            return Ok(None);
        }
        let document = conn.query_row(
            "SELECT title, origin, content FROM sources WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        );
        let (title, origin, body) = match document {
            Ok(document) => document,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        (title.unwrap_or(origin), body, Vec::new(), Vec::new())
    };
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update(identifier.as_bytes());
    digest.update(title.as_bytes());
    digest.update(body.as_bytes());
    for link in &links {
        digest.update(link.as_bytes());
    }
    for source_id in &source_ids {
        digest.update(source_id.to_be_bytes());
    }
    let mut statement = conn.prepare(
        "SELECT relation_type, to_identifier FROM semantic_relations
         WHERE from_identifier = ?1
            OR from_identifier IN (
                SELECT span_id FROM search_spans
                WHERE document_type = ?2 AND document_identifier = ?3 AND active = 1
            )
         ORDER BY relation_type, to_identifier",
    )?;
    let relations = statement
        .query_map(
            [
                format!("{kind}:{identifier}"),
                kind.to_string(),
                identifier.to_string(),
            ],
            |row| {
                Ok(ProjectionRelation {
                    relation_type: row.get(0)?,
                    to: row.get(1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for relation in &relations {
        digest.update(relation.relation_type.as_bytes());
        digest.update(relation.to.as_bytes());
    }
    Ok(Some(ProjectionDocument {
        key: format!("{kind}:{identifier}"),
        kind: kind.to_string(),
        identifier: identifier.to_string(),
        title,
        fingerprint: digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        links,
        source_ids,
        relations,
    }))
}

fn replace_grafeo(
    graph: &GrafeoDB,
    kind: &str,
    identifier: &str,
    document: Option<&ProjectionDocument>,
) -> Result<()> {
    let session = graph.session();
    session.execute("START TRANSACTION").map_err(graph_error)?;
    let result = (|| {
        let key = format!("{kind}:{identifier}");
        session
            .execute_with_params(
                "MATCH (d:Document {key: $key}) DETACH DELETE d",
                [("key".to_string(), GrafeoValue::String(key.into()))]
                    .into_iter()
                    .collect(),
            )
            .map_err(graph_error)?;
        if let Some(document) = document {
            session
                .execute_with_params(
                    "INSERT (:Document {key: $key, kind: $kind, identifier: $identifier, title: $title, fingerprint: $fingerprint, links: $links, source_ids: $source_ids, relations: $relations})",
                    grafeo_params(document),
                )
                .map_err(graph_error)?;
        }
        session.execute("COMMIT").map_err(graph_error)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = session.execute("ROLLBACK");
    }
    result
}

fn grafeo_params(document: &ProjectionDocument) -> HashMap<String, GrafeoValue> {
    [
        ("key", document.key.clone()),
        ("kind", document.kind.clone()),
        ("identifier", document.identifier.clone()),
        ("title", document.title.clone()),
        ("fingerprint", document.fingerprint.clone()),
        (
            "links",
            serde_json::to_string(&document.links).unwrap_or_default(),
        ),
        (
            "source_ids",
            serde_json::to_string(&document.source_ids).unwrap_or_default(),
        ),
        (
            "relations",
            serde_json::to_string(&document.relations).unwrap_or_default(),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), GrafeoValue::String(value.into())))
    .collect()
}

async fn open_surreal(database: &Path) -> Result<Surreal<surrealdb::engine::local::Db>> {
    let graph = Surreal::new::<SurrealKv>(sidecar(database, GraphSetting::Surrealdb)?)
        .await
        .map_err(surreal_error)?;
    graph
        .use_ns("lwc")
        .use_db("graph")
        .await
        .map_err(surreal_error)?;
    graph
        .query("DEFINE TABLE IF NOT EXISTS document SCHEMALESS")
        .await
        .map_err(surreal_error)?
        .check()
        .map_err(surreal_error)?;
    Ok(graph)
}

async fn replace_surreal(
    graph: &Surreal<surrealdb::engine::local::Db>,
    kind: &str,
    identifier: &str,
    document: Option<&ProjectionDocument>,
) -> Result<()> {
    let key = format!("{kind}:{identifier}");
    if let Some(document) = document {
        graph
            .query(
                "BEGIN TRANSACTION;
                 DELETE document WHERE key = $key;
                 CREATE document CONTENT $document;
                 COMMIT TRANSACTION;",
            )
            .bind(("key", key))
            .bind(("document", document.clone()))
            .await
            .map_err(surreal_error)?
            .check()
            .map_err(surreal_error)?;
    } else {
        graph
            .query(
                "BEGIN TRANSACTION;
                 DELETE document WHERE key = $key;
                 COMMIT TRANSACTION;",
            )
            .bind(("key", key))
            .await
            .map_err(surreal_error)?
            .check()
            .map_err(surreal_error)?;
    }
    Ok(())
}

fn sidecar(database: &Path, setting: GraphSetting) -> Result<std::path::PathBuf> {
    Ok(
        crate::scope::require_database_runtime_root(database)?.join(match setting {
            GraphSetting::Grafeo => "graph-grafeo",
            GraphSetting::Surrealdb => "graph-surrealdb",
            GraphSetting::Disabled | GraphSetting::Inherit => "graph-disabled",
        }),
    )
}

fn setting_name(setting: GraphSetting) -> &'static str {
    match setting {
        GraphSetting::Disabled => "disabled",
        GraphSetting::Grafeo => "grafeo",
        GraphSetting::Surrealdb => "surrealdb",
        GraphSetting::Inherit => "inherit",
    }
}

fn graph_error(error: grafeo::Error) -> AppError {
    AppError::new("grafeo_error", error.to_string())
}

fn surreal_error(error: surrealdb::Error) -> AppError {
    AppError::new("surrealdb_error", error.to_string())
}

#[derive(Deserialize, SurrealValue)]
struct CountRow {
    count: usize,
}

#[cfg(test)]
mod hook_status_tests {
    use super::*;
    use crate::{config::ConfigPatch, store::Store};
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };
    use tempfile::TempDir;

    fn configured_database(setting: GraphSetting) -> (TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".lwc");
        let database = root.join("wiki.db");
        fs::create_dir(&root).unwrap();
        config::update(
            &database,
            ConfigPatch {
                graph: Some(GraphSetting::Disabled),
                ..ConfigPatch::default()
            },
        )
        .unwrap();
        drop(Store::initialize("project", &database).unwrap());
        config::update(
            &database,
            ConfigPatch {
                graph: Some(setting),
                ..ConfigPatch::default()
            },
        )
        .unwrap();
        (temp, database)
    }

    fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                snapshot.insert(relative, None);
                let mut entries = fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect::<Vec<_>>();
                entries.sort();
                for entry in entries {
                    visit(root, &entry, snapshot);
                }
            } else if metadata.is_file() && !metadata.file_type().is_symlink() {
                snapshot.insert(relative, Some(fs::read(path).unwrap()));
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn create_single_file_grafeo(database: &Path, temp: &TempDir) -> PathBuf {
        let staging = temp.path().join("fixture.grafeo");
        let graph = GrafeoDB::open(&staging).unwrap();
        let document = ProjectionDocument {
            key: "page:hook-fixture".to_owned(),
            kind: "page".to_owned(),
            identifier: "hook-fixture".to_owned(),
            title: "PRIVATE GRAPH TITLE".to_owned(),
            fingerprint: "f".repeat(64),
            links: Vec::new(),
            source_ids: Vec::new(),
            relations: Vec::new(),
        };
        replace_grafeo(&graph, "page", "hook-fixture", Some(&document)).unwrap();
        graph.close().unwrap();
        let target = sidecar(database, GraphSetting::Grafeo).unwrap();
        fs::rename(staging, &target).unwrap();
        target
    }

    #[test]
    fn hook_status_reads_single_file_grafeo_without_changing_the_runtime_tree() {
        let (temp, database) = configured_database(GraphSetting::Grafeo);
        let sidecar = create_single_file_grafeo(&database, &temp);
        let runtime = database.parent().unwrap();
        let before = snapshot_tree(runtime);

        let status = hook_status(
            "project",
            &database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(
            status,
            json!({"engine":"grafeo","status":"ready","documents":1})
        );
        assert_eq!(snapshot_tree(runtime), before);
        assert!(sidecar.is_file());
    }

    #[test]
    fn hook_status_keeps_surreal_passive_and_does_not_invent_a_count() {
        let (_temp, database) = configured_database(GraphSetting::Surrealdb);
        let sidecar = sidecar(&database, GraphSetting::Surrealdb).unwrap();
        fs::create_dir(&sidecar).unwrap();
        fs::write(
            sidecar.join("PRIVATE-SURREAL-DATA"),
            b"PRIVATE SURREAL BODY",
        )
        .unwrap();
        let runtime = database.parent().unwrap();
        let before = snapshot_tree(runtime);

        let status = hook_status(
            "project",
            &database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(status, json!({"engine":"surrealdb","status":"unverified"}));
        assert!(status.get("documents").is_none());
        assert_eq!(snapshot_tree(runtime), before);
    }

    #[test]
    fn hook_status_keeps_a_legacy_grafeo_directory_passive_and_unchanged() {
        let (temp, database) = configured_database(GraphSetting::Grafeo);
        let sidecar = sidecar(&database, GraphSetting::Grafeo).unwrap();
        let graph = GrafeoDB::open(&sidecar).unwrap();
        let document = ProjectionDocument {
            key: "page:legacy-hook-fixture".to_owned(),
            kind: "page".to_owned(),
            identifier: "legacy-hook-fixture".to_owned(),
            title: "PRIVATE LEGACY GRAPH TITLE".to_owned(),
            fingerprint: "f".repeat(64),
            links: Vec::new(),
            source_ids: Vec::new(),
            relations: Vec::new(),
        };
        replace_grafeo(&graph, "page", "legacy-hook-fixture", Some(&document)).unwrap();
        graph.close().unwrap();
        assert!(sidecar.is_dir());
        let runtime = database.parent().unwrap();
        let before = snapshot_tree(runtime);

        let status = hook_status(
            "project",
            &database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();

        assert_eq!(
            status,
            json!({"engine":"grafeo","status":"ready","verification":"passive"})
        );
        assert!(status.get("documents").is_none());
        assert!(!serde_json::to_string(&status).unwrap().contains("PRIVATE"));
        assert_eq!(snapshot_tree(runtime), before);
        assert!(!status.to_string().contains(temp.path().to_str().unwrap()));
    }

    #[test]
    fn hook_status_corrupt_grafeo_fails_open_without_paths_or_writes() {
        let (temp, database) = configured_database(GraphSetting::Grafeo);
        let sidecar = sidecar(&database, GraphSetting::Grafeo).unwrap();
        fs::write(&sidecar, b"PRIVATE CORRUPT GRAFEO").unwrap();
        let runtime = database.parent().unwrap();
        let before = snapshot_tree(runtime);

        let error = hook_status(
            "project",
            &database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap_err();

        assert_eq!(error.code, "graph_hook_unavailable");
        let rendered = error.to_string();
        assert!(!rendered.contains(temp.path().to_str().unwrap()));
        assert!(!rendered.contains("PRIVATE CORRUPT GRAFEO"));
        assert_eq!(snapshot_tree(runtime), before);
    }

    #[test]
    fn hook_status_rejects_a_surreal_sidecar_with_the_wrong_filesystem_type() {
        let (surreal_temp, surreal_database) = configured_database(GraphSetting::Surrealdb);
        let surreal_sidecar = sidecar(&surreal_database, GraphSetting::Surrealdb).unwrap();
        fs::write(&surreal_sidecar, b"PRIVATE WRONG SURREAL TYPE").unwrap();
        let surreal_before = snapshot_tree(surreal_database.parent().unwrap());

        let surreal_error = hook_status(
            "project",
            &surreal_database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap_err();

        assert_eq!(surreal_error.code, "graph_hook_unavailable");
        let rendered = surreal_error.to_string();
        assert!(!rendered.contains(surreal_temp.path().to_str().unwrap()));
        assert!(!rendered.contains("PRIVATE WRONG SURREAL TYPE"));
        assert_eq!(
            snapshot_tree(surreal_database.parent().unwrap()),
            surreal_before
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_status_rejects_a_symlinked_sidecar_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let (temp, database) = configured_database(GraphSetting::Grafeo);
        let outside = temp.path().join("PRIVATE-OUTSIDE-GRAPH");
        fs::write(&outside, b"PRIVATE OUTSIDE GRAPH BODY").unwrap();
        let before = fs::read(&outside).unwrap();
        symlink(&outside, sidecar(&database, GraphSetting::Grafeo).unwrap()).unwrap();

        let error = hook_status(
            "project",
            &database,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap_err();

        assert_eq!(error.code, "graph_hook_unavailable");
        let rendered = error.to_string();
        assert!(!rendered.contains(temp.path().to_str().unwrap()));
        assert!(!rendered.contains("PRIVATE OUTSIDE GRAPH"));
        assert_eq!(fs::read(outside).unwrap(), before);
    }

    #[test]
    fn hook_status_has_truthful_missing_and_expired_states() {
        let (_temp, database) = configured_database(GraphSetting::Grafeo);
        assert_eq!(
            hook_status(
                "project",
                &database,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap(),
            json!({"engine":"grafeo","status":"pending","documents":0})
        );
        assert_eq!(
            hook_status("project", &database, Instant::now())
                .unwrap_err()
                .code,
            "graph_hook_timeout"
        );
    }

    #[test]
    fn grafeo_hook_config_is_read_only_and_uses_only_the_remaining_budget() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.grafeo");
        let budget = Duration::from_millis(25);
        let config = grafeo_hook_config(&path, Instant::now() + budget).unwrap();

        assert_eq!(config.path.as_deref(), Some(path.as_path()));
        assert_eq!(config.access_mode, grafeo::AccessMode::ReadOnly);
        assert!(!config.wal_enabled);
        let timeout = config.query_timeout.unwrap();
        assert!(!timeout.is_zero());
        assert!(timeout <= budget);
        assert_eq!(
            grafeo_hook_config(&path, Instant::now()).unwrap_err().code,
            "graph_hook_timeout"
        );
    }
}
