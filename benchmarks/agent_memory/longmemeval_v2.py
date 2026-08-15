from __future__ import annotations

import json
from pathlib import Path
import shutil
import uuid

from memory_modules.memory import Memory, MemoryConfig, MemoryContextItem, register_memory, require

from benchmarks.agent_memory.lwc_backend import LwcBackend


_PERSISTED_STATE_ROOT = "lwc_state"
_PRIVATE_EVALUATOR_FIELDS = {
    "answer",
    "answer_gold",
    "eval_function",
    "question",
    "question_id",
    "question_type",
}


@register_memory
class LwcMemory(Memory):
    memory_type = "lwc"

    def __init__(self, memory_params: dict[str, object]) -> None:
        allowed = {
            "state_root",
            "lwc_binary",
            "search_limit",
            "command_timeout_seconds",
        }
        unexpected = sorted(set(memory_params) - allowed)
        require(not unexpected, f"lwc memory_params contains unexpected keys: {unexpected}")

        state_root_value = memory_params.get("state_root")
        require(
            isinstance(state_root_value, str) and state_root_value.strip(),
            "lwc state_root must be a non-empty string",
        )
        state_path = Path(state_root_value)
        require(
            state_path.is_absolute() or state_root_value == _PERSISTED_STATE_ROOT,
            "lwc state_root must be absolute",
        )
        binary = memory_params.get("lwc_binary", "lwc")
        require(
            isinstance(binary, str) and binary.strip(),
            "lwc lwc_binary must be a non-empty string",
        )
        search_limit = memory_params.get("search_limit", 10)
        require(
            isinstance(search_limit, int)
            and not isinstance(search_limit, bool)
            and search_limit > 0,
            "lwc search_limit must be a positive integer",
        )
        timeout = memory_params.get("command_timeout_seconds", 120)
        require(
            isinstance(timeout, (int, float))
            and not isinstance(timeout, bool)
            and timeout > 0,
            "lwc command_timeout_seconds must be positive",
        )

        normalized = {
            "state_root": state_root_value,
            "lwc_binary": binary,
            "search_limit": search_limit,
            "command_timeout_seconds": timeout,
        }
        super().__init__(normalized)
        self.scope_id = uuid.uuid4().hex
        self.search_limit = search_limit
        self.command_timeout_seconds = float(timeout)
        initial_root = (
            state_path.resolve()
            if state_path.is_absolute()
            else (Path.cwd() / ".lwc-load-placeholder").resolve()
        )
        self.backend = LwcBackend(
            initial_root,
            binary=binary,
            timeout=self.command_timeout_seconds,
        )

    @property
    def memory_config(self) -> MemoryConfig:
        return {
            "memory_type": self.memory_type,
            "memory_params": {
                **self.memory_params,
                "state_root": _PERSISTED_STATE_ROOT,
            },
        }

    def insert(self, trajectory: dict[str, object]) -> None:
        require(isinstance(trajectory, dict), "lwc trajectory must be an object")
        trajectory_id = trajectory.get("id")
        require(
            isinstance(trajectory_id, str) and trajectory_id.strip(),
            "lwc trajectory id must be a non-empty string",
        )
        public_trajectory = {
            key: value
            for key, value in trajectory.items()
            if key not in _PRIVATE_EVALUATOR_FIELDS
        }
        try:
            content = json.dumps(
                public_trajectory,
                ensure_ascii=False,
                indent=2,
                sort_keys=True,
            )
        except (TypeError, ValueError) as error:
            raise RuntimeError("lwc trajectory must be JSON-serializable") from error
        self.backend.add(
            self.scope_id,
            trajectory_id,
            trajectory_id,
            [{"role": "trajectory", "content": content}],
        )

    def query(
        self,
        query: str,
        query_image: str | None = None,
    ) -> list[MemoryContextItem]:
        del query_image
        return [
            {"type": "text", "value": item.content}
            for item in self.backend.search(self.scope_id, query, self.search_limit)
            if item.content.strip()
        ]

    def _save_backend(self, output_dir: Path) -> None:
        source = self.backend.scope_root(self.scope_id)
        require(source.is_dir(), "lwc memory has no initialized backend state")
        state_root = output_dir / _PERSISTED_STATE_ROOT
        destination = state_root / source.name
        require(not destination.exists(), f"Refusing to overwrite LWC state: {destination}")
        state_root.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, destination)
        (output_dir / "lwc_backend.json").write_text(
            json.dumps(
                {"schema_version": 1, "scope_id": self.scope_id, "text_only": True},
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )

    def _load_backend(self, input_dir: Path) -> None:
        metadata_path = input_dir / "lwc_backend.json"
        require(metadata_path.is_file(), f"Missing LWC backend metadata: {metadata_path}")
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        require(isinstance(metadata, dict), "LWC backend metadata must be an object")
        scope_id = metadata.get("scope_id")
        require(
            isinstance(scope_id, str) and scope_id.strip(),
            "LWC backend metadata missing scope_id",
        )
        state_root = (input_dir / _PERSISTED_STATE_ROOT).resolve()
        backend = LwcBackend(
            state_root,
            binary=str(self.memory_params["lwc_binary"]),
            timeout=self.command_timeout_seconds,
        )
        require(
            (backend.scope_root(scope_id) / ".lwc" / "wiki.db").is_file(),
            "Saved LWC backend state is missing",
        )
        self.scope_id = scope_id
        self.backend = backend
