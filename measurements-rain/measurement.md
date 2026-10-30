# QMDB Measurement Plan

## Main goal

Measure the performance cost of QMDB's authentication machinery, especially
the activity bitmap and its Merkleization, under realistic state-update
workloads.

The central question is:

> How much CPU time, latency, storage I/O, and retained state does QMDB add to
> authenticated key-value updates, and when does bitmap/grafting work become a
> bottleneck as the database and workload scale?

The study should separate the cost of the operation-log Merkle tree from the
cost of tracking and authenticating current state. It should ultimately connect
microbenchmark results to a realistic workload, such as the QMDB-backed
Avax-pEVM branch.

## QMDB configurations to compare

QMDB records every mutation in an append-only operation log. It can use either
of two append-friendly Merkle families:

- **MMR (Merkle Mountain Range):** append-only forest of perfect Merkle trees;
  new leaves can trigger several immediate merges.
- **MMB (Merkle Mountain Belt):** postpones some merges, smoothing append work
  and creating at most one internal node per append.

The most informative authentication comparison is:

```text
store     unauthenticated key-value/log baseline
any       Merkleized operation log + local, unmerkleized activity bitmap
current   Merkleized operation log + bitmap authenticated by grafting
```

`any` proves that a key/value appeared in history. Its bitmap is used locally
to identify live operations and advance the compaction floor, but the bitmap is
not committed into its root. `current` wraps `any` and proves whether an
operation is still active/current.

For `current`, complete bitmap chunks are combined with matching operation-log
subtrees:

```text
grafted leaf = hash(bitmap_chunk || operation_subtree_root)
```

The upper tree then hashes these grafted leaves into one root. This is called
**grafting**. It avoids needing independent operation-tree and bitmap-tree
proofs.

## Measurement questions

1. What is the end-to-end overhead of `current` relative to `any`?
2. How much of that overhead is from:
   - operation encoding and leaf hashing;
   - MMR/MMB operation-tree Merkleization;
   - local bitmap updates;
   - bitmap-chunk hashing;
   - grafted-leaf and upper grafted-tree hashing;
   - applying the batch, committing, and syncing it durably?
3. How do MMR and MMB change CPU work, latency distribution, memory/state,
   and cache behavior?
4. How does the overhead change with workload shape?
   - inserting a new key versus overwriting/deleting an existing key;
   - batch size and many small batches versus fewer large batches;
   - key overlap within and across batches (temporal locality);
   - uniform-random versus Zipf/skewed key selection;
   - database size and amount of inactive history;
   - bitmap chunk size (currently 32 and 256 bytes are implemented).
5. Which work can execute in parallel, and where must committed mutations be
   serialized?

## Metrics to record

For every configuration, report:

- wall-clock latency per batch: median, p95, and p99;
- throughput: operations and batches per second;
- CPU: instructions, cycles/estimated cycles, and cache misses;
- storage: blob reads/writes, bytes written, and `commit`/`sync` latency;
- state: operation-log length, inactivity floor, retained operations, and
  bitmap-pruned chunks;
- workload parameters: keyspace, on-disk database size, batch size, update
  mix, overlap/locality, skew, Merkle family, index type, and bitmap chunk
  size.

The `any` versus `current` comparison must use identical generated operations,
keys, values, and database state. Otherwise, a difference cannot be attributed
to bitmap authentication.

## Initial local study

The initial experiments are defined in [`measureplan.md`](measureplan.md), which
is the operational source of truth for the local personal-machine runs. The
study changes one workload dimension at a time:

- database size: `100K`, `1M`, `2M`, and `4M` keys;
- fixed database size for the other sweeps: `4M` keys;
- write size: `200`, `500`, `1,000`, `1,500`, `2,000`, `5,000`, `7,000`, and
  `10,000` absolute write calls per batch;
- write skewness: Zipf exponents `0`, `0.5`, `1.0`, `1.5`, and `2.0`, where
  `0` is uniform;
- write type: separate 100% overwrite, 100% new-write, and 100% deletion
  workloads.

The initial study runs sequentially because it targets a local machine with
limited storage. It records database size, latency, throughput, write-call
count, distinct-key count, and the exact run parameters. Larger states,
additional representation variants, and realistic application workloads are
follow-up phases rather than prerequisites for this initial matrix.

## Existing measurement support

The Commonware checkout already provides useful starting points.

