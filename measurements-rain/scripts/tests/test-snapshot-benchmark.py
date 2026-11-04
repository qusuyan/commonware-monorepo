#!/usr/bin/env python3
"""Tiny end-to-end Criterion smoke checks. Pass snapshot CLI and qmdb executables.

Creates fewer than 200 keys and removes only the test-created default masters.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

snapshot, benchmark = map(lambda p: str(Path(p).resolve()), sys.argv[1:3])
repo = Path(__file__).resolve().parents[3]
seeds = repo / 'measurements-rain/storage-seeds'
pairs = [('any_unordered_btree', 'any_unordered_btree_fixed_mmb', 'any::unordered_btree::fixed::mmb'),
         ('current_ordered', 'current_ordered_fixed_mmb_chunk32', 'current::ordered::fixed::mmb chunk=32')]
count = next(k for k in range(113, 200) if all(not (seeds / f'k{k}_{suffix}').exists() for _, suffix, _ in pairs))
created = []


def run(binary, *args, env=None, success=True):
    result = subprocess.run([binary, *map(str, args)], cwd=repo, env=env, capture_output=True, text=True)
    assert (result.returncode == 0) == success, result.stdout + result.stderr
    return result


def inventory(path):
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in path.rglob('*') if p.is_file()}


try:
    with tempfile.TemporaryDirectory(prefix='qmdb-smoke-') as tmp:
        keyfile = Path(tmp) / 'keys.bin'
        run(snapshot, 'generate-keys', '--count', count, '--out', keyfile)
        env = dict(os.environ, QMDB_MERKLEIZE_KEYS=str(count), QMDB_MERKLEIZE_UPDATES='13')
        env.pop('QMDB_SNAPSHOT_REUSE', None)
        for variant, suffix, selector in pairs:
            name = f'qmdb::merkleize/v={selector} k={count} u=13 ch=false s=true cc=false'
            missing = run(benchmark, '--exact', name, '--test', env=env, success=False)
            assert 'seed-db.sh' in missing.stderr
            master = seeds / f'k{count}_{suffix}'
            # Refuse an unexpected existing path rather than deleting anyone's artifact.
            assert not master.exists()
            run(snapshot, 'seed-db', '--variant', variant, '--count', count, '--keys-file', keyfile)
            created.append(master)
            before = inventory(master)
            roots = []
            for _ in range(2):
                result = run(benchmark, '--exact', name, '--test', env=env)
                records = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                setup = next(x for x in records if x.get('qmdb_setup'))
                assert setup['source'] == 'reused' and setup['reuse_mode'] == 'required'
                roots.append(setup['initial_root'])
                assert inventory(master) == before
            fresh_env = dict(env, QMDB_SNAPSHOT_REUSE='off')
            result = run(benchmark, '--exact', name, '--test', env=fresh_env)
            setup = next(json.loads(line) for line in result.stdout.splitlines() if line.startswith('{'))
            assert setup['source'] == 'fresh' and setup['initial_root'] == roots[0] == roots[1]
            assert inventory(master) == before
        # Selecting an unsupported case must not resolve the filtered-out supported pair.
        name = f'qmdb::merkleize/v=any::unordered::fixed::mmb k={count} u=13 ch=false s=true cc=false'
        run(benchmark, '--exact', name, '--test', env=env)
        run(benchmark, '--exact', name, '--test', env=dict(env, QMDB_SNAPSHOT_REUSE='invalid'), success=False)
    print('Required reuse, repeat reuse, fresh equivalence, filtering and master preservation passed')
finally:
    for master in created:
        shutil.rmtree(master)
