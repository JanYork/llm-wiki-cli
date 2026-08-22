fn resolve_effective_store_path(
    scope: Scope,
    cwd: &Path,
    selected_changeset: Option<&str>,
) -> Result<StorePath> {
    let live = resolve_live_store_path(scope, cwd)?;
    changeset::resolve_effective(live, selected_changeset)
}

fn resolve_effective_read_store_paths(
    scope: Scope,
    cwd: &Path,
    selected_changeset: Option<&str>,
) -> Result<Vec<StorePath>> {
    if selected_changeset.is_none() {
        return resolve_live_read_store_paths(scope, cwd, true);
    }
    ensure_scope_supported(scope, false, "--changeset")?;
    Ok(vec![resolve_effective_store_path(
        scope,
        cwd,
        selected_changeset,
    )?])
}

fn materialize_if_live(store: &mut Store, selected_changeset: Option<&str>) -> Result<()> {
    if selected_changeset.is_none() {
        store.materialize_incremental(true)?;
    }
    Ok(())
}

fn materialize_wiki_if_live(store: &mut Store, selected_changeset: Option<&str>) -> Result<()> {
    if selected_changeset.is_none() {
        store.materialize_incremental(false)?;
    }
    Ok(())
}

fn plan_list_response(scope:Scope,cwd:&Path,query:Option<&str>,state:Option<&str>,tag:Option<&str>,limit:usize,offset:usize)->Result<Value>{
    validate_limit(limit)?;let paths=resolve_live_read_store_paths(scope,cwd,true)?;let mut plans=Vec::new();let mut enabled=false;for path in paths{if capability_enabled("plan",&path)?{enabled=true;plans.extend(Store::open_for_read(scope_name(path.scope),&path.path)?.plan_query(query,state,tag,limit+offset,0)?)}}if !enabled{return Err(capability_disabled("plan"))}plans.sort_by(|a,b|b["updated_at"].as_str().cmp(&a["updated_at"].as_str()).then_with(||a["scope"].as_str().cmp(&b["scope"].as_str())).then_with(||a["id"].as_str().cmp(&b["id"].as_str())));let plans=plans.into_iter().skip(offset).take(limit).collect::<Vec<_>>();Ok(json!({"returned":plans.len(),"plans":plans}))
}

