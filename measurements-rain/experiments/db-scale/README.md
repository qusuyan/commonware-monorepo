# Database-scaling experiment

Measure how keyspace size affects `any_unordered_btree` and `current_ordered`
at a fixed 2K uniform-overwrite batch. Both variants use the same
nonpartitioned ordered B-Tree index type.

The default sweep covers 1M, 2M, 4M, and 10M seeded keys:

```bash
measurements-rain/experiments/db-scale/run.sh

# Optional subset or explicit settings.
measurements-rain/experiments/db-scale/run.sh --updates 2000 --repeats 3 1000000 4000000
```

Each key count/repeat runs two exact Criterion cases. Configuration order
alternates across repeats. Input generation is outside timing. A timed
iteration covers staging, speculative Merkleization, and root retrieval, then
discards the batch.

Snapshot reuse is required by default. Prepare both variants for every selected
key count before the sweep:

```bash
for keys in 1000000 2000000 4000000 10000000; do
  measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count "$keys"
  measurements-rain/scripts/seed-db.sh --variant current_ordered --count "$keys"
done
```

Results are archived under `measurements-rain/results/db-scale/<timestamp>/`.
Each `k<count>-r<repeat>-comparison.json` records
`current_ordered_minus_any_unordered_btree`.

Set `QMDB_STUDY_OPERATION=apply` for the apply-only timer. The configuration
pair remains unchanged.

