use super::*;
use crate::store::{
    DetachedChangesetAction, DetachedChangesetIntent, DetachedIngestIntent, DetachedMetaIntent,
    DetachedPageAfterImage, DetachedPageIntent, DetachedSourceIntent, DetachedTagAfterImage,
    DetachedTagIntent, DetachedTagMembership, PagePutInput, SourceAddInput, Store,
};
use rusqlite::{Connection, MAIN_DB, params};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

fn live_store() -> (tempfile::TempDir, StorePath) {
    let temp = tempdir().unwrap();
    let root = temp.path().join(".lwc");
    fs::create_dir(&root).unwrap();
    let path = root.join("wiki.db");
    Store::initialize("project", &path).unwrap();
    (temp, StorePath::new(Scope::Project, path))
}

#[test]
fn detached_intent_is_typed_path_free_and_does_not_echo_replay_markers() {
    let (_temp, live) = live_store();
    begin(&live, "portable").unwrap();
    let draft_path = resolve_effective(live.clone(), Some("portable"))
        .unwrap()
        .path;
    let mut draft = Store::open("project", &draft_path).unwrap();
    draft
        .source_add(SourceAddInput {
            title: Some("portable".into()),
            origin: "/private/origin.md".into(),
            tracked_path: Some("/private/tracked.md".into()),
            content: "secret body".into(),
        })
        .unwrap();

    let exports = export_detached_intents(&live).unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].intent.sources.len(), 1);
    assert_eq!(exports[0].intent.sources[0].origin, None);
    assert_eq!(exports[0].blobs.len(), 1);
    assert_eq!(exports[0].blobs[0].draft_database, draft_path);
    let payload = serde_json::to_string(&exports[0].intent).unwrap();
    assert!(!payload.contains("secret body"));
    assert!(!payload.contains("/private/origin.md"));
    assert!(!payload.contains("/private/tracked.md"));
    assert!(!payload.contains("wiki.db"));

    draft
        .changeset_sync_replay_marker("a".repeat(64).as_str(), "b".repeat(64).as_str(), false)
        .unwrap();
    assert!(export_detached_intents(&live).unwrap().is_empty());
}

#[test]
fn replay_marker_rejects_noncanonical_ids_and_complete_before_start() {
    let (_temp, live) = live_store();
    begin(&live, "marker-validation").unwrap();
    let draft_path = resolve_effective(live, Some("marker-validation"))
        .unwrap()
        .path;
    let mut draft = Store::open("project", &draft_path).unwrap();
    let lower = "b".repeat(64);

    let uppercase = draft
        .changeset_sync_replay_marker(&"A".repeat(64), &lower, false)
        .unwrap_err();
    assert_eq!(uppercase.code, "changeset_replay_invalid");
    let incomplete = draft
        .changeset_sync_replay_marker(&"a".repeat(64), &lower, true)
        .unwrap_err();
    assert_eq!(incomplete.code, "changeset_replay_not_started");
}

#[cfg(unix)]
#[test]
fn detached_export_rejects_symlinked_draft_database() {
    use std::os::unix::fs::symlink;

    let (_temp, live) = live_store();
    let drafts = live.path.parent().unwrap().join("changesets");
    fs::create_dir(&drafts).unwrap();
    symlink(&live.path, drafts.join("linked.db")).unwrap();

    assert_eq!(
        export_detached_intents(&live).unwrap_err().code,
        "changeset_path_invalid"
    );
}

#[test]
fn detached_intent_reader_stays_on_one_wal_snapshot() {
    let (_temp, live) = live_store();
    begin(&live, "snapshot").unwrap();
    let draft_path = resolve_effective(live, Some("snapshot")).unwrap().path;
    let mut writer = Store::open("project", &draft_path).unwrap();
    writer
        .source_add(SourceAddInput {
            title: Some("first".into()),
            origin: "first.md".into(),
            tracked_path: None,
            content: "first".into(),
        })
        .unwrap();

    let reader = Store::open_for_read("project", &draft_path).unwrap();
    reader.begin_read_snapshot().unwrap();
    reader.identity().unwrap();
    writer
        .source_add(SourceAddInput {
            title: Some("second".into()),
            origin: "second.md".into(),
            tracked_path: None,
            content: "second".into(),
        })
        .unwrap();

    let (intent, _) = reader.detached_changeset_intent().unwrap().unwrap();
    assert_eq!(intent.sources.len(), 1);
    assert_eq!(intent.sources[0].title.as_deref(), Some("first"));
}

