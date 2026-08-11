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

        assert_eq!(store.tag_remove("Rules", "core-rule").unwrap()["action"], "removed");
        assert_eq!(store.tag_remove("Rules", "core-rule").unwrap()["action"], "unchanged");
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
            "DROP TABLE page_tags;
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
}
