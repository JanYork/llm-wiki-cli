use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn output(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lwc-tutor"))
        .current_dir(cwd)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap()
}

fn ok(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = output(cwd, home, args);
    assert!(
        output.status.success(),
        "{args:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn err(cwd: &Path, home: &Path, args: &[&str]) -> Value {
    let output = output(cwd, home, args);
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    serde_json::from_slice(&output.stderr).unwrap()
}

fn subject(cwd: &Path, home: &Path) -> String {
    let input = serde_json::json!({"name":"线性代数","request_id":"subject-linear"}).to_string();
    ok(cwd, home, &["subject", "create", "--json", &input])["result"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn named_subject(cwd: &Path, home: &Path, name: &str, request_id: &str) -> String {
    let input = serde_json::json!({"name":name,"request_id":request_id}).to_string();
    ok(cwd, home, &["subject", "create", "--json", &input])["result"]["subject"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn status_returns_complete_bounded_resume_context() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    let subject_id = named_subject(&cwd, &home, "英语", "resume-subject");
    let goal_id = goal(&cwd, &home, &subject_id, "resume-goal");
    let plan_input = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "agent-led",
        "deadline": "2099-12-31T23:59:59+08:00",
        "weekly_minutes": 60,
        "core_content": ["日常表达"],
        "order": ["先理解，再表达"],
        "pace": "由学习者主动推进",
        "method": "直接讲解后做迁移练习",
        "exercise_ratio": 0.3,
        "request_id": "resume-plan"
    })
    .to_string();
    let plan_id =
        ok(&cwd, &home, &["plan", "create", "--json", &plan_input])["result"]["plan"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
    let session_id = session(&cwd, &home, &subject_id, "learning", "resume-session");

    let committed_id = begin_turn(
        &cwd,
        &home,
        &session_id,
        "为什么英语疑问句需要助动词？",
        "resume-turn-committed",
    );
    let commit_input = serde_json::json!({
        "owner": "agent-local",
        "reply": "助动词承担时态和疑问结构，实义动词因此保持原形。",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "尚未区分助动词与实义动词的职责",
            "hint_level": 0,
            "learner_attempted": false,
            "explicit_answer_request": true,
            "full_answer": true,
            "feedback_evidence_refs": [],
            "anchor": {
                "current_node": "一般疑问句中的助动词",
                "mastered_nodes": ["陈述句基本语序"],
                "current_mode": "learning",
                "clearance_status": "未达标",
                "next_action": "用一个反例区分助动词与实义动词"
            }
        },
        "request_id": "resume-turn-committed-commit"
    })
    .to_string();
    ok(
        &cwd,
        &home,
        &[
            "turn",
            "commit",
            &committed_id,
            "--if-revision",
            "1",
            "--json",
            &commit_input,
        ],
    );
    let pending_id = begin_turn(
        &cwd,
        &home,
        &session_id,
        "继续讲一般疑问句。",
        "resume-turn-pending",
    );
    let _unplanned_goal_id = goal(&cwd, &home, &subject_id, "resume-goal-unplanned");

    let status = ok(&cwd, &home, &["status"]);
    let result = &status["result"];
    assert_eq!(result["active_sessions"], 1);
    assert_eq!(result["pending_turns"], 1);
    assert_eq!(result["resume_contexts_truncated"], false);
    assert!(
        result["soul"]["body"]
            .as_str()
            .unwrap()
            .contains("老师的灵魂")
    );

    let contexts = result["resume_contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context["session"]["id"], session_id);
    assert_eq!(context["subject"]["id"], subject_id);
    assert_eq!(context["goal"]["id"], goal_id);
    assert_eq!(context["plan"]["id"], plan_id);
    assert_eq!(context["latest_committed_turn"]["id"], committed_id);
    assert_eq!(
        context["latest_committed_turn"]["checkpoint"]["anchor"]["current_node"],
        "一般疑问句中的助动词"
    );
    assert_eq!(context["pending_turn_count"], 1);
    assert_eq!(context["pending_turns_truncated"], false);
    assert_eq!(context["pending_turns"][0]["id"], pending_id);
    assert_eq!(context["pending_turns"][0]["input"], "继续讲一般疑问句。");
    assert!(!status.to_string().contains("system_prompt"));
    assert!(!status.to_string().contains("chain_of_thought"));

    for ordinal in 0..20 {
        session(
            &cwd,
            &home,
            &subject_id,
            "learning",
            &format!("resume-session-{ordinal}"),
        );
    }
    let bounded = ok(&cwd, &home, &["status"]);
    assert_eq!(bounded["result"]["active_sessions"], 21);
    assert_eq!(
        bounded["result"]["resume_contexts"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
    assert_eq!(bounded["result"]["resume_contexts_truncated"], true);
}

#[test]
fn visible_turn_input_is_durable_before_reply_and_commit_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let session_input = serde_json::json!({
        "subject_id": subject_id,
        "mode": "learning",
        "request_id": "session-linear-1"
    })
    .to_string();
    let session = ok(
        &cwd,
        &home,
        &["session", "create", "--json", &session_input],
    );
    let session_id = session["result"]["session"]["id"].as_str().unwrap();

    let begin_input = serde_json::json!({
        "session_id": session_id,
        "owner": "agent-local",
        "input": "我不理解为什么线性无关与唯一表示有关。",
        "request_id": "turn-linear-1"
    })
    .to_string();
    let begun = ok(&cwd, &home, &["turn", "begin", "--json", &begin_input]);
    assert_eq!(begun["result"]["turn"]["state"], "pending");
    assert_eq!(begun["result"]["turn"]["revision"], 1);
    assert_eq!(
        begun["result"]["turn"]["input"],
        "我不理解为什么线性无关与唯一表示有关。"
    );
    assert!(begun["result"]["turn"]["reply"].is_null());
    let turn_id = begun["result"]["turn"]["id"].as_str().unwrap();

    let database = home.join(".lwc/plugins/tutor/data.sqlite3");
    let connection = Connection::open(&database).unwrap();
    let durable: (String, Option<String>, String) = connection
        .query_row(
            "SELECT input,reply,state FROM tutor_turns WHERE id=?1",
            [turn_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        durable,
        (
            "我不理解为什么线性无关与唯一表示有关。".to_owned(),
            None,
            "pending".to_owned()
        )
    );

    let pending = ok(&cwd, &home, &["turn", "pending", "--session", session_id]);
    assert_eq!(pending["result"]["turns"][0]["id"], turn_id);
    assert!(!pending.to_string().contains("system_prompt"));
    assert!(!pending.to_string().contains("chain_of_thought"));

    let commit_input = serde_json::json!({
        "owner": "agent-local",
        "reply": "关键在于：若同一向量有两种表示，相减就得到一组非零系数的零向量组合。",
        "checkpoint": {
            "kind": "diagnosis",
            "blocked_by": "尚未把表示唯一性转写为齐次方程只有零解",
            "hint_level": 1
        },
        "request_id": "turn-linear-1-commit"
    })
    .to_string();
    let stale_owner = serde_json::json!({
        "owner": "agent-other-machine",
        "reply": "不应写入",
        "checkpoint": {
            "kind": "diagnosis",
            "blocked_by": "不应写入",
            "hint_level": 1
        },
        "request_id": "turn-linear-stale-owner"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "turn",
                "commit",
                turn_id,
                "--if-revision",
                "1",
                "--json",
                &stale_owner,
            ],
        )["error"]["code"],
        "stale_owner"
    );
    let args = [
        "turn",
        "commit",
        turn_id,
        "--if-revision",
        "1",
        "--json",
        commit_input.as_str(),
    ];
    let committed = ok(&cwd, &home, &args);
    assert_eq!(committed["result"]["turn"]["state"], "committed");
    assert_eq!(committed["result"]["turn"]["revision"], 2);
    assert_eq!(
        committed["result"]["turn"]["reply"],
        "关键在于：若同一向量有两种表示，相减就得到一组非零系数的零向量组合。"
    );
    assert_eq!(ok(&cwd, &home, &args), committed);
    assert_eq!(
        ok(&cwd, &home, &["turn", "pending", "--session", session_id])["result"]["turns"],
        serde_json::json!([])
    );
}

#[test]
fn pending_turn_takeover_is_a_receipt_gated_explicit_command() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let session_input = serde_json::json!({
        "subject_id":subject_id,"mode":"learning","request_id":"takeover-session"
    })
    .to_string();
    let session = ok(
        &cwd,
        &home,
        &["session", "create", "--json", &session_input],
    );
    let begin = serde_json::json!({
        "session_id":session["result"]["session"]["id"],"owner":"mac-old",
        "input":"durable pending turn","request_id":"takeover-turn"
    })
    .to_string();
    let turn = ok(&cwd, &home, &["turn", "begin", "--json", &begin]);
    let takeover = serde_json::json!({
        "entity_id":turn["result"]["turn"]["id"],"old_owner":"mac-old",
        "new_owner":"mac-new","if_revision":1,"sync_session_id":"sync-tutor-takeover",
        "request_id":"takeover-turn-owner"
    })
    .to_string();
    let output = output(&cwd, &home, &["turn", "takeover", "--json", &takeover]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("\"code\":\"sync_receipt_missing\""));
}

#[test]
fn hidden_turn_content_is_rejected_and_never_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = subject(&cwd, &home);
    let session_input = serde_json::json!({
        "subject_id": subject_id,
        "mode": "question",
        "request_id": "session-hidden"
    })
    .to_string();
    let session = ok(
        &cwd,
        &home,
        &["session", "create", "--json", &session_input],
    );
    let session_id = session["result"]["session"]["id"].as_str().unwrap();
    let rejected = serde_json::json!({
        "session_id": session_id,
        "owner": "agent-local",
        "input": "可见问题",
        "system_prompt": "不得保存",
        "request_id": "turn-hidden"
    })
    .to_string();
    assert_eq!(
        err(&cwd, &home, &["turn", "begin", "--json", &rejected])["error"]["code"],
        "invalid_input"
    );
    let connection = Connection::open(home.join(".lwc/plugins/tutor/data.sqlite3")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM tutor_turns", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn existing_tutor_schema_corruption_fails_closed_instead_of_being_repaired() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    ok(&cwd, &home, &["status"]);
    let database = home.join(".lwc/plugins/tutor/data.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute("DROP INDEX tutor_turns_pending", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        err(&cwd, &home, &["status"])["error"]["code"],
        "corrupt_store"
    );
    let connection = Connection::open(database).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name='tutor_turns_pending'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn soul_is_fully_materialized_versioned_bounded_and_sensitive_changes_need_approval() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();

    let initial = ok(&cwd, &home, &["soul", "show"]);
    assert_eq!(initial["result"]["soul"]["revision"], 1);
    let initial_body = initial["result"]["soul"]["body"].as_str().unwrap();
    assert!(initial_body.contains("客观"));
    assert!(initial_body.contains("禁止谄媚"));
    assert_eq!(
        fs::read_to_string(home.join(".lwc/plugins/tutor/soul.md")).unwrap(),
        initial_body
    );

    let body = "# 老师的灵魂\n\n保持客观、科学、诚实；先定位阻塞，再逐级提示。\n";
    let publish = serde_json::json!({
        "body": body,
        "fact_refs": ["learner-fact-001"],
        "reason": "将已验证的教学原则整理为更短的完整版本",
        "sensitivity": "ordinary",
        "request_id": "soul-publish-2"
    })
    .to_string();
    let published = ok(
        &cwd,
        &home,
        &["soul", "publish", "--if-revision", "1", "--json", &publish],
    );
    assert_eq!(published["result"]["soul"]["revision"], 2);
    assert_eq!(
        fs::read_to_string(home.join(".lwc/plugins/tutor/soul.md")).unwrap(),
        body
    );

    let sensitive = serde_json::json!({
        "body": "# 老师的灵魂\n\n永久假设学生不擅长数学。\n",
        "fact_refs": ["learner-fact-002"],
        "reason": "改变对学生能力的稳定判断",
        "sensitivity": "behavior-changing",
        "approved": false,
        "request_id": "soul-sensitive"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "soul",
                "publish",
                "--if-revision",
                "2",
                "--json",
                &sensitive
            ]
        )["error"]["code"],
        "soul_approval_required"
    );
    assert_eq!(
        fs::read_to_string(home.join(".lwc/plugins/tutor/soul.md")).unwrap(),
        body
    );

    assert_eq!(initial["result"]["soul"]["max_bytes"], 65_536);
    let configure = serde_json::json!({
        "max_bytes": 220_000,
        "request_id": "soul-budget-220k"
    })
    .to_string();
    let configured = ok(
        &cwd,
        &home,
        &[
            "soul",
            "configure",
            "--if-revision",
            "1",
            "--json",
            &configure,
        ],
    );
    assert_eq!(configured["result"]["settings"]["max_bytes"], 220_000);
    assert_eq!(configured["result"]["settings"]["revision"], 2);

    let expanded = serde_json::json!({
        "body": "甲".repeat(70_000),
        "fact_refs": [],
        "reason": "在配置后的完整读取预算内扩展",
        "sensitivity": "ordinary",
        "request_id": "soul-expanded"
    })
    .to_string();
    fs::write(cwd.join("soul-expanded.json"), &expanded).unwrap();
    let expanded = ok(
        &cwd,
        &home,
        &[
            "soul",
            "publish",
            "--if-revision",
            "2",
            "--json",
            "@soul-expanded.json",
        ],
    );
    assert_eq!(expanded["result"]["soul"]["body_bytes"], 210_000);
    assert_eq!(expanded["result"]["soul"]["max_bytes"], 220_000);

    let oversized = serde_json::json!({
        "body": "甲".repeat(90_000),
        "fact_refs": [],
        "reason": "超过配置上限",
        "sensitivity": "ordinary",
        "request_id": "soul-oversized"
    })
    .to_string();
    fs::write(cwd.join("soul-oversized.json"), &oversized).unwrap();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "soul",
                "publish",
                "--if-revision",
                "3",
                "--json",
                "@soul-oversized.json"
            ]
        )["error"]["code"],
        "soul_too_large"
    );
}

