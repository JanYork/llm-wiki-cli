"""Bounded 16MiB source transport through real clients; no synthetic Store writes."""
import hashlib
import json
import sqlite3
import time
from team_sync_remote_acceptance import API, ROOT, client, cli
from team_sync_stress_acceptance import member


def main():
    api = API('lwc-sync-qa-final', 8790)
    space = api.space('16MiB source roundtrip')
    writer = client(member(api, 'Source writer', [space]), 'source-a-' + space[:8], space)
    reader = client(member(api, 'Source reader', [space]), 'source-b-' + space[:8], space)
    source = ROOT / 'fixtures/large-source-structured.md'
    size = 16 * 1024 * 1024
    paragraph = (b'Large source transmission evidence ' * 120) + b'.\n\n'
    source.write_bytes((paragraph * (size // len(paragraph) + 1))[:size])
    expected = hashlib.sha256(source.read_bytes()).hexdigest()
    started = time.monotonic()
    cli(writer, ['--space', space, 'source', 'add', str(source), '--title',
                 'Large source', '--allow-external-source'])
    assert cli(writer, ['space', 'sync', space])['status'] == 'synced'
    assert cli(reader, ['space', 'sync', space])['status'] == 'synced'
    head = api.call(f'/api/spaces/{space}/head')
    rows = api.call(f'/api/spaces/{space}/query', {
        'action': 'objects', 'kind': 'source', 'limit': 100, 'offset': 0,
    })['data']['objects']
    assert len(rows) == 1 and rows[0]['key'] == expected
    for home in (writer, reader):
        database = next(home.rglob('wiki.db'))
        with sqlite3.connect('file:' + str(database) + '?mode=ro', uri=True) as conn:
            content = conn.execute('SELECT content FROM sources WHERE content_hash=?',
                                   [expected]).fetchone()[0].encode()
        assert len(content) == size and hashlib.sha256(content).hexdigest() == expected
        assert cli(home, ['space', 'show', space])['replica']['remote_head']['digest'] == head['digest']
    result = {'space': space, 'bytes': size, 'content_sha256': expected,
              'seconds': time.monotonic() - started, 'head': head['head'], 'digest_equal': True}
    (ROOT / 'large-source-acceptance.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
