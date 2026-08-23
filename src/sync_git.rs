use crate::{
    error::{AppError, Result},
    sync::SyncMode,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_INDEX_DIRECTORY: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
struct RepositoryFingerprint {
    head: String,
    index_tree: String,
    tracked_worktree_sha256: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitSyncSummary {
    pub(crate) status: String,
    pub(crate) result_commit: Option<String>,
    pub(crate) applied_local: bool,
    pub(crate) published_remote: bool,
    pub(crate) tracked_wip_preserved: bool,
    pub(crate) tracked_wip_included: bool,
    pub(crate) conflicts: Vec<Value>,
}

pub(crate) fn run(
    cwd: &Path,
    host: &str,
    remote_directory: &Path,
    mode: SyncMode,
    session_id: &str,
) -> Result<GitSyncSummary> {
    if !git(cwd, &["rev-parse", "--is-inside-work-tree"])?
        .status
        .success()
    {
        return Ok(skipped("skipped_not_repository"));
    }
    let original = repository_fingerprint(cwd)?;
    let original_tracked_wip = tracked_wip(cwd)?;
    let remote = format!("{host}:{}", remote_directory.display());
    let remote_branch = remote_head_branch(cwd, &remote)?.unwrap_or_else(|| {
        let branch = git_text(cwd, &["symbolic-ref", "--short", "HEAD"])
            .unwrap_or_else(|_| "main".to_string());
        format!("refs/heads/{branch}")
    });
    let sync_ref = format!("refs/lwc-sync/{session_id}/remote");
    let fetch_spec = format!("+HEAD:{sync_ref}");
    let fetched = git(cwd, &["fetch", "--no-tags", &remote, &fetch_spec])?;
    if !fetched.status.success() {
        return Ok(GitSyncSummary {
            status: "remote_git_unavailable".to_string(),
            result_commit: None,
            applied_local: false,
            published_remote: false,
            tracked_wip_preserved: tracked_wip(cwd)?,
            tracked_wip_included: false,
            conflicts: vec![json!({"kind":"git","message":bounded_output(&fetched)})],
        });
    }
    require_unchanged(cwd, &original)?;
    let local_head = git_text(cwd, &["rev-parse", "HEAD"])?;
    let local = tracked_worktree_commit(cwd, &local_head, &original.index_tree, session_id)?;
    require_unchanged(cwd, &original)?;
    let remote_head = git_text(cwd, &["rev-parse", &sync_ref])?;
    let result = if is_ancestor(cwd, &remote_head, &local)? {
        local.clone()
    } else if is_ancestor(cwd, &local, &remote_head)? {
        remote_head.clone()
    } else {
        merge_commits_isolated(cwd, &local, &remote_head, session_id)?
    };
    let result_ref = format!("refs/lwc-sync/{session_id}/merged");
    require_git(
        cwd,
        &["update-ref", &result_ref, &result],
        "record merged Git result",
    )?;

    let mut published_remote = false;
    if mode != SyncMode::Pull && result != remote_head {
        require_unchanged(cwd, &original)?;
        let push_spec = format!("{result}:{remote_branch}");
        let lease = format!("--force-with-lease={remote_branch}:{remote_head}");
        let pushed = git(cwd, &["push", &lease, &remote, &push_spec])?;
        if !pushed.status.success() {
            return Ok(GitSyncSummary {
                status: "pending_remote_push".to_string(),
                result_commit: Some(result),
                applied_local: false,
                published_remote: false,
                tracked_wip_preserved: tracked_wip(cwd)?,
                tracked_wip_included: original_tracked_wip,
                conflicts: vec![json!({
                    "kind":"git",
                    "message":bounded_output(&pushed),
                })],
            });
        }
        require_unchanged(cwd, &original)?;
        published_remote = true;
    }

    let dirty = tracked_wip(cwd)?;
    let mut applied_local = false;
    let mut status = "completed".to_string();
    if mode != SyncMode::Push && result != local {
        require_unchanged(cwd, &original)?;
        if dirty {
            status = "pending_local_wip".to_string();
        } else {
            let applied = git(cwd, &["merge", "--ff-only", &result])?;
            if applied.status.success() {
                applied_local = true;
            } else {
                status = "pending_worktree_collision".to_string();
            }
        }
    }
    if status == "completed" {
        cleanup_ref_if_expected(cwd, &sync_ref, &remote_head)?;
        cleanup_ref_if_expected(cwd, &result_ref, &result)?;
    }
    Ok(GitSyncSummary {
        status,
        result_commit: Some(result),
        applied_local,
        published_remote,
        tracked_wip_preserved: dirty,
        tracked_wip_included: original_tracked_wip,
        conflicts: Vec::new(),
    })
}

fn repository_fingerprint(cwd: &Path) -> Result<RepositoryFingerprint> {
    let diff = git(cwd, &["diff", "--no-ext-diff", "--binary", "--"])?;
    if !diff.status.success() {
        return Err(AppError::new("sync_git_failed", bounded_output(&diff)));
    }
    Ok(RepositoryFingerprint {
        head: git_text(cwd, &["rev-parse", "HEAD"])?,
        index_tree: git_text(cwd, &["write-tree"])?,
        tracked_worktree_sha256: sha256_hex(&diff.stdout),
    })
}

fn require_unchanged(cwd: &Path, expected: &RepositoryFingerprint) -> Result<()> {
    if repository_fingerprint(cwd)? == *expected {
        Ok(())
    } else {
        Err(AppError::new(
            "sync_git_local_changed",
            "Git HEAD, index, or tracked worktree changed during Sync; resume to reconcile the newer state",
        ))
    }
}

fn merge_commits_isolated(cwd: &Path, local: &str, remote: &str, session: &str) -> Result<String> {
    let merge = git(
        cwd,
        &["merge-tree", "--write-tree", "--name-only", local, remote],
    )?;
    let text = String::from_utf8_lossy(&merge.stdout);
    let tree = text
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| line.len() == 40 || line.len() == 64)
        .ok_or_else(|| {
            AppError::new("sync_git_failed", "git merge-tree omitted its result tree")
        })?;
    let tree = if merge.status.success() {
        tree.to_string()
    } else {
        let paths = text
            .lines()
            .skip(1)
            .take_while(|line| !line.is_empty())
            .take(100)
            .map(str::to_string)
            .collect::<Vec<_>>();
        preserve_conflicting_files(cwd, tree, local, remote, session, &paths)?
    };
    commit_tree(cwd, &tree, local, remote, session)
}

fn tracked_worktree_commit(
    cwd: &Path,
    head: &str,
    index_tree: &str,
    session: &str,
) -> Result<String> {
    let index = TemporaryIndex::new(cwd, &format!("{session}-tracked-worktree"))?;
    require_index_git(
        cwd,
        &index.path,
        &["read-tree", index_tree],
        "stage Git index",
    )?;
    require_index_git(
        cwd,
        &index.path,
        &["add", "-u", "--", ":/"],
        "stage tracked worktree changes",
    )?;
    let tree = index_git_text(cwd, &index.path, &["write-tree"])?;
    if tree == git_text(cwd, &["rev-parse", &format!("{head}^{{tree}}")])? {
        Ok(head.to_string())
    } else {
        commit_tree_with_parents(
            cwd,
            &tree,
            &[head],
            &format!("lwc sync {session} tracked worktree"),
        )
    }
}

struct TemporaryIndex {
    path: PathBuf,
    directory: PathBuf,
}

impl TemporaryIndex {
    fn new(cwd: &Path, session: &str) -> Result<Self> {
        let digest = sha256_hex(session.as_bytes());
        let common = PathBuf::from(git_text(
            cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?);
        require_real_directory(&common, "Git common directory")?;
        let root = common.join("lwc-sync");
        ensure_private_directory(&root)?;
        for _ in 0..100 {
            let directory = root.join(format!(
                "{}-{}-{}",
                std::process::id(),
                NEXT_INDEX_DIRECTORY.fetch_add(1, Ordering::Relaxed),
                &digest[..16]
            ));
            match create_private_directory(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        path: directory.join("index"),
                        directory,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(AppError::new("sync_git_failed", error.to_string())),
            }
        }
        Err(AppError::new(
            "sync_git_failed",
            "could not reserve a private temporary Git index directory",
        ))
    }
}

impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("lock"));
        let _ = fs::remove_dir(&self.directory);
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match create_private_directory(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            require_real_directory(path, "temporary Git index root")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::symlink_metadata(path)
                    .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?
                    .permissions()
                    .mode();
                if mode & 0o077 != 0 {
                    return Err(AppError::new(
                        "sync_git_failed",
                        "temporary Git index root is not private",
                    ));
                }
            }
            Ok(())
        }
        Err(error) => Err(AppError::new("sync_git_failed", error.to_string())),
    }
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(AppError::new(
            "sync_git_failed",
            format!("{label} must be a real directory"),
        ))
    }
}

