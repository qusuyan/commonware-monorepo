//! CPU-focused component benchmarks for QMDB authentication.
//!
//! Inputs are constructed before timing. The components deliberately avoid database/index I/O so
//! their results can be compared with the end-to-end `qmdb::merkleize` benchmarks.

use commonware_cryptography::{Hasher as _, Sha256, sha256};
use commonware_storage::{
    merkle::{self, Family, Location, mem::Mem},
    qmdb,
};
use commonware_utils::bitmap::Prunable as BitMap;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

const UPDATES: usize = 1_024;
const CHUNK_BYTES: usize = 32;
const BITMAP_BITS: u64 = 32_768;

type Digest = sha256::Digest;
type StandardHasher<H> = merkle::hasher::Standard<H>;

fn payloads() -> Vec<[u8; 64]> {
    (0..UPDATES)
        .map(|i| {
            let mut payload = [0u8; 64];
            payload[..8].copy_from_slice(&(i as u64).to_be_bytes());
            payload[8..16].copy_from_slice(&(!(i as u64)).to_be_bytes());
            payload
        })
        .collect()
}

fn leaf_digests<F: Family>(payloads: &[[u8; 64]]) -> Vec<Digest> {
    let hasher = qmdb::hasher::<Sha256>();
    payloads
        .iter()
        .enumerate()
        .map(|(i, payload)| {
            merkle::hasher::Hasher::<F>::leaf_digest(
                &hasher,
                F::location_to_position(Location::new(i as u64)),
                payload,
            )
        })
        .collect()
}

fn bench_leaf_hashing<F: Family>(c: &mut Criterion, family: &str) {
    let inputs = payloads();
    let hasher = qmdb::hasher::<Sha256>();
    c.bench_function(
        &format!(
            "qmdb::component/component=leaf_hash family={family} keys=0 updates={UPDATES} locality=na chunk=0"
        ),
        |b| {
            b.iter(|| {
                for (i, payload) in inputs.iter().enumerate() {
                    black_box(merkle::hasher::Hasher::<F>::leaf_digest(
                        &hasher,
                        F::location_to_position(Location::new(i as u64)),
                        payload,
                    ));
                }
            });
        },
    );
}

fn bench_operation_tree<F: Family>(c: &mut Criterion, family: &str) {
    let leaves = leaf_digests::<F>(&payloads());
    let hasher = qmdb::hasher::<Sha256>();
    c.bench_function(
        &format!(
            "qmdb::component/component=operation_tree family={family} keys=0 updates={UPDATES} locality=na chunk=0"
        ),
        |b| {
            b.iter_batched(
                Mem::<F, Digest>::new,
                |mem| {
                    let mut batch = mem.new_batch();
                    for leaf in &leaves {
                        batch = batch.add(&hasher, leaf);
                    }
                    black_box(batch.merkleize(&mem, &hasher));
                },
                BatchSize::SmallInput,
            );
        },
    );
}

fn bitmap() -> BitMap<CHUNK_BYTES> {
    let mut bitmap = BitMap::default();
    bitmap.extend_to(BITMAP_BITS);
    bitmap
}

fn bench_bitmap_mutation(c: &mut Criterion) {
    let base = bitmap();
    let disjoint: Vec<u64> = (0..UPDATES).map(|i| (i * 17) as u64).collect();
    let repeated: Vec<u64> = (0..UPDATES).map(|i| (i % 32) as u64).collect();
    for (locality, locations) in [("disjoint", disjoint), ("repeated", repeated)] {
        c.bench_function(
            &format!(
                "qmdb::component/component=bitmap_mutation family=na keys=0 updates={UPDATES} locality={locality} chunk={CHUNK_BYTES}"
            ),
            |b| {
                b.iter_batched(
                    || base.clone(),
                    |mut bitmap| {
                        for location in &locations {
                            bitmap.set_bit(*location, true);
                            bitmap.set_bit(*location, false);
                        }
                        black_box(bitmap);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
}

fn graft_inputs() -> Vec<([u8; CHUNK_BYTES], Digest)> {
    (0..UPDATES)
        .map(|i| {
            let mut chunk = [0u8; CHUNK_BYTES];
            chunk[i % CHUNK_BYTES] = (i as u8).wrapping_add(1);
            (chunk, Sha256::hash(&[&(i as u64).to_be_bytes()]))
        })
        .collect()
}

fn bench_grafted_leaf_hashing(c: &mut Criterion) {
    let inputs = graft_inputs();
    let hasher: StandardHasher<Sha256> = qmdb::hasher();
    c.bench_function(
        &format!(
            "qmdb::component/component=grafted_leaf family=mmr keys=0 updates={UPDATES} locality=disjoint chunk={CHUNK_BYTES}"
        ),
        |b| {
            b.iter(|| {
                for (chunk, ops_root) in &inputs {
                    black_box(merkle::hasher::Hasher::<commonware_storage::merkle::mmr::Family>::hash(
                        &hasher,
                        &[chunk, ops_root.as_ref()],
                    ));
                }
            });
        },
    );
}

fn bench_zero_chunk_identity(c: &mut Criterion) {
    let ops_roots: Vec<_> = (0..UPDATES)
        .map(|i| Sha256::hash(&[&(i as u64).to_be_bytes()]))
        .collect();
    c.bench_function(
        &format!(
            "qmdb::component/component=grafted_leaf_identity family=mmr keys=0 updates={UPDATES} locality=zero chunk={CHUNK_BYTES}"
        ),
        |b| b.iter(|| black_box(ops_roots.clone())),
    );
}

fn bench_upper_grafted_tree<F: Family>(c: &mut Criterion, family: &str) {
    let hasher = qmdb::hasher::<Sha256>();
    let leaves: Vec<_> = graft_inputs()
        .into_iter()
        .map(|(chunk, ops_root)| {
            merkle::hasher::Hasher::<F>::hash(&hasher, &[chunk.as_slice(), ops_root.as_ref()])
        })
        .collect();
    c.bench_function(
        &format!(
            "qmdb::component/component=upper_grafted_tree family={family} keys=0 updates={UPDATES} locality=disjoint chunk={CHUNK_BYTES}"
        ),
        |b| {
            b.iter_batched(
                Mem::<F, Digest>::new,
                |mem| {
                    let mut batch = mem.new_batch();
                    for leaf in &leaves {
                        batch = batch.add(&hasher, leaf);
                    }
                    black_box(batch.merkleize(&mem, &hasher));
                },
                BatchSize::SmallInput,
            );
        },
    );
}

fn bench_components(c: &mut Criterion) {
    bench_leaf_hashing::<commonware_storage::merkle::mmr::Family>(c, "mmr");
    bench_leaf_hashing::<commonware_storage::merkle::mmb::Family>(c, "mmb");
    bench_operation_tree::<commonware_storage::merkle::mmr::Family>(c, "mmr");
    bench_operation_tree::<commonware_storage::merkle::mmb::Family>(c, "mmb");
    bench_bitmap_mutation(c);
    bench_grafted_leaf_hashing(c);
    bench_zero_chunk_identity(c);
    bench_upper_grafted_tree::<commonware_storage::merkle::mmr::Family>(c, "mmr");
    bench_upper_grafted_tree::<commonware_storage::merkle::mmb::Family>(c, "mmb");
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_components
}
criterion_main!(benches);
