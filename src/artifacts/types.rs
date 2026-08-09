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
