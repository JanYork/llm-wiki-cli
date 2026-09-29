use crate::error::{AppError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

pub(super) fn token() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| AppError::new("entropy_unavailable", "secure randomness unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const REPLICA_STATUS_SCHEMA: &str = "ALTER TABLE replicas ADD COLUMN sync_status TEXT NOT NULL DEFAULT 'unknown'; ALTER TABLE replicas ADD COLUMN pending_conflicts INTEGER NOT NULL DEFAULT 0;";

pub(super) fn open(directory: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        directory.join("control.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if !(1..=6).contains(&version) {
        return Err(AppError::new(
            "team_schema_unsupported",
            "initialize the team control store or use a compatible server version",
        ));
    }
    if version < 6 {
        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let migrated = (|| -> Result<()> {
            let locked: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if !(1..=6).contains(&locked) {
                return Err(AppError::new(
                    "team_schema_unsupported",
                    "control schema changed during migration",
                ));
            }
            if locked < 2 {
                conn.execute_batch(include_str!("identity.sql"))?;
            }
            if locked < 3 {
                conn.execute_batch(include_str!("policy.sql"))?;
            }
            if locked < 4 {
                conn.execute_batch(include_str!("delegation.sql"))?;
            }
            if locked < 5 {
                conn.execute_batch(REPLICA_STATUS_SCHEMA)?;
            }
            if locked < 6 {
                conn.execute_batch(include_str!("keys.sql"))?;
            }
            conn.pragma_update(None, "user_version", 6)?;
            conn.execute_batch("COMMIT;")?;
            Ok(())
        })();
        if migrated.is_err() {
            let _ = conn.execute_batch("ROLLBACK;");
        }
        migrated?;
    }
    Ok(conn)
}

pub(crate) fn initialize(directory: &Path, admin_email: &str, name: &str) -> Result<Value> {
    let email = normalize_email(admin_email)?;
    let name = label(name)?;
    super::private_directory(directory)?;
    let directory = &std::fs::canonicalize(directory)?;
    let mut conn = Connection::open(directory.join("control.db"))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
    )?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != 0 {
        return Err(AppError::new(
            "team_already_initialized",
            "team control store is already initialized; bootstrap does not replace administrators",
        ));
    }
    tx.execute_batch(include_str!("schema.sql"))?;
    tx.execute_batch(include_str!("identity.sql"))?;
    tx.execute_batch(include_str!("policy.sql"))?;
    tx.execute_batch(include_str!("delegation.sql"))?;
    let user = identity_user(&tx, "email", "", &email)?;
    tx.execute("UPDATE users SET administrator=1 WHERE id=?1", [&user])?;
    let team = create_team(&tx, &user, &name)?;
    tx.execute_batch(REPLICA_STATUS_SCHEMA)?;
    tx.execute_batch(include_str!("keys.sql"))?;
    tx.pragma_update(None, "user_version", 6)?;
    audit(&tx, &user, "bootstrap", &team)?;
    let token_file = super::access::initialize(directory)?;
    let key = super::keys::issue(&tx, &user, &user, "Initial administrator", 365)?;
    let key_file = directory.join("administrator.key");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = options.open(&key_file)?;
        file.write_all(key["personal_key"].as_str().unwrap().as_bytes())?;
        file.sync_all()?;
    }
    tx.commit()?;
    Ok(
        json!({"administrator_key_file":key_file,"server_token_file":token_file,"initialized":true,"user_id":user,"team_id":team,"data":directory}),
    )
}

pub(super) fn normalize_email(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    let (local, domain) = value
        .rsplit_once('@')
        .ok_or_else(|| AppError::new("invalid_email", "a valid email address is required"))?;
    if value.len() > 254
        || local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || local.contains('@')
    {
        return Err(AppError::new(
            "invalid_email",
            "a valid email address is required",
        ));
    }
    Ok(value)
}

