#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_store() -> Store {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (store, _) = Store::initialize("project", &database).unwrap();
        std::mem::forget(temp);
        store
    }

    #[test]
    fn duplicate_source_reuses_first_metadata_and_logs_each_attempt() {
        let mut store = test_store();

        let first = store
            .source_add(SourceAddInput {
                title: Some("First".to_string()),
                origin: "/tmp/first.md".to_string(),
                tracked_path: None,
                content: "same bytes".to_string(),
            })
            .unwrap();
        let second = store
            .source_add(SourceAddInput {
                title: Some("Second".to_string()),
                origin: "/tmp/second.md".to_string(),
                tracked_path: None,
                content: "same bytes".to_string(),
            })
            .unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.source.id, second.source.id);
        assert_eq!(second.source.title.as_deref(), Some("First"));
        assert_eq!(second.source.origin, "/tmp/first.md");

        let source_add_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'source_add'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_add_count, 2);
    }

    #[test]
    fn tag_mutations_are_validated_idempotent_and_independent_of_page_put() {
        let mut store = test_store();
        let page = PagePutInput {
            slug: "core-rule".to_string(),
            title: "Core rule".to_string(),
            kind: None,
            summary: None,
            body: "first".to_string(),
            source_ids: Vec::new(),
            provenance: vec!["user-provided".to_string()],
        };
        store.page_put(page.clone()).unwrap();

        let created = store
            .tag_set("  Rules  ", "core-rule", 20, "required context")
            .unwrap();
        assert_eq!(created["action"], "created");
        assert_eq!(created["tag"], "Rules");
        let unchanged = store
            .tag_set("Rules", "core-rule", 20, "required context")
            .unwrap();
        assert_eq!(unchanged["action"], "unchanged");

        let mut replacement = page;
        replacement.body = "replacement".to_string();
        store.page_put(replacement).unwrap();
        assert_eq!(store.tag_membership_count("Rules", "core-rule").unwrap(), 1);

        let enabled = store
            .tag_autoload("Rules", true, 100, 3, 4096, "session rules")
            .unwrap();
        assert_eq!(enabled["action"], "updated");
        assert_eq!(enabled["policy"]["autoload"], true);
        assert_eq!(
            store
                .tag_autoload("Rules", true, 100, 0, 4096, "session rules")
                .unwrap_err()
                .code,
            "invalid_tag_policy"
        );
        assert_eq!(
            store
                .tag_set("\u{0000}", "core-rule", 0, "bad")
                .unwrap_err()
                .code,
            "invalid_tag"
        );

        assert_eq!(
            store.tag_remove("Rules", "core-rule").unwrap()["action"],
            "removed"
        );
        assert_eq!(
            store.tag_remove("Rules", "core-rule").unwrap()["action"],
            "unchanged"
        );
        assert_eq!(store.tag_delete("Rules").unwrap()["action"], "deleted");
        assert_eq!(store.tag_delete("Rules").unwrap()["action"], "unchanged");
    }

    #[test]
    fn streamed_source_add_rolls_back_on_a_late_input_error() {
        let mut store = test_store();
        let tables = ["sources", "source_path_revisions", "operations"];
        let before = tables.map(|table| {
            store
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        });
        let inputs = std::iter::once(Ok(Some(SourceAddInput {
            title: Some("Safe".to_string()),
            origin: "docs/safe.md".to_string(),
            tracked_path: Some("docs/safe.md".to_string()),
            content: "safe evidence".to_string(),
        })))
        .chain(std::iter::once(Err(AppError::new(
            "possible_secret_detected",
            "late validation failure",
        ))));

        let error = store.source_add_stream(inputs).unwrap_err();

        assert_eq!(error.code, "possible_secret_detected");
        for (table, expected) in tables.into_iter().zip(before) {
            let count: i64 = store
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, expected, "{table} must roll back with the batch");
        }
    }

    #[test]
    fn streamed_source_add_does_not_lock_the_database_while_consuming_inputs() {
        let mut store = test_store();
        let probe = Connection::open(&store.database).unwrap();
        probe.busy_timeout(Duration::from_millis(25)).unwrap();
        let inputs = std::iter::once_with(move || {
            probe
                .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
                .map_err(|error| AppError::new("writer_probe_failed", error.to_string()))?;
            Ok(None)
        });

        let responses = store.source_add_stream(inputs).unwrap();

        assert!(responses.is_empty());
    }

    #[test]
    fn source_path_revisions_preserve_a_b_a_observations_with_content_deduplication() {
        let mut store = test_store();
        let path = "docs/source.md";

        let first = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "A".to_string(),
            })
            .unwrap();
        let second = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "B".to_string(),
            })
            .unwrap();
        let third = store
            .source_add(SourceAddInput {
                title: Some("Source".to_string()),
                origin: path.to_string(),
                tracked_path: Some(path.to_string()),
                content: "A".to_string(),
            })
            .unwrap();

        assert_eq!(first.source.id, third.source.id);
        assert_ne!(first.source.id, second.source.id);

        let revisions = store
            .conn
            .prepare(
                "SELECT revision, source_id
                 FROM source_path_revisions
                 WHERE tracked_path = ?1
                 ORDER BY revision",
            )
            .unwrap()
            .query_map(params![path], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();

        assert_eq!(
            revisions,
            vec![
                (1, first.source.id),
                (2, second.source.id),
                (3, first.source.id),
            ]
        );
    }

    #[test]
    fn title_token_coverage_bridges_query_separators() {
        let query = "系统设置 支付渠道管理";
        let explanation = lexical_explanation(
            "page",
            Some("01-系统设置-支付渠道管理.md"),
            "unrelated-slug",
            query,
            &tokenize_for_query(query),
            0.0,
            false,
        );

        assert_eq!(explanation.signals.title_match, 0.9);
        assert_eq!(explanation.contributions.title, -TITLE_WEIGHT * 0.9);
    }

    #[test]
    fn changeset_search_refresh_targets_only_changed_documents() {
        let mut live = test_store();
        for (slug, body) in [("alpha", "old alpha"), ("beta", "unchanged beta")] {
            live.page_put(PagePutInput {
                slug: slug.to_string(),
                title: slug.to_string(),
                kind: None,
                summary: None,
                body: body.to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        }

        let temp = tempdir().unwrap();
        let candidate_path = temp.path().join("candidate.db");
        live.snapshot_to(&candidate_path).unwrap();
        let mut candidate = Store::open("project", &candidate_path).unwrap();
        candidate
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "alpha".to_string(),
                kind: None,
                summary: None,
                body: "new alpha".to_string(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        drop(candidate);

        live.conn
            .execute(
                "ATTACH DATABASE ?1 AS candidate",
                params![candidate_path.to_string_lossy().as_ref()],
            )
            .unwrap();
        let tx = live
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let (sources, pages) = changed_search_documents(&tx, "candidate").unwrap();

        assert!(sources.is_empty());
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].0, "alpha");
        assert!(pages[0].1.is_some());
    }

    #[test]
    fn concurrent_source_adds_serialize_revisions_for_one_path() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        drop(Store::initialize("project", &database).unwrap().0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        std::thread::scope(|scope| {
            for content in ["A", "B"] {
                let database = database.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let mut store = Store::open("project", database).unwrap();
                    barrier.wait();
                    store
                        .source_add(SourceAddInput {
                            title: Some("Concurrent source".to_string()),
                            origin: "docs/concurrent.md".to_string(),
                            tracked_path: Some("docs/concurrent.md".to_string()),
                            content: content.to_string(),
                        })
                        .unwrap();
                });
            }
        });

        let store = Store::open("project", database).unwrap();
        let revisions = store
            .conn
            .prepare(
                "SELECT revision, source_id
                 FROM source_path_revisions
                 WHERE tracked_path = 'docs/concurrent.md'
                 ORDER BY revision",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].0, 1);
        assert_eq!(revisions[1].0, 2);
        assert_ne!(revisions[0].1, revisions[1].1);
    }

    #[test]
    fn page_put_deduplicates_repeated_links_and_source_ids() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
                tracked_path: None,
                content: "page evidence".to_string(),
            })
            .unwrap();

        let page = store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Alpha".to_string(),
                kind: Some("concept".to_string()),
                summary: Some("Alpha summary".to_string()),
                body: "See [[beta]] and [[beta]] and [[gamma]].".to_string(),
                source_ids: vec![source.source.id, source.source.id],
                provenance: vec![
                    "hypothesis".to_string(),
                    "agent-observed".to_string(),
                    "hypothesis".to_string(),
                ],
            })
            .unwrap();

        assert_eq!(page.page.source_ids, vec![source.source.id]);
        assert_eq!(
            page.page.provenance,
            vec![
                "source-grounded".to_string(),
                "agent-observed".to_string(),
                "hypothesis".to_string(),
            ]
        );
        assert_eq!(
            page.page.links,
            vec!["beta".to_string(), "gamma".to_string()]
        );

        let relation_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM page_sources WHERE page_slug = 'alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let link_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM links WHERE from_slug = 'alpha'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(relation_count, 1);
        assert_eq!(link_count, 2);
    }

    #[test]
    fn identical_page_put_is_a_true_noop() {
        let mut store = test_store();
        let input = PagePutInput {
            slug: "stable-page".to_string(),
            title: "Stable page".to_string(),
            kind: Some("concept".to_string()),
            summary: Some("Stable summary".to_string()),
            body: "stable alpha beta evidence.".to_string(),
            source_ids: Vec::new(),
            provenance: vec!["agent-observed".to_string()],
        };
        store.page_put(input.clone()).unwrap();
        let operations_before: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let response = store.page_put(input).unwrap();
        let operations_after: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!response.created);
        assert_eq!(operations_after, operations_before);
    }

    #[test]
    fn failed_page_update_leaves_page_relations_fts_and_log_unchanged() {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Evidence".to_string()),
                origin: "/tmp/evidence.md".to_string(),
                tracked_path: None,
                content: "page evidence".to_string(),
            })
            .unwrap();

        store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Alpha".to_string(),
                kind: None,
                summary: Some("summary".to_string()),
                body: "oldterm with [[beta]]".to_string(),
                source_ids: vec![source.source.id],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();

        let page_put_count_before: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let error = store
            .page_put(PagePutInput {
                slug: "alpha".to_string(),
                title: "Replacement".to_string(),
                kind: None,
                summary: Some("replacement".to_string()),
                body: "newterm with [[gamma]]".to_string(),
                source_ids: vec![9_999],
                provenance: vec!["hypothesis".to_string()],
            })
            .unwrap_err();
        assert_eq!(error.code, "source_not_found");

        let page = store.page_show("alpha").unwrap().page;
        assert_eq!(page.title, "Alpha");
        assert_eq!(page.links, vec!["beta".to_string()]);
        assert_eq!(page.source_ids, vec![source.source.id]);
        assert_eq!(
            page.provenance,
            vec!["source-grounded".to_string(), "agent-observed".to_string()]
        );

        let old_search = store.search("oldterm", 10).unwrap();
        assert_eq!(old_search.results.len(), 1);
        assert_eq!(old_search.results[0].identifier, "alpha");

        let new_search = store.search("newterm", 10).unwrap();
        assert!(new_search.results.is_empty());

        let page_put_count_after: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operations WHERE action = 'page_put'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(page_put_count_after, page_put_count_before);
    }

    #[test]
    fn readonly_open_sees_fresh_wal_commits_from_live_writer() {
        let store = test_store();
        let database = store.database.clone();

        store
            .conn
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO meta(key, value) VALUES ('purpose', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params!["fresh-from-wal"],
            )
            .unwrap();

        let reader = Store::open_read_only("project", &database).unwrap();
        assert_eq!(
            reader.purpose_show().unwrap().purpose,
            Some("fresh-from-wal".to_string())
        );
    }

    fn hook_sidecar(database: &Path, suffix: &str) -> PathBuf {
        let mut path = database.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }

    fn hook_tree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let mut entries = fs::read_dir(current)
                .unwrap()
                .map(|entry| entry.unwrap())
                .collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.file_type().is_symlink() {
                    let mut marker = b"symlink\0".to_vec();
                    marker.extend_from_slice(
                        fs::read_link(&path).unwrap().to_string_lossy().as_bytes(),
                    );
                    snapshot.insert(relative, marker);
                } else if metadata.is_dir() {
                    snapshot.insert(relative, b"directory".to_vec());
                    walk(root, &path, snapshot);
                } else {
                    snapshot.insert(relative, fs::read(path).unwrap());
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        walk(root, root, &mut snapshot);
        snapshot
    }

    fn create_blocked_hook_plan(store: &mut Store) {
        store
            .conn
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        let created = store
            .plan_create(PlanCreateInput {
                title: "Hook plan".to_string(),
                objective: "Prove the Hook snapshot".to_string(),
                done_when: "The snapshot is exact".to_string(),
                tags: Vec::new(),
                constraints: Vec::new(),
                steps: vec![PlanStepInput {
                    title: "Wait for a human".to_string(),
                    verify: None,
                }],
                request_id: None,
            })
            .unwrap();
        let id = created["plan"]["id"].as_str().unwrap();
        let step = created["plan"]["steps"][0]["id"].as_str().unwrap();
        store
            .plan_block(id, 1, step, "waiting for explicit input")
            .unwrap();
    }

    fn copy_database_with_live_wal(target: &Path, copy_shm: bool) -> (tempfile::TempDir, Store) {
        let source = tempdir().unwrap();
        let database = source.path().join("source/wiki.db");
        let (mut writer, _) = Store::initialize("project", &database).unwrap();
        create_blocked_hook_plan(&mut writer);
        let wal = hook_sidecar(&database, "-wal");
        assert!(fs::metadata(&wal).unwrap().len() > 32);

        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(&database, target).unwrap();
        fs::copy(wal, hook_sidecar(target, "-wal")).unwrap();
        if copy_shm {
            fs::copy(
                hook_sidecar(&database, "-shm"),
                hook_sidecar(target, "-shm"),
            )
            .unwrap();
        }
        (source, writer)
    }

    fn assert_hook_store_unavailable(error: AppError, private_path: &Path) {
        assert_eq!(error.code, "store_hook_unavailable");
        assert_eq!(error.message, "Hook store snapshot is unavailable");
        assert!(!error.message.contains(&private_path.to_string_lossy()[..]));
        assert!(error.details.is_none());
    }

    #[test]
    fn windows_hook_verbatim_drive_parser_accepts_only_local_drives() {
        assert_eq!(
            strip_windows_hook_verbatim_drive(r"\\?\C:\project space\中文\.lwc\wiki.db"),
            Some(r"C:\project space\中文\.lwc\wiki.db")
        );
        assert_eq!(
            strip_windows_hook_verbatim_drive(r"\\?\z:/project/.lwc/wiki.db"),
            Some(r"z:/project/.lwc/wiki.db")
        );
        for rejected in [
            r"\\?\UNC\server\share\wiki.db",
            r"\\server\share\wiki.db",
            r"\\?\Volume{1234}\wiki.db",
            r"C:\project\.lwc\wiki.db",
            r"\\?\C:",
        ] {
            assert_eq!(
                strip_windows_hook_verbatim_drive(rejected),
                None,
                "accepted non-local-verbatim path {rejected}"
            );
        }
    }

    #[test]
    fn hook_open_without_sidecars_reads_checkpointed_main_without_creating_files() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(if cfg!(windows) {
            "project space 中文/.lwc/wiki.db"
        } else {
            "project ?#% 中文/.lwc/wiki.db"
        });
        let (store, _) = Store::initialize("project", &database).unwrap();
        drop(store);
        let checkpoint = Connection::open(&database).unwrap();
        checkpoint
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(checkpoint);
        for suffix in ["-wal", "-shm"] {
            let sidecar = hook_sidecar(&database, suffix);
            if sidecar.exists() {
                fs::remove_file(sidecar).unwrap();
            }
        }

        let before = hook_tree_snapshot(temp.path());
        let hook = Store::open_for_hook_with_timeout("project", &database, Duration::ZERO).unwrap();
        hook.begin_hook_snapshot_with_timeout(Duration::ZERO)
            .unwrap();
        assert_eq!(hook.active_plan_count().unwrap(), 0);
        assert_eq!(hook.plan_tracking().unwrap(), None);
        drop(hook);

        assert_eq!(hook_tree_snapshot(temp.path()), before);
    }

    #[test]
    fn hook_open_with_empty_wal_reads_main_without_creating_an_index() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("project/.lwc/wiki.db");
        let (store, _) = Store::initialize("project", &database).unwrap();
        drop(store);
        let checkpoint = Connection::open(&database).unwrap();
        checkpoint
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(checkpoint);
        for suffix in ["-wal", "-shm"] {
            let sidecar = hook_sidecar(&database, suffix);
            if sidecar.exists() {
                fs::remove_file(sidecar).unwrap();
            }
        }
        fs::write(hook_sidecar(&database, "-wal"), []).unwrap();

        let before = hook_tree_snapshot(temp.path());
        let hook = Store::open_for_hook_with_timeout("project", &database, Duration::ZERO).unwrap();
        hook.begin_hook_snapshot_with_timeout(Duration::ZERO)
            .unwrap();
        assert_eq!(hook.active_plan_count().unwrap(), 0);
        drop(hook);

        assert_eq!(hook_tree_snapshot(temp.path()), before);
    }

    #[test]
    fn hook_open_rejects_a_corrupt_store_quickly_without_changing_it() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("PRIVATE_PROJECT/.lwc/wiki.db");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        fs::write(&database, b"PRIVATE CORRUPT DATABASE").unwrap();
        let before = hook_tree_snapshot(temp.path());

        let started = Instant::now();
        let error =
            Store::open_for_hook_with_timeout("project", &database, Duration::ZERO).unwrap_err();

        assert!(
            started.elapsed() < Duration::from_millis(100),
            "corrupt Hook store exceeded its zero-wait boundary"
        );
        assert_hook_store_unavailable(error, temp.path());
        assert_eq!(hook_tree_snapshot(temp.path()), before);
    }

    #[test]
    fn hook_open_reads_blocked_plan_from_live_wal_without_changing_sidecars() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("project/.lwc/wiki.db");
        let (_source, writer) = copy_database_with_live_wal(&database, true);
        assert!(fs::metadata(hook_sidecar(&database, "-wal")).unwrap().len() > 32);
        assert!(fs::metadata(hook_sidecar(&database, "-shm")).unwrap().len() > 0);

        let before = hook_tree_snapshot(temp.path());
        let hook =
            Store::open_for_hook_with_timeout("project", &database, Duration::from_millis(250))
                .unwrap();
        hook.begin_hook_snapshot_with_timeout(Duration::from_millis(250))
            .unwrap();
        assert_eq!(hook.active_plan_count().unwrap(), 1);
        let tracking = hook.plan_tracking().unwrap().unwrap();
        assert_eq!(tracking["current_step"]["status"], "blocked");
        drop(hook);

        assert_eq!(hook_tree_snapshot(temp.path()), before);
        drop(writer);
    }

    #[test]
    fn hook_open_fails_closed_on_nonempty_wal_without_a_valid_index() {
        for invalid_index in ["missing", "short", "corrupt"] {
            let temp = tempdir().unwrap();
            let database = temp.path().join("PRIVATE_PROJECT/.lwc/wiki.db");
            let (_source, _writer) =
                copy_database_with_live_wal(&database, invalid_index == "corrupt");
            match invalid_index {
                "missing" => {}
                "short" => {
                    fs::write(hook_sidecar(&database, "-shm"), b"not a wal index").unwrap();
                }
                "corrupt" => {
                    let shm = hook_sidecar(&database, "-shm");
                    let mut bytes = fs::read(&shm).unwrap();
                    bytes[0] ^= 1;
                    fs::write(shm, bytes).unwrap();
                }
                _ => unreachable!(),
            }
            let before = hook_tree_snapshot(temp.path());

            let error =
                Store::open_for_hook_with_timeout("project", &database, Duration::from_millis(250))
                    .unwrap_err();

            assert_hook_store_unavailable(error, temp.path());
            assert_eq!(hook_tree_snapshot(temp.path()), before);
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_open_rejects_symlinked_wal_or_shm_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        for suffix in ["-wal", "-shm"] {
            let temp = tempdir().unwrap();
            let database = temp.path().join("PRIVATE_PROJECT/.lwc/wiki.db");
            let (_source, _writer) = copy_database_with_live_wal(&database, true);
            let attacked = hook_sidecar(&database, suffix);
            fs::remove_file(&attacked).unwrap();
            let outside = temp.path().join(format!("outside{suffix}"));
            fs::write(&outside, b"PRIVATE OUTSIDE BYTES").unwrap();
            symlink(&outside, &attacked).unwrap();
            let before = hook_tree_snapshot(temp.path());
            let outside_before = fs::read(&outside).unwrap();

            let error =
                Store::open_for_hook_with_timeout("project", &database, Duration::from_millis(250))
                    .unwrap_err();

            assert_hook_store_unavailable(error, temp.path());
            assert_eq!(hook_tree_snapshot(temp.path()), before);
            assert_eq!(fs::read(outside).unwrap(), outside_before);
        }
    }

    #[test]
    #[ignore = "invoked in a separate process by the writer-held regression"]
    fn hook_open_writer_held_child() {
        let database = PathBuf::from(std::env::var_os("LWC_HOOK_WRITER_DATABASE").unwrap());
        let hook =
            Store::open_for_hook_with_timeout("project", &database, Duration::from_millis(250))
                .unwrap();
        hook.begin_hook_snapshot_with_timeout(Duration::from_millis(250))
            .unwrap();
        assert_eq!(hook.active_plan_count().unwrap(), 1);
        let tracking = hook.plan_tracking().unwrap().unwrap();
        assert_eq!(tracking["current_step"]["status"], "blocked");
    }

    #[test]
    fn hook_open_reads_writer_held_uncheckpointed_wal_without_changing_tree() {
        use std::process::Command;

        let temp = tempdir().unwrap();
        let database = temp.path().join("project space 中文/.lwc/wiki.db");
        let (mut writer, _) = Store::initialize("project", &database).unwrap();
        writer
            .conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        let main_before = fs::read(&database).unwrap();
        create_blocked_hook_plan(&mut writer);
        assert_eq!(fs::read(&database).unwrap(), main_before);
        assert!(fs::metadata(hook_sidecar(&database, "-wal")).unwrap().len() > 32);
        writer
            .conn
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
            .unwrap();
        let before = hook_tree_snapshot(temp.path());
        writer.conn.execute_batch("BEGIN IMMEDIATE;").unwrap();

        #[cfg(windows)]
        let hook_database = {
            let canonical = database.canonicalize().unwrap();
            assert!(canonical.to_string_lossy().starts_with(r"\\?\"));
            canonical
        };
        #[cfg(not(windows))]
        let hook_database = database.clone();

        let started = Instant::now();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "store::tests::hook_open_writer_held_child",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("LWC_HOOK_WRITER_DATABASE", hook_database)
            .output()
            .unwrap();
        assert!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "writer-held Hook child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "writer-held Hook accumulated a busy wait: {:?}",
            started.elapsed()
        );
        writer.conn.execute_batch("ROLLBACK;").unwrap();
        assert_eq!(hook_tree_snapshot(temp.path()), before);
    }

    #[test]
    fn concurrent_open_migrates_v1_once_and_preserves_searchable_data() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (mut store, _) = Store::initialize("project", &database).unwrap();
        store
            .page_put(PagePutInput {
                slug: "attention".to_string(),
                title: "注意力机制".to_string(),
                kind: Some("concept".to_string()),
                summary: None,
                body: "注意力机制帮助模型聚焦关键信号。".to_string(),
                source_ids: Vec::new(),
                provenance: Vec::new(),
            })
            .unwrap();
        drop(store);

        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "DROP TABLE memory_fts;
             DROP TABLE IF EXISTS memory_fts_data;
             DROP TABLE IF EXISTS memory_fts_idx;
             DROP TABLE IF EXISTS memory_fts_content;
             DROP TABLE IF EXISTS memory_fts_docsize;
             DROP TABLE IF EXISTS memory_fts_config;
             DROP TABLE memory_feedback;
             DROP TABLE memory_relations;
             DROP TABLE memory_evidence;
             DROP TABLE memory_changes;
             DROP TABLE memory_fragments;
             DROP TABLE memory_hint_state;
             DROP TABLE memory_state;
             DROP TABLE memory_events;
             DROP TABLE page_tags;
             DROP TABLE tags;
             DROP TABLE search_fts;
             DROP TABLE retrieval_feedback;
             DROP TABLE retrieval_weights;
             DROP TABLE source_path_revisions;
             DROP TABLE page_provenance;
             DROP TABLE ingest_jobs;
             ALTER TABLE sources DROP COLUMN structural_navigation;
             ALTER TABLE pages DROP COLUMN structural_navigation;
             CREATE VIRTUAL TABLE source_fts USING fts5(
                 source_id UNINDEXED, title, content
             );
             CREATE VIRTUAL TABLE page_fts USING fts5(
                 slug UNINDEXED, title, summary, body
             );
             UPDATE meta SET value = '1' WHERE key = 'format_version';
             DELETE FROM meta WHERE key = 'tokenizer';
             PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
        drop(conn);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..4 {
                let barrier = barrier.clone();
                let database = database.clone();
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    Store::open("project", database).unwrap()
                }));
            }
            for handle in handles {
                drop(handle.join().unwrap());
            }
        });

        let migrated = Store::open("project", &database).unwrap();
        let results = migrated.search("注意力", 10).unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identifier, "attention");
        let version: i32 = migrated
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let journal_mode: String = migrated
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(version, USER_VERSION);
        assert_eq!(journal_mode, "wal");
    }

    #[test]
    fn agent_state_migration_rechecks_version_after_competing_upgrade() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("wiki.db");
        let mut seed = Connection::open(&database).unwrap();
        configure_connection(&seed).unwrap();
        bootstrap_schema(&mut seed).unwrap();
        seed.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'format_version'",
            [TEMPORAL_MEMORY_VERSION.to_string()],
        )
        .unwrap();
        seed.pragma_update(None, "user_version", TEMPORAL_MEMORY_VERSION)
            .unwrap();
        drop(seed);

        let mut winner = Connection::open(&database).unwrap();
        configure_connection(&winner).unwrap();
        let mut waiter = Connection::open(&database).unwrap();
        configure_connection(&waiter).unwrap();
        let winning_tx = winner
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();

        let (started_tx, waiter_started) = std::sync::mpsc::channel();
        let waiting_migration = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            migrate_agent_state_v15(&mut waiter)
        });
        waiter_started.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));

        winning_tx
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'format_version'",
                [USER_VERSION.to_string()],
            )
            .unwrap();
        winning_tx
            .pragma_update(None, "user_version", USER_VERSION)
            .unwrap();
        winning_tx.commit().unwrap();

        waiting_migration.join().unwrap().unwrap();
        let version: i32 = winner
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, USER_VERSION);
    }

    #[test]
    fn stale_ingest_migration_step_accepts_a_newer_intermediate_version() {
        let mut store = test_store();
        store
            .conn
            .execute_batch(
                "DROP TABLE page_provenance;
                 UPDATE meta SET value = '6' WHERE key = 'format_version';
                 PRAGMA user_version = 6;",
            )
            .unwrap();

        migrate_ingest_workflow(&mut store.conn).unwrap();

        let version: i32 = store
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, COMPOUND_WIKI_VERSION);
    }

    #[test]
    fn lint_reports_missing_orphaned_and_duplicate_search_rows() {
        let mut store = test_store();
        for slug in ["missing", "duplicate"] {
            store
                .page_put(PagePutInput {
                    slug: slug.to_string(),
                    title: slug.to_string(),
                    kind: None,
                    summary: None,
                    body: format!("{slug} body"),
                    source_ids: Vec::new(),
                    provenance: Vec::new(),
                })
                .unwrap();
        }
        store
            .conn
            .execute(
                "DELETE FROM search_fts
                 WHERE doc_type = 'page' AND identifier = 'missing'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO search_fts(
                    doc_type, identifier, title_terms, summary_terms, body_terms
                 ) VALUES ('page', 'orphan', 'orphan', '', 'orphan')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO search_fts(
                    doc_type, identifier, title_terms, summary_terms, body_terms
                 )
                 SELECT doc_type, identifier, title_terms, summary_terms, body_terms
                 FROM search_fts
                 WHERE doc_type = 'page' AND identifier = 'duplicate'",
                [],
            )
            .unwrap();

        let codes = store
            .lint(100, 0)
            .unwrap()
            .issues
            .into_iter()
            .map(|issue| issue.code)
            .collect::<BTreeSet<_>>();
        assert!(codes.contains("search_index_missing"));
        assert!(codes.contains("search_index_orphan"));
        assert!(codes.contains("search_index_duplicate"));
    }

    #[test]
    fn markdown_links_ignore_code_and_include_relative_markdown_targets() {
        let body = r#"Real [[real-target]] and [relative](../docs/other-page.md#section).

`inline [[inline-fake]]`

```sh
rg '^[[:space:]]*[[fenced-fake]]'
```

    indented [[indented-fake]]
"#;

        assert_eq!(
            extract_links(body),
            vec!["other-page".to_string(), "real-target".to_string()]
        );
    }

    #[test]
    fn sync_export_uses_stable_semantic_objects_and_excludes_local_derived_state() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (mut store, _) = Store::initialize("project", &database).unwrap();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Guide".to_string()),
                origin: temp.path().join("private/guide.md").display().to_string(),
                tracked_path: None,
                content: "stable source bytes".to_string(),
            })
            .unwrap();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Guide".to_string(),
                kind: Some("concept".to_string()),
                summary: Some("sync fixture".to_string()),
                body: "Source-backed body".to_string(),
                source_ids: vec![source.source.id],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        store
            .conn
            .execute_batch(
                "INSERT INTO memory_events(
                 id,fingerprint,event_type,context,occurred_at,logical_bytes
             ) VALUES(
                 'mem-1',printf('%064d',1),'decision','sync memory',
                 '2026-08-23T00:00:00.000Z',11
             );
             INSERT INTO memory_fragments(event_id,kind,ordinal,value)
             VALUES('mem-1','decision',0,'keep semantic state');
             INSERT INTO todo_items(
                 id,fingerprint,title,state,revision,created_at,updated_at,target_at
             ) VALUES(
                 '11111111111111111111111111111111',printf('%064d',2),'Ship sync',
                 'open',1,'2026-08-23T00:00:00.000Z','2026-08-23T00:00:00.000Z',
                 '2026-08-24T00:00:00.000Z'
             );
             INSERT INTO plans(
                 id,fingerprint,title,objective,done_when,state,revision,created_at,updated_at
             ) VALUES(
                 '22222222222222222222222222222222',printf('%064d',3),'Sync plan',
                 'merge safely','tests pass','active',1,
                 '2026-08-23T00:00:00.000Z','2026-08-23T00:00:00.000Z'
             );
             INSERT INTO plan_steps(
                 plan_id,step_id,ordinal,title,status,created_revision,updated_revision,
                 created_at,updated_at
             ) VALUES(
                 '22222222222222222222222222222222',
                 '33333333333333333333333333333333',0,'Verify','in_progress',1,1,
                 '2026-08-23T00:00:00.000Z','2026-08-23T00:00:00.000Z'
             );",
            )
            .unwrap();

        let normalized = temp.path().join("normalized.db");
        let summary = store.export_sync_state(&normalized).unwrap();
        assert!(summary.object_count >= 5);
        assert_eq!(summary.blob_count, 1);

        let exported = Connection::open(&normalized).unwrap();
        let tables = exported
            .prepare("SELECT name FROM sqlite_schema WHERE type IN ('table','view') ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, vec!["sync_blobs", "sync_manifest", "sync_objects"]);
        let source_payload: String = exported
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!source_payload.contains(temp.path().to_str().unwrap()));
        let page_payload: Value = serde_json::from_str(
            &exported
                .query_row(
                    "SELECT payload_json FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            page_payload["source_hashes"],
            json!([source.source.content_hash])
        );
        for kind in ["memory", "todo", "plan"] {
            let count: i64 = exported
                .query_row(
                    "SELECT COUNT(*) FROM sync_objects WHERE kind=?1",
                    [kind],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing normalized {kind}");
        }
    }

    #[test]
    fn sync_transfer_selects_session_delta_and_round_trips_it() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Old".to_string(),
                kind: None,
                summary: None,
                body: "old".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let baseline = temp.path().join("baseline.db");
        store.export_sync_state(&baseline).unwrap();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "New".to_string(),
                kind: None,
                summary: None,
                body: "new".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let current = temp.path().join("current.db");
        store.export_sync_state(&current).unwrap();

        let transfer = select_sync_transfer(Some(&baseline), &current).unwrap();
        assert_eq!(transfer.kind, SyncTransferKind::Delta);
        assert!(!transfer.bytes.is_empty());
        let reconstructed = temp.path().join("reconstructed.db");
        apply_sync_transfer(Some(&baseline), &transfer, &reconstructed).unwrap();
        assert_eq!(
            sync_state_digest(&reconstructed).unwrap(),
            transfer.state_digest
        );

        let full = select_sync_transfer(None, &current).unwrap();
        assert_eq!(full.kind, SyncTransferKind::Full);
        assert_eq!(full.state_digest, transfer.state_digest);
    }

    #[test]
    fn sync_transfer_streams_production_delta_to_files() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Old".to_string(),
                kind: None,
                summary: None,
                body: "old".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let baseline = temp.path().join("baseline.db");
        store.export_sync_state(&baseline).unwrap();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "New".to_string(),
                kind: None,
                summary: None,
                body: "new".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let current = temp.path().join("current.db");
        store.export_sync_state(&current).unwrap();
        let artifact = temp.path().join("transfer.bin");

        let summary = prepare_sync_transfer(Some(&baseline), &current, &artifact).unwrap();

        assert_eq!(summary.kind, SyncTransferKind::Delta);
        assert_eq!(summary.size, fs::metadata(&artifact).unwrap().len());
        let baseline_digest = sync_state_digest(&baseline).unwrap();
        assert_eq!(
            summary.baseline_digest.as_deref(),
            Some(baseline_digest.as_str())
        );
        assert!(summary.size < fs::metadata(&current).unwrap().len());
        let reconstructed = temp.path().join("reconstructed.db");
        apply_sync_transfer_artifact(Some(&baseline), &artifact, &summary, &reconstructed).unwrap();
        assert_eq!(
            sync_state_digest(&reconstructed).unwrap(),
            summary.state_digest
        );
    }

    #[test]
    fn sync_export_uses_one_online_backup_snapshot_while_wal_writer_advances() {
        let temp = tempdir().unwrap();
        let database = temp.path().join(".lwc/wiki.db");
        let (mut store, _) = Store::initialize("project", &database).unwrap();
        store
            .page_put(PagePutInput {
                slug: "snapshot".to_string(),
                title: "0".to_string(),
                kind: None,
                summary: None,
                body: "snapshot".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        store.schema_set("0").unwrap();
        for index in 0..48 {
            store
                .source_add(SourceAddInput {
                    title: Some(format!("filler {index}")),
                    origin: format!("filler-{index}.md"),
                    tracked_path: None,
                    content: format!("{index}:{}", "x".repeat(128 * 1024)),
                })
                .unwrap();
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = std::sync::Arc::new(std::sync::Barrier::new(2));
        let writer_stop = stop.clone();
        let writer_started = started.clone();
        let writer_database = database.clone();
        let writer = std::thread::spawn(move || {
            let conn = Connection::open(writer_database).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            writer_started.wait();
            let mut revision = 1_u64;
            while revision <= 3_000 && !writer_stop.load(std::sync::atomic::Ordering::Relaxed) {
                conn.execute_batch("BEGIN IMMEDIATE").unwrap();
                conn.execute(
                    "UPDATE meta SET value=?1 WHERE key='schema'",
                    [revision.to_string()],
                )
                .unwrap();
                conn.execute(
                    "UPDATE pages SET title=?1 WHERE slug='snapshot'",
                    [revision.to_string()],
                )
                .unwrap();
                conn.execute_batch("COMMIT").unwrap();
                revision += 1;
            }
        });
        started.wait();
        let normalized = temp.path().join("normalized.db");
        store.export_sync_state(&normalized).unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();

        let conn = Connection::open(normalized).unwrap();
        let schema: Value = serde_json::from_str(&conn.query_row(
            "SELECT payload_json FROM sync_objects WHERE kind='meta' AND logical_key='schema'",
            [], |row| row.get::<_, String>(0),
        ).unwrap()).unwrap();
        let page: Value = serde_json::from_str(&conn.query_row(
            "SELECT payload_json FROM sync_objects WHERE kind='page' AND logical_key='snapshot'",
            [], |row| row.get::<_, String>(0),
        ).unwrap()).unwrap();
        assert_eq!(schema["value"], page["title"]);
    }

    fn mutate_sync_page(path: &Path, title: Option<&str>, body: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        let encoded: String = conn
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut payload: Value = serde_json::from_str(&encoded).unwrap();
        if let Some(title) = title {
            payload["title"] = json!(title);
        }
        if let Some(body) = body {
            payload["body"] = json!(body);
        }
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "UPDATE sync_objects SET payload_json=?1,payload_hash=?2
             WHERE kind='page' AND logical_key='guide'",
            params![encoded, hash_content(&encoded)],
        )
        .unwrap();
    }

    fn put_normalized_object(path: &Path, kind: &str, key: &str, payload: Value) {
        let conn = Connection::open(path).unwrap();
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO sync_objects(kind,logical_key,payload_json,payload_hash)
             VALUES(?1,?2,?3,?4)",
            params![kind, key, encoded, hash_content(&encoded)],
        )
        .unwrap();
    }

    fn normalized_object(path: &Path, kind: &str, key: &str) -> Option<Value> {
        Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind=?1 AND logical_key=?2",
                params![kind, key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap()
            .map(|encoded| serde_json::from_str(&encoded).unwrap())
    }

    #[test]
    fn sync_directional_baselines_preserve_retained_memory_and_unilateral_edits() {
        let temp = tempdir().unwrap();
        let empty = temp.path().join("empty.db");
        create_empty_sync_state(&empty).unwrap();
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        fs::copy(&empty, &local).unwrap();
        fs::copy(&empty, &remote).unwrap();
        put_normalized_object(&local, "page", "local-only", json!({"body": "local"}));
        put_normalized_object(
            &remote,
            "memory",
            "remote-memory",
            json!({"content": "remote"}),
        );

        let first = temp.path().join("first.db");
        merge_sync_states_directional(&empty, &local, &empty, &remote, &first).unwrap();
        assert!(normalized_object(&first, "page", "local-only").is_some());
        assert!(normalized_object(&first, "memory", "remote-memory").is_some());

        // The local acknowledgement includes the merged memory, while the remote
        // acknowledgement remains its own state. Local retention later evicts the
        // memory; that local absence must not become a semantic remote deletion.
        let baseline_local = temp.path().join("baseline-local.db");
        let baseline_remote = temp.path().join("baseline-remote.db");
        let retained_local = temp.path().join("retained-local.db");
        fs::copy(&first, &baseline_local).unwrap();
        fs::copy(&remote, &baseline_remote).unwrap();
        fs::copy(&first, &retained_local).unwrap();
        Connection::open(&retained_local)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='memory' AND logical_key='remote-memory'",
                [],
            )
            .unwrap();
        let after_retention = temp.path().join("after-retention.db");
        merge_sync_states_directional(
            &baseline_local,
            &retained_local,
            &baseline_remote,
            &remote,
            &after_retention,
        )
        .unwrap();
        assert!(normalized_object(&after_retention, "memory", "remote-memory").is_some());

        let retained_remote = temp.path().join("retained-remote.db");
        fs::copy(&first, &retained_remote).unwrap();
        Connection::open(&retained_remote)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='memory' AND logical_key='remote-memory'",
                [],
            )
            .unwrap();
        let after_remote_retention = temp.path().join("after-remote-retention.db");
        merge_sync_states_directional(
            &first,
            &first,
            &first,
            &retained_remote,
            &after_remote_retention,
        )
        .unwrap();
        assert!(normalized_object(&after_remote_retention, "memory", "remote-memory").is_some());

        let edited_local = temp.path().join("edited-local.db");
        let edited_remote = temp.path().join("edited-remote.db");
        fs::copy(&baseline_local, &edited_local).unwrap();
        fs::copy(&baseline_remote, &edited_remote).unwrap();
        put_normalized_object(
            &edited_local,
            "page",
            "local-only",
            json!({"body": "local-v2"}),
        );
        put_normalized_object(
            &edited_remote,
            "memory",
            "remote-memory",
            json!({"content": "remote-v2"}),
        );
        let edited = temp.path().join("edited.db");
        merge_sync_states_directional(
            &baseline_local,
            &edited_local,
            &baseline_remote,
            &edited_remote,
            &edited,
        )
        .unwrap();
        assert_eq!(
            normalized_object(&edited, "page", "local-only").unwrap()["body"],
            "local-v2"
        );
        assert_eq!(
            normalized_object(&edited, "memory", "remote-memory").unwrap()["content"],
            "remote-v2"
        );
    }

    #[test]
    fn sync_directional_merge_applies_field_delta_over_divergent_baseline() {
        let temp = tempdir().unwrap();
        let base_local = temp.path().join("base-local.db");
        let current_local = temp.path().join("current-local.db");
        let base_remote = temp.path().join("base-remote.db");
        let current_remote = temp.path().join("current-remote.db");
        create_empty_sync_state(&base_local).unwrap();
        create_empty_sync_state(&base_remote).unwrap();
        put_normalized_object(
            &base_local,
            "page",
            "guide",
            json!({"title": "Local", "body": "local"}),
        );
        put_normalized_object(
            &base_remote,
            "page",
            "guide",
            json!({"title": "Base", "body": "base"}),
        );
        fs::copy(&base_local, &current_local).unwrap();
        fs::copy(&base_remote, &current_remote).unwrap();
        put_normalized_object(
            &current_remote,
            "page",
            "guide",
            json!({"title": "Base", "body": "remote-new"}),
        );

        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states_directional(
            &base_local,
            &current_local,
            &base_remote,
            &current_remote,
            &merged,
        )
        .unwrap();

        assert_eq!(summary.conflict_count, 0);
        assert_eq!(
            normalized_object(&merged, "page", "guide").unwrap(),
            json!({"title": "Local", "body": "remote-new"})
        );
    }

    #[test]
    fn sync_directional_merge_conflicts_on_independent_same_key_creation() {
        let temp = tempdir().unwrap();
        let empty = temp.path().join("empty.db");
        let base_local = temp.path().join("base-local.db");
        let current_local = temp.path().join("current-local.db");
        let current_remote = temp.path().join("current-remote.db");
        create_empty_sync_state(&empty).unwrap();
        fs::copy(&empty, &base_local).unwrap();
        put_normalized_object(&base_local, "page", "guide", json!({"body": "local"}));
        fs::copy(&base_local, &current_local).unwrap();
        fs::copy(&empty, &current_remote).unwrap();
        put_normalized_object(&current_remote, "page", "guide", json!({"body": "remote"}));

        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states_directional(
            &base_local,
            &current_local,
            &empty,
            &current_remote,
            &merged,
        )
        .unwrap();

        assert_eq!(summary.conflict_count, 1);
        assert_eq!(summary.conflicts[0]["kind"], "page");
        assert_eq!(summary.conflicts[0]["logical_key"], "guide");
    }

    #[test]
    fn sync_three_way_merge_combines_independent_fields_and_converges() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Base title".to_string(),
                kind: None,
                summary: None,
                body: "Base body".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        mutate_sync_page(&local, Some("Local title"), None);
        mutate_sync_page(&remote, None, Some("Remote body"));

        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        assert_eq!(summary.conflict_count, 0);
        let conn = Connection::open(&merged).unwrap();
        let payload: Value = serde_json::from_str(
            &conn.query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["title"], "Local title");
        assert_eq!(payload["body"], "Remote body");

        let swapped = temp.path().join("swapped.db");
        merge_sync_states(&base, &remote, &local, &swapped).unwrap();
        assert_eq!(
            sync_state_digest(&merged).unwrap(),
            sync_state_digest(&swapped).unwrap()
        );
    }

    #[test]
    fn sync_three_way_merge_emits_semantic_conflict_packet_not_sql_rows() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Base".to_string(),
                kind: None,
                summary: None,
                body: "Body".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        mutate_sync_page(&local, Some("Local"), None);
        mutate_sync_page(&remote, Some("Remote"), None);

        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        assert_eq!(summary.conflict_count, 1);
        assert_eq!(summary.conflict_kinds, vec!["page"]);
        assert_eq!(summary.conflicts[0]["kind"], "page");
        assert_eq!(summary.conflicts[0]["logical_key"], "guide");
        assert_eq!(summary.conflicts[0]["fields"][0]["path"], "title");
        let packet = serde_json::to_string(&summary.conflicts).unwrap();
        assert!(packet.contains("Local"));
        assert!(packet.contains("Remote"));
        assert!(!packet.contains("payload_json"));
        assert!(!packet.contains("sync_objects"));

        let swapped = temp.path().join("swapped.db");
        let reverse = merge_sync_states(&base, &remote, &local, &swapped).unwrap();
        assert_eq!(summary.conflicts, reverse.conflicts);
        assert_eq!(
            sync_state_digest(&merged).unwrap(),
            sync_state_digest(&swapped).unwrap()
        );
    }

    #[test]
    fn sync_conflict_batches_are_bounded_to_twenty() {
        let conflicts = (0..25)
            .map(|index| {
                json!({
                    "conflict_id": format!("conflict-{index}"),
                    "kind": "page",
                    "logical_key": format!("page-{index}"),
                    "fields": []
                })
            })
            .collect::<Vec<_>>();

        let batch = next_sync_conflict_batch(&conflicts);

        assert_eq!(batch.len(), 20);
        assert_eq!(batch[0]["conflict_id"], "conflict-0");
        assert_eq!(batch[19]["conflict_id"], "conflict-19");
    }

    #[test]
    fn sync_resolution_rejects_stale_conflict_id_without_mutation() {
        let temp = tempdir().unwrap();
        let merged = temp.path().join("merged.db");
        create_empty_sync_state(&merged).unwrap();
        put_normalized_object(
            &merged,
            "page",
            "guide",
            json!({"slug": "guide", "title": "before"}),
        );
        let conflicts = vec![json!({
            "conflict_id": "a".repeat(64),
            "kind": "page",
            "logical_key": "guide",
            "fields": [{"path": "title", "candidates": ["after-a", "after-b"]}]
        })];
        let before = sync_state_digest(&merged).unwrap();

        let error = resolve_sync_conflicts(
            &merged,
            &conflicts,
            &json!({
                "version": 1,
                "decisions": [{
                    "conflict_id": "b".repeat(64),
                    "kind": "page",
                    "logical_key": "guide",
                    "path": "title",
                    "candidate": 0
                }]
            }),
        )
        .unwrap_err();

        assert_eq!(error.code, "sync_resolution_stale");
        assert_eq!(sync_state_digest(&merged).unwrap(), before);
    }

    #[test]
    fn sync_resolution_rejects_unknown_or_mixed_schema_without_mutation() {
        let conflict = json!({
            "conflict_id": "a".repeat(64),
            "kind": "page",
            "logical_key": "guide",
            "fields": [{"path": "title", "candidates": ["after-a", "after-b"]}]
        });
        let decision = json!({
            "conflict_id": "a".repeat(64),
            "kind": "page",
            "logical_key": "guide",
            "path": "title",
            "candidate": 0
        });
        let mut extra_decision = decision.clone();
        extra_decision["extra"] = json!(true);
        let mut unknown_strategy = decision.clone();
        unknown_strategy["strategy"] = json!("overwrite");
        let mut mixed = decision.clone();
        mixed["strategy"] = json!("preserve_both");
        let oversized = vec![decision.clone(); 21];
        let cases = vec![
            json!({"version":1,"decisions":[decision.clone()],"extra":true}),
            json!({"version":1,"decisions":[extra_decision]}),
            json!({"version":1,"decisions":[unknown_strategy]}),
            json!({"version":1,"decisions":[mixed]}),
            json!({"version":1,"decisions":oversized}),
        ];

        for (index, resolution) in cases.into_iter().enumerate() {
            let temp = tempdir().unwrap();
            let merged = temp.path().join(format!("merged-{index}.db"));
            create_empty_sync_state(&merged).unwrap();
            put_normalized_object(
                &merged,
                "page",
                "guide",
                json!({"slug":"guide","title":"before"}),
            );
            let before = sync_state_digest(&merged).unwrap();
            let error =
                resolve_sync_conflicts(&merged, std::slice::from_ref(&conflict), &resolution)
                    .unwrap_err();
            assert_eq!(error.code, "sync_resolution_invalid", "case {index}");
            assert_eq!(sync_state_digest(&merged).unwrap(), before, "case {index}");
        }
    }

    fn populated_sync_source() -> Store {
        let mut store = test_store();
        let source = store
            .source_add(SourceAddInput {
                title: Some("Remote source".to_string()),
                origin: "docs/remote.md".to_string(),
                tracked_path: None,
                content: "portable evidence".to_string(),
            })
            .unwrap()
            .source;
        store
            .page_put(PagePutInput {
                slug: "sync-guide".to_string(),
                title: "Sync guide".to_string(),
                kind: Some("concept".to_string()),
                summary: Some("Portable state".to_string()),
                body: "See [[sync-guide]].".to_string(),
                source_ids: vec![source.id],
                provenance: Vec::new(),
            })
            .unwrap();
        store
            .tag_set("sync", "sync-guide", 7, "portable tag")
            .unwrap();
        store
            .retrieval_weight_set(
                "page",
                "sync-guide",
                2,
                "portable preference",
                "agent-observed",
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO semantic_relations(
                    id,relation_type,from_identifier,to_identifier,confidence,provenance,
                    reason,source_ids_json,created_at,updated_at
                 ) VALUES('edge:sync','RELATED_TO','sync-guide','sync-guide',0.8,
                          'agent-observed','portable relation',?1,
                          '2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z')",
                [serde_json::to_string(&vec![source.id]).unwrap()],
            )
            .unwrap();
        store
            .todo_add(TodoCreateInput {
                title: "Sync child state".to_string(),
                tags: vec!["sync".to_string()],
                cue: Some("when publishing".to_string()),
                detail: Some("preserve todo semantics".to_string()),
                parent_id: None,
                target_at: Some("2026-08-24T08:00:00+08:00".to_string()),
                request_id: None,
            })
            .unwrap();
        store
            .plan_create(PlanCreateInput {
                title: "Sync plan".to_string(),
                objective: "Publish normalized state".to_string(),
                done_when: "Imported store validates".to_string(),
                tags: vec!["sync".to_string()],
                constraints: vec!["atomic".to_string()],
                steps: vec![PlanStepInput {
                    title: "Publish".to_string(),
                    verify: Some("integrity_check".to_string()),
                }],
                request_id: None,
            })
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO memory_events(
                    id,request_id,fingerprint,event_type,context,occurred_at,recorded_at,
                    valid_from,valid_until,pinned,logical_bytes
                 ) VALUES('11111111111111111111111111111111',NULL,?1,'decision','sync publish',
                          '2026-01-01T00:00:00.000Z','2026-01-01T00:00:00.000Z',NULL,NULL,1,12)",
                ["1".repeat(64)],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO memory_fragments(event_id,kind,ordinal,value)
                 VALUES('11111111111111111111111111111111','decision',0,'publish atomically')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE memory_state SET event_count=1,logical_bytes=12 WHERE id=1",
                [],
            )
            .unwrap();
        store
    }

    #[test]
    fn sync_publish_imports_complete_semantic_state_and_rebuilds_indexes() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();

        let mut target = test_store();
        let expected = target.identity().unwrap();
        let summary = target
            .publish_sync_state(&normalized, &expected, "session-success")
            .unwrap();

        assert_ne!(summary.revision, expected.revision);
        assert!(summary.checkpoint.is_file());
        for table in [
            "sources",
            "pages",
            "tags",
            "ingest_jobs",
            "retrieval_weights",
            "semantic_relations",
            "memory_events",
            "todo_items",
            "plans",
        ] {
            let count: i64 = target
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert!(count > 0, "{table} was not imported");
        }
        assert_eq!(
            target.page_show("sync-guide").unwrap().page.title,
            "Sync guide"
        );
        assert!(
            target
                .search("portable", 10)
                .unwrap()
                .results
                .iter()
                .any(|row| { row.identifier == "sync-guide" })
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM operations WHERE action='sync_merge'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        target.validate_changeset_integrity().unwrap();
    }

    #[test]
    fn sync_publish_reports_only_changed_page_and_source_identifiers() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let expected_source: String = Connection::open(&normalized)
            .unwrap()
            .query_row(
                "SELECT logical_key FROM sync_objects WHERE kind='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();

        let first = target
            .publish_sync_state(&normalized, &expected, "affected-first")
            .unwrap();
        assert_eq!(first.affected_pages, vec!["sync-guide"]);
        assert_eq!(first.affected_sources, vec![expected_source]);
        target
            .materialize_sync_selection(&serde_json::to_value(&first).unwrap())
            .unwrap();

        let expected = target.identity().unwrap();
        let second = target
            .publish_sync_state(&normalized, &expected, "affected-second")
            .unwrap();
        assert!(second.affected_pages.is_empty());
        assert!(second.affected_sources.is_empty());
    }

    #[test]
    fn sync_publish_preserves_unaffected_indexes_and_removes_only_deleted_documents() {
        let temp = tempdir().unwrap();
        let mut source = test_store();
        let alpha = source
            .source_add(SourceAddInput {
                title: Some("Alpha source".to_owned()),
                origin: "alpha.md".to_owned(),
                tracked_path: None,
                content: "alpha searchable evidence".to_owned(),
            })
            .unwrap()
            .source;
        let beta = source
            .source_add(SourceAddInput {
                title: Some("Beta source".to_owned()),
                origin: "beta.md".to_owned(),
                tracked_path: None,
                content: "beta searchable evidence".to_owned(),
            })
            .unwrap()
            .source;
        for (slug, title, source_id) in [
            ("alpha", "Alpha page", alpha.id),
            ("beta", "Beta page", beta.id),
        ] {
            source
                .page_put(PagePutInput {
                    slug: slug.to_owned(),
                    title: title.to_owned(),
                    kind: Some("concept".to_owned()),
                    summary: None,
                    body: format!("{slug} body with a sentence."),
                    source_ids: vec![source_id],
                    provenance: Vec::new(),
                })
                .unwrap();
        }
        let full = temp.path().join("full.db");
        source.export_sync_state(&full).unwrap();
        let hashes = Connection::open(&full)
            .unwrap()
            .prepare("SELECT logical_key FROM sync_objects WHERE kind='source' ORDER BY logical_key")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let alpha_hash = hash_content("alpha searchable evidence");
        let beta_hash = hash_content("beta searchable evidence");
        let mut expected_hashes = vec![alpha_hash.clone(), beta_hash.clone()];
        expected_hashes.sort();
        assert_eq!(hashes, expected_hashes);

        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&full, &expected, "selective-full")
            .unwrap();
        let alpha_local_id: i64 = target
            .conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash=?1",
                [&alpha_hash],
                |row| row.get(0),
            )
            .unwrap();
        let beta_local_id: i64 = target
            .conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash=?1",
                [&beta_hash],
                |row| row.get(0),
            )
            .unwrap();
        let beta_page_fts_rowid: i64 = target
            .conn
            .query_row(
                "SELECT rowid FROM search_fts WHERE doc_type='page' AND identifier='beta'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let beta_source_fts_rowid: i64 = target
            .conn
            .query_row(
                "SELECT rowid FROM search_fts WHERE doc_type='source' AND identifier=?1",
                [beta_local_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let beta_spans = {
            let mut statement = target
                .conn
                .prepare(
                    "SELECT span_id,content_fingerprint FROM search_spans
                     WHERE document_type='page' AND document_identifier='beta' AND active=1
                     ORDER BY span_id",
                )
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        let deleted = temp.path().join("deleted.db");
        std::fs::copy(&full, &deleted).unwrap();
        let normalized = Connection::open(&deleted).unwrap();
        normalized
            .execute(
                "DELETE FROM sync_objects WHERE (kind='page' AND logical_key='alpha')
                    OR (kind IN ('source','ingest') AND logical_key=?1)",
                [&alpha_hash],
            )
            .unwrap();
        normalized
            .execute("DELETE FROM sync_blobs WHERE content_hash=?1", [&alpha_hash])
            .unwrap();
        drop(normalized);

        let expected = target.identity().unwrap();
        let summary = target
            .publish_sync_state(&deleted, &expected, "selective-delete")
            .unwrap();
        assert_eq!(summary.affected_pages, vec!["alpha"]);
        assert_eq!(summary.affected_sources, vec![alpha_hash]);
        assert!(summary
            .affected_graph_documents
            .contains(&("page".to_owned(), "alpha".to_owned())));
        assert!(summary
            .affected_graph_documents
            .contains(&("source".to_owned(), alpha_local_id.to_string())));
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT rowid FROM search_fts WHERE doc_type='page' AND identifier='beta'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            beta_page_fts_rowid
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT rowid FROM search_fts WHERE doc_type='source' AND identifier=?1",
                    [beta_local_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            beta_source_fts_rowid
        );
        let after_beta_spans = {
            let mut statement = target
                .conn
                .prepare(
                    "SELECT span_id,content_fingerprint FROM search_spans
                     WHERE document_type='page' AND document_identifier='beta' AND active=1
                     ORDER BY span_id",
                )
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(after_beta_spans, beta_spans);
        for (kind, identifier) in [
            ("page", "alpha".to_owned()),
            ("source", alpha_local_id.to_string()),
        ] {
            assert_eq!(
                target
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM search_fts WHERE doc_type=?1 AND identifier=?2",
                        params![kind, identifier],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
            assert_eq!(
                target
                    .conn
                    .query_row(
                        "SELECT COUNT(*) FROM search_spans
                         WHERE document_type=?1 AND document_identifier=?2 AND active=1",
                        params![kind, identifier],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn sync_publish_removes_deleted_source_with_local_path_binding() {
        let temp = tempdir().unwrap();
        let mut source = test_store();
        let removed = source
            .source_add(SourceAddInput {
                title: Some("Removed source".to_owned()),
                origin: "docs/removed.md".to_owned(),
                tracked_path: Some("docs/removed.md".to_owned()),
                content: "removed evidence".to_owned(),
            })
            .unwrap()
            .source;
        let full = temp.path().join("full.db");
        source.export_sync_state(&full).unwrap();

        let mut target = test_store();
        let local = target
            .source_add(SourceAddInput {
                title: Some("Local binding".to_owned()),
                origin: "/Users/local/docs/removed.md".to_owned(),
                tracked_path: Some("docs/removed.md".to_owned()),
                content: "removed evidence".to_owned(),
            })
            .unwrap()
            .source;
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&full, &expected, "source-delete-full")
            .unwrap();

        source.source_remove(removed.id).unwrap();
        let deleted = temp.path().join("deleted.db");
        source.export_sync_state(&deleted).unwrap();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&deleted, &expected, "source-delete-empty")
            .unwrap();

        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sources WHERE id=?1",
                    [local.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM source_path_revisions WHERE source_id=?1",
                    [local.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn sync_publish_selects_relation_source_document_with_target_local_id() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let first = temp.path().join("relation-first.db");
        source.export_sync_state(&first).unwrap();
        let hash: String = Connection::open(&first)
            .unwrap()
            .query_row(
                "SELECT logical_key FROM sync_objects WHERE kind='source'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut relation = normalized_object(&first, "semantic_relation", "edge:sync").unwrap();
        relation["from"] = json!(format!("source-hash:{hash}"));
        put_normalized_object(&first, "semantic_relation", "edge:sync", relation.clone());

        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&first, &expected, "relation-first")
            .unwrap();
        let local_source_id: i64 = target
            .conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash=?1",
                [&hash],
                |row| row.get(0),
            )
            .unwrap();

        let second = temp.path().join("relation-second.db");
        std::fs::copy(&first, &second).unwrap();
        relation["reason"] = json!("changed relation only");
        put_normalized_object(&second, "semantic_relation", "edge:sync", relation);
        let expected = target.identity().unwrap();
        let summary = target
            .publish_sync_state(&second, &expected, "relation-second")
            .unwrap();

        assert!(summary.affected_pages.is_empty());
        assert!(summary.affected_sources.is_empty());
        assert_eq!(summary.affected_relations, vec!["edge:sync"]);
        assert_eq!(
            summary.affected_graph_documents,
            vec![("source".to_owned(), local_source_id.to_string())]
        );
    }

    #[test]
    fn sync_publish_persists_recoverable_receipt_when_postcommit_indexing_fails() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let mut page = normalized_object(&normalized, "page", "sync-guide").unwrap();
        page["body"] = json!("x\n\n".repeat(crate::segment::MAX_SPANS_PER_DOCUMENT / 2 + 1));
        put_normalized_object(&normalized, "page", "sync-guide", page);
        let state_digest = sync_state_digest(&normalized).unwrap();
        let mut target = test_store();
        let database = target.database.clone();
        let starting = target.identity().unwrap();

        let summary = target
            .publish_sync_state(&normalized, &starting, "postcommit-failure")
            .unwrap();

        assert!(summary.committed);
        assert_eq!(summary.derived["status"], "failed");
        assert_eq!(target.page_show("sync-guide").unwrap().page.title, "Sync guide");
        let ending = target.identity().unwrap();
        assert_ne!(ending.revision, starting.revision);
        let receipt = sync_publication_receipt(
            &database,
            "postcommit-failure",
            &state_digest,
        )
        .unwrap()
        .unwrap();
        assert_eq!(receipt["starting_identity"]["revision"], starting.revision);
        assert_eq!(receipt["ending_identity"]["revision"], ending.revision);
        assert_eq!(receipt["ending_identity"]["operation_id"], ending.operation_id);
        assert_eq!(receipt["derived"]["status"], "failed");
        assert_eq!(receipt["affected"]["page"], json!(["sync-guide"]));
        assert!(receipt["affected_graph_documents"]
            .as_array()
            .is_some_and(|documents| !documents.is_empty()));
        assert!(receipt["checkpoint"]
            .as_str()
            .is_some_and(|value| value.starts_with("sync-") && value.ends_with(".db")));
        assert!(
            sync_publication_receipt(&database, "postcommit-failure", &"0".repeat(64))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sync_publish_bounds_large_derived_selection_and_persists_full_fallback() {
        let temp = tempdir().unwrap();
        let normalized = temp.path().join("many-pages.db");
        create_empty_sync_state(&normalized).unwrap();
        let conn = Connection::open(&normalized).unwrap();
        for index in 0..300 {
            let slug = format!("page-{index:03}-{}", "x".repeat(1_024));
            insert_sync_object(
                &conn,
                "page",
                &slug,
                &json!({
                    "slug": slug,
                    "title": format!("Page {index}"),
                    "kind": "concept",
                    "summary": null,
                    "body": "bounded selection",
                    "structural_navigation": false,
                    "source_hashes": [],
                    "provenance": ["agent-observed"],
                    "created_at": "2026-01-01T00:00:00.000Z",
                    "updated_at": "2026-01-01T00:00:00.000Z"
                }),
            )
            .unwrap();
        }
        drop(conn);
        let digest = sync_state_digest(&normalized).unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();

        let summary = target
            .publish_sync_state(&normalized, &expected, "bounded-selection")
            .unwrap();

        assert!(summary.committed);
        assert_eq!(summary.derived_selection, "full");
        assert!(summary.affected_pages.is_empty());
        assert!(summary.affected_graph_documents.is_empty());
        assert_eq!(summary.affected_counts["page"], 300);
        let receipt = sync_publication_receipt(&target.database, "bounded-selection", &digest)
            .unwrap()
            .unwrap();
        assert_eq!(receipt["derived_selection"], "full");
        assert_eq!(receipt["affected_counts"]["page"], 300);
        assert!(receipt["affected"].is_null());
        let encoded = serde_json::to_vec(&receipt).unwrap();
        assert!(encoded.len() <= 256 * 1024);
    }

    #[test]
    fn sync_continuity_objects_do_not_expand_derived_selection() {
        let previous = BTreeMap::new();
        let mut current = BTreeMap::new();
        for index in 0..4_096 {
            current.insert(
                ("work_audit".to_owned(), format!("audit-{index:04}")),
                json!({"ordinal": index}),
            );
        }
        current.insert(
            ("draft_intent".to_owned(), "draft".to_owned()),
            json!({"version": 1}),
        );

        let affected = derived_sync_affected(&previous, &current);
        let selection = bounded_sync_selection(&affected, &[]).unwrap();

        assert_eq!(selection.mode, "exact");
        assert_eq!(selection.counts["work_audit"], 0);
        assert_eq!(selection.counts["draft_intent"], 0);
        assert_eq!(selection.affected, Some(json!({
            "draft_intent": [],
            "ingest": [],
            "memory": [],
            "meta": [],
            "page": [],
            "plan": [],
            "retrieval_feedback": [],
            "retrieval_weight": [],
            "semantic_relation": [],
            "source": [],
            "tag": [],
            "todo": [],
            "work_audit": []
        })));
    }

    #[test]
    fn sync_publish_ignores_detached_intent_and_records_terminal_work_audit_idempotently() {
        let temp = tempdir().unwrap();
        let normalized = temp.path().join("continuity.db");
        create_empty_sync_state(&normalized).unwrap();
        let origin = "a".repeat(64);
        let changeset_id = "draft-one";
        put_normalized_object(
            &normalized,
            "draft_intent",
            &format!("{origin}\0{changeset_id}"),
            json!({
                "origin_store_id": origin,
                "intent": {
                    "version": 1,
                    "origin_changeset_id": changeset_id,
                    "name": "portable draft",
                    "actions": [],
                    "sources": [],
                    "pages": [],
                    "tags": [],
                    "meta": []
                }
            }),
        );
        let audit_origin = "d".repeat(64);
        let audit_work_id = "work-one";
        let audit_key = hash_content(&format!("{audit_origin}\0{audit_work_id}"));
        let audit_digest = hash_content(
            &json!({
                "kind": "maintenance-reindex",
                "state": "succeeded",
                "completed": 1,
                "total": 1,
                "updated_at_unix_ms": 1234,
                "result_digest": "e".repeat(64),
                "error_code": null
            })
            .to_string(),
        );
        let audit = json!({
            "audit_key": audit_key,
            "digest": audit_digest,
            "origin_store_id": audit_origin,
            "origin_work_id": audit_work_id,
            "kind": "maintenance-reindex",
            "state": "succeeded",
            "completed": 1,
            "total": 1,
            "updated_at_unix_ms": 1234,
            "result_digest": "e".repeat(64)
        });
        put_normalized_object(&normalized, "work_audit", &audit_key, audit.clone());
        let mut target = test_store();

        for session in ["continuity-first", "continuity-retry"] {
            let expected = target.identity().unwrap();
            let summary = target
                .publish_sync_state(&normalized, &expected, session)
                .unwrap();
            assert_eq!(summary.derived_selection, "exact");
            assert_eq!(summary.affected_counts["work_audit"], 0);
            assert_eq!(summary.affected_counts["draft_intent"], 0);
        }
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM operations
                     WHERE action='sync_work_audit' AND target=?1",
                    [&audit_key],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM operations WHERE action LIKE 'changeset_%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        let conflicting = temp.path().join("continuity-conflict.db");
        fs::copy(&normalized, &conflicting).unwrap();
        let mut conflicting_audit = audit;
        conflicting_audit["completed"] = json!(2);
        conflicting_audit["total"] = json!(2);
        conflicting_audit["digest"] = json!(hash_content(
            &json!({
                "kind": "maintenance-reindex",
                "state": "succeeded",
                "completed": 2,
                "total": 2,
                "updated_at_unix_ms": 1234,
                "result_digest": "e".repeat(64),
                "error_code": null
            })
            .to_string()
        ));
        put_normalized_object(
            &conflicting,
            "work_audit",
            &audit_key,
            conflicting_audit,
        );
        let before = target.identity().unwrap();
        let error = target
            .publish_sync_state(&conflicting, &before, "continuity-conflict")
            .unwrap_err();
        assert_eq!(error.code, "sync_audit_conflict");
        assert_eq!(target.identity().unwrap(), before);
    }

    #[test]
    fn sync_prepare_rejects_forged_terminal_work_audits() {
        let valid_audit = || {
            let origin = "a".repeat(64);
            let work_id = "work-safe";
            let audit_key = hash_content(&format!("{origin}\0{work_id}"));
            let digest = hash_content(
                &json!({
                    "kind":"graph-project","state":"succeeded","completed":1,"total":1,
                    "updated_at_unix_ms":42,"result_digest":null,"error_code":null
                })
                .to_string(),
            );
            (
                audit_key.clone(),
                json!({
                    "audit_key":audit_key,"digest":digest,"origin_store_id":origin,
                    "origin_work_id":work_id,"kind":"graph-project","state":"succeeded",
                    "completed":1,"total":1,"updated_at_unix_ms":42
                }),
            )
        };
        for (index, mutate) in ["digest", "key", "kind", "state", "extra"]
            .into_iter()
            .enumerate()
        {
            let temp = tempdir().unwrap();
            let normalized = temp.path().join(format!("forged-{index}.db"));
            create_empty_sync_state(&normalized).unwrap();
            let (mut key, mut payload) = valid_audit();
            match mutate {
                "digest" => payload["digest"] = json!("0".repeat(64)),
                "key" => {
                    key = "f".repeat(64);
                    payload["audit_key"] = json!(key);
                }
                "kind" => payload["kind"] = json!("shell-command"),
                "state" => payload["state"] = json!("running"),
                "extra" => payload["unexpected"] = json!(true),
                _ => unreachable!(),
            }
            put_normalized_object(&normalized, "work_audit", &key, payload);

            let error = prepare_sync_state(&normalized).err().unwrap();

            assert_eq!(error.code, "sync_state_invalid", "{mutate}");
        }
    }

    #[test]
    fn sync_prepare_bounds_continuity_object_counts_before_replay() {
        for (kind, count) in [("draft_intent", 65), ("work_audit", 4_097)] {
            let temp = tempdir().unwrap();
            let normalized = temp.path().join(format!("{kind}.db"));
            create_empty_sync_state(&normalized).unwrap();
            let conn = Connection::open(&normalized).unwrap();
            for index in 0..count {
                insert_sync_object(&conn, kind, &format!("item-{index}"), &json!({})).unwrap();
            }
            drop(conn);

            let error = prepare_sync_state(&normalized).err().unwrap();

            assert_eq!(error.code, "sync_state_invalid", "{kind}");
            assert!(error.message.contains("fixed limits"), "{kind}");
        }
    }

    #[test]
    fn sync_exact_materialize_updates_and_deletes_only_selected_artifacts() {
        let mut store = test_store();
        let root = store.database.parent().unwrap().to_path_buf();
        let add_source = |store: &mut Store, name: &str| {
            store
                .source_add(SourceAddInput {
                    title: Some(format!("{name} source")),
                    origin: format!("{name}.txt"),
                    tracked_path: None,
                    content: format!("{name} original"),
                })
                .unwrap()
                .source
        };
        let alpha_source = add_source(&mut store, "alpha");
        let beta_source = add_source(&mut store, "beta");
        let deleted_source = add_source(&mut store, "deleted");
        for slug in ["alpha", "beta", "deleted"] {
            store
                .page_put(PagePutInput {
                    slug: slug.to_owned(),
                    title: format!("{slug} page"),
                    kind: Some("concept".to_owned()),
                    summary: None,
                    body: format!("{slug} original"),
                    source_ids: Vec::new(),
                    provenance: vec!["agent-observed".to_owned()],
                })
                .unwrap();
        }
        store.materialize().unwrap();
        let alpha_page = root.join("wiki/concepts/alpha.md");
        let beta_page = root.join("wiki/concepts/beta.md");
        let deleted_page = root.join("wiki/concepts/deleted.md");
        let alpha_raw = root.join(
            artifacts::source_artifact_rel_path(&alpha_source.id.to_string(), "alpha.txt")
                .unwrap(),
        );
        let beta_raw = root.join(
            artifacts::source_artifact_rel_path(&beta_source.id.to_string(), "beta.txt").unwrap(),
        );
        let deleted_raw = root.join(
            artifacts::source_artifact_rel_path(&deleted_source.id.to_string(), "deleted.txt")
                .unwrap(),
        );
        fs::write(&beta_page, "unaffected page sentinel").unwrap();
        fs::write(&beta_raw, "unaffected source sentinel").unwrap();
        store
            .conn
            .execute("UPDATE pages SET body='alpha selected update' WHERE slug='alpha'", [])
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE sources SET content='alpha selected source update' WHERE id=?1",
                [alpha_source.id],
            )
            .unwrap();
        store
            .conn
            .execute("DELETE FROM pages WHERE slug='deleted'", [])
            .unwrap();
        store
            .conn
            .execute("DELETE FROM sources WHERE id=?1", [deleted_source.id])
            .unwrap();
        let publication = json!({
            "derived_selection":"exact",
            "affected": {"page":["alpha","deleted"],"meta":[]},
            "affected_graph_documents": [
                ["source",alpha_source.id.to_string()],
                ["source",deleted_source.id.to_string()]
            ]
        });

        let response = store.materialize_sync_selection(&publication).unwrap();

        assert!(fs::read_to_string(&alpha_page).unwrap().contains("alpha selected update"));
        assert_eq!(fs::read_to_string(&alpha_raw).unwrap(), "alpha selected source update");
        assert!(!deleted_page.exists());
        assert!(!deleted_raw.exists());
        assert_eq!(fs::read_to_string(&beta_page).unwrap(), "unaffected page sentinel");
        assert_eq!(fs::read_to_string(&beta_raw).unwrap(), "unaffected source sentinel");
        assert!(!response.files.iter().any(|path| path.contains("beta")));
    }

    #[test]
    fn sync_full_materialize_fallback_rebuilds_complete_projection() {
        let mut store = test_store();
        let root = store.database.parent().unwrap().to_path_buf();
        store
            .page_put(PagePutInput {
                slug: "full-page".to_owned(),
                title: "Full page".to_owned(),
                kind: Some("concept".to_owned()),
                summary: None,
                body: "before fallback".to_owned(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_owned()],
            })
            .unwrap();
        store.materialize().unwrap();
        store
            .conn
            .execute(
                "UPDATE pages SET body='after full fallback' WHERE slug='full-page'",
                [],
            )
            .unwrap();

        store
            .materialize_sync_selection(&json!({"derived_selection":"full"}))
            .unwrap();

        assert!(
            fs::read_to_string(root.join("wiki/concepts/full-page.md"))
                .unwrap()
                .contains("after full fallback")
        );
    }

    #[test]
    fn sync_publish_rejects_stale_revision_without_mutating_live_store() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let mut target = test_store();
        let stale = target.identity().unwrap();
        target.purpose_set("newer local write").unwrap();
        let before = target.identity().unwrap();

        let error = target
            .publish_sync_state(&normalized, &stale, "session-stale")
            .unwrap_err();

        assert_eq!(error.code, "sync_store_changed");
        assert_eq!(target.identity().unwrap(), before);
        assert!(target.page_show("sync-guide").is_err());
    }

    #[test]
    fn sync_publish_preserves_local_identity_paths_config_and_audit() {
        let temp = tempdir().unwrap();
        let mut source = populated_sync_source();
        source.schema_set("remote schema").unwrap();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();

        let mut target = test_store();
        let local = target
            .source_add(SourceAddInput {
                title: Some("Local binding".to_string()),
                origin: "/Users/local/docs/remote.md".to_string(),
                tracked_path: Some("docs/remote.md".to_string()),
                content: "portable evidence".to_string(),
            })
            .unwrap()
            .source;
        target
            .conn
            .execute(
                "INSERT INTO meta(key,value) VALUES('local_config','keep')",
                [],
            )
            .unwrap();
        let draft_base = target.identity().unwrap();
        let draft = target
            .changeset_begin("survives-sync", &draft_base)
            .unwrap();
        let store_root = target.database.parent().unwrap().to_path_buf();
        let work = store_root.join("Work/local-job");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("state.json"), "local work sentinel").unwrap();
        let config = store_root.join("config.json");
        fs::write(&config, "{\"todo\":{\"setting\":\"enabled\"}}").unwrap();
        let expected = target.identity().unwrap();
        let operation_count: i64 = target
            .conn
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .unwrap();

        target
            .publish_sync_state(&normalized, &expected, "session-preserve")
            .unwrap();
        target.materialize().unwrap();

        assert_eq!(target.identity().unwrap().store_id, expected.store_id);
        assert_eq!(
            target.schema_show().unwrap().schema.as_deref(),
            Some("remote schema")
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT value FROM meta WHERE key='local_config'",
                    [],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            "keep"
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT tracked_path FROM source_path_revisions WHERE source_id=?1",
                    [local.id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "docs/remote.md"
        );
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT origin FROM sources WHERE id=?1",
                    [local.id],
                    |row| { row.get::<_, String>(0) }
                )
                .unwrap(),
            "/Users/local/docs/remote.md"
        );
        let after_operations: i64 = target
            .conn
            .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after_operations, operation_count + 1);
        assert_eq!(
            target.changeset_draft("survives-sync", 10).unwrap().id,
            draft.id
        );
        assert_eq!(
            fs::read_to_string(work.join("state.json")).unwrap(),
            "local work sentinel"
        );
        assert_eq!(
            fs::read_to_string(config).unwrap(),
            "{\"todo\":{\"setting\":\"enabled\"}}"
        );
        assert!(store_root.join("wiki/concepts/sync-guide.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn sync_publish_existing_target_rejects_checkpoint_symlink_without_mutation() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        let exported = source.export_sync_state(&normalized).unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        let session_id = "checkpoint-symlink";
        let checkpoint = checkpoint_path(
            &target.database,
            &format!(
                "sync-{}-{}-{}",
                &hash_content(session_id)[..8],
                &expected.revision[..8],
                &exported.state_digest[..8]
            ),
        )
        .unwrap();
        fs::create_dir_all(checkpoint.parent().unwrap()).unwrap();
        let sentinel = temp.path().join("sentinel.db");
        fs::write(&sentinel, b"untouched").unwrap();
        symlink(&sentinel, &checkpoint).unwrap();

        let error = target
            .publish_sync_state(&normalized, &expected, session_id)
            .unwrap_err();

        assert_eq!(error.code, "checkpoint_path_invalid");
        assert_eq!(target.identity().unwrap(), expected);
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
    }

    #[test]
    fn sync_postcommit_detach_failure_remains_committed_success() {
        let revision = "a".repeat(64);

        let settled =
            settle_sync_publication(Ok(revision.clone()), Err(rusqlite::Error::InvalidQuery))
                .unwrap();

        assert_eq!(settled, revision);
    }

    #[test]
    fn sync_publish_rolls_back_malformed_normalized_state() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        Connection::open(&normalized)
            .unwrap()
            .execute("DELETE FROM sync_blobs", [])
            .unwrap();
        let mut target = test_store();
        target.purpose_set("must survive").unwrap();
        let expected = target.identity().unwrap();

        let error = target
            .publish_sync_state(&normalized, &expected, "session-malformed")
            .unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert_eq!(target.identity().unwrap(), expected);
        assert_eq!(
            target.purpose_show().unwrap().purpose.as_deref(),
            Some("must survive")
        );
        assert!(target.page_show("sync-guide").is_err());
    }

    #[test]
    fn sync_three_way_set_merge_preserves_concurrent_baseline_deletions() {
        let base = json!(["alpha", "beta"]);
        let mut conflicts = Vec::new();

        let merged = merge_sync_arrays(
            Some(&base),
            &[json!("beta")],
            &[json!("alpha")],
            "provenance",
            &mut conflicts,
        );

        assert_eq!(merged, json!([]));
        assert!(conflicts.is_empty());
    }

    #[test]
    fn sync_directional_keyed_array_delete_and_modify_are_not_silently_reversed() {
        let base = json!([{"id": "step-1", "title": "base"}]);
        let mut conflicts = Vec::new();
        let deleted = merge_sync_arrays_directional(
            Some(&base),
            base.as_array().unwrap(),
            Some(&base),
            &[],
            "steps",
            &mut conflicts,
        );
        assert_eq!(deleted, json!([]));
        assert!(conflicts.is_empty());

        let modified = json!([{"id": "step-1", "title": "modified"}]);
        let merged = merge_sync_arrays_directional(
            Some(&base),
            modified.as_array().unwrap(),
            Some(&base),
            &[],
            "steps",
            &mut conflicts,
        );
        assert_eq!(merged, modified);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn sync_directional_string_set_add_vs_remove_requires_resolution() {
        let base_local = json!([]);
        let base_remote = json!(["shared"]);
        let left = json!(["shared"]);
        let right = json!([]);
        let mut conflicts = Vec::new();

        merge_sync_arrays_directional(
            Some(&base_local),
            left.as_array().unwrap(),
            Some(&base_remote),
            right.as_array().unwrap(),
            "tags",
            &mut conflicts,
        );

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0]["path"], "tags");
    }

    #[test]
    fn sync_merge_drops_orphaned_source_blobs() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        let removed = store
            .source_add(SourceAddInput {
                title: Some("removed".to_string()),
                origin: "removed.md".to_string(),
                tracked_path: None,
                content: "removed body".to_string(),
            })
            .unwrap()
            .source;
        let kept = store
            .source_add(SourceAddInput {
                title: Some("kept".to_string()),
                origin: "kept.md".to_string(),
                tracked_path: None,
                content: "kept body".to_string(),
            })
            .unwrap()
            .source;
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        for path in [&local, &remote] {
            Connection::open(path)
                .unwrap()
                .execute(
                    "DELETE FROM sync_objects WHERE kind='source' AND logical_key=?1",
                    [&removed.content_hash],
                )
                .unwrap();
        }

        let merged = temp.path().join("merged.db");
        merge_sync_states_directional(&base, &local, &base, &remote, &merged).unwrap();
        let conn = Connection::open(&merged).unwrap();
        let blobs = conn
            .prepare("SELECT content_hash FROM sync_blobs ORDER BY content_hash")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(blobs, vec![kept.content_hash]);
    }

    #[test]
    fn sync_only_emits_agent_conflicts_for_preserve_both_domain_kinds() {
        for kind in [
            "meta",
            "source",
            "tag",
            "ingest",
            "retrieval_weight",
            "retrieval_feedback",
            "semantic_relation",
            "page",
            "todo",
            "plan",
            "memory",
        ] {
            let base = SyncObjectRow {
                payload: json!({"value": "base", "updated_at": "2026-01-01T00:00:00Z"}),
            };
            let local = SyncObjectRow {
                payload: json!({"value": "local", "updated_at": "2026-01-02T00:00:00Z"}),
            };
            let remote = SyncObjectRow {
                payload: json!({"value": "remote", "updated_at": "2026-01-03T00:00:00Z"}),
            };
            let mut conflicts = Vec::new();
            merge_sync_object_directional(
                kind,
                "same-key",
                Some(&base),
                Some(&local),
                Some(&base),
                Some(&remote),
                &mut conflicts,
                &mut BTreeMap::new(),
            )
            .unwrap();
            assert_eq!(
                !conflicts.is_empty(),
                matches!(kind, "page" | "todo" | "plan" | "memory"),
                "unexpected resolver coverage for {kind}"
            );
        }
    }

    #[test]
    fn sync_mechanical_kinds_preserve_orthogonal_fields_and_memberships() {
        let base = SyncObjectRow {
            payload: json!({
                "name": "shared", "reason": "base", "autoload": false,
                "pages": [], "updated_at": "2026-01-01T00:00:00Z"
            }),
        };
        let local = SyncObjectRow {
            payload: json!({
                "name": "shared", "reason": "local reason", "autoload": false,
                "pages": [{"slug": "local-page"}], "updated_at": "2026-01-02T00:00:00Z"
            }),
        };
        let remote = SyncObjectRow {
            payload: json!({
                "name": "shared", "reason": "base", "autoload": true,
                "pages": [{"slug": "remote-page"}], "updated_at": "2026-01-03T00:00:00Z"
            }),
        };
        let mut conflicts = Vec::new();
        let merged = merge_sync_object_directional(
            "tag",
            "shared",
            Some(&base),
            Some(&local),
            Some(&base),
            Some(&remote),
            &mut conflicts,
            &mut BTreeMap::new(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(merged.payload["reason"], "local reason");
        assert_eq!(merged.payload["autoload"], true);
        let pages = merged.payload["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|page| page["slug"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(pages, BTreeSet::from(["local-page", "remote-page"]));
        assert!(conflicts.is_empty());
    }

    #[test]
    fn sync_relation_source_endpoints_are_remapped_by_content_hash() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let remote_source_id: i64 = source
            .conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash=?1",
                [hash_content("portable evidence")],
                |row| row.get(0),
            )
            .unwrap();
        source.conn.execute(
            "UPDATE semantic_relations SET from_identifier=?1,to_identifier=?1 WHERE id='edge:sync'",
            [format!("source:{remote_source_id}")],
        ).unwrap();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();

        let mut target = test_store();
        target
            .source_add(SourceAddInput {
                title: Some("occupies local id".to_string()),
                origin: "local.md".to_string(),
                tracked_path: None,
                content: "different local source".to_string(),
            })
            .unwrap();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&normalized, &expected, "relation-remap")
            .unwrap();

        let imported_source_id: i64 = target
            .conn
            .query_row(
                "SELECT id FROM sources WHERE content_hash=?1",
                [hash_content("portable evidence")],
                |row| row.get(0),
            )
            .unwrap();
        let endpoints: (String, String) = target
            .conn
            .query_row(
                "SELECT from_identifier,to_identifier FROM semantic_relations WHERE id='edge:sync'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            endpoints,
            (
                format!("source:{imported_source_id}"),
                format!("source:{imported_source_id}")
            )
        );
        assert_ne!(imported_source_id, remote_source_id);
    }

    #[test]
    fn sync_import_starts_todo_and_plan_at_local_revision_one() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let normalized_conn = Connection::open(&normalized).unwrap();
        for kind in ["todo", "plan"] {
            let (key, encoded): (String, String) = normalized_conn
                .query_row(
                    "SELECT logical_key,payload_json FROM sync_objects WHERE kind=?1",
                    [kind],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            let mut payload: Value = serde_json::from_str(&encoded).unwrap();
            payload["revision"] = json!(99);
            let encoded = serde_json::to_string(&payload).unwrap();
            normalized_conn.execute(
                "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind=?3 AND logical_key=?4",
                params![encoded, hash_content(&encoded), kind, key],
            ).unwrap();
        }
        drop(normalized_conn);

        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&normalized, &expected, "local-revisions")
            .unwrap();

        let todo_revision: i64 = target
            .conn
            .query_row("SELECT revision FROM todo_items", [], |row| row.get(0))
            .unwrap();
        let plan_revision: i64 = target
            .conn
            .query_row("SELECT revision FROM plans", [], |row| row.get(0))
            .unwrap();
        assert_eq!(todo_revision, 1);
        assert_eq!(plan_revision, 1);
    }

    #[test]
    fn sync_publish_preserves_target_local_request_ids() {
        let temp = tempdir().unwrap();
        let mut target = populated_sync_source();
        let todo_id: String = target
            .conn
            .query_row("SELECT id FROM todo_items", [], |row| row.get(0))
            .unwrap();
        let plan_id: String = target
            .conn
            .query_row("SELECT id FROM plans", [], |row| row.get(0))
            .unwrap();
        let memory_id: String = target
            .conn
            .query_row("SELECT id FROM memory_events", [], |row| row.get(0))
            .unwrap();
        target
            .conn
            .execute(
                "UPDATE todo_items SET request_id='local-todo-request' WHERE id=?1",
                [&todo_id],
            )
            .unwrap();
        target
            .conn
            .execute(
                "UPDATE plans SET request_id='local-plan-request' WHERE id=?1",
                [&plan_id],
            )
            .unwrap();
        target
            .conn
            .execute(
                "UPDATE memory_events SET request_id='local-memory-request' WHERE id=?1",
                [&memory_id],
            )
            .unwrap();
        let normalized = temp.path().join("normalized.db");
        target.export_sync_state(&normalized).unwrap();
        let encoded: Vec<String> = Connection::open(&normalized)
            .unwrap()
            .prepare("SELECT payload_json FROM sync_objects WHERE kind IN ('todo','plan','memory')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            encoded
                .iter()
                .all(|payload| !payload.contains("request_id"))
        );

        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&normalized, &expected, "request-id-locality")
            .unwrap();

        for (table, id, expected_request) in [
            ("todo_items", todo_id, "local-todo-request"),
            ("plans", plan_id, "local-plan-request"),
            ("memory_events", memory_id, "local-memory-request"),
        ] {
            let request: Option<String> = target
                .conn
                .query_row(
                    &format!("SELECT request_id FROM {table} WHERE id=?1"),
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(request.as_deref(), Some(expected_request));
        }
    }

    #[test]
    fn sync_preserve_both_keeps_large_page_variants_queryable() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Guide".to_string(),
                kind: None,
                summary: None,
                body: "base".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        let local_body = format!("local:{}", "L".repeat(20 * 1024));
        let remote_body = format!("remote:{}", "R".repeat(20 * 1024));
        mutate_sync_page(&local, None, Some(&local_body));
        mutate_sync_page(&remote, None, Some(&remote_body));
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        assert_eq!(summary.conflict_count, 1);
        assert_eq!(
            summary.conflicts[0]["fields"][0]["candidates"][0]["truncated"],
            true
        );

        let resolution = json!({
            "version": 1,
            "decisions": [{
                "conflict_id": summary.conflicts[0]["conflict_id"],
                "kind": "page",
                "logical_key": "guide",
                "strategy": "preserve_both"
            }]
        });
        let first_digest =
            resolve_sync_conflicts(&merged, &summary.conflicts, &resolution).unwrap();
        assert_eq!(
            resolve_sync_conflicts(&merged, &summary.conflicts, &resolution).unwrap(),
            first_digest
        );
        cleanup_sync_conflict_candidates(&merged).unwrap();

        let conn = Connection::open(&merged).unwrap();
        let rows = conn.prepare(
            "SELECT logical_key,payload_json FROM sync_objects WHERE kind='page' ORDER BY logical_key",
        ).unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "guide");
        assert!(rows[1].0.starts_with("guide--sync-"));
        let variant_key = rows[1].0.clone();
        let bodies = rows
            .iter()
            .map(|(_, encoded)| {
                serde_json::from_str::<Value>(encoded).unwrap()["body"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(bodies, BTreeSet::from([local_body, remote_body]));
        let temporary_candidates: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_blobs WHERE content_hash LIKE 'sync-candidate:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(temporary_candidates, 0);
        let conflict_tag: Value = serde_json::from_str(&conn.query_row(
            "SELECT payload_json FROM sync_objects WHERE kind='tag' AND logical_key='sync-conflict'",
            [],
            |row| row.get::<_, String>(0),
        ).unwrap()).unwrap();
        assert!(
            conflict_tag["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|page| page["slug"] == variant_key)
        );

        drop(conn);
        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&merged, &expected, "preserve-page-tag")
            .unwrap();
        assert_eq!(target.conn.query_row(
            "SELECT COUNT(*) FROM page_tags WHERE tag_name='sync-conflict' AND page_slug=?1",
            [&variant_key],
            |row| row.get::<_, i64>(0),
        ).unwrap(), 1);
    }

    #[test]
    fn sync_preserve_both_uses_collision_safe_variant_without_losing_payload() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_owned(),
                title: "Guide".to_owned(),
                kind: None,
                summary: None,
                body: "base".to_owned(),
                source_ids: Vec::new(),
                provenance: vec!["agent-observed".to_owned()],
            })
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        mutate_sync_page(&local, None, Some("local candidate"));
        mutate_sync_page(&remote, None, Some("remote candidate"));
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        let losing_hash = summary.conflicts[0]["candidate_refs"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|value| value.strip_prefix("sync-candidate:"))
            .max()
            .unwrap();
        let occupied_key = sync_conflict_variant_key("page", "guide", losing_hash).unwrap();
        put_normalized_object(
            &merged,
            "page",
            &occupied_key,
            json!({
                "slug": occupied_key,
                "title": "Occupied",
                "kind": null,
                "summary": null,
                "body": "unrelated occupant",
                "structural_navigation": false,
                "source_hashes": [],
                "provenance": ["agent-observed"],
                "created_at": "2026-01-01T00:00:00.000Z",
                "updated_at": "2026-01-01T00:00:00.000Z"
            }),
        );

        resolve_only_preserve_both(&merged, &summary);
        let first = sync_state_digest(&merged).unwrap();
        resolve_only_preserve_both(&merged, &summary);
        assert_eq!(sync_state_digest(&merged).unwrap(), first);

        let bodies = Connection::open(&merged)
            .unwrap()
            .prepare("SELECT payload_json FROM sync_objects WHERE kind='page'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| {
                serde_json::from_str::<Value>(&row.unwrap()).unwrap()["body"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            bodies,
            BTreeSet::from([
                "local candidate".to_owned(),
                "remote candidate".to_owned(),
                "unrelated occupant".to_owned(),
            ])
        );
    }

    #[test]
    fn sync_independent_create_preserve_both_keeps_existing_domain_references_on_original() {
        fn merge_independent(
            temp: &Path,
            name: &str,
            kind: &str,
            key: &str,
            local_payload: Value,
            remote_payload: Value,
            references: &[(&str, &str, Value)],
        ) -> PathBuf {
            let base = temp.join(format!("{name}-base.db"));
            let remote_base = temp.join(format!("{name}-remote-base.db"));
            let local = temp.join(format!("{name}-local.db"));
            let remote = temp.join(format!("{name}-remote.db"));
            create_empty_sync_state(&base).unwrap();
            fs::copy(&base, &local).unwrap();
            fs::copy(&base, &remote_base).unwrap();
            put_normalized_object(&local, kind, key, local_payload);
            put_normalized_object(&remote_base, kind, key, remote_payload);
            for (reference_kind, reference_key, payload) in references {
                put_normalized_object(&local, reference_kind, reference_key, payload.clone());
                put_normalized_object(
                    &remote_base,
                    reference_kind,
                    reference_key,
                    payload.clone(),
                );
            }
            fs::copy(&remote_base, &remote).unwrap();
            let merged = temp.join(format!("{name}-merged.db"));
            let summary = merge_sync_states_directional(
                &base,
                &local,
                &remote_base,
                &remote,
                &merged,
            )
            .unwrap();
            assert_eq!(summary.conflicts[0]["conflict"], "independent_create");
            resolve_only_preserve_both(&merged, &summary);
            merged
        }

        let temp = tempdir().unwrap();
        let todo = merge_independent(
            temp.path(),
            "todo",
            "todo",
            "parent",
            json!({"id":"parent","title":"Local","tags":[]}),
            json!({"id":"parent","title":"Remote","tags":[]}),
            &[(
                "todo",
                "child",
                json!({"id":"child","title":"Child","tags":[],"parent_id":"parent"}),
            )],
        );
        assert_eq!(
            normalized_object(&todo, "todo", "child").unwrap()["parent_id"],
            "parent"
        );

        let page = merge_independent(
            temp.path(),
            "page",
            "page",
            "guide",
            json!({
                "slug":"guide","title":"Local","body":"local","created_at":"2026-01-01T00:00:00.000Z",
                "updated_at":"2026-01-01T00:00:00.000Z"
            }),
            json!({
                "slug":"guide","title":"Remote","body":"remote","created_at":"2026-01-01T00:00:00.000Z",
                "updated_at":"2026-01-01T00:00:00.000Z"
            }),
            &[(
                "tag",
                "existing",
                json!({"name":"existing","pages":[{"slug":"guide"}]}),
            )],
        );
        assert_eq!(
            normalized_object(&page, "tag", "existing").unwrap()["pages"][0]["slug"],
            "guide"
        );

        let memory_payload = |id: &str, context: &str, relations: Value| {
            json!({
                "id":id,"fingerprint":"0".repeat(64),"type":"decision","context":context,
                "occurred_at":"2026-01-01T00:00:00.000Z","recorded_at":"2026-01-01T00:00:00.000Z",
                "valid_from":null,"valid_to":null,"pinned":false,"logical_bytes":1,
                "observed":[],"decision":[],"constraints":[],"learned":[],"unresolved":[],
                "outcome":[],"changes":[],"evidence":[],"relations":relations,"feedback":[]
            })
        };
        let memory = merge_independent(
            temp.path(),
            "memory",
            "memory",
            "event",
            memory_payload("event", "local", json!([])),
            memory_payload("event", "remote", json!([])),
            &[(
                "memory",
                "holder",
                memory_payload(
                    "holder",
                    "holder",
                    json!([{"type":"supports","target":"event","basis":null}]),
                ),
            )],
        );
        assert_eq!(
            normalized_object(&memory, "memory", "holder").unwrap()["relations"][0]["target"],
            "event"
        );
    }

    #[test]
    fn sync_delete_vs_edit_preserve_both_deletes_original_and_keeps_variant() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".to_string(),
                title: "Guide".to_string(),
                kind: None,
                summary: None,
                body: "base".to_string(),
                source_ids: vec![],
                provenance: vec!["agent-observed".to_string()],
            })
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        Connection::open(&local)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                [],
            )
            .unwrap();
        mutate_sync_page(&remote, None, Some("edited"));
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();

        resolve_sync_conflicts(
            &merged,
            &summary.conflicts,
            &json!({
                "version": 1,
                "decisions": [{
                    "conflict_id":summary.conflicts[0]["conflict_id"],
                    "kind":"page","logical_key":"guide","strategy":"preserve_both"
                }]
            }),
        )
        .unwrap();

        let conn = Connection::open(merged).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        let variant: (String, String) = conn
            .query_row(
                "SELECT logical_key,payload_json FROM sync_objects WHERE kind='page'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(variant.0.starts_with("guide--sync-"));
        assert_eq!(
            serde_json::from_str::<Value>(&variant.1).unwrap()["body"],
            "edited"
        );
    }

    fn resolve_only_preserve_both(merged: &Path, summary: &SyncMergeSummary) {
        let conflict = &summary.conflicts[0];
        resolve_sync_conflicts(
            merged,
            &summary.conflicts,
            &json!({
                "version":1,
                "decisions":[{
                    "conflict_id":conflict["conflict_id"],
                    "kind":conflict["kind"],
                    "logical_key":conflict["logical_key"],
                    "strategy":"preserve_both"
                }]
            }),
        )
        .unwrap();
    }

    #[test]
    fn sync_todo_parent_reference_remaps_to_delete_edit_variant() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        let parent = store
            .todo_add(TodoCreateInput {
                title: "parent".into(),
                tags: vec![],
                cue: None,
                detail: None,
                parent_id: None,
                target_at: None,
                request_id: None,
            })
            .unwrap();
        let parent_id = parent["todo"]["id"].as_str().unwrap().to_owned();
        let child = store
            .todo_add(TodoCreateInput {
                title: "child".into(),
                tags: vec![],
                cue: None,
                detail: None,
                parent_id: Some(parent_id.clone()),
                target_at: None,
                request_id: None,
            })
            .unwrap();
        let child_id = child["todo"]["id"].as_str().unwrap().to_owned();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        Connection::open(&local)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='todo' AND logical_key=?1",
                [&parent_id],
            )
            .unwrap();
        let conn = Connection::open(&remote).unwrap();
        let encoded: String = conn
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='todo' AND logical_key=?1",
                [&parent_id],
                |row| row.get(0),
            )
            .unwrap();
        let mut payload: Value = serde_json::from_str(&encoded).unwrap();
        payload["title"] = json!("edited parent");
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind='todo' AND logical_key=?3",
            params![encoded,hash_content(&encoded),parent_id]
        ).unwrap();
        drop(conn);
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        resolve_only_preserve_both(&merged, &summary);
        let conn = Connection::open(&merged).unwrap();
        let variant: String = conn.query_row(
            "SELECT logical_key FROM sync_objects WHERE kind='todo' AND logical_key<>?1 AND logical_key<>?2",
            params![parent_id,child_id], |row| row.get(0)
        ).unwrap();
        let child_payload: Value = serde_json::from_str(
            &conn
                .query_row(
                    "SELECT payload_json FROM sync_objects WHERE kind='todo' AND logical_key=?1",
                    [&child_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(child_payload["parent_id"], variant);
        drop(conn);
        cleanup_sync_conflict_candidates(&merged).unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&merged, &expected, "todo-remap")
            .unwrap();
        let published_parent: String = target
            .conn
            .query_row(
                "SELECT parent_id FROM todo_items WHERE id=?1",
                [child_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(published_parent, variant);
    }

    #[test]
    fn sync_page_tag_reference_remaps_to_delete_edit_variant() {
        let temp = tempdir().unwrap();
        let mut store = test_store();
        store
            .page_put(PagePutInput {
                slug: "guide".into(),
                title: "Guide".into(),
                kind: None,
                summary: None,
                body: "base".into(),
                source_ids: vec![],
                provenance: vec!["agent-observed".into()],
            })
            .unwrap();
        store
            .tag_set("Rules", "guide", 10, "keep membership")
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        Connection::open(&local)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='page' AND logical_key='guide'",
                [],
            )
            .unwrap();
        mutate_sync_page(&remote, None, Some("edited"));
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        resolve_only_preserve_both(&merged, &summary);
        let conn = Connection::open(&merged).unwrap();
        let variant: String = conn
            .query_row(
                "SELECT logical_key FROM sync_objects WHERE kind='page'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let tag:Value=serde_json::from_str(&conn.query_row(
            "SELECT payload_json FROM sync_objects WHERE kind='tag' AND logical_key='Rules'",[],
            |row|row.get::<_,String>(0)
        ).unwrap()).unwrap();
        assert!(
            tag["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|page| page["slug"] == variant)
        );
        assert!(
            !tag["pages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|page| page["slug"] == "guide")
        );
        drop(conn);
        cleanup_sync_conflict_candidates(&merged).unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&merged, &expected, "page-remap")
            .unwrap();
        assert_eq!(target.tag_membership_count("Rules", &variant).unwrap(), 1);
    }

    #[test]
    fn sync_memory_relation_remaps_to_delete_edit_variant() {
        let temp = tempdir().unwrap();
        let store = test_store();
        let original = "11111111111111111111111111111111";
        let related = "22222222222222222222222222222222";
        for (id, context) in [(original, "original"), (related, "related")] {
            store.conn.execute(
                "INSERT INTO memory_events(id,request_id,fingerprint,event_type,context,occurred_at,
                 recorded_at,valid_from,valid_until,pinned,logical_bytes)
                 VALUES(?1,NULL,?2,'decision',?3,'2026-01-01T00:00:00.000Z',
                        '2026-01-01T00:00:00.000Z',NULL,NULL,0,8)",
                params![id,hash_content(context),context]
            ).unwrap();
        }
        store
            .conn
            .execute(
                "INSERT INTO memory_relations(event_id,ordinal,relation_type,target_event_id,basis)
             VALUES(?1,0,'supports',?2,'portable')",
                params![related, original],
            )
            .unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        store.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        Connection::open(&local)
            .unwrap()
            .execute(
                "DELETE FROM sync_objects WHERE kind='memory' AND logical_key=?1",
                [original],
            )
            .unwrap();
        let conn = Connection::open(&remote).unwrap();
        let encoded: String = conn
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='memory' AND logical_key=?1",
                [original],
                |r| r.get(0),
            )
            .unwrap();
        let mut payload: Value = serde_json::from_str(&encoded).unwrap();
        payload["context"] = json!("edited");
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind='memory' AND logical_key=?3",
            params![encoded,hash_content(&encoded),original]
        ).unwrap();
        drop(conn);
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        resolve_only_preserve_both(&merged, &summary);
        let conn = Connection::open(&merged).unwrap();
        let variant: String = conn
            .query_row(
                "SELECT logical_key FROM sync_objects WHERE kind='memory' AND logical_key<>?1",
                [related],
                |r| r.get(0),
            )
            .unwrap();
        let related_payload: Value = serde_json::from_str(
            &conn
                .query_row(
                    "SELECT payload_json FROM sync_objects WHERE kind='memory' AND logical_key=?1",
                    [related],
                    |r| r.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(related_payload["relations"][0]["target"], variant);
        drop(conn);
        cleanup_sync_conflict_candidates(&merged).unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&merged, &expected, "memory-remap")
            .unwrap();
        let published_target: String = target
            .conn
            .query_row(
                "SELECT target_event_id FROM memory_relations WHERE event_id=?1",
                [related],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(published_target, variant);
    }

    #[test]
    fn sync_state_rejects_wrong_store_format() {
        let temp = tempdir().unwrap();
        let normalized = temp.path().join("normalized.db");
        create_empty_sync_state(&normalized).unwrap();
        Connection::open(&normalized)
            .unwrap()
            .execute(
                "UPDATE sync_manifest SET value='999' WHERE key='store_format'",
                [],
            )
            .unwrap();

        let error = sync_state_digest(&normalized).unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert!(error.message.contains("store format"));
    }

    #[test]
    fn sync_publish_rejects_todo_parent_cycles() {
        let temp = tempdir().unwrap();
        let mut source = populated_sync_source();
        let parent_id: String = source
            .conn
            .query_row("SELECT id FROM todo_items", [], |row| row.get(0))
            .unwrap();
        let child = source
            .todo_add(TodoCreateInput {
                title: "Child".to_string(),
                tags: vec![],
                cue: None,
                detail: None,
                parent_id: Some(parent_id.clone()),
                target_at: None,
                request_id: None,
            })
            .unwrap();
        let child_id = child["todo"]["id"].as_str().unwrap().to_owned();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let conn = Connection::open(&normalized).unwrap();
        let encoded: String = conn
            .query_row(
                "SELECT payload_json FROM sync_objects WHERE kind='todo' AND logical_key=?1",
                [&parent_id],
                |row| row.get(0),
            )
            .unwrap();
        let mut payload: Value = serde_json::from_str(&encoded).unwrap();
        payload["parent_id"] = json!(child_id);
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind='todo' AND logical_key=?3",
            params![encoded, hash_content(&encoded), parent_id],
        ).unwrap();
        drop(conn);
        let mut target = test_store();
        let expected = target.identity().unwrap();

        let error = target
            .publish_sync_state(&normalized, &expected, "todo-cycle")
            .unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert!(error.message.contains("cycle"));
        assert_eq!(target.identity().unwrap(), expected);
    }

    #[test]
    fn sync_publish_rejects_multiple_plan_focal_steps() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let conn = Connection::open(&normalized).unwrap();
        let (key, encoded): (String, String) = conn
            .query_row(
                "SELECT logical_key,payload_json FROM sync_objects WHERE kind='plan'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut payload: Value = serde_json::from_str(&encoded).unwrap();
        let mut second = payload["steps"][0].clone();
        second["id"] = json!("44444444444444444444444444444444");
        second["ordinal"] = json!(1);
        second["title"] = json!("Second focal");
        second["status"] = json!("blocked");
        payload["steps"].as_array_mut().unwrap().push(second);
        let encoded = serde_json::to_string(&payload).unwrap();
        conn.execute(
            "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind='plan' AND logical_key=?3",
            params![encoded, hash_content(&encoded), key],
        ).unwrap();
        drop(conn);
        let mut target = test_store();
        let expected = target.identity().unwrap();

        let error = target
            .publish_sync_state(&normalized, &expected, "plan-focals")
            .unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert!(error.message.contains("focal"));
        assert_eq!(target.identity().unwrap(), expected);
    }

    #[test]
    fn sync_plan_history_exports_portable_ordinals_not_local_revisions() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let conn = Connection::open(normalized).unwrap();
        let payload: Value = serde_json::from_str(
            &conn
                .query_row(
                    "SELECT payload_json FROM sync_objects WHERE kind='plan'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
        )
        .unwrap();

        for (ordinal, event) in payload["history"].as_array().unwrap().iter().enumerate() {
            assert!(event.get("revision").is_none());
            assert_eq!(event["ordinal"], ordinal as i64);
        }
    }

    #[test]
    fn sync_conflict_variants_have_stable_domain_identity_and_evidence() {
        let hash = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let todo = json!({"id":"11111111111111111111111111111111","tags":[]});
        let plan = json!({"id":"22222222222222222222222222222222","tags":[]});
        let memory = json!({
            "id":"33333333333333333333333333333333",
            "fingerprint":"0".repeat(64),
            "type":"decision",
            "context":"sync conflict",
            "occurred_at":"2026-01-01T00:00:00.000Z",
            "recorded_at":"2026-01-01T00:00:00.000Z",
            "valid_from":null,
            "valid_to":null,
            "pinned":false,
            "logical_bytes":1,
            "observed":[],
            "decision":["keep both"],
            "constraints":[],
            "learned":[],
            "unresolved":[],
            "outcome":[],
            "changes":[],
            "evidence":[],
            "relations":[],
            "feedback":[]
        });

        for (kind, original, payload) in [("todo", "todo-1", todo), ("plan", "plan-1", plan)] {
            let (first_key, first) = sync_conflict_variant(kind, original, hash, &payload).unwrap();
            let (second_key, second) =
                sync_conflict_variant(kind, original, hash, &payload).unwrap();
            assert_eq!((first_key.clone(), first.clone()), (second_key, second));
            assert_eq!(first_key.len(), 32);
            assert_eq!(first["id"], first_key);
            assert!(
                first["tags"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tag| tag == "sync-conflict")
            );
        }

        let (memory_key, memory_variant) =
            sync_conflict_variant("memory", "memory-1", hash, &memory).unwrap();
        assert_eq!(memory_variant["id"], memory_key);
        assert!(
            memory_variant["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|evidence| { evidence["reference"] == "lwc:sync-conflict:memory-1" })
        );
        assert_ne!(memory_variant["fingerprint"], memory["fingerprint"]);
        assert!(memory_variant["logical_bytes"].as_i64().unwrap() > 1);
    }

    #[test]
    fn sync_publish_to_missing_creates_canonical_store_only_after_validation() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        let database = temp.path().join("new/.lwc/wiki.db");

        let summary =
            Store::publish_sync_state_to_missing("project", &database, &normalized, "initial-sync")
                .unwrap();

        assert!(summary.checkpoint.is_file());
        let target = Store::open_for_read("project", &database).unwrap();
        assert_eq!(
            target.page_show("sync-guide").unwrap().page.title,
            "Sync guide"
        );
        assert_eq!(target.conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE action='sync_merge' AND target='initial-sync'",
            [], |row| row.get::<_, i64>(0),
        ).unwrap(), 1);
    }

    #[test]
    fn sync_publish_to_missing_rejects_malformed_state_without_partial_store() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();
        Connection::open(&normalized)
            .unwrap()
            .execute("DELETE FROM sync_blobs", [])
            .unwrap();
        let database = temp.path().join("new/.lwc/wiki.db");

        let error = Store::publish_sync_state_to_missing(
            "project",
            &database,
            &normalized,
            "initial-invalid",
        )
        .unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert!(!database.exists());
    }

    #[cfg(unix)]
    #[test]
    fn sync_publish_to_missing_rejects_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let normalized = temp.path().join("normalized.db");
        create_empty_sync_state(&normalized).unwrap();
        let real_parent = temp.path().join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let linked_parent = temp.path().join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        let database = linked_parent.join("wiki.db");

        let error = Store::publish_sync_state_to_missing(
            "project",
            &database,
            &normalized,
            "symlink-parent",
        )
        .unwrap_err();

        assert_eq!(error.code, "sync_target_unsafe");
        assert!(!real_parent.join("wiki.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn sync_publish_to_missing_rejects_symlinked_target_and_sidecar() {
        use std::os::unix::fs::symlink;

        for attacked in ["database", "wal"] {
            let temp = tempdir().unwrap();
            let normalized = temp.path().join("normalized.db");
            create_empty_sync_state(&normalized).unwrap();
            let database = temp.path().join("wiki.db");
            let sentinel = temp.path().join("sentinel");
            fs::write(&sentinel, b"must-not-change").unwrap();
            let link = if attacked == "database" {
                database.clone()
            } else {
                database.with_extension("db-wal")
            };
            symlink(&sentinel, link).unwrap();

            let error = Store::publish_sync_state_to_missing(
                "project",
                &database,
                &normalized,
                &format!("symlink-{attacked}"),
            )
            .unwrap_err();

            assert_eq!(error.code, "sync_target_unsafe", "{attacked}");
            assert_eq!(fs::read(&sentinel).unwrap(), b"must-not-change");
        }
    }

    #[cfg(unix)]
    #[test]
    fn sync_publish_to_missing_cleans_new_store_after_checkpoint_preflight_failure() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let normalized = temp.path().join("normalized.db");
        create_empty_sync_state(&normalized).unwrap();
        let database = temp.path().join("wiki.db");
        let sentinel = temp.path().join("checkpoint-sentinel");
        fs::create_dir(&sentinel).unwrap();
        fs::write(sentinel.join("keep"), b"untouched").unwrap();
        symlink(&sentinel, temp.path().join("checkpoints")).unwrap();

        Store::publish_sync_state_to_missing(
            "project",
            &database,
            &normalized,
            "checkpoint-preflight",
        )
        .unwrap_err();

        assert!(!database.exists());
        assert_eq!(fs::read(sentinel.join("keep")).unwrap(), b"untouched");
    }

    #[test]
    fn sync_plan_step_array_conflict_candidate_resolves_and_publishes() {
        let temp = tempdir().unwrap();
        let source = populated_sync_source();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        source.export_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        for (path, title) in [(&local, "Local step"), (&remote, "Remote step")] {
            let conn = Connection::open(path).unwrap();
            let (key, encoded): (String, String) = conn
                .query_row(
                    "SELECT logical_key,payload_json FROM sync_objects WHERE kind='plan'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            let mut payload: Value = serde_json::from_str(&encoded).unwrap();
            payload["steps"][0]["title"] = json!(title);
            let encoded = serde_json::to_string(&payload).unwrap();
            conn.execute(
                "UPDATE sync_objects SET payload_json=?1,payload_hash=?2 WHERE kind='plan' AND logical_key=?3",
                params![encoded, hash_content(&encoded), key],
            ).unwrap();
        }
        let merged = temp.path().join("merged.db");
        let summary = merge_sync_states(&base, &local, &remote, &merged).unwrap();
        let conflict = &summary.conflicts[0];
        let field = &conflict["fields"][0];
        assert_eq!(field["path"], "steps");
        resolve_sync_conflicts(
            &merged,
            &summary.conflicts,
            &json!({
                "version":1,
                "decisions":[{
                    "conflict_id":conflict["conflict_id"],
                    "kind":"plan",
                    "logical_key":conflict["logical_key"],
                    "path":"steps",
                    "candidate":0
                }]
            }),
        )
        .unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        target
            .publish_sync_state(&merged, &expected, "plan-array-resolution")
            .unwrap();
        let title: String = target
            .conn
            .query_row("SELECT title FROM plan_steps", [], |row| row.get(0))
            .unwrap();
        assert!(matches!(title.as_str(), "Local step" | "Remote step"));
    }

    #[test]
    fn sync_prepare_validates_large_blobs_without_retaining_them() {
        let temp = tempdir().unwrap();
        let mut source = test_store();
        source
            .source_add(SourceAddInput {
                title: Some("large".to_string()),
                origin: "large.md".to_string(),
                tracked_path: None,
                content: "x".repeat(4 * 1024 * 1024),
            })
            .unwrap();
        let normalized = temp.path().join("normalized.db");
        source.export_sync_state(&normalized).unwrap();

        let prepared = prepare_sync_state(&normalized).unwrap();

        assert_eq!(prepared.blob_count, 1);
        assert_eq!(prepared.buffered_blob_bytes, 0);
    }

    #[test]
    fn sync_blob_table_supports_incremental_io() {
        let temp = tempdir().unwrap();
        let normalized = temp.path().join("normalized.db");
        create_empty_sync_state(&normalized).unwrap();
        let conn = Connection::open(&normalized).unwrap();
        conn.execute(
            "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,zeroblob(1))",
            ["0".repeat(64)],
        )
        .unwrap();

        let rowid: i64 = conn
            .query_row("SELECT rowid FROM sync_blobs", [], |row| row.get(0))
            .unwrap();
        let blob = conn
            .blob_open(MAIN_DB, "sync_blobs", "content", rowid, true)
            .unwrap();
        assert_eq!(blob.len(), 1);
    }

    #[test]
    fn sync_object_inventory_does_not_copy_source_content() {
        const CONTENT_BYTES: i64 = 16 * 1024 * 1024;
        let temp = tempdir().unwrap();
        let store = test_store();
        store
            .conn
            .execute(
                "INSERT INTO sources(content_hash,title,origin,content,structural_navigation,created_at)
                 VALUES(?1,'large','large.bin',CAST(zeroblob(?2) AS TEXT),0,
                        '2026-01-01T00:00:00.000Z')",
                params!["0".repeat(64), CONTENT_BYTES],
            )
            .unwrap();
        let inventory = temp.path().join("inventory.db");

        store.export_sync_object_inventory(&inventory).unwrap();

        let inventory_bytes = fs::metadata(&inventory).unwrap().len();
        assert!(inventory_bytes < 1024 * 1024, "inventory copied source content");
        let conn = Connection::open(inventory).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sync_objects WHERE kind='source'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='sync_blobs'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn sync_large_blob_export_merge_publish_uses_bounded_rust_buffer() {
        const BLOB_BYTES: usize = 129 * 1024 * 1024;
        SYNC_BLOB_MAX_BUFFERED_BYTES.store(0, Ordering::Relaxed);
        let temp = tempdir().unwrap();
        let store = test_store();
        let mut hasher = Sha256::new();
        let zeros = [0_u8; 64 * 1024];
        for _ in 0..(BLOB_BYTES / zeros.len()) {
            hasher.update(zeros);
        }
        hasher.update(&zeros[..BLOB_BYTES % zeros.len()]);
        let content_hash = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        store
            .conn
            .execute(
                "INSERT INTO sources(content_hash,title,origin,content,structural_navigation,created_at)
                 VALUES(?1,'large','large.bin',CAST(zeroblob(?2) AS TEXT),0,
                        '2026-01-01T00:00:00.000Z')",
                params![content_hash, BLOB_BYTES as i64],
            )
            .unwrap();

        let normalized = temp.path().join("normalized.db");
        store.export_sync_state(&normalized).unwrap();
        sync_state_digest(&normalized).unwrap();
        let merged = temp.path().join("merged.db");
        merge_sync_states_directional(&normalized, &normalized, &normalized, &normalized, &merged)
            .unwrap();
        let mut target = test_store();
        let expected = target.identity().unwrap();
        let summary = target
            .publish_sync_state(&merged, &expected, "large-blob")
            .unwrap();
        assert!(summary.committed);
        assert_eq!(summary.derived["status"], "failed");
        assert_eq!(summary.derived["error"], "sync_source_index_too_large");
        let imported_bytes: i64 = target
            .conn
            .query_row(
                "SELECT length(CAST(content AS BLOB)) FROM sources",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(imported_bytes, BLOB_BYTES as i64);
        assert_eq!(
            target
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM search_fts WHERE doc_type='source'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert!(
            SYNC_BLOB_MAX_BUFFERED_BYTES.load(Ordering::Relaxed) <= 64 * 1024 + 3,
            "incremental validation exceeded its fixed Rust buffer"
        );
    }

    #[test]
    #[ignore = "256 MiB release-mode Session/RSS acceptance"]
    fn sync_256mib_session_delta_round_trip_is_small_and_streamed() {
        const BLOB_BYTES: usize = 1024 * 1024;
        const CHUNK_BYTES: usize = 64 * 1024;

        fn insert_streamed_source(
            conn: &Connection,
            index: usize,
            version: &str,
        ) -> String {
            let mut chunk = vec![b'x'; CHUNK_BYTES];
            let label = format!("source-{index:03}-{version}");
            chunk[..label.len()].copy_from_slice(label.as_bytes());
            let mut hasher = Sha256::new();
            for _ in 0..(BLOB_BYTES / CHUNK_BYTES) {
                hasher.update(&chunk);
            }
            let content_hash = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            conn.execute(
                "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,zeroblob(?2))",
                params![content_hash, BLOB_BYTES as i64],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            let mut blob = conn
                .blob_open(MAIN_DB, "sync_blobs", "content", rowid, false)
                .unwrap();
            for offset in (0..BLOB_BYTES).step_by(CHUNK_BYTES) {
                blob.write_at(&chunk, offset).unwrap();
            }
            insert_sync_object(
                conn,
                "source",
                &content_hash,
                &json!({
                    "content_hash": content_hash,
                    "title": format!("Source {index}"),
                    "origin": format!("source-{index:03}.txt"),
                    "structural_navigation": false,
                    "created_at": "2026-01-01T00:00:00.000Z"
                }),
            )
            .unwrap();
            content_hash
        }

        SYNC_BLOB_MAX_BUFFERED_BYTES.store(0, Ordering::Relaxed);
        let temp = tempdir().unwrap();
        let baseline = temp.path().join("baseline.db");
        create_empty_sync_state(&baseline).unwrap();
        let baseline_hashes = {
            let conn = Connection::open(&baseline).unwrap();
            (0..256)
                .map(|index| insert_streamed_source(&conn, index, "base"))
                .collect::<Vec<_>>()
        };
        let current = temp.path().join("current.db");
        fs::copy(&baseline, &current).unwrap();
        {
            let conn = Connection::open(&current).unwrap();
            for (index, baseline_hash) in baseline_hashes.iter().take(3).enumerate() {
                conn.execute(
                    "DELETE FROM sync_objects WHERE kind='source' AND logical_key=?1",
                    [baseline_hash],
                )
                .unwrap();
                conn.execute(
                    "DELETE FROM sync_blobs WHERE content_hash=?1",
                    [baseline_hash],
                )
                .unwrap();
                insert_streamed_source(&conn, index, "changed");
            }
        }

        let artifact = temp.path().join("transfer.bin");
        let summary = prepare_sync_transfer(Some(&baseline), &current, &artifact).unwrap();
        let full_bytes = fs::metadata(&current).unwrap().len();
        assert_eq!(summary.kind, SyncTransferKind::Delta);
        assert!(summary.size <= full_bytes / 10);
        let restored = temp.path().join("restored.db");
        apply_sync_transfer_artifact(Some(&baseline), &artifact, &summary, &restored).unwrap();
        assert_eq!(sync_state_digest(&restored).unwrap(), sync_state_digest(&current).unwrap());
        assert!(
            SYNC_BLOB_MAX_BUFFERED_BYTES.load(Ordering::Relaxed) <= (CHUNK_BYTES + 3) as u64
        );
    }

    #[test]
    fn sync_merge_preserves_and_validates_draft_only_required_source_blob() {
        let temp = tempdir().unwrap();
        let base = temp.path().join("base.db");
        let local = temp.path().join("local.db");
        let remote = temp.path().join("remote.db");
        create_empty_sync_state(&base).unwrap();
        fs::copy(&base, &local).unwrap();
        fs::copy(&base, &remote).unwrap();
        let content = b"draft-only portable content";
        let content_hash = hash_content(std::str::from_utf8(content).unwrap());
        let conn = Connection::open(&local).unwrap();
        conn.execute(
            "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,?2)",
            params![content_hash, content],
        )
        .unwrap();
        let origin = "a".repeat(64);
        let draft_key = format!("{origin}\0draft-only");
        insert_sync_object(
            &conn,
            "draft_intent",
            &draft_key,
            &json!({
                "origin_store_id": origin,
                "intent": {
                    "version": 1,
                    "origin_changeset_id": "draft-only",
                    "name": "Draft only blob",
                    "actions": [{"kind":"source_add","content_hash":content_hash}],
                    "sources": [{
                        "content_hash":content_hash,
                        "title":"Draft source",
                        "origin":"draft.txt",
                        "structural_navigation":false,
                        "base_fingerprint":"absent",
                        "content_required":true,
                        "ingest":{"status":"pending","attempts":0,"analysis":null,"no_derived_pages_reason":null}
                    }],
                    "pages":[],"tags":[],"meta":[]
                }
            }),
        )
        .unwrap();
        drop(conn);
        let mut uppercase = normalized_object(&local, "draft_intent", &draft_key).unwrap();
        uppercase["intent"]["sources"][0]["content_hash"] = json!("A".repeat(64));
        assert!(required_sync_draft_blobs(&draft_key, &uppercase).is_err());
        let merged = temp.path().join("merged.db");

        merge_sync_states(&base, &local, &remote, &merged).unwrap();

        assert_eq!(
            Connection::open(&merged)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM sync_blobs WHERE content_hash=?1",
                    [&content_hash],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        prepare_sync_state(&merged).unwrap();
        Connection::open(&merged)
            .unwrap()
            .execute("DELETE FROM sync_blobs WHERE content_hash=?1", [&content_hash])
            .unwrap();
        let error = prepare_sync_state(&merged).err().unwrap();
        assert_eq!(error.code, "sync_state_invalid");
        assert!(error.message.contains("draft"));
    }

    #[test]
    fn sync_state_rejects_unexpected_schema_objects_before_digest() {
        let temp = tempdir().unwrap();
        let normalized = temp.path().join("normalized.db");
        create_empty_sync_state(&normalized).unwrap();
        Connection::open(&normalized)
            .unwrap()
            .execute_batch(
                "CREATE TABLE injected(secret TEXT);
             CREATE TRIGGER injected_trigger AFTER INSERT ON sync_objects
             BEGIN INSERT INTO injected(secret) VALUES(NEW.payload_json); END;",
            )
            .unwrap();

        let error = sync_state_digest(&normalized).unwrap_err();

        assert_eq!(error.code, "sync_state_invalid");
        assert!(error.message.contains("schema"));
    }
}
