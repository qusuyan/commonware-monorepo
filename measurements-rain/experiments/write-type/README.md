# Overwrite, new-write, and deletion experiment

Compare speculative overwrites, inserts, and deletions on a committed 4M-key
QMDB. From the repository root, run the isolated 10K-write sweep:

```bash
measurements-rain/experiments/write-type/run.sh 10000 6
```

The first argument is write calls per timed batch; the second is independent
process repeats (default one). Each repeat runs six exact Criterion cases:
`any_unordered` and `current_ordered`, each with `w=overwrite`, `w=insert`,
and `w=delete`. Configuration order alternates across repeats; workload order
rotates across three repeats. Six repeats balance both orders together.
This reduces order bias; it does not eliminate machine drift or measurement noise.

Both configurations use fixed 32-byte keys and values, MMB,
4M seeded keys, direct batches (`ch=false`), a synced seed (`s=true`), and a
retained Commonware page cache (`cc=false`). Current uses 32-byte bitmap chunks.
The default snapshot reuse policy requires saved seeds for Current ordered:

```bash
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000
```

Any unordered builds its seed outside timing. Set `QMDB_SNAPSHOT_REUSE=off`
to build the Current ordered seed fresh outside timing as well.
`QMDB_MERKLEIZE_KEYS` can override the default key count.

Overwrites and deletions draw indices from the seeded range `0..K`; inserts draw
from the disjoint range `K..2K`. All sample uniformly with replacement. A batch
contains only its selected operation type, but repeated keys coalesce during
staging, so U write calls can produce fewer than U distinct mutations. Every
distinct insert key is absent from the seed; every distinct delete key is present.
Deletion does not draw values from the RNG, so its key sequence differs from
that of overwrites and inserts even with the same initial seed. Both engines use
the same deterministic generation rules for each workload. Criterion may run
different iteration counts per case, so whole-run samples are not paired batches.

Timing includes batch creation, write staging, speculative Merkleization, and
root retrieval. Keys and values are prepared before timing. Each measured batch
is discarded, without applying or committing it; every iteration sees the same
seed state. This measures maintaining proof-supporting structures, excluding
proof generation, verification, and durable mutation costs.

Outputs are archived under `measurements-rain/results/write-type/`. Each run
contains a manifest, exact commands, logs, raw Criterion data, normalized
summaries, and `comparison.json` (`schema_version: 2`). The comparison records
insert-minus-overwrite and delete-minus-overwrite differences per configuration,
and `current_ordered_minus_any_unordered` for each workload. The normalizer
also supports legacy six-row archives with three configurations and two workloads.
Mean differences and percentages are descriptive, without confidence intervals
for paired differences. Absolute observations retain Criterion means, confidence
intervals, and sample counts. Record load or thermal conditions in `notes.md`.

Any unordered uses its standard unordered index; Current ordered uses the ordered
B-Tree index. The delta includes this index difference, predecessor searches, larger ordered records,
operation-log amplification, current-state bitmap overlays, and Merkle grafting.
Any already has an authenticated operation log and a local activity bitmap.
The delta therefore measures combined design overhead, rather than exclusively
cryptographic hashing. Batch coalescing, shared predecessors, floor raising,
commit markers, and Merkle internal nodes prevent fixed per-call operation counts
from predicting an exact latency ratio.
