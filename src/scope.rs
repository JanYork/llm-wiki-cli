use crate::error::{AppError, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
};

const STORE_DIR: &str = ".lwc";
const STORE_FILE: &str = "wiki.db";

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
}

impl StorePath {
    fn new(scope: Scope, path: PathBuf) -> Self {
        Self { scope, path }
    }
}

pub fn init_store_path(scope: Scope, cwd: &Path) -> Result<StorePath> {
    match scope {
        Scope::Project => Ok(StorePath::new(
            Scope::Project,
            find_project_store(cwd).unwrap_or_else(|| project_store_path(cwd)),
        )),
        Scope::Global => Ok(StorePath::new(Scope::Global, global_store_path()?)),
        Scope::All => Err(scope_not_supported("init")),
    }
}

pub fn resolve_store_path(scope: Scope, cwd: &Path) -> Result<StorePath> {
    match scope {
        Scope::Project => {
            let path = find_project_store(cwd).ok_or_else(|| {
                store_not_found("no project wiki found from the current directory")
            })?;
            Ok(StorePath::new(Scope::Project, path))
        }
        Scope::Global => {
            let path = global_store_path()?;
            if !path.is_file() {
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
            if let Some(path) = find_project_store(cwd) {
                stores.push(StorePath::new(Scope::Project, path));
            }

            let global = global_store_path()?;
            if global.is_file() {
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

fn find_project_store(start: &Path) -> Option<PathBuf> {
    let global = global_store_path().ok();
    let home = global
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|home| start.starts_with(home));

    for candidate in start.ancestors() {
        let path = project_store_path(candidate);
        if global.as_ref() != Some(&path) && path.is_file() {
            return Some(path);
        }
        if home == Some(candidate) {
            break;
        }
    }
    None
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
