#!/usr/bin/env python3
import json
import os
import queue
import shlex
import subprocess
import sys
import threading
import time
from pathlib import Path

from learning_agent_ux_transport import JsonLineReader


if len(sys.argv) != 4:
    raise SystemExit("usage: learning_agent_ux_rpc.py HOST REMOTE_ROOT OUTPUT_DIR")

host, remote_root, output_dir = sys.argv[1:]
destination = Path(output_dir)
destination.mkdir(parents=True, exist_ok=True)
raw_rpc_path = destination / "app_server.jsonl"
stderr_path = destination / "app_server.stderr"
remote_home = f"{remote_root}/home"
remote_cwd = f"{remote_root}/no-code"
remote_trace = f"{remote_root}/lwc-trace.jsonl"
remote_base_path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"


def discover_remote(name, override):
    if override:
        return override
    result = subprocess.run(
        [
            "ssh",
            "-6",
            host,
            f"PATH={remote_base_path}; export PATH; command -v {name} || test ! -x \"$HOME/.local/bin/{name}\" || printf '%s\\n' \"$HOME/.local/bin/{name}\"",
        ],
        text=True,
        capture_output=True,
    )
    value = result.stdout.strip()
    if result.returncode or not value:
        raise SystemExit(f"remote {name} is not available on PATH")
    return value


remote_codex = discover_remote("codex", os.environ.get("LWC_UX_REMOTE_CODEX"))
remote_lwc = discover_remote("lwc", os.environ.get("LWC_UX_REMOTE_LWC"))
remote_user_home = os.environ.get("LWC_UX_REMOTE_HOME")
if not remote_user_home:
    home_result = subprocess.run(
        ["ssh", "-6", host, "printf '%s' \"$HOME\""],
        text=True,
        capture_output=True,
    )
    remote_user_home = home_result.stdout.strip()
    if home_result.returncode or not remote_user_home:
        raise SystemExit("remote HOME could not be resolved; set LWC_UX_REMOTE_HOME to override")
remote_codex_home = os.environ.get("LWC_UX_REMOTE_CODEX_HOME") or f"{remote_user_home}/.codex"
remote_path = f"{remote_root}/bin:{remote_base_path}"
remote_command = (
    f"PATH={remote_path}; export PATH; "
    f"HOME={shlex.quote(remote_user_home)}; export HOME; "
    f"CODEX_HOME={shlex.quote(remote_codex_home)}; export CODEX_HOME; "
    f"LWC_UX_DATA_HOME={shlex.quote(remote_home)}; export LWC_UX_DATA_HOME; "
    f"LWC_UX_TRACE={shlex.quote(remote_trace)}; export LWC_UX_TRACE; "
    f"LWC_UX_REAL_LWC={shlex.quote(remote_lwc)}; export LWC_UX_REAL_LWC; "
    f"exec {shlex.quote(remote_codex)} app-server --stdio"
)
process = subprocess.Popen(
    ["ssh", "-6", "-T", host, remote_command],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)
stderr = []


def drain_stderr():
    for line in process.stderr:
        stderr.append(line.rstrip())
        del stderr[:-100]
        with stderr_path.open("a", encoding="utf-8") as evidence:
            evidence.write(line)


threading.Thread(target=drain_stderr, daemon=True).start()
stdout_reader = JsonLineReader(process.stdout, raw_rpc_path)
request_id = 0
pending_messages = []
trace_offset = 0


def send(method, params=None, notification=False):
    global request_id
    message = {"method": method, "params": params or {}}
    if not notification:
        request_id += 1
        message["id"] = request_id
    process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    process.stdin.flush()
    return None if notification else request_id


