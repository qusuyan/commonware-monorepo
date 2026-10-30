# Write skew experiment

This script compares `any` and `current` speculative merkleization while changing
the exponent controlling key selection. The sampler accepts any finite exponent
greater than or equal to zero. The script's default sweep is `0 0.5 1 1.5 2`;
positional arguments replace that sweep.

```bash
# Functionality check: 10K seeded keys, 1K writes, shortened Criterion measurements.
measurements-rain/experiments/skew/run.sh --smoke

# An arbitrary sweep, including values outside the default list.
measurements-rain/experiments/skew/run.sh --smoke 0 0.75 3

# Full experiment: 10M seeded keys, 10K writes, three process runs per case.
measurements-rain/experiments/skew/run.sh --updates 10000 --repeats 3
```

Each case uses unordered fixed MMB, 32-byte current chunks, no pending parent,
a synced seed, and retained Commonware page cache. It overwrites seeded keys,
sampling with replacement. Exponent zero uses the original uniform generator;
positive exponents favor hot keys. A fixed number of write calls can therefore
change fewer distinct keys as skew increases.

One timer measures one batch: random key selection, key hashing, value generation,
batch writes, merkleization, and root retrieval. Sampler construction and database
seeding are outside timing. The batch is discarded; this does not accumulate
committed hot-key history. Runtime differences include the cost of the sampler.
A future experiment will prepare inputs outside timing for both uniform and Zipf.

Results live in `measurements-rain/results/skew/<UTC timestamp>/`. Every case has
its own manifest, exact command, console log, raw Criterion files, and CSV/JSON
summary. The group `summary.json` combines all cases. Smoke timings validate the
harness and metadata; use full measurements for performance conclusions.

The dedicated `target/measurements/skew-build/` build is reused across cases and
locked against overlapping script runs. Only its archived Criterion data is
cleared between cases. Existing write-size and write-type scripts clear the skew
environment variable so their original workloads remain uniform.

Criterion shortens long names in its console output. The normalizer uses the full
ID from each raw `benchmark.json`, preserving the exponent and all other settings.

## Validation

```bash
RUSTFLAGS='-A deprecated' cargo test -p commonware-storage --features test-traits --test qmdb_bench_sampling
measurements-rain/scripts/tests/test-skew.sh
```

The Rust tests cover arbitrary exponents, invalid inputs, bounds, reproducible
sequences, zero-skew compatibility, duplicate concentration, and write-type ranges.
The shell checks validate configuration handling and legacy/new summary parsing.
The deprecation flag matches the other measurement scripts and accommodates the
existing `cpufeatures` dependency with the installed compiler.
