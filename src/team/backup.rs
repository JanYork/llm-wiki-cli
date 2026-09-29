use crate::error::{AppError, Result};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn offline_lock(data: &Path) -> Result<fs::File> {
    let path = data.join("server.lock");
    if path.exists() && fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(AppError::new(
            "unsafe_backup",
            "server lock cannot be a symlink",
        ));
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    lock.try_lock().map_err(|_| {
        AppError::new(
            "server_running",
            "stop the server before backing up or restoring its data",
        )
    })?;
    Ok(lock)
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    super::private_directory(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == "server.lock" || entry.file_name() == "backup.complete" {
            continue;
        }
        let kind = entry.file_type()?;
        let path = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &path)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &path)?;
            fs::File::open(&path)?.sync_all()?;
        } else {
            return Err(AppError::new(
                "unsafe_backup",
                "backup cannot contain links or special files",
            ));
        }
    }
    #[cfg(unix)]
    fs::File::open(target)?.sync_all()?;
    Ok(())
}

/// A stopped, complete data directory is the backup format. Never overwrite a live volume.
pub(crate) fn snapshot(source: &Path, output: &Path, authority: Option<&Path>) -> Result<Value> {
    let restore = authority.is_some();
    let source = fs::canonicalize(source)?;
    super::private_directory(&source)?;
    let _lock = offline_lock(&source)?;
    if restore && fs::read(source.join("backup.complete"))? != b"lwc-team-backup/1\n" {
        return Err(AppError::new("invalid_backup", "backup was not completed"));
    }
    let parent = fs::canonicalize(output.parent().unwrap_or(Path::new(".")))?;
    let target =
        parent.join(output.file_name().ok_or_else(|| {
            AppError::new("invalid_backup", "destination requires a directory name")
        })?);
    if target.starts_with(&source) || target.exists() {
        return Err(AppError::new(
            "invalid_backup",
            "destination must be a new directory outside the source",
        ));
    }
    let authority = authority.map(fs::canonicalize).transpose()?;
    let _authority_lock = if let Some(path) = &authority {
        if path == &source || target.starts_with(path) {
            return Err(AppError::new(
                "invalid_backup",
                "restore requires the current independent authorization directory",
            ));
        }
        super::private_directory(path)?;
        if fs::read(path.join("policy-signing.key"))?
            != fs::read(source.join("policy-signing.key"))?
        {
            return Err(AppError::new(
                "invalid_backup",
                "backup and current authority have different signing identities",
            ));
        }
        Some(offline_lock(path)?)
    } else {
        None
    };
    let conn = super::control::open(&source)?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(AppError::new(
            "invalid_backup",
            "control store integrity check failed",
        ));
    }
    // Drain the control WAL while exclusively owning the server lifecycle lock.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(conn);
    copy_tree(&source, &target)?;
    if target.join("backup.complete").exists() {
        fs::remove_file(target.join("backup.complete"))?;
    }
    let mut conn = super::control::open(&target)?;
    if let Some(path) = &authority {
        let current = super::control::open(path)?;
        let check: String = current.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(AppError::new(
                "invalid_backup",
                "current authority store is damaged",
            ));
        }
        rusqlite::backup::Backup::new(&current, &mut conn)?.run_to_completion(
            128,
            std::time::Duration::from_millis(5),
            None,
        )?;
        fs::copy(
            path.join("server-access.token"),
            target.join("server-access.token"),
        )?;
    }
    let spaces = conn
        .prepare("SELECT id FROM spaces")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for space in &spaces {
        if let Some(path) = &authority {
            let original = path.join("spaces").join(space);
            let copied = target.join("spaces").join(space);
            if !copied.exists() && original.exists() {
                copy_tree(&original, &copied)?;
            }
        }
        let database = target.join("spaces").join(space).join("wiki.db");
        if database.exists() {
            let mut store = crate::store::Store::open("team", &database)?;
            store.team_head()?;
            if restore {
                let epoch = super::control::token()?;
                store.restore_team_epoch(&epoch)?;
                conn.execute(
                    "UPDATE spaces SET epoch=?1,revision=revision+1 WHERE id=?2",
                    rusqlite::params![epoch, space],
                )?;
            }
        } else if restore {
            conn.execute(
                "UPDATE spaces SET epoch=?1,revision=revision+1 WHERE id=?2",
                rusqlite::params![super::control::token()?, space],
            )?;
        }
    }
    if restore {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE replicas SET ack_head=0,sync_status='recovery_required',pending_conflicts=0",
            [],
        )?;
        tx.commit()?;
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    fs::write(target.join("backup.complete"), b"lwc-team-backup/1\n")?;
    fs::File::open(target.join("backup.complete"))?.sync_all()?;
    Ok(
        json!({"data":target,"restored":restore,"backed_up":!restore,"spaces":spaces.len(),"source_preserved":true}),
    )
}
