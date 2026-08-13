use crate::{
    error::{AppError, Result},
    scope::StorePath,
    store::{Store, WordGraphOptions},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{process::Command, sync::Arc};

const INDEX_HTML: &str = include_str!("../../web/dist/index.html");
const APP_JS: &str = include_str!("../../web/dist/assets/app.js");
const APP_CSS: &str = include_str!("../../web/dist/assets/index.css");

#[derive(Clone)]
struct AppState {
    store: Arc<StorePath>,
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

#[derive(Deserialize)]
struct WordGraphQuery {
    query: String,
    #[serde(default = "default_word_document_limit")]
    limit: usize,
    #[serde(default = "default_word_term_limit")]
    term_limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize {
    100
}

fn default_word_document_limit() -> usize {
    25
}

fn default_word_term_limit() -> usize {
    30
}

type ApiResult = std::result::Result<Json<Value>, (StatusCode, Json<Value>)>;

pub fn run(store: StorePath, port: u16, no_open: bool) -> Result<Value> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| AppError::new("view_runtime_failed", error.to_string()))?;
    runtime.block_on(async move {
        let app = Router::new()
            .route("/", get(index))
            .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }))
            .route("/assets/app.js", get(app_js))
            .route("/assets/index.css", get(app_css))
            .route("/api/status", get(status))
            .route("/api/pages", get(pages))
            .route("/api/pages/{*slug}", get(page))
            .route("/api/sources", get(sources))
            .route("/api/graphs/knowledge", get(knowledge_graph))
            .route("/api/graphs/code", get(code_graph))
            .route("/api/graphs/words", get(word_graph))
            .with_state(AppState {
                store: Arc::new(store),
            });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| AppError::new("view_bind_failed", error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| AppError::new("view_bind_failed", error.to_string()))?;
        let url = format!("http://{address}");
        let browser_opened = !no_open && open_browser(&url);
        println!(
            "{}",
            serde_json::to_string(&json!({
                "url": url,
                "address": address,
                "read_only": true,
                "browser_opened": browser_opened,
            }))
            .expect("view startup response is serializable")
        );
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|error| AppError::new("view_server_failed", error.to_string()))?;
        Ok(json!({"stopped": true, "url": url}))
    })
}

async fn index() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; style-src-elem 'self' 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=' 'sha256-9xjtvxMT1ApHlgn9ohbh2FNfvK5Tqtzy94BjfXBeMSY=' 'sha256-yfc2FhpkFR0EAy3T+zDsaAFGXSP9B3ELNvaJKDzNhkk=' 'sha256-0/4q5IwejFb2zgHlQwwtwmGHS8ZbXE1kmz/TkRFlZ7M='; style-src-attr 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    (headers, Html(INDEX_HTML)).into_response()
}

async fn app_js() -> Response {
    asset("application/javascript; charset=utf-8", APP_JS)
}

async fn app_css() -> Response {
    asset("text/css; charset=utf-8", APP_CSS)
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response()
}

async fn status(State(state): State<AppState>) -> ApiResult {
    let store = open_read(&state)?;
    let identity = store.identity().map_err(api_error)?;
    Ok(Json(json!({
        "scope": "project",
        "database": state.store.path,
        "revision": identity.revision,
        "operation_id": identity.operation_id,
        "read_only": true,
    })))
}

async fn pages(State(state): State<AppState>, Query(query): Query<PageQuery>) -> ApiResult {
    bounded(&query)?;
    let store = open_read(&state)?;
    to_json(
        store
            .page_list(query.limit, query.offset)
            .map_err(api_error)?,
    )
}

async fn page(State(state): State<AppState>, Path(slug): Path<String>) -> ApiResult {
    let store = open_read(&state)?;
    to_json(
        store
            .page_show(slug.trim_start_matches('/'))
            .map_err(api_error)?,
    )
}

async fn sources(State(state): State<AppState>, Query(query): Query<PageQuery>) -> ApiResult {
    bounded(&query)?;
    let store = open_read(&state)?;
    to_json(
        store
            .source_list(query.limit, query.offset)
            .map_err(api_error)?,
    )
}

async fn knowledge_graph(State(state): State<AppState>) -> ApiResult {
    let store = open_read(&state)?;
    let pages = store.page_list(1000, 0).map_err(api_error)?.pages;
    let nodes = pages
        .iter()
        .map(|page| json!({"id": page.slug, "label": page.title, "kind": page.kind}))
        .collect::<Vec<_>>();
    let mut edges = Vec::new();
    for page in &pages {
        for target in store.page_links(&page.slug).map_err(api_error)?.outgoing {
            edges.push(json!({
                "id": format!("{}->{target}", page.slug),
                "source": page.slug,
                "target": target,
                "type": "LINKS_TO",
            }));
            if edges.len() == 5000 {
                break;
            }
        }
        if edges.len() == 5000 {
            break;
        }
    }
    Ok(Json(json!({
        "available": true,
        "nodes": nodes,
        "edges": edges,
        "limits": {"nodes": 1000, "edges": 5000},
    })))
}

async fn code_graph(State(state): State<AppState>) -> ApiResult {
    Ok(Json(crate::codegraph::graph(&state.store)))
}

async fn word_graph(
    State(state): State<AppState>,
    Query(query): Query<WordGraphQuery>,
) -> ApiResult {
    let store = open_read(&state)?;
    let graph = store
        .word_graph(
            &query.query,
            &WordGraphOptions {
                document_limit: query.limit,
                term_limit: query.term_limit,
                offset: query.offset,
            },
        )
        .map_err(api_error)?;
    let nodes = graph
        .documents
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "type": "document",
                "document_type": node.document_type,
                "identifier": node.identifier,
                "kind": node.kind,
            })
        })
        .chain(graph.terms.iter().map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "type": "term",
                "sample_document_frequency": node.sample_document_frequency,
                "sample_occurrences": node.sample_occurrences,
            })
        }))
        .collect::<Vec<_>>();
    let edges = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "id": edge.id,
                "source": edge.term,
                "target": edge.document,
                "type": "SAMPLE_CONTAINS",
                "sample_occurrences": edge.sample_occurrences,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "available": true,
        "query": graph.query,
        "query_terms": graph.query_terms,
        "nodes": nodes,
        "edges": edges,
        "has_more": graph.has_more,
        "truncated": graph.truncated,
        "truncation_reasons": graph.truncation_reasons,
        "limits": graph.limits,
        "diagnostics": graph.diagnostics,
    })))
}

fn to_json(value: impl serde::Serialize) -> ApiResult {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|error| api_error(AppError::new("serialization_error", error.to_string())))
}

fn bounded(query: &PageQuery) -> std::result::Result<(), (StatusCode, Json<Value>)> {
    if !(1..=1000).contains(&query.limit) {
        return Err(api_error(AppError::new(
            "invalid_limit",
            "limit must be between 1 and 1000",
        )));
    }
    Ok(())
}

fn open_read(state: &AppState) -> std::result::Result<Store, (StatusCode, Json<Value>)> {
    Store::open_for_read("project", &state.store.path).map_err(api_error)
}

fn api_error(error: AppError) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": {"code": error.code, "message": error.message}})),
    )
}

fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command.spawn().is_ok()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