fn capability_enabled(name:&str,path:&StorePath)->Result<bool>{let setting=match name{"todo"=>config::resolve_todo(scope_name(path.scope),&path.path)?.setting,"plan"=>config::resolve_plan(scope_name(path.scope),&path.path)?.setting,_=>unreachable!()};Ok(setting==config::CapabilitySetting::Enabled)}
fn capability_disabled(name:&'static str)->AppError{AppError::new(if name=="todo"{"todo_disabled"}else{"plan_disabled"},format!("{name} capability is disabled; enable it with `lwc config set --{name} enabled`"))}
fn require_capability(name:&'static str,path:&StorePath)->Result<()> {if capability_enabled(name,path)?{Ok(())}else{Err(capability_disabled(name))}}

fn read_utf8(path: &Path, allow_stdin: bool) -> Result<String> {
    let bytes = if path == Path::new("-") {
        if !allow_stdin {
            return Err(AppError::new(
                "invalid_input",
                "stdin is not supported here",
            ));
        }
        let mut bytes = Vec::new();
        io::stdin()
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure_input_size(path, bytes.len() as u64, MAX_INPUT_BYTES)?;
        bytes
    } else {
        read_file_bounded(path, MAX_INPUT_BYTES)?
    };
    String::from_utf8(bytes)
        .map_err(|_| AppError::new("invalid_utf8", format!("{} is not UTF-8", path.display())))
}

fn read_memory_json(store_path: &StorePath, cwd: &Path, argument: &str) -> Result<String> {
    if argument == "-" {
        return read_utf8(Path::new("-"), true);
    }
    if let Some(path) = argument.strip_prefix('@') {
        if path.is_empty() {
            return Err(AppError::new(
                "invalid_input",
                "remember --json @PATH requires a file path",
            ));
        }
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let resolved = fs::canonicalize(&path)?;
        if store_path.scope == Scope::Project {
            ensure_project_path(&resolved, &project_root(store_path)?)?;
        }
        return read_utf8(&resolved, false);
    }
    ensure_input_size(
        Path::new("<inline-json>"),
        argument.len() as u64,
        MAX_INPUT_BYTES,
    )?;
    Ok(argument.to_owned())
}

fn read_file_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    ensure_input_size(path, fs::metadata(path)?.len(), max_bytes)?;
    let bytes = fs::read(path)?;
    ensure_input_size(path, bytes.len() as u64, max_bytes)?;
    Ok(bytes)
}

fn ensure_input_size(path: &Path, bytes: u64, max_bytes: u64) -> Result<()> {
    if bytes > max_bytes {
        return Err(AppError::new(
            "input_too_large",
            format!(
                "{} is {bytes} bytes; maximum supported input is {max_bytes} bytes",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn require_text(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AppError::new(
            "invalid_input",
            format!("{name} must not be empty"),
        ));
    }
    Ok(())
}

fn prepare_source_input(
    store_path: &StorePath,
    path: &Path,
    title: Option<String>,
    allow_external_source: bool,
    acknowledge_sensitive_source: bool,
) -> Result<SourceAddInput> {
    if path == Path::new("-") {
        return Err(AppError::new(
            "invalid_input",
            "source add requires a file path",
        ));
    }
    if let Some(title) = title.as_deref() {
        require_text("title", title)?;
    }
    let resolved = validate_source_scope(store_path, path, allow_external_source)?;
    let content = read_utf8(&resolved, false)?;
    require_text("source content", &content)?;
    validate_sensitive_source(&resolved, &content, acknowledge_sensitive_source)?;
    Ok(SourceAddInput {
        title,
        origin: path.display().to_string(),
        tracked_path: Some(tracked_source_path(store_path, &resolved)?),
        content,
    })
}

fn tracked_source_path(store_path: &StorePath, resolved: &Path) -> Result<String> {
    let tracked = if store_path.scope == Scope::Project {
        let project_root = project_root(store_path)?;
        resolved.strip_prefix(&project_root).unwrap_or(resolved)
    } else {
        resolved
    };
    tracked
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "invalid_source_path",
                format!("source path is not valid UTF-8: {}", resolved.display()),
            )
        })
}

fn ensure_source_status_unchanged(
    before: &store::SourceStatusTargets,
    after: &store::SourceStatusTargets,
) -> Result<()> {
    if before != after {
        return Err(AppError::new(
            "source_status_unstable",
            "source path revisions changed during the status check; retry",
        ));
    }
    Ok(())
}

fn run_source_diff(
    store_path: &StorePath,
    source_id: i64,
    tracked_path: Option<&str>,
    to_source: Option<i64>,
    max_chars: usize,
    allow_external_source: bool,
    acknowledge_sensitive_source: bool,
) -> Result<Value> {
    if !(1..=MAX_DIFF_OUTPUT_CHARS).contains(&max_chars) {
        return Err(AppError::new(
            "invalid_limit",
            format!("max-chars must be between 1 and {MAX_DIFF_OUTPUT_CHARS}"),
        ));
    }
    let mut store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
    let from = store.source_for_diff(source_id, MAX_DIFF_INPUT_BYTES)?;

    if let Some(to_source_id) = to_source {
        let to = store.source_for_diff(to_source_id, MAX_DIFF_INPUT_BYTES)?;
        let changed = from.content_hash != to.content_hash;
        let rendered = render_diff(
            &from.content,
            &to.content,
            &format!("source:{}", from.id),
            &format!("source:{}", to.id),
            max_chars,
        )?;
        return Ok(json!({
            "scope": scope_name(store_path.scope),
            "database": store_path.path.display().to_string(),
            "from": {
                "kind": "source",
                "source_id": from.id,
                "content_hash": from.content_hash,
                "bytes": from.content.len(),
            },
            "to": {
                "kind": "source",
                "source_id": to.id,
                "content_hash": to.content_hash,
                "bytes": to.content.len(),
            },
            "changed": changed,
            "diff": rendered_diff_json(rendered),
        }));
    }

    let selected = store.source_status_targets(vec![source_id], false)?;
    let target = select_source_diff_target(&selected, tracked_path)?;
    let resolved =
        resolve_tracked_source_path(store_path, &target.tracked_path, allow_external_source)?;
    let prepared = prepare_live_source(
        store_path,
        resolved.clone(),
        allow_external_source,
        MAX_DIFF_INPUT_BYTES as u64,
    )?;
    let (live, content) = read_prepared_source(prepared, MAX_DIFF_INPUT_BYTES as u64, true);
    let current = store.source_status_targets(vec![source_id], false)?;
    ensure_source_status_unchanged(&selected, &current)?;
    if live.state == "unstable" {
        return Err(AppError::new(
            "source_status_unstable",
            live.message
                .unwrap_or_else(|| "tracked source changed during diff".to_string()),
        ));
    }
    if live.state == "oversized" {
        return Err(AppError::new(
            "source_diff_too_large",
            live.message
                .unwrap_or_else(|| "live source exceeds the diff input limit".to_string()),
        ));
    }
    if live.state != "hashed" {
        return Err(AppError::new(
            "source_diff_unavailable",
            format!(
                "tracked source {} is {}{}",
                target.tracked_path,
                live.state,
                live.message
                    .as_deref()
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default()
            ),
        ));
    }
    let content = String::from_utf8(content.ok_or_else(|| {
        AppError::new(
            "source_diff_unavailable",
            "live source content was not captured",
        )
    })?)
    .map_err(|_| {
        AppError::new(
            "invalid_utf8",
            format!("{} is not UTF-8", resolved.display()),
        )
    })?;
    let live_hash = live
        .content_hash
        .expect("a successfully hashed live source has a hash");
    let live_bytes = live
        .bytes
        .expect("a successfully hashed live source has a byte count");
    let changed = from.content_hash != live_hash;
    if changed {
        validate_sensitive_source(&resolved, &content, acknowledge_sensitive_source)?;
    }
    let rendered = render_diff(
        &from.content,
        &content,
        &format!("source:{}", from.id),
        &format!("live:{}", target.tracked_path),
        max_chars,
    )?;
    Ok(json!({
        "scope": scope_name(store_path.scope),
        "database": store_path.path.display().to_string(),
        "from": {
            "kind": "source",
            "source_id": from.id,
            "content_hash": from.content_hash,
            "bytes": from.content.len(),
        },
        "to": {
            "kind": "live",
            "tracked_path": target.tracked_path,
            "head_source_id": target.head_source_id,
            "head_revision": target.head_revision,
            "content_hash": live_hash,
            "bytes": live_bytes,
        },
        "changed": changed,
        "diff": rendered_diff_json(rendered),
    }))
}

fn select_source_diff_target(
    selected: &store::SourceStatusTargets,
    tracked_path: Option<&str>,
) -> Result<store::SourceStatusTarget> {
    if selected.targets.is_empty() {
        return Err(AppError::new(
            "source_diff_untracked",
            format!(
                "source {} has no tracked path; use --to-source to compare immutable snapshots",
                selected
                    .untracked_source_ids
                    .first()
                    .copied()
                    .unwrap_or_default()
            ),
        ));
    }
    if let Some(tracked_path) = tracked_path {
        return selected
            .targets
            .iter()
            .find(|target| target.tracked_path == tracked_path)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    "source_diff_path_not_found",
                    format!("source was never observed at tracked path {tracked_path}"),
                )
            });
    }
    if selected.targets.len() > 1 {
        return Err(AppError::new(
            "source_diff_path_required",
            format!(
                "source has multiple tracked paths; retry with --path and one exact candidate: {}",
                selected
                    .targets
                    .iter()
                    .map(|target| target.tracked_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    Ok(selected.targets[0].clone())
}

fn rendered_diff_json(rendered: source_diff::RenderedDiff) -> Value {
    json!({
        "format": "unified",
        "context_lines": 3,
        "text": rendered.text,
        "returned_chars": rendered.returned_chars,
        "total_chars": rendered.total_chars,
        "truncated": rendered.truncated,
    })
}

fn resolve_tracked_source_path(
    store_path: &StorePath,
    tracked_path: &str,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let tracked = Path::new(tracked_path);
    if tracked.is_absolute() {
        if store_path.scope == Scope::Project {
            let root = project_root(store_path)?;
            if ensure_project_path(tracked, &root).is_err() && !allow_external_source {
                return Err(AppError::new(
                    "external_source_requires_acknowledgement",
                    format!(
                        "tracked source {} is outside project root {}; retry with --allow-external-source only with current authorization",
                        tracked.display(),
                        root.display()
                    ),
                ));
            }
        }
        return Ok(tracked.to_path_buf());
    }
    if store_path.scope != Scope::Project
        || tracked.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::new(
            "corrupt_store",
            format!("invalid tracked source path: {tracked_path}"),
        ));
    }
    let root = project_root(store_path)?;
    let path = root.join(tracked);
    if let Err(error) = ensure_project_path(&path, &root)
        && !allow_external_source
    {
        return Err(AppError::new(
            "external_source_requires_acknowledgement",
            format!(
                "tracked source {} escapes project root {}; retry with --allow-external-source only with current authorization: {error}",
                path.display(),
                root.display()
            ),
        ));
    }
    Ok(path)
}

fn prepare_live_source(
    store_path: &StorePath,
    path: PathBuf,
    allow_external_source: bool,
    max_bytes: u64,
) -> Result<PreparedLiveSource> {
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(PreparedLiveSource::Terminal {
                observed: None,
                path,
                status: LiveSourceStatus {
                    state: if error.kind() == io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "unreadable"
                    },
                    content_hash: None,
                    bytes: None,
                    message: Some(error.to_string()),
                },
            });
        }
    };
    let before = file_fingerprint(&metadata);
    if !metadata.is_file() {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: Some(before),
            status: LiveSourceStatus {
                state: "unreadable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source is not a regular file".to_string()),
            },
        });
    }
    if before.len > max_bytes {
        let bytes = before.len;
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: Some(before),
            status: LiveSourceStatus {
                state: "oversized",
                content_hash: None,
                bytes: Some(bytes),
                message: Some(format!(
                    "tracked source is {bytes} bytes; maximum supported input is {max_bytes} bytes"
                )),
            },
        });
    }
    let file = match open_live_source(&path) {
        Ok(file) => file,
        Err(error) => {
            return Ok(PreparedLiveSource::Terminal {
                observed: Some(before),
                path,
                status: LiveSourceStatus {
                    state: if error.kind() == io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "unreadable"
                    },
                    content_hash: None,
                    bytes: None,
                    message: Some(error.to_string()),
                },
            });
        }
    };
    if file
        .metadata()
        .ok()
        .map(|metadata| file_fingerprint(&metadata))
        .as_ref()
        != Some(&before)
    {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: None,
            status: LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source changed while it was being opened".to_string()),
            },
        });
    }
    if store_path.scope == Scope::Project && !allow_external_source {
        let root = project_root(store_path)?;
        let resolved = fs::canonicalize(&path).map_err(|error| {
            AppError::new(
                "source_status_unstable",
                format!(
                    "tracked source {} changed during authorization: {error}",
                    path.display()
                ),
            )
        })?;
        if !resolved.starts_with(&root) {
            return Err(AppError::new(
                "external_source_requires_acknowledgement",
                format!(
                    "tracked source {} resolves outside project root {}; retry with --allow-external-source only with current authorization",
                    path.display(),
                    root.display()
                ),
            ));
        }
    }
    if fs::metadata(&path)
        .ok()
        .map(|metadata| file_fingerprint(&metadata))
        .as_ref()
        != Some(&before)
    {
        return Ok(PreparedLiveSource::Terminal {
            path,
            observed: None,
            status: LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: None,
                message: Some("tracked source changed during authorization".to_string()),
            },
        });
    }
    Ok(PreparedLiveSource::Ready { path, file, before })
}

