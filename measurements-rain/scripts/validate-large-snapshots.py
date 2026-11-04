#!/usr/bin/env python3
"""Sequential 4M/10M snapshot validation; preserves masters and records evidence.

Usage: validate-large-snapshots.py RUN_DIR SNAPSHOT_EXECUTABLE QMDB_EXECUTABLE
Build executables in release mode first. No existing artifact is removed.
"""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parents[2]
RUN, SNAPSHOT, BENCH = [Path(p).resolve() for p in sys.argv[1:4]]
RUN.mkdir(parents=True, exist_ok=True)
STATE = json.loads((RUN/'status.json').read_text()) if (RUN/'status.json').exists() else {'state': 'running', 'steps': [], 'artifacts': []}
if (RUN/'status.json').exists():
    (RUN/f'attempt-{len(list(RUN.glob("attempt-*-status.json")))+1}-status.json').write_text(json.dumps(STATE, indent=2))
STATE['state'] = 'running'
STATE.pop('error', None)
GIB = 1024 ** 3
PAIRS = [('any_unordered_btree', 'any_unordered_btree_fixed_mmb', 'any::unordered_btree::fixed::mmb'),
         ('current_ordered', 'current_ordered_fixed_mmb_chunk32', 'current::ordered::fixed::mmb chunk=32')]


