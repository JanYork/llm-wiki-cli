fn ensure_projection_dirs(root: &Path) -> Result<(), Error> {
    ensure_root(root)?;
    for dir in FIXED_DIRS {
        mkdir(root, dir)?;
    }
    Ok(())
}

fn update_manifest(root: &Path, added: &[String], removed: &[String]) -> Result<(), Error> {
    let mut entries = load_manifest(root)?.into_iter().collect::<BTreeSet<_>>();
    for path in removed {
        entries.remove(path);
    }
    entries.extend(added.iter().cloned());
    save_manifest(root, &entries.into_iter().collect::<Vec<_>>())
}

fn remove_owned_file(root: &Path, relative: &str) -> Result<bool, Error> {
    let target = join(root, relative)?;
    reject_symlink_path(root, relative)?;
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error(format!(
            "refusing symlink target {}",
            target.display()
        ))),
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(&target)
                .map_err(|error| Error(format!("{}: {error}", target.display())))?;
            Ok(true)
        }
        Ok(_) => Err(Error(format!("expected file at {}", target.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error(format!("{}: {error}", target.display()))),
    }
}

fn update_index(root: &Path, page: &PlannedPage, remove_only: bool) -> Result<(), Error> {
    let mut index = read_or_empty_index(root)?;
    let target = format!(
        "{}/{}",
        page.folder,
        page.path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches(".md")
    );
    remove_index_target(&mut index, &target);
    if !remove_only {
        let mut line = format!("- [[{}|{}]]", target, wiki_label(&page.title));
        if let Some(summary) = page.summary.as_deref().and_then(inline_log_value) {
            line.push_str(&format!(" — {summary}"));
        }
        insert_index_line(&mut index, page.folder, line)?;
    }
    write(root, "wiki/index.md", &index.join("\n"))
}

fn update_index_slug(root: &Path, slug: &str) -> Result<(), Error> {
    let mut index = read_or_empty_index(root)?;
    index.retain(|line| {
        !line.starts_with("- [[")
            || !line
                .split("|")
                .next()
                .is_some_and(|target| target.ends_with(&format!("/{slug}")))
    });
    write(root, "wiki/index.md", &index.join("\n"))
}

fn read_or_empty_index(root: &Path) -> Result<Vec<String>, Error> {
    let target = join(root, "wiki/index.md")?;
    let content = match fs::read_to_string(&target) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => render_index(&[]),
        Err(error) => return Err(Error(format!("{}: {error}", target.display()))),
    };
    Ok(content.lines().map(str::to_string).collect())
}

fn remove_index_target(index: &mut Vec<String>, target: &str) {
    let prefix = format!("- [[{target}|");
    index.retain(|line| !line.starts_with(&prefix));
}

fn insert_index_line(index: &mut Vec<String>, folder: &str, line: String) -> Result<(), Error> {
    let label = match folder {
        "entities" => "Entities",
        "concepts" => "Concepts",
        "sources" => "Sources",
        "queries" => "Queries",
        "comparisons" => "Comparisons",
        "synthesis" => "Synthesis",
        "other" => "Other",
        _ => return Err(Error(format!("unsupported page folder: {folder}"))),
    };
    let heading = format!("## {label}");
    let start = index
        .iter()
        .position(|value| value == &heading)
        .ok_or_else(|| Error(format!("missing index section: {heading}")))?
        + 1;
    let end = index[start..]
        .iter()
        .position(|value| value.starts_with("## "))
        .map_or(index.len(), |offset| start + offset);
    let mut lines = index[start..end]
        .iter()
        .filter(|value| value.starts_with("- [["))
        .cloned()
        .collect::<Vec<_>>();
    lines.push(line);
    lines.sort();
    lines.dedup();
    index.splice(start..end, std::iter::once(String::new()).chain(lines));
    Ok(())
}

