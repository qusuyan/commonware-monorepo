#!/usr/bin/env python3
"""Apply-study phase isolation, suite reporting, and launcher integration fixtures."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
REPO = ROOT.parent
IDS = ('any_unordered_btree', 'current_ordered')


def rows(kind='overwrite', operation='apply_study'):
    return [dict(auth_mode=auth, index_order=order, bitmap_chunk_bytes=chunk,
        keyspace_keys='4000000', write_calls_per_batch=10000, value_layout='fixed',
        merkle_family='mmb', batch_parent='directly from database', seed_synced='yes',
        commonware_page_cache='retained (warm)', input_generation_timed=False,
        write_skew_exponent=0, mean_us=mean, sample_count=10, write_type=kind,
        operation=operation, timing_scope='apply_batch', durability='buffered',
        iteration_policy='independent_snapshot_fixed_inputs')
        for auth, order, chunk, mean in [('any', 'unordered_btree', '', 100),
                                         ('current', 'ordered', '32', 150)]]


def compare(data, kind='writesize', success=True):
    cmd = ['jq', '-e', '--arg', 'experiment', kind, '--argjson', 'keys', '4000000',
           '--argjson', 'updates', '10000', '--argjson', 'repeat', '1', '--argjson', 'skew', '0',
           '--arg', 'order', ','.join(IDS), '-f', str(ROOT/'scripts/compare-experiment.jq')]
    r = subprocess.run(cmd, input=json.dumps(data), text=True, capture_output=True)
    if not success:
        assert r.returncode != 0, r.stdout
        return
    assert r.returncode == 0, r.stderr
    return json.loads(r.stdout)


class ApplyStudy(unittest.TestCase):
    def test_comparison_boundaries_and_identity(self):
        for kind in ['writesize', 'skew', 'db-scale', 'write-type']:
            data = rows() if kind != 'write-type' else sum((rows(w) for w in ['overwrite', 'insert', 'delete']), [])
            report = compare(data, kind)
            self.assertEqual(report['operation'], 'apply_study')
            self.assertEqual(report['timing_scope'], 'apply_batch')
            self.assertEqual(set(report['configurations']), set(IDS))
        for field, value in [('operation', 'merkleize'), ('timing_scope', 'end_to_end'),
                             ('durability', 'synced'), ('iteration_policy', 'evolving')]:
            bad = rows(); bad[1][field] = value
            compare(bad, success=False)
        bad = rows(); bad[0]['index_order'] = 'unordered'
        compare(bad, success=False)

    def test_suite_pair_and_phase_isolation(self):
        spec = importlib.util.spec_from_file_location('suite', ROOT/'scripts/summarize-isolated-suite.py')
        suite_module = importlib.util.module_from_spec(spec); spec.loader.exec_module(suite_module)
        with tempfile.TemporaryDirectory(dir=ROOT/'results') as temp:
            suite = Path(temp); reports = []; logs = []
            for repeat in [1, 2, 3]:
                folder = suite/str(repeat); folder.mkdir()
                report = compare(sum((rows(w) for w in ['overwrite', 'insert', 'delete']), []), 'write-type')
                report['repeat'] = repeat
                path = folder/'comparison.json'; path.write_text(json.dumps(report)); reports.append(path)
                logs.append(f'Archived write-type comparison at {folder}')
            (suite/'suite.log').write_text('\n'.join(logs))
            output = suite/'summary.json'
            suite_module.summarize(suite, ['write-type'], output)
            result = json.loads(output.read_text())['write-type']
            self.assertEqual(result[0]['keyspace_keys'], 4000000)
            self.assertEqual(result[0]['timing_scope'], 'apply_batch')
            self.assertAlmostEqual(result[0]['current_ordered_minus_any_unordered_btree_ms_mean'], .05)
            mixed = json.loads(reports[1].read_text()); mixed['operation'] = 'merkleize'
            reports[1].write_text(json.dumps(mixed))
            with self.assertRaises(ValueError): suite_module.summarize(suite, ['write-type'], output)

    def test_launchers_with_fixture_cargo(self):
        # Run real launchers and parsers in an isolated temporary repository. Cargo emits
        # synthetic Criterion files here; this checks wiring, never performance.
        with tempfile.TemporaryDirectory() as temp:
            repo = Path(temp); measurements = repo/'measurements-rain'
            shutil.copytree(ROOT/'scripts', measurements/'scripts', ignore=shutil.ignore_patterns('__pycache__'))
            for kind in ['writesize', 'write-type', 'skew', 'db-scale']:
                dest = measurements/'experiments'/kind; dest.mkdir(parents=True)
                shutil.copy2(ROOT/'experiments'/kind/'run.sh', dest/'run.sh')
            for relative in ['storage/src/qmdb/benches/merkleize.rs', 'storage/src/qmdb/benches/apply_study.rs',
                             'storage/src/qmdb/benches/common.rs', 'storage/src/qmdb/benches/snapshots.rs',
                             'storage/src/qmdb/benches/merkleize_config.rs', 'bench/src/lib.rs',
                             'storage/Cargo.toml', 'Cargo.lock']:
                dest = repo/relative; dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(REPO/relative, dest)
            subprocess.run(['git', 'init', '-q', str(repo)], check=True)
            subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                            'commit', '-q', '--allow-empty', '-m', 'fixture'], check=True)
            binary = repo/'bin'; binary.mkdir()
            cargo = binary/'cargo'
            cargo.write_text(f'''#!{sys.executable}
import hashlib, json, os, pathlib, sys
args = sys.argv[1:]
if '--exact' in args: name = args[args.index('--exact') + 1]
else:
    name = next(x for x in args if x.startswith('^qmdb::')).strip('^$')
    name = name.replace('[^ ]+', os.environ['QMDB_MERKLEIZE_SKEW'])
path = pathlib.Path(os.environ['CARGO_TARGET_DIR'])/'criterion'/hashlib.sha256(name.encode()).hexdigest()/'new'
path.mkdir(parents=True, exist_ok=True)
(path/'benchmark.json').write_text(json.dumps({{'full_id':name}}))
(path/'sample.json').write_text(json.dumps({{'iters':[1,2,3]}}))
(path/'estimates.json').write_text(json.dumps({{'mean':{{'point_estimate':1000,'confidence_interval':{{'lower_bound':900,'upper_bound':1100}}}},'median':{{'point_estimate':1000}},'median_abs_dev':{{'point_estimate':10}}}}))
''')
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(binary)+os.pathsep+os.environ['PATH'],
                       QMDB_STUDY_OPERATION='apply', QMDB_MERKLEIZE_KEYS='1000')
            for kind, args in [('writesize', ['200', '1']), ('write-type', ['10000', '1']),
                               ('skew', ['--updates', '13', '--repeats', '1', '0', '2']),
                               ('db-scale', ['--updates', '13', '--repeats', '1', '1000'])]:
                r = subprocess.run(['bash', str(measurements/'experiments'/kind/'run.sh'), *args],
                                   cwd=repo, env=env, text=True, capture_output=True)
                self.assertEqual(r.returncode, 0, r.stdout+r.stderr)
                comparisons = list((measurements/'results/apply'/kind).rglob('*comparison.json'))
                self.assertTrue(comparisons)
                for path in comparisons:
                    report = json.loads(path.read_text())
                    self.assertEqual(report['operation'], 'apply_study')
                    self.assertEqual(set(report['configurations']), set(IDS))
                    self.assertTrue(all(c['durability']=='buffered' for c in
                        (report['configurations'].values() if kind!='write-type' else
                         [v for c in report['configurations'].values() for k,v in c.items() if k in ['overwrite','insert','delete']])))
            env['QMDB_STUDY_OPERATION']='invalid'
            r = subprocess.run(['bash', str(measurements/'experiments/writesize/run.sh'), '200'], env=env, capture_output=True)
            self.assertNotEqual(r.returncode,0)


if __name__ == '__main__': unittest.main()