#[test]
fn learner_facts_preserve_scope_evidence_precedence_and_history() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let algebra = named_subject(&cwd, &home, "线性代数", "subject-fact-algebra");
    let english = named_subject(&cwd, &home, "英语", "subject-fact-english");

    let provisional_input = serde_json::json!({
        "scope": "subject",
        "subject_id": algebra,
        "claim": "学生在抽象定义后看到具体反例时理解更快",
        "confidence": 0.55,
        "evidence_refs": ["turn-algebra-01"],
        "origin": "agent",
        "request_id": "fact-algebra-example"
    })
    .to_string();
    let provisional = ok(
        &cwd,
        &home,
        &["learner", "fact", "record", "--json", &provisional_input],
    );
    let fact_id = provisional["result"]["fact"]["id"].as_str().unwrap();
    assert_eq!(provisional["result"]["fact"]["status"], "provisional");
    assert_eq!(provisional["result"]["fact"]["scope"], "subject");
    assert_eq!(provisional["result"]["fact"]["subject_id"], algebra);
    assert!(
        provisional["result"]["fact"]
            .get("learning_style")
            .is_none()
    );

    let corroborate = serde_json::json!({
        "action": "corroborate",
        "evidence_refs": ["turn-algebra-02"],
        "confidence": 0.82,
        "request_id": "fact-algebra-corroborate"
    })
    .to_string();
    let confirmed = ok(
        &cwd,
        &home,
        &[
            "learner",
            "fact",
            "revise",
            fact_id,
            "--if-revision",
            "1",
            "--json",
            &corroborate,
        ],
    );
    assert_eq!(confirmed["result"]["fact"]["status"], "confirmed");
    assert_eq!(confirmed["result"]["fact"]["revision"], 2);
    assert_eq!(
        confirmed["result"]["fact"]["evidence_refs"],
        serde_json::json!(["turn-algebra-01", "turn-algebra-02"])
    );

    let other_subject = serde_json::json!({
        "scope": "subject",
        "subject_id": english,
        "claim": "学生在抽象定义后看到具体反例时理解更快",
        "confidence": 0.6,
        "evidence_refs": ["turn-english-01"],
        "origin": "agent",
        "request_id": "fact-english-example"
    })
    .to_string();
    let english_fact = ok(
        &cwd,
        &home,
        &["learner", "fact", "record", "--json", &other_subject],
    );
    assert_ne!(english_fact["result"]["fact"]["id"], fact_id);

    let correction = serde_json::json!({
        "action": "correct",
        "claim": "只有在先给出定义用途后，具体反例才会帮助我理解",
        "evidence_refs": ["learner-correction-01"],
        "confidence": 1.0,
        "origin": "learner",
        "request_id": "fact-algebra-correct"
    })
    .to_string();
    let corrected = ok(
        &cwd,
        &home,
        &[
            "learner",
            "fact",
            "revise",
            fact_id,
            "--if-revision",
            "2",
            "--json",
            &correction,
        ],
    );
    assert_eq!(corrected["result"]["previous"]["status"], "superseded");
    assert_eq!(corrected["result"]["fact"]["status"], "confirmed");
    assert_eq!(corrected["result"]["fact"]["origin"], "learner");
    assert_eq!(corrected["result"]["fact"]["supersedes_id"], fact_id);
    assert_eq!(
        ok(&cwd, &home, &["learner", "fact", "show", fact_id])["result"]["fact"]["status"],
        "superseded"
    );
}

