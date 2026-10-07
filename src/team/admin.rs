use super::{
    control,
    server::{HttpError, Shared, database, session},
};
use crate::error::{AppError, Result};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Browse {
    view: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    offset: u32,
}
fn rows(conn: &Connection, sql: &str, user: &str, scope: &str, offset: u32) -> Result<Vec<Value>> {
    let mut statement = conn.prepare(sql)?;
    let names = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rows = statement
        .query_map(params![user, scope, offset], |row| {
            let mut result = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                let value = match row.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    _ => Value::Null,
                };
                result.insert(name.clone(), value);
            }
            Ok(Value::Object(result))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
pub(super) async fn browse(
    State(state): State<Shared>,
    headers: HeaderMap,
    Query(input): Query<Browse>,
) -> std::result::Result<Json<Value>, HttpError> {
    let secret = session(&headers)?;
    Ok(Json(database(&state,move|conn|{
        let user=control::session_user(conn,&secret)?;
        if input.view=="policy" {
            control::authorize(conn,&user,&input.scope,"manager")?;
            let member=input.user.as_deref().ok_or_else(||AppError::new("invalid_request","policy user is required"))?;
            control::authorize(conn,member,&input.scope,"viewer")?;
            return Ok(json!({"rows":super::policy::denials(conn,member,&input.scope)?,"has_more":false}));
        }
        // Fixed statements only. Never expose session, invitation, or credential tables.
        let sql=match input.view.as_str(){
            "keys"=>"SELECT key_hash AS id,name,expires_at,revoked,created_at FROM personal_keys WHERE (user_id=?1 OR issued_by=?1) AND ?2='' ORDER BY created_at DESC,key_hash LIMIT 100 OFFSET ?3",
            "trash"=>"SELECT t.id,t.name,'team' AS kind,t.revision,t.deleted_at FROM teams t JOIN memberships m ON m.team_id=t.id WHERE t.archived=1 AND m.user_id=?1 AND m.role='owner' AND ?2='' UNION ALL SELECT s.id,s.name,'space' AS kind,s.revision,s.deleted_at FROM spaces s LEFT JOIN teams t ON t.id=s.team_id JOIN space_grants g ON g.space_id=s.id WHERE s.archived=1 AND COALESCE(t.archived,0)=0 AND g.user_id=?1 AND g.role='manager' AND ?2='' AND (s.user_owner=?1 OR EXISTS(SELECT 1 FROM memberships m WHERE m.team_id=s.team_id AND m.user_id=?1)) ORDER BY deleted_at DESC,id LIMIT 100 OFFSET ?3",
            "teams"=>"SELECT t.id,t.name,t.revision,m.role FROM teams t JOIN memberships m ON m.team_id=t.id WHERE m.user_id=?1 AND t.archived=0 AND (?2='' OR t.id=?2) ORDER BY t.name,t.id LIMIT 100 OFFSET ?3",
            "projects"=>"SELECT p.id,p.name,p.team_id,p.revision FROM projects p JOIN memberships m ON m.team_id=p.team_id WHERE m.user_id=?1 AND EXISTS(SELECT 1 FROM teams t WHERE t.id=p.team_id AND t.archived=0) AND (?2='' OR p.team_id=?2) ORDER BY p.name,p.id LIMIT 100 OFFSET ?3",
            "collections"=>"SELECT p.id,p.name,p.team_id,p.revision FROM collections p JOIN memberships m ON m.team_id=p.team_id WHERE m.user_id=?1 AND EXISTS(SELECT 1 FROM teams t WHERE t.id=p.team_id AND t.archived=0) AND (?2='' OR p.team_id=?2) ORDER BY p.name,p.id LIMIT 100 OFFSET ?3",
            "project_spaces"=>"SELECT s.id,s.name FROM project_spaces ps JOIN spaces s ON s.id=ps.space_id JOIN space_grants g ON g.space_id=s.id JOIN projects p ON p.id=ps.project_id JOIN memberships m ON m.team_id=p.team_id AND m.user_id=g.user_id WHERE g.user_id=?1 AND s.archived=0 AND EXISTS(SELECT 1 FROM teams t WHERE t.id=s.team_id AND t.archived=0) AND ps.project_id=?2 ORDER BY s.name,s.id LIMIT 100 OFFSET ?3",
            "collection_projects"=>"SELECT p.id,p.name FROM collection_projects cp JOIN projects p ON p.id=cp.project_id JOIN memberships m ON m.team_id=p.team_id WHERE m.user_id=?1 AND EXISTS(SELECT 1 FROM teams t WHERE t.id=p.team_id AND t.archived=0) AND cp.collection_id=?2 ORDER BY p.name,p.id LIMIT 100 OFFSET ?3",
            "space_members"=>{control::authorize(conn,&user,&input.scope,"manager")?;"SELECT m.user_id,COALESCE(NULLIF(p.nickname,''),(SELECT subject FROM identities i WHERE i.user_id=m.user_id AND i.provider='email' LIMIT 1),'') AS name,g.role FROM spaces s JOIN memberships m ON m.team_id=s.team_id LEFT JOIN user_profiles p ON p.user_id=m.user_id LEFT JOIN space_grants g ON g.space_id=s.id AND g.user_id=m.user_id WHERE s.id=?2 AND ?1<>'' ORDER BY name,m.user_id LIMIT 100 OFFSET ?3"},
            "members"=>"SELECT m.user_id,m.role,COALESCE(NULLIF(p.nickname,''),(SELECT subject FROM identities i WHERE i.user_id=m.user_id AND i.provider='email' LIMIT 1),'') AS name FROM memberships m LEFT JOIN user_profiles p ON p.user_id=m.user_id WHERE m.team_id=?2 AND EXISTS(SELECT 1 FROM memberships me WHERE me.team_id=m.team_id AND me.user_id=?1 AND me.role='owner' AND EXISTS(SELECT 1 FROM teams t WHERE t.id=m.team_id AND t.archived=0)) ORDER BY m.user_id LIMIT 100 OFFSET ?3",
            "devices"=>"SELECT id,metadata_json,registered_at,revoked FROM devices WHERE user_id=?1 AND (?2='' OR id=?2) ORDER BY registered_at DESC,id LIMIT 100 OFFSET ?3",
            "agents"=>"SELECT id,name,device_id,registered_at,revoked FROM agents WHERE user_id=?1 AND (?2='' OR id=?2) ORDER BY registered_at DESC,id LIMIT 100 OFFSET ?3",
            "identities"=>"SELECT provider,namespace,subject FROM identities WHERE user_id=?1 AND (?2='' OR provider=?2) ORDER BY provider,namespace,subject LIMIT 100 OFFSET ?3",
            "audit"=>"SELECT id,actor,action,target,created_at FROM control_audit WHERE actor=?1 AND (?2='' OR target=?2 OR target LIKE ?2||'/%') ORDER BY id DESC LIMIT 100 OFFSET ?3",
            "grants"=>{control::authorize(conn,&user,&input.scope,"manager")?;"SELECT g.user_id,g.role,COALESCE(NULLIF(p.nickname,''),(SELECT subject FROM identities i WHERE i.user_id=g.user_id AND i.provider='email' LIMIT 1),'') AS name FROM space_grants g LEFT JOIN user_profiles p ON p.user_id=g.user_id WHERE space_id=?2 AND ?1<>'' ORDER BY g.user_id LIMIT 100 OFFSET ?3"},
            "policies"=>{control::authorize(conn,&user,&input.scope,"manager")?;"SELECT user_id,kind,logical_key,action FROM memory_denials WHERE space_id=?2 AND ?1<>'' ORDER BY user_id,kind,logical_key,action LIMIT 100 OFFSET ?3"},
            "replicas"=>{control::authorize(conn,&user,&input.scope,"viewer")?;"SELECT id,user_id,device,revoked,ack_head,last_seen,pending_conflicts,CASE WHEN revoked=1 THEN 'revoked' WHEN last_seen<unixepoch()-120 THEN 'offline' ELSE sync_status END AS status FROM replicas WHERE space_id=?2 AND (user_id=?1 OR EXISTS(SELECT 1 FROM space_grants WHERE space_id=?2 AND user_id=?1 AND role='manager')) ORDER BY id LIMIT 100 OFFSET ?3"},
            _=>return Err(AppError::new("invalid_view","unknown administration view")),
        };
        let result=rows(conn,sql,&user,&input.scope,input.offset)?;
        Ok(json!({"rows":result,"offset":input.offset,"limit":100,"has_more":result.len()==100}))
    }).await?))
}

pub(super) async fn index(
    State(state): State<Shared>,
) -> std::result::Result<axum::response::Response, HttpError> {
    static_file(&state, "index.html").await
}
pub(super) async fn asset(
    State(state): State<Shared>,
    axum::extract::Path(path): axum::extract::Path<String>,
) -> std::result::Result<axum::response::Response, HttpError> {
    if path.contains('/')
        || path.contains('\\')
        || !path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(AppError::new("invalid_asset", "invalid asset path").into());
    }
    static_file(&state, &format!("assets/{path}")).await
}
async fn static_file(
    state: &Shared,
    path: &str,
) -> std::result::Result<axum::response::Response, HttpError> {
    use axum::response::IntoResponse;
    let root = state.config.admin_assets.as_ref().ok_or_else(|| {
        AppError::new(
            "provider_unavailable",
            "build admin/ and configure admin_assets",
        )
    })?;
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|_| AppError::new("provider_unavailable", "admin assets unavailable"))?;
    let file = tokio::fs::canonicalize(root.join(path))
        .await
        .map_err(|_| AppError::new("invalid_asset", "asset unavailable"))?;
    if !file.starts_with(&root)
        || tokio::fs::metadata(&file)
            .await
            .map_err(AppError::from)?
            .len()
            > 4 * 1024 * 1024
    {
        return Err(AppError::new("invalid_asset", "invalid asset").into());
    }
    let mime = match file.extension().and_then(|v| v.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        _ => return Err(AppError::new("invalid_asset", "unsupported asset").into()),
    };
    let body = tokio::fs::read(file).await.map_err(AppError::from)?;
    Ok(([(axum::http::header::CONTENT_TYPE,mime),(axum::http::header::CACHE_CONTROL,"no-store"),(axum::http::header::X_CONTENT_TYPE_OPTIONS,"nosniff"),(axum::http::header::REFERRER_POLICY,"no-referrer"),(axum::http::header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")],body).into_response())
}
