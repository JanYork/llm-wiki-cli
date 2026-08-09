fn migrate_schema_shadow(
    request: &WorkRequest,
    directory: &Path,
    progress: &mut dyn FnMut(usize, usize, &str) -> Result<()>,
) -> Result<Value> {
    let pending = directory.join("migrated.db");
    remove_sqlite_family(&pending)?;
    progress(0, 1, "snapshotting-live")?;
    let source = Connection::open_with_flags(&request.database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(2))?;
    let base_revision: String = source.query_row(
        "SELECT value FROM meta WHERE key = 'store_revision'",
        [],
        |row| row.get(0),
    )?;
    let mut destination = Connection::open(&pending)?;
    {
        let backup = Backup::new(&source, &mut destination)?;
        backup.run_to_completion(256, Duration::from_millis(2), None)?;
    }
    drop(destination);
    drop(source);
    progress(1, 1, "snapshotting-live")?;

    let signal_indexing = env::var_os("LWC_TEST_MIGRATION_READY").is_some();
    let indexing_ready = directory.join("migration-indexing-ready");
    let mut migration_progress = |completed: usize, total: usize, phase: &str| {
        progress(completed, total, phase)?;
        if signal_indexing && phase == "indexing" && !indexing_ready.exists() {
            write_bytes(&indexing_ready, b"ready\n")?;
        }
        Ok(())
    };
    let migrated =
        Store::open_with_migration_progress(&request.scope, &pending, &mut migration_progress)?;
    drop(migrated);
    let pending_connection = Connection::open(&pending)?;
    pending_connection.busy_timeout(Duration::from_secs(2))?;
    let pending_version: i32 =
        pending_connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if pending_version != CURRENT_SCHEMA_VERSION as i32 {
        return Err(AppError::new(
            "store_migration_failed",
            "shadow Wiki did not reach the target schema version",
        ));
    }
    let (busy, _, _): (i64, i64, i64) =
        pending_connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(AppError::new(
            "database_busy",
            "shadow Wiki checkpoint is busy; resume this work",
        ));
    }
    drop(pending_connection);
    remove_empty_sqlite_sidecars(&pending)?;

    progress(0, 1, "swapping-shadow")?;
    let live = Connection::open(&request.database)?;
    live.busy_timeout(Duration::from_secs(2))?;
    live.execute_batch("BEGIN IMMEDIATE")?;
    let observed_revision: String = live.query_row(
        "SELECT value FROM meta WHERE key = 'store_revision'",
        [],
        |row| row.get(0),
    )?;
    if observed_revision != base_revision {
        let _ = live.execute_batch("ROLLBACK");
        return Err(AppError::new(
            "migration_conflict",
            "live Wiki changed while its shadow migration was being built; resume to rebuild",
        ));
    }
    live.execute_batch("COMMIT")?;
    let (busy, _, _): (i64, i64, i64) =
        live.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(AppError::new(
            "database_busy",
            "live Wiki has an active reader; resume migration after it closes",
        ));
    }
    drop(live);

    let checkpoint_directory = request
        .database
        .parent()
        .ok_or_else(|| AppError::new("work_invalid", "Wiki database has no parent directory"))?
        .join("checkpoints");
    fs::create_dir_all(&checkpoint_directory)?;
    set_directory_mode(&checkpoint_directory)?;
    let backup_name = format!("pre-migration-{}-{}.db", &request.id[..12], now_ms());
    let backup = checkpoint_directory.join(backup_name);
    let live_wal = sqlite_sidecar(&request.database, "wal");
    let live_shm = sqlite_sidecar(&request.database, "shm");
    let backup_wal = sqlite_sidecar(&backup, "wal");
    let backup_shm = sqlite_sidecar(&backup, "shm");
    fs::rename(&request.database, &backup)?;
    move_if_exists(&live_wal, &backup_wal)?;
    move_if_exists(&live_shm, &backup_shm)?;
    if let Err(error) = fs::rename(&pending, &request.database) {
        let _ = move_if_exists(&backup_wal, &live_wal);
        let _ = move_if_exists(&backup_shm, &live_shm);
        let _ = fs::rename(&backup, &request.database);
        return Err(error.into());
    }
    progress(1, 1, "swapping-shadow")?;

    progress(0, 1, "materializing")?;
    let mut store = Store::open(&request.scope, &request.database)?;
    store.materialize()?;
    progress(1, 1, "materializing")?;
    Ok(json!({
        "database": request.database,
        "schema_version": CURRENT_SCHEMA_VERSION,
        "safety_checkpoint": backup,
    }))
}

fn sqlite_sidecar(database: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}-{suffix}", database.display()))
}

fn move_if_exists(from: &Path, to: &Path) -> Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_empty_sqlite_sidecars(database: &Path) -> Result<()> {
    for (path, must_be_empty) in [
        (sqlite_sidecar(database, "wal"), true),
        (sqlite_sidecar(database, "shm"), false),
    ] {
        match fs::metadata(&path) {
            Ok(metadata) if !must_be_empty || metadata.len() == 0 => {
                fs::remove_file(&path)?;
            }
            Ok(_) => {
                return Err(AppError::new(
                    "store_migration_failed",
                    "shadow Wiki still has uncheckpointed WAL data",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_sqlite_family(database: &Path) -> Result<()> {
    for path in [
        database.to_path_buf(),
        sqlite_sidecar(database, "wal"),
        sqlite_sidecar(database, "shm"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