#[test]
fn global_promotion_requires_cross_subject_evidence_and_private_wiki_stays_private() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let accounting = named_subject(&cwd, &home, "会计学", "subject-fact-accounting");
    let english = named_subject(&cwd, &home, "英语", "subject-fact-english-promotion");
    let input = serde_json::json!({
        "scope": "subject",
        "subject_id": accounting,
        "claim": "学生更愿意在工作日早晨完成短练习",
        "confidence": 0.8,
        "evidence_refs": ["turn-accounting-01"],
        "origin": "agent",
        "request_id": "fact-morning"
    })
    .to_string();
    let fact = ok(
        &cwd,
        &home,
        &["learner", "fact", "record", "--json", &input],
    );
    let fact_id = fact["result"]["fact"]["id"].as_str().unwrap();

    let insufficient = serde_json::json!({
        "action": "promote",
        "evidence_refs": ["turn-accounting-02"],
        "corroborating_subject_ids": [accounting],
        "confidence": 0.9,
        "request_id": "fact-morning-promote-one"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "learner",
                "fact",
                "revise",
                fact_id,
                "--if-revision",
                "1",
                "--json",
                &insufficient,
            ],
        )["error"]["code"],
        "cross_subject_evidence_required"
    );

    let promote = serde_json::json!({
        "action": "promote",
        "evidence_refs": ["turn-accounting-02", "turn-english-03"],
        "corroborating_subject_ids": [accounting, english],
        "confidence": 0.9,
        "request_id": "fact-morning-promote"
    })
    .to_string();
    let promoted = ok(
        &cwd,
        &home,
        &[
            "learner",
            "fact",
            "revise",
            fact_id,
            "--if-revision",
            "1",
            "--json",
            &promote,
        ],
    );
    assert_eq!(promoted["result"]["fact"]["scope"], "global");
    assert!(promoted["result"]["fact"]["subject_id"].is_null());
    assert_eq!(promoted["result"]["fact"]["status"], "confirmed");

    let private_page = home.join(format!(".lwc/plugins/tutor/wiki/subjects/{accounting}.md"));
    let page = fs::read_to_string(private_page).unwrap();
    assert!(page.contains("学生更愿意在工作日早晨完成短练习"));
    assert!(page.contains("superseded"));
    let learner_page = fs::read_to_string(home.join(".lwc/plugins/tutor/wiki/learner.md")).unwrap();
    assert!(learner_page.contains("学生更愿意在工作日早晨完成短练习"));
    assert!(learner_page.contains("confirmed"));
    assert!(!home.join(".lwc/wiki.db").exists());
}

