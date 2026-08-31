#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

    fn hook_state(id: &str, updated_at_unix_ms: u128, failed: bool) -> Value {
        json!({
            "id": id,
            "kind": "graph-project",
            "scope": "project",
            "database": "/private/hook-secret/wiki.db",
            "state": if failed { "failed" } else { "running" },
            "phase": if failed { "failed" } else { "projecting" },
            "completed": updated_at_unix_ms,
            "total": 10,
            "percent": 50.0,
            "sequence": updated_at_unix_ms as u64,
            "updated_at_unix_ms": updated_at_unix_ms,
            "cancel_requested": false,
            "pid": 4242,
            "message": "PRIVATE WORK MESSAGE",
            "result": {"body": "PRIVATE WORK RESULT"},
            "error": failed.then(|| json!({
                "code": "graph_failed",
                "message": "PRIVATE WORK ERROR"
            }))
        })
    }

    fn hook_store(temp: &tempfile::TempDir) -> (StorePath, PathBuf) {
        let runtime = temp.path().join(".lwc");
        fs::create_dir(&runtime).unwrap();
        let database = runtime.join("wiki.db");
        fs::write(&database, b"placeholder").unwrap();
        (
            StorePath::new(Scope::Project, database),
            runtime.join("work"),
        )
    }

    fn write_hook_state(root: &Path, id: &str, value: &Value) -> PathBuf {
        let directory = root.join(id);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("state.json");
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    #[test]
    fn hook_summary_is_bounded_redacted_deterministic_and_read_only() {
        let temp = tempdir().unwrap();
        let (store, root) = hook_store(&temp);
        fs::create_dir(&root).unwrap();
        let mut snapshots = Vec::new();
        for index in 0..5_u64 {
            let id = format!("{index:064x}");
            let path = write_hook_state(
                &root,
                &id,
                &hook_state(&id, 100 + index as u128, index == 4),
            );
            snapshots.push((path.clone(), fs::read(path).unwrap()));
        }
        let malformed_id = "a".repeat(64);
        write_hook_state(&root, &malformed_id, &json!({"not": "a work state"}));
        let oversized_id = "b".repeat(64);
        let oversized = root.join(&oversized_id);
        fs::create_dir(&oversized).unwrap();
        fs::write(
            oversized.join("state.json"),
            vec![b'x'; WORK_HOOK_MAX_STATE_BYTES as usize + 1],
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let linked_id = "c".repeat(64);
            let linked = root.join(linked_id);
            fs::create_dir(&linked).unwrap();
            let outside = temp.path().join("outside-state.json");
            fs::write(
                &outside,
                serde_json::to_vec(&hook_state(&"c".repeat(64), 999, true)).unwrap(),
            )
            .unwrap();
            symlink(outside, linked.join("state.json")).unwrap();
        }

        let first = hook_summary(&store).unwrap();
        let second = hook_summary(&store).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.works.len(), 3);
        assert_eq!(first.omitted, 2);
        assert!(first.has_more);
        assert_eq!(first.works[0].id, format!("{:064x}", 4));
        assert_eq!(first.works[0].error_code.as_deref(), Some("graph_failed"));
        assert_eq!(first.works[1].id, format!("{:064x}", 3));
        assert_eq!(first.works[2].id, format!("{:064x}", 2));

        let encoded = serde_json::to_value(&first).unwrap();
        assert_eq!(
            encoded
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["has_more".into(), "omitted".into(), "works".into()])
        );
        assert_eq!(
            encoded["works"][0]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "completed".into(),
                "error_code".into(),
                "id".into(),
                "kind".into(),
                "phase".into(),
                "sequence".into(),
                "state".into(),
                "total".into(),
            ])
        );
        let encoded = serde_json::to_string(&first).unwrap();
        for forbidden in [
            "database",
            "/private/hook-secret",
            "message",
            "PRIVATE WORK",
            "result",
            "pid",
            "scope",
            "updated_at_unix_ms",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "leaked {forbidden}: {encoded}"
            );
        }
        for (path, before) in snapshots {
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn hook_summary_missing_root_is_empty_and_does_not_create_it() {
        let temp = tempdir().unwrap();
        let (store, root) = hook_store(&temp);

        let summary = hook_summary(&store).unwrap();

        assert_eq!(summary.works, Vec::new());
        assert_eq!(summary.omitted, 0);
        assert!(!summary.has_more);
        assert!(!root.exists());
    }

    #[test]
    fn hook_summary_stops_at_the_fixed_directory_scan_cap() {
        let temp = tempdir().unwrap();
        let (store, root) = hook_store(&temp);
        fs::create_dir(&root).unwrap();
        for index in 0..=WORK_HOOK_MAX_SCAN_ITEMS {
            fs::write(root.join(format!("junk-{index}")), b"junk").unwrap();
        }

        assert_eq!(hook_summary(&store).unwrap_err().code, "work_hook_limit");
    }

    #[cfg(unix)]
    #[test]
    fn hook_summary_rejects_a_symlinked_work_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let (store, root) = hook_store(&temp);
        let outside = temp.path().join("outside-work");
        fs::create_dir(&outside).unwrap();
        symlink(outside, root).unwrap();

        assert_eq!(hook_summary(&store).unwrap_err().code, "work_invalid");
    }

    #[test]
    fn schema_preflight_does_not_scan_a_416_mib_wal() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("wiki.db");
        let mut header = [0_u8; 100];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        header[60..64].copy_from_slice(&10_u32.to_be_bytes());
        fs::write(&database, header).unwrap();
        fs::File::create(temp.path().join("wiki.db-wal"))
            .unwrap()
            .set_len(416 * 1024 * 1024)
            .unwrap();

        let started = Instant::now();
        assert!(schema_migration_needed(&database).unwrap());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "schema preflight must read only the SQLite header"
        );
    }

    #[test]
    fn transient_state_replace_conflicts_are_retried() {
        let mut attempts = 0;
        replace_with_retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn removing_an_already_released_active_file_is_idempotent() {
        let temp = tempdir().unwrap();
        remove_file_if_present(&temp.path().join("active")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn work_commands_reject_a_symlinked_work_root() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let store_dir = temp.path().join(".lwc");
        let outside = temp.path().join("outside");
        fs::create_dir(&store_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        let database = store_dir.join("wiki.db");
        fs::write(&database, b"placeholder").unwrap();
        symlink(&outside, store_dir.join("work")).unwrap();

        let store = StorePath::new(Scope::Project, database);
        let error = list(&store).unwrap_err();
        assert_eq!(error.code, "work_invalid");
    }
}
