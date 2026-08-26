#!/usr/bin/env python3
import json
import re
import sys
from pathlib import Path


def fail(message):
    raise SystemExit(f"learning Agent UX rejected: {message}")


def load(path):
    try:
        events = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    except (OSError, json.JSONDecodeError) as error:
        fail(str(error))
    if not events:
        fail("empty transcript")
    return events


def command_action(event):
    if event.get("action"):
        return event["action"]
    command = " ".join(map(str, event.get("argv", []))).lower()
    for action, pattern in (
        ("status", r"(?:^|\s)lwc\s+tutor\s+status(?:\s|$)"),
        ("subject", r"(?:^|\s)lwc\s+tutor\s+subject\s+create(?:\s|$)"),
        ("session", r"(?:^|\s)lwc\s+tutor\s+session\s+create(?:\s|$)"),
        ("begin", r"(?:^|\s)lwc\s+tutor\s+turn\s+begin(?:\s|$)"),
        ("commit", r"(?:^|\s)lwc\s+tutor\s+turn\s+commit(?:\s|$)"),
    ):
        if re.search(pattern, command):
            return action
    return None


def commands(events, turn, action):
    return [
        event
        for event in events
        if event.get("turn") == turn
        and event.get("kind") == "command"
        and command_action(event) == action
    ]


def require_turn(events, turn, allowed_status_counts):
    turn_events = [event for event in events if event.get("turn") == turn]
    assistant_events = [event for event in turn_events if event.get("kind") == "assistant"]
    finals = [event for event in assistant_events if event.get("phase") in (None, "final", "final_answer")]
    commentaries = [event for event in assistant_events if event.get("phase") == "commentary"]
    timings = [event.get("elapsed_ms") for event in turn_events if event.get("kind") == "agent_timing"]
    states = [event.get("pending") for event in turn_events if event.get("kind") == "state"]
    status = commands(events, turn, "status")
    begin = commands(events, turn, "begin")
    commit = commands(events, turn, "commit")
    if len(finals) != 1 or not finals[0].get("text", "").strip():
        fail(f"turn {turn} must contain exactly one final reply")
    if len(commentaries) > 1 or len(finals) + len(commentaries) != len(assistant_events):
        fail(f"turn {turn} contains unsupported or excessive assistant phases")
    for commentary in commentaries:
        text = commentary.get("text", "").strip()
        if not text or len(text) > 160:
            fail(f"turn {turn} commentary must be one short meaningful update")
        if re.search(
            r"\b(?:lwc|tutor|practice|sqlite|codegraph|status|checkpoint|request_id|if_revision|json|id)\b|检查.{0,8}状态|加载.{0,8}练习|已(?:保存|提交|持久化)",
            text,
            re.I,
        ):
            fail(f"turn {turn} commentary exposed control-plane mechanics")
    if len(timings) != 1 or not isinstance(timings[0], (int, float)) or timings[0] < 0:
        fail(f"turn {turn} must contain one non-negative Agent timing")
    if states != [0]:
        fail(f"turn {turn} must end with pending=0")
    if len(status) not in allowed_status_counts:
        fail(f"turn {turn} status count is {len(status)}, expected one of {sorted(allowed_status_counts)}")
    if len(begin) != 1 or len(commit) != 1:
        fail(f"turn {turn} must run begin and commit exactly once")
    for event in status + begin + commit:
        if event.get("exit_code") != 0:
            fail(f"turn {turn} contains a failed Tutor command")
        if not isinstance(event.get("elapsed_ms"), (int, float)) or event["elapsed_ms"] < 0:
            fail(f"turn {turn} command is missing timing")
    begin = begin[0]
    commit = commit[0]
    required_begin = ("session_id", "owner", "request_id", "turn_id", "revision", "input")
    required_commit = ("session_id", "owner", "request_id", "turn_id", "if_revision")
    if any(begin.get(field) in (None, "") for field in required_begin):
        fail(f"turn {turn} begin is missing identity or revision evidence")
    if any(commit.get(field) in (None, "") for field in required_commit):
        fail(f"turn {turn} commit is missing identity or revision evidence")
    for field in ("session_id", "owner", "turn_id"):
        if begin[field] != commit[field]:
            fail(f"turn {turn} commit does not reference begin {field}")
    if begin["revision"] != commit["if_revision"]:
        fail(f"turn {turn} commit does not reference begin revision")
    if begin["request_id"] == commit["request_id"]:
        fail(f"turn {turn} begin and commit must use distinct mutation request IDs")
    begin_index = turn_events.index(begin)
    commit_index = turn_events.index(commit)
    reply_index = turn_events.index(finals[0])
    state_index = next(index for index, event in enumerate(turn_events) if event.get("kind") == "state")
    if status and turn_events.index(status[0]) >= begin_index:
        fail(f"turn {turn} status must precede begin")
    if commentaries and turn_events.index(commentaries[0]) >= begin_index:
        fail(f"turn {turn} commentary must precede begin")
    if not begin_index < commit_index < reply_index < state_index:
        fail(f"turn {turn} must order begin < commit < final < pending=0")
    visible = [event.get("text", "").strip() for event in assistant_events]
    return finals[0]["text"].strip(), timings[0], begin, commit, visible