#[test]
fn goal_needs_complete_criterion_evidence_then_explicit_learner_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = named_subject(&cwd, &home, "线性代数", "subject-goal-linear");

    let no_criteria = serde_json::json!({
        "subject_id": subject_id,
        "statement": "掌握线性无关",
        "criteria": [],
        "request_id": "goal-no-criteria"
    })
    .to_string();
    assert_eq!(
        err(&cwd, &home, &["goal", "create", "--json", &no_criteria])["error"]["code"],
        "goal_criteria_required"
    );

    let create = serde_json::json!({
        "subject_id": subject_id,
        "statement": "能够判断并解释有限向量组是否线性无关",
        "criteria": [
            "能独立判断三组向量并说明依据",
            "能解释线性无关与表示唯一性的关系"
        ],
        "request_id": "goal-linear-independence"
    })
    .to_string();
    let created = ok(&cwd, &home, &["goal", "create", "--json", &create]);
    let goal = &created["result"]["goal"];
    let goal_id = goal["id"].as_str().unwrap();
    let criterion_a = goal["criteria"][0]["id"].as_str().unwrap();
    let criterion_b = goal["criteria"][1]["id"].as_str().unwrap();
    assert_eq!(goal["status"], "active");
    assert_eq!(goal["revision"], 1);

    let premature = serde_json::json!({
        "confirmed_by": "learner",
        "request_id": "goal-premature-complete"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "goal",
                "complete",
                goal_id,
                "--if-revision",
                "1",
                "--json",
                &premature,
            ],
        )["error"]["code"],
        "goal_not_ready"
    );

    let incomplete = serde_json::json!({
        "criteria": [{
            "criterion_id": criterion_a,
            "evidence_refs": ["attempt-vector-set-01"]
        }],
        "request_id": "goal-incomplete-evidence"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "goal",
                "evidence",
                goal_id,
                "--if-revision",
                "1",
                "--json",
                &incomplete,
            ],
        )["error"]["code"],
        "goal_criteria_incomplete"
    );
    assert_eq!(
        ok(&cwd, &home, &["goal", "show", goal_id])["result"]["goal"]["revision"],
        1
    );

    let evidence = serde_json::json!({
        "criteria": [
            {"criterion_id": criterion_a, "evidence_refs": ["attempt-vector-set-01"]},
            {"criterion_id": criterion_b, "evidence_refs": ["turn-unique-representation-04"]}
        ],
        "request_id": "goal-complete-evidence"
    })
    .to_string();
    let ready = ok(
        &cwd,
        &home,
        &[
            "goal",
            "evidence",
            goal_id,
            "--if-revision",
            "1",
            "--json",
            &evidence,
        ],
    );
    assert_eq!(ready["result"]["goal"]["status"], "ready_to_complete");
    assert_eq!(ready["result"]["goal"]["revision"], 2);

    let agent = serde_json::json!({
        "confirmed_by": "agent",
        "request_id": "goal-agent-complete"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "goal",
                "complete",
                goal_id,
                "--if-revision",
                "2",
                "--json",
                &agent,
            ],
        )["error"]["code"],
        "learner_confirmation_required"
    );

    let learner = serde_json::json!({
        "confirmed_by": "learner",
        "request_id": "goal-learner-complete"
    })
    .to_string();
    let completed = ok(
        &cwd,
        &home,
        &[
            "goal",
            "complete",
            goal_id,
            "--if-revision",
            "2",
            "--json",
            &learner,
        ],
    );
    assert_eq!(completed["result"]["goal"]["status"], "completed");
    assert_eq!(completed["result"]["goal"]["revision"], 3);
}

