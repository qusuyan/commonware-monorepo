# Overwrite, insert, and deletion experiment

Compare pure overwrite, insert, and delete batches between
`any_unordered_btree` and `current_ordered` on a committed 4M-key QMDB:

```bash
measurements-rain/experiments/write-type/run.sh 2000 3
```

The first argument is write calls per batch and the second is independent
process repeats. Each repeat runs six exact Criterion cases: two configurations
times three pure workload types. Configuration order alternates and workload
order rotates across repeats. A measured batch never mixes operation types.

Both configurations use fixed 32-byte keys and values, MMB, the same
nonpartitioned ordered B-Tree index type, direct batches, `s=true`, and
`cc=false`. Current uses 32-byte chunks. Prepare both masters before full runs:

```bash
measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count 4000000
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000
```

Overwrites and deletes sample seeded keys from `0..K`; inserts sample absent
keys from `K..2K`. Sampling is uniform with replacement, so repeated calls can
coalesce into fewer distinct mutations. Keys and values are prepared before
timing. Timing covers staging, speculative Merkleization, and root retrieval;
the batch is discarded.

Outputs live under `measurements-rain/results/write-type/`. Comparison JSON
records `current_ordered_minus_any_unordered_btree` for each workload and the
insert-minus-overwrite and delete-minus-overwrite values within each
configuration. Absolute cases retain Criterion means, confidence intervals,
and sample counts; pairwise differences are descriptive.

Set `QMDB_STUDY_OPERATION=apply` to use the apply-only timing boundary described
in [the apply study](../apply/README.md). The selected configuration pair stays
the same.
