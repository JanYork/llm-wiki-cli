use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

const BOOK_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const BOOK_BLOCK_ID: &str = "cccccccccccccccccccccccccccccccc";
const BOOK_HASH: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const TUTOR_TURN_ID: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn run(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lwc-practice"));
    command.current_dir(cwd).env("HOME", home).args(args);
    command.output().expect("run lwc-practice")
}

fn ok(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = run(cwd, home, args);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("success JSON")
}

fn err(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = run(cwd, home, args);
    assert!(!output.status.success(), "command unexpectedly succeeded");
    serde_json::from_slice(&output.stderr).expect("error JSON")
}

fn json_arg(value: Value) -> String {
    value.to_string()
}

fn subject(cwd: &Path, home: &Path) -> String {
    let input = json_arg(json!({"name":"会计学","request_id":"practice-subject-accounting"}));
    let id = ok(cwd, home, &["subject", "create", "--json", &input])["result"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    seed_source_truth(home, &id);
    id
}

fn seed_source_truth(home: &Path, subject_id: &str) {
    let book_root = home.join(".lwc/plugins/book");
    let tutor_root = home.join(".lwc/plugins/tutor");
    std::fs::create_dir_all(&book_root).unwrap();
    std::fs::create_dir_all(&tutor_root).unwrap();
    let book = rusqlite::Connection::open(book_root.join("data.sqlite3")).unwrap();
    book.execute_batch(
        "CREATE TABLE IF NOT EXISTS books(id TEXT PRIMARY KEY,subject_id TEXT NOT NULL,original_sha256 TEXT NOT NULL,normalized_sha256 TEXT,state TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS book_blocks(id TEXT PRIMARY KEY,book_id TEXT NOT NULL,text_hash TEXT NOT NULL);",
    )
    .unwrap();
    book.execute(
        "INSERT OR REPLACE INTO books VALUES(?1,?2,?3,?3,'ready')",
        rusqlite::params![BOOK_ID, subject_id, BOOK_HASH],
    )
    .unwrap();
    book.execute(
        "INSERT OR REPLACE INTO book_blocks VALUES(?1,?2,?3)",
        rusqlite::params![BOOK_BLOCK_ID, BOOK_ID, BOOK_HASH],
    )
    .unwrap();
    let tutor = rusqlite::Connection::open(tutor_root.join("data.sqlite3")).unwrap();
    tutor
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS tutor_sessions(id TEXT PRIMARY KEY,subject_id TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS tutor_turns(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,state TEXT NOT NULL,revision INTEGER NOT NULL);",
        )
        .unwrap();
    tutor
        .execute(
            "INSERT OR REPLACE INTO tutor_sessions VALUES('ffffffffffffffffffffffffffffffff',?1)",
            [subject_id],
        )
        .unwrap();
    tutor
        .execute(
            "INSERT OR REPLACE INTO tutor_turns VALUES(?1,'ffffffffffffffffffffffffffffffff','committed',1)",
            [TUTOR_TURN_ID],
        )
        .unwrap();
}

fn bank(cwd: &Path, home: &Path, subject_id: &str, key: &str, request_id: &str) -> Value {
    let input = json_arg(json!({
        "subject_id": subject_id,
        "key": key,
        "title": "精准题库",
        "source": {"kind":"book","id":BOOK_ID,"revision_or_hash":BOOK_HASH,"subject_id":subject_id},
        "request_id": request_id
    }));
    ok(cwd, home, &["bank", "create", "--json", &input])["result"]["bank"].clone()
}

fn item_input(subject_id: &str, request_id: &str, prompt: &str, answer: Value) -> Value {
    json!({
        "subject_id": subject_id,
        "item_type": "choice",
        "grading_kind": "objective",
        "prompt": prompt,
        "answer": answer,
        "rubric": null,
        "max_points": 2.0,
        "estimated_minutes": 3,
        "difficulty": 0.5,
        "source": {"kind":"book","id":BOOK_ID,"revision_or_hash":BOOK_HASH,"locator":BOOK_BLOCK_ID,"subject_id":subject_id},
        "request_id": request_id
    })
}

fn create_item(cwd: &Path, home: &Path, input: &Value) -> Value {
    let raw = json_arg(input.clone());
    ok(cwd, home, &["item", "create", "--json", &raw])["result"]["item"].clone()
}

fn verify_item(cwd: &Path, home: &Path, item: &Value, input: &Value, request_id: &str) -> Value {
    let verify = json_arg(json!({
        "prompt": input["prompt"],
        "answer": input["answer"],
        "rubric": input["rubric"],
        "source": input["source"],
        "request_id": request_id
    }));
    ok(
        cwd,
        home,
        &[
            "item",
            "verify",
            item["id"].as_str().unwrap(),
            "--if-revision",
            &item["revision"].to_string(),
            "--json",
            &verify,
        ],
    )["result"]["item"]
        .clone()
}

fn add_to_bank(cwd: &Path, home: &Path, bank: &Value, item: &Value, request: &str) -> Value {
    let input = json_arg(json!({
        "item_id": item["id"],
        "item_revision": item["revision"],
        "request_id": request
    }));
    ok(
        cwd,
        home,
        &[
            "bank",
            "add",
            bank["id"].as_str().unwrap(),
            "--if-revision",
            &bank["revision"].to_string(),
            "--json",
            &input,
        ],
    )["result"]["bank"]
        .clone()
}

#[test]
fn exact_sources_verification_and_bank_membership_are_versioned() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        &format!("book:{BOOK_ID}"),
        "bank-book",
    );
    assert_eq!(bank["key"], format!("book:{BOOK_ID}"));

    let input = item_input(&subject_id, "item-ledger", "借方增加哪类账户？", json!("A"));
    let draft = create_item(world.path(), home.path(), &input);
    assert_eq!(draft["state"], "draft");

    let stale = json_arg(json!({
        "prompt": input["prompt"], "answer": input["answer"], "rubric": null,
        "source": {"kind":"book","id":BOOK_ID,"revision_or_hash":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","locator":BOOK_BLOCK_ID,"subject_id":subject_id},
        "request_id":"verify-stale"
    }));
    let failure = err(
        world.path(),
        home.path(),
        &[
            "item",
            "verify",
            draft["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &stale,
        ],
    );
    assert_eq!(failure["error"]["code"], "stale_source");

    let source_database = home.path().join(".lwc/plugins/book/data.sqlite3");
    let source = rusqlite::Connection::open(&source_database).unwrap();
    source
        .execute("UPDATE book_blocks SET text_hash='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'", [])
        .unwrap();
    drop(source);
    let exact_but_mutated = json_arg(json!({
        "prompt": input["prompt"], "answer": input["answer"], "rubric": null,
        "source": input["source"], "request_id":"verify-mutated-source"
    }));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "item",
                "verify",
                draft["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--json",
                &exact_but_mutated,
            ],
        )["error"]["code"],
        "source_truth_mismatch"
    );
    let source = rusqlite::Connection::open(source_database).unwrap();
    source
        .execute("UPDATE book_blocks SET text_hash=?1", [BOOK_HASH])
        .unwrap();

    let verified = verify_item(world.path(), home.path(), &draft, &input, "verify-ledger");
    assert_eq!(verified["state"], "verified");
    assert_eq!(verified["revision"], 2);
    let bank = add_to_bank(
        world.path(),
        home.path(),
        &bank,
        &verified,
        "bank-add-ledger",
    );
    assert_eq!(bank["item_count"], 1);

    let retire = json_arg(json!({"reason":"source superseded","request_id":"retire-ledger"}));
    let retired = ok(
        world.path(),
        home.path(),
        &[
            "item",
            "retire",
            verified["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &retire,
        ],
    )["result"]["item"]
        .clone();
    assert_eq!(retired["state"], "retired");
    assert_eq!(retired["revision"], 3);
    let retired_paper = json_arg(
        json!({"bank_id":bank["id"],"count":1,"duration_minutes":10,"request_id":"retired-paper"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["paper", "create", "--json", &retired_paper],
        )["error"]["code"],
        "paper_shortage"
    );

    let replay = create_item(world.path(), home.path(), &input);
    assert_eq!(replay["id"], draft["id"]);
    let changed = json_arg(
        json!({"subject_id":subject_id,"item_type":"choice","grading_kind":"objective","prompt":"different","answer":"A","rubric":null,"max_points":2,"estimated_minutes":3,"difficulty":0.5,"source":input["source"],"request_id":"item-ledger"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["item", "create", "--json", &changed]
        )["error"]["code"],
        "request_id_reused"
    );
}

#[test]
fn book_default_and_subject_banks_allow_exact_mixed_sources() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());

    let book_bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        &format!("book:{BOOK_ID}"),
        "bank-book-default",
    );
    assert_eq!(book_bank["source"]["kind"], "book");

    let stale_input = json_arg(json!({
        "subject_id": subject_id,
        "key": "subject:stale",
        "title": "过期科目题库",
        "source": {
            "kind": "subject",
            "id": subject_id,
            "revision_or_hash": "2",
            "subject_id": subject_id
        },
        "request_id": "bank-subject-stale"
    }));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["bank", "create", "--json", &stale_input],
        )["error"]["code"],
        "source_truth_mismatch"
    );
    let practice =
        rusqlite::Connection::open(home.path().join(".lwc/plugins/practice/data.sqlite3")).unwrap();
    assert_eq!(
        practice
            .query_row("SELECT COUNT(*) FROM practice_banks", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    drop(practice);

    let mixed_input = json_arg(json!({
        "subject_id": subject_id,
        "key": format!("subject:{subject_id}"),
        "title": "科目混合题库",
        "source": {
            "kind": "subject",
            "id": subject_id,
            "revision_or_hash": "1",
            "subject_id": subject_id
        },
        "request_id": "bank-subject-mixed"
    }));
    let mixed = ok(
        world.path(),
        home.path(),
        &["bank", "create", "--json", &mixed_input],
    )["result"]["bank"]
        .clone();
    assert_eq!(mixed["key"], format!("subject:{subject_id}"));

    let book_input = item_input(&subject_id, "mixed-book-item", "Book evidence?", json!("A"));
    let book_draft = create_item(world.path(), home.path(), &book_input);
    let book_item = verify_item(
        world.path(),
        home.path(),
        &book_draft,
        &book_input,
        "mixed-book-verify",
    );
    let mixed = add_to_bank(
        world.path(),
        home.path(),
        &mixed,
        &book_item,
        "mixed-add-book",
    );

    let tutor_input = json!({
        "subject_id": subject_id,
        "item_type": "choice",
        "grading_kind": "objective",
        "prompt": "Tutor evidence?",
        "answer": "B",
        "rubric": null,
        "max_points": 2.0,
        "estimated_minutes": 3,
        "difficulty": 0.5,
        "source": {
            "kind": "tutor_turn",
            "id": TUTOR_TURN_ID,
            "revision_or_hash": "1",
            "subject_id": subject_id
        },
        "request_id": "mixed-tutor-item"
    });
    let tutor_draft = create_item(world.path(), home.path(), &tutor_input);
    let tutor_item = verify_item(
        world.path(),
        home.path(),
        &tutor_draft,
        &tutor_input,
        "mixed-tutor-verify",
    );
    let mixed = add_to_bank(
        world.path(),
        home.path(),
        &mixed,
        &tutor_item,
        "mixed-add-tutor",
    );
    assert_eq!(mixed["item_count"], 2);
}

#[test]
fn missing_stale_wrong_kind_and_cross_subject_sources_write_nothing() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let stale_bank = json_arg(json!({
        "subject_id":subject_id,"key":format!("book:{BOOK_ID}"),"title":"stale",
        "source":{"kind":"book","id":BOOK_ID,"revision_or_hash":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","subject_id":subject_id},
        "request_id":"stale-bank-source"
    }));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["bank", "create", "--json", &stale_bank],
        )["error"]["code"],
        "source_truth_mismatch"
    );
    let wrong_kind = json_arg(json!({
        "subject_id":subject_id,"key":"subject:wrong-kind","title":"wrong",
        "source":{"kind":"tutor_turn","id":TUTOR_TURN_ID,"revision_or_hash":"1","subject_id":subject_id},
        "request_id":"wrong-kind-bank-source"
    }));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["bank", "create", "--json", &wrong_kind],
        )["error"]["code"],
        "invalid_source_ref"
    );
    let mut missing_hash = item_input(&subject_id, "missing-item-hash", "missing", json!("A"));
    missing_hash["source"]["revision_or_hash"] = json!("");
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["item", "create", "--json", &json_arg(missing_hash)],
        )["error"]["code"],
        "invalid_source_ref"
    );
    let other = json_arg(json!({"name":"审计学","request_id":"other-subject"}));
    let other_id = ok(
        world.path(),
        home.path(),
        &["subject", "create", "--json", &other],
    )["result"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cross = json!({
        "subject_id":other_id,"item_type":"text","grading_kind":"subjective",
        "prompt":"cross","answer":null,"rubric":"exact","max_points":2,
        "estimated_minutes":1,"difficulty":0.5,
        "source":{"kind":"tutor_turn","id":TUTOR_TURN_ID,"revision_or_hash":"1","subject_id":other_id},
        "request_id":"cross-subject-item"
    });
    let draft = create_item(world.path(), home.path(), &cross);
    let verify = json_arg(json!({
        "prompt":"cross","answer":null,"rubric":"exact","source":cross["source"],
        "request_id":"cross-subject-verify"
    }));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "item",
                "verify",
                draft["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--json",
                &verify,
            ],
        )["error"]["code"],
        "source_truth_mismatch"
    );
    assert_eq!(
        ok(world.path(), home.path(), &["status"])["result"]["banks"],
        0
    );
}