fn goal(cwd: &Path, home: &Path, subject_id: &str, request_id: &str) -> String {
    let input = serde_json::json!({
        "subject_id": subject_id,
        "statement": "在约束时间内完成当前学习目标",
        "criteria": ["完成计划中的必修内容并提供阶段证据"],
        "request_id": request_id
    })
    .to_string();
    ok(cwd, home, &["goal", "create", "--json", &input])["result"]["goal"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn plan_modes_enforce_field_ownership_meaningful_adjustment_and_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = named_subject(&cwd, &home, "英语", "subject-plan-english");
    let goal_id = goal(&cwd, &home, &subject_id, "goal-plan-english");

    let fixed_input = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "fixed",
        "deadline": "2026-12-31T23:59:59+08:00",
        "weekly_minutes": 240,
        "core_content": ["听力", "阅读"],
        "order": ["听力", "阅读"],
        "pace": "每周四次，每次一小时",
        "method": "精听与分级阅读",
        "exercise_ratio": 0.4,
        "request_id": "plan-fixed-english"
    })
    .to_string();
    let fixed = ok(&cwd, &home, &["plan", "create", "--json", &fixed_input]);
    let fixed_id = fixed["result"]["plan"]["id"].as_str().unwrap();

    let agent_execution_change = serde_json::json!({
        "actor": "agent",
        "trigger": "meaningful_checkpoint",
        "reason": "连续三次阶段检查显示听力需要提前",
        "evidence_refs": ["checkpoint-listening-03"],
        "order": ["阅读", "听力"],
        "request_id": "plan-fixed-agent-change"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "plan",
                "revise",
                fixed_id,
                "--if-revision",
                "1",
                "--json",
                &agent_execution_change,
            ],
        )["error"]["code"],
        "fixed_plan_requires_learner"
    );

    let learner_change = serde_json::json!({
        "actor": "learner",
        "trigger": "learner_request",
        "reason": "我想先集中练阅读",
        "evidence_refs": ["learner-plan-request-01"],
        "order": ["阅读", "听力"],
        "request_id": "plan-fixed-learner-change"
    })
    .to_string();
    let fixed_revised = ok(
        &cwd,
        &home,
        &[
            "plan",
            "revise",
            fixed_id,
            "--if-revision",
            "1",
            "--json",
            &learner_change,
        ],
    );
    assert_eq!(fixed_revised["result"]["plan"]["revision"], 2);
    assert_eq!(
        fixed_revised["result"]["plan"]["order"],
        serde_json::json!(["阅读", "听力"])
    );

    let adaptive_input = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "adaptive",
        "deadline": "2026-12-31T23:59:59+08:00",
        "weekly_minutes": 240,
        "core_content": ["听力", "阅读"],
        "order": ["听力", "阅读"],
        "pace": "每周四次",
        "method": "精听与分级阅读",
        "exercise_ratio": 0.4,
        "request_id": "plan-adaptive-english"
    })
    .to_string();
    let adaptive = ok(&cwd, &home, &["plan", "create", "--json", &adaptive_input]);
    let adaptive_id = adaptive["result"]["plan"]["id"].as_str().unwrap();

    let hard_constraint = serde_json::json!({
        "actor": "agent",
        "trigger": "meaningful_checkpoint",
        "reason": "建议延长期限",
        "evidence_refs": ["checkpoint-pace-03"],
        "deadline": "2027-01-31T23:59:59+08:00",
        "request_id": "plan-agent-deadline"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "plan",
                "revise",
                adaptive_id,
                "--if-revision",
                "1",
                "--json",
                &hard_constraint,
            ],
        )["error"]["code"],
        "learner_owned_field"
    );

    let single_error = serde_json::json!({
        "actor": "agent",
        "trigger": "single_error",
        "reason": "刚才答错一题",
        "evidence_refs": ["response-one-error"],
        "pace": "减半",
        "request_id": "plan-single-error"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "plan",
                "revise",
                adaptive_id,
                "--if-revision",
                "1",
                "--json",
                &single_error,
            ],
        )["error"]["code"],
        "adjustment_not_meaningful"
    );

    let checkpoint = serde_json::json!({
        "actor": "agent",
        "trigger": "meaningful_checkpoint",
        "reason": "三个阶段检查均显示先精听再练习更有效",
        "evidence_refs": ["checkpoint-01", "checkpoint-02", "checkpoint-03"],
        "pace": "每周三次",
        "method": "先精听再即时复述",
        "exercise_ratio": 0.5,
        "request_id": "plan-checkpoint-adjust"
    })
    .to_string();
    let revised = ok(
        &cwd,
        &home,
        &[
            "plan",
            "revise",
            adaptive_id,
            "--if-revision",
            "1",
            "--json",
            &checkpoint,
        ],
    );
    assert_eq!(revised["result"]["plan"]["revision"], 2);
    assert_eq!(revised["result"]["plan"]["weekly_minutes"], 240);
    assert_eq!(revised["result"]["diff"]["pace"]["before"], "每周四次");
    assert_eq!(revised["result"]["diff"]["pace"]["after"], "每周三次");

    let rollback = serde_json::json!({
        "actor": "agent",
        "reason": "调整未改善阶段表现，恢复上一版本",
        "evidence_refs": ["checkpoint-after-adjustment-02"],
        "request_id": "plan-rollback-v1"
    })
    .to_string();
    let rolled_back = ok(
        &cwd,
        &home,
        &[
            "plan",
            "rollback",
            adaptive_id,
            "--if-revision",
            "2",
            "--to-revision",
            "1",
            "--json",
            &rollback,
        ],
    );
    assert_eq!(rolled_back["result"]["plan"]["revision"], 3);
    assert_eq!(rolled_back["result"]["plan"]["pace"], "每周四次");
    assert_eq!(rolled_back["result"]["rolled_back_to"], 1);
}