fn label(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 160 || value.chars().any(char::is_control) {
        return Err(AppError::new(
            "invalid_label",
            "name must contain 1–160 printable characters",
        ));
    }
    Ok(value.to_owned())
}

pub(super) fn identity_user(
    tx: &Transaction<'_>,
    provider: &str,
    namespace: &str,
    subject: &str,
) -> Result<String> {
    if subject.is_empty() || subject.len() > 512 {
        return Err(AppError::new(
            "invalid_identity",
            "invalid identity subject",
        ));
    }
    if let Some(id) = tx
        .query_row(
            "SELECT user_id FROM identities WHERE provider=?1 AND namespace=?2 AND subject=?3",
            params![provider, namespace, subject],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let id = token()?;
    tx.execute("INSERT INTO users(id) VALUES(?1)", [&id])?;
    tx.execute(
        "INSERT INTO identities(provider,namespace,subject,user_id) VALUES(?1,?2,?3,?4)",
        params![provider, namespace, subject, id],
    )?;
    Ok(id)
}

pub(super) fn link_identity(
    tx: &Transaction<'_>,
    user: &str,
    provider: &str,
    namespace: &str,
    subject: &str,
) -> Result<()> {
    active(tx, user)?;
    let current: Option<String> = tx
        .query_row(
            "SELECT user_id FROM identities WHERE provider=?1 AND namespace=?2 AND subject=?3",
            params![provider, namespace, subject],
            |r| r.get(0),
        )
        .optional()?;
    if current.as_deref().is_some_and(|owner| owner != user) {
        return Err(AppError::new(
            "identity_in_use",
            "identity belongs to another account; accounts are not automatically merged",
        ));
    }
    tx.execute("INSERT INTO identities VALUES(?1,?2,?3,?4) ON CONFLICT(provider,namespace,subject) DO NOTHING",params![provider,namespace,subject,user])?;
    audit(tx, user, "identity.link", provider)
}

pub(super) fn audit(tx: &Transaction<'_>, actor: &str, action: &str, target: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO control_audit(actor,action,target) VALUES(?1,?2,?3)",
        params![actor, action, target],
    )?;
    Ok(())
}

fn create_team(tx: &Transaction<'_>, actor: &str, name: &str) -> Result<String> {
    active(tx, actor)?;
    let id = token()?;
    tx.execute(
        "INSERT INTO teams(id,name) VALUES(?1,?2)",
        params![id, label(name)?],
    )?;
    tx.execute(
        "INSERT INTO memberships(team_id,user_id,role) VALUES(?1,?2,'owner')",
        params![id, actor],
    )?;
    audit(tx, actor, "team.create", &id)?;
    Ok(id)
}

fn active(conn: &Connection, user: &str) -> Result<()> {
    let active = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1 AND disabled=0)",
        [user],
        |r| r.get::<_, bool>(0),
    )?;
    if !active {
        return Err(AppError::new("unauthorized", "account is not active"));
    }
    Ok(())
}

pub(super) fn authorize(conn: &Connection, user: &str, space: &str, required: &str) -> Result<()> {
    active(conn, user)?;
    let rank = match required {
        "viewer" => 1,
        "editor" => 2,
        "manager" => 3,
        _ => return Err(AppError::new("invalid_role", "unknown space role")),
    };
    let permitted=conn.query_row("SELECT EXISTS(
        SELECT 1 FROM spaces s JOIN space_grants g ON g.space_id=s.id
        WHERE s.id=?1 AND g.user_id=?2 AND s.archived=0
        AND (s.user_owner=?2 OR EXISTS(SELECT 1 FROM memberships m WHERE m.team_id=s.team_id AND m.user_id=?2))
        AND CASE g.role WHEN 'viewer' THEN 1 WHEN 'editor' THEN 2 WHEN 'manager' THEN 3 ELSE 0 END >= ?3
    )",params![space,user,rank],|r|r.get::<_,bool>(0))?;
    if !permitted {
        return Err(AppError::new("forbidden", "space access is not granted"));
    }
    Ok(())
}