fn replay_intent(contents: &[&str]) -> DetachedChangesetIntent {
    let sources = contents
        .iter()
        .map(|content| DetachedSourceIntent {
            content_hash: Sha256::digest(content.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            title: Some((*content).to_string()),
            origin: Some(format!("{content}.md")),
            structural_navigation: false,
            base_fingerprint: "absent".into(),
            content_required: true,
            ingest: Default::default(),
        })
        .collect::<Vec<_>>();
    DetachedChangesetIntent {
        version: 1,
        origin_changeset_id: "b".repeat(64),
        name: "origin-draft".into(),
        actions: sources
            .iter()
            .map(|source| DetachedChangesetAction::SourceAdd {
                content_hash: source.content_hash.clone(),
            })
            .collect(),
        sources,
        pages: Vec::new(),
        tags: Vec::new(),
        meta: Vec::new(),
    }
}

fn normalized_blobs(contents: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().unwrap();
    let path = temp.path().join("normalized.db");
    crate::store::create_empty_sync_state(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    for content in contents {
        let hash = replay_sha256(content.as_bytes());
        conn.execute(
            "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,?2)",
            params![hash, content.as_bytes()],
        )
        .unwrap();
    }
    drop(conn);
    (temp, path)
}

fn start_replay_draft(live: &StorePath, intent: &DetachedChangesetIntent) -> PathBuf {
    let name = format!(
        "sync-{}-{}",
        &"a".repeat(64)[..12],
        &intent.origin_changeset_id[..12]
    );
    begin(live, &name).unwrap();
    let path = resolve_effective(live.clone(), Some(&name)).unwrap().path;
    Store::open("project", &path)
        .unwrap()
        .changeset_sync_replay_marker(&"a".repeat(64), &intent.origin_changeset_id, false)
        .unwrap();
    path
}

fn start_crashing_item<T: Serialize>(
    draft: &Path,
    intent: &DetachedChangesetIntent,
    key: &str,
    after: &T,
) {
    Store::open("project", draft)
        .unwrap()
        .changeset_sync_replay_start_item(
            &"a".repeat(64),
            &intent.origin_changeset_id,
            key,
            &replay_item_digest(after).unwrap(),
        )
        .unwrap();
}

#[test]
fn typed_replay_creates_a_fresh_local_draft_and_complete_retry_is_idempotent() {
    let (_temp, live) = live_store();
    let intent = replay_intent(&["portable source"]);
    let (_blobs, blob_path) = normalized_blobs(&["portable source"]);
    let first =
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| Ok(blob_path.clone())).unwrap();
    assert_eq!(first.status, "complete");
    assert!(first.created);
    assert_eq!(first.changeset_id.len(), 64);

    let second = replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
        panic!("completed replay must not resolve content again")
    })
    .unwrap();
    assert_eq!(second.changeset_id, first.changeset_id);
    assert!(!second.created);
    assert_eq!(second.status, "complete");
}

#[test]
fn typed_replay_hash_mismatch_fails_closed_then_resumes_without_duplicate_items() {
    let (_temp, live) = live_store();
    let intent = replay_intent(&["first", "second"]);
    let (_first_blobs, first_blob_path) = normalized_blobs(&["first"]);
    let (_wrong_blobs, wrong_blob_path) = normalized_blobs(&["wrong"]);
    let mut first_attempt = 0;
    let error = replay_detached_intent(&live, &"a".repeat(64), &intent, |hash| {
        first_attempt += 1;
        if hash == intent.sources[0].content_hash {
            Ok(first_blob_path.clone())
        } else {
            let conn = Connection::open(&wrong_blob_path).unwrap();
            conn.execute(
                "UPDATE sync_blobs SET content_hash = ?1",
                [&intent.sources[1].content_hash],
            )
            .unwrap();
            Ok(wrong_blob_path.clone())
        }
    })
    .unwrap_err();
    assert_eq!(error.code, "changeset_replay_hash_mismatch");
    assert_eq!(first_attempt, 2);

    let (_second_blobs, second_blob_path) = normalized_blobs(&["second"]);
    let mut resumed = Vec::new();
    let replay = replay_detached_intent(&live, &"a".repeat(64), &intent, |hash| {
        resumed.push(hash.to_string());
        Ok(second_blob_path.clone())
    })
    .unwrap();
    assert_eq!(replay.status, "complete");
    assert_eq!(resumed, vec![intent.sources[1].content_hash.clone()]);
}

