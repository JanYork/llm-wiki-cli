#!/usr/bin/env python3
"""Install the native Codex plugin and archive duplicate Codex-only Skills."""
import datetime
import json
import os
from pathlib import Path
import subprocess


def archive_duplicate_skills(codex_home, plugin_root):
    names = sorted(p.name for p in (plugin_root / 'skills').iterdir()
                   if p.is_dir() and (p / 'SKILL.md').is_file())
    duplicates = [codex_home / 'skills' / name for name in names
                  if os.path.lexists(codex_home / 'skills' / name)]
    if not duplicates:
        return []
    backup = codex_home / 'backups' / ('lwc-plugin-skills-' +
              datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ'))
    backup.mkdir(parents=True, exist_ok=False)
    moved = []
    try:
        for source in duplicates:
            destination = backup / source.name
            source.rename(destination)  # Move links themselves, never their targets.
            moved.append((source, destination))
    except OSError:
        for source, destination in reversed(moved):
            destination.rename(source)
        raise
    return [str(destination) for _, destination in moved]


def main():
    plugin_root = Path(__file__).resolve().parents[1]
    manifest = json.loads((plugin_root / '.codex-plugin/plugin.json').read_text())
    marketplace = json.loads((plugin_root / '.agents/plugins/marketplace.json').read_text())
    subprocess.run(['codex', 'plugin', 'marketplace', 'add', str(plugin_root)], check=True)
    subprocess.run(['codex', 'plugin', 'add', manifest['name'] + '@' + marketplace['name']], check=True)
    home = Path(os.environ.get('CODEX_HOME') or Path.home() / '.codex').expanduser()
    print(json.dumps({'archived_codex_skills': archive_duplicate_skills(home, plugin_root)}))


if __name__ == '__main__':
    main()