def no_code_init(events):
    reply, elapsed, begin, _, visible = require_turn(events, "init", {1})
    subject = commands(events, "init", "subject")
    session = commands(events, "init", "session")
    if len(subject) != 1 or len(session) != 1:
        fail("first English lesson must create exactly one subject and one session")
    turn_events = [event for event in events if event.get("turn") == "init"]
    status = commands(events, "init", "status")[0]
    if not turn_events.index(status) < turn_events.index(subject[0]) < turn_events.index(session[0]) < turn_events.index(begin):
        fail("first lesson must order status < subject < session < begin")
    if "$using-tutor" in begin["input"]:
        fail("Tutor control invocation was persisted as learner input")
    if "开始学习英语" not in begin["input"]:
        fail("no-code journey did not persist the learner-visible English request")
    if re.search(r"codegraph|lwc\s+cg|代码图|启用.{0,12}(?:文档图|图能力)", "\n".join(visible), re.I):
        fail("no-code learning journey exposed graph guidance")
    for event in events:
        if event.get("kind") not in {"command", "probe_command"}:
            continue
        command = " ".join(map(str, event.get("argv", []))).lower()
        if re.search(r"(?:^|\s)(?:\S*/)?(?:lwc\s+)?(?:cg|codegraph)\s+(?:init|status)(?:\s|$)", command):
            fail("no-code learning journey probed or initialized CodeGraph")
        if (
            re.search(r"(?:^|\s)(?:\S*/)?lwc\s+(?:tutor\s+)?(?:goal|plan|practice)(?:\s|$)", command)
            or "--help" in command
            or command.endswith(" help")
            or "skill.md" in command
            or "/skills/using-" in command
            or re.search(r"(?:^|\s)(?:\S*/)?sqlite3?(?:\s|$)", command)
        ):
            fail("first lesson used speculative or private control-plane discovery")
    return {"init_agent_ms": elapsed}


def steady_state(events):
    reply_a, elapsed_a, begin_a, commit_a, _ = require_turn(events, "A", {0, 1})
    reply_b, elapsed_b, begin_b, commit_b, visible_b = require_turn(events, "B", {0})
    del reply_a
    if begin_a["session_id"] != begin_b["session_id"] or begin_a["owner"] != begin_b["owner"]:
        fail("A/B turns must keep the same session and owner")
    if begin_a["input"] != "A" or begin_b["input"] != "B":
        fail("A/B turns must persist the exact learner-visible answers")
    if begin_a["turn_id"] == begin_b["turn_id"]:
        fail("A/B turns must have distinct turn IDs")
    request_ids = [begin_a["request_id"], commit_a["request_id"], begin_b["request_id"], commit_b["request_id"]]
    if len(set(request_ids)) != len(request_ids):
        fail("A/B begin and commit mutations must use fresh request IDs")
    forbidden_commands = []
    for event in events:
        if event.get("turn") != "B" or event.get("kind") not in {"command", "probe_command"}:
            continue
        command = " ".join(map(str, event.get("argv", []))).lower()
        if (
            re.search(r"(?:^|\s)(?:\S*/)?lwc\s+practice(?:\s|$)", command)
            or re.search(r"(?:^|\s)(?:\S*/)?lwc\s+tutor\s+status(?:\s|$)", command)
            or re.search(r"(?:^|\s)(?:\S*/)?sqlite3?(?:\s|$)", command)
            or "--help" in command
            or command.endswith(" help")
            or "skill.md" in command
            or "/skills/using-" in command
        ):
            forbidden_commands.append(command)
    if forbidden_commands:
        fail(f"turn B repeated control-plane discovery: {forbidden_commands}")
    if re.search(
        r"\b(?:lwc|tutor|practice|sqlite|codegraph|status|checkpoint|request_id|if_revision|json)\b|检查.{0,8}状态|加载.{0,8}练习",
        "\n".join(visible_b),
        re.I,
    ):
        fail("turn B exposed control-plane narration")
    return {"turn_a_agent_ms": elapsed_a, "turn_b_agent_ms": elapsed_b}


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in {"no-code-init", "steady-state"}:
        fail("usage: learning_agent_ux_check.py no-code-init|steady-state TRANSCRIPT.jsonl")
    events = load(sys.argv[2])
    for event in events:
        if event.get("kind") in {"command", "probe_command"} and event.get("exit_code") != 0:
            fail("transcript contains a failed command")
    summary = no_code_init(events) if sys.argv[1] == "no-code-init" else steady_state(events)
    print(json.dumps({"accepted": True, **summary}, ensure_ascii=False, sort_keys=True))


if __name__ == "__main__":
    main()