fn team_owner(conn: &Connection, user: &str, team: &str) -> Result<()> {
    active(conn, user)?;
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM memberships WHERE team_id=?1 AND user_id=?2 AND role='owner')",
        params![team, user],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(AppError::new("forbidden", "team owner role required"));
    }
    Ok(())
}

pub(super) fn manage(conn: &mut Connection, actor: &str, input: &Value) -> Result<Value> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(&tx, actor)?;
    let text = |key: &str| {
        input[key]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| AppError::new("invalid_request", format!("{key} is required")))
    };
    let action = text("action")?;
    let result = match action {
        "key.create" => super::keys::issue(
            &tx,
            actor,
            actor,
            &label(text("name")?)?,
            input["days"].as_i64().unwrap_or(90),
        )?,
        "key.revoke" => {
            let id = text("key_id")?;
            if tx.execute("UPDATE personal_keys SET revoked=1 WHERE key_hash=?1 AND (user_id=?2 OR issued_by=?2)",params![id,actor])?!=1 {return Err(AppError::new("forbidden","key is not owned or issued by this user"));}
            audit(&tx, actor, action, id)?;
            json!({"revoked":true})
        }
        "member.create" => {
            let team = text("team_id")?;
            team_owner(&tx, actor, team)?;
            let name = label(text("name")?)?;
            let user = token()?;
            tx.execute("INSERT INTO users(id) VALUES(?1)", [&user])?;
            tx.execute(
                "INSERT INTO user_profiles(user_id,email_hint,nickname) VALUES(?1,'',?2)",
                params![user, name],
            )?;
            tx.execute(
                "INSERT INTO memberships VALUES(?1,?2,'member')",
                params![team, user],
            )?;
            // Initial grants and the new identity commit together or not at all.
            if let Some(grants) = input.get("grants") {
                let grants = grants
                    .as_array()
                    .filter(|g| g.len() <= 256)
                    .ok_or_else(|| {
                        AppError::new("invalid_request", "grants must contain at most 256 spaces")
                    })?;
                let mut seen = std::collections::HashSet::new();
                for grant in grants {
                    let space = grant["space_id"]
                        .as_str()
                        .ok_or_else(|| AppError::new("invalid_request", "space_id is required"))?;
                    let role = grant["role"].as_str().unwrap_or("");
                    let revision = grant["expected_revision"].as_i64().ok_or_else(|| {
                        AppError::new("revision_required", "expected_revision is required")
                    })?;
                    if !seen.insert(space) || !matches!(role, "viewer" | "editor" | "manager") {
                        return Err(AppError::new(
                            "invalid_request",
                            "duplicate space or invalid role",
                        ));
                    }
                    authorize(&tx, actor, space, "manager")?;
                    let same: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM spaces WHERE id=?1 AND team_id=?2)",
                        params![space, team],
                        |r| r.get(0),
                    )?;
                    if !same {
                        return Err(AppError::new(
                            "forbidden",
                            "initial space must belong to this team",
                        ));
                    }
                    if tx.execute(
                        "UPDATE spaces SET revision=revision+1 WHERE id=?1 AND revision=?2",
                        params![space, revision],
                    )? != 1
                    {
                        return Err(AppError::new(
                            "revision_conflict",
                            "space changed; reload before granting access",
                        ));
                    }
                    tx.execute(
                        "INSERT INTO space_grants VALUES(?1,?2,?3)",
                        params![space, user, role],
                    )?;
                    audit(&tx, actor, "space.grant", &format!("{space}/{user}"))?;
                }
            }
            tx.execute("UPDATE teams SET revision=revision+1 WHERE id=?1", [team])?;
            let key = super::keys::issue(
                &tx,
                &user,
                actor,
                &name,
                input["days"].as_i64().unwrap_or(90),
            )?;
            audit(&tx, actor, action, &user)?;
            key
        }
        "team.create" => json!({"id":create_team(&tx,actor,text("name")?)?,"revision":1}),
        "project.create" | "collection.create" => {
            let team = text("team_id")?;
            team_owner(&tx, actor, team)?;
            let id = token()?;
            let name = label(text("name")?)?;
            let table = if action == "project.create" {
                "projects"
            } else {
                "collections"
            };
            tx.execute(
                &format!("INSERT INTO {table}(id,team_id,name) VALUES(?1,?2,?3)"),
                params![id, team, name],
            )?;
            audit(&tx, actor, action, &id)?;
            json!({"id":id,"revision":1})
        }
        "project.spaces" | "collection.projects" => {
            let table = if action == "project.spaces" {
                "projects"
            } else {
                "collections"
            };
            let id = text("id")?;
            let team: String = tx.query_row(
                &format!("SELECT team_id FROM {table} WHERE id=?1"),
                [id],
                |r| r.get(0),
            )?;
            team_owner(&tx, actor, &team)?;
            let revision = input["expected_revision"].as_i64().ok_or_else(|| {
                AppError::new("revision_required", "expected_revision is required")
            })?;
            if tx.execute(
                &format!("UPDATE {table} SET revision=revision+1 WHERE id=?1 AND revision=?2"),
                params![id, revision],
            )? != 1
            {
                return Err(AppError::new(
                    "revision_conflict",
                    "directory changed; reload",
                ));
            }
            let items = input["items"]
                .as_array()
                .filter(|v| v.len() <= 256)
                .ok_or_else(|| {
                    AppError::new(
                        "invalid_request",
                        "items must be an array with at most 256 IDs",
                    )
                })?;
            let (join, left, right, target) = if action == "project.spaces" {
                ("project_spaces", "project_id", "space_id", "spaces")
            } else {
                (
                    "collection_projects",
                    "collection_id",
                    "project_id",
                    "projects",
                )
            };
            if action == "project.spaces" {
                let prior = tx
                    .prepare("SELECT space_id FROM project_spaces WHERE project_id=?1")?
                    .query_map([id], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for prior in prior {
                    authorize(&tx, actor, &prior, "viewer")?;
                }
            }
            tx.execute(&format!("DELETE FROM {join} WHERE {left}=?1"), [id])?;
            for item in items {
                let item = item
                    .as_str()
                    .ok_or_else(|| AppError::new("invalid_request", "item must be an ID"))?;
                let same: bool = tx.query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {target} WHERE id=?1 AND team_id=?2)"),
                    params![item, team],
                    |r| r.get(0),
                )?;
                if !same {
                    return Err(AppError::new(
                        "forbidden",
                        "directory entries must belong to this team",
                    ));
                }
                // Catalog membership never creates a memory grant.
                if action == "project.spaces" {
                    authorize(&tx, actor, item, "viewer")?;
                }
                tx.execute(
                    &format!("INSERT OR IGNORE INTO {join}({left},{right}) VALUES(?1,?2)"),
                    params![id, item],
                )?;
            }
            audit(&tx, actor, action, id)?;
            json!({"id":id,"revision":revision+1})
        }
        "invitation.create" => {
            let team = text("team_id")?;
            team_owner(&tx, actor, team)?;
            let invitation = token()?;
            tx.execute("INSERT INTO invitations(token_hash,team_id,email,expires_at) VALUES(?1,?2,?3,unixepoch()+604800)",params![digest(&invitation),team,normalize_email(text("email")?)?])?;
            audit(&tx, actor, action, team)?;
            json!({"invitation_token":invitation,"expires_in":604800})
        }
        "invitation.accept" => {
            let hash = digest(text("invitation_token")?);
            let team:Option<String>=tx.query_row("SELECT i.team_id FROM invitations i JOIN identities e ON e.provider='email' AND e.namespace='' AND e.subject=i.email WHERE i.token_hash=?1 AND i.expires_at>unixepoch() AND i.consumed_by IS NULL AND e.user_id=?2",params![hash,actor],|r|r.get(0)).optional()?;
            let team = team.ok_or_else(|| {
                AppError::new(
                    "invalid_invitation",
                    "invitation is invalid, expired, used, or addressed to another verified email",
                )
            })?;
            tx.execute("INSERT INTO memberships VALUES(?1,?2,'member') ON CONFLICT(team_id,user_id) DO NOTHING",params![team,actor])?;
            tx.execute("UPDATE teams SET revision=revision+1 WHERE id=?1", [&team])?;
            tx.execute(
                "UPDATE invitations SET consumed_by=?1 WHERE token_hash=?2",
                params![actor, hash],
            )?;
            audit(&tx, actor, action, &team)?;
            json!({"team_id":team,"joined":true})
        }
        "member.remove" => {
            let team = text("team_id")?;
            let user = text("user_id")?;
            team_owner(&tx, actor, team)?;
            let revision = input["expected_revision"].as_i64().ok_or_else(|| {
                AppError::new("revision_required", "expected_revision is required")
            })?;
            if tx.execute(
                "UPDATE teams SET revision=revision+1 WHERE id=?1 AND revision=?2",
                params![team, revision],
            )? != 1
            {
                return Err(AppError::new(
                    "revision_conflict",
                    "team changed; reload before changing membership",
                ));
            }
            let orphan=tx.query_row("SELECT EXISTS(SELECT 1 FROM spaces s JOIN space_grants g ON g.space_id=s.id WHERE s.team_id=?1 AND g.user_id=?2 AND g.role='manager' AND NOT EXISTS(SELECT 1 FROM space_grants other JOIN memberships m ON m.user_id=other.user_id AND m.team_id=s.team_id JOIN users u ON u.id=m.user_id WHERE other.space_id=s.id AND other.role='manager' AND other.user_id<>?2 AND u.disabled=0))",params![team,user],|r|r.get::<_,bool>(0))?;
            if orphan {
                return Err(AppError::new(
                    "last_manager",
                    "assign another space manager before removing this member",
                ));
            }
            tx.execute(
                "DELETE FROM memberships WHERE team_id=?1 AND user_id=?2",
                params![team, user],
            )?;
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM memberships WHERE team_id=?1 AND role='owner')",
                [team],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(AppError::new("last_owner", "team must retain an owner"));
            }
            // Rejoining never resurrects an old grant or device registration.
            tx.execute("UPDATE spaces SET revision=revision+1 WHERE team_id=?1 AND id IN(SELECT space_id FROM space_grants WHERE user_id=?2)",params![team,user])?;
            tx.execute("DELETE FROM space_grants WHERE user_id=?1 AND space_id IN(SELECT id FROM spaces WHERE team_id=?2)",params![user,team])?;
            tx.execute("UPDATE replicas SET revoked=1 WHERE user_id=?1 AND space_id IN(SELECT id FROM spaces WHERE team_id=?2)",params![user,team])?;
            audit(&tx, actor, action, &format!("{team}/{user}"))?;
            json!({"team_id":team,"revision":revision+1,"removed":true})
        }
        "identity.unlink" => {
            let provider = text("provider")?;
            let namespace = input["namespace"].as_str().unwrap_or("");
            let subject = text("subject")?;
            tx.execute("DELETE FROM identities WHERE user_id=?1 AND provider=?2 AND namespace=?3 AND subject=?4",params![actor,provider,namespace,subject])?;
            if !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM identities WHERE user_id=?1)",
                [actor],
                |r| r.get::<_, bool>(0),
            )? {
                return Err(AppError::new(
                    "last_identity",
                    "retain at least one login identity",
                ));
            }
            audit(&tx, actor, action, provider)?;
            json!({"unlinked":true})
        }
        "space.create" => {
            let team = input["team_id"].as_str();
            if let Some(team) = team {
                team_owner(&tx, actor, team)?;
            }
            let id = token()?;
            tx.execute(
                "INSERT INTO spaces(id,name,team_id,user_owner,epoch) VALUES(?1,?2,?3,?4,?5)",
                params![
                    id,
                    label(text("name")?)?,
                    team,
                    team.is_none().then_some(actor),
                    token()?
                ],
            )?;
            tx.execute(
                "INSERT INTO space_grants(space_id,user_id,role) VALUES(?1,?2,'manager')",
                params![id, actor],
            )?;
            audit(&tx, actor, action, &id)?;
            json!({"id":id,"revision":1})
        }
        "agent.revoke" | "device.revoke" => {
            let target = text("id")?;
            let changed = if action == "agent.revoke" {
                tx.execute(
                    "UPDATE agents SET revoked=1 WHERE user_id=?1 AND id=?2",
                    params![actor, target],
                )?
            } else {
                tx.execute(
                    "UPDATE devices SET revoked=1 WHERE user_id=?1 AND id=?2",
                    params![actor, target],
                )?
            };
            if changed != 1 {
                return Err(AppError::new(
                    "forbidden",
                    "identity is not owned by this user",
                ));
            }
            audit(&tx, actor, action, target)?;
            json!({"revoked":true,"id":target})
        }
        "space.policy" => {
            let space = text("space_id")?;
            let user = text("user_id")?;
            authorize(&tx, actor, space, "manager")?;
            authorize(&tx, user, space, "viewer")?;
            let revision = input["expected_revision"].as_i64().ok_or_else(|| {
                AppError::new("revision_required", "expected_revision is required")
            })?;
            let rules: Vec<super::policy::Denial> =
                serde_json::from_value(input["denials"].clone()).map_err(|_| {
                    AppError::new("invalid_policy", "denials must be a resource/action array")
                })?;
            if rules.len() > 256 {
                return Err(AppError::new(
                    "invalid_policy",
                    "at most 256 explicit restrictions per space member",
                ));
            }
            for rule in &rules {
                rule.validate()?;
            }
            if tx.execute(
                "UPDATE spaces SET revision=revision+1 WHERE id=?1 AND revision=?2",
                params![space, revision],
            )? != 1
            {
                return Err(AppError::new(
                    "revision_conflict",
                    "space changed; reload before changing permissions",
                ));
            }
            tx.execute(
                "DELETE FROM memory_denials WHERE space_id=?1 AND user_id=?2",
                params![space, user],
            )?;
            for rule in &rules {
                tx.execute(
                    "INSERT OR IGNORE INTO memory_denials VALUES(?1,?2,?3,?4,?5)",
                    params![space, user, rule.kind, rule.key, rule.action],
                )?;
            }
            audit(&tx, actor, action, space)?;
            json!({"space_id":space,"user_id":user,"revision":revision+1,"denials":rules})
        }
        "space.grant" | "space.revoke" => {
            let space = text("space_id")?;
            let user = text("user_id")?;
            authorize(&tx, actor, space, "manager")?;
            let revision = input["expected_revision"].as_i64().ok_or_else(|| {
                AppError::new("revision_required", "expected_revision is required")
            })?;
            if tx.execute(
                "UPDATE spaces SET revision=revision+1 WHERE id=?1 AND revision=?2",
                params![space, revision],
            )? != 1
            {
                return Err(AppError::new(
                    "revision_conflict",
                    "space changed; reload before changing permissions",
                ));
            }
            if action == "space.grant" {
                active(&tx, user)?;
                let member=tx.query_row("SELECT EXISTS(SELECT 1 FROM spaces s WHERE s.id=?1 AND (s.user_owner=?2 OR EXISTS(SELECT 1 FROM memberships m WHERE m.team_id=s.team_id AND m.user_id=?2)))",params![space,user],|r|r.get::<_,bool>(0))?;
                if !member {
                    return Err(AppError::new(
                        "forbidden",
                        "grant recipient must belong to the space owner",
                    ));
                }
                let role = text("role")?;
                if !matches!(role, "viewer" | "editor" | "manager") {
                    return Err(AppError::new("invalid_role", "unknown space role"));
                }
                tx.execute("INSERT INTO space_grants VALUES(?1,?2,?3) ON CONFLICT(space_id,user_id) DO UPDATE SET role=excluded.role",params![space,user,role])?;
            } else {
                tx.execute(
                    "DELETE FROM space_grants WHERE space_id=?1 AND user_id=?2",
                    params![space, user],
                )?;
            }
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM space_grants WHERE space_id=?1 AND role='manager'",
                [space],
                |r| r.get(0),
            )?;
            if count == 0 {
                return Err(AppError::new(
                    "last_manager",
                    "a space must retain a manager",
                ));
            }
            audit(&tx, actor, action, &format!("{space}/{user}"))?;
            json!({"id":space,"revision":revision+1})
        }
        _ => {
            return Err(AppError::new(
                "invalid_action",
                "unsupported management action",
            ));
        }
    };
    tx.commit()?;
    Ok(result)
}

