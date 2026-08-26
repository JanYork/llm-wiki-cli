#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


if len(sys.argv) != 4:
    raise SystemExit("usage: learning_agent_ux_replay.py TRACE.jsonl TUTOR_RUNTIME OUTPUT.json")

trace_path, runtime_path, output_path = map(Path, sys.argv[1:])
lwc = os.environ.get("LWC_UX_LWC") or shutil.which("lwc")
if not lwc:
    raise SystemExit("lwc is not available on PATH; set LWC_UX_LWC to override")
with tempfile.TemporaryDirectory(prefix="lwc-agent-ux-replay-") as root:
    home = Path(root) / "home"
    cwd = Path(root) / "no-code"
    home.mkdir()
    cwd.mkdir()
    env = {**os.environ, "HOME": str(home)}
    subprocess.run([lwc, "--scope", "global", "init"], env=env, cwd=cwd, check=True, capture_output=True)
    subprocess.run(
        [lwc, "--scope", "global", "config", "set", "--tutor", "enabled"],
        env=env,
        cwd=cwd,
        check=True,
        capture_output=True,
    )
    shutil.copytree(runtime_path, home / ".lwc/runtime/tutor" / runtime_path.name)
    summary = []
    for index, line in enumerate(trace_path.read_text().splitlines(), 1):
        event = json.loads(line)
        argv = event.get("argv", [])
        if argv[:1] != ["lwc"]:
            continue
        result = subprocess.run(
            [lwc, *argv[1:]],
            env=env,
            cwd=cwd,
            text=True,
            capture_output=True,
        )
        record = {"index": index, "action": event.get("action"), "exit_code": result.returncode}
        if result.returncode:
            try:
                error = json.loads(result.stderr)["error"]
                record.update({"code": error.get("code"), "message": error.get("message")})
            except (json.JSONDecodeError, KeyError):
                record["message"] = result.stderr.strip()[:500]
        summary.append(record)
    output_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(summary, ensure_ascii=False))
