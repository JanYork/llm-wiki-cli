"""External acceptance driver: only explicit isolated containers and synthetic data.
Run on the authorized test host after deploying the two QA services. No secrets emitted.
"""
import concurrent.futures as futures
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path.home() / "lwc-sync-qa-20261006"
BIN = ROOT / "bin/lwc-linux-final"
REPORT = json.loads((ROOT / "acceptance.json").read_text()) if (ROOT / "acceptance.json").exists() else {"cases": [], "timings": [], "scope": "isolated synthetic data"}
if "failure" in REPORT:
    REPORT.setdefault("previous_failures", []).append(REPORT.pop("failure"))


def docker(*args, input=None, check=True):
    return subprocess.run(["sudo", "-n", "docker", *args], input=input, text=True,
                          capture_output=True, check=check)


class API:
    def __init__(self, name, port):
        self.name = name
        self.origin = f"http://127.0.0.1:{port}"
        self.gate = docker("exec", name, "cat", "/data/server-access.token").stdout.strip()
        key = docker("exec", name, "cat", "/data/administrator.key").stdout.strip()
        self.token = None
        self.login = self.call("/api/auth/key", {"key": key, "cli": True})
        self.token = self.login["access_token"]
        teams = self.call("/api/admin?view=teams")["rows"]
        owned = [t for t in teams if t["name"] == "Sync QA" and t["role"] == "owner"]
        self.team = owned[0]["id"] if owned else self.call("/api/manage", {"action": "team.create", "name": "Sync QA"})["id"]

    def call(self, path, body=None, method=None, expected=200, raw=None):
        content = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
        headers = {"X-LWC-Server-Token": self.gate, "Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        req = urllib.request.Request(self.origin + path, data=content, headers=headers,
                                     method=method or ("POST" if content is not None else "GET"))
        started = time.monotonic()
        try:
            response = urllib.request.urlopen(req, timeout=120)
        except urllib.error.HTTPError as error:
            response = error
        status = response.code
        data = response.read()
        elapsed = time.monotonic() - started
        REPORT["timings"].append({"path": path.split("/")[-1], "seconds": elapsed, "status": status})
        decoded = json.loads(data) if data else {}
        if status != expected:
            code = decoded.get("error", {}).get("code", "unexpected_status")
            raise AssertionError(f"{path.split('/')[-1]}: {status} {code}, expected {expected}")
        return decoded

    def space(self, name):
        return self.call("/api/manage", {"action": "space.create", "team_id": self.team, "name": name})["id"]

    def fixture(self, space, count):
        root = f"/api/spaces/{space}"
        replica = self.call(root + "/replicas", {"device": "Synthetic fixture", "device_id": secrets.token_hex(32)})
        transfer = json.loads((ROOT / f"fixtures/{count}.json").read_text())
        upload = self.call(root + "/uploads", {"replica_id": replica["replica_id"], "request_id": secrets.token_hex(32), "transfer": transfer})
        self.call(root + "/uploads/" + upload["artifact_id"], method="PUT", raw=(ROOT / f"fixtures/{count}.bin").read_bytes())
        batch = {"protocol": "lwc-team-sync/1", "share_schema": 1,
                 "server_epoch": replica["head"]["server_epoch"], "expected_head": replica["head"]["head"],
                 "replica_id": replica["replica_id"], "batch_id": secrets.token_hex(32),
                 "artifact_id": upload["artifact_id"], "payload_digest": transfer["state_digest"]}
        return root, replica, batch


def cli(home, args, stdin=None, okay=True):
    home.mkdir(parents=True, exist_ok=True)
    home.chmod(0o700)
    env = {**os.environ, "HOME": str(home), "USERPROFILE": str(home)}
    env.pop("LWC_PROJECT_ROOT", None)
    env.pop("LWC_TEAM_CREDENTIALS_FILE", None)
    started = time.monotonic()
    done = subprocess.run([str(BIN), *args], input=stdin, text=True, capture_output=True,
                          cwd=ROOT, env=env, timeout=120)
    REPORT["timings"].append({"cli": args[0], "seconds": time.monotonic() - started, "status": done.returncode})
    if okay and done.returncode:
        try:
            code = json.loads(done.stderr).get("error", {}).get("code", "cli_failed")
        except ValueError:
            code = "cli_failed"
        raise AssertionError(f"CLI {args[0]}: {code}")
    if not okay:
        return done
    return json.loads(done.stdout)


def client(api, name, space):
    home = ROOT / "clients" / name
    if any(home.rglob("replica.json")):
        return home
    cli(home, ["config", "server", "--server", api.origin, "--token-stdin"], api.gate)
    key = getattr(api, "personal_key", None) or docker("exec", api.name, "cat", "/data/administrator.key").stdout.strip()
    cli(home, ["login", "--server", api.origin, "--key-stdin"], key)
    cli(home, ["space", "join", space, "--server", api.origin, "--manual"])
    return home


def put(home, space, slug, body):
    path = ROOT / "clients" / (home.name + "-body.md")
    path.write_text(body)
    return cli(home, ["--space", space, "page", "put", slug, "--title", slug,
                      "--file", str(path), "--provenance", "agent-observed"])


def case(name, fn):
    if any(c["name"] == name and c["status"] == "passed" for c in REPORT["cases"]):
        return
    started = time.monotonic()
    fn()
    REPORT["cases"].append({"name": name, "status": "passed", "seconds": time.monotonic() - started})
    (ROOT / "acceptance.json").write_text(json.dumps(REPORT, indent=2))
    print(name + ": passed", flush=True)