def receive(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise SystemExit(f"app-server response timeout: {stderr[-10:]}")
    try:
        message = stdout_reader.receive(remaining)
    except queue.Empty:
        raise SystemExit(f"app-server response timeout: {stderr[-10:]}")
    if isinstance(message, EOFError):
        raise SystemExit(f"app-server closed early: {stderr[-10:]}")
    if isinstance(message, ValueError):
        raise SystemExit(str(message))
    return message


def response(expected_id, timeout=60):
    deadline = time.monotonic() + timeout
    while True:
        message = receive(deadline)
        if message.get("id") == expected_id and ("result" in message or "error" in message):
            if "error" in message:
                raise SystemExit(f"app-server error: {message['error']}")
            return message["result"]
        pending_messages.append(message)


def remote_json(command):
    result = subprocess.run(
        ["ssh", "-6", host, f"PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH; export PATH; {command}"],
        text=True,
        capture_output=True,
    )
    if result.returncode:
        raise SystemExit(f"remote readback failed: {result.stderr}")
    return json.loads(result.stdout)


def new_trace_events():
    global trace_offset
    result = subprocess.run(
        ["ssh", "-6", host, f"PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH; export PATH; sed -n '1,99999p' {shlex.quote(remote_trace)}"],
        text=True,
        capture_output=True,
    )
    if result.returncode:
        raise SystemExit(f"trace readback failed: {result.stderr}")
    events = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    unseen = events[trace_offset:]
    trace_offset = len(events)
    return unseen


def pending_count():
    status = remote_json(f"HOME={shlex.quote(remote_home)} {shlex.quote(remote_lwc)} tutor status")
    return status["result"]["pending_turns"]


def start_thread():
    result = response(send("thread/start", {
        "cwd": remote_cwd,
        "ephemeral": True,
        "approvalPolicy": "never",
        "sandbox": "workspace-write",
    }))
    return result["thread"]["id"]


def run_turn(thread_id, label, text):
    started = time.perf_counter()
    result = response(send("turn/start", {
        "threadId": thread_id,
        "input": [{"type": "text", "text": text}],
        "approvalPolicy": "never",
        "sandboxPolicy": {
            "type": "workspaceWrite",
            "writableRoots": [remote_root],
            "networkAccess": False,
        },
    }))
    turn_id = result["turn"]["id"]
    timeline = []
    deadline = time.monotonic() + 180
    while True:
        message = pending_messages.pop(0) if pending_messages else receive(deadline)
        if "id" in message and "method" in message:
            raise SystemExit(f"unhandled server request: {message['method']}")
        if message.get("method") == "item/completed":
            params = message.get("params", {})
            item = params.get("item", {})
            completed = params.get("completedAtMs")
            if not isinstance(completed, int) or completed <= 0:
                raise SystemExit("item/completed is missing required completedAtMs")
            if item.get("type") == "commandExecution":
                timeline.append((completed, {
                    "turn": label,
                    "kind": "probe_command",
                    "argv": [item.get("command", "")],
                    "exit_code": item.get("exitCode"),
                    "elapsed_ms": item.get("durationMs") or 0,
                }))
            elif item.get("type") == "agentMessage":
                timeline.append((completed, {
                    "turn": label,
                    "kind": "assistant",
                    "phase": item.get("phase"),
                    "text": item.get("text", ""),
                }))
        if message.get("method") == "turn/completed" and message.get("params", {}).get("turn", {}).get("id") == turn_id:
            if message["params"]["turn"].get("status") != "completed":
                raise SystemExit(f"turn ended with status {message['params']['turn'].get('status')!r}")
            break
    for event in new_trace_events():
        event["turn"] = label
        timeline.append((event.pop("completed_at_ms"), event))
    timeline.sort(key=lambda pair: pair[0])
    events = [event for _, event in timeline]
    events.append({"turn": label, "kind": "state", "pending": pending_count()})
    events.append({
        "turn": label,
        "kind": "agent_timing",
        "elapsed_ms": round((time.perf_counter() - started) * 1000, 1),
    })
    return events


def write_jsonl(path, events):
    path.write_text("".join(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n" for event in events))


try:
    response(send("initialize", {"clientInfo": {"name": "learning-agent-ux", "version": "0.1.0"}}))
    send("initialized", notification=True)
    thread_id = start_thread()
    init_events = run_turn(
        thread_id,
        "init",
        "$using-tutor 开始学习英语。请先自然讲解一个小知识点，再给我一道能只用 A 或 B 回答的选择题。",
    )
    turn_a = run_turn(thread_id, "A", "A")
    turn_b = run_turn(thread_id, "B", "B")
    init_path = destination / "no_code_init.jsonl"
    steady_path = destination / "steady_state.jsonl"
    write_jsonl(init_path, init_events)
    write_jsonl(steady_path, [*turn_a, *turn_b])
    print(json.dumps({
        "init_transcript": str(init_path),
        "steady_transcript": str(steady_path),
        "init_agent_ms": next(event["elapsed_ms"] for event in init_events if event["kind"] == "agent_timing"),
        "turn_a_agent_ms": next(event["elapsed_ms"] for event in turn_a if event["kind"] == "agent_timing"),
        "turn_b_agent_ms": next(event["elapsed_ms"] for event in turn_b if event["kind"] == "agent_timing"),
    }, ensure_ascii=False, sort_keys=True))
finally:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
