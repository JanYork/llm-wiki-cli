use super::*;
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn live_store() -> (tempfile::TempDir, StorePath) {
    let temp = tempdir().unwrap();
    let root = temp.path().join(".lwc");
    fs::create_dir(&root).unwrap();
    let path = root.join("wiki.db");
    Store::initialize("project", &path).unwrap();
    (temp, StorePath::new(Scope::Project, path))
}

fn operation_count(database: &Path) -> i64 {
    Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn hook_store_timeout_companions_preserve_default_and_zero_busy_timeouts() {
    let (_temp, live) = live_store();

    let default_open = Store::open_for_hook("project", &live.path).unwrap();
    assert_eq!(default_open.busy_timeout_millis_for_test().unwrap(), 250);

    let zero_open =
        Store::open_for_hook_with_timeout("project", &live.path, Duration::ZERO).unwrap();
    assert_eq!(zero_open.busy_timeout_millis_for_test().unwrap(), 0);

    let default_snapshot =
        Store::open_for_hook_with_timeout("project", &live.path, Duration::ZERO).unwrap();
    default_snapshot.begin_hook_snapshot().unwrap();
    assert_eq!(
        default_snapshot.busy_timeout_millis_for_test().unwrap(),
        250
    );

    let zero_snapshot = Store::open_for_hook("project", &live.path).unwrap();
    zero_snapshot
        .begin_hook_snapshot_with_timeout(Duration::ZERO)
        .unwrap();
    assert_eq!(zero_snapshot.busy_timeout_millis_for_test().unwrap(), 0);
}

#[test]
fn hook_summary_is_bounded_redacted_deterministic_and_read_only() {
    let (temp, live) = live_store();
    let mut drafts = Vec::new();
    for name in ["draft-a", "draft-b", "draft-c", "draft-d", "draft-e"] {
        begin(&live, name).unwrap();
        drafts.push(resolve_effective(live.clone(), Some(name)).unwrap().path);
    }
    for (index, path) in drafts.iter().enumerate() {
        Store::open("project", path)
            .unwrap()
            .schema_set(&format!("PRIVATE DRAFT BODY {index}"))
            .unwrap();
    }
    let directory = live.path.parent().unwrap().join("changesets");
    fs::write(directory.join("irrelevant.txt"), b"not sqlite").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&live.path, directory.join("linked.db")).unwrap();
    }

    let live_operations = operation_count(&live.path);
    let draft_operations = drafts
        .iter()
        .map(|path| operation_count(path))
        .collect::<Vec<_>>();
    let first = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();
    let second = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.changesets.len(), 3);
    assert_eq!(first.omitted, 2);
    assert_eq!(first.changesets[0].name, "draft-a");
    assert!(!first.changesets[0].empty);
    assert_eq!(first.changesets[0].staged_operation_count, 1);
    assert_eq!(first.changesets[0].status, "draft");
    assert!(!first.changesets[0].conflict);

    let encoded = serde_json::to_value(&first).unwrap();
    assert_eq!(
        encoded
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["changesets".into(), "omitted".into()])
    );
    assert_eq!(
        encoded["changesets"][0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "conflict".into(),
            "empty".into(),
            "id".into(),
            "name".into(),
            "staged_operation_count".into(),
            "status".into(),
        ])
    );
    let encoded = serde_json::to_string(&first).unwrap();
    for forbidden in [
        "database",
        "base_revision",
        "draft_revision",
        "operations",
        "path",
        "PRIVATE DRAFT BODY",
        temp.path().to_str().unwrap(),
    ] {
        assert!(
            !encoded.contains(forbidden),
            "leaked {forbidden}: {encoded}"
        );
    }
    assert_eq!(operation_count(&live.path), live_operations);
    assert_eq!(
        drafts
            .iter()
            .map(|path| operation_count(path))
            .collect::<Vec<_>>(),
        draft_operations
    );
}

#[test]
fn hook_summary_filters_empty_drafts_before_the_limit() {
    let (_temp, live) = live_store();
    for name in ["empty-a", "empty-b", "empty-c"] {
        begin(&live, name).unwrap();
    }
    begin(&live, "relevant-z").unwrap();
    let relevant = resolve_effective(live.clone(), Some("relevant-z"))
        .unwrap()
        .path;
    Store::open("project", relevant)
        .unwrap()
        .schema_set("PRIVATE FOURTH DRAFT BODY")
        .unwrap();

    let summary = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();

    assert_eq!(summary.changesets.len(), 1);
    assert_eq!(summary.changesets[0].name, "relevant-z");
    assert!(!summary.changesets[0].empty);
    assert_eq!(summary.omitted, 0);
    assert!(
        !serde_json::to_string(&summary)
            .unwrap()
            .contains("PRIVATE FOURTH DRAFT BODY")
    );
}

#[test]
fn hook_summary_orders_conflicts_before_nonempty_drafts_then_by_name() {
    let (_temp, live) = live_store();
    for name in ["active-a", "conflict-z"] {
        begin(&live, name).unwrap();
        let path = resolve_effective(live.clone(), Some(name)).unwrap().path;
        Store::open("project", &path)
            .unwrap()
            .schema_set("PRIVATE RELEVANT DRAFT BODY")
            .unwrap();
        if name == "conflict-z" {
            Connection::open(path)
                .unwrap()
                .execute(
                    "UPDATE meta SET value=?1 WHERE key='store_id'",
                    ["f".repeat(64)],
                )
                .unwrap();
        }
    }

    let summary = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();

    assert_eq!(summary.changesets.len(), 2);
    assert_eq!(summary.changesets[0].name, "conflict-z");
    assert!(summary.changesets[0].conflict);
    assert_eq!(summary.changesets[1].name, "active-a");
    assert!(!summary.changesets[1].conflict);
    assert_eq!(summary.omitted, 0);
}

