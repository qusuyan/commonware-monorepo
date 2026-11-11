# QMDB Architecture & Benchmarks: Study Notes & Q&A

A quick-reference summary of concepts, architecture, and codebase mechanisms discussed.

---

## 1. Rust & Codebase Foundations

### Q: What do `u64` and `f64` mean, and what is the difference?
* **`u64` (Unsigned 64-bit integer):** Stores non-negative whole numbers ($0$ to $2^{64}-1$). Exact arithmetic. Used for counting things like `num_keys`.
* **`f64` (Floating-point 64-bit number):** Stores real numbers with decimals (e.g., `0.99`, `3.14`). Used for continuous math, skews, and probabilities (`exponent`, `delta`, `proposal_mass`).

---

### Q: What do `struct`, `pub struct`, and `impl` mean?
* **`struct`:** Groups related data fields together. Private by default (only visible in the file it is defined in).
* **`pub struct`:** Public struct. Can be imported and used by other files/modules.
* **`impl`:** Defines functions and methods for a struct.
  > *(In Java/Python terms: `struct` = class attributes, `impl` = class methods).*

---

### Q: What does `Option<StableZipf>` do in `WriteSampler`?
Rust has no `null`. Instead, `Option<T>` represents either presence (`Some(T)`) or absence (`None`):
* When skew = 0.0 (uniform random access), it stores `None` to bypass Zipf overhead.
* When skew > 0.0, it stores `Some(StableZipf)` to generate skewed Zipfian keys.

---

### Q: What is `StableZipf` and why does it exist?
* **Purpose:** An $O(1)$-time, $O(1)$-memory rejection sampler for generating Zipfian (power-law) random key access for benchmarks.
* **Why "Stable":** Standard Zipf formulas divide by $(1 - s)$. When the skew exponent $s \approx 1.0$, this causes division-by-zero or severe floating-point precision loss. `StableZipf` uses numerical identities (`exp_m1`, `ln_1p`) to remain numerically stable around $s = 1.0$.

---

### Q: Why do we have `common.rs`? How does it relate to other files?
* **`common.rs` = Shared Toolbox:** It contains common benchmark configurations, constants (`PAGE_SIZE`, `THREADS`), complex type aliases, and data generators (`WriteSampler`). It has no `main()` and cannot be run by itself.
* **Benchmark Files = Main Programs:** Files like `merkleize.rs` and `apply_batch.rs` are the executables (`cargo bench --bench <name>`). They import the shared toolbox from `common.rs`.
* **Shell Scripts (`run.sh`):** Orchestrate matrix sweeps (varying skews, batch sizes, key counts) across the benchmark executables.

---

## 2. Database Write Lifecycle & Disk Reality

### Q: The 4-Phase Write Timeline
In modern blockchain state engines (like QMDB), writes pass through four distinct phases:

```
       PHASE 1                  PHASE 2                  PHASE 3                  PHASE 4
User / Tx Execution      Cryptographic Proof       Database Application      Physical Persistence
┌────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐   ┌─────────────────────┐
│ batch.write(k, v)  │──►│  batch.merkleize()  │──►│ db.apply_batch(b)   │──►│ db.commit() / sync()│
└────────────────────┘   └─────────────────────┘   └─────────────────────┘   └─────────────────────┘
     In RAM                    In CPU/RAM                In OS Page Cache             On Flash SSD
(Buffer staging)         (Merkle root calc)        (Appended to files)       (NAND cells flushed)
```

---

### Q: Is `batch.write()` or `batch.merkleize()` written to disk?
**No. It is 100% in memory.**
* `batch.write()` appends to a Rust in-memory `Vec`.
* `batch.merkleize()` hashes those memory buffers and updates the Merkle tree in RAM.
* **Disk writes only start at Phase 3 (`db.apply_batch()`) and Phase 4 (`db.commit() / sync()`).**

---

### Q: Is this timeline real production design or just for benchmarks?
**It is the real, production architecture.**
In blockchains, blocks are **speculative**. A validator must compute the State Root (`merkleize()`) so nodes can reach consensus on the block header *before* committing data to disk. If blocks were written directly to disk during transaction execution, every orphaned/reorged block would corrupt the database files.

