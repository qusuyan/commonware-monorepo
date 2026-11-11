#!/usr/bin/env python3
"""Small process-level regressions. Pass the built qmdb_snapshot executable."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve())


def run(*args):
    return subprocess.run([BINARY, *map(str, args)], capture_output=True, text=True)


def files(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


class SnapshotCLI(unittest.TestCase):
    def test_create_inspect_and_repeat_preserve_master(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            keys = root / 'keys.bin'
            self.assertEqual(run('generate-keys', '--count', 65, '--out', keys).returncode, 0)
            for variant in ('any_unordered_btree', 'current_ordered'):
                db = root / variant
                args = ('seed-db', '--count', 65, '--keys-file', keys,
                        '--variant', variant, '--out', db)
                result = run(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                before = files(db)
                for command in (('inspect-db', '--db-path', db), args):
                    result = run(*command)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(files(db), before)
                exercised = run('exercise-db', '--db-path', db, '--updates', 17)
                self.assertEqual(exercised.returncode, 0, exercised.stderr)
                roots = json.loads(exercised.stdout)
                self.assertEqual(len(roots['batches']), 4)
                repeated = run('exercise-db', '--db-path', db, '--updates', 17)
                self.assertEqual(json.loads(repeated.stdout), roots)
                self.assertEqual(files(db), before)

    def test_invalid_commands_preserve_existing_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            db = root / 'existing'
            db.mkdir()
            marker = db / 'keep'
            marker.write_text('existing data')
            for extra in (('--variant', 'invalid'),
                          ('--variant', 'current_ordered', '--count', '0'),
                          ('--variant', 'current_ordered', '--count', 'bad'),
                          ('--variant', 'current_ordered', '--unknown', 'value'),
                          ('--variant', 'current_ordered', '--variant', 'current_ordered')):
                self.assertNotEqual(run('seed-db', '--out', db, *extra).returncode, 0)
                self.assertEqual(marker.read_text(), 'existing data')

    def test_corrupt_keys_and_manifest_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            keys = root / 'keys.bin'
            self.assertEqual(run('generate-keys', '--count', 5, '--out', keys).returncode, 0)
            original = keys.read_bytes()
            keys.write_bytes(original[:32] + original[:32] + original[64:])
            db = root / 'db'
            args = ('seed-db', '--count', 5, '--keys-file', keys,
                    '--variant', 'any_unordered_btree', '--out', db)
            self.assertNotEqual(run(*args).returncode, 0)
            self.assertFalse(db.exists())
            keys.write_bytes(original)
            self.assertEqual(run(*args).returncode, 0)
            manifest_path = db / 'snapshot_manifest.json'
            manifest = json.loads(manifest_path.read_text())
            for field, value in (('merkle_root', '0' * 64), ('schema_version', 1),
                                 ('key_count', 0), ('configuration_fingerprint', '0' * 64)):
                changed = dict(manifest, **{field: value})
                manifest_path.write_text(json.dumps(changed))
                before = files(db)
                self.assertNotEqual(run('inspect-db', '--db-path', db).returncode, 0)
                self.assertEqual(files(db), before)
            empty = root / 'empty'
            empty.mkdir()
            (empty / 'snapshot_manifest.json').write_text(json.dumps(manifest))
            before = files(empty)
            self.assertNotEqual(run('inspect-db', '--db-path', empty).returncode, 0)
            self.assertEqual(files(empty), before)


if __name__ == '__main__':
    unittest.main()