def save():
    STATE['updated_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    temporary = RUN / '.status.tmp'
    temporary.write_text(json.dumps(STATE, indent=2))
    temporary.replace(RUN / 'status.json')


def checksum(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def sources():
    paths = [REPO/'Cargo.toml', REPO/'Cargo.lock', REPO/'storage/Cargo.toml']
    for folder in ('storage/src/qmdb', 'storage/src/index'):
        paths += sorted((REPO / folder).rglob('*.rs'))
    return {str(p.relative_to(REPO)): checksum(p) for p in paths}


BASE = sources()
if (RUN/'source-hashes.json').exists():
    if json.loads((RUN/'source-hashes.json').read_text()) != BASE:
        raise RuntimeError('cannot resume validation after source changes; use a new run directory')
else:
    (RUN / 'source-hashes.json').write_text(json.dumps(BASE, indent=2))


def rss_tree(pid):
    # The orchestrator may run under a sandbox that disallows process inspection.
    result = subprocess.run(['ps', '-axo', 'pid=,ppid=,rss='], capture_output=True, text=True)
    if result.returncode:
        return None
    rows = [tuple(map(int, line.split())) for line in result.stdout.splitlines()]
    children = {pid}
    while True:
        expanded = children | {p for p, parent, _ in rows if parent in children}
        if expanded == children:
            break
        children = expanded
    return sum(rss * 1024 for p, _, rss in rows if p in children)


def command(label, args, env=None):
    if sources() != BASE:
        raise RuntimeError('source files changed during validation; preserve results and rebuild before resuming')
    if shutil.disk_usage(REPO).free < 20 * GIB:
        raise RuntimeError('free disk fell below 20 GiB; stopping before next step')
    for old in reversed(STATE['steps']):
        same_command = old.get('command') == list(map(str,args))
        # Fresh comparison databases are disposable; their completed output remains evidence.
        disposable_fresh = '-fresh-' in label
        if old['label'] == label and old.get('exit_code') == 0 and (same_command or disposable_fresh):
            print('REUSE COMPLETED', label, flush=True)
            return (RUN/f'{label}.stdout.log').read_text()
    print('START', label, flush=True)
    STATE['current'] = label
    save()
    start = time.monotonic()
    out = RUN / f'{label}.stdout.log'
    err = RUN / f'{label}.stderr.log'
    peak = 0
    row = {'label': label, 'command': list(map(str, args)), 'started_utc': datetime.datetime.now(datetime.timezone.utc).isoformat()}
    if env:
        row['benchmark_environment'] = {k: v for k, v in env.items() if k.startswith(('QMDB_', 'CRITERION_'))}
    with out.open('w') as stdout, err.open('w') as stderr:
        proc = subprocess.Popen(['/usr/bin/time', '-l', *map(str, args)], cwd=REPO,
                                env=env, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            while proc.poll() is None:
                rss = rss_tree(proc.pid)
                if rss is not None:
                    peak = max(peak, rss)
                    if rss > 18 * GIB:
                        raise RuntimeError('process tree exceeded 18 GiB resident memory')
                if shutil.disk_usage(REPO).free < 20 * GIB:
                    raise RuntimeError('free disk fell below 20 GiB')
                if time.monotonic() - start > 1800:
                    raise RuntimeError('step exceeded 30-minute validation limit')
                time.sleep(2)
        except BaseException:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()
            raise
    row.update(exit_code=proc.returncode, elapsed_seconds=time.monotonic()-start,
               sampled_peak_rss_bytes=peak, free_disk_bytes=shutil.disk_usage(REPO).free)
    match = re.search(r'(\d+)\s+maximum resident set size', err.read_text())
    if match:
        row['peak_rss_bytes'] = int(match.group(1))
    STATE['steps'].append(row)
    save()
    if proc.returncode:
        raise RuntimeError(f'{label} failed with exit {proc.returncode}; see {err}')
    print('DONE', label, round(row['elapsed_seconds'], 2), 'seconds', flush=True)
    return out.read_text()


def inventory(root):
    return {str(p.relative_to(root)): {'bytes': p.stat().st_size, 'sha256': checksum(p)}
            for p in sorted(root.rglob('*')) if p.is_file()}


def verify_keys(path):
    if path.stat().st_size != 320_000_000:
        raise RuntimeError('10M keyset length mismatch')
    h = hashlib.sha256()
    prefixes = {}
    index = 0
    with path.open('rb') as f:
        for block in iter(lambda: f.read(32 * 32768), b''):
            for (key,) in struct.iter_unpack('32s', block):
                if key != hashlib.sha256(struct.pack('>Q', index)).digest():
                    raise RuntimeError(f'noncanonical key at rank {index}')
                h.update(key)
                index += 1
                if index in (4_000_000, 10_000_000):
                    prefixes[str(index)] = h.hexdigest()
    sidecar = json.loads(Path(str(path)+'.json').read_text())
    if index != 10_000_000 or sidecar['count'] != index or sidecar['checksum'] != h.hexdigest():
        raise RuntimeError('keyset sidecar mismatch')
    return {'count': index, 'bytes': path.stat().st_size, 'sha256': h.hexdigest(), 'prefix_digests': prefixes}


def validate_database(count, variant, suffix, selector, keyfile):
    label = f'k{count}-{variant}'
    master = REPO / 'measurements-rain/storage-seeds' / f'k{count}_{suffix}'
    command(label+'-seed', [SNAPSHOT, 'seed-db', '--variant', variant, '--count', count, '--keys-file', keyfile])
    before = inventory(master)
    inventory_file = RUN / f'{label}-inventory.json'
    if inventory_file.exists() and json.loads(inventory_file.read_text()) != before:
        raise RuntimeError('master files changed since previous validation attempt')
    inventory_file.write_text(json.dumps(before, indent=2))
    manifest = json.loads((master/'snapshot_manifest.json').read_text())
    if manifest['keyset_prefix_digest'] != STATE['keyset']['prefix_digests'][str(count)]:
        raise RuntimeError('snapshot does not identify expected canonical key prefix')
    command(label+'-inspect', [SNAPSHOT, 'inspect-db', '--db-path', master])
    command(label+'-idempotent', [SNAPSHOT, 'seed-db', '--variant', variant, '--count', count, '--keys-file', keyfile])
    observed = []
    for repeat in (1, 2):
        observed.append(json.loads(command(label+f'-exercise-r{repeat}', [SNAPSHOT, 'exercise-db', '--db-path', master, '--updates', 10000])))
    if observed[0] != observed[1]:
        raise RuntimeError('batch roots changed across independent reuse processes')
    with tempfile.TemporaryDirectory(prefix='qmdb-full-scale-fresh-') as temporary:
        fresh = Path(temporary)/'master'
        command(label+'-fresh-seed', [SNAPSHOT, 'seed-db', '--variant', variant, '--count', count,
                                    '--keys-file', keyfile, '--out', fresh])
        expected = json.loads(command(label+'-fresh-exercise', [SNAPSHOT, 'exercise-db', '--db-path', fresh, '--updates', 10000]))
        if expected != observed[0]:
            raise RuntimeError('fresh/reused initial or batch root mismatch')
    for write_type, skew in [('overwrite', '0'), ('insert', '0'), ('mixed', '0'), ('overwrite', '2')]:
        name = f'qmdb::merkleize/v={selector} k={count} u=10000 w={write_type} zipf={skew} ch=false s=true cc=false'
        env = dict(os.environ, QMDB_SNAPSHOT_REUSE='required', QMDB_MERKLEIZE_KEYS=str(count),
                   QMDB_MERKLEIZE_UPDATES='10000', QMDB_MERKLEIZE_WRITE_TYPE=write_type,
                   QMDB_MERKLEIZE_SKEW=skew, CRITERION_HOME=str(RUN/'criterion'/label))
        for repeat in (1, 2):
            text = command(label+f'-benchmark-{write_type}-s{skew}-r{repeat}', [BENCH, '--exact', name, '--test'], env)
            setup = [json.loads(line) for line in text.splitlines() if line.startswith('{') and json.loads(line).get('qmdb_setup')]
            if not setup or any(x.get('source') != 'reused' or x.get('initial_root') != manifest['merkle_root'] for x in setup if x.get('qmdb_setup')):
                raise RuntimeError('benchmark failed to report required snapshot reuse')
        if write_type == 'overwrite' and skew == '0':
            command(label+'-timing', [BENCH, '--bench', '--exact', name, '--sample-size', '10', '--warm-up-time', '0.1',
                                     '--measurement-time', '0.2', '--noplot'], env)
    if inventory(master) != before:
        raise RuntimeError('master files changed during validation')
    if not any((RUN/'criterion'/label).rglob('estimates.json')):
        raise RuntimeError('Criterion timing observations are missing')
    artifact = {'variant':variant, 'key_count':count, 'path':str(master), 'bytes':sum(x['bytes'] for x in before.values()),
                'initial_root':manifest['merkle_root'], 'configuration_fingerprint':manifest['configuration_fingerprint'],
                'batch_roots':observed[0]['batches'], 'master_unchanged':True}
    STATE['artifacts'] = [x for x in STATE['artifacts'] if (x['variant'],x['key_count']) != (variant,count)]
    STATE['artifacts'].append(artifact)
    save()


try:
    keyfile = REPO / 'measurements-rain/data/keys_10M.bin'
    command('keys-generate', [SNAPSHOT, 'generate-keys', '--count', 10000000])
    STATE['keyset'] = verify_keys(keyfile)
    initial_stat = keyfile.stat()
    command('keys-reload', [SNAPSHOT, 'generate-keys', '--count', 10000000])
    if (keyfile.stat().st_size, keyfile.stat().st_mtime_ns) != (initial_stat.st_size, initial_stat.st_mtime_ns):
        raise RuntimeError('idempotent key preparation rewrote key file')
    save()
    for count in (4_000_000, 10_000_000):
        for variant, suffix, selector in PAIRS:
            validate_database(count, variant, suffix, selector, keyfile)
    STATE['state'] = 'complete'
    STATE['current'] = 'complete'
    save()
    print('Full-scale validation complete', flush=True)
except BaseException as error:
    STATE['state'] = 'failed'
    STATE['error'] = str(error)
    save()
    raise
