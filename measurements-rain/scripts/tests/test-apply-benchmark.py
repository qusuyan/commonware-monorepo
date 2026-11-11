#!/usr/bin/env python3
"""Small actual-binary apply checks; arguments: snapshot executable, qmdb executable.

Creates independent tiny masters, exercises both engines and all workloads, and removes
only artifacts created by this test. Timing observations are smoke data, not study results.
"""
import hashlib
import json
import math
import statistics
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

snapshot, benchmark = [str(Path(p).resolve()) for p in sys.argv[1:3]]
repo = Path(__file__).resolve().parents[3]
seeds = repo/'measurements-rain/storage-seeds'
pairs = [('any_unordered_btree', 'any_unordered_btree_fixed_mmb', 'any::unordered_btree::fixed::mmb'),
         ('current_ordered', 'current_ordered_fixed_mmb_chunk32', 'current::ordered::fixed::mmb chunk=32')]
count = next(k for k in range(131, 200) if all(not (seeds/f'k{k}_{suffix}').exists() for _,suffix,_ in pairs))
created = []


def run(exe, *args, env=None, success=True):
    r = subprocess.run([exe, *map(str,args)], cwd=repo, env=env, text=True, capture_output=True)
    assert (r.returncode == 0) == success, r.stdout+r.stderr
    return r


def inventory(path):
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in path.rglob('*') if p.is_file()}


try:
    with tempfile.TemporaryDirectory(prefix='qmdb-apply-smoke-') as tmp:
        keyfile = Path(tmp)/'keys.bin'
        run(snapshot, 'generate-keys', '--count', count, '--out', keyfile)
        env = dict(os.environ, QMDB_MERKLEIZE_KEYS=str(count), QMDB_MERKLEIZE_UPDATES='13',
                   QMDB_SNAPSHOT_REUSE='required', CRITERION_HOME=str(Path(tmp)/'criterion'))
        for variant, suffix, selector in pairs:
            def name(write_type, skew):
                return f'qmdb::apply_study/v={selector} k={count} u=13 w={write_type} zipf={skew} ch=false s=true cc=false'
            env.update(QMDB_MERKLEIZE_WRITE_TYPE='overwrite', QMDB_MERKLEIZE_SKEW='0')
            missing = run(benchmark, '--exact', name('overwrite',0), '--test', env=env, success=False)
            assert 'seed-db.sh' in missing.stderr
            master = seeds/f'k{count}_{suffix}'
            assert not master.exists()
            run(snapshot, 'seed-db', '--variant', variant, '--count', count, '--keys-file', keyfile)
            created.append(master)
            before = inventory(master)
            for kind in ['overwrite', 'insert', 'delete', 'mixed']:
                for skew in [0,2]:
                    env.update(QMDB_MERKLEIZE_WRITE_TYPE=kind, QMDB_MERKLEIZE_SKEW=str(skew))
                    for _ in range(2):
                        result = run(benchmark, '--exact', name(kind,skew), '--test', env=env)
                        setups = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                        assert setups and all(x['source']=='reused' and x['iteration_policy']=='independent_snapshot_fixed_inputs' for x in setups)
                        assert inventory(master) == before
            env.update(QMDB_MERKLEIZE_WRITE_TYPE='overwrite', QMDB_MERKLEIZE_SKEW='0')
            run(benchmark, '--bench', '--exact', name('overwrite',0), '--sample-size','10',
                '--warm-up-time','0.05','--measurement-time','0.05','--noplot', env=env)
            assert inventory(master)==before
        estimates = list((Path(tmp)/'criterion').glob('**/new/estimates.json'))
        assert len(estimates) == 2, 'expected one actual Criterion result per engine'
        raw_means = {}
        for path in estimates:
            sample = json.loads((path.parent/'sample.json').read_text())
            assert len(sample['iters']) == len(sample['times']) >= 10
            assert all(i > 0 for i in sample['iters']) and all(t > 0 for t in sample['times'])
            per_iteration_ns = [t/i for t,i in zip(sample['times'], sample['iters'])]
            mean_ns = json.loads(path.read_text())['mean']['point_estimate']
            assert math.isclose(mean_ns, statistics.mean(per_iteration_ns), rel_tol=1e-9), 'incorrect iteration normalization'
            identity = json.loads((path.parent/'benchmark.json').read_text())['full_id']
            raw_means[identity] = mean_ns
        # Exercise the real reporting pipeline on these actual samples, including ns -> us.
        results = repo/'measurements-rain/results'
        with tempfile.TemporaryDirectory(prefix='apply-latency-review-', dir=results) as archive:
            archive = Path(archive)
            shutil.copytree(Path(tmp)/'criterion', archive/'criterion')
            run_id = str(archive.relative_to(results))
            run(str(repo/'measurements-rain/scripts/summarize-criterion.sh'), run_id)
            normalized = json.loads((archive/'summary.json').read_text())
            assert len(normalized) == 2
            for row in normalized:
                assert row['operation'] == 'apply_study' and row['timing_scope'] == 'apply_batch'
                assert row['durability'] == 'buffered' and row['input_generation_timed'] is False
                assert math.isclose(row['mean_us'], raw_means[row['benchmark']]/1000, rel_tol=1e-9), 'incorrect latency units'
            report = subprocess.run(['jq', '-e', '--arg', 'experiment', 'writesize',
                '--argjson', 'keys', str(count), '--argjson', 'updates', '13', '--argjson', 'repeat', '1',
                '--argjson', 'skew', '0', '--arg', 'order', 'any_unordered_btree,current_ordered',
                '-f', str(repo/'measurements-rain/scripts/compare-experiment.jq'), str(archive/'summary.json')],
                capture_output=True, text=True)
            assert report.returncode == 0, report.stderr
            parsed = json.loads(report.stdout)
            assert parsed['operation'] == 'apply_study' and parsed['timing_scope'] == 'apply_batch'
            assert set(parsed['configurations']) == {'any_unordered_btree', 'current_ordered'}
        # The independent Merkleize family must remain selectable with the new registration.
        selector = pairs[0][2]
        name = f'qmdb::merkleize/v={selector} k={count} u=13 w=overwrite zipf=0 ch=false s=true cc=false'
        run(benchmark, '--exact', name, '--test', env=env)
    print('Both engines: 32 repeated workload checks, 2 timing smoke cases, raw latency normalization, real report units, master preservation and Merkleize selection passed')
finally:
    for master in created:
        shutil.rmtree(master)
