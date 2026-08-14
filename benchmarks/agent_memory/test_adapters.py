import hashlib
import importlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request

from benchmarks.agent_memory.aml_api import create_server
from benchmarks.agent_memory.lwc_backend import ConflictError, LwcBackend
from benchmarks.agent_memory.longmemeval_v1 import evaluate_dataset


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


class LongMemEvalV1Tests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dataset = self.root / "longmemeval_s_cleaned.json"
        self.dataset.write_text(
            json.dumps(
                [
                    {
                        "question_id": "q1",
                        "question_type": "single-session-user",
                        "question": "Where is the cobalt passport?",
                        "answer": "In the cedar drawer.",
                        "question_date": "2024-02-01",
                        "haystack_session_ids": ["s1", "s2"],
                        "haystack_dates": ["2024-01-01", "2024-01-02"],
                        "haystack_sessions": [
                            [
                                {
                                    "role": "user",
                                    "content": "The cobalt passport is in the cedar drawer.",
                                    "has_answer": True,
                                },
                                {"role": "assistant", "content": "I will remember that."},
                            ],
                            [{"role": "user", "content": "The bicycle is in the garage."}],
                        ],
                        "answer_session_ids": ["s1"],
                    },
                    {
                        "question_id": "q2_abs",
                        "question_type": "single-session-user",
                        "question": "What is the unavailable code?",
                        "answer": "I don't know.",
                        "question_date": "2024-02-02",
                        "haystack_session_ids": ["s3"],
                        "haystack_dates": ["2024-01-03"],
                        "haystack_sessions": [
                            [{"role": "user", "content": "No code was discussed."}]
                        ],
                        "answer_session_ids": [],
                    },
                ]
            ),
            encoding="utf-8",
        )

    def test_complete_runner_writes_official_log_and_metrics(self) -> None:
        output = self.root / "report.json"
        state = self.root / "state"

        report = evaluate_dataset(
            data_path=self.dataset,
            state_root=state,
            output_path=output,
            upstream_revision="test-revision",
            lwc_commit="test-lwc-commit",
            binary=lwc_binary(),
        )

        self.assertTrue(report["complete"])
        self.assertFalse(report["partial"])
        self.assertEqual(report["instances_total"], 2)
        self.assertEqual(report["instances_processed"], 2)
        self.assertEqual(report["instances_retrieval_scored"], 1)
        self.assertEqual(report["metrics"]["recall_at_5"], 1.0)
        self.assertEqual(report["metrics"]["recall_at_10"], 1.0)
        self.assertEqual(report["metrics"]["mrr"], 1.0)
        self.assertEqual(report["upstream_revision"], "test-revision")
        self.assertEqual(report["lwc_commit"], "test-lwc-commit")
        self.assertEqual(
            report["adapter_config"],
            {
                "state_root": str(state.resolve()),
                "search_limit": 50,
                "source_type": "source",
                "granularity": "passage",
                "scope": "isolated-per-question",
            },
        )
        self.assertEqual(
            report["dataset_sha256"], hashlib.sha256(self.dataset.read_bytes()).hexdigest()
        )
        self.assertIn("lwc ", report["lwc_version"])
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), report)

        records = [
            json.loads(line)
            for line in output.with_suffix(".jsonl").read_text(encoding="utf-8").splitlines()
        ]
        self.assertEqual(len(records), 2)
        self.assertEqual(records[0]["retrieval_results"]["ranked_items"][0]["corpus_id"], "s1")
        self.assertIn("recall_any@5", records[0]["retrieval_results"]["metrics"]["session"])
        source_text = "\n".join(
            path.read_text(encoding="utf-8") for path in state.rglob("sources/*.md")
        )
        self.assertIn("## user @ 2024-01-01", source_text)
        self.assertIn("## assistant @ 2024-01-01", source_text)

    def test_limit_marks_report_partial(self) -> None:
        report = evaluate_dataset(
            data_path=self.dataset,
            state_root=self.root / "state",
            output_path=self.root / "partial.json",
            upstream_revision="test-revision",
            lwc_commit="test-lwc-commit",
            binary=lwc_binary(),
            limit=1,
        )

        self.assertFalse(report["complete"])
        self.assertTrue(report["partial"])
        self.assertEqual(report["instances_processed"], 1)

    def test_duplicate_session_ids_preserve_each_occurrence(self) -> None:
        entries = json.loads(self.dataset.read_text(encoding="utf-8"))
        entry = entries[0]
        entry["haystack_session_ids"].append("s1")
        entry["haystack_dates"].append("2024-01-04")
        entry["haystack_sessions"].append(
            [{"role": "user", "content": "A later occurrence has different content."}]
        )
        self.dataset.write_text(json.dumps([entry]), encoding="utf-8")
        state = self.root / "duplicate-state"

        report = evaluate_dataset(
            data_path=self.dataset,
            state_root=state,
            output_path=self.root / "duplicate.json",
            upstream_revision="test-revision",
            lwc_commit="test-lwc-commit",
            binary=lwc_binary(),
        )

        self.assertTrue(report["complete"])
        sources = [
            path.read_text(encoding="utf-8") for path in state.glob("*/sources/*.md")
        ]
        self.assertEqual(len(sources), 3)
        self.assertEqual(sum('"session_id":"s1"' in source for source in sources), 2)


class AmlApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        backend = LwcBackend(
            Path(self.temp.name) / "state",
            binary=lwc_binary(),
            timeout=30,
        )
        self.server = create_server(backend, host="127.0.0.1", port=0)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self._stop_server)
        self.base_url = f"http://127.0.0.1:{self.server.server_port}"

    def _stop_server(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def request(
        self,
        method: str,
        path: str,
        payload: object | None = None,
        headers: dict[str, str] | None = None,
    ) -> tuple[int, dict[str, object]]:
        body = None if payload is None else json.dumps(payload).encode()
        request = urllib.request.Request(
            self.base_url + path,
            data=body,
            method=method,
            headers={"Content-Type": "application/json", **(headers or {})},
        )
        try:
            response = urllib.request.urlopen(request, timeout=30)
        except urllib.error.HTTPError as error:
            try:
                return error.code, json.loads(error.read())
            finally:
                error.close()
        with response:
            return response.status, json.loads(response.read())

    def test_health_add_search_retry_and_scope_isolation(self) -> None:
        status, health = self.request("GET", "/health")
        self.assertEqual(status, 200)
        self.assertEqual(health, {"status": "ok"})

        add = {
            "request_id": "request-a",
            "messages": [{"role": "user", "timestamp": 1_704_067_200_000, "content": "indigo-orchid belongs to user A"}],
            "user_id": "user-a",
            "session_id": "session-a",
        }
        status, response = self.request("POST", "/add", add)
        self.assertEqual(status, 200)
        self.assertEqual(
            response,
            {
                "success": True,
                "request_id": "request-a",
                "user_id": "user-a",
                "session_id": "session-a",
            },
        )
        self.assertEqual(self.request("POST", "/add", add)[0], 200)

        changed = dict(add)
        changed["messages"] = [{"role": "user", "content": "changed"}]
        self.assertEqual(self.request("POST", "/add", changed)[0], 409)

        self.request(
            "POST",
            "/add",
            {
                "request_id": "request-b",
                "messages": [{"role": "user", "content": "amber-comet belongs to user B"}],
                "user_id": "user-b",
                "session_id": "session-b",
            },
        )
        status, search = self.request(
            "POST",
            "/search",
            {"query": "indigo orchid", "user_id": "user-a", "top_k": 1},
        )
        self.assertEqual(status, 200)
        self.assertEqual(len(search["data"]), 1)
        self.assertIn("indigo-orchid", search["data"][0]["content"])
        self.assertNotIn("amber-comet", search["data"][0]["content"])
        self.assertTrue(search["data"][0]["id"])

    def test_validation_and_authentication(self) -> None:
        self.assertEqual(self.request("POST", "/add", {"messages": []})[0], 422)

        backend = LwcBackend(
            Path(self.temp.name) / "auth-state",
            binary=lwc_binary(),
            timeout=30,
        )
        protected = create_server(
            backend,
            host="127.0.0.1",
            port=0,
            api_key="secret-test-key",
        )
        thread = threading.Thread(target=protected.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{protected.server_port}"
            with urllib.request.urlopen(url + "/health", timeout=10) as response:
                self.assertEqual(response.status, 200)
            request = urllib.request.Request(
                url + "/search",
                data=json.dumps(
                    {"query": "needle", "user_id": "user", "top_k": 1}
                ).encode(),
                method="POST",
                headers={"Content-Type": "application/json"},
            )
            with self.assertRaises(urllib.error.HTTPError) as caught:
                urllib.request.urlopen(request, timeout=10)
            self.assertEqual(caught.exception.code, 401)
            caught.exception.close()
            request.add_header("Authorization", "Bearer secret-test-key")
            with urllib.request.urlopen(request, timeout=10) as response:
                self.assertEqual(response.status, 200)
        finally:
            protected.shutdown()
            protected.server_close()
            thread.join(timeout=5)


class LongMemEvalV2Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        configured = os.environ.get("LME_V2_ROOT")
        if not configured:
            raise unittest.SkipTest("LME_V2_ROOT is not configured")
        cls.upstream = Path(configured).resolve()
        cls.patch = Path(__file__).with_name("longmemeval_v2.patch")
        patch_check = subprocess.run(
            ["git", "-C", str(cls.upstream), "apply", "--check", str(cls.patch)],
            capture_output=True,
            text=True,
        )
        if patch_check.returncode != 0:
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(cls.upstream),
                    "apply",
                    "--reverse",
                    "--check",
                    str(cls.patch),
                ],
                check=True,
                capture_output=True,
                text=True,
            )
        sys.path.insert(0, str(cls.upstream))
        importlib.invalidate_caches()
        importlib.import_module("memory_modules")
        module = importlib.import_module("benchmarks.agent_memory.longmemeval_v2")
        cls.LwcMemory = module.LwcMemory
        from memory_modules.memory import MEMORY_TYPES, load_memory

        cls.load_memory = staticmethod(load_memory)
        if MEMORY_TYPES.get("lwc") is not cls.LwcMemory:
            raise AssertionError("LwcMemory was not registered as memory_type=lwc")

    @classmethod
    def tearDownClass(cls) -> None:
        upstream = str(getattr(cls, "upstream", ""))
        if upstream in sys.path:
            sys.path.remove(upstream)

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def memory(self, name: str = "state"):
        return self.LwcMemory(
            {
                "state_root": str((self.root / name).resolve()),
                "lwc_binary": str(lwc_binary()),
                "search_limit": 3,
                "command_timeout_seconds": 30,
            }
        )

    @staticmethod
    def trajectory(trajectory_id: str, needle: str) -> dict[str, object]:
        return {
            "id": trajectory_id,
            "goal": f"Locate {needle}",
            "outcome": f"Found {needle} in the cedar panel.",
            "start_url": "https://example.test/start",
            "states": [
                {
                    "url": "https://example.test/item",
                    "action": "open the cedar panel",
                    "thought": "inspect the labelled compartment",
                    "accessibility_tree": f"The stored marker is {needle}.",
                    "screenshot": "/fixture/trajectory-screen.png",
                }
            ],
            "answer_gold": "private evaluator answer must not be indexed",
            "eval_function": "private evaluator function must not be indexed",
        }

    def test_insert_query_preserves_trajectory_without_gold_or_query_image(self) -> None:
        memory = self.memory()
        memory.insert(self.trajectory("trajectory-a", "indigo-orchid"))

        context = memory.query(
            "Where is the indigo orchid?",
            query_image="/private/question-image.png",
        )

        self.assertTrue(context)
        self.assertTrue(all(item["type"] == "text" and item["value"] for item in context))
        indexed = "\n".join(
            path.read_text(encoding="utf-8")
            for path in (self.root / "state").rglob("sources/*.md")
        )
        self.assertIn("open the cedar panel", indexed)
        self.assertIn("indigo-orchid", indexed)
        self.assertNotIn("private evaluator answer", indexed)
        self.assertNotIn("private evaluator function", indexed)
        self.assertNotIn("/private/question-image.png", "\n".join(item["value"] for item in context))

    def test_instances_are_isolated_even_with_one_state_root(self) -> None:
        first = self.memory("shared")
        second = self.memory("shared")
        self.assertNotEqual(first.scope_id, second.scope_id)
        first.insert(self.trajectory("trajectory-a", "indigo-orchid"))
        second.insert(self.trajectory("trajectory-b", "amber-comet"))

        evidence = first.query("amber comet")

        self.assertFalse(any("amber-comet" in item["value"] for item in evidence))

    def test_saved_memory_relocates_and_loads_backend(self) -> None:
        state_root = (self.root / "creator-state").resolve()
        memory = self.memory("creator-state")
        memory.insert(self.trajectory("trajectory-a", "indigo-orchid"))
        saved = self.root / "saved"
        memory.save_memory(saved)
        config_text = (saved / "memory_config.json").read_text(encoding="utf-8")
        self.assertNotIn(str(state_root), config_text)

        moved = self.root / "moved"
        shutil.move(saved, moved)
        loaded = self.load_memory(moved)

        self.assertEqual(loaded.backend.state_root, (moved / "lwc_state").resolve())
        self.assertTrue(loaded.query("indigo orchid"))


if __name__ == "__main__":
    unittest.main()
