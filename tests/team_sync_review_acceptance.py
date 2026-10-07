"""Bounded review regressions on the explicitly isolated Linux candidate only."""
import concurrent.futures
import hashlib
import json
import os
import signal
import sqlite3
import ssl
import sys
import threading
import time
import urllib.request

import team_sync_remote_acceptance as qa
from team_sync_stress_acceptance import member, percentile

qa.BIN = qa.ROOT / "bin/lwc-linux-review"
qa.REPORT = {"cases": [], "timings": [], "scope": "20261007 isolated review candidate"}
REPORT = qa.REPORT
OUTPUT = qa.ROOT / "review-acceptance.json"


def save():
    OUTPUT.write_text(json.dumps(REPORT, indent=2))


def pages(home):
    path = next(home.rglob("wiki.db"))
    with sqlite3.connect("file:" + str(path) + "?mode=ro", uri=True) as conn:
        return dict(conn.execute("SELECT slug,body FROM pages"))


def verify_original_objects(home):
    record_path = next(home.rglob("replica.json"))
    record = json.loads(record_path.read_text())
    with sqlite3.connect(qa.ROOT / "fixtures/4500.bin") as conn:
        original = {(kind, key): (json.loads(raw), digest) for kind, key, raw, digest in
                    conn.execute("SELECT kind,logical_key,payload_json,payload_hash FROM sync_objects")}
    snapshot = record_path.parent / "generations" / record["baseline_generation"] / "remote.db"
    with sqlite3.connect(snapshot) as conn:
        accepted = {(kind, key): digest for kind, key, digest in
                    conn.execute("SELECT kind,logical_key,payload_hash FROM sync_objects")}
    assert all(accepted.get(key) == value[1] for key, value in original.items())
    with sqlite3.connect("file:" + str(record_path.parent / "wiki.db") + "?mode=ro", uri=True) as conn:
        for (kind, key), (payload, _) in original.items():
            if kind == "page":
                assert conn.execute("SELECT body FROM pages WHERE slug=?", (key,)).fetchone()[0] == payload["body"]
            elif kind == "source":
                content = conn.execute("SELECT content FROM sources WHERE content_hash=?", (key,)).fetchone()[0]
                assert hashlib.sha256(content.encode()).hexdigest() == key
            elif kind in ("memory_audit", "source_revision"):
                raw = conn.execute("SELECT payload_json FROM replica_history WHERE kind=? AND logical_key=?", (kind, key)).fetchone()[0]
                assert json.loads(raw) == payload
            elif kind == "meta":
                assert conn.execute("SELECT value FROM meta WHERE key=?", (key,)).fetchone()[0] == payload["value"]
            else:
                raise AssertionError("unexpected baseline kind: " + kind)
    return len(original)


def sync(home, space):
    for _ in range(8):
        result = qa.cli(home, ["space", "sync", space])
        if result["status"] != "retry":
            return result
    raise AssertionError("bounded synchronization did not advance")


def stop_worker(home, space):
    # Only this driver's synthetic space and candidate executable are eligible.
    listing = qa.subprocess.check_output(["ps", "-eo", "pid,args"], text=True)
    for row in listing.splitlines():
        fields = row.strip().split(None, 1)
        if len(fields) == 2 and fields[1].startswith(str(qa.BIN) + " space watch ") and space in fields[1]:
            os.kill(int(fields[0]), signal.SIGTERM)
    for _ in range(20):
        result = qa.cli(home, ["space", "configure", space, "--automatic", "false"], okay=False)
        if result.returncode == 0:
            return
        time.sleep(.25)
    raise AssertionError("could not stop synthetic sync worker")


def core(api):
    space = api.space("Review complete core patch")
    subject = member(api, "Review core reader", [space])
    home = qa.client(subject, "review-core-" + space[:8], space)
    root, replica, batch = api.fixture(space, "core")
    accepted = api.call(root + "/push", batch)
    assert accepted["team"]["accepted_head"] == 1
    assert api.call(root + "/push", batch)["team"]["accepted_head"] == 1
    # Joined empty, then receives every core kind through the new patch path.
    assert sync(home, space)["status"] == "synced"
    assert sync(home, space)["status"] == "current"
    record = next(home.rglob("replica.json"))
    directory = record.parent
    generation = json.loads(record.read_text())["baseline_generation"]
    with sqlite3.connect(qa.ROOT / "fixtures/core.db") as conn:
        expected = dict(((k, key), digest) for k, key, digest in conn.execute("SELECT kind,logical_key,payload_hash FROM sync_objects"))
    with sqlite3.connect(directory / "generations" / generation / "remote.db") as conn:
        actual = dict(((k, key), digest) for k, key, digest in conn.execute("SELECT kind,logical_key,payload_hash FROM sync_objects"))
    assert expected == actual
    # A subsequent export/upload exposes canonical content, not just saved files.
    qa.put(home, space, "after-core", "All original objects must remain")
    sync(home, space)
    kinds = sorted({k for k, _ in expected})
    for kind in kinds:
        rows = api.call(root + "/query", {"action": "objects", "kind": kind, "limit": 100, "offset": 0})["data"]["objects"]
        found = {(row["kind"], row["key"]): row["hash"] for row in rows}
        for key, digest in expected.items():
            if key[0] == kind:
                assert found.get(key) == digest, key
    receipt = api.call(root + f"/receipts/{replica['replica_id']}/{batch['batch_id']}")["receipt"]
    assert receipt["team"]["accepted_digest"] == batch["payload_digest"]
    REPORT["core_kinds"] = kinds


