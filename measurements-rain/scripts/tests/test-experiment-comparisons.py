#!/usr/bin/env python3
"""Fixture checks for three-configuration comparisons and archive compatibility."""
import json
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'experiments/writesize'))
from report import read_comparison

try:
    import matplotlib
    HAS_MATPLOTLIB = True
except ImportError:
    HAS_MATPLOTLIB = False

IDS = ('any_unordered', 'current_unordered', 'current_ordered')
PAIRWISE_IDS = ('any_unordered_btree', 'current_ordered')

def rows(kind='overwrite', keys=10_000_000, updates=10000):
    result = []
    for name, mean in zip(IDS, (100., 120., 150.)):
        auth, order = name.split('_')
        result.append(dict(auth_mode=auth, index_order=order,
            bitmap_chunk_bytes='' if auth == 'any' else '32', keyspace_keys=str(keys),
            write_calls_per_batch=updates, value_layout='fixed', merkle_family='mmb',
            batch_parent='directly from database', seed_synced='yes',
            commonware_page_cache='retained (warm)', input_generation_timed=False,
            write_skew_exponent=0, mean_us=mean, sample_count=10,
            mean_ci_low_us=mean-1, mean_ci_high_us=mean+1, write_type=kind))
    return result

def pairwise_rows(kind='overwrite', keys=10_000_000, updates=10000):
    result = []
    for name, mean in zip(PAIRWISE_IDS, (100., 150.)):
        if name == 'any_unordered_btree':
            auth, order, chunk = 'any', 'unordered_btree', ''
        else:
            auth, order, chunk = 'current', 'ordered', '32'
        result.append(dict(auth_mode=auth, index_order=order,
            bitmap_chunk_bytes=chunk, keyspace_keys=str(keys),
            write_calls_per_batch=updates, value_layout='fixed', merkle_family='mmb',
            batch_parent='directly from database', seed_synced='yes',
            commonware_page_cache='retained (warm)', input_generation_timed=False,
            write_skew_exponent=0, mean_us=mean, sample_count=10,
            mean_ci_low_us=mean-1, mean_ci_high_us=mean+1, write_type=kind))
    return result

def compare(data, experiment='writesize', success=True, ids=IDS, updates=10000, keys=10_000_000):
    cmd = ['jq', '-e', '--arg', 'experiment', experiment, '--argjson', 'keys', str(keys),
           '--argjson', 'updates', str(updates), '--argjson', 'repeat', '1', '--argjson', 'skew', '0',
           '--arg', 'order', ','.join(ids), '-f', str(ROOT/'scripts/compare-experiment.jq')]
    run = subprocess.run(cmd, input=json.dumps(data), text=True, capture_output=True)
    if not success:
        assert run.returncode != 0, run.stdout
        return
    assert run.returncode == 0, run.stderr
    return json.loads(run.stdout)