fn open_live_source(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    options.custom_flags(LIVE_SOURCE_NONBLOCK);
    options.open(path)
}

fn inspect_prepared_source(prepared: PreparedLiveSource) -> LiveSourceStatus {
    read_prepared_source(prepared, MAX_INPUT_BYTES, false).0
}

fn read_prepared_source(
    prepared: PreparedLiveSource,
    max_bytes: u64,
    capture: bool,
) -> (LiveSourceStatus, Option<Vec<u8>>) {
    let (path, mut file, before) = match prepared {
        PreparedLiveSource::Ready { path, file, before } => (path, file, before),
        PreparedLiveSource::Terminal {
            path,
            observed,
            status,
        } => {
            let current = fs::metadata(&path)
                .ok()
                .map(|metadata| file_fingerprint(&metadata));
            if current != observed {
                return (
                    LiveSourceStatus {
                        state: "unstable",
                        content_hash: None,
                        bytes: current.as_ref().map(|fingerprint| fingerprint.len),
                        message: Some("tracked source changed during status preflight".to_string()),
                    },
                    None,
                );
            }
            return (status, None);
        }
    };

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    let mut content = capture.then(|| Vec::with_capacity(before.len as usize));
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                return (
                    LiveSourceStatus {
                        state: "unreadable",
                        content_hash: None,
                        bytes: Some(bytes),
                        message: Some(error.to_string()),
                    },
                    None,
                );
            }
        };
        bytes += read as u64;
        if bytes > max_bytes {
            return (
                LiveSourceStatus {
                    state: "oversized",
                    content_hash: None,
                    bytes: Some(bytes),
                    message: Some(format!(
                        "tracked source exceeded the {max_bytes}-byte input limit while reading"
                    )),
                },
                None,
            );
        }
        if let Some(content) = content.as_mut() {
            content.extend_from_slice(&buffer[..read]);
        }
        hasher.update(&buffer[..read]);
    }

    let after_handle = file
        .metadata()
        .ok()
        .map(|metadata| file_fingerprint(&metadata));
    let after_path = fs::metadata(path)
        .ok()
        .map(|metadata| file_fingerprint(&metadata));
    let content_hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if after_handle.as_ref() != Some(&before) || after_path.as_ref() != Some(&before) {
        return (
            LiveSourceStatus {
                state: "unstable",
                content_hash: None,
                bytes: Some(bytes),
                message: Some("tracked source changed while it was being hashed".to_string()),
            },
            None,
        );
    }
    (
        LiveSourceStatus {
            state: "hashed",
            content_hash: Some(content_hash),
            bytes: Some(bytes),
            message: None,
        },
        content,
    )
}