fn session(cwd: &Path, home: &Path, subject_id: &str, mode: &str, request_id: &str) -> String {
    let input = serde_json::json!({
        "subject_id": subject_id,
        "mode": mode,
        "request_id": request_id
    })
    .to_string();
    ok(cwd, home, &["session", "create", "--json", &input])["result"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn begin_turn(cwd: &Path, home: &Path, session_id: &str, input: &str, request_id: &str) -> String {
    let input = serde_json::json!({
        "session_id": session_id,
        "owner": "agent-local",
        "input": input,
        "request_id": request_id
    })
    .to_string();
    ok(cwd, home, &["turn", "begin", "--json", &input])["result"]["turn"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn teaching_checkpoint_enforces_blockage_progressive_hints_answer_timing_and_exam_mode() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = named_subject(&cwd, &home, "线性代数", "subject-teaching-linear");
    let learning = session(&cwd, &home, &subject_id, "learning", "session-teaching");
    let first = begin_turn(
        &cwd,
        &home,
        &learning,
        "直接告诉我完整证明",
        "turn-teaching-1",
    );

    let premature_answer = serde_json::json!({
        "owner": "agent-local",
        "reply": "完整答案",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "尚未把唯一表示转成齐次方程",
            "hint_level": 0,
            "learner_attempted": false,
            "explicit_answer_request": false,
            "full_answer": true,
            "feedback_evidence_refs": []
        },
        "request_id": "turn-teaching-1-premature"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "turn",
                "commit",
                &first,
                "--if-revision",
                "1",
                "--json",
                &premature_answer,
            ],
        )["error"]["code"],
        "full_answer_not_allowed"
    );

    let first_hint = serde_json::json!({
        "owner": "agent-local",
        "reply": "先把两种表示相减，会得到什么？",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "尚未把唯一表示转成齐次方程",
            "hint_level": 1,
            "learner_attempted": false,
            "explicit_answer_request": false,
            "full_answer": false,
            "feedback_evidence_refs": []
        },
        "request_id": "turn-teaching-1-hint"
    })
    .to_string();
    ok(
        &cwd,
        &home,
        &[
            "turn",
            "commit",
            &first,
            "--if-revision",
            "1",
            "--json",
            &first_hint,
        ],
    );

    let second = begin_turn(&cwd, &home, &learning, "我还是卡住了", "turn-teaching-2");
    let skipped_hint = serde_json::json!({
        "owner": "agent-local",
        "reply": "三级提示",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "不知道相减后的系数含义",
            "hint_level": 3,
            "learner_attempted": true,
            "explicit_answer_request": false,
            "full_answer": false,
            "feedback_evidence_refs": ["turn-teaching-2"]
        },
        "request_id": "turn-teaching-2-skip"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "turn",
                "commit",
                &second,
                "--if-revision",
                "1",
                "--json",
                &skipped_hint,
            ],
        )["error"]["code"],
        "hint_level_not_progressive"
    );

    let answer_after_attempt = serde_json::json!({
        "owner": "agent-local",
        "reply": "现在给出完整推导。",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "不知道相减后的系数含义",
            "hint_level": 2,
            "learner_attempted": true,
            "explicit_answer_request": false,
            "full_answer": true,
            "feedback_evidence_refs": ["turn-teaching-2"]
        },
        "request_id": "turn-teaching-2-answer"
    })
    .to_string();
    ok(
        &cwd,
        &home,
        &[
            "turn",
            "commit",
            &second,
            "--if-revision",
            "1",
            "--json",
            &answer_after_attempt,
        ],
    );

    let exam = session(&cwd, &home, &subject_id, "exam", "session-exam-no-hints");
    let exam_turn = begin_turn(&cwd, &home, &exam, "给一点提示", "turn-exam-hint");
    let exam_hint = serde_json::json!({
        "owner": "agent-local",
        "reply": "提示",
        "checkpoint": {
            "kind": "teaching",
            "blocked_by": "考试题卡点",
            "hint_level": 1,
            "learner_attempted": true,
            "explicit_answer_request": false,
            "full_answer": false,
            "feedback_evidence_refs": ["turn-exam-hint"]
        },
        "request_id": "turn-exam-hint-commit"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "turn",
                "commit",
                &exam_turn,
                "--if-revision",
                "1",
                "--json",
                &exam_hint,
            ],
        )["error"]["code"],
        "exam_hints_forbidden"
    );
}

