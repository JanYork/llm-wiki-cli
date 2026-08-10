#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

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
