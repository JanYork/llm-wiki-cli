//! Soft deletion is control state, never a memory-file operation.
use super::control;
use crate::error::{AppError, Result};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};

pub(super) fn owner(tx: &Transaction<'_>, actor: &str, team: &str) -> Result<()> {
    if !tx.query_row("SELECT EXISTS(SELECT 1 FROM memberships m JOIN users u ON u.id=m.user_id WHERE m.team_id=?1 AND m.user_id=?2 AND m.role='owner' AND u.disabled=0)",params![team,actor],|r|r.get::<_,bool>(0))? {
        return Err(AppError::new("forbidden", "current team owner required"));
    }
    Ok(())
}

pub(super) fn manage(
    tx: &Transaction<'_>,
    actor: &str,
    action: &str,
    input: &Value,
) -> Result<Value> {
    let team = action.starts_with("team.");
    let id = input[if team { "team_id" } else { "space_id" }]
        .as_str()
        .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| AppError::new("invalid_request", "resource ID required"))?;
    let (name, revision, archived): (String, i64, bool) = if team {
        owner(tx, actor, id)?;
        tx.query_row(
            "SELECT name,revision,archived FROM teams WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    } else {
        // Restore checks current membership/grants, not a historic permission snapshot.
        let row:Option<(String,i64,bool,bool)>=tx.query_row("SELECT s.name,s.revision,s.archived,COALESCE(t.archived,0) FROM spaces s LEFT JOIN teams t ON t.id=s.team_id JOIN space_grants g ON g.space_id=s.id JOIN users u ON u.id=g.user_id WHERE s.id=?1 AND g.user_id=?2 AND g.role='manager' AND u.disabled=0 AND (s.user_owner=?2 OR EXISTS(SELECT 1 FROM memberships m WHERE m.team_id=s.team_id AND m.user_id=?2))",params![id,actor],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (name, revision, archived, parent_deleted) =
            row.ok_or_else(|| AppError::new("forbidden", "current space manager required"))?;
        if parent_deleted {
            return Err(AppError::new(
                "team_deleted",
                "restore the team before its spaces",
            ));
        }
        (name, revision, archived)
    };
    let affected: i64 = if team {
        tx.query_row(
            "SELECT COUNT(*) FROM spaces WHERE team_id=?1 AND (archived=0 OR archive_team=?1)",
            [id],
            |r| r.get(0),
        )?
    } else {
        1
    };
    let mut receipt = json!({"id":id,"name":name,"kind":if team{"team"}else{"space"},"revision":revision,"archived":archived,"affected_spaces":affected});
    if action.ends_with(".preview") {
        return Ok(receipt);
    }
    let expected = input["expected_revision"]
        .as_i64()
        .ok_or_else(|| AppError::new("revision_required", "expected_revision required"))?;
    if expected != revision {
        return Err(AppError::new(
            "revision_conflict",
            "resource changed; reload the preview",
        ));
    }
    let deleting = action.ends_with(".delete");
    if archived == deleting {
        receipt["unchanged"] = json!(true);
        return Ok(receipt);
    }
    if team {
        tx.execute("UPDATE teams SET archived=?2,deleted_at=CASE WHEN ?2 THEN unixepoch() ELSE NULL END,revision=revision+1 WHERE id=?1",params![id,deleting])?;
        if deleting {
            tx.execute("UPDATE spaces SET archived=1,deleted_at=unixepoch(),archive_team=?1,revision=revision+1 WHERE team_id=?1 AND archived=0",[id])?;
            tx.execute("UPDATE invitations SET expires_at=unixepoch() WHERE team_id=?1 AND consumed_by IS NULL",[id])?;
        } else {
            // Independently deleted spaces remain in the recycle bin.
            tx.execute("UPDATE spaces SET archived=0,deleted_at=NULL,archive_team=NULL,revision=revision+1 WHERE archive_team=?1",[id])?;
        }
    } else {
        tx.execute("UPDATE spaces SET archived=?2,deleted_at=CASE WHEN ?2 THEN unixepoch() ELSE NULL END,archive_team=NULL,revision=revision+1 WHERE id=?1",params![id,deleting])?;
    }
    control::audit(tx, actor, action, id)?;
    receipt["revision"] = json!(revision + 1);
    receipt["archived"] = json!(deleting);
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_preserves_independent_deletions_and_checks_current_authority() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("server");
        let init = control::initialize(&data, "owner@example.com", "Team").unwrap();
        let owner = init["user_id"].as_str().unwrap();
        let team = init["team_id"].as_str().unwrap();
        let mut conn = control::open(&data).unwrap();
        let a = control::manage(
            &mut conn,
            owner,
            &json!({"action":"space.create","team_id":team,"name":"A"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let b = control::manage(
            &mut conn,
            owner,
            &json!({"action":"space.create","team_id":team,"name":"B"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other = {
            let tx = conn.transaction().unwrap();
            let id = control::identity_user(&tx, "email", "", "other@example.com").unwrap();
            tx.execute(
                "INSERT INTO memberships VALUES(?1,?2,'member')",
                params![team, id],
            )
            .unwrap();
            tx.commit().unwrap();
            id
        };
        let req = |action: &str, id: &str, revision: i64| json!({"action":action,"team_id":id,"space_id":id,"expected_revision":revision});
        assert_eq!(
            control::manage(&mut conn, &other, &req("team.delete", team, 1))
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(
            control::manage(&mut conn, &other, &req("space.delete", &a, 1))
                .unwrap_err()
                .code,
            "forbidden"
        );
        control::manage(&mut conn, owner, &req("space.delete", &a, 1)).unwrap();
        assert_eq!(
            control::authorize(&conn, owner, &a, "viewer")
                .unwrap_err()
                .code,
            "space_deleted"
        );
        assert_eq!(
            control::manage(&mut conn, owner, &req("space.restore", &a, 1))
                .unwrap_err()
                .code,
            "revision_conflict"
        );
        control::manage(&mut conn, owner, &req("team.delete", team, 1)).unwrap();
        assert_eq!(
            control::authorize(&conn, owner, &b, "editor")
                .unwrap_err()
                .code,
            "team_deleted"
        );
        assert_eq!(
            control::manage(&mut conn, owner, &req("space.restore", &a, 2))
                .unwrap_err()
                .code,
            "team_deleted"
        );
        assert!(
            control::manage(
                &mut conn,
                owner,
                &json!({"action":"space.create","team_id":team,"name":"No"})
            )
            .is_err()
        );
        // Current revision retries do not create another deletion transition.
        assert_eq!(
            control::manage(&mut conn, owner, &req("team.delete", team, 2)).unwrap()["unchanged"],
            true
        );
        control::manage(&mut conn, owner, &req("team.restore", team, 2)).unwrap();
        control::authorize(&conn, owner, &b, "editor").unwrap();
        assert_eq!(
            control::authorize(&conn, owner, &a, "viewer")
                .unwrap_err()
                .code,
            "space_deleted"
        );
        assert!(control::manage(&mut conn, &other, &req("space.restore", &a, 2)).is_err());
        control::manage(&mut conn, owner, &req("space.restore", &a, 2)).unwrap();
        control::authorize(&conn, owner, &a, "manager").unwrap();
    }
}
