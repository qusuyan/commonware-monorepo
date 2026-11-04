# Proposal 06: Overwrite, Insert, and Delete: `any+unordered` vs. `current+ordered`

- **Proposal ID:** PROP-06
- **Status:** Implemented / Ready for Review
- **Author:** Rain Sun
- **Target Components:**
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)
  - [`storage/tests/qmdb_prepared_writes.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_prepared_writes.rs)
  - [`measurements-rain/experiments/write-type/`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/experiments/write-type)
  - [`measurements-rain/scripts/experiment-configs.sh`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/experiment-configs.sh)
  - [`measurements-rain/scripts/compare-experiment.jq`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/compare-experiment.jq)
  - [`measurements-rain/scripts/tests/test-experiment-comparisons.py`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/measurements-rain/scripts/tests/test-experiment-comparisons.py)

---

## 1. Scientific Motivation & Confounding Variable Elimination

The primary research goal of our project is to quantify the **combined current-state and ordered-design overhead** of authenticated storage:

> **What is the true performance cost of full authentication (current-state inclusion proofs via bitmap grafting + negative exclusion proofs via ordered predecessor chains) compared to an authenticated operation-log baseline?**

Compare the standard `any::unordered::fixed::mmb` baseline with
`current::ordered::fixed::mmb chunk=32`. The Any variant uses the standard
unordered index; Current ordered uses a B-Tree index. Both retain an authenticated
operation log. Current additionally supports authenticated current state and
ordered exclusion proofs.

This comparison measures combined design overhead, including the index difference,
predecessor searches, ordered-record sizes, operation-log amplification, bitmap
overlays, and Merkle grafting. It cannot isolate cryptographic hashing alone.

---

## 2. The Tri-Workload Spectrum (100% Pure, No Mixed)

We evaluate this head-to-head comparison across three mutually disjoint, 100% pure workloads on a pre-seeded database of $N$ keys ($K_0 \dots K_{N-1}$):

$$\text{Workloads} = \{\text{100\% Overwrites}, \text{100\% Inserts (New Writes)}, \text{100\% Deletions}\}$$

Each batch contains only the selected operation type. Sampling is with replacement:
repeated keys coalesce during staging, so U calls need not produce U distinct
mutations. Inserts target keys absent from the seed; deletions target keys present
in the seed. Measured batches are discarded, so later iterations retain the same
starting state. Deletions skip value RNG draws and therefore use a different key
sequence from the other workloads. Generation is deterministic within each workload.

Both engines maintain an authenticated operation log and local activity bitmap.
Current additionally computes speculative bitmap overlays and Merkle grafting;
ordered inserts and deletions can rewrite surviving predecessors' successor links.
Counts depend on distinct keys, shared predecessors, floor raising, commit markers,
and Merkle internal nodes. They are not a fixed one-versus-two hash count per call.
Database index and persistent bitmap changes happen on apply; this benchmark times
speculative staging and Merkleization without apply or commit.

---

## 3. Attribution and Research Hypotheses

For each workload, report the mean latency of `current+ordered` minus the mean
latency of `any+unordered`, plus the percentage relative to the latter.
These are descriptive differences, not confidence intervals for paired batches.
Criterion may choose different iteration counts for each engine.

Overwrites exercise replacement and current-state grafting without introducing
new keys. Inserts and deletions additionally exercise ordered-chain maintenance.
These mechanisms suggest workload-dependent overhead, but neither a fixed 2x
ratio nor a 15-30x ratio follows from the code. A small smoke run validates
execution and result parsing; repeated controlled runs are needed for performance
claims. Rotate workload order and alternate engine order across six repeats to
balance both schedules. This reduces order bias without eliminating machine drift.

---

## 4. Technical Implementation Plan

The measured variants are the existing standard Any unordered and Current ordered
engines. B-Tree-backed Any archives remain readable by the comparison normalizer,
but the write-type runner selects standard Any unordered.

### Phase 1: Benchmark Workload Engine — `WriteType::Delete`

1. **Extend `WriteType` Enum in `storage/src/qmdb/benches/common.rs`:**
   ```rust
   #[derive(Clone, Copy, Debug, Eq, PartialEq)]
   pub enum WriteType {
       Overwrite,
       Insert,
       Delete,
       Mixed,
   }

   impl WriteType {
       pub const fn name(self) -> &'static str {
           match self {
               Self::Overwrite => "overwrite",
               Self::Insert => "insert",
               Self::Delete => "delete",
               Self::Mixed => "mixed",
           }
       }
   }
   ```

2. **Decouple Value Storage to `Option<Digest>`:**
   Update input preparation buffer from `Vec<(Digest, Digest)>` to `Vec<(Digest, Option<Digest>)>`:
   ```rust
   pub fn prepare_sampled_writes(
       writes: &mut Vec<(Digest, Option<Digest>)>,
       num_writes: u64,
       rng: &mut TestRng,
       write_type: WriteType,
       sampler: &WriteSampler,
   ) {
       writes.clear();
       for _ in 0..num_writes {
           let sampled = sampler.sample(rng);
           match write_type {
               WriteType::Overwrite => {
                   let key = Sha256::hash(&[&sampled.to_be_bytes()]);
                   writes.push((key, Some(make_fixed_value(rng))));
               }
               WriteType::Insert => {
                   let idx = sampled + sampler.num_keys;
                   let key = Sha256::hash(&[&idx.to_be_bytes()]);
                   writes.push((key, Some(make_fixed_value(rng))));
               }
               WriteType::Delete => {
                   // Deletions target existing seeded keys in [0, num_keys)
                   let key = Sha256::hash(&[&sampled.to_be_bytes()]);
                   writes.push((key, None));
               }
               WriteType::Mixed => {
                   let insert = rng.next_u32().is_multiple_of(2);
                   let idx = if insert { sampled + sampler.num_keys } else { sampled };
                   let key = Sha256::hash(&[&idx.to_be_bytes()]);
                   writes.push((key, Some(make_fixed_value(rng))));
               }
           }
       }
   }
   ```

3. **Stage Staged Writes:**
   Update `stage_prepared_writes` to pass `Option<Digest>` directly into `batch.write(key, value)`:
   ```rust
   pub fn stage_prepared_writes<B, Db>(mut batch: B, writes: &[(Digest, Option<Digest>)]) -> B
   where
       B: UnmerkleizedBatch<Db, K = Digest, V = Digest>,
       Db: ?Sized,
   {
       for &(key, value) in writes {
           batch = batch.write(key, value);
       }
       batch
   }
   ```

4. **Harness Integration in `storage/src/qmdb/benches/merkleize.rs`:**
   - Add CLI parsing for `QMDB_MERKLEIZE_WRITE_TYPE=delete`.
   - Update `MerkleizeWorkload.prepared_writes` from `Vec<(Digest, Digest)>` to `Vec<(Digest, Option<Digest>)>`.

### Phase 2: Pairwise Runner & Evaluation Pipeline

1. **Pairwise Configuration Selection (`measurements-rain/scripts/experiment-configs.sh`):**
   Configure the clean 2-configuration selection:
   ```bash
   experiment_modes_pairwise() {
       case $(( ($1 - 1) % 2 )) in
           0) modes=(any_unordered current_ordered) ;;
           1) modes=(current_ordered any_unordered) ;;
       esac
   }

   experiment_variant() {
       case "$1" in
           any_unordered) variant="any::unordered::fixed::mmb" ;;
           current_ordered)     variant="current::ordered::fixed::mmb chunk=32" ;;
           *) echo "unknown configuration: $1" >&2; return 1 ;;
       esac
   }
   ```

2. **Runner Extension (`measurements-rain/experiments/write-type/run.sh`):**
   - Sweeps 2 configurations (`any_unordered` and `current_ordered`) across 3 pure workloads (`overwrite`, `insert`, `delete`).
   - Total rows per repeat: $2 \text{ configurations} \times 3 \text{ write types} = 6 \text{ rows}$.

3. **Schema & Comparison (`measurements-rain/scripts/compare-experiment.jq`):**
   - Parse both standard `unordered` results and historical `unordered_btree` results.
   - Use `any_unordered` as the baseline for the standard runner.
   - Output pairwise comparison for each write type:
     ```jq
     current_ordered_minus_any_unordered: {
         delta_mean_us: ($ordered.mean_us - $any_unordered.mean_us),
         overhead_percent: (100 * ($ordered.mean_us / $any_unordered.mean_us - 1))
     }
     ```

4. **Python Comparison Test (`measurements-rain/scripts/tests/test-experiment-comparisons.py`):**
   - Update test fixtures to validate 6-row pairwise comparisons with `delete`.

---

## 5. Validation Plan & Quality Controls

1. **Deterministic RNG & Preparation Test:**
   - In [`storage/tests/qmdb_prepared_writes.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/tests/qmdb_prepared_writes.rs), run `preparation_preserves_operations_and_rng_across_iterations` with `WriteType::Delete`.
   - Assert `prepared_rng.next_u64() == original_rng.next_u64()`.
   - Assert all prepared delete entries have `value == None` and sampled indices in `0..num_keys`.