---

### Q: Does our primary benchmark (`merkleize.rs`) measure writing to disk?
* **No.** `merkleize.rs` intentionally measures only **Phase 2 (CPU cryptographic bottleneck)**. It isolates hashing and Merkle math from disk I/O variations.
* Disk writing is measured separately in `apply_batch.rs` (Phase 3) and full durable commit benchmarks (Phase 4).

---

## 3. Cryptography & Internal Structures

### Q: What is "Grafting"?
* **Problem:** In an append-only log, an old entry at position 10 might have been overwritten by position 1,000,000. A standard Merkle proof only proves the entry existed in history, not that it is still active today.
* **Solution (Grafting):**
  1. QMDB keeps an in-memory **Activity Bitmap** (1 bit per log entry: `1` = active, `0` = superseded/deleted).
  2. For every chunk of 32 operations, QMDB hashes the bitmap chunk with the operation subtree root to create a **Grafted Leaf**.
  3. A Merkle tree built over these grafted leaves creates the canonical State Root.
* **Result:** Allows clients to verify that an entry is currently active without rehashing the entire historical log.

---

### Q: In the paper's tree diagram, what is physical log vs. calculated hash?
* **Only the bottom circles (`ENTRIES / LEAVES`):** The actual physical log records (`key`, `value`, `next_key`) written to SSD blobs.
* **Everything above the bottom row:** Calculated cryptographic hashes (Twig roots, upper Merkle nodes, Shard root).
* If you strip away all Merkle proofs, only the bottom row of log entries remains.

---

### Q: Where are the hashes stored?
* **State Root & Peaks:** Kept in **RAM** (an MMR with 100M items has at most 20–30 peak hashes, taking < 1 KB of memory).
* **On Disk:** QMDB defaults to a compact layout where only the bottom log entries are saved on SSD. Intermediate hashes are re-derived on the fly as needed rather than storing tens of millions of hash digests on disk.

---

### Q: What is the content the hash is calculated on?
Hashing happens hierarchically from the bottom up:
1. **Leaf Hashes:** Calculated on **each individual `(Key, Value)` operation record**.
2. **Twig / Subtree Roots:** Calculated on **subsets/pairs of operations** (e.g., chunks of 32 operations).
3. **Grafted Leaves:** Calculated on `Hash(Bitmap Chunk || Op Subtree Root)`.
4. **State Root:** Calculated over the **entire batch + all prior history combined**.

---

## 4. Indexer vs. Bitmap: Core Data Structures

### Q: What is the difference between the Indexer and the Bitmap?
They answer completely opposite questions:

| Feature | **The Indexer (`snapshot`)** | **The Bitmap (`bitmap`)** |
| :--- | :--- | :--- |
| **Question it answers** | *"Where is Alice's latest balance on disk?"* | *"Is position 500 on disk still alive, or is it dead?"* |
| **Direction of Mapping** | **Key $\longrightarrow$ Log Location** | **Log Location $\longrightarrow$ 1 bit (`1` or `0`)** |
| **Data Structure** | In-memory Map (`HashMap` or `BTreeMap`) | In-memory packed array of bits (`01101001...`) |
| **Who uses it?** | **Read queries & Updates:** To fetch values or replace pointers. | **Merkle Tree & Pruner:** To prove current status and reclaim disk space. |

* **Example:**
  * Log position 0: Alice deposits \$100.
  * Log position 1: Alice updates balance to \$120.
  * **Indexer holds:** `"Alice" -> Position 1` (points only to the latest tip).
  * **Bitmap holds:** `Position 0 = 0 (dead)`, `Position 1 = 1 (alive)`.

---

### Q: What does "Snapshot Lookup" mean? Is it loading from disk?
* **No.** It does NOT load from disk. The database is already resident in RAM.
* In QMDB, `snapshot` is simply the variable name for the **In-Memory Indexer**.
* A **"snapshot lookup"** is querying that in-memory map (`snapshot.get(K)`) to see if a key already exists and finding its latest log offset.

---

