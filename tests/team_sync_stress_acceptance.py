"""Bounded isolation stress; credentials stay in memory. Import the shared QA driver."""
import json
import os
import sqlite3
import ssl
import threading
import time
import urllib.request
from pathlib import Path
from team_sync_remote_acceptance import API, ROOT, REPORT, cli, client, docker, put


def member(owner, name, spaces):
    visible = {v['id']: v for v in owner.call('/api/me')['spaces']}
    opened = owner.call('/api/manage', {'action': 'member.create', 'team_id': owner.team,
        'name': name, 'days': 7, 'grants': [{'space_id': s, 'role': 'editor',
        'expected_revision': visible[s]['revision']} for s in spaces]})
    api = object.__new__(API)
    api.name, api.origin, api.gate = owner.name, owner.origin, owner.gate
    api.personal_key, api.token, api.team = opened['personal_key'], None, owner.team
    api.login = api.call('/api/auth/key', {'key': api.personal_key, 'cli': True})
    api.token = api.login['access_token']
    return api


def percentile(samples, quantile):
    return sorted(samples)[min(len(samples) - 1, int((len(samples) - 1) * quantile))]


def main():
    owner = API('lwc-sync-qa-final', 8790)
    space = owner.space('Automatic stress ' + str(int(time.time())))
    a_api, b_api = member(owner, 'Stress writer', [space]), member(owner, 'Stress reader', [space])
    a, b = client(a_api, 'stress-writer-' + space[:8], space), client(b_api, 'stress-reader-' + space[:8], space)
    # Watching uses the real detached worker and its per-replica lock.
    for home in (a, b):
        cli(home, ['space', 'configure', space, '--automatic', 'true', '--interval-ms', '1000'])
    gate = docker('exec', 'lwc-team-memory-1', 'cat', '/data/server-access.token').stdout.strip()
    ca = docker('exec', 'lwc-team-https-1', 'cat', '/data/caddy/pki/authorities/local/root.crt').stdout
    tls = ssl.create_default_context(cadata=ca)
    stop = threading.Event()
    health, management, propagated, heads = [], [], [], []
    failure = []
    def monitor():
        consecutive = 0
        while not stop.is_set():
            started = time.monotonic()
            try:
                request = urllib.request.Request('https://10.10.10.17:8788/health', headers={'X-LWC-Server-Token': gate})
                with urllib.request.urlopen(request, context=tls, timeout=3) as response:
                    assert response.code == 200
                health.append(time.monotonic() - started)
                consecutive = 0
                if len(health) >= 20 and percentile(health[-20:], .95) > max(2, 2 * percentile(health[:10], .95)):
                    raise RuntimeError('live baseline latency protection')
            except Exception:
                consecutive += 1
                if consecutive >= 3:
                    failure.append('live health protection')
                    stop.set()
                    docker('stop', 'lwc-sync-qa-final', 'lwc-sync-qa-fault', check=False)
                    return
            stop.wait(2)
    thread = threading.Thread(target=monitor, daemon=True)
    thread.start()
    started = time.monotonic()
    write_duration = int(os.environ.get('LWC_QA_WRITE_SECONDS', '600'))
    total_duration = int(os.environ.get('LWC_QA_TOTAL_SECONDS', '600'))
    writes = 0
    try:
        # One hundred writes during a finite window, then sustained 1/s editing.
        for i in range(100):
            put(a, space, f'burst-{i}', f'Burst {i}')
            writes += 1
        started = time.monotonic()
        written = []
        def producer():
            nonlocal writes
            try:
                while not stop.is_set() and time.monotonic() - started < write_duration:
                    tick = time.monotonic()
                    slug, body = f'continuous-{writes}', f'Evidence {writes}'
                    put(a, space, slug, body)
                    written.append((tick, slug, body))
                    writes += 1
                    stop.wait(max(0, 1 - (time.monotonic() - tick)))
            except Exception as error:
                failure.append(type(error).__name__ + ': ' + str(error))
                stop.set()
        producer_thread = threading.Thread(target=producer, daemon=True)
        producer_thread.start()
        next_progress, sample, observed = time.monotonic(), 19, None
        while time.monotonic() - started < total_duration or producer_thread.is_alive():
            if stop.is_set():
                raise AssertionError(failure[0])
            stamp = time.monotonic()
            owner.call('/api/admin?view=teams')
            management.append(time.monotonic() - stamp)
            heads.append(owner.call(f'/api/spaces/{space}/head')['head'])
            if observed is None and len(written) > sample:
                observed = written[sample]
                sample += 20
            if observed:
                stamp, slug, body = observed
                shown = cli(b, ['--space', space, 'page', 'show', slug], okay=False)
                if shown.returncode == 0:
                    assert json.loads(shown.stdout)['page']['body'] == body
                    propagated.append(time.monotonic() - stamp)
                    observed = None
                elif time.monotonic() - stamp > 120:
                    raise AssertionError('automatic propagation did not converge while editing continued')
            stop.wait(1)
            if time.monotonic() >= next_progress:
                print(json.dumps({'stress_seconds': round(time.monotonic() - started), 'writes': writes,
                    'head': heads[-1]}), flush=True)
                next_progress = time.monotonic() + 30
        producer_thread.join(timeout=5)
        # Worker convergence is checked against authoritative pages and final digest.
        deadline = time.monotonic() + 120
        while True:
            left, right = cli(a, ['space', 'show', space]), cli(b, ['space', 'show', space])
            remote = owner.call(f'/api/spaces/{space}/head')
            if left['replica']['remote_head']['digest'] == right['replica']['remote_head']['digest'] == remote['digest']:
                rows = []
                while True:
                    page = cli(b, ['--space', space, 'page', 'list', '--limit', '100', '--offset', str(len(rows))])['pages']
                    rows.extend(page)
                    if len(page) < 100:
                        break
                if len(rows) == writes:
                    break
            assert time.monotonic() < deadline, 'workers did not reach equal digests'
            time.sleep(1)
        assert len(rows) == writes, (len(rows), writes)
        bodies = []
        for home in (a, b):
            database = next(home.rglob('wiki.db'))
            with sqlite3.connect('file:' + str(database) + '?mode=ro', uri=True) as conn:
                pages = dict(conn.execute('SELECT slug,body FROM pages'))
            assert len(pages) == writes
            for slug, body in pages.items():
                number = int(slug.rsplit('-', 1)[1])
                assert body == (f'Burst {number}' if slug.startswith('burst-') else f'Evidence {number}')
            bodies.append(pages)
        assert bodies[0] == bodies[1]
        REPORT['stress'] = {'space': space, 'seconds': time.monotonic() - started,
            'writes': writes, 'accepted_heads': remote['head'], 'digest_equal': True,
            'all_page_bodies_equal': True,
            'management_p95': percentile(management, .95), 'health_p95': percentile(health, .95),
            'propagation_samples': propagated, 'head_observations': heads}
        REPORT['cases'].append({'name': 'bounded_automatic_burst_and_sustained_stress', 'status': 'passed'})
        print(json.dumps(REPORT['stress']), flush=True)
    except Exception as error:
        REPORT['stress_failure'] = {'error': type(error).__name__ + ': ' + str(error),
            'writes': writes, 'seconds': time.monotonic() - started,
            'head_observations': heads, 'propagation_samples': propagated}
        raise
    finally:
        stop.set()
        thread.join(timeout=4)
        for home in (a, b):
            deadline = time.monotonic() + 15
            while cli(home, ['space', 'configure', space, '--automatic', 'false'], okay=False).returncode:
                if time.monotonic() >= deadline:
                    # Only terminate the worker whose private HOME and executable match this fixture.
                    for path in home.rglob('worker.json'):
                        pid = json.loads(path.read_text()).get('pid')
                        process = Path('/proc') / str(pid)
                        if process.exists() and b'space\x00watch\x00' in (process / 'cmdline').read_bytes() and ('HOME=' + str(home)).encode() in (process / 'environ').read_bytes().split(b'\x00'):
                            os.kill(pid, 15)
                    cli(home, ['space', 'configure', space, '--automatic', 'false'])
                    break
                time.sleep(.2)
        (ROOT / 'stress-acceptance.json').write_text(json.dumps(REPORT, indent=2))


if __name__ == '__main__':
    main()
