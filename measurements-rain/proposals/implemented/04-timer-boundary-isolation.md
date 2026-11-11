# Proposal 04: Timer Boundary Isolation & Input Pre-Generation

- **Proposal ID:** IMP-04
- **Status:** Implemented (Working tree changes in `merkleize.rs`, `common.rs`, and tests)
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)
  - [`storage/tests/qmdb_prepared_writes.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_prepared_writes.rs)
  - [`measurements-rain/scripts/write-manifest.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/write-manifest.sh)

---

## 1. Problem Statement & Motivation

In previous versions of the benchmark harness, the timed loop (`iter()`) executed both **input workload generation** and **database execution**:

```rust
// Legacy iter() method:
async fn iter(&mut self) -> Self::Output {
    let batch = db.new_batch();
    // ⚠️ RNG sampling, key hashing, and payload allocations were TIMED!
    let batch = write_random_writes(batch, num_updates, self.options.num_keys, &mut self.rng, write_type);
    let merkleized = batch.merkleize(db, None).await.unwrap();
    merkleized.root()
}
```

This violated a fundamental principle of systems benchmarking:
- **Measurement Pollution:** Generating random numbers with `TestRng`, computing SHA-256 hashes for 10,000 keys, allocating temporary Rust vectors, and evaluating rejection sampling loops were included in the reported database latency.
- **Confounding Fast Paths:** For fast operations (like inserts in `current::unordered` which take ~4 ms), input generation represented up to **20–30% of the entire timed interval**, skewing comparative ratios between configurations.

---

## 2. Design & Technical Implementation

### A. Separation into `before_iter()` and `iter()`

We re-architected the benchmark lifecycle using Criterion's two-stage execution model:
1. **`before_iter()` (Untimed Phase):** Samples key ranks using `WriteSampler`, synthesizes key/value pairs, and buffers them into a pre-allocated vector (`prepared_writes: Vec<(Digest, Digest)>`).
2. **`iter()` (Timed Phase):** Only executes database staging (`batch.write(k, v)`), batch Merkleization (`batch.merkleize()`), and canonical root extraction (`merkleized.root()`).

```rust
// In storage/src/qmdb/benches/merkleize.rs:

async fn before_iter(&mut self) {
    if self.options.chained {
        // Untimed parent batch preparation...
    }
    let num_updates = self.options.num_updates();
    let write_type = self.options.write_type.unwrap_or(WriteType::Overwrite);
    
    // Generate writes into pre-allocated buffer outside timing:
    if self.options.skew.unwrap_or(0.0) > 0.0 {
        prepare_sampled_writes(&mut self.prepared_writes, num_updates, &mut self.rng, write_type, &self.sampler);
    } else {
        prepare_random_writes(&mut self.prepared_writes, num_updates, self.options.num_keys, &mut self.rng, write_type);
    }
}

async fn iter(&mut self) -> Self::Output {
    let db = self.db.as_ref().unwrap();
    let batch = if self.options.chained {
        db.new_batch_from_parent(self.parent.as_ref().unwrap())
    } else {
        db.new_batch()
    };
    
    // ONLY database batch staging and Merkle tree computation are timed:
    let batch = stage_prepared_writes(batch, &self.prepared_writes);
    let merkleized = batch.merkleize(db, None).await.unwrap();
    merkleized.root()
}
```

### B. Invariant Preservation & Equivalence Testing

To guarantee that pre-generation did not inadvertently alter the sequence of random numbers or database state, we implemented comprehensive equivalence tests in [`storage/tests/qmdb_prepared_writes.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_prepared_writes.rs).

The test suite runs an exhaustive permutation of:
- All skew exponents ($s \in \{0.0, 0.75, 1.0, 3.0\}$)
- All write types (`Overwrite`, `Insert`, `Mixed`)
- Multiple batch sizes ($0, 1, 16, 32$)
- Asserts that `stage_prepared_writes(Writes::default(), &inputs)` produces the **exact bit-for-bit identical vector of writes** as the legacy inline `write_random_writes` and advances the RNG state identically.

---

## 3. Impact & Manifest Tracking

All run manifests now explicitly record this boundary condition:
```json
{
  "merkleize_input_generation_timed": false,
  "timing_scope": "batch_creation_staging_merkleization_root"
}
```

By removing CPU cycles spent on SHA-256 key pre-hashing and PRNG draws from Criterion timers, reported latencies now strictly measure the **internal cryptographic and index mechanisms of QMDB**.
