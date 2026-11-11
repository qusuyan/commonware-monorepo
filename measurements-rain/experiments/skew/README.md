# Write-skew experiment

Compare `any_unordered_btree` with `current_ordered` while changing the Zipf
exponent used to select overwritten seeded keys. Both variants use the same
nonpartitioned ordered B-Tree index type.

```bash
# Harness check with 10K keys and short Criterion windows.
measurements-rain/experiments/skew/run.sh --smoke

# Full default sweep: exponents 0, 0.5, 1, 1.5, and 2.
measurements-rain/experiments/skew/run.sh --updates 2000 --repeats 3
```

The sampler accepts any finite nonnegative exponent. Zero is uniform; positive
values favor hot keys. Sampling uses replacement, so skew also changes the
number of distinct keys affected by 2K write calls. Input generation and sampler
evaluation happen before timing. Timing covers staging, speculative
Merkleization, and root retrieval; batches are discarded.

Both cases use fixed 32-byte keys and values, MMB, 4M seeded keys for full runs,
direct batches, `s=true`, and `cc=false`. Current uses 32-byte chunks. Prepare
both 4M masters before a full run. Each exponent/repeat runs two cases and
alternates their order across repeats.

Results live under `measurements-rain/results/skew/<timestamp>/`. Each case has
its own manifest, exact command, log, raw Criterion files, and normalized
summary. `zipf<exponent>-r<repeat>-comparison.json` records
`current_ordered_minus_any_unordered_btree`; the group `summary.json` combines
all absolute cases.

Validation commands:

```bash
RUSTFLAGS='-A deprecated' cargo test -p commonware-storage --features test-traits --test qmdb_bench_sampling
measurements-rain/scripts/tests/test-skew.sh
```

Set `QMDB_STUDY_OPERATION=apply` for the apply-only timer. The selected pair is
unchanged.