### Q: Why does `any + unordered_btree` still do an index lookup if it has no inclusion/exclusion proofs?
* **Reason 1: Basic Key-Value Logic (Insert vs. Overwrite):**
  The database must know whether to insert a new key or update an existing key. The indexer must update its pointer to the newer position on disk (`Key -> Offset 10,000`), otherwise future reads would return stale data.
* **Reason 2: Disk Garbage Collection (Log Compaction):**
  QMDB is an append-only log. When an existing key is updated, the old record is dead. `any` queries the index to discover the old offset (`base_old_loc`) so the background compaction pruner knows it can safely delete that disk sector later.
* **Key Point:** In `any`, this index lookup is **local housekeeping**, NOT cryptographic state proof generation.

---

### Q: How does the system know if a write is an Insert or an Overwrite?
It queries the in-memory **Indexer** (`snapshot`):
* **Key found:** Overwrite. Returns the old log location (`old_loc`).
* **Key not found:** Insert (new key).

---

### Q: Why does speed differ between Inserts and Overwrites?

The performance completely flips depending on the database variant:

| Variant | Faster Operation | Slower Operation | Why? |
| :--- | :--- | :--- | :--- |
| **`current::unordered`** | **Inserts** | **Overwrites (~2.1x slower)** | Overwrites flip bits from `1` to `0` in historical bitmap chunks, forcing **Merkle tree re-grafting** of dirty chunks. |
| **`current::ordered`** | **Overwrites** | **Inserts (~31x slower!)** | Inserts must maintain an ordered chain for **exclusion proofs**. Every insert requires a predecessor search in the B-Tree, **writing 2 entries instead of 1**, and shifting sorted arrays. |

---

## 5. Storage Engine Internals & Disk Layout

### Q: What is a "Blob" in Commonware?
* A **Blob** is a single sequential segment file on disk (named `0.blob`, `1.blob`, `2.blob`...).
* Instead of storing all logs in one infinite file, the append-only log splits into fixed-capacity blob files.
* An "item" inside a blob is a single serialized operation record `(Key, Value, next_key)`.

---

### Q: What is `ITEMS_PER_BLOB` and why is it set to 10,000,000 in `apply_batch.rs`?
* By default in `common.rs`, `ITEMS_PER_BLOB = 50,000`.
* Whenever a blob fills up, the engine must seal the file, issue an `fsync`, and open a new file descriptor.
* Because `fsync` causes sudden multi-millisecond stalls that would distort benchmark statistics, `apply_batch.rs` **overrides** the limit to 10,000,000 using `any_fix_cfg_with(ctx, ITEMS_PER_BLOB, ...)`. This ensures the test stays within a single file.

---

### Q: What is `fsync`?
* `fsync` (File Synchronization) is a POSIX system call that forces dirty pages in the Operating System page cache (RAM) to be flushed and physically written onto the non-volatile flash memory chips (NAND) of the SSD.
* Standard `write()` takes nanoseconds (RAM buffer); `fsync` takes milliseconds (physical hardware barrier).

---

### Q: What happens if a batch size exceeds `ITEMS_PER_BLOB`?
The system uses `batch_count_to_blob_boundary()`:
1. It writes up to the capacity of the current blob file.
2. It seals that blob and issues an `fsync`.
3. It creates and opens the next file (`1.blob`) and continues writing the remainder of the batch seamlessly.

---

## 6. Benchmark Scaffolding & Detailed Timer Logic

### Q: What is a "Batch" and what does "stage-write" do?
* **A Batch:** A grouping of multiple writes (analogous to a Block in a blockchain). Instead of writing and merkleizing key-by-key, writes are bundled together.
* **Stage-write (`stage_prepared_writes`):** Takes a pre-generated slice of `(Key, Value)` pairs sitting in RAM and registers them into the QMDB batch handle via `batch.write(k, v)`.

---

### Q: Did we get rid of batch staging and sampling from the timer?
* **Input Sampling & Key Generation (EXCLUDED):**
  Drawing random numbers (`rng.next_u64()`), Zipf sampling, hashing fake keys, and vector allocations are performed in `before_iter` **outside the timer**.