pub(super) fn create_session(tx: &Transaction<'_>, user: &str) -> Result<String> {
    active(tx, user)?;
    let secret = token()?;
    tx.execute(
        "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES(?1,?2,unixepoch()+2592000)",
        params![digest(&secret), user],
    )?;
    Ok(secret)
}

pub(super) fn session_user(conn: &Connection, secret: &str) -> Result<String> {
    if secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::new("unauthorized", "invalid or expired session"));
    }
    super::keys::check(conn, &digest(secret))?;
    conn.query_row("SELECT s.user_id FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=?1 AND s.expires_at>unixepoch() AND u.disabled=0",[digest(secret)],|r|r.get(0)).optional()?.ok_or_else(|| AppError::new("unauthorized","invalid or expired session"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn team_control_default_deny_cas_revocation_and_session_disable() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("team");
        let receipt = initialize(&data, "owner@example.com", "Test team").unwrap();
        let owner = receipt["user_id"].as_str().unwrap();
        let team = receipt["team_id"].as_str().unwrap();
        let mut conn = open(&data).unwrap();
        let (member, session) = {
            let tx = conn.transaction().unwrap();
            let user = identity_user(&tx, "github", "app", "123").unwrap();
            tx.execute(
                "INSERT INTO memberships VALUES(?1,?2,'member')",
                params![team, user],
            )
            .unwrap();
            let session = create_session(&tx, &user).unwrap();
            tx.commit().unwrap();
            (user, session)
        };
        let space = manage(
            &mut conn,
            owner,
            &json!({"action":"space.create","name":"Memory","team_id":team}),
        )
        .unwrap();
        let id = space["id"].as_str().unwrap();
        assert!(authorize(&conn, &member, id, "viewer").is_err());
        let grant = json!({"action":"space.grant","space_id":id,"user_id":member,"role":"editor","expected_revision":1});
        manage(&mut conn, owner, &grant).unwrap();
        assert!(authorize(&conn, &member, id, "editor").is_ok());
        assert!(authorize(&conn, &member, id, "manager").is_err());
        assert_eq!(
            manage(&mut conn, owner, &grant).unwrap_err().code,
            "revision_conflict"
        );
        conn.execute(
            "DELETE FROM memberships WHERE team_id=?1 AND user_id=?2",
            params![team, member],
        )
        .unwrap();
        assert!(authorize(&conn, &member, id, "viewer").is_err());
        assert_eq!(session_user(&conn, &session).unwrap(), member);
        conn.execute("UPDATE users SET disabled=1 WHERE id=?1", [&member])
            .unwrap();
        assert!(session_user(&conn, &session).is_err());
        assert!(initialize(&data, "other@example.com", "Overwrite").is_err());
        let recipient = {
            let tx = conn.transaction().unwrap();
            let user = identity_user(&tx, "email", "", "invitee@example.com").unwrap();
            assert!(link_identity(&tx, &user, "email", "", "owner@example.com").is_err());
            link_identity(&tx, &user, "github", "app", "456").unwrap();
            tx.commit().unwrap();
            user
        };
        let invitation = manage(
            &mut conn,
            owner,
            &json!({"action":"invitation.create","team_id":team,"email":"invitee@example.com"}),
        )
        .unwrap();
        let accept =
            json!({"action":"invitation.accept","invitation_token":invitation["invitation_token"]});
        assert!(manage(&mut conn, owner, &accept).is_err());
        manage(&mut conn, &recipient, &accept).unwrap();
        assert!(manage(&mut conn, &recipient, &accept).is_err());
        assert!(authorize(&conn, &recipient, id, "viewer").is_err());
        manage(&mut conn,&recipient,&json!({"action":"identity.unlink","provider":"github","namespace":"app","subject":"456"})).unwrap();
        assert!(manage(&mut conn,&recipient,&json!({"action":"identity.unlink","provider":"email","subject":"invitee@example.com"})).is_err());
    }
}

