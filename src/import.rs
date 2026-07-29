use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

const ALLOWED_EXTENSIONS: &[&str] = &[
    "md", "mdx", "txt", "csv", "json", "html", "htm", "rtf", "xml", "yaml", "yml", "org", "sql",
    "base",
];
const SKIPPED_DIRS: &[&str] = &[".lwc", ".git", ".obsidian", ".claudian"];
const MAX_DOCUMENTS: usize = 100_000;

#[derive(Debug)]
pub enum ImportError {
    InvalidRoot(PathBuf),
    TooManyFiles {
        limit: usize,
    },
    Io {
        path: PathBuf,
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot(path) => write!(
                f,
                "import root must be a non-symlink directory: {}",
                path.display()
            ),
            Self::TooManyFiles { limit } => {
                write!(f, "import document count exceeds limit {}", limit)
            }
            Self::Io {
                path,
                operation,
                source,
            } => write!(f, "{} {}: {}", operation, path.display(), source),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn collect_documents(root: &Path) -> Result<Vec<PathBuf>, ImportError> {
    ensure_valid_root(root)?;

    let mut documents = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut entries = Vec::new();
        let read_dir = fs::read_dir(&dir).map_err(|source| ImportError::Io {
            path: dir.clone(),
            operation: "failed to read directory",
            source,
        })?;

        for entry in read_dir {
            let entry = entry.map_err(|source| ImportError::Io {
                path: dir.clone(),
                operation: "failed to read directory entry",
                source,
            })?;
            entries.push(entry);
        }

        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| ImportError::Io {
                path: path.clone(),
                operation: "failed to inspect path",
                source,
            })?;

            if metadata.file_type().is_symlink() {
                continue;
            }

            let relative = path
                .strip_prefix(root)
                .map_err(|_| ImportError::InvalidRoot(root.into()))?;
            if is_hidden_relative_path(relative) {
                continue;
            }

            if metadata.is_dir() {
                if should_skip_dir(&path) {
                    continue;
                }
                stack.push(path);
                continue;
            }

            if !metadata.is_file() || !has_allowed_extension(&path) {
                continue;
            }

            documents.push(path);
            if documents.len() > MAX_DOCUMENTS {
                return Err(ImportError::TooManyFiles {
                    limit: MAX_DOCUMENTS,
                });
            }
        }
    }

    documents.sort_by_cached_key(|path| relative_sort_key(root, path));
    Ok(documents)
}

fn ensure_valid_root(root: &Path) -> Result<(), ImportError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| ImportError::Io {
        path: root.to_path_buf(),
        operation: "failed to inspect import root",
        source,
    })?;

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ImportError::InvalidRoot(root.to_path_buf()));
    }

    Ok(())
}

fn has_allowed_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| {
            ALLOWED_EXTENSIONS
                .iter()
                .any(|allowed| ext.eq_ignore_ascii_case(allowed))
        })
        .unwrap_or(false)
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .map(|name| SKIPPED_DIRS.contains(&name))
        .unwrap_or(false)
}

fn is_hidden_relative_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => name
            .to_str()
            .map(|part| part.starts_with('.'))
            .unwrap_or(false),
        _ => false,
    })
}

fn relative_sort_key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::{ImportError, collect_documents};
    use std::{
        env, fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn collect_documents_filters_recurses_and_sorts() {
        let root = test_root("filters");
        write_file(&root.join("zeta.MD"));
        write_file(&root.join("alpha/base.BASE"));
        write_file(&root.join("alpha/note.txt"));
        write_file(&root.join("alpha/skip.bin"));
        write_file(&root.join("beta/deep/report.JSON"));
        write_file(&root.join("beta/deep/report.tmp"));
        write_file(&root.join(".hidden/secret.md"));
        write_file(&root.join(".hidden.txt"));
        write_file(&root.join(".git/ignored.md"));
        write_file(&root.join(".obsidian/vault.md"));

        let actual = relative_paths(&root, collect_documents(&root).unwrap());
        let expected = vec![
            PathBuf::from("alpha/base.BASE"),
            PathBuf::from("alpha/note.txt"),
            PathBuf::from("beta/deep/report.JSON"),
            PathBuf::from("zeta.MD"),
        ];
        assert_eq!(actual, expected);

        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn collect_documents_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlinks");
        let target_dir = root.join("nested");
        let target_file = target_dir.join("kept.md");
        write_file(&target_file);
        symlink(&target_file, root.join("linked.md")).unwrap();
        symlink(&target_dir, root.join("linked-dir")).unwrap();

        let actual = relative_paths(&root, collect_documents(&root).unwrap());
        assert_eq!(actual, vec![PathBuf::from("nested/kept.md")]);

        cleanup(&root);
    }

    #[test]
    fn collect_documents_rejects_invalid_root() {
        let root = test_root("invalid-root");
        let file_root = root.join("plain.txt");
        write_file(&file_root);
        assert!(matches!(
            collect_documents(&file_root),
            Err(ImportError::InvalidRoot(path)) if path == file_root
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let real_dir = root.join("real-dir");
            fs::create_dir_all(&real_dir).unwrap();
            let symlink_root = root.join("dir-link");
            symlink(&real_dir, &symlink_root).unwrap();
            assert!(matches!(
                collect_documents(&symlink_root),
                Err(ImportError::InvalidRoot(path)) if path == symlink_root
            ));
        }

        cleanup(&root);
    }

    fn relative_paths(root: &Path, documents: Vec<PathBuf>) -> Vec<PathBuf> {
        documents
            .into_iter()
            .map(|path| path.strip_prefix(root).unwrap().to_path_buf())
            .collect()
    }

    fn write_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"test").unwrap();
    }

    fn test_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("lwc-import-{label}-{unique}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }
}