- `storage/src/qmdb/benches/merkleize.rs`
  - Criterion benchmark for speculative batch construction and `merkleize()`.
  - Covers `any` and `current`, MMR/MMB, ordered/unordered indexes,
    fixed/variable values, and 32/256-byte bitmap chunks.
  - Includes fresh, synced/cache-cleared, chained-batch, and churned-overwrite
    scenarios; reports blob reads.
  - Supports independent write counts, Zipf skew exponents, and overwrite,
    insert, and mixed write modes through benchmark configuration.
- `storage/src/qmdb/benches/merkleize_gungraun.rs`
  - Callgrind/Gungraun CPU profiling for two representative configurations.
  - Reports instruction count, cache behavior, memory read/write activity, and
    estimated cycles. It should be extended to the full comparison matrix.
- `storage/src/qmdb/benches/constantinople.rs`
  - Block-shaped harness: default 1M keys, 32K reads, and 32K updates.
  - Separates staged-load and speculative-merkleization latency for selected
    `any` and `current` MMB configurations.
- `storage/src/qmdb/benches/chained_growth.rs`
  - Measures `current` Merkleization with long speculative batch chains and
    sparse updates.
- `storage/src/qmdb/benches/generate.rs` and `init.rs`
  - Measure write-heavy database construction and startup/replay.
- `storage/src/qmdb/benches/scale.rs`
  - Builds a reusable multi-GB `any` database, supports uniform or Zipfian
    generation, and measures initialization plus cold/warm `get` and
    `get_many` performance at configurable concurrency.

QMDB also exports operation-level metrics for apply, get, commit, sync, prune,
and state boundaries in `storage/src/qmdb/metrics.rs`.

## What is not yet measured cleanly

The current benchmark work does not yet isolate bitmap mutation, bitmap
hashing, and grafted-tree hashing as separate CPU components. It also does not
yet provide a complete `store`/`any`/`current` end-to-end suite, a deletion
mode in the `merkleize` write-type benchmark, or a concurrent-writer
throughput test. Overwrite-versus-insert sweeps, independent write-count
sweeps, and Zipf-controlled write selection are implemented; the initial
matrix and local execution constraints are documented in `measureplan.md`.

Committed database mutations are intentionally serialized by QMDB's ownership-
based API: a mutating call consumes the database handle and returns it on
success. Speculative batches and internal computation can still use parallelism,
but a separate harness is needed to quantify application-level multiwriter
contention and batching.

## Proposed experiment sequence

### 1. Establish whole-system baselines

First run the initial local matrix from `measureplan.md` for `any` and
`current`, changing one of database size, absolute write count, Zipf exponent,
or exclusive write type at a time. Use identical seeded state and generated
operations for each comparison. After the initial matrix, extend the study to:

- MMR and MMB;
- ordered and unordered index variants;
- 32-byte and 256-byte chunks for `current`;
- warm and cache-cleared state;
- fresh and churned-overwrite state.

This establishes the practical cost of current-state authentication before
adding instrumentation and broader workload variants.

### 2. Attribute CPU work

Add precomputed-input microbenchmarks that time only:

1. operation digest/leaf construction;
2. MMR or MMB operation-tree batch Merkleization;
3. activity-bit changes with no hashing;
4. bitmap-chunk digest construction;
5. `hash(bitmap_chunk || operation_subtree_root)` for grafted leaves;
6. upper grafted-tree Merkleization.

Run each under Callgrind/Gungraun. Compare the sum of these components with
end-to-end `current::merkleize()`; the remaining gap captures index lookups,
batch bookkeeping, allocations, and control flow.

### 3. Sweep workload dimensions

For the initial baseline, use the exact matrix in `measureplan.md`. For later
baselines and component benchmarks, continue to vary one dimension at a time:

- keyspace/on-disk size beyond the initial 4M-key local limit;
- batch size beyond the initial absolute write-count sweep;
- update mix beyond the initial all-new, all-overwrite, and all-delete cases;
- temporal locality: disjoint keys, repeated keys in one batch, and hot keys
  across batches;
- distribution: uniform and several Zipf exponents;
- durability mode: merkleize only, apply, commit, and sync.

### 4. Validate against a realistic workload

Use the QMDB-backed Avax-pEVM workload after the microbenchmark trends are
understood. Reproduce the key workload statistics where possible: key/value
size, batch size, overwrite rate, locality, and persistence cadence. Report
both end-to-end latency and the share attributable to QMDB authentication.

## Expected outputs

The final report should include:

- end-to-end latency/throughput curves for `store`, `any`, and `current`;
- a CPU breakdown of operation-tree versus bitmap/grafting work;
- MMR-versus-MMB comparison curves;
- scaling curves over database size, batch size, overwrite rate, and skew;
- a statement of the serialization point and any observed benefit from
  parallel batch computation;
- a realism check using the Avax-pEVM workload.
