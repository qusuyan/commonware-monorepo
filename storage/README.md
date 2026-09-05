# commonware-storage

[![Crates.io](https://img.shields.io/crates/v/commonware-storage.svg)](https://crates.io/crates/commonware-storage)
[![Docs.rs](https://docs.rs/commonware-storage/badge.svg)](https://docs.rs/commonware-storage)

Persist and retrieve data from an abstract store.

This crate is a toolbox of composable storage primitives — append-only journals, in-memory
indexes, Merkle structures — that culminate in `qmdb`: a family of authenticated ("Merkleized")
databases inspired by the [QMDB paper](https://arxiv.org/abs/2501.05262) (the module was
originally named `adb`, for *authenticated database*, and was renamed in #2409). This README is
an index for reading the codebase: what each module does, how the modules depend on each other,
and how they are assembled into QMDB.

## Architecture at a glance

Every persistent structure is built on the `commonware-runtime` storage abstraction (blobs in
named partitions), so everything here is runtime-agnostic and testable under the deterministic
runtime. Modules are layered roughly like this (arrows point at dependencies):

```text
                                  ┌─────────────────────────────────┐
  Layer 4: authenticated DBs      │              qmdb               │
                                  │ any · current · immutable ·     │
                                  │ keyless · store · compact · sync│
                                  └───┬───────┬───────┬─────────┬───┘
                                      │       │       │         │
  Layer 3: authenticated          ┌───▼───┐ ┌─▼─────────────┐   │
  structures                      │bitmap │ │    merkle     │   │
                                  │       │ │ mmr · mmb ·   │   │
                                  │       │ │ persisted     │   │  (bmt: standalone
                                  └───┬───┘ └─┬─────────┬───┘   │   Merkle tree, used
                                      │       │         │       │   independently)
  Layer 2: derived stores  ┌──────────┼───────┼─────────┼───────┤
  (not used by qmdb)       │ archive  │ cache │ freezer │ queue │
                           └───┬──────┴──┬────┴────┬────┴──┬────┘
                               │         │         │       │
  Layer 1: persistence     ┌───▼─────────▼─────────▼───────▼────┐
                           │   journal (contiguous · segmented) │
                           │   metadata        ordinal          │
                           └───────────────┬────────────────────┘
                                           │
  Layer 0: in-memory utils ┌───────────────▼────────────────────┐
                           │  index · translator · rmap         │
                           └────────────────────────────────────┘
```

Two conventions appear everywhere and are worth internalizing before reading any module:

- **Ownership-based crash safety.** Mutating async methods take the structure *by value* and
  return it on success. If a call errors or its future is dropped mid-flight, the handle is gone:
  in-memory (non-durable) state is discarded, but everything already on disk remains recoverable
  by re-initializing. This turns "partial write" bugs into a compile-time-visible pattern.
- **Explicit durability.** Writes buffer in memory until `commit()`/`sync()`. Recovery replays
  or truncates uncommitted suffixes on startup.

## Module index

### Layer 0 — in-memory utilities

- **[`rmap`](src/rmap/mod.rs)** — `RMap`, a set of disjoint inclusive `u64` ranges with
  automatic merging of adjacent ranges. Used by `ordinal`, `cache`, `queue`, and
  `archive::prunable` to track which indices are present and answer `next_gap` queries (which
  drives "what do I still need to fetch" logic during sync).
- **[`translator`](src/translator.rs)** — the `Translator` trait: compress an arbitrary key into
  a short fixed-size representation (`OneCap`/`TwoCap`/`FourCap`/`EightCap` take the first N
  bytes; `Hashed` adds seeded hashing for flood resistance). Translated keys make in-memory
  indexes cheap (a few bytes per key) at the cost of collisions, which callers resolve by
  checking the authoritative on-disk record.
- **[`index`](src/index/mod.rs)** — memory-efficient maps from *translated* keys to values
  (typically log locations). `unordered::Index` is hashmap-backed; `ordered::Index` adds
  next/previous navigation over the translated key space. Because of translation, `get` returns
  *all* values whose keys collide. [`index/partitioned`](src/index/partitioned/mod.rs) shards
  the key space by a 1–3 byte prefix into independent partitions (sorted struct-of-arrays with
  prefix elision, or hash sub-indexes) — this both cuts per-key memory and enables the parallel
  snapshot rebuild used by `qmdb`.

### Layer 1 — persistence primitives

- **[`journal`](src/journal/mod.rs)** — the append-only log at the base of nearly everything.
  Two families:
  - [`contiguous`](src/journal/contiguous/mod.rs): items addressed by a global 0-indexed
    *position* that is stable across pruning. `fixed` (fixed-size items, O(1) offset math) and
    `variable` (length-prefixed items plus a side journal of offsets). Data is spread across
    blobs of `items_per_blob` items; pruning drops whole blobs from the front. A careful
    seal/fsync rollover pipeline bounds how much of the log can be non-durable and lets
    recovery validate only the last two blobs. `qmdb` stores its operation log here, and
    `journal::authenticated` (see `merkle` below) pairs a contiguous journal with a Merkle
    structure.
  - [`segmented`](src/journal/segmented/mod.rs): items addressed by `(section, offset)`, one
    blob per section. `fixed`, `variable`, `glob` (raw values + CRC32, sizes tracked
    externally), and `oversized` (a fixed index journal + a glob value store with coordinated
    crash recovery). This family underlies `cache`, `freezer`, and `archive`.
- **[`metadata`](src/metadata/mod.rs)** — an atomically-committed store for *small* key-value
  collections (checkpoints, pinned digests, config). Double-buffers between two blobs with a
  version counter and CRC32 trailer so the latest complete state always survives a crash;
  writes deltas when the key set is stable. The "commit point" companion to larger stores.
- **[`ordinal`](src/ordinal/mod.rs)** — a persistent map from *sparse* `u64` indices to
  fixed-size values, where the file offset is computed directly from the index (`N *
  RECORD_SIZE`, each record CRC-protected). Unlike a journal it supports out-of-order
  insertion; an `RMap` tracks which indices exist.

### Layer 2 — derived stores

These compose Layer 0/1 into ready-made stores. **They are not used by `qmdb`** — they serve
other parts of the Commonware stack (e.g. syncing and archiving blocks in consensus) — but they
are good, smaller case studies of the crate's composition style.

- **[`cache`](src/cache/mod.rs)** — prunable cache of ordered data with index-based lookups:
  a `segmented::variable` journal plus an in-memory index→(offset,length) map (~16 B/item), so
  every read is a single blob op. Supports `next_gap` for sync.
- **[`queue`](src/queue/mod.rs)** — durable at-least-once queue over a `contiguous::variable`
  journal; explicit `ack` advances the prune floor, and the `shared` submodule splits it into a
  clonable multi-producer `Writer` and a `Reader`.
- **[`freezer`](src/freezer/mod.rs)** — immutable key-value store for permanent, write-once
  data with minimal memory: an on-disk extendible hash table (epoch-stamped dual slots for
  crash consistency) pointing into a `segmented::oversized` key/value journal. Never compacts.
- **[`archive`](src/archive/mod.rs)** — write-once store addressed by *both* an index and a
  key. [`immutable`](src/archive/immutable/mod.rs) = `freezer` (key lookups) + `ordinal`
  (index lookups) + `metadata` (the commit checkpoint). [`prunable`](src/archive/prunable/mod.rs)
  = `segmented::oversized` journals + an in-memory `index` keyed by translated keys + `rmap`,
  for history that gets discarded (e.g. old blocks).

### Layer 3 — authenticated structures

- **[`merkle`](src/merkle/mod.rs)** — the shared framework for append-friendly Merkle
  structures. A `Family` trait abstracts the tree shape behind generic `Position` (node
  coordinates), `Location` (leaf coordinates), proof generation, and verification code written
  once. Two families implement it:
  - **[`mmr`](src/merkle/mmr/mod.rs)** — Merkle Mountain Range: a forest of perfect binary
    trees of strictly decreasing height whose peaks are "bagged" into a root. Append-only, so
    updates never touch historical nodes — ideal for logs.
  - **[`mmb`](src/merkle/mmb/mod.rs)** — Merkle Mountain Belt (arXiv:2511.13582): like an MMR
    but with delayed merging, so each append creates at most one internal node and the
    structure is never larger than the equivalent MMR.

  Both families share [`persisted`](src/merkle/persisted/full.rs) backends: `full` stores all
  unpruned nodes in a `contiguous::fixed` journal with pruned-but-needed ("pinned") digests in
  `metadata`, and exposes `proof`/`range_proof`/`historical_proof`; `compact` keeps only the
  leaf count and pinned frontier in memory — enough to keep appending and computing roots, with
  no history. Proofs support single elements, contiguous ranges, arbitrary multi-element sets,
  and *historical* variants (proving against a past size — essential for state sync). The
  `Graftable` extension of `Family` provides the coordinate math used to overlay a bitmap tree
  onto an ops tree (see `qmdb::current` below).
- **[`bitmap`](src/bitmap/authenticated.rs)** — an authenticated bitmap: bits grouped into
  N-byte chunks, with a Merkle tree over chunks that deliberately *mimics MMR structure* so it
  can be aligned with an MMR built over the same elements. A type-state API
  (`UnmerkleizedBitMap` → `merkleize()` → `MerkleizedBitMap`) makes "you can only prove after
  merkleizing" a compile-time rule. A standalone primitive; `qmdb::current` applies the same
  chunk/graft design with its own machinery.
- **[`bmt`](src/bmt/mod.rs)** — a plain balanced Binary Merkle Tree (position-hashed leaves,
  root bound to leaf count) with single/range/multi proofs. Standalone — not part of the
  `Family` framework and not used by `qmdb`; suited to one-shot trees like a block's
  transaction root.

### Layer 4 — [`qmdb`](src/qmdb/mod.rs): the authenticated databases

Covered in depth in the next section. Variants: [`any`](src/qmdb/any/mod.rs),
[`current`](src/qmdb/current/mod.rs), [`immutable`](src/qmdb/immutable/mod.rs),
[`keyless`](src/qmdb/keyless/mod.rs), [`store`](src/qmdb/store/mod.rs), plus `compact`
counterparts and the [`sync`](src/qmdb/sync/mod.rs) engine.

## How the pieces build QMDB

### The idea, from the paper

The [QMDB paper](https://arxiv.org/abs/2501.05262) (LayerZero Labs; Commonware is acknowledged
in it) targets the blockchain-state bottleneck: keeping key-value state and a Merkle Patricia
Trie in separate stores makes every state update cost `O((log N)^2)` random SSD I/O. QMDB's fix
is to **unify world state and Merkle storage in one append-only structure**: every state change
appends an immutable *entry*; entries are grouped into fixed-size subtrees ("*twigs*") whose
roots plus an *ActiveBits* bitmap (which entries are still current) are small enough to keep in
DRAM. Result: one SSD read per state access, O(1) amortized I/O per update, Merkleization
entirely in memory, plus historical proofs and twig-granularity state sync. A compaction worker
re-appends still-active old entries so exhausted twigs can be pruned.

### Commonware's translation

This crate implements those ideas on its own primitives — an operation log authenticated by an
MMR/MMB rather than the paper's literal twig layout:

| QMDB paper concept | This crate |
|---|---|
| Entry (immutable state-change record) | An `Operation` (`Update`/`Delete`/`Set`/`Append`/`Commit…`) at a `Location` in an append-only log |
| Twigs + upper nodes over entries | MMR/MMB (`merkle::Family`) over the operation log |
| Entry storage on SSD, tree in DRAM | `journal::contiguous` holds operations; `journal::authenticated` pairs it with a persisted Merkle structure (nodes journaled, pinned digests in `metadata`) |
| ActiveBits bitmap per twig | The activity bitmap: one bit per operation, 1 = most recent update of an active key |
| Twig root ⊕ ActiveBits authentication | *Grafting* (`qmdb::current`): `hash(bitmap chunk ‖ ops-subtree root)` merges both trees into one proof path |
| In-memory indexer (key → SSD offset) | `index` (+ `translator`) mapping translated keys → log `Location`; collisions resolved by reading the log |
| Compaction worker / active-entry ratio | The *inactivity floor*: commits re-append still-active operations from the floor and advance it; everything below the floor is prunable |
| Historical inclusion via OldId/Version graph | Historical range proofs against any commit-boundary log size |
| Twig-granularity peer sync | `qmdb::sync`: fetch operation ranges + range proofs, verify against a target root, replay |

### Anatomy of a database

Every variant is the same core sandwich, from [qmdb/mod.rs](src/qmdb/mod.rs):

1. **An authenticated operation log** — `journal::authenticated::Journal`, i.e. a
   `contiguous` journal of encoded `Operation`s zipped with a persisted MMR/MMB whose i-th leaf
   is the digest of the i-th operation. Appending an op and appending its leaf are one motion;
   the log's root authenticates the entire history.
2. **A snapshot index** — an in-memory `index` (optionally `partitioned` for parallel rebuild
   at startup) mapping each translated key to the `Location` of its latest update. This is the
   read path: index → location → one journal read. Rebuilt on startup by replaying the log from
   the inactivity floor (`build_snapshot_from_log`, with a sharded parallel path for
   partitioned indexes).
3. **An activity bitmap** — bit *i* says whether operation *i* is active. Kept alongside the
   log by `any`/`current`; used for floor management and (in `current`) merkleized via
   grafting.
4. **The inactivity floor** — a log location, carried inside every `Commit` operation (and thus
   folded into the root, so all replicas agree on it). Each commit "raises the floor" some
   number of steps: the first active operation above the floor is re-appended at the tip and
   the floor moves past it. This is the paper's compaction: it bounds inactive operations to a
   constant factor of active ones and makes everything below the floor prunable — from the
   journal, the Merkle structure (which pins the few digests still needed for proofs), and the
   bitmap.

Mutation follows a uniform **batch lifecycle**: create a batch → stage mutations → `merkleize`
(resolve against current state, compute the would-be root) → optionally build child batches on
top (a *batch chain*, validated best-effort by [batch_chain.rs](src/qmdb/batch_chain.rs)) →
`apply` → `commit()`/`sync()` for durability. This lets a blockchain compute and check a state
root before deciding to persist a block, and pipeline speculative branches.

### The variants

| Variant | Can prove | Notes |
|---|---|---|
| `any` | Key *K* had value *V* at some point in history | The minimal construction: log + MMR/MMB + index + floor. Cannot prove currency or absence. |
| `current` | …and that *V* is (or is not) *K*'s *current* value | Wraps `any`, adds the merkleized activity bitmap via grafting. |
| `immutable` | Set-once key *K* has value *V* | No updates/deletes; a key may be set at most once, so "some value" = "the value". |
| `keyless` | Element at location *L* is *V* | Pure authenticated append-only log; no keys, no index. |
| `store` | (nothing — unauthenticated) | Same batch/commit machinery and index, plain journal, no Merkle tree. |
| `compact` (of `immutable`/`keyless`) | Same roots as its full counterpart | Keeps only Merkle peaks + a witness journal; no history, no `get`/`proof`. |

Keyed variants come in **unordered** and **ordered** flavors: ordered variants additionally
track each active key's lexicographic successor among active keys, which is what enables
**exclusion proofs** ("no key exists between A and B, so K is absent") — the paper's
`NextKey` field — at extra bookkeeping cost. Fixed- and variable-size value encodings are
supported throughout.

**Grafting** ([current/grafting.rs](src/qmdb/current/grafting.rs), and the
"[Grafting Trees to Prove Current State](https://commonware.xyz/blogs/adb-current.html)" blog)
is the crate's twist on the paper's twig authentication. The bitmap's chunk tree has exactly
the shape of the ops tree with its bottom `G = log2(chunk bits)` levels removed, so instead of
maintaining two trees with two proof paths, each complete bitmap chunk is hashed together with
its corresponding height-`G` ops-subtree root to form a "grafted leaf". One branch through the
grafted tree then authenticates an operation *and* its activity bit; with 256-bit chunks a
current-value proof costs about one digest more than an any-value proof. Chunks that are not
yet graftable (the trailing partial chunk, and — under MMB's delayed merges — at most one
"pending" complete chunk whose ops ancestor hasn't been born) are hashed directly into the
canonical root until they are. The grafted tree and unpruned bitmap live entirely in RAM
(pruned-chunk state is persisted via `metadata`), matching the paper's in-memory
Merkleization.

### State sync

[`qmdb/sync`](src/qmdb/sync/mod.rs) rebuilds a database at a target root without replaying the
full history — the reason the whole design centers on an authenticated *log*. An `Engine` is
given a `Target { root, range }` (the range typically starting at the inactivity floor — the
paper's "state at twig granularity", here "active operations only"), a `Resolver` (any full
database implements it, over the network in practice) and fetches operation batches with
**historical range proofs** verifying each batch against the target root. Batches can arrive
out of order (gap tracking), targets can move while syncing, and pinned Merkle nodes for the
pruned prefix are fetched or reused from local storage. Once the range is complete the engine
hands back a fully-initialized database whose root matches the target. `current` syncs against
the plain *ops* root and then deterministically reconstructs bitmap + grafted tree locally;
`compact` variants support serving and syncing witness-only state. See
[examples/sync](../examples/sync/README.md) for a working client/server.

## Suggested reading order

1. **Skim the utilities**: [rmap](src/rmap/mod.rs), [translator](src/translator.rs),
   [index](src/index/mod.rs) — small, self-contained, used everywhere.
2. **journal**, starting with [contiguous](src/journal/contiguous/mod.rs) (the qmdb substrate);
   [segmented](src/journal/segmented/mod.rs) if you care about the Layer-2 stores.
3. **merkle**: the [MMR blog](https://commonware.xyz/blogs/mmr.html), then
   [merkle/mod.rs](src/merkle/mod.rs) (`Family`), [mmr](src/merkle/mmr/mod.rs),
   [persisted/full.rs](src/merkle/persisted/full.rs), and proofs
   ([proof.rs](src/merkle/proof.rs), [verification.rs](src/merkle/verification.rs)). Treat
   [mmb](src/merkle/mmb/mod.rs) as an optimization detail on first pass.
4. **qmdb core**: [qmdb/mod.rs](src/qmdb/mod.rs) module docs (terminology, lifecycle,
   ownership), [operation.rs](src/qmdb/operation.rs), then
   [any](src/qmdb/any/mod.rs) alongside the
   ["Simplest Database You Need" blog](https://commonware.xyz/blogs/adb-any.html).
5. **qmdb::current**: [bitmap](src/bitmap/authenticated.rs) for the chunked-bitmap idea, then
   [current](src/qmdb/current/mod.rs) and [grafting.rs](src/qmdb/current/grafting.rs) with the
   [grafting blog](https://commonware.xyz/blogs/adb-current.html).
6. **The rest of qmdb**: [immutable](src/qmdb/immutable/mod.rs),
   [keyless](src/qmdb/keyless/mod.rs), [store](src/qmdb/store/mod.rs), the `compact` variants,
   and [sync](src/qmdb/sync/mod.rs) with [examples/sync](../examples/sync/).
7. **Independent extras** as needed: [bmt](src/bmt/mod.rs), [cache](src/cache/mod.rs),
   [queue](src/queue/mod.rs), [freezer](src/freezer/mod.rs), [archive](src/archive/mod.rs).

Also see the [QMDB paper](https://arxiv.org/abs/2501.05262) itself and
[TESTING.md](TESTING.md) for how the deterministic-runtime failure tests work.

## Status

Stability varies by primitive. See [README](https://github.com/commonwarexyz/monorepo#stability) for details.