fn file_fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    FileFingerprint {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed_seconds: metadata.ctime(),
        #[cfg(unix)]
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn project_root(store_path: &StorePath) -> Result<PathBuf> {
    let root = store_path
        .authority_path()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            AppError::new(
                "invalid_store_path",
                "project Wiki path has no project root",
            )
        })?;
    Ok(fs::canonicalize(root)?)
}

fn validate_source_scope(
    store_path: &StorePath,
    path: &Path,
    allow_external_source: bool,
) -> Result<PathBuf> {
    let resolved = fs::canonicalize(path)?;
    if store_path.scope == Scope::Project && !allow_external_source {
        let project_root = project_root(store_path)?;
        if !resolved.starts_with(&project_root) {
            return Err(AppError::new(
                "external_source_requires_acknowledgement",
                format!(
                    "source {} resolves outside project root {}; retry with --allow-external-source only after confirming it belongs in this Wiki",
                    resolved.display(),
                    project_root.display()
                ),
            ));
        }
    }
    Ok(resolved)
}

fn validate_sensitive_source(path: &Path, content: &str, acknowledged: bool) -> Result<()> {
    if acknowledged {
        return Ok(());
    }
    let reasons = crate::secret_scan::detect_possible_secret_reasons(path, content);

    if reasons.is_empty() {
        return Ok(());
    }
    Err(AppError::new(
        "possible_secret_detected",
        format!(
            "possible sensitive source {}: {}; inspect it and retry with --acknowledge-sensitive-source only when the immutable snapshot is safe",
            path.display(),
            reasons.join(", ")
        ),
    ))
}

