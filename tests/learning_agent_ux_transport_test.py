#!/usr/bin/env python3
import subprocess
import sys
import tempfile
from pathlib import Path

from learning_agent_ux_transport import JsonLineReader


with tempfile.TemporaryDirectory(prefix="lwc-agent-ux-transport-") as root:
    evidence = Path(root) / "app_server.jsonl"
    process = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import sys,time; sys.stdout.write('{\\\"n\\\":1}\\n{\\\"n\\\":2}\\n'); sys.stdout.flush(); time.sleep(1)",
        ],
        stdout=subprocess.PIPE,
        text=True,
    )
    reader = JsonLineReader(process.stdout, evidence)
    assert reader.receive(0.5) == {"n": 1}
    assert reader.receive(0.1) == {"n": 2}
    process.wait()
    assert evidence.read_text().splitlines() == ['{"n":1}', '{"n":2}']
print("learning Agent UX transport test: passed")