#[test]
fn papers_are_all_or_nothing_and_attempt_responses_survive_terminal_state() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:default",
        "bank-default",
    );

    let shortage = json_arg(
        json!({"bank_id":bank["id"],"count":2,"duration_minutes":10,"request_id":"paper-short"}),
    );
    let failure = err(
        world.path(),
        home.path(),
        &["paper", "create", "--json", &shortage],
    );
    assert_eq!(failure["error"]["code"], "paper_shortage");
    assert_eq!(failure["error"]["details"]["missing"], 2);
    assert_eq!(
        ok(world.path(), home.path(), &["status"])["result"]["papers"],
        0
    );

    let item_input = item_input(
        &subject_id,
        "item-cash",
        "库存现金增加记哪边？",
        json!("借"),
    );
    let item = create_item(world.path(), home.path(), &item_input);
    let item = verify_item(world.path(), home.path(), &item, &item_input, "verify-cash");
    bank = add_to_bank(world.path(), home.path(), &bank, &item, "bank-add-cash");

    let paper_input = json_arg(
        json!({"bank_id":bank["id"],"count":1,"duration_minutes":10,"request_id":"paper-one"}),
    );
    let paper = ok(
        world.path(),
        home.path(),
        &["paper", "create", "--json", &paper_input],
    )["result"]["paper"]
        .clone();
    assert_eq!(paper["items"].as_array().unwrap().len(), 1);
    assert_eq!(paper["items"][0]["item_revision"], 2);
    assert_eq!(paper["items"][0]["prompt"], "库存现金增加记哪边？");

    let attempt_input =
        json_arg(json!({"paper_id":paper["id"],"owner":"mac-a","request_id":"attempt-one"}));
    let attempt = ok(
        world.path(),
        home.path(),
        &["attempt", "create", "--json", &attempt_input],
    )["result"]["attempt"]
        .clone();
    let response_input = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"choice","value":"贷","request_id":"response-one"}),
    );
    let saved = ok(
        world.path(),
        home.path(),
        &[
            "response",
            "save",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--owner",
            "mac-a",
            "--json",
            &response_input,
        ],
    )["result"]
        .clone();
    assert_eq!(saved["response"]["revision"], 1);
    assert_eq!(saved["attempt"]["revision"], 2);

    let edit = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"choice","value":"借","request_id":"response-edit"}),
    );
    let edited = ok(
        world.path(),
        home.path(),
        &[
            "response",
            "save",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--owner",
            "mac-a",
            "--json",
            &edit,
        ],
    )["result"]
        .clone();
    assert_eq!(edited["response"]["revision"], 2);

    let submit = json_arg(json!({"owner":"mac-a","request_id":"attempt-submit"}));
    let submitted = ok(
        world.path(),
        home.path(),
        &[
            "attempt",
            "submit",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "3",
            "--json",
            &submit,
        ],
    )["result"]["attempt"]
        .clone();
    assert_eq!(submitted["state"], "submitted");
    let frozen_input = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"choice","value":"借","request_id":"response-after-submit"}),
    );
    let frozen = err(
        world.path(),
        home.path(),
        &[
            "response",
            "save",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "4",
            "--owner",
            "mac-a",
            "--json",
            &frozen_input,
        ],
    );
    assert_eq!(frozen["error"]["code"], "attempt_frozen");

    let shown = ok(
        world.path(),
        home.path(),
        &["attempt", "show", attempt["id"].as_str().unwrap()],
    )["result"]["attempt"]
        .clone();
    assert_eq!(
        shown["responses"][0]["history"].as_array().unwrap().len(),
        2
    );
    assert_eq!(shown["responses"][0]["value"], "借");
}