2. **Key Eradication Integration Test:**
   - In `storage/tests/qmdb_prepared_writes.rs`, seed a test database with 100 keys.
   - Execute a batch of 20 deletions of existing keys.
   - Assert:
     1. Speculative root changes: $\text{root}_{\text{post}} \ne \text{root}_{\text{pre}}$.
     2. Committed keys return `None`: `db.get(deleted_key) == None`.
     3. Untouched keys remain readable: `db.get(surviving_key) == Some(val)`.
     4. The active key count decreases by the number of distinct deleted keys, which can be less than 20. Check all survivor values and verify that discarding a speculative batch leaves the seed unchanged.
   - Exercise all three workloads on both measured variants: `any::unordered` and `current::ordered` with MMB.

3. **Small-Scale Smoke Benchmark:**
   - Run Criterion smoke tests ($K = 10\text{K}, U = 100$) across `any::unordered::fixed::mmb` and `current::ordered::fixed::mmb chunk=32` for `overwrite`, `insert`, and `delete`.
   - Confirm zero panics, valid Criterion output, and accurate JSON parsing.

4. **Pipeline Test:**
   - Run `python3 measurements-rain/scripts/tests/test-experiment-comparisons.py`.

---

## 6. Execution Order & Next Steps

1. **User Review:** Review this proposal and confirm approval.
2. **Implementation (Deletion Workloads & Pipeline):**
   - Apply `WriteType::Delete` and `Option<Digest>` buffer refactoring in `common.rs` and `merkleize.rs`.
   - Add unit tests in `storage/tests/qmdb_prepared_writes.rs`.
   - Update runner script (`write-type/run.sh`) and jq comparison (`compare-experiment.jq`) to support the 2-configuration pairwise comparison.
3. **Synchronization & Verification:**
   - Run the small-scale smoke benchmarks and unit tests across both measured engines.