#[test]
fn hook_summary_all_empty_drafts_are_idle() {
    let (_temp, live) = live_store();
    for name in ["empty-a", "empty-b", "empty-c", "empty-d"] {
        begin(&live, name).unwrap();
    }

    let summary = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();

    assert!(summary.changesets.is_empty());
    assert_eq!(summary.omitted, 0);
}

#[test]
fn hook_summary_missing_directory_is_empty_and_does_not_create_it() {
    let (_temp, live) = live_store();
    let directory = live.path.parent().unwrap().join("changesets");

    let summary = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap();

    assert!(summary.changesets.is_empty());
    assert_eq!(summary.omitted, 0);
    assert!(!directory.exists());
}

#[test]
fn hook_summary_fails_open_at_the_fixed_directory_scan_cap() {
    let (_temp, live) = live_store();
    let directory = live.path.parent().unwrap().join("changesets");
    fs::create_dir(&directory).unwrap();
    for index in 0..=CHANGESET_HOOK_MAX_SCAN_ITEMS {
        fs::write(directory.join(format!("junk-{index}")), b"junk").unwrap();
    }

    assert_eq!(
        hook_summary(&live, Instant::now() + Duration::from_secs(2))
            .unwrap_err()
            .code,
        "changeset_hook_limit"
    );
}

#[cfg(unix)]
#[test]
fn hook_summary_rejects_a_symlinked_changeset_directory() {
    let (temp, live) = live_store();
    let outside = temp.path().join("outside-changesets");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(outside, live.path.parent().unwrap().join("changesets")).unwrap();

    let error = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap_err();

    assert_eq!(error.code, "changeset_hook_unavailable");
    assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
}

#[test]
fn hook_summary_fails_open_without_accumulating_busy_waits_across_locked_drafts() {
    let (_temp, live) = live_store();
    let mut draft_paths = Vec::new();
    for index in 0..10 {
        let name = format!("locked-{index:02}");
        begin(&live, &name).unwrap();
        draft_paths.push(resolve_effective(live.clone(), Some(&name)).unwrap().path);
    }
    let operations = draft_paths
        .iter()
        .map(|path| operation_count(path))
        .collect::<Vec<_>>();
    let locks = draft_paths
        .iter()
        .map(|path| {
            let connection = Connection::open(path).unwrap();
            connection
                .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE;")
                .unwrap();
            connection
        })
        .collect::<Vec<_>>();

    let started = Instant::now();
    let error = hook_summary(&live, Instant::now() + Duration::from_millis(400)).unwrap_err();
    let elapsed = started.elapsed();
    drop(locks);

    assert!(
        elapsed < Duration::from_millis(500),
        "Hook summary accumulated per-draft busy waits: {elapsed:?}"
    );
    assert_eq!(error.code, "changeset_hook_unavailable");
    assert!(!error.to_string().contains("locked-"));
    assert_eq!(
        draft_paths
            .iter()
            .map(|path| operation_count(path))
            .collect::<Vec<_>>(),
        operations
    );
}

#[test]
fn hook_summary_fails_open_for_a_shaped_but_unreadable_draft_without_leaking_it() {
    let (temp, live) = live_store();
    begin(&live, "available-draft").unwrap();
    let available = resolve_effective(live.clone(), Some("available-draft"))
        .unwrap()
        .path;
    let before = operation_count(&available);
    let directory = live.path.parent().unwrap().join("changesets");
    fs::write(
        directory.join("private-draft.db"),
        b"PRIVATE INVALID DATABASE",
    )
    .unwrap();

    let error = hook_summary(&live, Instant::now() + Duration::from_secs(2)).unwrap_err();

    assert_eq!(error.code, "changeset_hook_unavailable");
    let error = error.to_string();
    assert!(!error.contains("private-draft"));
    assert!(!error.contains("PRIVATE INVALID DATABASE"));
    assert!(!error.contains(temp.path().to_str().unwrap()));
    assert_eq!(operation_count(&available), before);
}

#[test]
fn hook_summary_expired_deadline_fails_open_without_partial_results_or_writes() {
    let (_temp, live) = live_store();
    begin(&live, "deadline-a").unwrap();
    begin(&live, "deadline-b").unwrap();
    let paths = ["deadline-a", "deadline-b"]
        .map(|name| resolve_effective(live.clone(), Some(name)).unwrap().path);
    Store::open("project", &paths[0])
        .unwrap()
        .schema_set("PRIVATE DEADLINE BODY")
        .unwrap();
    let live_operations = operation_count(&live.path);
    let draft_operations = paths
        .iter()
        .map(|path| operation_count(path))
        .collect::<Vec<_>>();

    let started = Instant::now();
    let error = hook_summary(&live, started).unwrap_err();

    assert_eq!(error.code, "changeset_hook_timeout");
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(operation_count(&live.path), live_operations);
    assert_eq!(
        paths
            .iter()
            .map(|path| operation_count(path))
            .collect::<Vec<_>>(),
        draft_operations
    );
    assert!(!error.to_string().contains("PRIVATE DEADLINE BODY"));
}