#[test]
fn grading_sets_and_review_budget_preserve_history_and_visible_debt() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut review_bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:default",
        "bank-review",
    );

    let mut item_ids = Vec::new();
    for n in 0..2 {
        let mut input = item_input(
            &subject_id,
            &format!("review-item-{n}"),
            &format!("题目 {n}"),
            json!("A"),
        );
        input["item_type"] = json!("flashcard");
        input["answer"] = json!(3);
        let item = create_item(world.path(), home.path(), &input);
        let item = verify_item(
            world.path(),
            home.path(),
            &item,
            &input,
            &format!("verify-review-{n}"),
        );
        review_bank = add_to_bank(
            world.path(),
            home.path(),
            &review_bank,
            &item,
            &format!("bank-review-add-{n}"),
        );
        item_ids.push(item["id"].as_str().unwrap().to_owned());
    }

    let set_input = json_arg(
        json!({"subject_id":subject_id,"name":"错题集","kind":"mistake","request_id":"mistake-set"}),
    );
    let set = ok(
        world.path(),
        home.path(),
        &["set", "create", "--json", &set_input],
    )["result"]["set"]
        .clone();
    let add = json_arg(
        json!({"item_id":item_ids[0],"item_revision":2,"reason":"wrong","request_id":"mistake-add"}),
    );
    let member = ok(
        world.path(),
        home.path(),
        &[
            "set",
            "add",
            set["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &add,
        ],
    )["result"]
        .clone();
    assert_eq!(member["set"]["active_count"], 1);
    let repeat = json_arg(
        json!({"item_id":item_ids[0],"item_revision":2,"reason":"wrong again","request_id":"mistake-repeat"}),
    );
    let repeated = ok(
        world.path(),
        home.path(),
        &[
            "set",
            "add",
            set["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &repeat,
        ],
    )["result"]
        .clone();
    assert_eq!(repeated["set"]["active_count"], 1);
    assert_eq!(repeated["member"]["event_count"], 2);
    let resolve = json_arg(json!({"reason":"mastered","request_id":"mistake-resolve"}));
    let resolved = ok(
        world.path(),
        home.path(),
        &[
            "set",
            "resolve",
            set["id"].as_str().unwrap(),
            item_ids[0].as_str(),
            "--if-revision",
            "3",
            "--json",
            &resolve,
        ],
    )["result"]
        .clone();
    assert_eq!(resolved["member"]["state"], "resolved");

    for (n, item_id) in item_ids.iter().enumerate() {
        let review = json_arg(json!({
            "rating": 3,
            "reviewed_at": "2026-08-01T00:00:00Z",
            "estimated_minutes": 4,
            "request_id": format!("review-rate-{n}")
        }));
        let state = ok(
            world.path(),
            home.path(),
            &["review", "rate", item_id, "--json", &review],
        )["result"]["card"]
            .clone();
        assert_eq!(state["scheduler"]["crate"], "rs-fsrs");
        assert_eq!(state["scheduler"]["version"], "1.2.1");
        assert!(state["due_at"].as_str().unwrap() >= "2026-08-01T00:00:00Z");
    }

    let goal_bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:review-goal",
        "review-goal-bank",
    );
    let goal_item = ok(
        world.path(),
        home.path(),
        &["item", "show", item_ids[1].as_str()],
    )["result"]["item"]
        .clone();
    let goal_bank = add_to_bank(
        world.path(),
        home.path(),
        &goal_bank,
        &goal_item,
        "review-goal-add",
    );

    let queue = ok(
        world.path(),
        home.path(),
        &[
            "review",
            "queue",
            "--subject",
            &subject_id,
            "--goal-bank",
            goal_bank["id"].as_str().unwrap(),
            "--budget-minutes",
            "4",
            "--now",
            "2030-01-01T00:00:00Z",
        ],
    )["result"]
        .clone();
    assert_eq!(queue["selected_minutes"], 4);
    assert_eq!(queue["selected"].as_array().unwrap().len(), 1);
    assert_eq!(queue["selected"][0]["item_id"], item_ids[1]);
    assert_eq!(queue["selected"][0]["goal_value"], 1.0);
    assert_eq!(queue["debt"]["count"], 1);
    assert_eq!(queue["debt"]["minutes"], 4);
    let database = home.path().join(".lwc/plugins/practice/data.sqlite3");
    let connection = rusqlite::Connection::open(&database).unwrap();
    let revision_before: String = connection
        .query_row(
            "SELECT value FROM plugin_meta WHERE key='revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let debt_events_before: i64 = connection
        .query_row("SELECT COUNT(*) FROM review_debt_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    drop(connection);
    let repeated = ok(
        world.path(),
        home.path(),
        &[
            "review",
            "queue",
            "--subject",
            &subject_id,
            "--goal-bank",
            goal_bank["id"].as_str().unwrap(),
            "--budget-minutes",
            "4",
            "--now",
            "2030-01-01T00:00:00Z",
        ],
    )["result"]
        .clone();
    assert_eq!(repeated, queue);
    let connection = rusqlite::Connection::open(database).unwrap();
    let revision_after: String = connection
        .query_row(
            "SELECT value FROM plugin_meta WHERE key='revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let debt_events_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM review_debt_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revision_after, revision_before);
    assert_eq!(debt_events_after, debt_events_before);
}

#[test]
fn subjective_grades_require_frozen_rubric_and_low_confidence_review() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let input = json!({
        "subject_id":subject_id,"item_type":"text","grading_kind":"subjective",
        "prompt":"解释复式记账","answer":null,"rubric":"同时说明借贷对应与恒等关系",
        "max_points":10,"estimated_minutes":5,"difficulty":0.7,
        "source":{"kind":"tutor_turn","id":TUTOR_TURN_ID,"revision_or_hash":"1","subject_id":subject_id},
        "request_id":"essay-item"
    });
    let item = create_item(world.path(), home.path(), &input);
    let item = verify_item(world.path(), home.path(), &item, &input, "essay-verify");
    let bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:default",
        "essay-bank",
    );
    let bank = add_to_bank(world.path(), home.path(), &bank, &item, "essay-bank-add");
    let paper_input = json_arg(
        json!({"bank_id":bank["id"],"count":1,"duration_minutes":10,"request_id":"essay-paper"}),
    );
    let paper = ok(
        world.path(),
        home.path(),
        &["paper", "create", "--json", &paper_input],
    )["result"]["paper"]
        .clone();
    let attempt_input = json_arg(
        json!({"paper_id":paper["id"],"owner":"essay-owner","request_id":"essay-attempt"}),
    );
    let attempt = ok(
        world.path(),
        home.path(),
        &["attempt", "create", "--json", &attempt_input],
    )["result"]["attempt"]
        .clone();
    let response_input = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"text","value":"借贷对应并保持会计恒等式","request_id":"essay-response"}),
    );
    let response = ok(
        world.path(),
        home.path(),
        &[
            "response",
            "save",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--owner",
            "essay-owner",
            "--json",
            &response_input,
        ],
    )["result"]["response"]
        .clone();
    let submit = json_arg(json!({"owner":"essay-owner","request_id":"essay-submit"}));
    ok(
        world.path(),
        home.path(),
        &[
            "attempt",
            "submit",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &submit,
        ],
    );
    let grade = json_arg(json!({
        "item_revision": item["revision"], "response_revision": 1,
        "score": 7.5, "rationale":"提到对应关系，但恒等关系解释不完整", "confidence":0.4,
        "method":"agent_rubric", "request_id":"essay-grade"
    }));
    let stale_grade = json_arg(
        json!({"item_revision":2,"response_revision":2,"score":7.5,"rationale":"stale","confidence":0.4,"method":"agent_rubric","request_id":"essay-grade-stale"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "grade",
                "subjective",
                response["id"].as_str().unwrap(),
                "--json",
                &stale_grade
            ]
        )["error"]["code"],
        "stale_response_revision"
    );
    let result = ok(
        world.path(),
        home.path(),
        &[
            "grade",
            "subjective",
            response["id"].as_str().unwrap(),
            "--json",
            &grade,
        ],
    )["result"]["grade"]
        .clone();
    assert_eq!(result["state"], "pending_review");
    let forbidden_override = json_arg(
        json!({"score":8.0,"reason":"agent cannot decide","actor":"agent","request_id":"essay-override-agent"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "grade",
                "override",
                result["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--json",
                &forbidden_override,
            ],
        )["error"]["code"],
        "learner_control_required"
    );
    let override_input = json_arg(
        json!({"score":8.0,"reason":"学习者确认应计入恒等关系表述","actor":"learner","request_id":"essay-override"}),
    );
    let overridden = ok(
        world.path(),
        home.path(),
        &[
            "grade",
            "override",
            result["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &override_input,
        ],
    )["result"]["grade"]
        .clone();
    assert_eq!(overridden["state"], "overridden");
    assert_eq!(overridden["history"].as_array().unwrap().len(), 2);
}

#[test]
fn target_precedence_is_exact_and_never_falls_back() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let plan = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:plan",
        "target-plan",
    );
    let goal = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:goal",
        "target-goal",
    );
    let default = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:default",
        "target-default",
    );

    let exact = json_arg(
        json!({"subject_id":subject_id,"explicit_plan_target":{"kind":"bank","id":plan["id"]},"goal_bank_id":goal["id"],"subject_default_bank_id":default["id"]}),
    );
    let resolved = ok(world.path(), home.path(), &["next", "--json", &exact])["result"].clone();
    assert_eq!(resolved["resolved_from"], "plan");
    assert_eq!(resolved["target"]["id"], plan["id"]);

    let missing = json_arg(
        json!({"subject_id":subject_id,"explicit_plan_target":{"kind":"bank","id":"00000000000000000000000000000000"},"goal_bank_id":goal["id"],"subject_default_bank_id":default["id"]}),
    );
    assert_eq!(
        err(world.path(), home.path(), &["next", "--json", &missing])["error"]["code"],
        "target_not_found"
    );
    let wrong_kind = json_arg(
        json!({"subject_id":subject_id,"explicit_plan_target":{"kind":"title","id":plan["id"]},"goal_bank_id":goal["id"],"subject_default_bank_id":default["id"]}),
    );
    assert_eq!(
        err(world.path(), home.path(), &["next", "--json", &wrong_kind])["error"]["code"],
        "invalid_target_kind"
    );
    let fuzzy = json_arg(
        json!({"subject_id":subject_id,"title":"精准题库","subject_default_bank_id":default["id"]}),
    );
    assert_eq!(
        err(world.path(), home.path(), &["next", "--json", &fuzzy])["error"]["code"],
        "invalid_input"
    );
}

