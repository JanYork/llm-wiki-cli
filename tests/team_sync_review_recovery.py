"""Changed-path crash, revocation and fixed-window regressions; synthetic QA only."""
import concurrent.futures
import json
import os
import time

import team_sync_remote_acceptance as qa

qa.BIN = qa.ROOT / "bin/lwc-linux-review"
qa.REPORT = {"cases": [], "timings": [], "scope": "20261007 isolated review recovery"}
import team_sync_extreme_acceptance as previous

OUTPUT = qa.ROOT / "review-recovery.json"


def case(name, operation):
    operation()
    qa.REPORT["cases"].append({"name": name, "status": "passed"})
    OUTPUT.write_text(json.dumps(qa.REPORT, indent=2))
    print(name + ": passed", flush=True)


def selected(name, operation):
    if name.startswith("prepare_then_revoke_") or name == "same_batch_concurrent_replay_and_lost_response_receipt":
        case("review_" + name, operation)


def main():
    previous.case = selected
    previous.main()
    fault = qa.API("lwc-sync-qa-fault", 8791)
    for stage, expected in [("before-commit", 0), ("after-commit", 1)]:
        def crash(stage=stage, expected=expected):
            space = fault.space("Review " + stage)
            root, replica, batch = fault.fixture(space, 4500)
            qa.arm(stage, "exit")
            try:
                fault.call(root + "/push", batch)
            except (OSError, AssertionError):
                pass
            qa.docker("start", "lwc-sync-qa-fault")
            deadline = time.monotonic() + 30
            while True:
                try:
                    head = fault.call(root + "/head")
                    break
                except (OSError, AssertionError):
                    assert time.monotonic() < deadline
                    time.sleep(.2)
            assert head["head"] == expected
            known = fault.call(root + f"/receipts/{replica['replica_id']}/{batch['batch_id']}")["receipt"]
            assert (known is not None) == bool(expected)
            if expected:
                body = fault.call(root + "/query", {"action": "get", "slug": "qa-0"})
                assert body["data"]["page"]["body"] == "Synthetic evidence 0"
            assert fault.call(root + "/push", batch)["team"]["accepted_head"] == 1
            deadline = time.monotonic() + 45
            while True:
                try:
                    found = fault.call(root + "/query", {"action": "search", "query": "Synthetic evidence", "limit": 10})
                    assert found["data"]["results"]
                    break
                except AssertionError as error:
                    assert "sync_derived_unavailable" in str(error), str(error)
                    assert time.monotonic() < deadline, "index recovery did not finish"
                    time.sleep(.5)
        case("review_crash_" + stage, crash)

    def frozen():
        api = qa.API("lwc-sync-qa-final", 8790)
        space = api.space("Review fixed accepted window")
        home = qa.client(api, "review-frozen-" + space[:8], space)
        qa.put(home, space, "frozen", "Accepted window")
        directory = qa.ROOT / "review-client-faults"
        directory.mkdir(mode=0o700, exist_ok=True)
        for suffix in ("hit", "resume"):
            (directory / ("before-upload." + suffix)).unlink(missing_ok=True)
        (directory / "before-upload.arm").write_text("pause")
        old_binary, old_env = qa.BIN, os.environ.get("LWC_SYNC_TEST_DIR")
        qa.BIN = qa.ROOT / "bin/lwc-linux-review-fault"
        os.environ["LWC_SYNC_TEST_DIR"] = str(directory)
        try:
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(qa.cli, home, ["space", "sync", space])
                deadline = time.monotonic() + 45
                while not (directory / "before-upload.hit").exists():
                    assert time.monotonic() < deadline
                    time.sleep(.05)
                qa.put(home, space, "later", "Must remain queued")
                (directory / "before-upload.resume").touch()
                assert pending.result()["head"] == 1
        finally:
            qa.BIN = old_binary
            if old_env is None:
                os.environ.pop("LWC_SYNC_TEST_DIR", None)
            else:
                os.environ["LWC_SYNC_TEST_DIR"] = old_env
        # New CLI processes recover the pending state and submit the next batch.
        assert qa.cli(home, ["space", "sync", space])["head"] == 2
        assert qa.cli(home, ["space", "sync", space])["status"] == "current"
        assert api.call(f"/api/spaces/{space}/query", {"action": "get", "slug": "later"})["data"]["page"]["body"] == "Must remain queued"
    case("review_fixed_window_later_write_restart", frozen)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        qa.REPORT["failure"] = type(error).__name__ + ": " + str(error)
        OUTPUT.write_text(json.dumps(qa.REPORT, indent=2))
        raise
    finally:
        qa.docker("stop", "lwc-sync-qa-fault", check=False)