* **Database Ingestion (`stage_prepared_writes`) (TIMED):**
  Looping through the prepared keys and calling `batch = batch.write(k, v)` is **inside the timer** because it is the actual database write API.

---

### Q: How does the timer logic work in `merkleize.rs` vs. `apply_batch.rs` vs. `components.rs`?

| Benchmark | Timer Mechanism | Scope of Measurement | Typical Latency |
| :--- | :--- | :--- | :--- |
| **`components.rs`** | Criterion `b.iter(\|\| ...)` | **Micro-routine:** Pure algorithm in L1 cache (single loop of leaf hashing, bit flipping, or grafted leaf hashing). | **~100 nanoseconds** |
| **`merkleize.rs`** | Criterion `b.iter(\|\| iter())` | **Phase 2 (Crypto in RAM):** Batch staging + snapshot index lookup + leaf hashing + MMR tree calculation + bitmap grafting. Discards batch after timing. | **~1 to 50 milliseconds** |
| **`apply_batch.rs`** | `let start = Instant::now(); db.apply_batch(b); start.elapsed()` | **Phase 3 (Storage Installation):** Disk log append (`.blob`) + indexer pointer update (`snapshot`) + bitmap inactivation + ancestor diff merging. | **~2 to 20 milliseconds** |

---

### Q: Inside `apply_batch.rs`, are the internal steps timed separately or summed?
**It times the aggregate SUM of all 4 steps together as one single block:**
```rust
let start = Instant::now();
let (db, _) = db.apply_batch(batch).await.unwrap(); // <── Single stopwatch around all 4 steps!
let elapsed = start.elapsed();
```
It does not report individual sub-breakdowns (e.g., disk time vs. index time).

---

### Q: Was `components.rs` part of original Commonware or did we add it? Does it cover `apply_batch`?
* **We added it:** Created on branch `rain-dev-latest` under Proposal 05 (`IMP-05`) for fine-grained CPU attribution.
* **Does it cover `apply_batch`:** **No.** It strictly isolates the subroutines of `merkleize` (leaf hashing, tree building, bitmap flipping). It has no disk I/O, no indexer lookups, and no ancestor reconciliation.

---

### Q: What is "End-to-End Latency" and why does the mentor care?
* **End-to-End Latency:** Measures the total elapsed time across the entire pipeline:
  $$\text{End-to-End} = \text{Phase 1 (Staging)} + \text{Phase 2 (Merkleize)} + \text{Phase 3 (Apply to Log)} + \text{Phase 4 (fsync)}$$
* **The Question:** Exclusion proofs make Phase 2 **~31x slower** in RAM. Does that 31x cryptographic tax dominate real-world block commit times, or does slow NVMe SSD `fsync` I/O mask the difference?

---

## 7. Our Research Goal: The Cryptographic Tax

To understand the price of every guarantee QMDB provides:

```
┌────────────────────────────────────────────────────────────────────────┐
│ current::ordered       (Log Integrity + Current Inclusion + Exclusion) │  ▲
├────────────────────────────────────────────────────────────────────────┤  │ Exclusion Proof Tax (~31x insert slowdown)
│ current::unordered     (Log Integrity + Current State via Grafting)    │  ▼
├────────────────────────────────────────────────────────────────────────┤  ▲
│ any::unordered (btree) (MANDATORY BASELINE: Log Integrity, Zero Auth)  │  │ Bitmap Grafting Tax (~2.1x overwrite slowdown)
├────────────────────────────────────────────────────────────────────────┤  ▼
│ raw store / contiguous [Reference Only: Unauthenticated, breaks integrity]
└────────────────────────────────────────────────────────────────────────┘
```

---

### Q: Comparing `current + ordered` vs. `any + unordered_btree` Internal Stages

Both variants are standardized on the **exact same B-Tree indexer** (our scientific control). Here is how their internal execution stages differ:

#### 1. Inside `merkleize.rs` (Phase 2: Cryptographic Proof)