#[test]
fn set_reopen_archive_and_history_are_independent_gates() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let input = item_input(&subject_id, "set-history-item", "集合历史题", json!("A"));
    let item = create_item(world.path(), home.path(), &input);
    let item = verify_item(
        world.path(),
        home.path(),
        &item,
        &input,
        "set-history-verify",
    );
    let set_input = json_arg(
        json!({"subject_id":subject_id,"name":"复习集","kind":"ordinary","request_id":"history-set"}),
    );
    let set = ok(
        world.path(),
        home.path(),
        &["set", "create", "--json", &set_input],
    )["result"]["set"]
        .clone();
    let add = json_arg(
        json!({"item_id":item["id"],"item_revision":2,"reason":"加入复习","request_id":"history-add"}),
    );
    ok(
        world.path(),
        home.path(),
        &[
            "set",
            "add",
            set["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--json",
            &add,
        ],
    );
    let resolve = json_arg(json!({"reason":"已掌握","request_id":"history-resolve"}));
    ok(
        world.path(),
        home.path(),
        &[
            "set",
            "resolve",
            set["id"].as_str().unwrap(),
            item["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &resolve,
        ],
    );
    let reopen = json_arg(json!({"reason":"再次出错","request_id":"history-reopen"}));
    let reopened = ok(
        world.path(),
        home.path(),
        &[
            "set",
            "reopen",
            set["id"].as_str().unwrap(),
            item["id"].as_str().unwrap(),
            "--if-revision",
            "3",
            "--json",
            &reopen,
        ],
    )["result"]
        .clone();
    assert_eq!(reopened["member"]["state"], "active");
    assert_eq!(reopened["member"]["event_count"], 3);
    let archive = json_arg(json!({"reason":"阶段完成","request_id":"history-archive"}));
    let archived = ok(
        world.path(),
        home.path(),
        &[
            "set",
            "archive",
            set["id"].as_str().unwrap(),
            "--if-revision",
            "4",
            "--json",
            &archive,
        ],
    )["result"]["set"]
        .clone();
    assert_eq!(archived["state"], "archived");
    let archived_reopen =
        json_arg(json!({"reason":"archive is terminal","request_id":"history-archived-reopen"}));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "set",
                "reopen",
                set["id"].as_str().unwrap(),
                item["id"].as_str().unwrap(),
                "--if-revision",
                "5",
                "--json",
                &archived_reopen,
            ],
        )["error"]["code"],
        "set_archived"
    );
    let later = json_arg(
        json!({"item_id":item["id"],"item_revision":2,"reason":"不应加入","request_id":"history-after-archive"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "set",
                "add",
                set["id"].as_str().unwrap(),
                "--if-revision",
                "5",
                "--json",
                &later
            ]
        )["error"]["code"],
        "set_archived"
    );
}

