import hashlib
import os
from pathlib import Path
import tempfile
import unittest

from benchmarks.agent_memory.lwc_backend import ConflictError, LwcBackend


def lwc_binary() -> Path:
    configured = os.environ.get("LWC_BENCH_BINARY")
    if configured:
        return Path(configured).resolve()
    return Path(__file__).resolve().parents[2] / "target" / "debug" / "lwc"


class BackendTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.backend = LwcBackend(
            Path(self.temp.name) / "state",
            binary=lwc_binary(),
            timeout=30,
        )

    def test_scope_path_is_fixed_hash_below_state_root(self) -> None:
        scope = "../../unsafe/user"
        root = self.backend.scope_root(scope)

        self.assertEqual(root.parent, (Path(self.temp.name) / "state").resolve())
        self.assertEqual(root.name, hashlib.sha256(scope.encode()).hexdigest())

    def test_add_is_idempotent_and_rejects_changed_retry(self) -> None:
        messages = [{"role": "user", "content": "remember indigo-orchid"}]

        self.backend.add("user-a", "request-1", "session-1", messages)
        self.backend.add("user-a", "request-1", "session-1", messages)

        with self.assertRaises(ConflictError):
            self.backend.add(
                "user-a",
                "request-1",
                "session-1",
                [{"role": "user", "content": "changed content"}],
            )

    def test_search_returns_ranked_source_content_and_metadata(self) -> None:
        self.backend.add(
            "user-a",
            "request-1",
            "session-1",
            [{"role": "assistant", "timestamp": 1_704_067_200_000, "content": "indigo-orchid lives here"}],
        )

        evidence = self.backend.search("user-a", "indigo orchid", 5)

        self.assertTrue(evidence)
        self.assertIn("indigo-orchid lives here", evidence[0].content)
        self.assertEqual(evidence[0].session_id, "session-1")
        self.assertTrue(evidence[0].id)
        self.assertIsInstance(evidence[0].score, float)
        self.assertTrue(evidence[0].created_at)

    def test_search_never_crosses_scope(self) -> None:
        self.backend.add(
            "user-a",
            "request-a",
            "session-a",
            [{"role": "user", "content": "shared words indigo-orchid"}],
        )
        self.backend.add(
            "user-b",
            "request-b",
            "session-b",
            [{"role": "user", "content": "shared words amber-comet"}],
        )

        evidence = self.backend.search("user-a", "amber comet", 5)

        self.assertFalse(any("amber-comet" in item.content for item in evidence))


if __name__ == "__main__":
    unittest.main()
