use crate::error::{AppError, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const STORE_DIR: &str = ".lwc";
const STORE_FILE: &str = "wiki.db";
const PROJECT_ROOT_ENV: &str = "LWC_PROJECT_ROOT";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lower")]
pub enum Scope {
    Project,
    Global,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorePath {
    pub scope: Scope,
    pub path: PathBuf,
    authority_path: PathBuf,
}

impl StorePath {
    pub(crate) fn new(scope: Scope, path: PathBuf) -> Self {
        Self {
            scope,
            authority_path: path.clone(),
            path,
        }
    }

    pub(crate) fn with_database(&self, path: PathBuf) -> Self {
        Self {
            scope: self.scope,
            path,
            authority_path: self.authority_path.clone(),
        }
    }

    pub(crate) fn authority_path(&self) -> &Path {
        &self.authority_path
    }
}

pub fn init_store_path(scope: Scope, cwd: &Path) -> Result<StorePath> {
    match scope {
        Scope::Project => Ok(StorePath::new(
            Scope::Project,
            project_store(cwd, true)?.expect("initialization always returns a project path"),
        )),
        Scope::Global => {
            let path = global_store_path()?;
            inspect_store_path(&path, None)?;
            Ok(StorePath::new(Scope::Global, path))
        }
        Scope::All => Err(scope_not_supported("init")),
    }
}

pub fn resolve_store_path(scope: Scope, cwd: &Path) -> Result<StorePath> {
    match scope {
        Scope::Project => {
            let path = project_store(cwd, false)?.ok_or_else(|| {
                store_not_found("no project wiki found from the current directory")
            })?;
            Ok(StorePath::new(Scope::Project, path))
        }
        Scope::Global => {
            let path = global_store_path()?;
            if !inspect_store_path(&path, None)? {
                return Err(store_not_found("global wiki is not initialized"));
            }
            Ok(StorePath::new(Scope::Global, path))
        }
        Scope::All => Err(scope_not_supported("this command")),
    }
}

pub fn resolve_read_store_paths(
    scope: Scope,
    cwd: &Path,
    allow_all: bool,
) -> Result<Vec<StorePath>> {
    match scope {
        Scope::All => {
            if !allow_all {
                return Err(scope_not_supported("this command"));
            }

            let mut stores = Vec::with_capacity(2);
            if let Some(path) = project_store(cwd, false)? {
                stores.push(StorePath::new(Scope::Project, path));
            }

            let global = global_store_path()?;
            if inspect_store_path(&global, None)? {
                stores.push(StorePath::new(Scope::Global, global));
            }

            if stores.is_empty() {
                return Err(store_not_found("no project or global wiki is initialized"));
            }

            Ok(stores)
        }
        _ => resolve_store_path(scope, cwd).map(|store| vec![store]),
    }
}

pub fn ensure_scope_supported(scope: Scope, allow_all: bool, command: &str) -> Result<()> {
    if scope == Scope::All && !allow_all {
        return Err(scope_not_supported(command));
    }
    Ok(())
}

fn project_store_path(root: &Path) -> PathBuf {
    root.join(STORE_DIR).join(STORE_FILE)
}

fn project_store(cwd: &Path, initialize: bool) -> Result<Option<PathBuf>> {
    let configured = configured_project_paths(cwd)?;
    let (start, boundary, initialization_root) = match configured.as_ref() {
        Some((cwd, root)) => (cwd.as_path(), Some(root.as_path()), root.as_path()),
        None => (cwd, None, cwd),
    };

    let store = find_project_store(start, boundary)?
        .or_else(|| initialize.then(|| project_store_path(initialization_root)));
    if let Some(path) = store.as_deref() {
        let root = boundary.unwrap_or_else(|| {
            path.parent()
                .and_then(Path::parent)
                .expect("project store always has a project root")
        });
        if boundary.is_some() {
            ensure_project_path(path, root)?;
        }
        inspect_store_path(path, Some(root))?;
    }
    Ok(store)
}

fn configured_project_paths(cwd: &Path) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(root) = env::var_os(PROJECT_ROOT_ENV) else {
        return Ok(None);
    };
    if root.is_empty() {
        return Err(AppError::new(
            "project_root_invalid",
            format!("{PROJECT_ROOT_ENV} is empty"),
        ));
    }

    let root = fs::canonicalize(&root).map_err(|error| {
        AppError::new(
            "project_root_invalid",
            format!("cannot resolve {PROJECT_ROOT_ENV}: {error}"),
        )
    })?;
    if !root.is_dir() {
        return Err(AppError::new(
            "project_root_invalid",
            format!("{PROJECT_ROOT_ENV} is not a directory: {}", root.display()),
        ));
    }

    let cwd = fs::canonicalize(cwd)?;
    if !cwd.starts_with(&root) {
        return Err(AppError::new(
            "project_root_mismatch",
            format!(
                "current directory {} is outside {PROJECT_ROOT_ENV} {}",
                cwd.display(),
                root.display()
            ),
        ));
    }

    Ok(Some((cwd, root)))
}

pub(crate) fn ensure_project_path(path: &Path, root: &Path) -> Result<()> {
    if !path.starts_with(root) {
        return Err(project_root_escape(path, root, "path is outside the root"));
    }

    let mut existing = path;
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    project_root_escape(path, root, "path has no existing ancestor")
                })?;
            }
            Err(error) => {
                return Err(project_root_escape(path, root, &error.to_string()));
            }
        }
    }

    let resolved = fs::canonicalize(existing)
        .map_err(|error| project_root_escape(path, root, &error.to_string()))?;
    if !resolved.starts_with(root) {
        return Err(project_root_escape(
            path,
            root,
            &format!("{} resolves outside the root", existing.display()),
        ));
    }
    Ok(())
}

