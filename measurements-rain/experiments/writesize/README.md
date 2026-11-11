# Write-size experiment

Compare `any_unordered_btree` with `current_ordered` while varying write calls
per speculative batch on a 4M-key QMDB. Both variants use the same
nonpartitioned ordered B-Tree index type. This controls the index implementation
while retaining their different record and authentication behavior.

Run from the repository root:

```bash
for updates in 200 500 1000 1500 2000 5000 7000 8000 10000; do
  measurements-rain/experiments/writesize/run.sh "$updates" 3
done
```

The second argument is the number of independent process repeats. Each repeat
runs two exact Criterion cases in separate processes. Their order alternates
between Any/Current and Current/Any. The build is reused from
`target/measurements/writesize-build/`.

Both cases use fixed 32-byte keys and values, MMB, 4M seeded keys, `ch=false`,
`s=true`, and `cc=false`. Current uses 32-byte bitmap chunks. Updates are
uniform overwrites sampled with replacement, so the selected count is write
calls rather than guaranteed distinct keys. Timing covers staging, speculative
Merkleization, and root retrieval; the batch is discarded.

Because both synced-seed variants support snapshot reuse, prepare both 4M
masters before a full run:

```bash
measurements-rain/scripts/seed-db.sh --variant any_unordered_btree --count 4000000
measurements-rain/scripts/seed-db.sh --variant current_ordered --count 4000000
```

Each group is archived under
`measurements-rain/results/writesize/<timestamp>-u<count>-r<repeat>/` with a
manifest, exact commands, `any_unordered_btree.log`, `current_ordered.log`, raw
Criterion data, normalized summaries, and `comparison.json`. The comparison key
is `current_ordered_minus_any_unordered_btree`.

Generate one group plot with:

```bash
python3 measurements-rain/experiments/writesize/plot.py \
  measurements-rain/results/writesize/<run-id>/comparison.json
```

After three groups at each count, `aggregate.py` combines comparison paths,
checks provenance, and writes CSV/JSON plus PNG/PDF sweep plots. Error bars are
sample standard deviations across process groups. The readers retain support
for legacy standard-Any pairs and three-configuration archives, but one
aggregate cannot mix configuration sets.

The pairwise delta is descriptive and has no paired-difference confidence
interval. It includes the Current record layout, authenticated bitmap activity,
predecessor handling, and Merkle grafting; it is not an isolated hashing cost.

