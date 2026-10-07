"""Run against a candidate binary in an isolated directory; never a business server."""
import argparse
import json
import os
import pathlib
import socket
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--assets", required=True)
    parser.add_argument("--keep-for-browser", action="store_true")
    args = parser.parse_args()
    root = pathlib.Path(tempfile.mkdtemp(prefix="lwc-lifecycle-"))
    root.chmod(0o700)
    binary = str(pathlib.Path(args.binary).resolve())
    env = dict(os.environ, HOME=str(root / "client"))
    pathlib.Path(env["HOME"]).mkdir(mode=0o700)
    def cli(*words, stdin=None, success=True):
        result = subprocess.run([binary, *words], cwd=root, env=env, input=stdin, text=True, capture_output=True, timeout=90)
        assert (result.returncode == 0) == success, (words, result.stdout, result.stderr)
        return json.loads(result.stdout or result.stderr)
    initial = cli("server", "init", "--data", str(root / "data"), "--admin-email", "owner@example.com")
    key = (root / "data/administrator.key").read_text().strip()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0)); port = sock.getsockname()[1]
    origin = f"http://127.0.0.1:{port}"
    config = root / "server.jsonc"
    config.write_text(json.dumps({"data": str(root / "data"), "listen": f"127.0.0.1:{port}", "public_url": origin, "admin_assets": str(pathlib.Path(args.assets).resolve())}))
    log = (root / "server.log").open("w")
    process = subprocess.Popen([binary, "server", "run", "--config", str(config)], cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    token = None
    def call(path, body=None, status=200, credential=None):
        headers = {"X-LWC-Server-Token": key, "Content-Type": "application/json", "Origin": origin}
        if credential or token: headers["Authorization"] = "Bearer " + (credential or token)
        request = urllib.request.Request(origin + path, headers=headers, data=json.dumps(body).encode() if body is not None else None)
        try:
            response = http.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            response = error
        assert response.code == status, (path, response.code, status)
        return json.load(response)
    passed = False
    try:
        for _ in range(100):
            try: call("/health"); break
            except OSError: time.sleep(0.1)
        token = call("/api/auth/key", {"key": key, "cli": True})["access_token"]
        team = initial["team_id"]
        manage = lambda body: call("/api/manage", body)
        a = manage({"action": "space.create", "team_id": team, "name": "Acceptance A"})["id"]
        b = manage({"action": "space.create", "team_id": team, "name": "Independently deleted"})["id"]
        project = manage({"action": "project.create", "team_id": team, "name": "Retained directory"})["id"]
        manage({"action": "project.spaces", "id": project, "expected_revision": 1, "items": [a, b]})
        reader = manage({"action": "member.create", "team_id": team, "name": "Reader", "grants": [{"space_id": a, "role": "viewer", "expected_revision": 1}]})
        reader_token = call("/api/auth/key", {"key": reader["personal_key"], "cli": True})["access_token"]
        other_team = call("/api/manage", {"action": "team.create", "name": "Separate team"}, credential=reader_token)["id"]
        other_space = call("/api/manage", {"action": "space.create", "team_id": other_team, "name": "Separate memory"}, credential=reader_token)["id"]
        call("/api/manage", {"action": "team.delete.preview", "team_id": other_team}, status=403)
        call("/api/manage", {"action": "space.delete.preview", "space_id": other_space}, status=403)
        revision = next(s["revision"] for s in call("/api/me")["spaces"] if s["id"] == a)
        call("/api/manage", {"action": "space.delete", "space_id": a, "expected_revision": revision}, status=403, credential=reader_token)
        cli("config", "server", "--server", origin, "--token-stdin", stdin=key)
        cli("login", "--server", origin, "--key-stdin", stdin=key)
        cli("space", "join", a, "--server", origin, "--manual")
        body = "# Retained\n\nOriginal content must survive deletion and restore.\n"
        cli("--space", a, "page", "put", "retained", "--title", "Retained", "--file", "-", "--provenance", "agent-observed", stdin=body)
        cli("space", "sync", a)
        old_head = call(f"/api/spaces/{a}/head")
        cli("space", "bind", a)
        cli("--space", a, "page", "put", "pending", "--title", "Pending", "--file", "-", "--provenance", "agent-observed", stdin="Unsent content survives")
        archived = manage({"action": "space.delete", "space_id": a, "expected_revision": revision})
        assert manage({"action": "space.delete", "space_id": a, "expected_revision": archived["revision"]})["unchanged"]
        call("/api/manage", {"action": "space.restore", "space_id": a, "expected_revision": revision}, status=409)
        assert call(f"/api/spaces/{a}/query", {"action": "get", "slug": "retained"}, status=410)["error"]["code"] == "space_deleted"
        stale = {"protocol": "lwc-team-sync/1", "share_schema": 1, "server_epoch": old_head["server_epoch"], "expected_head": old_head["head"], "replica_id": "1" * 64, "batch_id": "2" * 64, "artifact_id": "3" * 64, "payload_digest": "4" * 64}
        call(f"/api/spaces/{a}/push", stale, status=410)
        cli("space", "sync", a, success=False)
        local = cli("--space", a, "page", "show", "retained")
        assert local["page"]["body"] == body
        assert "replica.resource.deleted" in json.dumps(local)
        hook = cli("agent", "hook", "--agent", "codex", "--event", "SessionStart", stdin=json.dumps({"cwd": str(root), "session_id": "019cb37c-7608-744c-a692-50bf2f5c63a7"}))
        assert "replica.resource.deleted" in json.dumps(hook), hook
        assert "replica.conflict.required" not in json.dumps(hook)
        cli("--space", a, "page", "put", "blocked", "--title", "Blocked", "--file", "-", "--provenance", "agent-observed", stdin="Must reject", success=False)
        manage({"action": "space.restore", "space_id": a, "expected_revision": archived["revision"]})
        cli("space", "sync", a)
        assert call(f"/api/spaces/{a}/query", {"action": "get", "slug": "retained"})["data"]["page"]["body"] == body
        assert call(f"/api/spaces/{a}/query", {"action": "get", "slug": "pending"})["data"]["page"]["body"] == "Unsent content survives"
        old_head = call(f"/api/spaces/{a}/head")
        manage({"action": "space.delete", "space_id": b, "expected_revision": 1})
        invitation = manage({"action": "invitation.create", "team_id": team, "email": "invitee@example.com"})
        team_revision = next(t["revision"] for t in call("/api/admin?view=teams")["rows"] if t["id"] == team)
        deleted = manage({"action": "team.delete", "team_id": team, "expected_revision": team_revision})
        assert call("/api/me")["spaces"] == []
        call(f"/api/spaces/{other_space}/head", credential=reader_token)
        assert call(f"/api/spaces/{a}/head", status=410)["error"]["code"] == "team_deleted"
        assert any(r["id"] == team for r in call("/api/admin?view=trash")["rows"])
        assert call("/api/admin?view=trash", credential=reader_token)["rows"] == []
        manage({"action": "team.restore", "team_id": team, "expected_revision": deleted["revision"]})
        call("/api/invitations/preview", {"invitation_token": invitation["invitation_token"]}, status=400)
        visible = {s["id"] for s in call("/api/me")["spaces"]}
        assert a in visible and b not in visible
        assert [r["id"] for r in call("/api/admin?view=project_spaces&scope=" + project)["rows"]] == [a]
        cli("space", "sync", a)
        assert call(f"/api/spaces/{a}/head")["head"] == old_head["head"]
        cli("--space", a, "page", "put", "after-restore", "--title", "Restored", "--file", "-", "--provenance", "agent-observed", stdin="New writes work after restore")
        cli("space", "sync", a)
        assert call(f"/api/spaces/{a}/query", {"action": "get", "slug": "after-restore"})["data"]["page"]["body"] == "New writes work after restore"
        cli("logout", "--server", origin)
        passed = True
        print(json.dumps({"passed": True, "cases": ["reader denied", "CAS and idempotence", "read/push reject deleted resource", "local memory retained and writes blocked", "restore and sync preserve head/body", "team cascade and independent deletion", "trash authorization", "old invitation remains invalid", "catalog preserved", "unsent content retained", "new writes after restore", "CLI and Hook deletion signal", "cross-team isolation"], "origin": origin, "private_fixture": str(root), "server_pid": process.pid}))
    finally:
        if not args.keep_for_browser or not passed:
            process.terminate(); process.wait(timeout=15)
        log.close()
        if passed and not args.keep_for_browser:
            shutil.rmtree(root)


if __name__ == "__main__":
    main()