#[test]
fn typed_replay_conflicts_when_user_changes_its_local_draft() {
    let (_temp, live) = live_store();
    let intent = replay_intent(&["portable source"]);
    let (_blobs, blob_path) = normalized_blobs(&["portable source"]);
    let replay =
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| Ok(blob_path.clone())).unwrap();
    let draft_path = resolve_effective(live.clone(), Some(&replay.name))
        .unwrap()
        .path;
    Store::open("project", draft_path)
        .unwrap()
        .schema_set("user changed this draft")
        .unwrap();

    let error = replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
        panic!("conflicted replay must not resolve content")
    })
    .unwrap_err();
    assert_eq!(error.code, "changeset_replay_conflict");
    assert_eq!(error.details.as_ref().unwrap()["mutated"], false);
}

#[test]
fn exported_typed_entities_replay_through_store_apis() {
    let (_origin_temp, origin_live) = live_store();
    let origin_store_id = Store::open_for_read("project", &origin_live.path)
        .unwrap()
        .identity()
        .unwrap()
        .store_id;
    begin(&origin_live, "typed-all").unwrap();
    let origin_draft_path = resolve_effective(origin_live.clone(), Some("typed-all"))
        .unwrap()
        .path;
    let added = Store::open("project", &origin_draft_path)
        .unwrap()
        .source_add(SourceAddInput {
            title: Some("source".into()),
            origin: "source.md".into(),
            tracked_path: None,
            content: "portable body".into(),
        })
        .unwrap();
    prepare_page_touch(&origin_live, "typed-all", "source-page", &[added.source.id]).unwrap();
    Store::open("project", &origin_draft_path)
        .unwrap()
        .page_put(crate::store::PagePutInput {
            slug: "source-page".into(),
            title: "Source page".into(),
            kind: Some("source".into()),
            summary: Some("portable".into()),
            body: "portable".into(),
            source_ids: vec![added.source.id],
            provenance: Vec::new(),
        })
        .unwrap();
    prepare_tag_touch(
        &origin_live,
        "typed-all",
        "portable",
        Some("source-page"),
        false,
    )
    .unwrap();
    let mut origin_draft = Store::open("project", &origin_draft_path).unwrap();
    origin_draft
        .tag_set("portable", "source-page", 7, "portable tag")
        .unwrap();
    origin_draft
        .tag_autoload("portable", true, 3, 10, 1_000, "portable tag")
        .unwrap();
    origin_draft.schema_set("portable schema").unwrap();
    origin_draft.ingest_claim(added.source.id, 1, None).unwrap();
    origin_draft
        .ingest_analyze(added.source.id, "portable analysis")
        .unwrap();
    origin_draft
        .ingest_complete(added.source.id, Some("no shared change"))
        .unwrap();
    drop(origin_draft);
    let intent = export_detached_intents(&origin_live)
        .unwrap()
        .remove(0)
        .intent;

    let (_target_temp, target_live) = live_store();
    let (_blobs, blob_path) = normalized_blobs(&["portable body"]);
    let replay = replay_detached_intent(&target_live, &origin_store_id, &intent, |_| {
        Ok(blob_path.clone())
    })
    .unwrap();
    let target_draft_path = resolve_effective(target_live, Some(&replay.name))
        .unwrap()
        .path;
    let target = Store::open_for_read("project", target_draft_path).unwrap();
    assert_eq!(
        target.schema_show().unwrap().schema.as_deref(),
        Some("portable schema")
    );
    assert_eq!(
        target.page_show("source-page").unwrap().page.title,
        "Source page"
    );
    assert_eq!(target.tag_page_identities("portable", 10).unwrap().len(), 1);
    assert_eq!(
        target
            .ingest_list(Some("completed"), 10, 0)
            .unwrap()
            .jobs
            .len(),
        1
    );
}