fn validate_graph_depth(depth: usize) -> Result<()> {
    if !(1..=10).contains(&depth) {
        return Err(AppError::new(
            "invalid_graph_depth",
            "graph depth must be between 1 and 10",
        ));
    }
    Ok(())
}

fn validate_graph_limit(limit: usize) -> Result<()> {
    if !(1..=5000).contains(&limit) {
        return Err(AppError::new(
            "invalid_graph_limit",
            "graph node limit must be between 1 and 5000",
        ));
    }
    Ok(())
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=1000).contains(&limit) {
        return Err(AppError::new(
            "invalid_limit",
            "limit must be between 1 and 1000",
        ));
    }
    Ok(())
}

fn validate_offset(offset: usize) -> Result<()> {
    if i64::try_from(offset).is_err() {
        return Err(AppError::new(
            "invalid_offset",
            "offset must fit in a signed 64-bit integer",
        ));
    }
    Ok(())
}

fn scope_priority(scope: &str) -> u8 {
    if scope == "project" { 0 } else { 1 }
}

fn search_type_priority(result_type: &str, mode: SearchMode) -> u8 {
    if matches!(mode, SearchMode::Auto | SearchMode::All) && result_type == "source" {
        1
    } else {
        0
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        Scope::Global => "global",
        Scope::All => "all",
    }
}