def wait_hit(stage):
    deadline = time.monotonic() + 20
    while docker("exec", "lwc-sync-qa-fault", "test", "-f", f"/faults/{stage}.hit", check=False).returncode:
        if time.monotonic() > deadline:
            raise AssertionError("fault barrier not reached")
        time.sleep(0.1)


def arm(stage, action):
    docker("exec", "lwc-sync-qa-fault", "sh", "-c", f"rm -f /faults/{stage}.hit /faults/{stage}.resume")
    docker("exec", "-i", "lwc-sync-qa-fault", "sh", "-c", f"cat > /faults/{stage}.arm", input=action)


def main():
    api = API("lwc-sync-qa-final", 8790)
    spaces = {}
    existing = api.call("/api/me")["spaces"]
    for count in (4500, 45000):
        previous = [v for v in existing if v["name"] == f"Synthetic {count}"]
        if previous:
            spaces[count] = previous[0]["id"]
            continue
        space = api.space(f"Synthetic {count}")
        root, _, batch = api.fixture(space, count)
        def accept(root=root, batch=batch, space=space, count=count):
            receipt = api.call(root + "/push", batch)
            assert receipt["team"]["accepted_head"] == 1
            assert api.call(root + "/push", batch)["team"]["accepted_head"] == 1
            pages = api.call(root + "/query", {"action": "list", "limit": 100, "offset": 0})
            assert len(pages["data"]["pages"]) == 100
            assert api.call(root + "/head")["digest"] == batch["payload_digest"]
        case(f"full_{count}_idempotency", accept)
        spaces[count] = space
    space = spaces[4500]
    a, b = client(api, "a", space), client(api, "b", space)
    def roundtrip():
        put(a, space, "new-record", "Real CLI evidence")
        cli(a, ["space", "sync", space])
        cli(b, ["space", "sync", space])
        assert cli(b, ["--space", space, "page", "show", "new-record"])["page"]["body"] == "Real CLI evidence"
    case("real_cli_incremental_roundtrip", roundtrip)
    def concurrent():
        homes = [a, b] + [client(api, f"c{i}", space) for i in range(2)]
        for i, home in enumerate(homes):
            put(home, space, f"parallel-{i}", f"Independent writer {i}")
        with futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(lambda home: cli(home, ["space", "sync", space]), homes))
        for _ in range(2):
            for home in homes:
                cli(home, ["space", "sync", space])
        for i in range(4):
            assert cli(a, ["--space", space, "page", "show", f"parallel-{i}"])["page"]["body"] == f"Independent writer {i}"
    case("four_clients_same_space_convergence", concurrent)
    def large_incremental():
        home = client(api, "large", spaces[45000])
        put(home, spaces[45000], "single-delta", "Small write in a large inventory")
        cli(home, ["space", "sync", spaces[45000]])
        read = api.call(f"/api/spaces/{spaces[45000]}/query", {"action": "get", "slug": "single-delta"})
        assert read["data"]["page"]["body"] == "Small write in a large inventory"
    case("45000_objects_single_delta", large_incremental)
    fault = API("lwc-sync-qa-fault", 8791)
    def barrier():
        space = fault.space("Paused submission")
        root, _, batch = fault.fixture(space, 4500)
        arm("before-commit", "pause")
        with futures.ThreadPoolExecutor(max_workers=1) as pool:
            pending = pool.submit(fault.call, root + "/push", batch)
            wait_hit("before-commit")
            assert fault.call(root + "/head")["head"] == 0
            other = fault.space("Independent during pause")
            other_root, _, other_batch = fault.fixture(other, 4500)
            assert fault.call(other_root + "/push", other_batch)["team"]["accepted_head"] == 1
            # Hold beyond the old five-second busy timeout after proving progress.
            time.sleep(6)
            docker("exec", "lwc-sync-qa-fault", "touch", "/faults/before-commit.resume")
            assert pending.result()["team"]["accepted_head"] == 1
    case("slow_prepare_same_space_reads_other_space_writes", barrier)
    def crash(stage, expected_head):
        space = fault.space("Crash " + stage)
        root, replica, batch = fault.fixture(space, 4500)
        arm(stage, "exit")
        try:
            fault.call(root + "/push", batch)
        except (OSError, AssertionError):
            pass
        docker("start", "lwc-sync-qa-fault")
        deadline = time.monotonic() + 20
        while True:
            try:
                head = fault.call(root + "/head")
                break
            except (OSError, AssertionError):
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.2)
        assert head["head"] == expected_head
        receipt = fault.call(root + f"/receipts/{replica['replica_id']}/{batch['batch_id']}")["receipt"]
        assert (receipt is not None) == bool(expected_head)
        assert fault.call(root + "/push", batch)["team"]["accepted_head"] == 1
        assert fault.call(root + "/head")["head"] == 1
        found = fault.call(root + "/query", {"action": "search", "query": "Synthetic evidence", "limit": 10})
        assert found["data"]["results"]
    case("crash_before_commit_rolls_back", lambda: crash("before-commit", 0))
    case("crash_after_commit_recovers_receipt_and_indexes", lambda: crash("after-commit", 1))
    def malformed():
        space = api.space("Rejected malformed upload")
        root, _, batch = api.fixture(space, 4500)
        changed = dict(batch, payload_digest="f" * 64)
        api.call(root + "/push", changed, expected=400)
        assert api.call(root + "/head")["head"] == 0
    case("wrong_digest_never_publishes", malformed)
    print(json.dumps({"passed": len(REPORT["cases"]), "report": str(ROOT / "acceptance.json")}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        REPORT["failure"] = type(error).__name__ + ": " + str(error)
        (ROOT / "acceptance.json").write_text(json.dumps(REPORT, indent=2))
        raise
