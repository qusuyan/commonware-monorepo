# Proposal 07: Standardizing on B-Tree Indexing as an Experimental Control

- **Proposal ID:** IMP-07 (formerly PROP-01)
- **Status:** Implemented
- **Author:** Rain Sun
- **Target Files:**
  - [`storage/src/qmdb/current/unordered/fixed.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/fixed.rs)
  - [`storage/src/qmdb/current/unordered/test_trait_impls.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/test_trait_impls.rs)
  - [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs)
  - [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs)

---

## 1. Problem Statement: Indexer as a Confounding Variable

Our research evaluates the **cryptographic tax** of exclusion proofs by comparing `current::ordered` against `current::unordered`.

In upstream benchmarks, this comparison conflated two independent factors:
1. **The Cryptographic Exclusion Protocol:** Querying predecessor keys, double-entry logging ($E_K$ and $E'_p$), and maintaining successor pointers.
2. **The In-Memory Indexer Data Structure:** `current::unordered` used a `HashMap`, whereas `current::ordered` used a `BTreeMap`.

Because `HashMap` and `BTreeMap` have fundamentally different cache locality, branch predictability, and asymptotic behavior, the indexer acted as a **confounding variable**. To ensure scientific rigor, **our main purpose is control**: the in-memory indexer is held constant as a control variable.

---

## 2. Solution: Standardizing `current::unordered` on B-Tree (`BTreeDb`)

Commonware's index trait architecture makes `Ordered` a sub-trait of `Unordered`:

```rust
// storage/src/index/mod.rs
pub trait Ordered: Unordered + Send + Sync
```

Because `crate::index::ordered::Index` implements `UnorderedIndex`, `current::unordered` can be instantiated directly with the identical B-Tree index structure used by `current::ordered`.

### Crucial Architectural Invariant

- In `current::ordered`, the B-Tree is used alongside exclusion proof logic (predecessor lookups, `next_key` pointers, and 2-entry log appends per insert).
- In `current::unordered` (`BTreeDb`), the B-Tree is **purely an in-memory key-to-location store (`T::Key -> Location<F>`)**. It contains **zero `next_key` tracking, zero predecessor searches, and zero exclusion proof machinery**. It writes **exactly 1 log entry** per insert ($E_K$).

### Controlled Attribution

$$\text{Controlled Exclusion Proof Tax} = \text{Latency}(\text{Ordered}_{\text{BTree}}) - \text{Latency}(\text{Unordered}_{\text{BTree}})$$

By holding the indexer constant as a B-Tree across both configurations, the observed latency delta is **strictly attributable to the cryptographic exclusion proof protocol**.

---

## 3. Technical Implementation

### A. Storage Engine Core (`storage/src/qmdb/current/unordered/fixed.rs`)

We added `BTreeDb` (and its partitioned counterpart) using `crate::index::ordered::Index`:

```rust
/// An unordered variant of [crate::qmdb::current] specializing on fixed-size values
/// backed by an ordered (B-Tree) index.
///
/// Holds the indexer structure constant (B-Tree) as an experimental control for fair
/// comparison against [crate::qmdb::current::ordered::fixed::Db].
///
/// Stored entries do NOT contain `next_key` pointers or exclusion proof metadata;
/// the B-Tree serves strictly as an in-memory key-to-location mapping.
pub type BTreeDb<F, E, K, V, H, T, const N: usize, S> = super::db::Db<
    F,
    E,
    Journal<E, Operation<F, K, V>>,
    K,
    FixedEncoding<V>,
    crate::index::ordered::Index<T, Location<F>>,
    H,
    N,
    S,
>;

impl<
    F: Graftable,
    E: Context + Spawner,
    K: Array,
    V: FixedValue,
    H: Hasher,
    T: Translator,
    const N: usize,
    S: Strategy,
> BTreeDb<F, E, K, V, H, T, N, S>
{
    pub async fn init(context: E, config: Config<T, S>) -> Result<Self, Error<F>> {
        crate::qmdb::current::init(context, config).await
    }
}
```

### B. Test Traits (`storage/src/qmdb/current/unordered/test_trait_impls.rs`)

Implemented `DbAny` and `BitmapPrunedBits` for `fixed::BTreeDb` and `fixed::partitioned::BTreeDb` under `#[cfg(any(test, feature = "test-traits"))]`.

### C. Benchmark Harness & Criterion Registry

1. In [`storage/src/qmdb/benches/common.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/common.rs):
   - Defined `CurUFixBTreeDb<F>`.
   - Registered `CurrentUnorderedBTreeFixedMmr` and `CurrentUnorderedBTreeFixedMmb` in `define_fixed_variants!`.
2. In [`storage/src/qmdb/benches/merkleize.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/benches/merkleize.rs):
   - Defined type aliases `CurUFixBTree32` and `CurUFixBTree32Mmb`.
   - Registered variant selector: `CurrentUnorderedBTreeFixed32Mmb` (`"current::unordered_btree::fixed::mmb chunk=32"`).

---

## 4. Verification & Validation Results

1. **Deterministic Root Parity Test:**
   - In `storage/src/qmdb/current/unordered/fixed.rs::test`, `test_current_unordered_btree_parity()` executes identical mutation sequences on both `Db` (`HashMap`) and `BTreeDb` (`BTreeMap`).
   - Proved `assert_eq!(btree_db.root(), hash_db.root())` across 500 initial inserts, 200 overwrites, 100 deletes, and 200 new inserts, as well as 700 point get verifications.
2. **Lifecycle & Proof Test:**
   - `test_current_unordered_btree_lifecycle_and_proof()` verified proof generation, verification, commit, sync, and pruning on `BTreeDb`.
3. **Full Test Suite:**
   - All 12 unit tests in `qmdb::current::unordered::fixed` passed with 0 failures.
4. **Criterion Benchmark Smoke Run:**
   - Successfully executed on `current::unordered_btree::fixed::mmb chunk=32` with 10,000 keys (`time: [653.49 µs 673.38 µs 708.14 µs]`).