fn preserve_conflicting_files(
    cwd: &Path,
    merge_tree: &str,
    local: &str,
    remote: &str,
    session: &str,
    paths: &[String],
) -> Result<String> {
    if paths.is_empty() {
        return Err(AppError::new(
            "sync_git_failed",
            "Git reported a conflict without bounded conflict paths",
        ));
    }
    let index = TemporaryIndex::new(cwd, session)?;
    require_index_git(
        cwd,
        &index.path,
        &["read-tree", merge_tree],
        "stage merge tree",
    )?;
    for path in paths {
        match tree_entry(cwd, local, path)? {
            Some((mode, oid)) => require_index_git(
                cwd,
                &index.path,
                &["update-index", "--add", "--cacheinfo", &mode, &oid, path],
                "preserve local conflict candidate",
            )?,
            None => require_index_git(
                cwd,
                &index.path,
                &["update-index", "--force-remove", "--", path],
                "preserve local deletion",
            )?,
        }
        if let Some((mode, oid)) = tree_entry(cwd, remote, path)? {
            let variant = remote_variant_path(cwd, local, remote, path)?;
            require_index_git(
                cwd,
                &index.path,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &mode,
                    &oid,
                    &variant,
                ],
                "preserve remote conflict candidate",
            )?;
        }
    }
    index_git_text(cwd, &index.path, &["write-tree"])
}

