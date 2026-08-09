fn validate_checkpoint_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || name != name.trim()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "checkpoint_name_invalid",
            "checkpoint name must be one safe filename segment of at most 80 bytes",
        ));
    }
    Ok(())
}

fn checkpoint_directory(database: &Path) -> Result<PathBuf> {
    let store_directory = database
        .parent()
        .ok_or_else(|| AppError::new("invalid_store_path", "database has no parent"))?;
    let directory = store_directory.join("checkpoints");
    if let Ok(metadata) = fs::symlink_metadata(&directory)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Err(AppError::new(
            "checkpoint_path_invalid",
            format!(
                "checkpoint directory is not a regular directory: {}",
                directory.display()
            ),
        ));
    }
    Ok(directory)
}

fn checkpoint_path(database: &Path, name: &str) -> Result<PathBuf> {
    validate_checkpoint_name(name)?;
    Ok(checkpoint_directory(database)?.join(format!("{name}.db")))
}

fn create_checkpoint(source: &Connection, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("checkpoint_path_invalid", "checkpoint has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
                "checkpoint_exists"
            } else {
                "checkpoint_create_failed"
            };
            AppError::new(code, format!("cannot create {}: {error}", path.display()))
        })?;

    let result = (|| -> Result<()> {
        let mut destination = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        {
            let backup = Backup::new(source, &mut destination)?;
            backup.run_to_completion(100, Duration::from_millis(10), None)?;
        }
        prepare_store_read_only(&destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn write_sparse_inverse(path: &Path, envelope: &SparseInverseEnvelope) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("checkpoint_path_invalid", "checkpoint has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
            "checkpoint_exists"
        } else {
            "checkpoint_create_failed"
        };
        AppError::new(code, format!("cannot create {}: {error}", path.display()))
    })?;
    let encoded = serde_json::to_vec(envelope)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if let Err(error) = file.write_all(&encoded).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

fn load_sparse_inverse(path: &Path) -> Result<SparseInverseEnvelope> {
    let encoded = fs::read(path)?;
    let envelope: SparseInverseEnvelope = serde_json::from_slice(&encoded)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if envelope.payload.version != 1 {
        return Err(AppError::new(
            "changeset_corrupt",
            "sparse inverse patch version is unsupported",
        ));
    }
    let payload = serde_json::to_vec(&envelope.payload)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    let payload = std::str::from_utf8(&payload)
        .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
    if hash_content(payload) != envelope.checksum {
        return Err(AppError::new(
            "changeset_corrupt",
            "sparse inverse patch checksum does not match",
        ));
    }
    Ok(envelope)
}

fn load_sparse_page_snapshot(conn: &Connection, slug: &str) -> Result<Option<SparsePageSnapshot>> {
    let page = conn
        .query_row(
            "SELECT slug, title, kind, summary, body, structural_navigation,
                    created_at, updated_at
             FROM pages WHERE slug = ?1",
            params![slug],
            |row| {
                Ok(SparsePageSnapshot {
                    slug: row.get(0)?,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    body: row.get(4)?,
                    structural_navigation: row.get(5)?,
                    source_ids: Vec::new(),
                    provenance: Vec::new(),
                    links: Vec::new(),
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()?;
    let Some(mut page) = page else {
        return Ok(None);
    };
    page.source_ids = {
        let mut statement = conn.prepare(
            "SELECT source_id FROM page_sources WHERE page_slug = ?1 ORDER BY source_id",
        )?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    page.provenance = {
        let mut statement = conn.prepare(
            "SELECT provenance FROM page_provenance WHERE page_slug = ?1 ORDER BY provenance",
        )?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    page.links = {
        let mut statement =
            conn.prepare("SELECT to_slug FROM links WHERE from_slug = ?1 ORDER BY to_slug")?;
        statement
            .query_map(params![slug], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(Some(page))
}

fn sparse_page_fingerprint(page: &SparsePageSnapshot) -> String {
    page_content_fingerprint(
        &page.title,
        page.kind.as_deref(),
        page.summary.as_deref(),
        &page.body,
        page.structural_navigation,
        &page.source_ids,
        &page.provenance,
        &page.links,
    )
}

fn changeset_created_source_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT o.detail_json
         FROM operations o
         JOIN changesets c ON o.id > c.begin_operation_id
         WHERE c.status = 'draft' AND o.action = 'source_add'
         ORDER BY o.id",
    )?;
    let mut ids = BTreeSet::new();
    for row in statement.query_map([], |row| row.get::<_, String>(0))? {
        let detail: Value = serde_json::from_str(&row?)
            .map_err(|error| AppError::new("changeset_corrupt", error.to_string()))?;
        if detail.get("created").and_then(Value::as_bool) == Some(true) {
            let id = detail
                .get("source_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    AppError::new("changeset_corrupt", "source_add operation lacks source_id")
                })?;
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().collect())
}

fn load_sparse_source_snapshot(
    conn: &Connection,
    source_id: i64,
) -> Result<Option<SparseSourceSnapshot>> {
    let source = conn
        .query_row(
            "SELECT s.id, s.content_hash, s.title, s.origin, s.content,
                    s.structural_navigation, s.created_at,
                    j.status, j.attempts, j.analysis, j.last_error,
                    j.no_derived_pages_reason, j.updated_at
             FROM sources s JOIN ingest_jobs j ON j.source_id = s.id
             WHERE s.id = ?1",
            params![source_id],
            |row| {
                Ok(SparseSourceSnapshot {
                    id: row.get(0)?,
                    content_hash: row.get(1)?,
                    title: row.get(2)?,
                    origin: row.get(3)?,
                    content: row.get(4)?,
                    structural_navigation: row.get(5)?,
                    created_at: row.get(6)?,
                    ingest: SparseIngestSnapshot {
                        status: row.get(7)?,
                        attempts: row.get(8)?,
                        analysis: row.get(9)?,
                        last_error: row.get(10)?,
                        no_derived_pages_reason: row.get(11)?,
                        updated_at: row.get(12)?,
                    },
                    paths: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut source) = source else {
        return Ok(None);
    };
    source.paths = {
        let mut statement = conn.prepare(
            "SELECT tracked_path, revision, observed_at
             FROM source_path_revisions WHERE source_id = ?1
             ORDER BY tracked_path, revision",
        )?;
        statement
            .query_map(params![source_id], |row| {
                Ok(SparseSourcePathSnapshot {
                    tracked_path: row.get(0)?,
                    revision: row.get(1)?,
                    observed_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(Some(source))
}

fn sparse_source_fingerprint(source: &SparseSourceSnapshot) -> String {
    hash_content(&serde_json::to_string(source).unwrap_or_default())
}

fn fresh_checkpoint_name(database: &Path, prefix: &str) -> Result<String> {
    validate_checkpoint_name(prefix)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AppError::new("system_time_error", error.to_string()))?
        .as_millis();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("{prefix}-{millis}")
        } else {
            format!("{prefix}-{millis}-{suffix}")
        };
        if !checkpoint_path(database, &name)?.exists() {
            return Ok(name);
        }
    }
    Err(AppError::new(
        "checkpoint_create_failed",
        "could not allocate a safety checkpoint name",
    ))
}

fn artifact_snapshot(tx: &Connection, include_source_content: bool) -> Result<artifacts::Snapshot> {
    if std::env::var("LWC_TEST_FORBID_FULL_ARTIFACT_SNAPSHOT").as_deref() == Ok("1") {
        return Err(AppError::new(
            "forbidden_full_artifact_snapshot",
            "injected guard rejected a complete artifact snapshot",
        ));
    }
    let meta = |key: &str, fallback: &str| -> Result<String> {
        Ok(tx
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| fallback.to_string()))
    };

    let mut sources = Vec::new();
    let mut source_paths = BTreeMap::new();
    {
        let sql = if include_source_content {
            "SELECT id, title, origin, content FROM sources ORDER BY id"
        } else {
            "SELECT id, title, origin, '' FROM sources ORDER BY id"
        };
        let mut statement = tx.prepare(sql)?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, title, origin, content) = row?;
            let id_text = id.to_string();
            let path = artifacts::source_artifact_rel_path(&id_text, &origin)
                .map_err(|error| AppError::new("artifact_write_failed", error.to_string()))?;
            source_paths.insert(id, path);
            sources.push(artifacts::Source {
                id: id_text,
                title,
                origin,
                content,
            });
        }
    }

    let mut citations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let mut statement = tx.prepare(
            "SELECT page_slug, source_id
             FROM page_sources
             ORDER BY page_slug, source_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (slug, source_id) = row?;
            if let Some(path) = source_paths.get(&source_id) {
                citations.entry(slug).or_default().push(path.clone());
            }
        }
    }

    let pages = {
        let mut statement = tx.prepare(
            "SELECT p.slug, p.title, p.kind, p.summary, p.body, p.created_at, p.updated_at,
                    EXISTS(
                        SELECT 1 FROM page_sources ps WHERE ps.page_slug = p.slug
                    ),
                    (
                        SELECT GROUP_CONCAT(pp.provenance, ',')
                        FROM page_provenance pp
                        WHERE pp.page_slug = p.slug
                    )
             FROM pages p
             ORDER BY p.slug",
        )?;
        statement
            .query_map([], |row| {
                let slug = row.get::<_, String>(0)?;
                Ok(artifacts::Page {
                    source_artifact_paths: citations.get(&slug).cloned().unwrap_or_default(),
                    slug,
                    title: row.get(1)?,
                    kind: row.get(2)?,
                    summary: row.get(3)?,
                    body: row.get(4)?,
                    provenance: provenance_from_parts(row.get::<_, i64>(7)? != 0, row.get(8)?),
                    created: row.get(5)?,
                    updated: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let operations = {
        let mut statement = tx.prepare(
            "SELECT created_at, action, target, detail_json
             FROM operations
             ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok(artifacts::Operation {
                    created_at: row.get(0)?,
                    action: row.get(1)?,
                    target: row.get(2)?,
                    detail: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };

    Ok(artifacts::Snapshot {
        schema: meta("schema", DEFAULT_SCHEMA)?,
        purpose: meta("purpose", DEFAULT_PURPOSE)?,
        sources,
        pages,
        operations,
    })
}