fn materialize_snapshot_mode(
    root: &Path,
    snapshot: &Snapshot,
    include_raw_sources: bool,
) -> Result<Vec<String>, Error> {
    let source_paths = source_paths(&snapshot.sources)?;
    let pages = plan_pages(&snapshot.pages)?;
    let mut files = BTreeMap::new();

    if include_raw_sources {
        for source in &snapshot.sources {
            let path = source_paths
                .get(source.id.trim())
                .ok_or_else(|| Error(format!("missing planned source path for {}", source.id)))?;
            add(&mut files, path, source.content.clone())?;
        }
    }
    for page in &pages {
        add(&mut files, &page.path, render_page(page))?;
    }
    add(&mut files, "schema.md", snapshot.schema.clone())?;
    add(&mut files, "purpose.md", snapshot.purpose.clone())?;
    add(&mut files, "wiki/index.md", render_index(&pages))?;
    add(&mut files, "wiki/log.md", render_log(&snapshot.operations))?;
    add(
        &mut files,
        "wiki/overview.md",
        render_overview(snapshot, &pages),
    )?;
    add(
        &mut files,
        ".obsidian/app.json",
        obsidian_app_json().to_string(),
    )?;
    let mut manifest_paths: BTreeSet<String> = source_paths.into_values().collect();
    manifest_paths.extend(files.keys().cloned());

    ensure_root(root)?;
    for dir in FIXED_DIRS {
        mkdir(root, dir)?;
    }
    cleanup_stale(root, &manifest_paths)?;

    let mut written = Vec::with_capacity(files.len());
    for (path, content) in files {
        write(root, &path, &content)?;
        written.push(path);
    }
    save_manifest(root, &manifest_paths.into_iter().collect::<Vec<_>>())?;
    Ok(written)
}

#[derive(Clone)]
struct PlannedPage {
    path: String,
    folder: &'static str,
    title: String,
    kind: String,
    summary: Option<String>,
    body: String,
    sources: Vec<String>,
    provenance: Vec<String>,
    created: String,
    updated: String,
}

fn source_paths(sources: &[Source]) -> Result<BTreeMap<String, String>, Error> {
    let (mut ids, mut paths, mut out) = (BTreeSet::new(), BTreeSet::new(), BTreeMap::new());
    for source in sources {
        let id = source.id.trim();
        if id.is_empty() {
            return Err(Error("source id must not be empty".into()));
        }
        if source.origin.trim().is_empty() {
            return Err(Error(format!("source origin must not be empty for {id}")));
        }
        if !ids.insert(id.to_string()) {
            return Err(Error(format!("duplicate source id: {id}")));
        }
        let path = source_artifact_rel_path(id, &source.origin)?;
        if !paths.insert(path.clone()) {
            return Err(Error(format!("duplicate raw source output path: {path}")));
        }
        out.insert(id.to_string(), path);
    }
    Ok(out)
}

fn plan_pages(pages: &[Page]) -> Result<Vec<PlannedPage>, Error> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(pages.len());
    for page in pages {
        if page.title.trim().is_empty() {
            return Err(Error(format!(
                "page title must not be empty for {}",
                page.slug
            )));
        }
        if page.created.trim().is_empty() || page.updated.trim().is_empty() {
            return Err(Error(format!(
                "page created/updated must not be empty for {}",
                page.slug
            )));
        }
        let slug = normalize_slug(&page.slug)?;
        let folder = folder_for(page.kind.as_deref());
        let path = format!("wiki/{folder}/{slug}.md");
        if !seen.insert(path.clone()) {
            return Err(Error(format!("duplicate page output path: {path}")));
        }
        let mut sources = page.source_artifact_paths.clone();
        sources.sort();
        sources.dedup();
        for source in &sources {
            validate_raw_ref(source)?;
        }
        let provenance = normalize_provenance(&page.provenance)?;
        out.push(PlannedPage {
            path,
            folder,
            title: page.title.trim().to_string(),
            kind: normalize_kind(page.kind.as_deref()),
            summary: trimmed(page.summary.as_deref()),
            body: text(&page.body),
            sources,
            provenance,
            created: page.created.trim().to_string(),
            updated: page.updated.trim().to_string(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.title.cmp(&b.title)));
    Ok(out)
}

fn add(files: &mut BTreeMap<String, String>, path: &str, content: String) -> Result<(), Error> {
    if files.insert(path.to_string(), content).is_some() {
        Err(Error(format!("duplicate output file: {path}")))
    } else {
        Ok(())
    }
}

fn ensure_root(root: &Path) -> Result<(), Error> {
    if let Ok(meta) = fs::symlink_metadata(root) {
        if meta.file_type().is_symlink() {
            return Err(Error(format!("refusing symlink root {}", root.display())));
        }
        if meta.is_dir() {
            return Ok(());
        }
        return Err(Error(format!(
            "output root is not a directory: {}",
            root.display()
        )));
    }
    fs::create_dir_all(root).map_err(|e| Error(format!("{}: {}", root.display(), e)))
}

fn mkdir(root: &Path, relative: &str) -> Result<(), Error> {
    let target = join(root, relative)?;
    reject_symlink_path(root, relative)?;
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(Error(format!(
                "refusing symlink directory {}",
                target.display()
            )));
        }
        return if meta.is_dir() {
            Ok(())
        } else {
            Err(Error(format!("expected directory at {}", target.display())))
        };
    }
    fs::create_dir(&target).map_err(|e| Error(format!("{}: {}", target.display(), e)))
}

