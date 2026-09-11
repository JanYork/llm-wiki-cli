#!/usr/bin/env python3
"""Exercise native CG and checkout isolation with real source in disposable worktrees.
No downloads, user configuration changes, or production index writes.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import shutil
import signal
import statistics
import subprocess
import tempfile
import time


def run(args, cwd, env, ok=True):
    result = subprocess.run([str(a) for a in args], cwd=cwd, env=env, capture_output=True, timeout=120)
    if ok and result.returncode:
        raise RuntimeError(f"{args}: {result.stderr.decode(errors='replace')[:2000]}")
    return result


class MCP:
    def __init__(self, args, cwd, env):
        self.process = subprocess.Popen([str(a) for a in args], cwd=cwd, env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, start_new_session=True)
        self.sequence = 0
        self.call('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                                'clientInfo': {'name': 'lwc-ux-check', 'version': '1'}})
        self.process.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        self.process.stdin.flush()

    def call(self, method, params):
        self.sequence += 1
        request = {'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params}
        self.process.stdin.write(json.dumps(request).encode() + b'\n')
        self.process.stdin.flush()
        deadline = time.monotonic() + 70
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while time.monotonic() < deadline:
                if not selector.select(max(0, deadline - time.monotonic())):
                    break
                line = self.process.stdout.readline()
                if not line:
                    raise RuntimeError('MCP exited before replying')
                response = json.loads(line)
                if response.get('id') == self.sequence:
                    assert 'error' not in response, response
                    return response['result']
        raise TimeoutError('MCP response deadline')

    def close(self):
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGKILL)
        self.process.wait(timeout=5)


def stats(values):
    values = sorted(values)
    return {'samples': len(values), 'p50_ms': round(statistics.median(values), 2),
            'p95_ms': round(values[math.ceil(.95 * len(values)) - 1], 2)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['candidate', 'baseline', 'codegraph', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    env.pop('LWC_PROJECT_ROOT', None)
    env.pop('LWC_CODEGRAPH_BINARY', None)
    runtime = json.loads(run([args.baseline, 'cg', 'status'], repo, env).stdout)['runtime']
    manifest = json.loads((Path(runtime) / 'runtime.json').read_text())
    bundled = Path(runtime) / manifest['binary']
    report = {'candidate_sha256': hashlib.file_digest(args.candidate.open('rb'), 'sha256').hexdigest(),
              'baseline': run([args.baseline, '--version'], repo, env).stdout.decode().strip(),
              'independent_codegraph': run([args.codegraph, '--version'], repo, env).stdout.decode().strip(),
              'measurement': 'small real-source fixture; observational samples, not a performance guarantee'}
    with tempfile.TemporaryDirectory(prefix='lwc-agent-ux-') as root:
        root = Path(root).resolve()
        home = root / 'home'
        main_tree, worktree = root / 'main', root / 'worktree'
        (main_tree / 'src').mkdir(parents=True)
        (home / '.lwc').mkdir(parents=True)
        # Keep the normal update checker quiet in this disposable HOME.
        (home / '.lwc/update-check.json').write_text(json.dumps({'schema': 1, 'last_attempt': 2**64-1,
                                                               'latest_version': None, 'notified_version': None}))
        env.update(HOME=str(home), USERPROFILE=str(home), CODEGRAPH_TELEMETRY='0', DO_NOT_TRACK='1', NO_COLOR='1')
        for source in ['mcp.rs', 'scope.rs']:
            shutil.copyfile(repo / 'src' / source, main_tree / 'src' / source)
        (main_tree / '.gitignore').write_text('.lwc/\n.codegraph/\n')
        run(['git', 'init', '-q'], main_tree, env)
        run(['git', 'add', '.'], main_tree, env)
        run(['git', '-c', 'user.name=LWC Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture'], main_tree, env)
        run(['git', 'worktree', 'add', '--detach', str(worktree), 'HEAD'], main_tree, env)
        bundled_env = dict(env, LWC_CODEGRAPH_BINARY=str(bundled))
        run([args.baseline, 'init'], main_tree, bundled_env)
        run([args.baseline, 'cg', 'init'], main_tree, bundled_env)
        samples = {}
        for label, binary in [('baseline', args.baseline), ('candidate', args.candidate)]:
            times = []
            for _ in range(5):
                start = time.perf_counter()
                result = run([binary, 'cg', 'query', 'call_codegraph', '--json'], main_tree, bundled_env)
                times.append((time.perf_counter() - start) * 1000)
            samples[label] = stats(times)
            samples[label]['output_bytes'] = len(result.stdout)
            samples[label]['calls_per_query'] = 1
        report['bundled_cli'] = samples
        hook_input = json.dumps({'source': 'startup', 'session_id': 'agent-ux-fixture'}).encode()
        hooks = {}
        for label, binary in [('baseline', args.baseline), ('candidate', args.candidate)]:
            output = subprocess.run([str(binary), 'agent', 'hook', '--agent', 'codex', '--event', 'SessionStart'],
                                    input=hook_input, cwd=main_tree, env=bundled_env, capture_output=True, check=True, timeout=10)
            hooks[label] = {'output_bytes': len(output.stdout)}
        report['hook'] = hooks
        client = MCP([args.candidate, 'serve', '--mcp', '--path', root], main_tree, bundled_env)
        try:
            query = {'command': 'search', 'projectPath': str(main_tree), 'arguments': {'query': 'call_codegraph', 'limit': 5}}
            start = time.perf_counter()
            native_result = client.call('tools/call', {'name': 'lwc_codegraph', 'arguments': query})
            report['mcp_first_query_ms'] = round((time.perf_counter() - start) * 1000, 2)
            assert not native_result.get('isError'), native_result
            warm = []
            for _ in range(20):
                start = time.perf_counter()
                assert client.call('tools/call', {'name': 'lwc_codegraph', 'arguments': query}) == native_result
                warm.append((time.perf_counter() - start) * 1000)
            report['mcp_warm'] = stats(warm)
        finally:
            client.close()
        for tree in [main_tree, worktree]:
            run([args.candidate, 'cg', 'configure', '--executable', args.codegraph.resolve()], tree, env)
        marker = 'checkout_only_marker'
        (worktree / 'src/scope_probe.rs').write_text(f'pub fn {marker}() -> bool {{ true }}\n')
        absent = json.loads(run([args.candidate, 'cg', 'status'], worktree, env).stdout)
        assert not absent['initialized'] and Path(absent['index']) == worktree / '.codegraph'
        for tree in [main_tree, worktree]:
            run([args.candidate, 'cg', 'init'], tree, env)
        direct_env = dict(env, CODEGRAPH_DIR='.codegraph')
        direct = run([args.codegraph, 'query', 'call_codegraph', '--json'], main_tree, direct_env)
        forwarded = run([args.candidate, 'cg', 'query', 'call_codegraph', '--json'], main_tree, env)
        assert (direct.stdout, direct.stderr, direct.returncode) == (forwarded.stdout, forwarded.stderr, forwarded.returncode)
        main_status = json.loads(run([args.candidate, 'doctor'], main_tree, env).stdout)
        work_status = json.loads(run([args.candidate, 'doctor'], worktree, env).stdout)
        assert main_status['project']['git_common_dir'] == work_status['project']['git_common_dir']
        assert main_status['code_graph']['index'] != work_status['code_graph']['index']
        client = MCP([args.candidate, 'serve', '--mcp', '--path', root], main_tree, env)
        try:
            outcomes = []
            for tree in [main_tree, worktree, main_tree]:
                outcomes.append(client.call('tools/call', {'name': 'lwc_codegraph', 'arguments': {
                    'command': 'search', 'projectPath': str(tree), 'arguments': {'query': marker}}}))
            assert outcomes[0] == outcomes[2] and outcomes[0] != outcomes[1], outcomes
        finally:
            client.close()
        fresh = json.loads(run([args.candidate, 'cg', 'check', 'src/mcp.rs', '--require-fresh'], main_tree, env).stdout)
        assert fresh['fresh']
        with (main_tree / 'src/mcp.rs').open('a') as file:
            file.write('\n// dirty freshness acceptance\n')
        stale = run([args.candidate, 'cg', 'check', 'src/mcp.rs', '--require-fresh'], main_tree, env, ok=False)
        assert stale.returncode != 0
        new = main_tree / 'src/not_indexed.rs'
        new.write_text('pub fn never_indexed() {}\n')
        assert json.loads(run([args.candidate, 'cg', 'check', 'src/not_indexed.rs'], main_tree, env).stdout)['files'][0]['state'] == 'not_indexed'
        report['checks'] = {'native_cli_byte_equality': True, 'worktree_index_isolation': True,
                            'shared_logical_repository': True, 'fresh_dirty_untracked': True,
                            'repeat_query_equality': True}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