| Stage | `any + unordered_btree` (Baseline) | `current + ordered` (Full Auth) |
| :--- | :--- | :--- |
| **1. Index Lookup** | Queries B-Tree: does key exist? ($O(\log N)$) | Searches predecessor/successor span: $\text{pred} < K < \text{succ}$ ($O(\log N)$) |
| **2. Op Construction** | Encodes $N$ operations | If insert: **encodes $2N$ operations** (new key + updated predecessor) |
| **3. MMR Tree Hashing**| Hashes $N$ ops into Merkle Mountain Range | Hashes $N$ (or $2N$) ops into Merkle Mountain Range |
| **4. Bitmap & Grafting**| ❌ **Skipped** (No activity bitmap in tree) | **Heavy:** Computes dirty chunk hashes and builds upper Grafted Merkle Tree |

#### 2. Inside `apply_batch.rs` (Phase 3: Storage Ingestion)

| Stage | `any + unordered_btree` (Baseline) | `current + ordered` (Full Auth) |
| :--- | :--- | :--- |
| **1. Disk Append** | Appends $N$ records to `.blob` | Appends $N$ records (overwrites) or **$2N$ records** (inserts) |
| **2. Indexer Update** | B-Tree `insert(key, loc)` ($O(\log N)$) | B-Tree `insert(key, loc)` + updates predecessor pointer ($O(\log N)$) |
| **3. Bitmap Update** | Extends bitmap length; sets old loc `0` | Extends bitmap length; sets old loc `0` |
| **4. Grafted Tree** | ❌ None | Advances in-memory `grafted_tree` handle |

---

### Q: Why is `any::unordered` our baseline instead of raw `store`?
* **Getting Rid of Authentication:** In our study, we want to isolate the cost of **state authentication** (providing light-client proofs of current key-value state and non-membership). `any::unordered` strips away all state authentication machinery—it has no activity bitmap grafting, no predecessor pointer chains, and does not serve current-state proofs.
* **Maintaining Database Integrity:** Crucially, `any::unordered` **maintains full cryptographic log integrity**. Every operation written to the log is committed into an MMR/MMB Merkle tree, ensuring tamper-evident history, fork prevention, and consensus commitment.
* **Why `store` Cannot Be the Baseline:** Raw `store` removes the Merkle tree entirely. While fast, it destroys tamper-evidence and auditability—no production blockchain or verifiable database could run it. Therefore, `store` is strictly an academic hardware limit reference, while `any::unordered` is our true production-viable baseline.

---

### Q: What is the exact difference between "Integrity" and "Authentication" in QMDB?
* **1. Log Integrity (Non-Negotiable Invariant):**
  * Guarantees that the sequential append-only log of transactions/operations is **tamper-evident and immutable**.
  * Handled by building an MMR/MMB Merkle tree over the sequential operation log (`any`).
  * If an attacker alters a historical byte in a blob on disk, the State Root changes and consensus rejects the state.
* **2. Key-Value State Authentication (The Measured Cryptographic Tax):**
  * Guarantees that an untrusted server can generate a verifiable proof to a light client answering:
    * *"What is the current value of key $K$?"* (Inclusion proof $\to$ requires **Bitmap Grafting** in `current::unordered`).
    * *"Does key $K$ exist?"* (Exclusion / Non-membership proof $\to$ requires **Ordered B-Tree Index + Predecessor Chains** in `current::ordered`).
  * Our research measures the exact latency, CPU, and disk penalties paid to provide these client-facing state proofs over a baseline that only guarantees log integrity.

---

## 8. Proof Verification, Code Implementation, & The Need for Bitmaps

### Q: Does `any` only merkleize the log while getting rid of the bitmap?
* **Yes.** `any` eliminates the activity bitmap completely from its Merkle tree calculations.
* **What `any` merkleizes:** Strictly the sequential append-only operations log.
* **What an `any` proof proves:** Membership in log history—specifically, that at some point in time, operation $(K, V)$ was appended to the log at position $L$. It **cannot** prove whether that key was subsequently overwritten or deleted.
* **What a `current` proof proves:** That $(K, V)$ was logged at location $L$ **AND** is still active right now. To achieve this under a single root, `current` grafts the bitmap chunk into the operations subtree root.

---

