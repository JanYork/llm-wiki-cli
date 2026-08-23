use super::*;
use tempfile::tempdir;

fn state(id: &str, status: &str, message: &str) -> Value {
    json!({
        "id": id,
        "kind": "graph-project",
        "scope": "project",
        "database": "/private/wiki.db",
        "state": status,
        "phase": "done",
        "completed": 7,
        "total": 9,
        "percent": 77.7,
        "sequence": 3,
        "updated_at_unix_ms": 1234,
        "cancel_requested": false,
        "pid": 4242,
        "message": message,
        "result": {"raw": "do-not-export"},
        "error": {"code": "graph_failed", "message": "do-not-export-error"}
    })
}

#[test]
fn terminal_sync_audits_are_stable_bounded_and_redacted() {
    let temp = tempdir().unwrap();
    let runtime = temp.path().join(".lwc");
    let root = runtime.join("work");
    fs::create_dir_all(&root).unwrap();
    let database = runtime.join("wiki.db");
    fs::write(&database, b"placeholder").unwrap();
    for (suffix, status) in [("a", "succeeded"), ("b", "running"), ("c", "failed")] {
        let id = suffix.repeat(64);
        let dir = root.join(&id);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("state.json"),
            serde_json::to_vec(&state(&id, status, "private message")).unwrap(),
        )
        .unwrap();
    }
    let malformed = root.join("d".repeat(64));
    fs::create_dir(&malformed).unwrap();
    fs::write(malformed.join("state.json"), b"not-json").unwrap();

    let first = terminal_sync_audits(&database, &"e".repeat(64)).unwrap();
    let second = terminal_sync_audits(&database, &"e".repeat(64)).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|audit| audit.origin_store_id == "e".repeat(64))
    );
    assert!(first.iter().all(|audit| audit.origin_work_id.len() == 64));
    assert!(first.iter().all(|audit| audit.result_digest.is_some()));
    assert!(
        first
            .iter()
            .all(|audit| audit.error_code.as_deref() == Some("graph_failed"))
    );
    let encoded = serde_json::to_string(&first).unwrap();
    for forbidden in [
        "/private",
        "\"scope\":\"project\"",
        "4242",
        "private message",
        "do-not-export",
        "do-not-export-error",
        "running",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "leaked {forbidden}: {encoded}"
        );
    }
}

#[test]
fn terminal_sync_audits_fail_closed_above_the_fixed_item_limit() {
    let temp = tempdir().unwrap();
    let runtime = temp.path().join(".lwc");
    let root = runtime.join("work");
    fs::create_dir_all(&root).unwrap();
    let database = runtime.join("wiki.db");
    fs::write(&database, b"placeholder").unwrap();
    for index in 0..=TERMINAL_SYNC_AUDIT_MAX_ITEMS as u64 {
        let id = format!("{index:064x}");
        let dir = root.join(&id);
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("state.json"),
            serde_json::to_vec(&state(&id, "succeeded", "private message")).unwrap(),
        )
        .unwrap();
    }

    let error = terminal_sync_audits(&database, &"e".repeat(64)).unwrap_err();
    assert_eq!(error.code, "sync_audit_limit");
}