fn tree_entry(cwd: &Path, commit: &str, path: &str) -> Result<Option<(String, String)>> {
    let output = git(cwd, &["ls-tree", commit, "--", path])?;
    if !output.status.success() {
        return Err(AppError::new("sync_git_failed", bounded_output(&output)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((metadata, _)) = text.split_once('\t') else {
        return Ok(None);
    };
    let mut fields = metadata.split_whitespace();
    let mode = fields.next().unwrap_or_default();
    let kind = fields.next().unwrap_or_default();
    let oid = fields.next().unwrap_or_default();
    if kind != "blob" || mode.is_empty() || oid.is_empty() {
        return Err(AppError::new(
            "sync_git_failed",
            format!("unsupported Git conflict entry at {path}"),
        ));
    }
    Ok(Some((mode.to_string(), oid.to_string())))
}

fn remote_variant_path(cwd: &Path, local: &str, remote: &str, path: &str) -> Result<String> {
    let simple = format!("{path}.lwc-sync-remote");
    if tree_entry(cwd, local, &simple)?.is_none() && tree_entry(cwd, remote, &simple)?.is_none() {
        return Ok(simple);
    }
    let stable = format!("{simple}-{}", &remote[..12.min(remote.len())]);
    if tree_entry(cwd, local, &stable)?.is_none() && tree_entry(cwd, remote, &stable)?.is_none() {
        return Ok(stable);
    }
    for index in 2..=100 {
        let candidate = format!("{stable}-{index}");
        if tree_entry(cwd, local, &candidate)?.is_none()
            && tree_entry(cwd, remote, &candidate)?.is_none()
        {
            return Ok(candidate);
        }
    }
    Err(AppError::new(
        "sync_git_failed",
        format!("no bounded preserve-both variant is available for {path}"),
    ))
}

fn require_index_git(cwd: &Path, index: &Path, args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "sync_git_failed",
            format!("failed to {action}: {}", bounded_output(&output)),
        ))
    }
}

