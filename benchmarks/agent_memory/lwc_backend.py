from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading


_METADATA_PREFIX = "Agent memory metadata: "


@dataclass(frozen=True)
class Evidence:
    id: str
    content: str
    score: float
    created_at: str
    session_id: str


class AdapterError(RuntimeError):
    pass


class ConflictError(AdapterError):
    pass


class LwcBackend:
    def __init__(
        self,
        state_root: Path,
        binary: str | Path = "lwc",
        timeout: float = 120.0,
    ) -> None:
        self.state_root = Path(state_root).resolve()
        self.binary = str(binary)
        self.timeout = timeout
        self._locks_guard = threading.Lock()
        self._locks: dict[str, threading.RLock] = {}

    def scope_root(self, scope_id: str) -> Path:
        scope_id = self._required_text(scope_id, "scope_id")
        return self.state_root / hashlib.sha256(scope_id.encode("utf-8")).hexdigest()

    def add(
        self,
        scope_id: str,
        memory_id: str,
        session_id: str,
        messages: list[dict[str, object]],
    ) -> None:
        self.add_many(scope_id, [(memory_id, session_id, messages)])

    def add_many(
        self,
        scope_id: str,
        memories: list[tuple[str, str, list[dict[str, object]]]],
    ) -> None:
        scope_id = self._required_text(scope_id, "scope_id")
        prepared = [self._prepare_memory(*memory) for memory in memories]
        if not prepared:
            raise AdapterError("memories must not be empty")

        with self._scope_lock(scope_id):
            root = self.scope_root(scope_id)
            self._ensure_initialized(root)
            source_dir = root / "sources"
            source_dir.mkdir(exist_ok=True)
            pending: list[tuple[Path, Path]] = []
            for memory_id, body in prepared:
                digest = hashlib.sha256(memory_id.encode("utf-8")).hexdigest()
                path = source_dir / f"{digest}.md"
                receipt = source_dir / f"{digest}.added"
                if path.exists():
                    if path.read_text(encoding="utf-8") != body:
                        raise ConflictError(f"memory_id reused with different content: {memory_id}")
                    if receipt.exists():
                        continue
                else:
                    self._write_atomic(path, body)
                pending.append((path, receipt))

            if not pending:
                return

            manifest = {
                "sources": [
                    {"path": str(path), "title": f"Benchmark memory {path.stem[:12]}"}
                    for path, _ in pending
                ]
            }
            manifest_path = root / "source-manifest.json"
            self._write_atomic(
                manifest_path,
                json.dumps(manifest, ensure_ascii=False, sort_keys=True) + "\n",
            )
            self._run(root, ["source", "add-manifest", str(manifest_path)])
            for _, receipt in pending:
                self._write_atomic(receipt, "added\n")

    def search(self, scope_id: str, query: str, limit: int) -> list[Evidence]:
        scope_id = self._required_text(scope_id, "scope_id")
        query = self._required_text(query, "query")
        if not isinstance(limit, int) or isinstance(limit, bool) or limit < 1:
            raise AdapterError("limit must be a positive integer")

        root = self.scope_root(scope_id)
        if not (root / ".lwc" / "wiki.db").is_file():
            return []
        response = self._run(
            root,
            [
                "search",
                query,
                "--type",
                "source",
                "--granularity",
                "passage",
                "--limit",
                str(limit),
            ],
        )
        evidence: list[Evidence] = []
        seen: set[str] = set()
        for result in response.get("results", []):
            document = result.get("document") or {}
            source_id = document.get("identifier")
            if source_id is None and result.get("type") == "source":
                source_id = result.get("identifier")
            source_id = str(source_id or "")
            if not source_id or source_id in seen:
                continue
            seen.add(source_id)
            source = self._read_source(root, source_id)
            metadata = self._parse_metadata(source["content"])
            evidence.append(
                Evidence(
                    id=str(metadata.get("memory_id") or source_id),
                    content=source["content"],
                    score=1.0 / (len(evidence) + 1),
                    created_at=str(source.get("created_at") or ""),
                    session_id=str(metadata.get("session_id") or ""),
                )
            )
            if len(evidence) >= limit:
                break
        return evidence

    def _prepare_memory(
        self,
        memory_id: str,
        session_id: str,
        messages: list[dict[str, object]],
    ) -> tuple[str, str]:
        memory_id = self._required_text(memory_id, "memory_id")
        session_id = self._required_text(session_id, "session_id")
        if not isinstance(messages, list) or not messages:
            raise AdapterError("messages must be a non-empty list")

        normalized = []
        for message in messages:
            if not isinstance(message, dict):
                raise AdapterError("each message must be an object")
            role = self._required_text(message.get("role"), "message.role")
            content = self._required_text(message.get("content"), "message.content")
            item: dict[str, object] = {"role": role, "content": content}
            if "timestamp" in message and message["timestamp"] is not None:
                item["timestamp"] = message["timestamp"]
            normalized.append(item)

        metadata = json.dumps(
            {"memory_id": memory_id, "session_id": session_id},
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        lines = [_METADATA_PREFIX + metadata, "", "# Memory", ""]
        for item in normalized:
            timestamp = item.get("timestamp")
            heading = f"## {item['role']}"
            if timestamp is not None:
                heading += f" @ {timestamp}"
            lines.extend([heading, "", str(item["content"]), ""])
        return memory_id, "\n".join(lines)

    def _ensure_initialized(self, root: Path) -> None:
        if (root / ".lwc" / "wiki.db").is_file():
            return
        root.mkdir(parents=True, exist_ok=True)
        self._run(root, ["init"])
        # Benchmark state must not inherit a user's global graph engine. Source
        # retrieval is the measured system, and detached graph work breaks both
        # reproducibility and immediate disposable-state cleanup.
        self._run(root, ["config", "set", "--graph", "disabled"])

    def _read_source(self, root: Path, source_id: str) -> dict[str, object]:
        chunks: list[str] = []
        offset = 0
        source: dict[str, object] | None = None
        while True:
            response = self._run(
                root,
                [
                    "source",
                    "show",
                    source_id,
                    "--offset-chars",
                    str(offset),
                    "--max-chars",
                    "100000",
                ],
            )
            source = response["source"]
            chunks.append(str(source["content"]))
            window = response["window"]
            if not window["has_more"]:
                break
            offset = int(window["next_offset_chars"])
        assert source is not None
        source = dict(source)
        source["content"] = "".join(chunks)
        return source

    def _run(self, root: Path, args: list[str]) -> dict[str, object]:
        environment = os.environ.copy()
        environment["LWC_PROJECT_ROOT"] = str(root)
        try:
            completed = subprocess.run(
                [self.binary, *args],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=self.timeout,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise AdapterError(f"lwc command failed to start or timed out: {args[0]}") from error
        if completed.returncode != 0:
            try:
                payload = json.loads(completed.stderr)
                detail = payload.get("error", {})
                code = detail.get("code", "lwc_command_failed")
                message = detail.get("message", "lwc command failed")
            except json.JSONDecodeError:
                code, message = "lwc_command_failed", "lwc command returned invalid error JSON"
            raise AdapterError(f"{code}: {message}")
        try:
            value = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            raise AdapterError("lwc command returned invalid JSON") from error
        if not isinstance(value, dict):
            raise AdapterError("lwc command did not return a JSON object")
        return value

    def _scope_lock(self, scope_id: str) -> threading.RLock:
        digest = hashlib.sha256(scope_id.encode("utf-8")).hexdigest()
        with self._locks_guard:
            return self._locks.setdefault(digest, threading.RLock())

    @staticmethod
    def _required_text(value: object, name: str) -> str:
        if not isinstance(value, str) or not value.strip():
            raise AdapterError(f"{name} must be a non-empty string")
        return value

    @staticmethod
    def _write_atomic(path: Path, content: str) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            "w",
            encoding="utf-8",
            dir=path.parent,
            delete=False,
        ) as temporary:
            temporary.write(content)
            temporary_path = Path(temporary.name)
        os.replace(temporary_path, path)

    @staticmethod
    def _parse_metadata(content: str) -> dict[str, object]:
        first_line = content.splitlines()[0] if content else ""
        if not first_line.startswith(_METADATA_PREFIX):
            return {}
        try:
            value = json.loads(first_line[len(_METADATA_PREFIX) :])
        except json.JSONDecodeError:
            return {}
        return value if isinstance(value, dict) else {}
