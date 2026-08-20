fn run(cli: Cli) -> Result<Value> {
    let cwd = env::current_dir()?;
    let selected_changeset = cli.changeset.clone();
    if !matches!(
        &cli.command,
        Command::Serve { .. }
            | Command::Init { .. }
            | Command::Work { .. }
            | Command::WorkRun { .. }
            | Command::Cg { .. }
            | Command::Office { .. }
            | Command::View { .. }
            | Command::Trans { .. }
            | Command::Agent { .. }
    ) {
        let paths = if cli.scope == Scope::All {
            resolve_live_read_store_paths(cli.scope, &cwd, true)?
        } else {
            vec![resolve_live_store_path(cli.scope, &cwd)?]
        };
        for path in paths {
            if work::schema_migration_needed(&path.path)? {
                Store::open(scope_name(path.scope), &path.path)?;
            }
        }
    }
    match cli.command {
        Command::Serve { mcp, path } => {
            changeset::reject_selector(selected_changeset.as_deref(), "serve")?;
            if !mcp {
                return Err(AppError::new(
                    "invalid_serve_mode",
                    "only `lwc serve --mcp` is supported",
                ));
            }
            crate::mcp::serve(path.as_deref())
        }
        Command::Init { no_git_exclude } => {
            changeset::reject_selector(selected_changeset.as_deref(), "init")?;
            ensure_scope_supported(cli.scope, false, "init")?;
            let store_path = init_store_path(cli.scope, &cwd)?;
            let git_exclude =
                configure_git_exclude(store_path.scope, &store_path.path, no_git_exclude)?;
            let (mut store, created) =
                Store::initialize(scope_name(store_path.scope), &store_path.path)?;
            store.materialize()?;
            let mut response = json!({
                "scope": scope_name(store_path.scope),
                "database": store_path.path,
                "created": created,
                "git_exclude": git_exclude,
                "recommendations": {
                    "md_trans": {
                        "default": "disabled",
                        "config_show": "lwc config show",
                        "convert": "lwc trans INPUT --output OUTPUT.md",
                        "ingest_after_review": "lwc source add OUTPUT.md",
                        "automatic_actions": [],
                        "engines": {
                            "anydoc": {
                                "install": "npm install --global @firecrawl/anydoc",
                                "configure": "lwc config set --trans anydoc",
                                "repository": "https://github.com/firecrawl/anydoc"
                            },
                            "markitdown": {
                                "install": "python3 -m pip install 'markitdown[all]'",
                                "configure": "lwc config set --trans markitdown",
                                "repository": "https://github.com/microsoft/markitdown"
                            }
                        }
                    }
                }
            });
            if store_path.scope == Scope::Project {
                response["recommendations"]["lwc_readiness"] =
                    crate::agent::readiness(&cwd)?;
            }
            Ok(response)
        }
        Command::Work { command } => {
            ensure_scope_supported(cli.scope, false, "work")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                WorkCommand::List => work::list(&store_path),
                WorkCommand::Status { id } => work::status(&store_path, &id),
                WorkCommand::Watch { id } => work::watch(&store_path, &id),
                WorkCommand::Cancel { id } => work::cancel(&store_path, &id),
                WorkCommand::Resume { id } => work::resume(&store_path, &id),
            }
        }
        Command::WorkRun { root, id } => {
            changeset::reject_selector(selected_changeset.as_deref(), "work runner")?;
            work::run(&root, &id)
        }
        Command::Cg { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "cg")?;
            ensure_scope_supported(cli.scope, false, "cg")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                CgCommand::Init { verbose } => codegraph::init(&store_path, verbose),
                CgCommand::Status => codegraph::status(&store_path),
                CgCommand::Run(args) => codegraph::run(&store_path, &args),
            }
        }
        Command::Office {
            command: OfficeCommand::Run(args),
        } => {
            changeset::reject_selector(selected_changeset.as_deref(), "office")?;
            crate::office::run(&cwd, &args)
        }
        Command::View { port, no_open } => {
            changeset::reject_selector(selected_changeset.as_deref(), "view")?;
            ensure_scope_supported(cli.scope, false, "view")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            view::run(store_path, port, no_open)
        }
        Command::Changeset { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "changeset")?;
            ensure_scope_supported(cli.scope, false, "changeset")?;
            let live = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                ChangesetCommand::Begin { name } => to_json(changeset::begin(&live, &name)?),
                ChangesetCommand::List => to_json(changeset::list(&live, 1000)?),
                ChangesetCommand::Show { name, limit } => {
                    validate_limit(limit)?;
                    to_json(changeset::show(&live, &name, limit)?)
                }
                ChangesetCommand::Commit {
                    name,
                    allow_lint_issues,
                    reason,
                } => to_json(changeset::commit(
                    &live,
                    &name,
                    allow_lint_issues,
                    reason.as_deref(),
                )?),
                ChangesetCommand::Discard { name } => to_json(changeset::discard(&live, &name)?),
                ChangesetCommand::Rollback { changeset_id } => {
                    to_json(changeset::rollback(&live, &changeset_id)?)
                }
            }
        }
        Command::Schema { command } => {
            ensure_scope_supported(cli.scope, false, "schema")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                SchemaCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let schema = read_utf8(&file, true)?;
                    require_text("schema", &schema)?;
                    let response = store.schema_set(&schema)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                SchemaCommand::Show => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.schema_show()?)
                }
            }
        }
        Command::Purpose { command } => {
            ensure_scope_supported(cli.scope, false, "purpose")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                PurposeCommand::Set { file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let purpose = read_utf8(&file, true)?;
                    require_text("purpose", &purpose)?;
                    let response = store.purpose_set(&purpose)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                PurposeCommand::Show => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.purpose_show()?)
                }
            }
        }
        Command::Source { command } => {
            ensure_scope_supported(cli.scope, false, "source")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                SourceCommand::Add {
                    file,
                    title,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    let input = prepare_source_input(
                        &store_path,
                        &file,
                        title,
                        allow_external_source,
                        acknowledge_sensitive_source,
                    )?;
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.source_add(input)?;
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                SourceCommand::AddDir {
                    directory,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    validate_source_scope(&store_path, &directory, allow_external_source)?;
                    let files = collect_documents(&directory).map_err(|error| {
                        AppError::new("invalid_source_directory", error.to_string())
                    })?;
                    let discovered = files.len();
                    let mut skipped_files = Vec::new();
                    let inputs = files.into_iter().map(|file| {
                        let resolved = match validate_source_scope(
                            &store_path,
                            &file,
                            allow_external_source,
                        ) {
                            Ok(path) => path,
                            Err(error) if error.code == "io_error" => {
                                skipped_files.push(file.display().to_string());
                                return Ok(None);
                            }
                            Err(error) => return Err(error),
                        };
                        let content = match read_utf8(&resolved, false) {
                            Ok(content) if !content.trim().is_empty() => content,
                            _ => {
                                skipped_files.push(file.display().to_string());
                                return Ok(None);
                            }
                        };
                        validate_sensitive_source(
                            &resolved,
                            &content,
                            acknowledge_sensitive_source,
                        )?;
                        let title = file
                            .strip_prefix(&directory)
                            .unwrap_or(&file)
                            .to_string_lossy()
                            .replace('\\', "/");
                        Ok(Some(SourceAddInput {
                            title: Some(title),
                            origin: file.display().to_string(),
                            tracked_path: Some(tracked_source_path(&store_path, &resolved)?),
                            content,
                        }))
                    });
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let responses = store.source_add_stream(inputs)?;
                    let created = responses.iter().filter(|response| response.created).count();
                    let duplicates = responses.len() - created;
                    if !responses.is_empty() {
                        materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    }
                    if !skipped_files.is_empty() {
                        let examples = skipped_files
                            .iter()
                            .take(10)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(AppError::new(
                            "partial_import",
                            format!(
                                "imported {created} new and reused {duplicates} sources, but skipped {} files: {examples}; fix them and rerun (the import is idempotent)",
                                skipped_files.len()
                            ),
                        ));
                    }
                    Ok(json!({
                        "scope": scope_name(store_path.scope),
                        "database": store_path.path,
                        "discovered": discovered,
                        "created": created,
                        "duplicates": duplicates,
                        "skipped": skipped_files.len(),
                        "skipped_files": skipped_files,
                    }))
                }
                SourceCommand::AddManifest {
                    manifest,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => {
                    let raw = read_utf8(&manifest, false)?;
                    let parsed: SourceManifest = serde_json::from_str(&raw)
                        .map_err(|error| AppError::new("invalid_manifest", error.to_string()))?;
                    if parsed.sources.is_empty() {
                        return Err(AppError::new(
                            "invalid_manifest",
                            "manifest sources must not be empty",
                        ));
                    }
                    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
                    let mut paths = Vec::with_capacity(parsed.sources.len());
                    let mut inputs = Vec::with_capacity(parsed.sources.len());
                    for entry in parsed.sources {
                        let path = base.join(entry.path);
                        let input = prepare_source_input(
                            &store_path,
                            &path,
                            entry.title,
                            allow_external_source,
                            acknowledge_sensitive_source,
                        )?;
                        paths.push(path);
                        inputs.push(input);
                    }

                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let responses = store.source_add_many(inputs)?;
                    let created = responses.iter().filter(|response| response.created).count();
                    let duplicates = responses.len() - created;
                    let sources = paths
                        .into_iter()
                        .zip(responses)
                        .map(|(path, response)| {
                            json!({
                                "path": path,
                                "source": response.source,
                                "created": response.created
                            })
                        })
                        .collect::<Vec<_>>();
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    Ok(json!({
                        "scope": scope_name(store_path.scope),
                        "database": store_path.path,
                        "created": created,
                        "duplicates": duplicates,
                        "sources": sources
                    }))
                }
                SourceCommand::List { limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.source_list(limit, offset)?)
                }
                SourceCommand::Status {
                    source_ids,
                    all,
                    allow_external_source,
                } => {
                    let mut store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    let selected = store.source_status_targets(source_ids.clone(), all)?;
                    let mut resolved = BTreeMap::new();
                    for target in &selected.targets {
                        if !resolved.contains_key(&target.tracked_path) {
                            resolved.insert(
                                target.tracked_path.clone(),
                                resolve_tracked_source_path(
                                    &store_path,
                                    &target.tracked_path,
                                    allow_external_source,
                                )?,
                            );
                        }
                    }
                    let mut live = BTreeMap::new();
                    for (tracked_path, path) in resolved {
                        let source = prepare_live_source(
                            &store_path,
                            path,
                            allow_external_source,
                            MAX_INPUT_BYTES,
                        )?;
                        live.insert(tracked_path, inspect_prepared_source(source));
                    }
                    let current = store.source_status_targets(source_ids, all)?;
                    ensure_source_status_unchanged(&selected, &current)?;
                    let checks = selected
                        .targets
                        .into_iter()
                        .map(|target| {
                            let live = live
                                .get(&target.tracked_path)
                                .expect("every preflighted path is inspected");
                            let filesystem_state = if live.state == "hashed" {
                                if live.content_hash.as_deref()
                                    == Some(target.head_content_hash.as_str())
                                {
                                    "current"
                                } else {
                                    "modified"
                                }
                            } else {
                                live.state
                            };
                            SourceStatusCheck {
                                requested_source_id: target.requested_source_id,
                                tracked_path: target.tracked_path,
                                head_source_id: target.head_source_id,
                                head_revision: target.head_revision,
                                lineage_state: if target.requested_source_id
                                    == target.head_source_id
                                {
                                    "current"
                                } else {
                                    "superseded"
                                },
                                filesystem_state,
                                head_content_hash: target.head_content_hash,
                                live_content_hash: live.content_hash.clone(),
                                live_bytes: live.bytes,
                                message: live.message.clone(),
                            }
                        })
                        .collect();
                    to_json(SourceStatusResponse {
                        scope: scope_name(store_path.scope).to_string(),
                        database: store_path.path.display().to_string(),
                        checked_at_unix_ms: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(|error| AppError::new("system_time_error", error.to_string()))?
                            .as_millis(),
                        checks,
                        untracked_source_ids: selected.untracked_source_ids,
                    })
                }
                SourceCommand::Diff {
                    id,
                    path,
                    to_source,
                    max_chars,
                    allow_external_source,
                    acknowledge_sensitive_source,
                } => run_source_diff(
                    &store_path,
                    id,
                    path.as_deref(),
                    to_source,
                    max_chars,
                    allow_external_source,
                    acknowledge_sensitive_source,
                ),
                SourceCommand::Show {
                    id,
                    offset_chars,
                    max_chars,
                } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_offset(offset_chars)?;
                    if max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "max-chars must be greater than zero",
                        ));
                    }
                    to_json(store.source_show(id, offset_chars, max_chars)?)
                }
                SourceCommand::Refs { id, limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.source_refs(id, limit, offset)?)
                }
                SourceCommand::Remove { id } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.source_remove(id)?;
                    materialize_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Page { command } => {
            ensure_scope_supported(cli.scope, false, "page")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                PageCommand::Put {
                    slug,
                    title,
                    kind,
                    summary,
                    file,
                    source_ids,
                    provenance,
                } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_page_touch(&live, name, &slug, &source_ids)?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    require_text("slug", &slug)?;
                    require_text("title", &title)?;
                    require_text("kind", &kind)?;
                    let body = read_utf8(&file, true)?;
                    require_text("page body", &body)?;
                    let response = store.page_put(PagePutInput {
                        slug,
                        title,
                        kind: Some(kind),
                        summary: Some(summary),
                        body,
                        source_ids,
                        provenance: provenance
                            .into_iter()
                            .map(|value| value.as_str().to_string())
                            .collect(),
                    })?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                PageCommand::List { limit, offset } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.page_list(limit, offset)?)
                }
                PageCommand::Show { slug } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.page_show(&slug)?)
                }
                PageCommand::Links { slug } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.page_links(&slug)?)
                }
                PageCommand::Remove { slug } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_page_touch(&live, name, &slug, &[])?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.page_remove(&slug)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Tag {
            command: TagCommand::List,
        } if cli.scope == Scope::All => {
            let paths = resolve_effective_read_store_paths(
                cli.scope,
                &cwd,
                selected_changeset.as_deref(),
            )?;
            let stores = paths
                .into_iter()
                .map(|path| {
                    Store::open_for_read(scope_name(path.scope), &path.path)?.tag_list()
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"scope": "all", "stores": stores}))
        }
        Command::Tag { command } => {
            ensure_scope_supported(cli.scope, false, "tag")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                TagCommand::Set {
                    tag,
                    page,
                    priority,
                    reason,
                } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_tag_touch(&live, name, &tag, Some(&page), false)?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    store.tag_set(&tag, &page, priority, &reason)
                }
                TagCommand::Remove { tag, page } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_tag_touch(&live, name, &tag, Some(&page), false)?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    store.tag_remove(&tag, &page)
                }
                TagCommand::Delete { tag } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_tag_touch(&live, name, &tag, None, false)?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    store.tag_delete(&tag)
                }
                TagCommand::Autoload {
                    tag,
                    enable,
                    disable: _,
                    priority,
                    limit,
                    max_chars,
                    reason,
                } => {
                    if let Some(name) = selected_changeset.as_deref() {
                        let live = resolve_live_store_path(cli.scope, &cwd)?;
                        changeset::prepare_tag_touch(&live, name, &tag, None, enable)?;
                    }
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    store.tag_autoload(&tag, enable, priority, limit, max_chars, &reason)
                }
                TagCommand::List => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    store.tag_list()
                }
            }
        }
        Command::Load { command } => match command {
            LoadCommand::Tag { tag, limit } => {
                if !(1..=100).contains(&limit) {
                    return Err(AppError::new(
                        "invalid_limit",
                        "tag load limit must be between 1 and 100",
                    ));
                }
                let paths = resolve_effective_read_store_paths(
                    cli.scope,
                    &cwd,
                    selected_changeset.as_deref(),
                )?;
                let stores = paths
                    .into_iter()
                    .map(|path| Store::open_for_read(scope_name(path.scope), &path.path))
                    .collect::<Result<Vec<_>>>()?;
                for store in &stores {
                    store.begin_read_snapshot()?;
                }
                let mut identities = Vec::new();
                let mut found = false;
                for (index, store) in stores.iter().enumerate() {
                    match store.tag_page_identities(&tag, limit + 1) {
                        Ok(rows) => {
                            found = true;
                            identities.extend(rows.into_iter().map(|row| (index, row)));
                        }
                        Err(error) if error.code == "tag_not_found" && cli.scope == Scope::All => {}
                        Err(error) => return Err(error),
                    }
                }
                if !found {
                    return Err(AppError::new(
                        "tag_not_found",
                        format!("tag {} does not exist", tag.trim()),
                    ));
                }
                identities.sort_by(|(_, left), (_, right)| {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then_with(|| scope_priority(&left.scope).cmp(&scope_priority(&right.scope)))
                        .then_with(|| left.page_slug.cmp(&right.page_slug))
                });
                let has_more = identities.len() > limit;
                identities.truncate(limit);
                let mut pages = Vec::with_capacity(identities.len());
                for (ordinal, (store, identity)) in identities.into_iter().enumerate() {
                    pages.push(stores[store].tagged_page(identity, ordinal + 1)?);
                }
                let body_bytes = pages.iter().map(|page| page.page.body.len()).sum::<usize>();
                let body_chars = pages
                    .iter()
                    .map(|page| page.page.body.chars().count())
                    .sum::<usize>();
                Ok(json!({
                    "scope": scope_name(cli.scope),
                    "tag": tag.trim(),
                    "pages": pages,
                    "limit": limit,
                    "returned": pages.len(),
                    "has_more": has_more,
                    "body_bytes": body_bytes,
                    "body_chars": body_chars,
                }))
            }
        },
        Command::Agent { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "agent")?;
            match command {
                AgentCommand::Install {
                    target,
                    location,
                    yes,
                    print_config,
                    no_prompt_hook,
                } => crate::agent::install(
                    &cwd,
                    target.as_deref(),
                    location.map(Into::into),
                    yes,
                    print_config.as_deref(),
                    !no_prompt_hook,
                ),
                AgentCommand::Status { target, location } => crate::agent::status(
                    &cwd,
                    target.as_deref(),
                    location.map(Into::into),
                ),
                AgentCommand::Refresh { target, location } => crate::agent::refresh(
                    &cwd,
                    target.as_deref(),
                    location.map(Into::into),
                ),
                AgentCommand::Uninstall {
                    target,
                    location,
                    yes,
                } => crate::agent::uninstall(
                    &cwd,
                    target.as_deref(),
                    location.map(Into::into),
                    yes,
                ),
                AgentCommand::Hook { agent, event, raw } => {
                    let value = crate::agent::hook(agent.into(), &event, cli.scope, &cwd);
                    if raw {
                        print!("{}", value.as_str().unwrap_or_default());
                        Ok(Value::Null)
                    } else {
                        Ok(value)
                    }
                }
            }
        }
        Command::Ingest { command } => {
            ensure_scope_supported(cli.scope, false, "ingest")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                IngestCommand::List {
                    status,
                    limit,
                    offset,
                } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(limit)?;
                    validate_offset(offset)?;
                    to_json(store.ingest_list(status.as_deref(), limit, offset)?)
                }
                IngestCommand::Next {
                    context_limit,
                    source_max_chars,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(context_limit)?;
                    if source_max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "source-max-chars must be greater than zero",
                        ));
                    }
                    let response = store.ingest_next(context_limit, source_max_chars)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Claim {
                    source_id,
                    context_limit,
                    source_max_chars,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    validate_limit(context_limit)?;
                    if source_max_chars == Some(0) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "source-max-chars must be greater than zero",
                        ));
                    }
                    let response =
                        store.ingest_claim(source_id, context_limit, source_max_chars)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Analyze { source_id, file } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let analysis = read_utf8(&file, true)?;
                    require_text("analysis", &analysis)?;
                    let response = store.ingest_analyze(source_id, &analysis)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Complete {
                    source_id,
                    no_derived_pages_reason,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    if let Some(reason) = no_derived_pages_reason.as_deref() {
                        require_text("no-derived-pages-reason", reason)?;
                    }
                    let response =
                        store.ingest_complete(source_id, no_derived_pages_reason.as_deref())?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Fail { source_id, message } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    require_text("message", &message)?;
                    let response = store.ingest_fail(source_id, &message)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                IngestCommand::Retry { source_id } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.ingest_retry(source_id)?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Config { command } => {
            ensure_scope_supported(cli.scope, false, "config")?;
            if selected_changeset.is_some() {
                return Err(AppError::new(
                    "changeset_command_not_supported",
                    "configuration is deployment-local and cannot be changed in a changeset",
                ));
            }
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                ConfigCommand::Show => {
                    config::response(scope_name(store_path.scope), &store_path.path)
                }
                ConfigCommand::Set {
                    graph,
                    trans,
                    office,
                    memory,
                    memory_max_age_days,
                    memory_max_bytes,
                    trans_timeout,
                    trans_args,
                } => {
                    if graph.is_none()
                        && trans.is_none()
                        && office.is_none()
                        && memory.is_none()
                    {
                        return Err(AppError::new(
                            "invalid_input",
                            "config set requires --graph, --trans, --memory, or --office",
                        ));
                    }
                    if office.is_some() && cli.scope != Scope::Global {
                        return Err(AppError::new(
                            "office_config_global_only",
                            "Office capability configuration requires `--scope global`",
                        ));
                    }
                    if trans.is_none() && (trans_timeout.is_some() || !trans_args.is_empty()) {
                        return Err(AppError::new(
                            "invalid_input",
                            "config set requires --trans when using --trans-timeout or --trans-arg",
                        ));
                    }
                    if memory.is_none()
                        && (memory_max_age_days.is_some() || memory_max_bytes.is_some())
                    {
                        return Err(AppError::new(
                            "invalid_input",
                            "config set requires --memory when using memory limits",
                        ));
                    }

                    let graph_setting = graph
                        .as_deref()
                        .map(config::parse_graph_setting)
                        .transpose()?;
                    let trans_setting = match trans {
                        Some(trans) => Some(config::build_trans_settings(
                            &store_path.path,
                            config::parse_trans_setting(&trans)?,
                            trans_timeout,
                            trans_args,
                        )?),
                        None => None,
                    };
                    let office_setting = office
                        .as_deref()
                        .map(config::parse_office_setting)
                        .transpose()?;
                    let memory_setting = match memory {
                        Some(memory) => Some(config::build_memory_settings(
                            &store_path.path,
                            config::parse_memory_setting(&memory)?,
                            memory_max_age_days,
                            memory_max_bytes,
                        )?),
                        None => None,
                    };
                    config::update(
                        &store_path.path,
                        config::ConfigPatch {
                            graph: graph_setting,
                            trans: trans_setting,
                            office: office_setting,
                            memory: memory_setting,
                        },
                    )?;
                    Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let mut response =
                        config::response(scope_name(store_path.scope), &store_path.path)?;
                    if matches!(
                        graph_setting,
                        Some(setting)
                            if setting != config::GraphSetting::Disabled
                                && setting != config::GraphSetting::Inherit
                    )
                    {
                        response["work"] = work::start_graph_projection(
                            scope_name(store_path.scope),
                            &store_path.path,
                        )?["work"]
                            .clone();
                    }
                    Ok(response)
                }
                ConfigCommand::Unset {
                    graph,
                    trans,
                    memory,
                } => {
                    if !graph && !trans && !memory {
                        return Err(AppError::new(
                            "invalid_input",
                            "config unset requires --graph, --trans, or --memory",
                        ));
                    }
                    config::update(
                        &store_path.path,
                        config::ConfigPatch {
                            graph: graph.then_some(config::GraphSetting::Inherit),
                            trans: trans.then_some(config::inherit_trans_settings()),
                            office: None,
                            memory: memory.then_some(config::inherit_memory_settings()),
                        },
                    )?;
                    Store::open(scope_name(store_path.scope), &store_path.path)?;
                    config::response(scope_name(store_path.scope), &store_path.path)
                }
            }
        }
        Command::Trans {
            file,
            output,
            allow_external_source,
        } => {
            changeset::reject_selector(selected_changeset.as_deref(), "trans")?;
            ensure_scope_supported(cli.scope, false, "trans")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            trans::run(&store_path, &cwd, &file, &output, allow_external_source)
        }
        Command::Graph { command } => {
            ensure_scope_supported(cli.scope, false, "graph")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                GraphCommand::Related { slug, limit } => {
                    validate_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.graph_related(&slug, limit)?)
                }
                GraphCommand::Explore {
                    identifier,
                    depth,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_depth(depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    match identifier {
                        Some(identifier) => Ok(store.graph_explore(
                            &identifier,
                            depth,
                            limit,
                            direction.as_str(),
                            &edge_types,
                        )?),
                        None => Ok(store.graph_explore_macro(depth, limit, &edge_types)?),
                    }
                }
                GraphCommand::Node { identifier } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_node(&identifier)?)
                }
                GraphCommand::Neighbors {
                    identifier,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_neighbors(
                        &identifier,
                        limit,
                        direction.as_str(),
                        &edge_types,
                    )?)
                }
                GraphCommand::Path {
                    from,
                    to,
                    max_depth,
                    limit,
                    direction,
                    edge_types,
                } => {
                    validate_graph_depth(max_depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_path(
                        &from,
                        &to,
                        max_depth,
                        limit,
                        direction.as_str(),
                        &edge_types,
                    )?)
                }
                GraphCommand::Impact {
                    identifier,
                    max_depth,
                    limit,
                } => {
                    validate_graph_depth(max_depth)?;
                    validate_graph_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_impact(&identifier, max_depth, limit)?)
                }
                GraphCommand::Overview { limit } => {
                    validate_limit(limit)?;
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_overview(limit)?)
                }
                GraphCommand::Status => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_status()?)
                }
                GraphCommand::Verify => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    Ok(store.graph_verify()?)
                }
                GraphCommand::Relation { command } => match command {
                    GraphRelationCommand::Set {
                        from,
                        relation_type,
                        to,
                        provenance,
                        reason,
                        confidence,
                        source_ids,
                    } => {
                        let mut store =
                            Store::open(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_set(
                            &from,
                            &relation_type,
                            &to,
                            &provenance,
                            &reason,
                            confidence,
                            &source_ids,
                        )?)
                    }
                    GraphRelationCommand::List {
                        from,
                        to,
                        relation_type,
                        limit,
                    } => {
                        validate_limit(limit)?;
                        let store =
                            Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_list(
                            from.as_deref(),
                            to.as_deref(),
                            relation_type.as_deref(),
                            limit,
                        )?)
                    }
                    GraphRelationCommand::Retract {
                        from,
                        relation_type,
                        to,
                        reason,
                    } => {
                        let mut store =
                            Store::open(scope_name(store_path.scope), &store_path.path)?;
                        Ok(store.graph_relation_retract(&from, &relation_type, &to, &reason)?)
                    }
                },
            }
        }
        Command::Weight { command } => {
            ensure_scope_supported(cli.scope, false, "weight")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            match command {
                WeightCommand::List { target, identifier } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.retrieval_weight_list(target.as_str(), &identifier)?)
                }
                WeightCommand::Set {
                    target,
                    identifier,
                    value,
                    reason,
                    provenance,
                } => {
                    let value = value.parse::<i32>().map_err(|_| {
                        AppError::new("invalid_weight", "weight must be one of -2, -1, 1, or 2")
                    })?;
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_weight_set(
                        target.as_str(),
                        &identifier,
                        value,
                        &reason,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::Clear {
                    target,
                    identifier,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_weight_clear(
                        target.as_str(),
                        &identifier,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::Feedback {
                    target,
                    identifier,
                    query,
                    signal,
                    reason,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_feedback_set(
                        target.as_str(),
                        &identifier,
                        &query,
                        signal.value(),
                        &reason,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
                WeightCommand::FeedbackClear {
                    target,
                    identifier,
                    query,
                    provenance,
                } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.retrieval_feedback_clear(
                        target.as_str(),
                        &identifier,
                        &query,
                        provenance.as_str(),
                    )?;
                    materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                    to_json(response)
                }
            }
        }
        Command::Maintenance { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "maintenance")?;
            ensure_scope_supported(cli.scope, false, "maintenance")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                MaintenanceCommand::Materialize => work::start_materialize(&store_path),
                MaintenanceCommand::Reindex => work::start_reindex(&store_path),
                MaintenanceCommand::Compact => work::start_compact(&store_path),
            }
        }
        Command::Checkpoint { command } => {
            changeset::reject_selector(selected_changeset.as_deref(), "checkpoint")?;
            ensure_scope_supported(cli.scope, false, "checkpoint")?;
            let store_path = resolve_live_store_path(cli.scope, &cwd)?;
            match command {
                CheckpointCommand::Create { name } => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.checkpoint_create(&name)?)
                }
                CheckpointCommand::List => {
                    let store =
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                    to_json(store.checkpoint_list()?)
                }
                CheckpointCommand::Restore { name } => {
                    let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                    let response = store.checkpoint_restore(&name)?;
                    let graph_work = work::start_graph_projection(
                        scope_name(store_path.scope),
                        &store_path.path,
                    )
                    .map(|response| Some(response["work"].clone()))
                    .or_else(|error| {
                        if error.code == "graph_disabled" {
                            Ok(None)
                        } else {
                            Err(error)
                        }
                    });
                    let graph_recovery_command = || {
                        config::resolve(scope_name(store_path.scope), &store_path.path)
                            .ok()
                            .and_then(|current| match current.setting {
                                config::GraphSetting::Disabled => None,
                                config::GraphSetting::Grafeo => Some("grafeo"),
                                config::GraphSetting::Surrealdb => Some("surrealdb"),
                                config::GraphSetting::Inherit => {
                                    unreachable!("resolved graph setting")
                                }
                            })
                            .map_or_else(
                                || {
                                    format!(
                                        "lwc --scope {} config show",
                                        scope_name(store_path.scope),
                                    )
                                },
                                |engine| {
                                    format!(
                                        "lwc --scope {} config set --graph {}",
                                        scope_name(store_path.scope),
                                        engine,
                                    )
                                },
                            )
                    };
                    let materialized = store.materialize();
                    let graph_work = match (graph_work, materialized) {
                        (Ok(graph_work), Ok(_)) => graph_work,
                        (Ok(graph_work), Err(error)) => {
                            return Err(AppError::new(
                                "checkpoint_restored_materialization_failed",
                                format!(
                                    "checkpoint restored canonically but Markdown materialization failed: {error}"
                                ),
                            )
                            .with_details(json!({
                                "checkpoint_restored": true,
                                "checkpoint": name,
                                "safety_checkpoint": response.safety_checkpoint,
                                "graph_work": graph_work,
                                "cause": error.code,
                                "recovery_command": format!(
                                    "lwc --scope {} maintenance materialize",
                                    scope_name(store_path.scope),
                                ),
                            })));
                        }
                        (Err(error), Ok(_)) => {
                            return Err(AppError::new(
                                "graph_projection_failed",
                                "checkpoint restored canonically but graph Work could not be queued",
                            )
                            .with_details(json!({
                                "checkpoint_restored": true,
                                "checkpoint": name,
                                "safety_checkpoint": response.safety_checkpoint,
                                "cause": error.code,
                                "recovery_command": graph_recovery_command(),
                            })));
                        }
                        (Err(graph_error), Err(materialize_error)) => {
                            return Err(AppError::new(
                                "checkpoint_restored_projection_failed",
                                "checkpoint restored canonically but graph and Markdown projections failed",
                            )
                            .with_details(json!({
                                "checkpoint_restored": true,
                                "checkpoint": name,
                                "safety_checkpoint": response.safety_checkpoint,
                                "graph_cause": graph_error.code,
                                "materialize_cause": materialize_error.code,
                                "recovery_commands": [
                                    format!(
                                        "lwc --scope {} maintenance materialize",
                                        scope_name(store_path.scope),
                                    ),
                                    graph_recovery_command(),
                                ],
                            })));
                        }
                    };
                    let mut response = to_json(response)?;
                    if let Some(graph_work) = graph_work {
                        response["graph_work"] = graph_work;
                    }
                    Ok(response)
                }
            }
        }
        Command::Search {
            query,
            target,
            granularity,
            group_by,
            kinds,
            limit,
            record,
            explain,
        } => {
            require_text("query", &query)?;
            validate_limit(limit)?;
            for kind in &kinds {
                require_text("kind", kind)?;
            }
            if matches!(target, SearchTarget::Source) && !kinds.is_empty() {
                return Err(AppError::new(
                    "invalid_input",
                    "--kind filters Wiki pages and cannot be combined with --type source",
                ));
            }
            let mode = match target {
                SearchTarget::Auto => SearchMode::Auto,
                SearchTarget::Page => SearchMode::Page,
                SearchTarget::Source => SearchMode::Source,
                SearchTarget::All => SearchMode::All,
            };
            let granularity = match granularity {
                SearchGranularityArg::Document => SearchGranularity::Document,
                SearchGranularityArg::Passage => SearchGranularity::Passage,
                SearchGranularityArg::Sentence => SearchGranularity::Sentence,
                SearchGranularityArg::All => SearchGranularity::All,
            };
            let grouping = match group_by {
                SearchGroupArg::Auto if granularity == SearchGranularity::All => {
                    SearchGrouping::Document
                }
                SearchGroupArg::Auto | SearchGroupArg::None => SearchGrouping::None,
                SearchGroupArg::Document if granularity == SearchGranularity::All => {
                    SearchGrouping::Document
                }
                SearchGroupArg::Document => {
                    return Err(AppError::new(
                        "invalid_input",
                        "--group-by document requires --granularity all",
                    ));
                }
            };
            let options = SearchOptions {
                mode,
                granularity,
                grouping,
                kinds,
                explain,
            };
            let paths =
                resolve_effective_read_store_paths(cli.scope, &cwd, selected_changeset.as_deref())?;
            let mut stores = if record {
                paths
                    .into_iter()
                    .map(|store_path| Store::open(scope_name(store_path.scope), &store_path.path))
                    .collect::<Result<Vec<_>>>()?
            } else {
                paths
                    .into_iter()
                    .map(|store_path| {
                        Store::open_for_read(scope_name(store_path.scope), &store_path.path)
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            let mut results = Vec::new();
            for store in &stores {
                results.extend(store.search_with_options(&query, limit, &options)?.results);
            }
            if record {
                for store in &mut stores {
                    store.record_search(&query, limit)?;
                    materialize_wiki_if_live(store, selected_changeset.as_deref())?;
                }
            }
            results.sort_by(|left, right| {
                search_type_priority(&left.result_type, mode)
                    .cmp(&search_type_priority(&right.result_type, mode))
                    .then_with(|| left.rank.total_cmp(&right.rank))
                    .then_with(|| scope_priority(&left.scope).cmp(&scope_priority(&right.scope)))
                    .then_with(|| left.result_type.cmp(&right.result_type))
                    .then_with(|| left.identifier.cmp(&right.identifier))
            });
            results.truncate(limit);
            Ok(json!({"results": results}))
        }
        Command::Span { command } => {
            ensure_scope_supported(cli.scope, false, "span")?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            match command {
                SpanCommand::Get { identifier } => {
                    require_text("identifier", &identifier)?;
                    to_json(store.span_get(&identifier)?)
                }
                SpanCommand::Expand {
                    identifier,
                    before,
                    after,
                    children,
                } => {
                    require_text("identifier", &identifier)?;
                    if before > 20 || after > 20 {
                        return Err(AppError::new(
                            "invalid_limit",
                            "before and after must not exceed 20",
                        ));
                    }
                    if !(1..=200).contains(&children) {
                        return Err(AppError::new(
                            "invalid_limit",
                            "children must be between 1 and 200",
                        ));
                    }
                    to_json(store.span_expand(&identifier, before, after, children)?)
                }
            }
        }
        Command::Context { limit } => {
            validate_limit(limit)?;
            let paths =
                resolve_effective_read_store_paths(cli.scope, &cwd, selected_changeset.as_deref())?;
            let mut stores = Vec::new();
            for store_path in paths {
                let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                stores.push(store.context_store(limit)?);
            }
            Ok(json!({"stores": stores}))
        }
        Command::Lint {
            limit,
            offset,
            record,
        } => {
            ensure_scope_supported(cli.scope, false, "lint")?;
            validate_limit(limit)?;
            validate_offset(offset)?;
            if let Some(name) = selected_changeset.as_deref() {
                if record {
                    return Err(AppError::new(
                        "changeset_record_unsupported",
                        "changeset overlay lint cannot be recorded in the sparse draft",
                    ));
                }
                let live = resolve_live_store_path(cli.scope, &cwd)?;
                return to_json(changeset::lint(&live, name, limit, offset)?);
            }
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            if record {
                let mut store = Store::open(scope_name(store_path.scope), &store_path.path)?;
                let response = store.lint(limit, offset)?;
                store.record_lint(response.total)?;
                materialize_wiki_if_live(&mut store, selected_changeset.as_deref())?;
                to_json(response)
            } else {
                let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
                to_json(store.lint(limit, offset)?)
            }
        }
        Command::Log { limit } => {
            ensure_scope_supported(cli.scope, false, "log")?;
            validate_limit(limit)?;
            let store_path =
                resolve_effective_store_path(cli.scope, &cwd, selected_changeset.as_deref())?;
            let store = Store::open_for_read(scope_name(store_path.scope), &store_path.path)?;
            to_json(store.log(limit)?)
        }
    }
}