#[test]
fn replay_recovers_source_mutation_committed_before_item_marker() {
    let (_temp, live) = live_store();
    let intent = replay_intent(&["crash-safe source"]);
    let draft_path = start_replay_draft(&live, &intent);
    let source = &intent.sources[0];
    start_crashing_item(
        &draft_path,
        &intent,
        &format!("source\0{}", source.content_hash),
        source,
    );
    Store::open("project", &draft_path)
        .unwrap()
        .source_add(SourceAddInput {
            title: source.title.clone(),
            origin: source.origin.clone().unwrap(),
            tracked_path: None,
            content: "crash-safe source".into(),
        })
        .unwrap();

    let replay = replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
        panic!("matching committed source must not be resolved twice")
    })
    .unwrap();
    let draft = Store::open_for_read("project", &draft_path).unwrap();
    assert_eq!(replay.status, "complete");
    assert_eq!(
        draft
            .changeset_draft(&replay.name, 0)
            .unwrap()
            .action_counts["source_add"],
        1
    );
}

#[test]
fn replay_pending_item_does_not_absorb_an_unrelated_user_mutation() {
    let (_temp, live) = live_store();
    let intent = replay_intent(&["crash-safe source"]);
    let draft_path = start_replay_draft(&live, &intent);
    let source = &intent.sources[0];
    start_crashing_item(
        &draft_path,
        &intent,
        &format!("source\0{}", source.content_hash),
        source,
    );
    let mut draft = Store::open("project", &draft_path).unwrap();
    draft
        .source_add(SourceAddInput {
            title: source.title.clone(),
            origin: source.origin.clone().unwrap(),
            tracked_path: None,
            content: "crash-safe source".into(),
        })
        .unwrap();
    draft.schema_set("unrelated user edit").unwrap();
    drop(draft);

    let error = replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
        panic!("conflict must not resolve content")
    })
    .unwrap_err();
    assert_eq!(error.code, "changeset_replay_conflict");
    assert_eq!(
        Store::open_for_read("project", &draft_path)
            .unwrap()
            .schema_show()
            .unwrap()
            .schema
            .as_deref(),
        Some("unrelated user edit")
    );
}

#[test]
fn replay_streams_source_larger_than_128_mib_with_a_fixed_buffer() {
    const BYTES: usize = 129 * 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    let zeroes = [0_u8; CHUNK];
    let mut hasher = Sha256::new();
    for _ in 0..(BYTES / CHUNK) {
        hasher.update(zeroes);
    }
    let hash = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let blob_temp = tempdir().unwrap();
    let blob_path = blob_temp.path().join("normalized.db");
    crate::store::create_empty_sync_state(&blob_path).unwrap();
    let conn = Connection::open(&blob_path).unwrap();
    conn.execute(
        "INSERT INTO sync_blobs(content_hash,content) VALUES(?1,zeroblob(?2))",
        params![&hash, BYTES as i64],
    )
    .unwrap();
    let rowid: i64 = conn
        .query_row("SELECT rowid FROM sync_blobs", [], |row| row.get(0))
        .unwrap();
    let mut blob = conn
        .blob_open(MAIN_DB, "sync_blobs", "content", rowid, false)
        .unwrap();
    for offset in (0..BYTES).step_by(CHUNK) {
        blob.write_at(&zeroes, offset).unwrap();
    }
    drop(blob);
    drop(conn);

    let (_temp, live) = live_store();
    let mut intent = replay_intent(&[""]);
    intent.sources[0].content_hash = hash.clone();
    intent.sources[0].title = Some("large".into());
    intent.sources[0].origin = Some("large.txt".into());
    intent.actions = vec![DetachedChangesetAction::SourceAdd { content_hash: hash }];
    replay_detached_intent(&live, &"a".repeat(64), &intent, |_| Ok(blob_path.clone())).unwrap();
    assert!(Store::changeset_replay_blob_max_buffered_bytes() <= (CHUNK + 3) as u64);
}