#[test]
fn sensitive_soul_proposal_survives_until_learner_approval_and_explicit_publish() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let initial = ok(&cwd, &home, &["soul", "show"]);
    let initial_body = initial["result"]["soul"]["body"]
        .as_str()
        .unwrap()
        .to_owned();

    let proposal_input = serde_json::json!({
        "body": "# 老师的灵魂\n\n在多次独立证据确认前，不对学生形成稳定人格判断。\n",
        "fact_refs": ["fact-sensitive-01", "fact-sensitive-02"],
        "reason": "将人格判断改为严格证据门槛",
        "sensitivity": "behavior-changing",
        "request_id": "soul-proposal-sensitive"
    })
    .to_string();
    let proposed = ok(
        &cwd,
        &home,
        &[
            "soul",
            "propose",
            "--if-revision",
            "1",
            "--json",
            &proposal_input,
        ],
    );
    let proposal_id = proposed["result"]["proposal"]["id"].as_str().unwrap();
    assert_eq!(proposed["result"]["proposal"]["state"], "proposed");
    assert_eq!(
        ok(&cwd, &home, &["soul", "show"])["result"]["soul"]["revision"],
        1
    );
    assert_eq!(
        fs::read_to_string(home.join(".lwc/plugins/tutor/soul.md")).unwrap(),
        initial_body
    );

    let publish_before_approval = serde_json::json!({
        "request_id": "soul-publish-before-approval"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "soul",
                "publish-proposal",
                proposal_id,
                "--if-revision",
                "1",
                "--json",
                &publish_before_approval,
            ],
        )["error"]["code"],
        "soul_approval_required"
    );

    let approval = serde_json::json!({
        "approved_by": "learner",
        "request_id": "soul-approve-sensitive"
    })
    .to_string();
    let approved = ok(
        &cwd,
        &home,
        &[
            "soul",
            "approve",
            proposal_id,
            "--if-revision",
            "1",
            "--json",
            &approval,
        ],
    );
    assert_eq!(approved["result"]["proposal"]["state"], "approved");
    assert_eq!(
        ok(&cwd, &home, &["soul", "show"])["result"]["soul"]["revision"],
        1
    );

    let publish = serde_json::json!({"request_id": "soul-publish-sensitive"}).to_string();
    let published = ok(
        &cwd,
        &home,
        &[
            "soul",
            "publish-proposal",
            proposal_id,
            "--if-revision",
            "1",
            "--json",
            &publish,
        ],
    );
    assert_eq!(published["result"]["soul"]["revision"], 2);
    assert_eq!(published["result"]["proposal"]["state"], "published");
    assert!(
        fs::read_to_string(home.join(".lwc/plugins/tutor/soul.md"))
            .unwrap()
            .contains("稳定人格判断")
    );

    let history = ok(&cwd, &home, &["soul", "history"]);
    assert_eq!(history["result"]["versions"].as_array().unwrap().len(), 2);
    assert_eq!(history["result"]["proposals"][0]["id"], proposal_id);
    assert_eq!(history["result"]["proposals"][0]["state"], "published");
}

#[test]
fn skipped_diagnosis_is_audited_without_changing_goal_or_plan() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = named_subject(&cwd, &home, "会计学", "subject-diagnosis-accounting");
    let goal_id = goal(&cwd, &home, &subject_id, "goal-diagnosis-accounting");
    let plan_input = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "adaptive",
        "deadline": "2026-12-31T23:59:59+08:00",
        "weekly_minutes": 180,
        "core_content": ["复式记账"],
        "order": ["复式记账"],
        "pace": "每周三次",
        "method": "例题驱动",
        "exercise_ratio": 0.5,
        "request_id": "plan-diagnosis-accounting"
    })
    .to_string();
    let plan = ok(&cwd, &home, &["plan", "create", "--json", &plan_input]);
    let plan_id = plan["result"]["plan"]["id"].as_str().unwrap();
    let session_id = session(
        &cwd,
        &home,
        &subject_id,
        "learning",
        "session-diagnosis-accounting",
    );

    let input = serde_json::json!({
        "outcome": "skipped",
        "reason": "学习者已有可靠的近期掌握记录并选择跳过",
        "evidence_refs": ["fact-accounting-baseline-01"],
        "request_id": "diagnosis-skipped-accounting"
    })
    .to_string();
    let result = ok(
        &cwd,
        &home,
        &[
            "session",
            "diagnosis",
            &session_id,
            "--if-revision",
            "1",
            "--json",
            &input,
        ],
    );
    assert_eq!(result["result"]["diagnosis"]["outcome"], "skipped");
    assert_eq!(result["result"]["session"]["revision"], 2);
    assert_eq!(
        ok(&cwd, &home, &["goal", "show", &goal_id])["result"]["goal"]["revision"],
        1
    );
    assert_eq!(
        ok(&cwd, &home, &["plan", "show", plan_id])["result"]["plan"]["revision"],
        1
    );
}

