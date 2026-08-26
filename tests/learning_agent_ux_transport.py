import json
import queue
import threading


class JsonLineReader:
    def __init__(self, stream, evidence_path):
        self._stream = stream
        self._evidence_path = evidence_path
        self._messages = queue.Queue()
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self):
        for line in self._stream:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                self._messages.put(ValueError(f"non-JSON app-server stdout: {line!r}"))
                continue
            with self._evidence_path.open("a", encoding="utf-8") as evidence:
                evidence.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n")
            self._messages.put(message)
        self._messages.put(EOFError("app-server stdout closed"))

    def receive(self, timeout):
        return self._messages.get(timeout=timeout)