#[test]
fn replay_recovers_page_meta_tag_and_ingest_crash_windows_without_duplicates() {
    // Page after-image.
    {
        let (_temp, live) = live_store();
        let page = DetachedPageIntent {
            slug: "crash-page".into(),
            base_fingerprint: "absent".into(),
            after: Some(DetachedPageAfterImage {
                title: "Crash page".into(),
                kind: Some("source".into()),
                summary: None,
                body: "body".into(),
                source_hashes: Vec::new(),
                provenance: vec!["user-provided".into()],
            }),
        };
        let intent = DetachedChangesetIntent {
            version: 1,
            origin_changeset_id: "b".repeat(64),
            name: "page".into(),
            actions: vec![DetachedChangesetAction::PagePut {
                slug: page.slug.clone(),
            }],
            sources: Vec::new(),
            pages: vec![page.clone()],
            tags: Vec::new(),
            meta: Vec::new(),
        };
        let draft_path = start_replay_draft(&live, &intent);
        start_crashing_item(&draft_path, &intent, "page\0crash-page", &page);
        prepare_page_touch(&live, "sync-aaaaaaaaaaaa-bbbbbbbbbbbb", "crash-page", &[]).unwrap();
        Store::open("project", &draft_path)
            .unwrap()
            .page_put(PagePutInput {
                slug: page.slug.clone(),
                title: "Crash page".into(),
                kind: Some("source".into()),
                summary: None,
                body: "body".into(),
                source_ids: Vec::new(),
                provenance: vec!["user-provided".into()],
            })
            .unwrap();
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
            panic!("page replay has no blobs")
        })
        .unwrap();
        assert_eq!(
            Store::open_for_read("project", &draft_path)
                .unwrap()
                .changeset_draft("sync-aaaaaaaaaaaa-bbbbbbbbbbbb", 0)
                .unwrap()
                .action_counts["page_put"],
            1
        );
    }

    // Meta after-image.
    {
        let (_temp, live) = live_store();
        let meta = DetachedMetaIntent {
            key: "schema".into(),
            base_fingerprint: "absent".into(),
            value: "crash schema".into(),
        };
        let intent = DetachedChangesetIntent {
            version: 1,
            origin_changeset_id: "b".repeat(64),
            name: "meta".into(),
            actions: vec![DetachedChangesetAction::MetaSet {
                key: "schema".into(),
            }],
            sources: Vec::new(),
            pages: Vec::new(),
            tags: Vec::new(),
            meta: vec![meta.clone()],
        };
        let draft_path = start_replay_draft(&live, &intent);
        start_crashing_item(&draft_path, &intent, "meta\0schema", &meta);
        Store::open("project", &draft_path)
            .unwrap()
            .schema_set("crash schema")
            .unwrap();
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
            panic!()
        })
        .unwrap();
        assert_eq!(
            Store::open_for_read("project", &draft_path)
                .unwrap()
                .changeset_draft("sync-aaaaaaaaaaaa-bbbbbbbbbbbb", 0)
                .unwrap()
                .action_counts["schema_set"],
            1
        );
    }

    // Tag after-image, with its member page present in the live base.
    {
        let (_temp, live) = live_store();
        Store::open("project", &live.path)
            .unwrap()
            .page_put(PagePutInput {
                slug: "member".into(),
                title: "Member".into(),
                kind: None,
                summary: None,
                body: "member".into(),
                source_ids: Vec::new(),
                provenance: vec!["user-provided".into()],
            })
            .unwrap();
        let tag = DetachedTagIntent {
            name: "crash-tag".into(),
            base_fingerprint: "absent".into(),
            after: Some(DetachedTagAfterImage {
                autoload: true,
                autoload_priority: 2,
                autoload_limit: 10,
                autoload_max_chars: 1000,
                reason: "reason".into(),
                memberships: vec![DetachedTagMembership {
                    page_slug: "member".into(),
                    priority: 3,
                    reason: "member reason".into(),
                }],
            }),
        };
        let intent = DetachedChangesetIntent {
            version: 1,
            origin_changeset_id: "b".repeat(64),
            name: "tag".into(),
            actions: vec![DetachedChangesetAction::Tag {
                action: "tag_set".into(),
                name: tag.name.clone(),
            }],
            sources: Vec::new(),
            pages: Vec::new(),
            tags: vec![tag.clone()],
            meta: Vec::new(),
        };
        let draft_path = start_replay_draft(&live, &intent);
        start_crashing_item(&draft_path, &intent, "tag\0crash-tag", &tag);
        prepare_tag_touch(
            &live,
            "sync-aaaaaaaaaaaa-bbbbbbbbbbbb",
            "crash-tag",
            Some("member"),
            false,
        )
        .unwrap();
        let mut draft = Store::open("project", &draft_path).unwrap();
        draft
            .tag_set("crash-tag", "member", 3, "member reason")
            .unwrap();
        draft
            .tag_autoload("crash-tag", true, 2, 10, 1000, "reason")
            .unwrap();
        drop(draft);
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
            panic!()
        })
        .unwrap();
        assert_eq!(
            Store::open_for_read("project", &draft_path)
                .unwrap()
                .tag_page_identities("crash-tag", 10)
                .unwrap()
                .len(),
            1
        );
    }

    // Ingest after-image for an existing source; its content is not resolved.
    {
        let (_temp, live) = live_store();
        let added = Store::open("project", &live.path)
            .unwrap()
            .source_add(SourceAddInput {
                title: Some("existing".into()),
                origin: "existing.md".into(),
                tracked_path: None,
                content: "existing".into(),
            })
            .unwrap();
        let source = DetachedSourceIntent {
            content_hash: added.source.content_hash.clone(),
            title: Some("existing".into()),
            origin: Some("existing.md".into()),
            structural_navigation: false,
            base_fingerprint: added.source.content_hash.clone(),
            content_required: false,
            ingest: DetachedIngestIntent {
                status: "analyzing".into(),
                attempts: 1,
                analysis: None,
                no_derived_pages_reason: None,
            },
        };
        let intent = DetachedChangesetIntent {
            version: 1,
            origin_changeset_id: "b".repeat(64),
            name: "ingest".into(),
            actions: vec![DetachedChangesetAction::Ingest {
                action: "ingest_claim".into(),
                content_hash: source.content_hash.clone(),
            }],
            sources: vec![source.clone()],
            pages: Vec::new(),
            tags: Vec::new(),
            meta: Vec::new(),
        };
        let draft_path = start_replay_draft(&live, &intent);
        Store::open("project", &draft_path)
            .unwrap()
            .changeset_replay_prepare_existing_source(&live.path, &source)
            .unwrap();
        let ingest_key = format!("ingest\0{}", source.content_hash);
        start_crashing_item(&draft_path, &intent, &ingest_key, &source.ingest);
        Store::open("project", &draft_path)
            .unwrap()
            .ingest_claim(added.source.id, 1, None)
            .unwrap();
        replay_detached_intent(&live, &"a".repeat(64), &intent, |_| -> Result<PathBuf> {
            panic!("existing source ingest must not resolve content")
        })
        .unwrap();
        assert!(
            Store::open_for_read("project", &draft_path)
                .unwrap()
                .changeset_replay_ingest_matches(&source.content_hash, &source.ingest)
                .unwrap()
        );
    }
}

#[test]
fn replay_markers_are_idempotent_and_sparse_safe() {
    let (_temp, live) = live_store();
    begin(&live, "marker").unwrap();
    let draft_path = resolve_effective(live, Some("marker")).unwrap().path;
    let mut draft = Store::open("project", &draft_path).unwrap();
    let origin = "a".repeat(64);
    let changeset = "b".repeat(64);

    assert!(
        draft
            .changeset_sync_replay_marker(&origin, &changeset, false)
            .unwrap()
    );
    assert!(
        !draft
            .changeset_sync_replay_marker(&origin, &changeset, false)
            .unwrap()
    );
    assert!(
        draft
            .changeset_sync_replay_marker(&origin, &changeset, true)
            .unwrap()
    );
    assert!(
        !draft
            .changeset_sync_replay_marker(&origin, &changeset, true)
            .unwrap()
    );
    draft.validate_changeset_integrity().unwrap();
}
