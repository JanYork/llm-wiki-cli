#!/usr/bin/env python3
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


root = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="lwc-ux-probe-test-") as temporary:
    trace = Path(temporary) / "trace.jsonl"
    isolated_home = Path(temporary) / "isolated-home"
    isolated_home.mkdir()
    env = {
        **os.environ,
        "HOME": "/tmp/lwc-agent-ux-home",
        "LWC_UX_DATA_HOME": str(isolated_home),
        "LWC_UX_REAL_LWC": sys.executable,
        "LWC_UX_TRACE": str(trace),
    }
    result = subprocess.run(
        [
            sys.executable,
            str(root / "learning_agent_ux_lwc_probe.py"),
            "-c",
            "import json,os; print(json.dumps({'home': os.environ['HOME']}))",
        ],
        env=env,
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["home"] == str(isolated_home)
    assert json.loads(trace.read_text())["exit_code"] == 0
print("learning Agent UX probe isolation test: passed")