class Comparisons(unittest.TestCase):
    def test_suite_summary_formats(self):
        spec = importlib.util.spec_from_file_location('suite_summary', ROOT/'scripts/summarize-isolated-suite.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for ids, make_rows, kinds in [(PAIRWISE_IDS, pairwise_rows, ('overwrite', 'insert', 'delete')),
                                       (IDS, rows, ('overwrite', 'insert'))]:
            with self.subTest(ids=ids), tempfile.TemporaryDirectory(dir=ROOT/'results') as temp:
                suite = Path(temp)
                logs = []
                for repeat in (1, 2, 3):
                    folder = suite / f'run-{repeat}'
                    folder.mkdir()
                    data = [row for kind in kinds for row in make_rows(kind, updates=2000)]
                    for row in data:
                        if row['write_type'] == 'delete':
                            row['mean_us'] *= 2
                    report = compare(data, 'write-type', ids=ids, updates=2000)
                    report['repeat'] = repeat
                    (folder/'comparison.json').write_text(json.dumps(report))
                    logs.append(f'Archived write-type comparison at {folder}')
                (suite/'suite.log').write_text('\n'.join(logs))
                output = suite/'summary.json'
                module.summarize(suite, aspects=['write-type'], json_output=output)
                rows_out = json.loads(output.read_text())['write-type']
                self.assertEqual([r['write_type'] for r in rows_out], list(kinds))
                self.assertTrue(all(r['write_calls_per_batch'] == 2000 for r in rows_out))
                if 'delete' in kinds:
                    self.assertEqual(rows_out[-1]['current_ordered_minus_any_unordered_btree_ms_mean'], .1)
                    self.assertEqual(rows_out[-1]['any_unordered_btree_delete_vs_overwrite_percent_mean'], 100)

    def test_three_cases(self):
        for experiment in ('writesize', 'skew'):
            report = compare(rows(), experiment)
            self.assertEqual(set(report['configurations']), set(IDS))
            diffs = report['comparisons']
            self.assertEqual([v['delta_mean_us'] for v in diffs.values()], [20,30,50])
            self.assertAlmostEqual(diffs['current_ordered_minus_current_unordered']['overhead_percent'],25)

    def test_six_cases(self):
        inserts = rows('insert')
        for row in inserts:
            row['mean_us'] *= 2
        report = compare(rows() + inserts, 'write-type')
        for config in report['configurations'].values():
            self.assertEqual(config['insert_overhead_percent'],100)
        self.assertEqual(report['comparisons']['insert']['current_ordered_minus_any_unordered']['delta_mean_us'],100)

    def test_pairwise_write_type(self):
        overwrites = pairwise_rows('overwrite')
        inserts = pairwise_rows('insert')
        for row in inserts:
            row['mean_us'] *= 2
        deletes = pairwise_rows('delete')
        for row in deletes:
            row['mean_us'] *= 1.5
        report = compare(overwrites + inserts + deletes, 'write-type', ids=PAIRWISE_IDS)
        self.assertEqual(set(report['configurations']), set(PAIRWISE_IDS))
        self.assertEqual(report['configuration_order'], list(PAIRWISE_IDS))
        self.assertIn('delete', report['comparisons'])
        self.assertIn('overwrite', report['comparisons'])
        self.assertIn('insert', report['comparisons'])
        self.assertIn('current_ordered_minus_any_unordered_btree', report['comparisons']['delete'])
        for config in report['configurations'].values():
            self.assertEqual(config['insert_overhead_percent'], 100)
            self.assertEqual(config['delete_overhead_percent'], 50)

    def test_btree_pairwise_archive(self):
        data = pairwise_rows() + pairwise_rows('insert') + pairwise_rows('delete')
        report = compare(data, 'write-type', ids=PAIRWISE_IDS)
        self.assertIn('current_ordered_minus_any_unordered_btree', report['comparisons']['delete'])

    def test_writesize_reader_accepts_active_btree_pair(self):
        report = compare(pairwise_rows(keys=4_000_000), 'writesize', ids=PAIRWISE_IDS, keys=4_000_000)
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'comparison.json'
            path.write_text(json.dumps(report))
            _, cases, differences = read_comparison(path)
        self.assertEqual(set(cases), set(PAIRWISE_IDS))
        self.assertIn('current_ordered_minus_any_unordered_btree', differences)

    def test_bad_pairwise_cases(self):
        data = pairwise_rows() + pairwise_rows('insert') + pairwise_rows('delete')
        for index, field, value in [(4, 'write_type', 'mixed'),
                                     (4, 'index_order', 'ordered'),
                                     (5, 'bitmap_chunk_bytes', '256')]:
            bad = [dict(row) for row in data]
            bad[index][field] = value
            compare(bad, 'write-type', success=False, ids=PAIRWISE_IDS)
        compare(data[:-1] + [data[-2]], 'write-type', success=False, ids=PAIRWISE_IDS)

    def test_bad_cases(self):
        compare(rows()[:-1], success=False)
        compare(rows()[:2]+[rows()[1]], success=False)
        for field, value in [('index_order','unordered'), ('bitmap_chunk_bytes','256'),
                             ('keyspace_keys','10000'), ('write_skew_exponent',1),
                             ('seed_synced','no'), ('commonware_page_cache','cleared'),
                             ('write_type','insert')]:
            bad = rows(); bad[-1][field] = value
            compare(bad, success=False)
        compare(rows()+rows('insert')[:-1], 'write-type', success=False)

    def test_rotation(self):
        script = f'source "{ROOT}/scripts/experiment-configs.sh"; for r in 1 2 3 4; do experiment_modes "$r"; echo "${{modes[*]}}"; done'
        output = subprocess.check_output(['bash','-c',script], text=True).splitlines()
        self.assertEqual(output, [' '.join(IDS), ' '.join(IDS[1:]+IDS[:1]), ' '.join(IDS[2:]+IDS[:2]), ' '.join(IDS)])

    def test_pairwise_rotation(self):
        script = f'source "{ROOT}/scripts/experiment-configs.sh"; for r in 1 2 3; do experiment_modes_pairwise "$r"; echo "${{modes[*]}}"; done'
        output = subprocess.check_output(['bash','-c',script], text=True).splitlines()
        self.assertEqual(output, ['any_unordered_btree current_ordered', 'current_ordered any_unordered_btree', 'any_unordered_btree current_ordered'])

    def test_active_studies_use_btree_pair(self):
        script = f'source "{ROOT}/scripts/experiment-configs.sh"; QMDB_STUDY_OPERATION=merkleize; experiment_study_modes 1; echo "${{modes[*]}}"; QMDB_STUDY_OPERATION=apply; experiment_study_modes 2; echo "${{modes[*]}}"'
        output = subprocess.check_output(['bash','-c',script], text=True).splitlines()
        self.assertEqual(output, ['any_unordered_btree current_ordered', 'current_ordered any_unordered_btree'])

    @unittest.skipUnless(HAS_MATPLOTLIB, "matplotlib not installed")
    def test_archives_and_plots(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for version in (1,2):
                paths=[]
                for count in (10000,100000,1000000):
                    for repeat in range(3):
                        folder=root/f'v{version}-{count}-{repeat}';folder.mkdir()
                        data=compare(rows())
                        data['write_calls_per_batch']=count
                        for case in data['configurations'].values():
                            case['write_calls_per_batch']=count
                            for metric in ('mean_us','mean_ci_low_us','mean_ci_high_us'):
                                case[metric] *= (1 + repeat * .1) * count / 10000
                        if version==1:
                            data={k:v for k,v in data.items() if k not in ('schema_version','configurations','comparisons')}
                            data.update(any=rows()[0], current=rows()[1], delta_mean_us=20, overhead_percent=20)
                        path=folder/'comparison.json';path.write_text(json.dumps(data));paths.append(path)
                        manifest=dict(run_id=folder.name, fixed_updates_per_batch=count, git_revision='fixture',
                                      host='fixture',os='fixture',cpu='fixture',rustc='fixture',timestamp_utc='fixture')
                        (folder/'manifest.json').write_text(json.dumps(manifest))
                        for name in ('source-hashes.txt','source-hashes-after.txt'):
                            (folder/name).write_text('fixture')
                        self.assertEqual(len(read_comparison(path)[1]), version+1)
                subprocess.run([sys.executable,str(ROOT/'experiments/writesize/plot.py'),str(paths[0])],check=True)
                subprocess.run([sys.executable,str(ROOT/'experiments/writesize/aggregate.py'),str(root/f'aggregate-v{version}'),*map(str,paths)],check=True)
                if version==1: legacy_paths=paths
                else:
                    mixed=subprocess.run([sys.executable,str(ROOT/'experiments/writesize/aggregate.py'),str(root/'mixed'),str(legacy_paths[0]),*map(str,paths[1:])],capture_output=True,text=True)
                    self.assertNotEqual(mixed.returncode,0)
                    self.assertIn('cannot mix',mixed.stderr)
                preview=Path('/tmp/qmdb-three-config-preview')
                if version==2:
                    subprocess.run([sys.executable,str(ROOT/'experiments/writesize/plot.py'),str(paths[0]),str(preview)],check=True)
                    import shutil
                    shutil.copyfile(root/'aggregate-v2/latency-sweep.png','/tmp/qmdb-three-config-sweep.png')

if __name__=='__main__': unittest.main()