### Q: How are proofs named and called in the codebase? Where is the code?
* **`any` proofs:**
  * **Function:** [`any::Db::proof(loc, max_ops)`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/any/db.rs#L520).
  * **Proof Type:** Returns `(merkle::Proof<F, H::Digest>, Vec<Operation>)`.
  * **Verification:** Standard Merkle verification in [`verify.rs`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/verify.rs). Recomputes sibling hashes strictly over operation digests up to the root.
* **`current` proofs:**
  * **Function:** [`current::Db::key_value_proof(key)`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/unordered/db.rs#L91).
  * **Proof Type:** [`OperationProof`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/proof/mod.rs#L534), which bundles:
    1. `loc: Location`: Log position.
    2. `chunk: [u8; N]`: The exact bytes of the bitmap chunk covering `loc`.
    3. `range_proof: RangeProof`: The Merkle proof inside the grafted tree.
  * **Verification:** In [`OperationProof::verify`](file:///Users/rains/rainworkspace/UWMadison/cs698/commonware-monorepo/storage/src/qmdb/current/proof/mod.rs#L579):
    1. Checks the bitmap bit: `BitMap::get_bit_from_chunk(&chunk, *loc) == true`.
    2. Checks the grafted leaf hash: $\text{Hash}(\text{chunk} \parallel \text{operation\_subtree\_root})$.
    3. Verifies the Merkle path to the canonical State Root.

---

### Q: If key $K$ appears multiple times in the log, which location is returned?
* **In `any`:** Proofs are **location-based**, not key-based (`db.proof(loc, max_ops)`).
  * If key $K$ appears at locations 5, 17, and 42, the caller specifies the exact historical location they want. The database returns the Merkle path for that specific index.
* **In `current`:** Proofs are **key-based** (`db.key_value_proof(key)`).
  * It queries the in-memory indexer (`self.any.get_with_loc(&key)`), which always points to the single **latest/active** location (e.g., 42).
  * It returns an `OperationProof` for that location. If an attacker tries to pass location 5, verification fails because bit 5 was flipped to `0` in the authenticated bitmap chunk.

---

### Q: If the server already has an in-memory indexer (`snapshot`), why do we need a bitmap at all?
* **The Indexer is NOT Authenticated:** The index (`snapshot`) is just a local, unauthenticated in-memory hash table / B-tree in the server's RAM. Light clients do not possess the index and do not trust the server.
* **The Threat:** A dishonest or compromised server could pretend key $K$ has not changed by returning an old historical log position (e.g., location 5 where Alice had \$100), rather than location 42 (where Alice spent it). If the client only verified against the log Merkle tree, the proof for location 5 would appear valid!
* **Why the Bitmap is Required:** The bitmap is cryptographically grafted into the 32-byte State Root. The server cannot fake the chunk bytes without breaking the Merkle root. When the client checks bit 5 in that authenticated chunk, it sees `0` (stale/inactive) and immediately catches the lie.

---

### Q: How did previous storage systems work without a bitmap?
* **Key-Based Tries (MPT / JMT):** Older systems (like Ethereum's Merkle Patricia Trie or Diem/Aptos's Jellyfish Merkle Tree) did not use an append-only log. They authenticated a trie where the key itself defines the tree path from the root down to the leaf.
* **In-Place Mutation:** When a key updated, the trie navigated to that key's single leaf node and replaced it, rehashing the path up to the root. Because there was only ever one leaf node per key, any valid proof was automatically the current value—no bitmap needed.
* **The Problem:** In-place trie mutations require random disk reads and writes across millions of tiny branch nodes (severe read/write amplification). QMDB avoids this by using sequential append-only logging + compact bitmap grafting.

---

### Q: Does Merkleization happen when a proof is requested?
* **No.** Proof generation does **not** merkleize or hash the database. Proof generation is a fast read that fetches already-computed sibling hashes and bitmap chunks.
* **When Merkleization actually happens:** At **block/batch commit time** (`batch.merkleize()`).
  * Before transactions can be finalized or agreed upon by consensus, validators must calculate and publish the 32-byte State Root.
  * Once calculated, the Merkle nodes are persisted or cached. When a client later requests a proof, the server simply looks up and serves the pre-calculated digests.
