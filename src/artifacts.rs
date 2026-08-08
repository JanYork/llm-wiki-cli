use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const FIXED_DIRS: &[&str] = &[
    "raw",
    "raw/sources",
    "raw/assets",
    "wiki",
    "wiki/entities",
    "wiki/concepts",
    "wiki/sources",
    "wiki/queries",
    "wiki/comparisons",
    "wiki/synthesis",
    "wiki/other",
    ".obsidian",
];
const MANIFEST_PATH: &str = ".lwc-artifacts-manifest";
const CURSOR_PATH: &str = ".lwc-artifacts-cursor";
const LOCK_PATH: &str = ".lwc-artifacts-lock";
const LOCK_WAIT: Duration = Duration::from_secs(10);
const LOCK_RETRY: Duration = Duration::from_millis(5);
const SOURCE_ID_MAX: usize = 48;
const BASENAME_MAX: usize = 120;
const SLUG_SEGMENT_MAX: usize = 80;
const REL_PATH_MAX: usize = 240;
static ARTIFACT_COUNTER: AtomicU64 = AtomicU64::new(0);
const PROVENANCE_ORDER: [&str; 4] = [
    "source-grounded",
    "user-provided",
    "agent-observed",
    "hypothesis",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub schema: String,
    pub purpose: String,
    pub sources: Vec<Source>,
    pub pages: Vec<Page>,
    pub operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: String,
    pub title: Option<String>,
    pub origin: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub slug: String,
    pub title: String,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub body: String,
    pub source_artifact_paths: Vec<String>,
    pub provenance: Vec<String>,
    pub created: String,
    pub updated: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub created_at: String,
    pub action: String,
    pub target: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

pub(crate) struct ProjectionLock {
    _file: fs::File,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub(crate) fn lock_projection(root: &Path) -> Result<ProjectionLock, Error> {
    ensure_root(root)?;
    reject_symlink_path(root, LOCK_PATH)?;
    let path = join(root, LOCK_PATH)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| Error(format!("{}: {error}", path.display())))?;
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ProjectionLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                thread::sleep(LOCK_RETRY);
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(Error(format!(
                    "artifact projection remained busy for {} seconds",
                    LOCK_WAIT.as_secs()
                )));
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(Error(format!(
                    "cannot lock artifact projection {}: {error}",
                    path.display()
                )));
            }
        }
    }
}

pub fn source_artifact_rel_path(source_id: &str, origin: &str) -> Result<String, Error> {
    Ok(format!(
        "raw/sources/{}--{}",
        normalize_source_id(source_id)?,
        safe_basename(origin)
    ))
}

pub fn materialize_snapshot(root: &Path, snapshot: &Snapshot) -> Result<Vec<String>, Error> {
    materialize_snapshot_mode(root, snapshot, true)
}

pub fn materialize_wiki_snapshot(root: &Path, snapshot: &Snapshot) -> Result<Vec<String>, Error> {
    materialize_snapshot_mode(root, snapshot, false)
}

