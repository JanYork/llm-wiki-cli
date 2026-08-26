#!/usr/bin/env python3
import json
import os
import statistics
import subprocess
import tempfile
import time


def run(argv, env):
    started = time.perf_counter()
    result = subprocess.run(argv, env=env, text=True, capture_output=True)
    elapsed_ms = round((time.perf_counter() - started) * 1000, 1)
    if result.returncode:
        raise SystemExit(f"{argv!r} failed\nstdout: {result.stdout}\nstderr: {result.stderr}")
    return elapsed_ms, json.loads(result.stdout) if result.stdout.strip().startswith("{") else result.stdout.strip()


def median(samples):
    return round(statistics.median(samples), 1)


with tempfile.TemporaryDirectory(prefix="lwc-tutor-ux-") as root:
    home = os.path.join(root, "home")
    cwd = os.path.join(root, "no-code")
    os.makedirs(home)
    os.makedirs(cwd)
    env = {**os.environ, "HOME": home}
    os.chdir(cwd)

    process = [run(["lwc", "--version"], env)[0] for _ in range(5)]
    run(["lwc", "--scope", "global", "init"], env)
    run(["lwc", "--scope", "global", "config", "set", "--tutor", "enabled"], env)
    cold_status, _ = run(["lwc", "tutor", "status"], env)

    _, subject = run(
        ["lwc", "tutor", "subject", "create", "--json", json.dumps({"name": "英语", "request_id": "ux-subject"}, ensure_ascii=False)],
        env,
    )
    subject_id = subject["result"]["subject"]["id"]
    _, session = run(
        ["lwc", "tutor", "session", "create", "--json", json.dumps({"subject_id": subject_id, "mode": "learning", "request_id": "ux-session"})],
        env,
    )
    session_id = session["result"]["session"]["id"]
    warm_status = [run(["lwc", "tutor", "status"], env)[0] for _ in range(5)]

    begins = []
    commits = []
    for label, learner_input, reply in (
        ("A", "I am tired.", "对，I am tired 表示“我累了”。下一步：He ___ tired。"),
        ("B", "is", "对，He is tired。he 后面用 is。再换成复数：They ___ tired。"),
    ):
        begin_ms, begun = run(
            ["lwc", "tutor", "turn", "begin", "--json", json.dumps({"session_id": session_id, "owner": "ux-benchmark", "input": learner_input, "request_id": f"ux-turn-{label}"}, ensure_ascii=False)],
            env,
        )
        turn_id = begun["result"]["turn"]["id"]
        turn_revision = begun["result"]["turn"]["revision"]
        begins.append(begin_ms)
        checkpoint = {
            "kind": "teaching",
            "blocked_by": "正在练习 be 动词人称变化",
            "hint_level": 0,
            "learner_attempted": True,
            "explicit_answer_request": False,
            "full_answer": True,
            "feedback_evidence_refs": [],
            "anchor": {
                "current_node": "be 动词人称变化",
                "mastered_nodes": [],
                "current_mode": "learning",
                "clearance_status": "练习中",
                "next_action": "继续单句迁移",
            },
        }
        commit_ms, _ = run(
            ["lwc", "tutor", "turn", "commit", turn_id, "--if-revision", str(turn_revision), "--json", json.dumps({"owner": "ux-benchmark", "reply": reply, "checkpoint": checkpoint, "request_id": f"ux-turn-{label}-commit"}, ensure_ascii=False)],
            env,
        )
        commits.append(commit_ms)

    _, pending = run(["lwc", "tutor", "turn", "pending", "--session", session_id], env)
    pending_count = len(pending["result"]["turns"])
    if pending_count:
        raise SystemExit(f"benchmark left {pending_count} pending turns")
    print(json.dumps({
        "isolated": True,
        "process_ms": {"samples": process, "median": median(process)},
        "status_cold_ms": cold_status,
        "status_warm_ms": {"samples": warm_status, "median": median(warm_status)},
        "begin_ms": {"samples": begins, "median": median(begins)},
        "commit_ms": {"samples": commits, "median": median(commits)},
        "pending": pending_count,
    }, ensure_ascii=False, sort_keys=True))
