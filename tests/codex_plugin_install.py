"""Run: python3 tests/codex_plugin_install.py"""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('installer', Path(__file__).resolve().parents[1] / 'integrations/codex-lwc/scripts/install.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class CodexPluginInstall(unittest.TestCase):
    def test_archives_only_duplicate_codex_entries_and_preserves_link_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plugin = root / 'plugin'
            codex = root / 'codex'
            other = root / 'other-agent'
            other.mkdir()
            (other / 'SKILL.md').write_text('unchanged')
            for name in ('using-lwc', 'using-plan'):
                (plugin / 'skills' / name).mkdir(parents=True)
                (plugin / 'skills' / name / 'SKILL.md').write_text(name)
            (codex / 'skills').mkdir(parents=True)
            (codex / 'skills/using-lwc').symlink_to(other, target_is_directory=True)
            (codex / 'skills/using-plan').mkdir()
            (codex / 'skills/using-plan/SKILL.md').write_text('original')
            (codex / 'skills/unrelated').mkdir()
            archived = installer.archive_duplicate_skills(codex, plugin)
            self.assertEqual(len(archived), 2)
            self.assertTrue(Path(archived[0]).is_symlink())
            self.assertEqual((other / 'SKILL.md').read_text(), 'unchanged')
            self.assertEqual((Path(archived[1]) / 'SKILL.md').read_text(), 'original')
            self.assertTrue((codex / 'skills/unrelated').is_dir())
            self.assertEqual(installer.archive_duplicate_skills(codex, plugin), [])


if __name__ == '__main__':
    unittest.main()