pub fn load_cursor(root: &Path) -> Result<i64, Error> {
    let target = join(root, CURSOR_PATH)?;
    reject_symlink_path(root, CURSOR_PATH)?;
    match fs::read_to_string(&target) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|_| Error(format!("invalid artifact cursor in {}", target.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(Error(format!("{}: {error}", target.display()))),
    }
}

pub fn save_cursor(root: &Path, cursor: i64) -> Result<(), Error> {
    write(root, CURSOR_PATH, &format!("{cursor}\n"))
}

pub fn materialize_page(root: &Path, page: &Page) -> Result<Vec<String>, Error> {
    let planned = plan_pages(std::slice::from_ref(page))?
        .pop()
        .ok_or_else(|| Error("missing planned page".into()))?;
    ensure_projection_dirs(root)?;
    write(root, &planned.path, &render_page(&planned))?;
    update_index(root, &planned, false)?;
    update_manifest(root, &[planned.path.clone(), "wiki/index.md".into()], &[])?;
    Ok(vec![planned.path, "wiki/index.md".into()])
}

pub fn remove_page(root: &Path, slug: &str) -> Result<Vec<String>, Error> {
    let slug = normalize_slug(slug)?;
    let mut removed = Vec::new();
    for folder in [
        "entities",
        "concepts",
        "sources",
        "queries",
        "comparisons",
        "synthesis",
        "other",
    ] {
        let path = format!("wiki/{folder}/{slug}.md");
        if remove_owned_file(root, &path)? {
            removed.push(path);
        }
    }
    update_index_slug(root, &slug)?;
    update_manifest(root, &["wiki/index.md".into()], &removed)?;
    removed.push("wiki/index.md".into());
    Ok(removed)
}

pub fn materialize_source(root: &Path, source: &Source) -> Result<Vec<String>, Error> {
    ensure_projection_dirs(root)?;
    let path = source_artifact_rel_path(&source.id, &source.origin)?;
    write(root, &path, &source.content)?;
    update_manifest(root, std::slice::from_ref(&path), &[])?;
    Ok(vec![path])
}

pub fn remove_source(root: &Path, source_id: &str) -> Result<Vec<String>, Error> {
    let prefix = format!("raw/sources/{}--", normalize_source_id(source_id)?);
    let removed = load_manifest(root)?
        .into_iter()
        .filter(|path| path.starts_with(&prefix))
        .filter_map(|path| match remove_owned_file(root, &path) {
            Ok(true) => Some(Ok(path)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    update_manifest(root, &[], &removed)?;
    Ok(removed)
}

pub fn materialize_text(root: &Path, path: &str, content: &str) -> Result<(), Error> {
    ensure_projection_dirs(root)?;
    write(root, path, content)?;
    update_manifest(root, &[path.to_string()], &[])
}

pub fn append_operations(root: &Path, operations: &[Operation]) -> Result<(), Error> {
    if operations.is_empty() {
        return Ok(());
    }
    ensure_projection_dirs(root)?;
    let path = "wiki/log.md";
    let target = join(root, path)?;
    let mut current = match fs::read_to_string(&target) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "# Wiki Log\n".into(),
        Err(error) => return Err(Error(format!("{}: {error}", target.display()))),
    };
    let rendered = render_log(operations);
    current.push_str(rendered.strip_prefix("# Wiki Log\n").unwrap_or(&rendered));
    write(root, path, &current)?;
    update_manifest(root, &[path.into()], &[])
}

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

fn render_page(page: &PlannedPage) -> String {
    let mut out = vec![
        "---".to_string(),
        format!("type: {}", yaml(&page.kind)),
        format!("title: {}", yaml(&page.title)),
        "sources:".to_string(),
    ];
    if page.sources.is_empty() {
        out.push("  []".to_string());
    } else {
        for source in &page.sources {
            out.push(format!("  - {}", yaml(source)));
        }
    }
    out.push("provenance:".to_string());
    if page.provenance.is_empty() {
        out.push("  []".to_string());
    } else {
        for provenance in &page.provenance {
            out.push(format!("  - {}", yaml(provenance)));
        }
    }
    out.push(format!("created: {}", yaml(&page.created)));
    out.push(format!("updated: {}", yaml(&page.updated)));
    if let Some(summary) = &page.summary {
        out.push(format!("summary: {}", yaml(summary)));
    }
    out.push("---".to_string());
    out.push(String::new());
    if !page.body.trim().is_empty() {
        out.push(page.body.trim_end_matches('\n').to_string());
    }
    out.join("\n") + "\n"
}

fn render_index(pages: &[PlannedPage]) -> String {
    let mut groups: BTreeMap<&str, Vec<&PlannedPage>> = BTreeMap::new();
    for page in pages {
        groups.entry(page.folder).or_default().push(page);
    }
    let mut out = String::from("# Wiki Index\n");
    for (key, label) in [
        ("entities", "Entities"),
        ("concepts", "Concepts"),
        ("sources", "Sources"),
        ("queries", "Queries"),
        ("comparisons", "Comparisons"),
        ("synthesis", "Synthesis"),
        ("other", "Other"),
    ] {
        out.push_str(&format!("\n## {label}\n"));
        if let Some(entries) = groups.get_mut(key) {
            entries.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.path.cmp(&b.path)));
            for page in entries {
                let target = page
                    .path
                    .strip_prefix("wiki/")
                    .unwrap_or(&page.path)
                    .strip_suffix(".md")
                    .unwrap_or(&page.path);
                out.push_str(&format!("- [[{}|{}]]", target, wiki_label(&page.title)));
                if let Some(summary) = page.summary.as_deref().and_then(inline_log_value) {
                    out.push_str(&format!(" — {summary}"));
                }
                out.push('\n');
            }
        }
    }
    out
}

fn render_log(operations: &[Operation]) -> String {
    let mut out = String::from("# Wiki Log\n");
    for op in operations {
        let action = inline_log_value(&op.action).unwrap_or_else(|| "unknown".into());
        let target = inline_log_value(&op.target).unwrap_or_else(|| "unknown".into());
        out.push_str(&format!(
            "\n## [{}] {} | {}\n",
            day(&op.created_at),
            action,
            target
        ));
        out.push_str(&format!("Timestamp: {}\n", op.created_at.trim()));
        if let Some(detail) = trimmed(op.detail.as_deref()) {
            out.push_str(&format!("Detail: {}\n", detail));
        }
    }
    out
}

fn inline_log_value(value: &str) -> Option<String> {
    let cleaned = value
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn render_overview(snapshot: &Snapshot, pages: &[PlannedPage]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    let (mut created, mut updated) = ("unknown", "unknown");
    for (index, page) in pages.iter().enumerate() {
        *counts.entry(page.folder).or_default() += 1;
        if index == 0 || page.created.as_str() < created {
            created = &page.created;
        }
        if index == 0 || page.updated.as_str() > updated {
            updated = &page.updated;
        }
    }
    let mut out = vec![
        "---".to_string(),
        r#"type: "overview""#.to_string(),
        r#"title: "Project Overview""#.to_string(),
        format!("created: {}", yaml(created)),
        format!("updated: {}", yaml(updated)),
        "---".to_string(),
        String::new(),
        "# Overview".to_string(),
        String::new(),
        format!("- Sources: {}", snapshot.sources.len()),
        format!("- Pages: {}", pages.len()),
        format!("- Operations: {}", snapshot.operations.len()),
    ];
    if !counts.is_empty() {
        out.push(String::new());
        out.push("## Page Types".to_string());
        out.push(String::new());
        for (folder, count) in counts {
            out.push(format!("- {}: {}", folder, count));
        }
    }
    let purpose = text(&snapshot.purpose);
    if !purpose.trim().is_empty() {
        out.push(String::new());
        out.push("## Purpose Snapshot".to_string());
        out.push(String::new());
        out.push(purpose.trim_end().to_string());
    }
    out.join("\n") + "\n"
}

fn normalize_slug(slug: &str) -> Result<String, Error> {
    let slug = slug.trim().replace('\\', "/");
    if slug.is_empty() || slug.starts_with('/') || slug.len() > REL_PATH_MAX {
        return Err(Error(format!("unsafe page slug: {slug}")));
    }
    let mut parts = Vec::new();
    for part in slug.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(' ')
            || part.ends_with('.')
            || part.chars().count() > SLUG_SEGMENT_MAX
            || part.chars().any(bad_path_char)
        {
            return Err(Error(format!("unsafe page slug: {slug}")));
        }
        parts.push(part.to_string());
    }
    Ok(parts.join("/"))
}

fn validate_raw_ref(path: &str) -> Result<(), Error> {
    let path = path.trim().replace('\\', "/");
    if !(path.starts_with("raw/sources/") || path.starts_with("raw/assets/")) {
        return Err(Error(format!(
            "artifact reference must stay under raw/: {path}"
        )));
    }
    for component in Path::new(&path).components() {
        let Component::Normal(part) = component else {
            return Err(Error(format!("unsafe artifact reference: {path}")));
        };
        let part = part.to_string_lossy();
        if part.is_empty() || part == "." || part == ".." || part.chars().any(bad_path_char) {
            return Err(Error(format!("unsafe artifact reference: {path}")));
        }
    }
    Ok(())
}

fn normalize_source_id(id: &str) -> Result<String, Error> {
    let id = id.trim();
    if id.is_empty() {
        return Err(Error("source id must not be empty".into()));
    }
    let mut out = String::new();
    let mut dashed = false;
    for ch in id.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            dashed = false;
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '-' | '_') {
            dashed = false;
            Some(ch)
        } else if !dashed {
            dashed = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
            if out.chars().count() >= SOURCE_ID_MAX {
                break;
            }
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        Err(Error(format!(
            "source id cannot be normalized safely: {id}"
        )))
    } else {
        Ok(out)
    }
}

