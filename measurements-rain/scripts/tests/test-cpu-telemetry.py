#!/usr/bin/env python3
"""Launcher fixtures; set QMDB_CPU_BINARY and QMDB_SNAPSHOT_BINARY for native smoke tests."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
REPO = ROOT.parent


def invoke(command, **kwargs):
    return subprocess.run(command, text=True, capture_output=True, **kwargs)


class Launcher(unittest.TestCase):
    def test_capture_and_existing_destination(self):
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp)
            scripts = repo / 'measurements-rain/scripts'
            scripts.mkdir(parents=True)
            for name in ['run-cpu-telemetry.sh', 'write-manifest.sh', 'validate-skew.sh']:
                shutil.copy2(ROOT / 'scripts' / name, scripts / name)
            bin_dir = repo / 'bin'
            bin_dir.mkdir()
            cargo = bin_dir / 'cargo'
            cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
out = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
out.mkdir()
(out/'invocation.json').write_text(json.dumps(sys.argv[1:]))
print('native runner fixture')
''')
            cargo.chmod(0o755)
            git = bin_dir / 'git'
            git.write_text('#!/bin/sh\nif [ "$1" = rev-parse ]; then echo fixture-revision; fi\n')
            git.chmod(0o755)
            env = dict(os.environ, PATH=f'{bin_dir}:{os.environ["PATH"]}')
            command = ['bash', str(scripts/'run-cpu-telemetry.sh'), 'fixture',
                       '--phase', 'apply', '--variant', 'current_ordered',
                       '--keys', '32', '--updates', '16', '--samples', '2']
            result = invoke(command, env=env, cwd=temp)
            self.assertEqual(result.returncode, 0, result.stderr)
            run = repo/'measurements-rain/results/fixture'
            manifest = json.loads((run/'manifest.json').read_text())
            self.assertEqual(manifest['kind'], 'cpu')
            self.assertEqual(manifest['fixed_updates_per_batch'], 16)
            self.assertEqual(manifest['study_operation'], 'apply')
            self.assertIn('native runner fixture', (run/'output.log').read_text())
            before = (run/'cpu/invocation.json').read_bytes()
            self.assertNotEqual(invoke(command, env=env).returncode, 0)
            self.assertEqual((run/'cpu/invocation.json').read_bytes(), before)
            for bad in [['../escape', '--keys', '1'], ['bad', '--output', '/tmp/escape'],
                        ['bad', '--updates', '+16'], ['bad', '--updates', '016'],
                        ['bad', '--updates', '18446744073709551616']]:
                self.assertNotEqual(invoke(['bash', str(scripts/'run-cpu-telemetry.sh'), *bad], env=env).returncode, 0)


@unittest.skipUnless(os.environ.get('QMDB_CPU_BINARY') and os.environ.get('QMDB_SNAPSHOT_BINARY'),
                     'set paths to built qmdb_cpu and qmdb_snapshot executables')
class Native(unittest.TestCase):
    def test_phases_roots_provenance_and_cli_errors(self):
        cpu = str(Path(os.environ['QMDB_CPU_BINARY']).resolve())
        snapshot = str(Path(os.environ['QMDB_SNAPSHOT_BINARY']).resolve())
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            keys = temp/'keys.bin'
            result = invoke([snapshot, 'generate-keys', '--count', '37', '--out', str(keys)])
            self.assertEqual(result.returncode, 0, result.stderr)
            for variant, suffix in [('any_unordered_btree', 'any_unordered_btree_fixed_mmb'),
                                    ('current_ordered', 'current_ordered_fixed_mmb_chunk32')]:
                master = ROOT/'storage-seeds'/f'k37_{suffix}'
                created = not master.exists()
                try:
                    result = invoke([snapshot, 'seed-db', '--variant', variant, '--count', '37',
                                     '--keys-file', str(keys), '--out', str(master)])
                    self.assertEqual(result.returncode, 0, result.stderr)
                    def fingerprint():
                        return {str(p.relative_to(master)): hashlib.sha256(p.read_bytes()).hexdigest()
                                for p in master.rglob('*') if p.is_file()}
                    before = fingerprint()
                    roots = []
                    for phase in ['merkleize', 'apply']:
                        output = temp/f'{variant}-{phase}'
                        command = [cpu, '--phase', phase, '--variant', variant, '--keys', '37',
                                   '--updates', '16', '--samples', '2', '--output', str(output)]
                        env = dict(os.environ, QMDB_SNAPSHOT_REUSE='required')
                        # Exercise the marker appended by cargo bench as well as direct invocation.
                        result = invoke([*command, '--bench'], env=env)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        rows = [json.loads(line) for line in (output/'samples.jsonl').read_text().splitlines()]
                        self.assertEqual(len(rows), 2)
                        for row in rows:
                            self.assertEqual(row['phase'], phase)
                            self.assertEqual(row['variant'], variant)
                            self.assertEqual(row['source'], 'libc_getrusage')
                            self.assertEqual(row['measured_batches'], 1)
                            self.assertFalse(row['process_cpu_monitoring'])
                            self.assertEqual(row['provenance']['source'], 'reused')
                            self.assertIn('keyset_prefix_digest', row['provenance'])
                            self.assertEqual(row['total_cpu_ns'], row['user_cpu_ns'] + row['system_cpu_ns'])
                            self.assertAlmostEqual(row['cpu_ns_per_write'], row['total_cpu_ns']/16)
                            self.assertGreater(row['wall_ns'], 0)
                        self.assertEqual(rows[0]['result_root'], rows[1]['result_root'])
                        roots.append(rows[0]['result_root'])
                        summary = json.loads((output/'summary.json').read_text())
                        self.assertEqual(summary['sample_count'], 2)
                        expected_mean = sum(r['total_cpu_ns'] for r in rows)/2
                        self.assertEqual(summary['statistics']['total_cpu_ns']['mean'], expected_mean)
                        self.assertFalse(summary['overhead_subtracted'])
                        self.assertEqual(len((output/'overhead.jsonl').read_text().splitlines()), 2)
                        self.assertNotEqual(invoke(command, env=env).returncode, 0)
                        self.assertEqual(fingerprint(), before)
                    self.assertEqual(roots[0], roots[1])
                finally:
                    if created and master.exists():
                        shutil.rmtree(master)
            for extra in [['--keys', '0'], ['--keys', '18446744073709551615'],
                          ['--updates', '18446744073709551615'], ['--skew', 'nan'], ['--samples', '0'],
                          ['--phase', 'durable'], ['--unknown', '1']]:
                command = [cpu, '--phase', 'apply', '--variant', 'current_ordered',
                           '--keys', '37', '--updates', '16', '--output', str(temp/'invalid')]
                if extra[0] in command:
                    command[command.index(extra[0])+1] = extra[1]
                else:
                    command.extend(extra)
                result = invoke(command)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((temp/'invalid').exists())


if __name__ == '__main__':
    unittest.main()
