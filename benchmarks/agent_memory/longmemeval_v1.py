from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time
from typing import Any

from benchmarks.agent_memory.lwc_backend import AdapterError, LwcBackend


_CUTOFFS = (1, 3, 5, 10, 30, 50)


def evaluate_dataset(
    *,
    data_path: Path,
    state_root: Path,
    output_path: Path,
    upstream_revision: str,
    lwc_commit: str,
    binary: str | Path = "lwc",
    limit: int | None = None,
) -> dict[str, Any]:
    data_path = Path(data_path).resolve()
    output_path = Path(output_path).resolve()
    state_root = Path(state_root).resolve()
    if not data_path.is_file():
        raise AdapterError(f"dataset does not exist: {data_path}")
    if not upstream_revision.strip():
        raise AdapterError("upstream_revision must be non-empty")
    if not lwc_commit.strip():
        raise AdapterError("lwc_commit must be non-empty")
    if limit is not None and (isinstance(limit, bool) or limit < 1):
        raise AdapterError("limit must be a positive integer")

    raw = data_path.read_bytes()
    dataset_sha256 = hashlib.sha256(raw).hexdigest()
    dataset = json.loads(raw)
    if not isinstance(dataset, list) or not dataset:
        raise AdapterError("dataset must be a non-empty JSON array")
    selected = dataset[:limit] if limit is not None else dataset
    backend = LwcBackend(state_root, binary=binary)
    records: list[dict[str, Any]] = []
    scored: list[dict[str, Any]] = []

    for entry in selected:
        record, measurement = _evaluate_entry(backend, dataset_sha256, entry)
        records.append(record)
        if measurement is not None:
            scored.append(measurement)

    log_path = output_path.with_suffix(".jsonl")
    _write_atomic(
        log_path,
        "".join(json.dumps(record, ensure_ascii=False) + "\n" for record in records),
    )
    report = {
        "benchmark": "LongMemEval-S",
        "mode": "session-source-retrieval",
        "complete": limit is None and len(records) == len(dataset),
        "partial": limit is not None or len(records) != len(dataset),
        "instances_total": len(dataset),
        "instances_processed": len(records),
        "instances_retrieval_scored": len(scored),
        "dataset_path": str(data_path),
        "dataset_sha256": dataset_sha256,
        "upstream_revision": upstream_revision,
        "lwc_version": _lwc_version(binary),
        "lwc_commit": lwc_commit,
        "adapter_config": {
            "state_root": str(state_root),
            "search_limit": 50,
            "source_type": "source",
            "granularity": "passage",
            "scope": "isolated-per-question",
        },
        "retrieval_log": str(log_path),
        "text_only": True,
        "metrics": _aggregate(scored),
    }
    _write_atomic(output_path, json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    return report


def _evaluate_entry(
    backend: LwcBackend,
    dataset_sha256: str,
    entry: object,
) -> tuple[dict[str, Any], dict[str, Any] | None]:
    if not isinstance(entry, dict):
        raise AdapterError("every dataset entry must be an object")
    question_id = _text(entry.get("question_id"), "question_id")
    question = _text(entry.get("question"), "question")
    session_ids = _list(entry.get("haystack_session_ids"), "haystack_session_ids")
    sessions = _list(entry.get("haystack_sessions"), "haystack_sessions")
    dates = _list(entry.get("haystack_dates"), "haystack_dates")
    if not (len(session_ids) == len(sessions) == len(dates)):
        raise AdapterError(f"haystack arrays differ in length for {question_id}")

    memories = []
    date_by_memory_id: dict[str, object] = {}
    for occurrence, (session_id, session, date) in enumerate(
        zip(session_ids, sessions, dates, strict=True)
    ):
        session_id = _text(session_id, "haystack_session_id")
        if not isinstance(session, list) or not session:
            raise AdapterError(f"session {session_id} must be a non-empty message list")
        messages = []
        for message in session:
            if not isinstance(message, dict):
                raise AdapterError(f"session {session_id} contains a non-object message")
            normalized = dict(message)
            normalized.setdefault("timestamp", date)
            messages.append(normalized)
        memory_id = f"{session_id}:{occurrence}"
        date_by_memory_id[memory_id] = date
        memories.append((memory_id, session_id, messages))

    scope_id = f"longmemeval-v1:{dataset_sha256}:{question_id}"
    backend.add_many(scope_id, memories)
    started = time.perf_counter()
    evidence = backend.search(scope_id, question, 50)
    latency_ms = (time.perf_counter() - started) * 1000.0
    ranked_ids = [item.session_id for item in evidence if item.session_id]
    ranked_items = [
        {
            "corpus_id": item.session_id,
            "text": item.content,
            "timestamp": date_by_memory_id.get(item.id),
        }
        for item in evidence
        if item.session_id
    ]
    answers = [str(value) for value in _list(entry.get("answer_session_ids"), "answer_session_ids")]
    metrics = _entry_metrics(ranked_ids, answers)
    record = dict(entry)
    record["retrieval_results"] = {
        "query": question,
        "ranked_items": ranked_items,
        "metrics": {"session": metrics, "turn": {}},
        "latency_ms": round(latency_ms, 3),
    }
    measurement = None
    if not question_id.endswith("_abs"):
        first_hit = next(
            (index + 1 for index, session_id in enumerate(ranked_ids) if session_id in answers),
            None,
        )
        measurement = {
            "latency_ms": latency_ms,
            "hit_at_5": any(value in answers for value in ranked_ids[:5]),
            "hit_at_10": any(value in answers for value in ranked_ids[:10]),
            "reciprocal_rank": 0.0 if first_hit is None else 1.0 / first_hit,
        }
    return record, measurement


def _entry_metrics(ranked_ids: list[str], answers: list[str]) -> dict[str, float]:
    metrics: dict[str, float] = {}
    for cutoff in _CUTOFFS:
        selected = ranked_ids[:cutoff]
        metrics[f"recall_any@{cutoff}"] = float(any(item in answers for item in selected))
        metrics[f"recall_all@{cutoff}"] = float(
            bool(answers) and all(item in selected for item in answers)
        )
        metrics[f"ndcg_any@{cutoff}"] = _ndcg(selected, answers, cutoff)
    return metrics


def _ndcg(ranked_ids: list[str], answers: list[str], cutoff: int) -> float:
    actual = 0.0
    for index, item in enumerate(ranked_ids[:cutoff]):
        if item in answers:
            actual += 1.0 if index == 0 else 1.0 / math.log2(index + 1)
    ideal_hits = min(len(set(answers)), cutoff)
    ideal = sum(1.0 if index == 0 else 1.0 / math.log2(index + 1) for index in range(ideal_hits))
    return 0.0 if ideal == 0 else actual / ideal


def _aggregate(measurements: list[dict[str, Any]]) -> dict[str, Any]:
    if not measurements:
        return {
            "recall_at_5": 0.0,
            "recall_at_10": 0.0,
            "mrr": 0.0,
            "latency_ms": {"p50": 0.0, "p95": 0.0},
        }
    count = len(measurements)
    return {
        "recall_at_5": round(sum(item["hit_at_5"] for item in measurements) / count, 6),
        "recall_at_10": round(sum(item["hit_at_10"] for item in measurements) / count, 6),
        "mrr": round(sum(item["reciprocal_rank"] for item in measurements) / count, 6),
        "latency_ms": {
            "p50": round(_percentile([item["latency_ms"] for item in measurements], 50), 3),
            "p95": round(_percentile([item["latency_ms"] for item in measurements], 95), 3),
        },
    }


def _percentile(values: list[float], percentile: int) -> float:
    ordered = sorted(values)
    index = math.ceil((len(ordered) - 1) * percentile / 100)
    return ordered[index]


def _lwc_version(binary: str | Path) -> str:
    try:
        completed = subprocess.run(
            [str(binary), "--version"],
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise AdapterError("unable to read lwc version") from error
    return completed.stdout.strip()


def _text(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise AdapterError(f"{name} must be a non-empty string")
    return value


def _list(value: object, name: str) -> list[Any]:
    if not isinstance(value, list):
        raise AdapterError(f"{name} must be a list")
    return value


def _write_atomic(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, delete=False
    ) as temporary:
        temporary.write(content)
        temporary_path = Path(temporary.name)
    os.replace(temporary_path, path)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run LWC retrieval on LongMemEval-S")
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--state-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--upstream-revision", required=True)
    parser.add_argument("--lwc-commit", required=True)
    parser.add_argument("--lwc-binary", default="lwc")
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    report = evaluate_dataset(
        data_path=args.data,
        state_root=args.state_root,
        output_path=args.output,
        upstream_revision=args.upstream_revision,
        lwc_commit=args.lwc_commit,
        binary=args.lwc_binary,
        limit=args.limit,
    )
    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