fn index_git_text(cwd: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::new("sync_git_failed", bounded_output(&output)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn skipped(status: &str) -> GitSyncSummary {
    GitSyncSummary {
        status: status.to_string(),
        result_commit: None,
        applied_local: false,
        published_remote: false,
        tracked_wip_preserved: false,
        tracked_wip_included: false,
        conflicts: Vec::new(),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))
}

fn git_text(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = git(cwd, args)?;
    if !output.status.success() {
        return Err(AppError::new("sync_git_failed", bounded_output(&output)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn require_git(cwd: &Path, args: &[&str], action: &str) -> Result<()> {
    let output = git(cwd, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            "sync_git_failed",
            format!("failed to {action}: {}", bounded_output(&output)),
        ))
    }
}

fn cleanup_ref_if_expected(cwd: &Path, reference: &str, expected: &str) -> Result<()> {
    let deleted = git(cwd, &["update-ref", "-d", reference, expected])?;
    if deleted.status.success() {
        return Ok(());
    }
    let current = git(cwd, &["rev-parse", "--verify", "--quiet", reference])?;
    match current.status.code() {
        Some(1) => Ok(()),
        Some(0) if String::from_utf8_lossy(&current.stdout).trim() != expected => Ok(()),
        _ => Err(AppError::new(
            "sync_git_failed",
            format!(
                "failed to clean owned Sync ref: {}",
                bounded_output(&deleted)
            ),
        )),
    }
}

fn remote_head_branch(cwd: &Path, remote: &str) -> Result<Option<String>> {
    let output = git(cwd, &["ls-remote", "--symref", remote, "HEAD"])?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: ")
                .and_then(|line| line.split_once('\t'))
                .map(|(name, _)| name.to_string())
        }))
}

fn is_ancestor(cwd: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    Ok(
        git(cwd, &["merge-base", "--is-ancestor", ancestor, descendant])?
            .status
            .success(),
    )
}

fn tracked_wip(cwd: &Path) -> Result<bool> {
    Ok(
        !git(cwd, &["status", "--porcelain", "--untracked-files=no"])?
            .stdout
            .is_empty(),
    )
}

fn commit_tree(cwd: &Path, tree: &str, local: &str, remote: &str, session: &str) -> Result<String> {
    commit_tree_with_parents(cwd, tree, &[local, remote], &format!("lwc sync {session}"))
}