fn safe_basename(origin: &str) -> String {
    let raw = origin
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("source.txt");
    let mut out = String::new();
    let mut dashed = false;
    for ch in raw.chars() {
        if out.chars().count() >= BASENAME_MAX {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            out.push(ch);
            dashed = false;
        } else if !dashed {
            out.push('-');
            dashed = true;
        }
    }
    let out = out.trim_matches(['-', '.']).to_string();
    if out.is_empty() {
        "source.txt".into()
    } else if out.contains('.') {
        out
    } else {
        format!("{out}.txt")
    }
}

fn normalize_kind(kind: Option<&str>) -> String {
    let kind = kind.unwrap_or("other").trim().to_ascii_lowercase();
    if kind.is_empty() {
        "other".into()
    } else {
        kind
    }
}

fn normalize_provenance(values: &[String]) -> Result<Vec<String>, Error> {
    let values = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if let Some(value) = values
        .iter()
        .find(|value| !PROVENANCE_ORDER.contains(value))
    {
        return Err(Error(format!("unsupported page provenance: {value}")));
    }
    Ok(PROVENANCE_ORDER
        .iter()
        .filter(|value| values.contains(**value))
        .map(|value| (*value).to_string())
        .collect())
}

fn folder_for(kind: Option<&str>) -> &'static str {
    match normalize_kind(kind).as_str() {
        "entity" | "entities" => "entities",
        "concept" | "concepts" => "concepts",
        "source" | "sources" => "sources",
        "query" | "queries" => "queries",
        "comparison" | "comparisons" => "comparisons",
        "synthesis" => "synthesis",
        _ => "other",
    }
}