fn project_root_escape(path: &Path, root: &Path, reason: &str) -> AppError {
    AppError::new(
        "project_root_escape",
        format!(
            "project path {} escapes LWC_PROJECT_ROOT {}: {reason}",
            path.display(),
            root.display()
        ),
    )
}

fn find_project_store(start: &Path, configured_boundary: Option<&Path>) -> Result<Option<PathBuf>> {
    let global = global_store_path().ok();
    let boundary = configured_boundary.or_else(|| {
        global
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .filter(|home| start.starts_with(home))
    });
    let mut found: Option<PathBuf> = None;

    for candidate in start.ancestors() {
        let path = project_store_path(candidate);
        let exists = global.as_ref() != Some(&path)
            && inspect_store_path(&path, Some(configured_boundary.unwrap_or(candidate)))?;
        if exists {
            if configured_boundary.is_none() {
                return Ok(Some(path));
            }
            if let Some(previous) = found {
                return Err(AppError::new(
                    "project_scope_conflict",
                    format!(
                        "multiple project Wikis exist inside LWC_PROJECT_ROOT: {} and {}",
                        previous.display(),
                        path.display()
                    ),
                ));
            }
            found = Some(path);
        }
        if boundary == Some(candidate) {
            break;
        }
    }
    Ok(found)
}

fn inspect_store_path(path: &Path, project_root: Option<&Path>) -> Result<bool> {
    let directory = path
        .parent()
        .expect("wiki database path always has a parent directory");
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(invalid_store_path(
                path,
                project_root,
                "store directory is a symbolic link",
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(invalid_store_path(
                path,
                project_root,
                "store directory is not a directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(invalid_store_path(path, project_root, &error.to_string())),
    }

    let mut database_exists = false;
    for candidate in [
        path.to_path_buf(),
        store_sidecar(path, "-wal"),
        store_sidecar(path, "-shm"),
    ] {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_store_path(
                    &candidate,
                    project_root,
                    "store database or sidecar is not a regular file",
                ));
            }
            Ok(_) if candidate == path => database_exists = true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(invalid_store_path(
                    &candidate,
                    project_root,
                    &error.to_string(),
                ));
            }
        }
    }
    Ok(database_exists)
}

fn store_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    sidecar.into()
}

fn invalid_store_path(path: &Path, project_root: Option<&Path>, reason: &str) -> AppError {
    match project_root {
        Some(root) => project_root_escape(path, root, reason),
        None => AppError::new(
            "store_path_invalid",
            format!("Wiki store path {} is unsafe: {reason}", path.display()),
        ),
    }
}

fn global_store_path() -> Result<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty())
        })
        .or_else(|| {
            let drive = env::var_os("HOMEDRIVE")?;
            let path = env::var_os("HOMEPATH")?;
            let mut home = PathBuf::from(drive);
            home.push(path);
            (!home.as_os_str().is_empty()).then_some(home)
        })
        .ok_or_else(|| AppError::new("home_not_set", "user home directory is not available"))?;
    Ok(project_store_path(&home))
}

fn store_not_found(message: &str) -> AppError {
    AppError::new("store_not_found", message)
}