def continuous(api):
    space = api.space("Review ongoing bidirectional edits")
    root, _, batch = api.fixture(space, 4500)
    api.call(root + "/push", batch)
    homes = [qa.client(member(api, "Review writer " + str(i), [space]), "review-writer-" + space[:8] + str(i), space) for i in range(2)]
    health, admin, writes, progress = [], [], [[], []], [[], []]
    failures, stop = [], threading.Event()
    ca = qa.docker("exec", "lwc-team-https-1", "cat", "/data/caddy/pki/authorities/local/root.crt").stdout
    tls = ssl.create_default_context(cadata=ca)
    gate = qa.docker("exec", "lwc-team-memory-1", "cat", "/data/server-access.token").stdout.strip()

    def monitor():
        errors = 0
        while not stop.is_set():
            try:
                tick = time.monotonic()
                request = urllib.request.Request("https://10.10.10.17:8788/health", headers={"X-LWC-Server-Token": gate})
                with urllib.request.urlopen(request, context=tls, timeout=3) as response:
                    assert response.code == 200
                health.append(time.monotonic() - tick)
                errors = 0
                if len(health) > 20 and percentile(health[-20:], .95) > max(2, percentile(health[:10], .95) * 2):
                    raise RuntimeError("live health latency protection")
            except Exception:
                errors += 1
                if errors >= 3:
                    failures.append("live health protection")
                    stop.set()
            stop.wait(2)

    def producer(index):
        until = time.monotonic() + 360
        while not stop.is_set() and time.monotonic() < until:
            if all(len(events) >= 2 for events in progress):
                break
            tick = time.monotonic()
            slug = f"writer-{index}-{len(writes[index])}"
            body = "Exact evidence " + slug
            try:
                qa.put(homes[index], space, slug, body)
                writes[index].append((slug, body, time.monotonic() - tick))
            except Exception as error:
                failures.append(type(error).__name__ + ": " + str(error))
                stop.set()
            stop.wait(max(0, 1 - (time.monotonic() - tick)))

    monitor_thread = threading.Thread(target=monitor, daemon=True)
    monitor_thread.start()
    started, heads, last = time.monotonic(), [], [-1, -1]
    try:
        for home in homes:
            qa.cli(home, ["space", "configure", space, "--automatic", "true", "--interval-ms", "1000"])
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            producers = [pool.submit(producer, i) for i in range(2)]
            while not all(p.done() for p in producers):
                assert not failures, failures
                tick = time.monotonic()
                api.call("/api/admin?view=teams")
                admin.append(time.monotonic() - tick)
                heads.append(api.call(root + "/head")["head"])
                for i, home in enumerate(homes):
                    opposite = "writer-" + str(1 - i) + "-"
                    observed = max((int(key[len(opposite):]) for key in pages(home) if key.startswith(opposite)), default=-1)
                    if observed > last[i] and all(not p.done() for p in producers):
                        progress[i].append({"seconds": round(time.monotonic() - started, 2), "remote_page": observed})
                        last[i] = observed
                if len(heads) % 10 == 0:
                    print(json.dumps({"seconds": round(time.monotonic() - started), "writes": [len(v) for v in writes], "head": heads[-1], "progress": [len(v) for v in progress]}), flush=True)
                stop.wait(2)
            for result in producers:
                result.result()
        assert not failures, failures
        assert all(len(v) >= 2 for v in progress), "both directions must progress repeatedly BEFORE producers stop"
        expected = {slug: body for rows in writes for slug, body, _ in rows}
        deadline = time.monotonic() + 180
        while True:
            current = [pages(home) for home in homes]
            if all(all(rows.get(slug) == body for slug, body in expected.items()) for rows in current):
                break
            assert time.monotonic() < deadline, "backlog did not drain"
            stop.wait(2)
        for home in homes:
            stop_worker(home, space)
        for _ in range(6):
            if all(sync(home, space)["status"] == "current" for home in homes):
                break
        else:
            raise AssertionError("final accepted digests did not settle")
        head = api.call(root + "/head")
        for home in homes:
            record = json.loads(next(home.rglob("replica.json")).read_text())
            assert record["remote_head"]["digest"] == head["digest"]
            rows = pages(home)
            assert all(rows.get(slug) == body for slug, body in expected.items())
            assert verify_original_objects(home) == 4500
        REPORT["continuous"] = {"space": space, "writes": [len(v) for v in writes], "accepted_batches": head["head"] - 1,
            "progress_while_both_producers_active": progress, "total_seconds": time.monotonic() - started,
            "local_write_p95_seconds": [percentile([row[2] for row in rows], .95) for rows in writes],
            "admin_p95_seconds": percentile(admin, .95), "live_health_p95_seconds": percentile(health, .95),
            "final_digest": head["digest"], "all_bodies_equal": True, "original_objects_preserved": 4500}
    finally:
        REPORT["continuous_observation"] = {"space": space, "writes": [len(v) for v in writes],
            "progress": progress, "elapsed_seconds": time.monotonic() - started, "failures": failures}
        stop.set()
        monitor_thread.join(timeout=4)
        for home in homes:
            stop_worker(home, space)


if __name__ == "__main__":
    try:
        owner = qa.API("lwc-sync-qa-final", 8790)
        cases = [("ongoing_bidirectional_progress", continuous)]
        if "--continuous-only" not in sys.argv:
            cases.insert(0, ("all_core_patch_roundtrip", core))
        for name, operation in cases:
            operation(owner)
            REPORT["cases"].append({"name": name, "status": "passed"})
            save()
            print(name + ": passed", flush=True)
    except Exception as error:
        REPORT["failure"] = type(error).__name__ + ": " + str(error)
        save()
        raise