fn configure_git_exclude(scope: Scope, database: &Path, disabled: bool) -> Result<Value> {
    if scope != Scope::Project {
        return Ok(json!({ "status": "not_applicable" }));
    }
    if disabled {
        return Ok(json!({ "status": "disabled" }));
    }

    let project_root = database
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| AppError::new("git_exclude_failed", "project Wiki path has no root"))?;
    let top_level = match git_output(project_root, &["rev-parse", "--show-toplevel"])? {
        Some(output) => PathBuf::from(output),
        None => return Ok(json!({ "status": "not_git" })),
    };
    let top_level = fs::canonicalize(top_level)?;
    let project_root = fs::canonicalize(project_root)?;
    let relative = project_root.strip_prefix(&top_level).map_err(|_| {
        AppError::new(
            "git_exclude_failed",
            format!(
                "project root {} is outside Git root {}",
                project_root.display(),
                top_level.display()
            ),
        )
    })?;
    let relative_lwc = relative.join(".lwc");
    let git_path = relative_lwc.to_string_lossy().replace('\\', "/");
    let pattern = format!("/{git_path}/");

    let ignored = ProcessCommand::new("git")
        .current_dir(&top_level)
        .args(["check-ignore", "-q", "--no-index", "--", &git_path])
        .status()
        .map_err(|error| AppError::new("git_exclude_failed", error.to_string()))?;
    if ignored.success() {
        return Ok(json!({ "status": "already_ignored", "pattern": pattern }));
    }
    if ignored.code() != Some(1) {
        return Err(AppError::new(
            "git_exclude_failed",
            "git check-ignore failed",
        ));
    }

    let exclude_text = git_output(&top_level, &["rev-parse", "--git-path", "info/exclude"])?
        .ok_or_else(|| AppError::new("git_exclude_failed", "Git exclude path is unavailable"))?;
    let exclude_path = {
        let path = PathBuf::from(exclude_text);
        if path.is_absolute() {
            path
        } else {
            top_level.join(path)
        }
    };
    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = match fs::read_to_string(&exclude_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::new("git_exclude_failed", error.to_string())),
    };
    if !existing.lines().any(|line| line.trim() == pattern) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude_path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        writeln!(file, "{pattern}")?;
    }
    Ok(json!({
        "status": "added",
        "path": exclude_path,
        "pattern": pattern
    }))
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = match ProcessCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::new("git_exclude_failed", error.to_string())),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| AppError::new("git_exclude_failed", "Git returned non-UTF-8 output"))?;
    Ok(Some(value.trim().to_string()))
}