#[test]
fn objective_grade_uses_exact_saved_response_and_attempt_owner_is_cas_protected() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:default",
        "objective-bank",
    );
    let mut input = item_input(&subject_id, "objective-item", "2+2?", json!("4"));
    input["topic"] = json!("arithmetic");
    let item = verify_item(
        world.path(),
        home.path(),
        &create_item(world.path(), home.path(), &input),
        &input,
        "objective-verify",
    );
    bank = add_to_bank(world.path(), home.path(), &bank, &item, "objective-add");
    let wrong_points = json_arg(
        json!({"bank_id":bank["id"],"count":1,"duration_minutes":5,"topic_counts":{"arithmetic":1},"total_points":3.0,"request_id":"objective-paper-wrong-points"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["paper", "create", "--json", &wrong_points],
        )["error"]["code"],
        "paper_shortage"
    );
    let paper_input = json_arg(
        json!({"bank_id":bank["id"],"count":1,"duration_minutes":5,"difficulty_min":0.4,"difficulty_max":0.6,"source_kind":"book","item_type_counts":{"choice":1},"topic_counts":{"arithmetic":1},"total_points":2.0,"request_id":"objective-paper"}),
    );
    let paper = ok(
        world.path(),
        home.path(),
        &["paper", "create", "--json", &paper_input],
    )["result"]["paper"]
        .clone();
    let attempt_input = json_arg(
        json!({"paper_id":paper["id"],"owner":"owner-a","request_id":"objective-attempt"}),
    );
    let attempt = ok(
        world.path(),
        home.path(),
        &["attempt", "create", "--json", &attempt_input],
    )["result"]["attempt"]
        .clone();
    let unsupported = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"audio","value":"4","request_id":"objective-audio"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "response",
                "save",
                attempt["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--owner",
                "owner-a",
                "--json",
                &unsupported
            ]
        )["error"]["code"],
        "unsupported_response_format"
    );
    let stale_owner = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"choice","value":"4","request_id":"objective-stale-owner"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "response",
                "save",
                attempt["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--owner",
                "owner-b",
                "--json",
                &stale_owner
            ]
        )["error"]["code"],
        "stale_owner"
    );
    let response_input = json_arg(
        json!({"paper_item_id":paper["items"][0]["id"],"format":"choice","value":"4","request_id":"objective-response"}),
    );
    let response = ok(
        world.path(),
        home.path(),
        &[
            "response",
            "save",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "1",
            "--owner",
            "owner-a",
            "--json",
            &response_input,
        ],
    )["result"]["response"]
        .clone();
    let grade_input = json_arg(json!({"response_revision":1,"request_id":"objective-grade"}));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "grade",
                "objective",
                response["id"].as_str().unwrap(),
                "--json",
                &grade_input,
            ],
        )["error"]["code"],
        "attempt_not_terminal"
    );
    let submit = json_arg(json!({"owner":"owner-a","request_id":"objective-submit"}));
    ok(
        world.path(),
        home.path(),
        &[
            "attempt",
            "submit",
            attempt["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &submit,
        ],
    );
    let grade = ok(
        world.path(),
        home.path(),
        &[
            "grade",
            "objective",
            response["id"].as_str().unwrap(),
            "--json",
            &grade_input,
        ],
    )["result"]["grade"]
        .clone();
    assert_eq!(grade["score"], 2.0);
    assert_eq!(grade["method"], "deterministic");
    let duplicate =
        json_arg(json!({"response_revision":1,"request_id":"objective-grade-duplicate"}));
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "grade",
                "objective",
                response["id"].as_str().unwrap(),
                "--json",
                &duplicate,
            ],
        )["error"]["code"],
        "grade_already_exists"
    );
    assert_eq!(
        ok(
            world.path(),
            home.path(),
            &["attempt", "show", attempt["id"].as_str().unwrap()]
        )["result"]["attempt"]["responses"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn paper_blueprint_solves_overlapping_constraints_and_freezes_sections() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:blueprint",
        "blueprint-bank",
    );
    for (ordinal, (topic, item_type, difficulty)) in [
        ("assets", "choice", 0.1),
        ("assets", "text", 0.2),
        ("liabilities", "choice", 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = item_input(
            &subject_id,
            &format!("blueprint-item-{ordinal}"),
            item_type,
            json!("A"),
        );
        input["item_type"] = json!(item_type);
        input["topic"] = json!(topic);
        input["difficulty"] = json!(difficulty);
        let item = create_item(world.path(), home.path(), &input);
        let item = verify_item(
            world.path(),
            home.path(),
            &item,
            &input,
            &format!("blueprint-verify-{ordinal}"),
        );
        bank = add_to_bank(
            world.path(),
            home.path(),
            &bank,
            &item,
            &format!("blueprint-add-{ordinal}"),
        );
    }
    let blueprint = json_arg(json!({
        "bank_id":bank["id"], "count":2, "duration_minutes":10,
        "item_type_counts":{"choice":1,"text":1},
        "topic_counts":{"assets":1,"liabilities":1},
        "section_counts":{"calculation":1,"concepts":1},
        "total_points":4.0, "request_id":"blueprint-overlap"
    }));
    let paper = ok(
        world.path(),
        home.path(),
        &["paper", "create", "--json", &blueprint],
    )["result"]["paper"]
        .clone();
    assert_eq!(paper["items"].as_array().unwrap().len(), 2);
    let mut sections = paper["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["section"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    sections.sort();
    assert_eq!(sections, ["calculation", "concepts"]);
}

#[test]
fn adversarial_blueprint_stops_at_an_explicit_complexity_gate() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut bank = bank(
        world.path(),
        home.path(),
        &subject_id,
        "subject:complexity",
        "complexity-bank",
    );
    for ordinal in 0..20 {
        let (topic, item_type) = if ordinal < 10 {
            ("assets", "choice")
        } else {
            ("liabilities", "text")
        };
        let mut input = item_input(
            &subject_id,
            &format!("complexity-item-{ordinal}"),
            &format!("complexity-{ordinal}"),
            json!("A"),
        );
        input["item_type"] = json!(item_type);
        input["topic"] = json!(topic);
        let item = create_item(world.path(), home.path(), &input);
        let item = verify_item(
            world.path(),
            home.path(),
            &item,
            &input,
            &format!("complexity-verify-{ordinal}"),
        );
        bank = add_to_bank(
            world.path(),
            home.path(),
            &bank,
            &item,
            &format!("complexity-add-{ordinal}"),
        );
    }
    let blueprint = json_arg(json!({
        "bank_id":bank["id"], "count":10, "duration_minutes":100,
        "topic_counts":{"assets":5,"liabilities":5},
        "item_type_counts":{"choice":6,"text":4},
        "request_id":"complexity-paper"
    }));
    let started = std::time::Instant::now();
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["paper", "create", "--json", &blueprint],
        )["error"]["code"],
        "blueprint_too_complex"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(
        ok(world.path(), home.path(), &["status"])["result"]["papers"],
        0
    );
}