fn write(root: &Path, relative: &str, content: &str) -> Result<(), Error> {
    let target = join(root, relative)?;
    reject_symlink_path(root, relative)?;
    if let Some(parent) = Path::new(relative).parent() {
        let parent = parent.to_string_lossy().replace('\\', "/");
        if !parent.is_empty() {
            mkdir(root, &parent)?;
        }
    }
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(Error(format!(
                "refusing symlink target {}",
                target.display()
            )));
        }
        if meta.is_dir() {
            return Err(Error(format!("expected file at {}", target.display())));
        }
        if meta.len() == content.len() as u64 && file_matches(&target, content.as_bytes())? {
            return Ok(());
        }
    }
    let temporary = target.with_extension(format!(
        "lwc-tmp-{}-{}",
        std::process::id(),
        ARTIFACT_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| Error(format!("{}: {}", temporary.display(), e)))?;
    if let Err(error) = file
        .write_all(content.as_bytes())
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(Error(format!("{}: {}", temporary.display(), error)));
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(Error(format!("{}: {}", target.display(), error)));
    }
    Ok(())
}

fn file_matches(path: &Path, expected: &[u8]) -> Result<bool, Error> {
    let mut file = fs::File::open(path).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    let mut actual = vec![0; expected.len()];
    file.read_exact(&mut actual)
        .map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    Ok(actual == expected)
}

fn cleanup_stale(root: &Path, planned: &BTreeSet<String>) -> Result<(), Error> {
    for stale in load_manifest(root)? {
        if planned.contains(&stale) || stale.starts_with("raw/assets/") {
            continue;
        }
        let target = join(root, &stale)?;
        reject_symlink_path(root, &stale)?;
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error(format!(
                    "refusing symlink target {}",
                    target.display()
                )));
            }
            Ok(meta) if meta.is_file() => {
                fs::remove_file(&target)
                    .map_err(|e| Error(format!("{}: {}", target.display(), e)))?;
            }
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn load_manifest(root: &Path) -> Result<Vec<String>, Error> {
    let target = join(root, MANIFEST_PATH)?;
    reject_symlink_path(root, MANIFEST_PATH)?;
    let mut file = match fs::File::open(&target) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error(format!("{}: {}", target.display(), err))),
    };
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .map_err(|e| Error(format!("{}: {}", target.display(), e)))?;
    let mut entries = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        join(root, line)?;
        entries.push(line.to_string());
    }
    Ok(entries)
}

fn save_manifest(root: &Path, written: &[String]) -> Result<(), Error> {
    let mut content = String::new();
    for path in written {
        content.push_str(path);
        content.push('\n');
    }
    write(root, MANIFEST_PATH, &content)
}

fn reject_symlink_path(root: &Path, relative: &str) -> Result<(), Error> {
    if fs::symlink_metadata(root)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(Error(format!("refusing symlink root {}", root.display())));
    }
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(segment) = component else {
            return Err(Error(format!("unsafe relative path: {relative}")));
        };
        current.push(segment);
        if fs::symlink_metadata(&current)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(Error(format!(
                "refusing symlink path {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn join(root: &Path, relative: &str) -> Result<PathBuf, Error> {
    for component in Path::new(relative).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(Error(format!("unsafe relative path: {relative}")));
        }
    }
    Ok(root.join(relative))
}
