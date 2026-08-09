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
