"""Write failure stays inside a capped 32MiB tmpfs, never the host disk."""
import secrets
from team_sync_remote_acceptance import API, ROOT, case, docker


def main():
    api = API('lwc-sync-qa-disk', 8793)
    space = api.space('Small test volume')
    root, _, first = api.fixture(space, 4500)
    assert api.call(root + '/push', first)['team']['accepted_head'] == 1
    before = api.call(root + '/head')
    root, _, batch = api.fixture(space, 4500)
    # Use all free space in this tmpfs only. Scope is Docker's hard 32MiB mount.
    fill = docker('exec', 'lwc-sync-qa-disk', 'sh', '-c', 'dd if=/dev/zero of=/data/qa-fill bs=64K status=none', check=False)
    assert fill.returncode != 0
    try:
        rejected = api.call(root + '/push', batch, expected=500)
        assert rejected['error']['code'] in ('database_error', 'io_error')
    finally:
        docker('exec', 'lwc-sync-qa-disk', 'rm', '-f', '/data/qa-fill')
    after = api.call(root + '/head')
    assert after['head'] == before['head'] and after['digest'] == before['digest']
    assert api.call(root + '/push', batch)['team']['accepted_head'] == 2
    assert len(api.call(root + '/query', {'action': 'list', 'limit': 100, 'offset': 0})['data']['pages']) == 100
    case('bounded_tmpfs_full_rejects_preserves_head_and_recovers', lambda: None)
    docker('stop', 'lwc-sync-qa-disk')


if __name__ == '__main__':
    main()