#[test]
fn official_fsrs_rating_vector_is_exact() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let mut input = item_input(&subject_id, "fsrs-vector-item", "FSRS", json!("A"));
    input["item_type"] = json!("flashcard");
    let item = create_item(world.path(), home.path(), &input);
    let item = verify_item(
        world.path(),
        home.path(),
        &item,
        &input,
        "fsrs-vector-verify",
    );
    let ratings = [3, 3, 3, 3, 3, 3, 1, 1, 3, 3, 3, 3, 3];
    let expected = [0, 4, 15, 48, 136, 351, 0, 0, 7, 13, 24, 43, 77];
    let mut reviewed_at = "2022-11-29T12:30:00Z".to_owned();
    let mut last_reviewed_at = reviewed_at.clone();
    let mut actual = Vec::new();
    for (ordinal, rating) in ratings.into_iter().enumerate() {
        let review = json_arg(
            json!({"rating":rating,"reviewed_at":reviewed_at,"estimated_minutes":1,"request_id":format!("fsrs-vector-{ordinal}")}),
        );
        let card = ok(
            world.path(),
            home.path(),
            &[
                "review",
                "rate",
                item["id"].as_str().unwrap(),
                "--json",
                &review,
            ],
        )["result"]["card"]
            .clone();
        actual.push(card["events"][ordinal]["scheduled_days"].as_i64().unwrap());
        last_reviewed_at = reviewed_at;
        let due = DateTime::parse_from_rfc3339(card["due_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
        let previous = DateTime::parse_from_rfc3339(&last_reviewed_at)
            .unwrap()
            .with_timezone(&Utc);
        reviewed_at = if due <= previous {
            (previous + Duration::seconds(1)).to_rfc3339()
        } else {
            due.to_rfc3339()
        };
    }
    assert_eq!(actual, expected);
    let backwards = json_arg(
        json!({"rating":3,"reviewed_at":last_reviewed_at,"estimated_minutes":1,"request_id":"fsrs-backwards"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "review",
                "rate",
                item["id"].as_str().unwrap(),
                "--json",
                &backwards,
            ],
        )["error"]["code"],
        "non_monotonic_review_time"
    );
}

#[test]
fn learner_owned_review_controls_cap_queue_and_due_is_not_writable() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    let agent = json_arg(
        json!({"subject_id":subject_id,"daily_budget_minutes":4,"desired_retention":0.9,"actor":"agent","request_id":"control-agent"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &["review", "configure", "--json", &agent]
        )["error"]["code"],
        "learner_control_required"
    );
    let learner = json_arg(
        json!({"subject_id":subject_id,"daily_budget_minutes":4,"desired_retention":0.85,"actor":"learner","request_id":"control-learner"}),
    );
    let control = ok(
        world.path(),
        home.path(),
        &["review", "configure", "--json", &learner],
    )["result"]["control"]
        .clone();
    assert_eq!(control["daily_budget_minutes"], 4);
    assert_eq!(control["desired_retention"], 0.85);
    let choice_input = item_input(&subject_id, "control-choice", "choice", json!("A"));
    let choice = create_item(world.path(), home.path(), &choice_input);
    let review = json_arg(
        json!({"rating":3,"reviewed_at":"2026-01-01T00:00:00Z","estimated_minutes":1,"request_id":"control-rate"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "review",
                "rate",
                choice["id"].as_str().unwrap(),
                "--json",
                &review,
            ],
        )["error"]["code"],
        "review_requires_verified_flashcard"
    );
    let mut card_input = item_input(&subject_id, "control-item", "controlled", json!(3));
    card_input["item_type"] = json!("flashcard");
    let card_item = create_item(world.path(), home.path(), &card_input);
    let card_item = verify_item(
        world.path(),
        home.path(),
        &card_item,
        &card_input,
        "control-item-verify",
    );
    let review = json_arg(
        json!({"rating":3,"reviewed_at":"2026-01-01T08:00:00+08:00","estimated_minutes":1,"request_id":"control-rate-card"}),
    );
    let card = ok(
        world.path(),
        home.path(),
        &[
            "review",
            "rate",
            card_item["id"].as_str().unwrap(),
            "--json",
            &review,
        ],
    )["result"]["card"]
        .clone();
    assert_eq!(card["scheduler"]["parameters"]["request_retention"], 0.85);
    assert!(card["scheduler"]["parameters"]["weights"].is_array());
    assert_eq!(card["events"][0]["reviewed_at"], "2026-01-01T00:00:00Z");
    let retire = json_arg(json!({"reason":"retired card","request_id":"control-card-retire"}));
    ok(
        world.path(),
        home.path(),
        &[
            "item",
            "retire",
            card_item["id"].as_str().unwrap(),
            "--if-revision",
            "2",
            "--json",
            &retire,
        ],
    );
    let retired_queue = ok(
        world.path(),
        home.path(),
        &[
            "review",
            "queue",
            "--subject",
            &subject_id,
            "--budget-minutes",
            "4",
            "--now",
            "2030-01-01T00:00:00Z",
        ],
    )["result"]
        .clone();
    assert!(retired_queue["selected"].as_array().unwrap().is_empty());
    assert!(
        retired_queue["debt"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let due_edit = json_arg(
        json!({"rating":3,"reviewed_at":"2026-01-01T00:00:00Z","estimated_minutes":1,"due_at":"2026-01-02T00:00:00Z","request_id":"due-edit"}),
    );
    assert_eq!(
        err(
            world.path(),
            home.path(),
            &[
                "review",
                "rate",
                "00000000000000000000000000000000",
                "--json",
                &due_edit
            ]
        )["error"]["code"],
        "invalid_input"
    );
}

#[test]
fn all_four_response_forms_are_accepted_and_no_fifth_form_exists() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let subject_id = subject(world.path(), home.path());
    for (ordinal, (format, answer, value)) in [
        ("choice", json!("A"), json!("A")),
        ("text", json!("exact"), json!("exact")),
        ("numeric", json!(42), json!(42)),
        ("flashcard", json!(3), json!(3)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut item_spec =
            item_input(&subject_id, &format!("form-item-{ordinal}"), format, answer);
        item_spec["item_type"] = json!(format);
        let item = create_item(world.path(), home.path(), &item_spec);
        let item = verify_item(
            world.path(),
            home.path(),
            &item,
            &item_spec,
            &format!("form-verify-{ordinal}"),
        );
        let mut bank = bank(
            world.path(),
            home.path(),
            &subject_id,
            &format!("subject:form-{ordinal}"),
            &format!("form-bank-{ordinal}"),
        );
        bank = add_to_bank(
            world.path(),
            home.path(),
            &bank,
            &item,
            &format!("form-add-{ordinal}"),
        );
        let paper_input = json_arg(
            json!({"bank_id":bank["id"],"count":1,"duration_minutes":5,"request_id":format!("form-paper-{ordinal}")}),
        );
        let paper = ok(
            world.path(),
            home.path(),
            &["paper", "create", "--json", &paper_input],
        )["result"]["paper"]
            .clone();
        let attempt_input = json_arg(
            json!({"paper_id":paper["id"],"owner":"forms","request_id":format!("form-attempt-{ordinal}")}),
        );
        let attempt = ok(
            world.path(),
            home.path(),
            &["attempt", "create", "--json", &attempt_input],
        )["result"]["attempt"]
            .clone();
        let response_input = json_arg(
            json!({"paper_item_id":paper["items"][0]["id"],"format":format,"value":value,"request_id":format!("form-response-{ordinal}")}),
        );
        let response = ok(
            world.path(),
            home.path(),
            &[
                "response",
                "save",
                attempt["id"].as_str().unwrap(),
                "--if-revision",
                "1",
                "--owner",
                "forms",
                "--json",
                &response_input,
            ],
        )["result"]["response"]
            .clone();
        assert_eq!(response["format"], format);
        if format == "numeric" || format == "flashcard" {
            let invalid_value = if format == "numeric" {
                json!("42")
            } else {
                json!(5)
            };
            let invalid = json_arg(
                json!({"paper_item_id":paper["items"][0]["id"],"format":format,"value":invalid_value,"request_id":format!("form-invalid-{ordinal}")}),
            );
            assert_eq!(
                err(
                    world.path(),
                    home.path(),
                    &[
                        "response",
                        "save",
                        attempt["id"].as_str().unwrap(),
                        "--if-revision",
                        "2",
                        "--owner",
                        "forms",
                        "--json",
                        &invalid,
                    ],
                )["error"]["code"],
                "invalid_response_value"
            );
        }
    }
}

#[test]
fn practice_schema_corruption_fails_closed_instead_of_being_masked() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    ok(world.path(), home.path(), &["status"]);
    let database = home.path().join(".lwc/plugins/practice/data.sqlite3");
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute_batch(
            "DROP TABLE review_debt; CREATE TABLE review_debt(item_id TEXT PRIMARY KEY);",
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        err(world.path(), home.path(), &["status"])["error"]["code"],
        "practice_schema_invalid"
    );
}

#[test]
fn critical_practice_queries_use_indexes() {
    let world = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    ok(world.path(), home.path(), &["status"]);
    let database = home.path().join(".lwc/plugins/practice/data.sqlite3");
    let connection = rusqlite::Connection::open(database).unwrap();
    for (sql, expected) in [
        (
            "EXPLAIN QUERY PLAN SELECT d.item_id,d.due_at,d.estimated_minutes,c.card_json,CASE WHEN 'goal' IS NOT NULL AND EXISTS(SELECT 1 FROM bank_items goal WHERE goal.bank_id='goal' AND goal.item_id=i.id AND goal.item_revision=i.revision) THEN 1.0 ELSE 0.0 END FROM review_debt d JOIN fsrs_cards c ON c.item_id=d.item_id JOIN practice_items i ON i.id=d.item_id AND i.revision=(SELECT MAX(revision) FROM practice_items WHERE id=d.item_id) WHERE d.subject_id='s' AND d.state='open' AND i.state='verified' AND i.item_type='flashcard'",
            "review_debt_subject",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT i.id FROM bank_items b JOIN practice_items i ON i.id=b.item_id AND i.revision=b.item_revision WHERE b.bank_id='b' AND i.state='verified'",
            "sqlite_autoindex_bank_items_1",
        ),
        (
            "EXPLAIN QUERY PLAN SELECT id FROM attempts WHERE paper_id='p' AND state='in_progress'",
            "attempts_paper_state",
        ),
    ] {
        let details = connection
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n");
        assert!(
            details.contains(expected),
            "expected {expected} in {details}"
        );
        assert!(
            !details.lines().any(|line| line.starts_with("SCAN ")),
            "unexpected full scan: {details}"
        );
    }
}
