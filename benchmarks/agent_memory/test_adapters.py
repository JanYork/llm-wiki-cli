import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest

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
            binary=lwc_binary(),
            limit=1,
        )

        self.assertFalse(report["complete"])
        self.assertTrue(report["partial"])
        self.assertEqual(report["instances_processed"], 1)


if __name__ == "__main__":
    unittest.main()