fn day(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
    {
        value[..10].to_string()
    } else {
        "undated".into()
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn text(value: &str) -> String {
    let mut value = value.replace("\r\n", "\n").replace('\r', "\n");
    if !value.is_empty() && !value.ends_with('\n') {
        value.push('\n');
    }
    value
}

fn yaml(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn wiki_label(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '|' | '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bad_path_char(ch: char) -> bool {
    ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')
}

fn obsidian_app_json() -> &'static str {
    "{\n  \"attachmentFolderPath\": \"raw/assets\",\n  \"userIgnoreFilters\": [\n    \".lwc\"\n  ],\n  \"useMarkdownLinks\": false,\n  \"newLinkFormat\": \"shortest\",\n  \"showUnsupportedFiles\": false\n}\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::{
            Arc, Barrier,
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lwc-artifacts-{tag}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn source_path_is_stable() {
        assert_eq!(
            source_artifact_rel_path("SRC 42", "/tmp/Foo Bar?.pdf").unwrap(),
            "raw/sources/src-42--Foo-Bar-.pdf"
        );
    }

    #[test]
    fn projection_lock_serializes_writers() {
        let root = temp_dir("projection-lock");
        let first = lock_projection(&root).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let (acquired, receiver) = mpsc::channel();
        let other_root = root.clone();
        let other_barrier = Arc::clone(&barrier);
        let handle = std::thread::spawn(move || {
            other_barrier.wait();
            let lock = lock_projection(&other_root).unwrap();
            acquired.send(()).unwrap();
            drop(lock);
        });
        barrier.wait();
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn materialize_snapshot_writes_core_tree() {
        let root = temp_dir("tree");
        let source_path = source_artifact_rel_path("source-1", "/tmp/Attention Paper.pdf").unwrap();
        let snapshot = Snapshot {
            schema: "# Schema\r\nLine 2".into(),
            purpose: "# Purpose\r\nExact".into(),
            sources: vec![Source {
                id: "source-1".into(),
                title: Some("Paper".into()),
                origin: "/tmp/Attention Paper.pdf".into(),
                content: "raw source body\r\nline 2".into(),
            }],
            pages: vec![Page {
                slug: "attention/mechanism".into(),
                title: "Attention\nMechanism".into(),
                kind: Some("concept".into()),
                summary: Some("Line 1\nLine 2".into()),
                body: "# Attention\n\nBody".into(),
                provenance: vec!["source-grounded".into(), "agent-observed".into()],
                source_artifact_paths: vec![source_path.clone()],
                created: "2026-07-28".into(),
                updated: "2026-07-29".into(),
            }],
            operations: vec![Operation {
                created_at: "2026-07-29T09:00:00Z".into(),
                action: "page_put\nextra".into(),
                target: "attention/mechanism|v1".into(),
                detail: Some("created concept page".into()),
            }],
        };

        let written = materialize_snapshot(&root, &snapshot).unwrap();
        assert!(written.contains(&"wiki/concepts/attention/mechanism.md".to_string()));
        let concept =
            fs::read_to_string(root.join("wiki/concepts/attention/mechanism.md")).unwrap();
        assert!(concept.contains("sources:\n  - \"raw/sources/source-1--Attention-Paper.pdf\""));
        assert!(concept.contains("provenance:\n  - \"source-grounded\"\n  - \"agent-observed\""));
        assert!(concept.contains("title: \"Attention\\nMechanism\""));
        assert_eq!(
            fs::read_to_string(root.join("raw/sources/source-1--Attention-Paper.pdf")).unwrap(),
            "raw source body\r\nline 2"
        );
        assert_eq!(
            fs::read_to_string(root.join("schema.md")).unwrap(),
            "# Schema\r\nLine 2"
        );
        assert_eq!(
            fs::read_to_string(root.join("purpose.md")).unwrap(),
            "# Purpose\r\nExact"
        );
        let index = fs::read_to_string(root.join("wiki/index.md")).unwrap();
        assert!(
            index.contains("[[concepts/attention/mechanism|Attention Mechanism]] — Line 1 Line 2")
        );
        let log = fs::read_to_string(root.join("wiki/log.md")).unwrap();
        assert!(log.contains("## [2026-07-29] page_put extra | attention/mechanism v1"));
        let obsidian = fs::read_to_string(root.join(".obsidian/app.json")).unwrap();
        assert!(obsidian.contains("\"attachmentFolderPath\": \"raw/assets\""));
    }

    #[test]
    fn removes_stale_projected_files_but_keeps_user_files() {
        let root = temp_dir("cleanup");
        let first = Snapshot {
            schema: "# Schema".into(),
            purpose: "# Purpose".into(),
            sources: Vec::new(),
            pages: vec![Page {
                slug: "alpha".into(),
                title: "Alpha".into(),
                kind: Some("concept".into()),
                summary: None,
                body: "Body".into(),
                provenance: Vec::new(),
                source_artifact_paths: Vec::new(),
                created: "2026-07-28".into(),
                updated: "2026-07-29".into(),
            }],
            operations: Vec::new(),
        };
        materialize_snapshot(&root, &first).unwrap();
        fs::write(root.join("wiki/concepts/user-notes.md"), "keep me").unwrap();

        let second = Snapshot {
            schema: "# Schema".into(),
            purpose: "# Purpose".into(),
            sources: Vec::new(),
            pages: vec![Page {
                slug: "alpha".into(),
                title: "Alpha".into(),
                kind: Some("entity".into()),
                summary: None,
                body: "Body".into(),
                provenance: Vec::new(),
                source_artifact_paths: Vec::new(),
                created: "2026-07-28".into(),
                updated: "2026-07-29".into(),
            }],
            operations: Vec::new(),
        };
        materialize_snapshot(&root, &second).unwrap();

        assert!(!root.join("wiki/concepts/alpha.md").exists());
        assert!(root.join("wiki/entities/alpha.md").is_file());
        assert_eq!(
            fs::read_to_string(root.join("wiki/concepts/user-notes.md")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn skips_rewriting_unchanged_readonly_file() {
        let root = temp_dir("readonly");
        let snapshot = Snapshot {
            schema: String::new(),
            purpose: String::new(),
            sources: vec![Source {
                id: "source-1".into(),
                title: None,
                origin: "/tmp/source.txt".into(),
                content: "unchanged raw bytes".into(),
            }],
            pages: Vec::new(),
            operations: Vec::new(),
        };

        materialize_snapshot(&root, &snapshot).unwrap();

        let raw_path = root.join("raw/sources/source-1--source.txt");
        let mut perms = fs::metadata(&raw_path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&raw_path, perms).unwrap();

        materialize_snapshot(&root, &snapshot).unwrap();

        assert_eq!(
            fs::read_to_string(&raw_path).unwrap(),
            "unchanged raw bytes"
        );
    }

    #[test]
    fn materialize_wiki_snapshot_keeps_existing_raw_files() {
        let root = temp_dir("wiki-only");
        let initial = Snapshot {
            schema: "# Schema v1".into(),
            purpose: "# Purpose v1".into(),
            sources: vec![Source {
                id: "source-1".into(),
                title: None,
                origin: "/tmp/source.txt".into(),
                content: "raw v1".into(),
            }],
            pages: vec![Page {
                slug: "alpha".into(),
                title: "Alpha".into(),
                kind: Some("concept".into()),
                summary: Some("first summary".into()),
                body: "Body v1".into(),
                provenance: Vec::new(),
                source_artifact_paths: vec!["raw/sources/source-1--source.txt".into()],
                created: "2026-07-28".into(),
                updated: "2026-07-29".into(),
            }],
            operations: vec![Operation {
                created_at: "2026-07-29T09:00:00Z".into(),
                action: "page_put".into(),
                target: "alpha".into(),
                detail: Some("created".into()),
            }],
        };
        materialize_snapshot(&root, &initial).unwrap();

        let updated = Snapshot {
            schema: "# Schema v2".into(),
            purpose: "# Purpose v2".into(),
            sources: vec![Source {
                id: "source-1".into(),
                title: None,
                origin: "/tmp/source.txt".into(),
                content: "raw v2 should not be written".into(),
            }],
            pages: vec![Page {
                slug: "alpha".into(),
                title: "Alpha".into(),
                kind: Some("concept".into()),
                summary: Some("second summary".into()),
                body: "Body v2".into(),
                provenance: Vec::new(),
                source_artifact_paths: vec!["raw/sources/source-1--source.txt".into()],
                created: "2026-07-28".into(),
                updated: "2026-07-30".into(),
            }],
            operations: vec![Operation {
                created_at: "2026-07-30T09:00:00Z".into(),
                action: "page_put".into(),
                target: "alpha".into(),
                detail: Some("updated".into()),
            }],
        };

        let written = materialize_wiki_snapshot(&root, &updated).unwrap();

        assert!(!written.contains(&"raw/sources/source-1--source.txt".to_string()));
        let raw_path = root.join("raw/sources/source-1--source.txt");
        assert!(raw_path.is_file());
        assert_eq!(fs::read_to_string(&raw_path).unwrap(), "raw v1");
        assert_eq!(
            fs::read_to_string(root.join("schema.md")).unwrap(),
            "# Schema v2"
        );
        assert_eq!(
            fs::read_to_string(root.join("purpose.md")).unwrap(),
            "# Purpose v2"
        );
        let concept = fs::read_to_string(root.join("wiki/concepts/alpha.md")).unwrap();
        assert!(concept.contains("summary: \"second summary\""));
        assert!(concept.contains("updated: \"2026-07-30\""));
        assert!(concept.ends_with("\n\nBody v2\n"));
        assert!(
            fs::read_to_string(root.join("wiki/log.md"))
                .unwrap()
                .contains("updated")
        );
    }

    #[test]
    fn rejects_slug_traversal() {
        let root = temp_dir("slug");
        let snapshot = Snapshot {
            schema: String::new(),
            purpose: String::new(),
            sources: Vec::new(),
            pages: vec![Page {
                slug: "../escape".into(),
                title: "Escape".into(),
                kind: Some("concept".into()),
                summary: None,
                body: "Body".into(),
                provenance: Vec::new(),
                source_artifact_paths: Vec::new(),
                created: "2026-07-29".into(),
                updated: "2026-07-29".into(),
            }],
            operations: Vec::new(),
        };
        assert!(
            materialize_snapshot(&root, &snapshot)
                .unwrap_err()
                .0
                .contains("unsafe page slug")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_parent() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("symlink");
        fs::create_dir_all(root.join("wiki")).unwrap();
        symlink(temp_dir("outside"), root.join("wiki/concepts")).unwrap();
        let snapshot = Snapshot {
            schema: String::new(),
            purpose: String::new(),
            sources: Vec::new(),
            pages: vec![Page {
                slug: "alpha".into(),
                title: "Alpha".into(),
                kind: Some("concept".into()),
                summary: None,
                body: "Body".into(),
                provenance: Vec::new(),
                source_artifact_paths: Vec::new(),
                created: "2026-07-29".into(),
                updated: "2026-07-29".into(),
            }],
            operations: Vec::new(),
        };
        assert!(
            materialize_snapshot(&root, &snapshot)
                .unwrap_err()
                .0
                .contains("symlink")
        );
    }
}
