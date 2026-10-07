"""Isolated fault/RBAC and real-core contract checks; uses shared bounded QA tools."""
import concurrent.futures as futures
import hashlib
import json
import secrets
import sqlite3
import time
from team_sync_remote_acceptance import API, ROOT, REPORT, arm, case, cli, client, docker, put, wait_hit
from team_sync_stress_acceptance import member


def main():
    api = API('lwc-sync-qa-final', 8790)
    fault = API('lwc-sync-qa-fault', 8791)
    def core():
        space = api.space('All core memory')
        root, _, batch = api.fixture(space, 'core')
        assert api.call(root + '/push', batch)['team']['accepted_head'] == 1
        db = sqlite3.connect(ROOT / 'fixtures/core.db')
        expected = {(k, key): digest for k, key, digest in db.execute('SELECT kind,logical_key,payload_hash FROM sync_objects')}
        kinds = sorted({k for k, _ in expected})
        for kind in kinds:
            rows = api.call(root + '/query', {'action': 'objects', 'kind': kind, 'limit': 100, 'offset': 0})['data']['objects']
            assert len(rows) == len([key for k, key in expected if k == kind]), (kind, rows)
            for row in rows:
                assert row['hash'] == expected[(row['kind'], row['key'])]
        reader = member(api, 'Core reader', [space])
        home = client(reader, 'core-reader-' + space[:8], space)
        assert cli(home, ['space', 'sync', space])['status'] in ('current', 'synced')
        assert cli(home, ['space', 'sync', space])['status'] == 'current'
        assert api.call(root + '/head')['digest'] == batch['payload_digest']
        REPORT['core_kinds'] = kinds
    case('all_core_http_and_cli_roundtrip', core)

    def revocation(action):
        space = fault.space('Guard race ' + action)
        editor = member(fault, 'Guard editor', [space])
        root, _, batch = editor.fixture(space, 4500)
        user = editor.login['user_id']
        arm('before-commit', 'pause')
        with futures.ThreadPoolExecutor(max_workers=1) as pool:
            pending = pool.submit(editor.call, root + '/push', batch, expected=401 if action == 'key' else (409 if action == 'policy' else 403))
            wait_hit('before-commit')
            revision = next(v['revision'] for v in fault.call('/api/me')['spaces'] if v['id'] == space)
            if action == 'key':
                change = {'action': 'key.revoke', 'key_id': hashlib.sha256(editor.personal_key.encode()).hexdigest()}
            elif action == 'policy':
                change = {'action': 'space.policy', 'space_id': space, 'user_id': user, 'expected_revision': revision,
                          'denials': [{'kind': 'page', 'key': '*', 'action': 'create'}]}
            else:
                change = {'action': 'space.grant', 'space_id': space, 'user_id': user, 'expected_revision': revision, 'role': 'viewer'}
            fault.call('/api/manage', change)
            docker('exec', 'lwc-sync-qa-fault', 'touch', '/faults/before-commit.resume')
            pending.result()
        assert fault.call(root + '/head')['head'] == 0
        assert not fault.call(root + '/query', {'action': 'list', 'limit': 100, 'offset': 0})['data']['pages']
        if action == 'policy':
            editor.call(root + '/push', batch, expected=403)
    for action in ('role', 'policy', 'key'):
        case('prepare_then_revoke_' + action, lambda action=action: revocation(action))

    def repeated():
        space = api.space('Same batch concurrency')
        root, replica, batch = api.fixture(space, 4500)
        with futures.ThreadPoolExecutor(max_workers=2) as pool:
            receipts = list(pool.map(lambda _: api.call(root + '/push', batch), range(2)))
        assert all(v['team']['accepted_head'] == 1 for v in receipts)
        assert api.call(root + '/head')['head'] == 1
        altered = dict(batch, payload_digest='f' * 64)
        api.call(root + '/push', altered, expected=409)
        # Simulate the client losing the response by discarding it and querying the fixed identity.
        known = api.call(root + f"/receipts/{replica['replica_id']}/{batch['batch_id']}")['receipt']
        assert known['team']['accepted_head'] == 1
    case('same_batch_concurrent_replay_and_lost_response_receipt', repeated)

    def malformed():
        space = api.space('Interrupted transport')
        root = f'/api/spaces/{space}'
        replica = api.call(root + '/replicas', {'device': 'Transport QA', 'device_id': secrets.token_hex(32)})
        transfer = json.loads((ROOT / 'fixtures/4500.json').read_text())
        args = {'replica_id': replica['replica_id'], 'request_id': secrets.token_hex(32), 'transfer': transfer}
        reservation = api.call(root + '/uploads', args)
        upload = root + '/uploads/' + reservation['artifact_id']
        api.call(upload, method='PUT', raw=b'truncated', expected=400)
        assert api.call(root + '/uploads', args)['status'] == 'receiving'
        api.call(upload, method='DELETE')
        args['request_id'] = secrets.token_hex(32)
        reservation = api.call(root + '/uploads', args)
        api.call(root + '/uploads/' + reservation['artifact_id'], method='PUT', raw=b'X' * transfer['size'])
        batch = {'protocol': 'lwc-team-sync/1', 'share_schema': 1, 'server_epoch': replica['head']['server_epoch'],
                 'expected_head': 0, 'replica_id': replica['replica_id'], 'batch_id': secrets.token_hex(32),
                 'artifact_id': reservation['artifact_id'], 'payload_digest': transfer['state_digest']}
        api.call(root + '/push', batch, expected=400)
        huge = dict(transfer, size=268435457)
        api.call(root + '/uploads', dict(args, request_id=secrets.token_hex(32), transfer=huge), expected=413)
        assert api.call(root + '/head')['head'] == 0
    case('truncated_corrupt_and_over_limit_uploads', malformed)
    print(json.dumps({'passed': len(REPORT['cases'])}), flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        REPORT['failure'] = type(error).__name__ + ': ' + str(error)
        (ROOT / 'acceptance.json').write_text(json.dumps(REPORT, indent=2))
        raise