#[cfg(test)]
mod provisioning_tests {
    use super::*;
    #[test]
    fn member_initial_grants_are_atomic_and_scope_checked() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("server");
        let initialized = initialize(&data, "owner@example.com", "Provision").unwrap();
        let mut conn = open(&data).unwrap();
        let actor = initialized["user_id"].as_str().unwrap();
        let team = initialized["team_id"].as_str().unwrap();
        let space = manage(
            &mut conn,
            actor,
            &json!({"action":"space.create","team_id":team,"name":"Shared"}),
        )
        .unwrap();
        let other_team = manage(
            &mut conn,
            actor,
            &json!({"action":"team.create","name":"Other"}),
        )
        .unwrap();
        let other_space = manage(
            &mut conn,
            actor,
            &json!({"action":"space.create","team_id":other_team["id"],"name":"Other space"}),
        )
        .unwrap();
        let users: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        for (target, revision) in [(space["id"].clone(), 99), (other_space["id"].clone(), 1)] {
            assert!(manage(&mut conn, actor, &json!({"action":"member.create","team_id":team,"name":"Must rollback","grants":[{"space_id":space["id"],"role":"viewer","expected_revision":1},{"space_id":target,"role":"editor","expected_revision":revision}]})).is_err());
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                users
            );
            assert_eq!(
                conn.query_row(
                    "SELECT revision FROM spaces WHERE id=?1",
                    [space["id"].as_str().unwrap()],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        let created=manage(&mut conn, actor, &json!({"action":"member.create","team_id":team,"name":"Ready member","grants":[{"space_id":space["id"],"role":"viewer","expected_revision":1}]})).unwrap();
        let user =
            super::super::keys::user(&conn, created["personal_key"].as_str().unwrap()).unwrap();
        authorize(&conn, &user, space["id"].as_str().unwrap(), "viewer").unwrap();
        assert!(authorize(&conn, &user, space["id"].as_str().unwrap(), "editor").is_err());
        assert!(authorize(&conn, &user, other_space["id"].as_str().unwrap(), "viewer").is_err());
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn team_identity_and_policy_migrate_existing_control_store() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("server");
        let initialized = initialize(&data, "owner@example.com", "Migration").unwrap();
        let conn = open(&data).unwrap();
        // Fixture representing the previous control schema, with its original account intact.
        conn.execute_batch("DROP TABLE key_credentials; DROP TABLE personal_keys; DROP TABLE agent_sessions; DROP TABLE agents; DROP TABLE devices; DROP TABLE user_profiles; DROP TABLE memory_denials; ALTER TABLE replicas DROP COLUMN sync_status; ALTER TABLE replicas DROP COLUMN pending_conflicts; PRAGMA user_version=1;").unwrap();
        drop(conn);
        let migrated = open(&data).unwrap();
        assert_eq!(
            migrated
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert!(
            migrated
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM users WHERE id=?1)",
                    [initialized["user_id"].as_str().unwrap()],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        for table in [
            "devices",
            "agents",
            "user_profiles",
            "memory_denials",
            "agent_sessions",
        ] {
            assert!(
                migrated
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                        [table],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
        }
        drop(migrated);
        open(&data).unwrap();
    }
}