#[test]
fn plan_steps_keep_exact_practice_targets_and_never_rewrite_missed_or_deferred_work() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("cwd");
    let home = temp.path().join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let subject_id = named_subject(&cwd, &home, "英语", "subject-step-english");
    let goal_id = goal(&cwd, &home, &subject_id, "goal-step-english");
    let overflow = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "adaptive",
        "deadline": "2026-12-31T23:59:59+08:00",
        "weekly_minutes": 240,
        "core_content": ["听力"],
        "order": ["听力"],
        "pace": "每周一次",
        "method": "精听",
        "exercise_ratio": 1.0,
        "steps": [{"title": "超出本周上限", "estimated_minutes": 241}],
        "request_id": "plan-step-overflow"
    })
    .to_string();
    assert_eq!(
        err(&cwd, &home, &["plan", "create", "--json", &overflow])["error"]["code"],
        "plan_time_cap_exceeded"
    );
    let input = serde_json::json!({
        "subject_id": subject_id,
        "goal_id": goal_id,
        "mode": "adaptive",
        "deadline": "2026-12-31T23:59:59+08:00",
        "weekly_minutes": 240,
        "core_content": ["听力", "阅读"],
        "order": ["听力", "阅读"],
        "pace": "每周四次",
        "method": "精听与分级阅读",
        "exercise_ratio": 0.5,
        "steps": [
            {
                "title": "完成第一组听力训练",
                "estimated_minutes": 120,
                "practice_target_kind": "bank",
                "practice_target_id": "1234567890abcdef"
            },
            {
                "title": "完成第一篇分级阅读",
                "estimated_minutes": 120
            }
        ],
        "request_id": "plan-step-english"
    })
    .to_string();
    let created = ok(&cwd, &home, &["plan", "create", "--json", &input]);
    let plan = &created["result"]["plan"];
    let plan_id = plan["id"].as_str().unwrap();
    let first = plan["steps"][0]["id"].as_str().unwrap();
    let second = plan["steps"][1]["id"].as_str().unwrap();
    assert_eq!(plan["steps"][0]["practice_target_kind"], "bank");
    assert_eq!(plan["steps"][0]["practice_target_id"], "1234567890abcdef");
    let lower_cap = serde_json::json!({
        "actor": "learner",
        "trigger": "schedule_change",
        "reason": "本周只能安排更少时间",
        "evidence_refs": ["learner-schedule-limit-01"],
        "weekly_minutes": 200,
        "request_id": "plan-step-lower-cap"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "plan",
                "revise",
                plan_id,
                "--if-revision",
                "1",
                "--json",
                &lower_cap,
            ],
        )["error"]["code"],
        "plan_time_cap_exceeded"
    );

    let missed = serde_json::json!({
        "status": "missed",
        "actor": "agent",
        "reason": "学习者本周明确未完成该安排",
        "evidence_refs": ["turn-week-review-01"],
        "request_id": "plan-step-missed"
    })
    .to_string();
    let missed = ok(
        &cwd,
        &home,
        &[
            "plan",
            "step",
            "update",
            plan_id,
            first,
            "--if-revision",
            "1",
            "--json",
            &missed,
        ],
    );
    assert_eq!(missed["result"]["step"]["status"], "missed");
    assert_eq!(missed["result"]["step"]["revision"], 2);

    let falsified = serde_json::json!({
        "status": "completed",
        "actor": "agent",
        "reason": "错误地补记完成",
        "evidence_refs": ["turn-week-review-02"],
        "request_id": "plan-step-false-complete"
    })
    .to_string();
    assert_eq!(
        err(
            &cwd,
            &home,
            &[
                "plan",
                "step",
                "update",
                plan_id,
                first,
                "--if-revision",
                "2",
                "--json",
                &falsified,
            ],
        )["error"]["code"],
        "plan_step_terminal"
    );

    let deferred = serde_json::json!({
        "status": "deferred",
        "actor": "learner",
        "reason": "本周工作冲突，明确延期",
        "evidence_refs": ["learner-schedule-change-01"],
        "request_id": "plan-step-deferred"
    })
    .to_string();
    ok(
        &cwd,
        &home,
        &[
            "plan",
            "step",
            "update",
            plan_id,
            second,
            "--if-revision",
            "1",
            "--json",
            &deferred,
        ],
    );
    let shown = ok(&cwd, &home, &["plan", "show", plan_id]);
    assert_eq!(shown["result"]["plan"]["steps"][0]["status"], "missed");
    assert_eq!(shown["result"]["plan"]["steps"][1]["status"], "deferred");
}
