"""Finite 8-client/4-space load and a deterministic frozen-window regression."""
import concurrent.futures as futures
import json
import os
import secrets
import time
import team_sync_remote_acceptance as qa
from team_sync_stress_acceptance import member


def main():
    api = qa.API('lwc-sync-qa-final', 8790)
    def convergence(spaces, clients, prefix):
        subjects = [member(api, f'{prefix} {i}', [spaces[i % len(spaces)]]) for i in range(clients)]
        homes = [qa.client(subject, f'{prefix}-{secrets.token_hex(4)}', spaces[i % len(spaces)]) for i, subject in enumerate(subjects)]
        for i, home in enumerate(homes):
            qa.put(home, spaces[i % len(spaces)], f'concurrent-{i}', f'Writer {i}')
        failures = []
        with futures.ThreadPoolExecutor(max_workers=clients) as pool:
            results = list(pool.map(lambda item: qa.cli(item[1], ['space', 'sync', spaces[item[0] % len(spaces)]], okay=False), enumerate(homes)))
        for result in results:
            if result.returncode:
                error = json.loads(result.stderr)['error']
                assert error['details']['http_status'] == 429, error['code']
                failures.append(error['details']['code'])
        # Head races are expected; abandoned reservations must not exhaust the four slots.
        for _ in range(clients * 2 + 1):
            ready = True
            for i, home in enumerate(homes):
                result = qa.cli(home, ['space', 'sync', spaces[i % len(spaces)]])
                ready &= result['status'] == 'current'
            if ready:
                break
        assert ready, 'bounded retries did not converge'
        for i, space in enumerate(spaces):
            for writer in range(i, clients, len(spaces)):
                assert qa.cli(homes[i], ['--space', space, 'page', 'show', f'concurrent-{writer}'])['page']['body'] == f'Writer {writer}'
        qa.REPORT.setdefault('expected_load_errors', []).extend(failures)
    qa.case('eight_real_members_same_space_converge', lambda: convergence([api.space('Eight writers')], 8, 'eight'))
    qa.case('eight_clients_four_spaces_converge', lambda: convergence([api.space('Four spaces ' + str(i)) for i in range(4)], 8, 'four-spaces'))

    def frozen():
        space = api.space('Fixed-window edits')
        subject = member(api, 'Window writer', [space])
        home = qa.client(subject, 'fixed-window-' + space[:8], space)
        qa.put(home, space, 'frozen', 'First window')
        directory = qa.ROOT / 'client-faults'
        directory.mkdir(mode=0o700, exist_ok=True)
        for name in ('before-upload.hit', 'before-upload.resume'):
            (directory / name).unlink(missing_ok=True)
        (directory / 'before-upload.arm').write_text('pause')
        old_binary, old_env = qa.BIN, os.environ.get('LWC_SYNC_TEST_DIR')
        qa.BIN = qa.ROOT / 'bin/lwc-linux-fault'
        os.environ['LWC_SYNC_TEST_DIR'] = str(directory)
        try:
            with futures.ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(qa.cli, home, ['space', 'sync', space])
                deadline = time.monotonic() + 120
                while not (directory / 'before-upload.hit').exists():
                    assert time.monotonic() < deadline, 'client barrier not reached'
                    time.sleep(.05)
                qa.put(home, space, 'later', 'Next window is preserved')
                (directory / 'before-upload.resume').touch()
                assert pending.result()['head'] == 1
            assert api.call(f'/api/spaces/{space}/head')['head'] == 1
            assert qa.cli(home, ['--space', space, 'page', 'show', 'later'])['page']['body'] == 'Next window is preserved'
        finally:
            qa.BIN = old_binary
            if old_env is None:
                os.environ.pop('LWC_SYNC_TEST_DIR', None)
            else:
                os.environ['LWC_SYNC_TEST_DIR'] = old_env
        assert qa.cli(home, ['space', 'sync', space])['head'] == 2
        remote = api.call(f'/api/spaces/{space}/query', {'action': 'get', 'slug': 'later'})
        assert remote['data']['page']['body'] == 'Next window is preserved'
    qa.case('frozen_batch_advances_despite_later_local_edit', frozen)


if __name__ == '__main__':
    main()