fn to_json<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|error| AppError::new("serialization_error", error.to_string()))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{
        MAX_INPUT_BYTES, Scope, StorePath, file_fingerprint, inspect_prepared_source,
        open_live_source, prepare_live_source,
    };
    use super::{ensure_source_status_unchanged, read_file_bounded};
    use crate::store::{SourceStatusTarget, SourceStatusTargets};

    #[test]
    fn bounded_file_read_rejects_oversized_input() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oversized.txt");
        std::fs::write(&path, b"12345").unwrap();

        let error = read_file_bounded(&path, 4).unwrap_err();

        assert_eq!(error.code, "input_too_large");
    }

    #[cfg(unix)]
    #[test]
    fn file_fingerprint_detects_same_size_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("tracked.txt");
        let replacement = temp.path().join("replacement.txt");
        std::fs::write(&tracked, b"alpha").unwrap();
        std::fs::write(&replacement, b"bravo").unwrap();
        let before = file_fingerprint(&std::fs::metadata(&tracked).unwrap());

        std::fs::rename(&replacement, &tracked).unwrap();
        let after = file_fingerprint(&std::fs::metadata(&tracked).unwrap());

        assert!(before != after);
    }

    #[cfg(unix)]
    #[test]
    fn prepared_source_rejects_path_replacement_before_hashing() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let tracked = temp.path().join("tracked.txt");
        let external_dir = tempfile::tempdir().unwrap();
        let external = external_dir.path().join("external.txt");
        std::fs::write(&tracked, b"inside").unwrap();
        std::fs::write(&external, b"outside").unwrap();
        let store_path = StorePath::new(Scope::Project, temp.path().join(".lwc/wiki.db"));
        let prepared =
            prepare_live_source(&store_path, tracked.clone(), false, MAX_INPUT_BYTES).unwrap();

        std::fs::remove_file(&tracked).unwrap();
        symlink(&external, &tracked).unwrap();
        let status = inspect_prepared_source(prepared);

        assert_eq!(status.state, "unstable");
        assert!(status.content_hash.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn live_source_open_does_not_block_on_a_fifo() {
        use std::{sync::mpsc, thread, time::Duration};

        let temp = tempfile::tempdir().unwrap();
        let fifo = temp.path().join("source.fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || sender.send(open_live_source(&fifo)).unwrap());

        let file = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("opening a FIFO must not block")
            .unwrap();
        assert!(!file.metadata().unwrap().is_file());
    }

    #[test]
    fn source_status_rejects_a_head_change_during_live_hashing() {
        let targets =
            |head_source_id, head_revision, head_content_hash: &str| SourceStatusTargets {
                targets: vec![SourceStatusTarget {
                    requested_source_id: 1,
                    tracked_path: "docs/source.md".to_string(),
                    head_source_id,
                    head_revision,
                    head_content_hash: head_content_hash.to_string(),
                }],
                untracked_source_ids: Vec::new(),
            };
        let before = targets(1, 1, "hash-a");
        let after = targets(2, 2, "hash-b");

        let error = ensure_source_status_unchanged(&before, &after).unwrap_err();

        assert_eq!(error.code, "source_status_unstable");
    }
}
