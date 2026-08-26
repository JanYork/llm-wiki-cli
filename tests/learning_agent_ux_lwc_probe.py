#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import time


args = sys.argv[1:]
stdin = sys.stdin.read() if "--json" in args and args[args.index("--json") + 1] == "-" else None
started = time.perf_counter()
real_lwc = os.environ.get("LWC_UX_REAL_LWC") or shutil.which("lwc")
if not real_lwc:
    raise SystemExit("lwc is not available on PATH; set LWC_UX_REAL_LWC to override")
result = subprocess.run(
    [real_lwc, *args],
    input=stdin,
    text=True,
    capture_output=True,
    env={**os.environ, "HOME": os.environ["LWC_UX_DATA_HOME"]},
)
elapsed_ms = round((time.perf_counter() - started) * 1000, 1)
sys.stdout.write(result.stdout)
sys.stderr.write(result.stderr)


def payload():
    if "--json" not in args:
        return {}
    value = args[args.index("--json") + 1]
    if value == "-":
        value = stdin
    elif value.startswith("@"):
        with open(value[1:], encoding="utf-8") as file:
            value = file.read()
    try:
        return json.loads(value)
    except (TypeError, json.JSONDecodeError):
        return {}


event = {
    "kind": "command",
    "argv": ["lwc", *args],
    "exit_code": result.returncode,
    "elapsed_ms": elapsed_ms,
    "completed_at_ms": int(time.time() * 1000),
}
if args[:2] == ["tutor", "status"]:
    event["action"] = "status"
elif args[:3] == ["tutor", "subject", "create"]:
    event["action"] = "subject"
elif args[:3] == ["tutor", "session", "create"]:
    event["action"] = "session"
elif args[:3] == ["tutor", "turn", "begin"]:
    event["action"] = "begin"
    mutation = payload()
    event.update({key: mutation.get(key) for key in ("session_id", "owner", "request_id")})
elif args[:3] == ["tutor", "turn", "commit"]:
    event["action"] = "commit"
    mutation = payload()
    event.update({"owner": mutation.get("owner"), "request_id": mutation.get("request_id")})
    if len(args) > 3:
        event["turn_id"] = args[3]
    if "--if-revision" in args:
        event["if_revision"] = int(args[args.index("--if-revision") + 1])
else:
    event["action"] = "other"

try:
    output = json.loads(result.stdout)
    turn = output.get("result", {}).get("turn", {})
    if turn:
        event.update({
            "session_id": turn.get("session_id"),
            "owner": turn.get("owner"),
            "turn_id": turn.get("id"),
            "input": turn.get("input"),
        })
        if event["action"] == "begin":
            event["revision"] = turn.get("revision")
except json.JSONDecodeError:
    pass

with open(os.environ["LWC_UX_TRACE"], "a", encoding="utf-8") as trace:
    trace.write(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n")
raise SystemExit(result.returncode)