fn scope_not_supported(command: &str) -> AppError {
    AppError::new(
        "scope_not_supported",
        format!("--scope all is not supported for {command}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Mutex};
    use tempfile::TempDir;

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn project_scope_uses_nearest_ancestor_store() {
        let world = TempDir::new().unwrap();
        let project = world.path().join("project");
        let nested = project.join("a/b/c");
        let parent_store = project.join(STORE_DIR);
        fs::create_dir_all(&parent_store).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(parent_store.join(STORE_FILE), "").unwrap();

        let resolved = resolve_store_path(Scope::Project, &nested).unwrap();

        assert_eq!(resolved.scope, Scope::Project);
        assert_eq!(resolved.path, parent_store.join(STORE_FILE));
    }

    #[test]
    fn project_init_reuses_nearest_ancestor_store() {
        let world = TempDir::new().unwrap();
        let project = world.path().join("project");
        let nested = project.join("a/b/c");
        let parent_store = project.join(STORE_DIR);
        fs::create_dir_all(&parent_store).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(parent_store.join(STORE_FILE), "").unwrap();

        let resolved = init_store_path(Scope::Project, &nested).unwrap();

        assert_eq!(resolved.scope, Scope::Project);
        assert_eq!(resolved.path, parent_store.join(STORE_FILE));
        assert!(!nested.join(STORE_DIR).join(STORE_FILE).exists());
    }

    #[test]
    fn project_init_uses_current_directory_when_no_ancestor_store_exists() {
        let world = TempDir::new().unwrap();
        let nested = world.path().join("fresh/a/b");
        fs::create_dir_all(&nested).unwrap();

        let resolved = init_store_path(Scope::Project, &nested).unwrap();

        assert_eq!(resolved.scope, Scope::Project);
        assert_eq!(resolved.path, nested.join(STORE_DIR).join(STORE_FILE));
    }

    #[test]
    fn project_scope_does_not_reuse_global_store_at_home() {
        let _lock = HOME_LOCK.lock().unwrap();
        let world = TempDir::new().unwrap();
        let home = world.path().join("home");
        let project = home.join("work/project");
        let global_store = home.join(STORE_DIR);
        fs::create_dir_all(&global_store).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(global_store.join(STORE_FILE), "").unwrap();

        let home_text = home.to_string_lossy().into_owned();
        // SAFETY: test-only environment mutation is scoped to this process.
        unsafe { env::set_var("HOME", &home_text) };

        assert!(resolve_store_path(Scope::Project, &project).is_err());
        assert_eq!(
            init_store_path(Scope::Project, &project).unwrap().path,
            project.join(STORE_DIR).join(STORE_FILE)
        );
    }

    #[test]
    fn project_scope_does_not_search_above_home_boundary() {
        let _lock = HOME_LOCK.lock().unwrap();
        let world = TempDir::new().unwrap();
        let home = world.path().join("isolated-home");
        let project = home.join("work/project");
        let outside_store = world.path().join(STORE_DIR);
        fs::create_dir_all(&outside_store).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::write(outside_store.join(STORE_FILE), "").unwrap();

        let home_text = home.to_string_lossy().into_owned();
        // SAFETY: test-only environment mutation is scoped to this process.
        unsafe { env::set_var("HOME", &home_text) };

        assert!(resolve_store_path(Scope::Project, &project).is_err());
        assert_eq!(
            init_store_path(Scope::Project, &project).unwrap().path,
            project.join(STORE_DIR).join(STORE_FILE)
        );
    }

    #[test]
    fn global_scope_uses_home_directory_store() {
        let _lock = HOME_LOCK.lock().unwrap();
        let world = TempDir::new().unwrap();
        let home = world.path().join("home");
        let store_dir = home.join(STORE_DIR);
        fs::create_dir_all(&store_dir).unwrap();
        fs::write(store_dir.join(STORE_FILE), "").unwrap();

        let home_text = home.to_string_lossy().into_owned();
        // SAFETY: test-only environment mutation is scoped to this process.
        unsafe { env::set_var("HOME", &home_text) };

        let resolved = resolve_store_path(Scope::Global, world.path()).unwrap();

        assert_eq!(resolved.scope, Scope::Global);
        assert_eq!(resolved.path, store_dir.join(STORE_FILE));
    }

    #[test]
    fn all_scope_errors_when_no_store_exists() {
        let _lock = HOME_LOCK.lock().unwrap();
        let world = TempDir::new().unwrap();
        let home = world.path().join("home");
        fs::create_dir_all(&home).unwrap();

        let home_text = home.to_string_lossy().into_owned();
        // SAFETY: test-only environment mutation is scoped to this process.
        unsafe { env::set_var("HOME", &home_text) };

        let error = resolve_read_store_paths(Scope::All, world.path(), true).unwrap_err();

        assert_eq!(error.code, "store_not_found");
    }

    #[test]
    fn unsupported_all_scope_returns_scope_not_supported() {
        let world = TempDir::new().unwrap();

        let error = resolve_read_store_paths(Scope::All, world.path(), false).unwrap_err();

        assert_eq!(error.code, "scope_not_supported");
    }
}