fn commit_tree_with_parents(
    cwd: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String> {
    let mut args = vec!["commit-tree", tree];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-m", message]);
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "LWC Sync")
        .env("GIT_AUTHOR_EMAIL", "lwc-sync@localhost")
        .env("GIT_COMMITTER_NAME", "LWC Sync")
        .env("GIT_COMMITTER_EMAIL", "lwc-sync@localhost")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .args(args)
        .output()
        .map_err(|error| AppError::new("sync_git_failed", error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::new("sync_git_failed", bounded_output(&output)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn bounded_output(output: &Output) -> String {
    String::from_utf8_lossy(if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    })
    .chars()
    .take(4096)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TestRepo(PathBuf);

    impl TestRepo {
        fn new() -> Self {
            Self::new_with_init(&["init", "-q"])
        }

        fn new_sha256() -> Self {
            Self::new_with_init(&["init", "-q", "--object-format=sha256"])
        }

        fn new_with_init(init: &[&str]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "lwc-sync-git-test-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            require_git(&path, init, "initialize test repository").unwrap();
            require_git(&path, &["config", "user.name", "LWC Test"], "set user name").unwrap();
            require_git(
                &path,
                &["config", "user.email", "lwc-test@localhost"],
                "set user email",
            )
            .unwrap();
            Self(path)
        }

        fn commit(&self, path: &str, body: &str, message: &str) -> String {
            fs::write(self.0.join(path), body).unwrap();
            require_git(&self.0, &["add", path], "stage test file").unwrap();
            require_git(
                &self.0,
                &["commit", "-q", "-m", message],
                "commit test file",
            )
            .unwrap();
            git_text(&self.0, &["rev-parse", "HEAD"]).unwrap()
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fingerprint_detects_head_index_and_tracked_worktree_without_tracking_local_extras() {
        let repo = TestRepo::new();
        repo.commit("tracked.txt", "base\n", "base");
        let base = repository_fingerprint(&repo.0).unwrap();

        fs::write(repo.0.join("untracked.txt"), "keep\n").unwrap();
        fs::write(repo.0.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(repo.0.join("ignored.txt"), "keep\n").unwrap();
        assert_eq!(repository_fingerprint(&repo.0).unwrap(), base);

        fs::write(repo.0.join("tracked.txt"), "worktree\n").unwrap();
        let worktree = repository_fingerprint(&repo.0).unwrap();
        assert_ne!(worktree, base);
        require_git(&repo.0, &["add", "tracked.txt"], "stage tracked edit").unwrap();
        let index = repository_fingerprint(&repo.0).unwrap();
        assert_ne!(index, worktree);
        require_git(&repo.0, &["commit", "-q", "-m", "head"], "advance head").unwrap();
        assert_ne!(repository_fingerprint(&repo.0).unwrap(), index);
    }

    #[test]
    fn conflicting_commits_are_reconciled_outside_the_original_worktree() {
        let repo = TestRepo::new();
        let base = repo.commit("guide.md", "base\n", "base");
        require_git(
            &repo.0,
            &["branch", "remote", &base],
            "create remote branch",
        )
        .unwrap();
        let local = repo.commit("guide.md", "local\n", "local");
        require_git(&repo.0, &["checkout", "-q", "remote"], "checkout remote").unwrap();
        let remote = repo.commit("guide.md", "remote\n", "remote");
        require_git(&repo.0, &["checkout", "-q", "-"], "restore local branch").unwrap();

        fs::write(repo.0.join("guide.md"), "tracked wip\n").unwrap();
        fs::write(repo.0.join("untracked.keep"), "untracked\n").unwrap();
        fs::write(repo.0.join(".gitignore"), "ignored.keep\n").unwrap();
        fs::write(repo.0.join("ignored.keep"), "ignored\n").unwrap();
        let before = fs::read(repo.0.join("guide.md")).unwrap();

        let merged = merge_commits_isolated(&repo.0, &local, &remote, "session-1").unwrap();

        assert_eq!(fs::read(repo.0.join("guide.md")).unwrap(), before);
        assert_eq!(
            fs::read(repo.0.join("untracked.keep")).unwrap(),
            b"untracked\n"
        );
        assert_eq!(fs::read(repo.0.join("ignored.keep")).unwrap(), b"ignored\n");
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:guide.md")]).unwrap(),
            "local"
        );
        assert_eq!(
            git_text(
                &repo.0,
                &["show", &format!("{merged}:guide.md.lwc-sync-remote")]
            )
            .unwrap(),
            "remote"
        );
    }

    #[test]
    fn isolated_merge_accepts_sha256_result_trees() {
        let repo = TestRepo::new_sha256();
        let base = repo.commit("base.txt", "base\n", "base");
        require_git(
            &repo.0,
            &["branch", "remote", &base],
            "create remote branch",
        )
        .unwrap();
        let local = repo.commit("local.txt", "local\n", "local");
        require_git(&repo.0, &["checkout", "-q", "remote"], "checkout remote").unwrap();
        let remote = repo.commit("remote.txt", "remote\n", "remote");
        require_git(&repo.0, &["checkout", "-q", "-"], "restore local branch").unwrap();

        let merged = merge_commits_isolated(&repo.0, &local, &remote, "sha256").unwrap();

        assert_eq!(merged.len(), 64);
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:local.txt")]).unwrap(),
            "local"
        );
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:remote.txt")]).unwrap(),
            "remote"
        );
    }

    #[test]
    fn conflict_variant_never_overwrites_an_existing_variant_path() {
        let repo = TestRepo::new();
        let base = repo.commit("guide.md", "base\n", "base");
        require_git(
            &repo.0,
            &["branch", "remote", &base],
            "create remote branch",
        )
        .unwrap();
        require_git(&repo.0, &["checkout", "-q", "remote"], "checkout remote").unwrap();
        let remote = repo.commit("guide.md", "remote\n", "remote");
        require_git(&repo.0, &["checkout", "-q", "-"], "restore local branch").unwrap();
        fs::write(repo.0.join("guide.md"), "local\n").unwrap();
        fs::write(repo.0.join("guide.md.lwc-sync-remote"), "simple\n").unwrap();
        let occupied = format!("guide.md.lwc-sync-remote-{}", &remote[..12]);
        fs::write(repo.0.join(&occupied), "occupied\n").unwrap();
        fs::write(repo.0.join(format!("{occupied}-2")), "occupied-2\n").unwrap();
        require_git(&repo.0, &["add", "."], "stage local collision").unwrap();
        require_git(&repo.0, &["commit", "-q", "-m", "local"], "commit local").unwrap();
        let local = git_text(&repo.0, &["rev-parse", "HEAD"]).unwrap();

        let merged = merge_commits_isolated(&repo.0, &local, &remote, "collision").unwrap();

        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:{occupied}")]).unwrap(),
            "occupied"
        );
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:{occupied}-2")]).unwrap(),
            "occupied-2"
        );
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{merged}:{occupied}-3")]).unwrap(),
            "remote"
        );
    }

    #[test]
    fn delete_vs_edit_keeps_the_deletion_and_a_remote_variant() {
        let repo = TestRepo::new();
        let base = repo.commit("guide.md", "base\n", "base");
        require_git(
            &repo.0,
            &["branch", "remote", &base],
            "create remote branch",
        )
        .unwrap();
        fs::remove_file(repo.0.join("guide.md")).unwrap();
        require_git(&repo.0, &["add", "-u"], "stage local deletion").unwrap();
        require_git(
            &repo.0,
            &["commit", "-q", "-m", "delete"],
            "commit deletion",
        )
        .unwrap();
        let local = git_text(&repo.0, &["rev-parse", "HEAD"]).unwrap();
        require_git(&repo.0, &["checkout", "-q", "remote"], "checkout remote").unwrap();
        let remote = repo.commit("guide.md", "remote edit\n", "remote edit");
        require_git(&repo.0, &["checkout", "-q", "-"], "restore local branch").unwrap();

        let merged = merge_commits_isolated(&repo.0, &local, &remote, "delete-edit").unwrap();

        assert!(git_text(&repo.0, &["show", &format!("{merged}:guide.md")]).is_err());
        assert_eq!(
            git_text(
                &repo.0,
                &["show", &format!("{merged}:guide.md.lwc-sync-remote")]
            )
            .unwrap(),
            "remote edit"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temporary_index_rejects_a_prepositioned_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let repo = TestRepo::new();
        let head = repo.commit("tracked.txt", "base\n", "base");
        fs::create_dir(repo.0.join("sentinel-dir")).unwrap();
        fs::write(repo.0.join("sentinel-dir/sentinel"), "safe\n").unwrap();
        symlink(repo.0.join("sentinel-dir"), repo.0.join(".git/lwc-sync")).unwrap();
        let fingerprint = repository_fingerprint(&repo.0).unwrap();

        let error =
            tracked_worktree_commit(&repo.0, &head, &fingerprint.index_tree, "hostile-session")
                .unwrap_err();

        assert_eq!(error.code, "sync_git_failed");
        assert_eq!(
            fs::read(repo.0.join("sentinel-dir/sentinel")).unwrap(),
            b"safe\n"
        );
        assert_eq!(
            fs::read_dir(repo.0.join("sentinel-dir")).unwrap().count(),
            1
        );
    }

    #[test]
    fn tracked_worktree_commit_is_deterministic_across_lost_response_retries() {
        let repo = TestRepo::new();
        let head = repo.commit("tracked.txt", "base\n", "base");
        fs::write(repo.0.join("tracked.txt"), "dirty\n").unwrap();
        let fingerprint = repository_fingerprint(&repo.0).unwrap();

        let first =
            tracked_worktree_commit(&repo.0, &head, &fingerprint.index_tree, "retry").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let second =
            tracked_worktree_commit(&repo.0, &head, &fingerprint.index_tree, "retry").unwrap();

        assert_eq!(first, second);
        assert_ne!(first, head);
        assert_eq!(
            git_text(&repo.0, &["show", &format!("{first}:tracked.txt")]).unwrap(),
            "dirty"
        );
        assert_eq!(
            fs::read_dir(repo.0.join(".git/lwc-sync")).unwrap().count(),
            0
        );
    }

    #[test]
    fn ref_cleanup_deletes_only_the_expected_owned_oid() {
        let repo = TestRepo::new();
        let owned = repo.commit("tracked.txt", "owned\n", "owned");
        let external = repo.commit("tracked.txt", "external\n", "external");
        let owned_ref = "refs/lwc-sync/session/owned";
        let external_ref = "refs/lwc-sync/session/external";
        require_git(
            &repo.0,
            &["update-ref", owned_ref, &owned],
            "create owned ref",
        )
        .unwrap();
        require_git(
            &repo.0,
            &["update-ref", external_ref, &external],
            "simulate external ref rewrite",
        )
        .unwrap();

        cleanup_ref_if_expected(&repo.0, owned_ref, &owned).unwrap();
        cleanup_ref_if_expected(&repo.0, external_ref, &owned).unwrap();

        assert!(
            git(&repo.0, &["show-ref", "--verify", owned_ref])
                .unwrap()
                .stdout
                .is_empty()
        );
        assert_eq!(
            git_text(&repo.0, &["rev-parse", external_ref]).unwrap(),
            external
        );
    }

    #[test]
    fn ref_cleanup_distinguishes_missing_from_locked_owned_refs() {
        let repo = TestRepo::new();
        let owned = repo.commit("tracked.txt", "owned\n", "owned");
        let directory = repo.0.join(".git/refs/lwc-sync/session");
        fs::create_dir_all(&directory).unwrap();

        let missing_ref = "refs/lwc-sync/session/missing";
        fs::write(directory.join("missing.lock"), b"locked").unwrap();
        cleanup_ref_if_expected(&repo.0, missing_ref, &owned).unwrap();

        let locked_ref = "refs/lwc-sync/session/locked";
        require_git(
            &repo.0,
            &["update-ref", locked_ref, &owned],
            "create locked ref",
        )
        .unwrap();
        fs::write(directory.join("locked.lock"), b"locked").unwrap();
        let error = cleanup_ref_if_expected(&repo.0, locked_ref, &owned).unwrap_err();
        assert_eq!(error.code, "sync_git_failed");
        assert_eq!(
            git_text(&repo.0, &["rev-parse", locked_ref]).unwrap(),
            owned
        );
    }
}
