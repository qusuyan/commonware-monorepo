#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::{
    Digest, WriteSampler, WriteType, prepare_random_writes, prepare_sampled_writes,
    stage_prepared_writes, write_random_updates, write_random_writes, write_sampled_writes,
};
use commonware_cryptography::Hasher as _;
use commonware_runtime::{Runner as _, Supervisor as _, deterministic};
use commonware_storage::{
    merkle::{self, mmr::Family},
    qmdb::{
        Error,
        any::{
            traits::{DbAny, MerkleizedBatch, UnmerkleizedBatch},
            unordered::fixed::Db,
        },
    },
};
use commonware_utils::TestRng;
use rand::Rng as _;

#[derive(Default, Debug, PartialEq)]
struct Writes(Vec<(Digest, Option<Digest>)>);

struct Root;

#[test]
fn mixed_writes_preserve_legacy_draw_order() {
    for exponent in [0.0, 0.75, 3.0] {
        let sampler = WriteSampler::new(100, exponent).unwrap();
        let mut expected_rng = TestRng::new(99);
        let mut direct_rng = TestRng::new(99);
        let mut prepared_rng = TestRng::new(99);
        let mut prepared = Vec::new();
        for count in [32, 0, 16] {
            let expected: Vec<_> = (0..count)
                .map(|_| {
                    let sampled = sampler.sample(&mut expected_rng);
                    let insert = expected_rng.next_u32().is_multiple_of(2);
                    let idx = sampled + if insert { 100 } else { 0 };
                    (
                        commonware_cryptography::Sha256::hash(&[&idx.to_be_bytes()]),
                        Some(common::make_fixed_value(&mut expected_rng)),
                    )
                })
                .collect();
            let direct = if exponent == 0.0 {
                write_random_writes(
                    Writes::default(),
                    count,
                    100,
                    &mut direct_rng,
                    WriteType::Mixed,
                )
            } else {
                write_sampled_writes(
                    Writes::default(),
                    count,
                    &mut direct_rng,
                    WriteType::Mixed,
                    &sampler,
                )
            };
            prepare_sampled_writes(
                &mut prepared,
                count,
                &mut prepared_rng,
                WriteType::Mixed,
                &sampler,
            );
            assert_eq!(direct.0, expected);
            assert_eq!(prepared, expected);
        }
        let expected_next = expected_rng.next_u64();
        assert_eq!(direct_rng.next_u64(), expected_next);
        assert_eq!(prepared_rng.next_u64(), expected_next);
    }
}

impl MerkleizedBatch for Root {
    type Digest = Digest;

    fn root(&self) -> Digest {
        unreachable!("recording batch only checks generated operations")
    }
}

impl UnmerkleizedBatch<()> for Writes {
    type Family = Family;
    type K = Digest;
    type V = Digest;
    type Metadata = Digest;
    type Merkleized = Root;

    fn write(mut self, key: Digest, value: Option<Digest>) -> Self {
        self.0.push((key, value));
        self
    }

    async fn merkleize(self, _: &(), _: Option<Digest>) -> Result<Self::Merkleized, Error<Family>> {
        unreachable!("recording batch only checks generated operations")
    }
}

#[test]
fn preparation_preserves_operations_and_rng_across_iterations() {
    for exponent in [0.0, 0.75, 1.0, 3.0] {
        let sampler = WriteSampler::new(4, exponent).unwrap();
        for write_type in [
            WriteType::Overwrite,
            WriteType::Insert,
            WriteType::Delete,
            WriteType::Mixed,
        ] {
            let mut original_rng = TestRng::new(99);
            let mut prepared_rng = TestRng::new(99);
            let mut inputs = Vec::new();
            for count in [32, 0, 1, 16] {
                let original = if exponent == 0.0 {
                    write_random_writes(Writes::default(), count, 4, &mut original_rng, write_type)
                } else {
                    write_sampled_writes(
                        Writes::default(),
                        count,
                        &mut original_rng,
                        write_type,
                        &sampler,
                    )
                };
                let capacity = inputs.capacity();
                if exponent == 0.0 {
                    prepare_random_writes(&mut inputs, count, 4, &mut prepared_rng, write_type);
                } else {
                    prepare_sampled_writes(
                        &mut inputs,
                        count,
                        &mut prepared_rng,
                        write_type,
                        &sampler,
                    );
                }
                assert_eq!(inputs.len(), count as usize);
                assert!(inputs.capacity() >= capacity);
                assert_eq!(original, stage_prepared_writes(Writes::default(), &inputs));
                assert_eq!(original_rng.next_u64(), prepared_rng.next_u64());
            }
        }
    }
    let mut original_rng = TestRng::new(99);
    let mut prepared_rng = TestRng::new(99);
    let mut inputs = Vec::new();
    prepare_random_writes(&mut inputs, 32, 4, &mut prepared_rng, WriteType::Overwrite);
    assert_eq!(
        write_random_updates(Writes::default(), 32, 4, &mut original_rng),
        stage_prepared_writes(Writes::default(), &inputs),
    );
    assert_eq!(original_rng.next_u64(), prepared_rng.next_u64());
}

type AnyTestDb<F> = Db<
    F,
    deterministic::Context,
    Digest,
    Digest,
    commonware_cryptography::Sha256,
    commonware_storage::translator::EightCap,
    commonware_parallel::Rayon,
>;

type CurrentTestDb<F> = commonware_storage::qmdb::current::unordered::fixed::Db<
    F,
    deterministic::Context,
    Digest,
    Digest,
    commonware_cryptography::Sha256,
    commonware_storage::translator::EightCap,
    32,
    commonware_parallel::Rayon,
>;

type CurrentOrderedTestDb<F> = commonware_storage::qmdb::current::ordered::fixed::Db<
    F,
    deterministic::Context,
    Digest,
    Digest,
    commonware_cryptography::Sha256,
    commonware_storage::translator::EightCap,
    32,
    commonware_parallel::Rayon,
>;

async fn assert_equivalent_roots<F, C>(db: C, new_child: fn(&C::Merkleized) -> C::Batch)
where
    F: merkle::Family,
    C: DbAny<F, Key = Digest, Value = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
{
    let db = common::seed_db(db, 4).await;
    let mut inputs = Vec::new();
    for exponent in [0.0, 0.75, 3.0] {
        let sampler = WriteSampler::new(4, exponent).unwrap();
        for write_type in [
            WriteType::Overwrite,
            WriteType::Insert,
            WriteType::Delete,
            WriteType::Mixed,
        ] {
            for chained in [false, true] {
                let mut original_rng = TestRng::new(99);
                let mut prepared_rng = TestRng::new(99);
                for _ in 0..3 {
                    let parent = if chained {
                        Some(
                            write_sampled_writes::<_, C>(
                                db.new_batch(),
                                16,
                                &mut original_rng,
                                WriteType::Overwrite,
                                &sampler,
                            )
                            .merkleize(&db, None)
                            .await
                            .unwrap(),
                        )
                    } else {
                        None
                    };
                    let prepared_parent = if chained {
                        Some(
                            write_sampled_writes::<_, C>(
                                db.new_batch(),
                                16,
                                &mut prepared_rng,
                                WriteType::Overwrite,
                                &sampler,
                            )
                            .merkleize(&db, None)
                            .await
                            .unwrap(),
                        )
                    } else {
                        None
                    };
                    let original_batch = parent.as_ref().map_or_else(|| db.new_batch(), new_child);
                    let original = write_sampled_writes::<_, C>(
                        original_batch,
                        16,
                        &mut original_rng,
                        write_type,
                        &sampler,
                    )
                    .merkleize(&db, None)
                    .await
                    .unwrap();
                    prepare_sampled_writes(
                        &mut inputs,
                        16,
                        &mut prepared_rng,
                        write_type,
                        &sampler,
                    );
                    let prepared_batch = prepared_parent
                        .as_ref()
                        .map_or_else(|| db.new_batch(), new_child);
                    let prepared = stage_prepared_writes::<_, C>(prepared_batch, &inputs)
                        .merkleize(&db, None)
                        .await
                        .unwrap();
                    assert_eq!(original.root(), prepared.root());
                    assert_eq!(original_rng.next_u64(), prepared_rng.next_u64());
                }
            }
        }
    }
    db.destroy().await.unwrap();
}

#[test]
fn prepared_writes_preserve_roots_with_duplicates_and_pending_parents() {
    deterministic::Runner::default().start(|context| async move {
        let db = AnyTestDb::<merkle::mmr::Family>::init(
            context.child("any_mmr"),
            common::any_fix_cfg(&context),
        )
        .await
        .unwrap();
        assert_equivalent_roots(db, |parent| parent.new_batch()).await;

        let db = AnyTestDb::<merkle::mmb::Family>::init(
            context.child("any_mmb"),
            common::any_fix_cfg(&context),
        )
        .await
        .unwrap();
        assert_equivalent_roots(db, |parent| parent.new_batch()).await;

        let db = CurrentTestDb::<merkle::mmr::Family>::init(
            context.child("current_mmr"),
            common::cur_fix_cfg(&context),
        )
        .await
        .unwrap();
        assert_equivalent_roots(db, |parent| parent.new_batch()).await;

        let db = CurrentTestDb::<merkle::mmb::Family>::init(
            context.child("current_mmb"),
            common::cur_fix_cfg(&context),
        )
        .await
        .unwrap();
        assert_equivalent_roots(db, |parent| parent.new_batch()).await;
    });
}

#[test]
fn criterion_boundary_excludes_preparation_delay() {
    use commonware_bench::{Benchmark, Workload};
    use std::time::{Duration, Instant};

    struct DelayedPreparation {
        prepared: bool,
        iterations: u64,
    }

    impl Workload for DelayedPreparation {
        type Output = u64;

        async fn before_iter(&mut self) {
            assert!(!self.prepared);
            std::thread::sleep(Duration::from_millis(20));
            self.prepared = true;
        }

        async fn iter(&mut self) -> u64 {
            assert!(self.prepared);
            self.prepared = false;
            self.iterations += 1;
            std::thread::sleep(Duration::from_millis(1));
            self.iterations
        }

        async fn teardown(&mut self) {
            assert_eq!(self.iterations, 3);
            assert!(!self.prepared);
        }
    }

    deterministic::Runner::default().start(|_| async {
        let wall_start = Instant::now();
        let measured = Benchmark::new("timer_boundary")
            .criterion(
                DelayedPreparation {
                    prepared: false,
                    iterations: 0,
                },
                3,
            )
            .await;
        let wall = wall_start.elapsed();
        // The three deliberate preparation delays must all be outside reported time.
        assert!(wall.saturating_sub(measured) >= Duration::from_millis(60));
        assert!(measured >= Duration::from_millis(3));
        println!(
            "boundary probe: wall={wall:?}, reported={measured:?}, excluded={:?}",
            wall.saturating_sub(measured)
        );
    });
}

async fn assert_workload_commit<C>(db: C, write_type: WriteType)
where
    C: DbAny<merkle::mmb::Family, Key = Digest, Value = Digest, Digest = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
{
    let db = common::seed_db(db, 100).await;
    let initial_root = db.root();
    let mut original = Vec::new();
    for i in 0u64..200 {
        let key = commonware_cryptography::Sha256::hash(&[&i.to_be_bytes()]);
        let value = db.get(&key).await.unwrap();
        assert_eq!(value.is_some(), i < 100);
        original.push((key, value));
    }

    let mut writes = Vec::new();
    prepare_random_writes(&mut writes, 20, 100, &mut TestRng::new(1234), write_type);
    for &(key, value) in &writes {
        assert_eq!(value.is_none(), write_type == WriteType::Delete);
        assert_eq!(
            db.get(&key).await.unwrap().is_some(),
            write_type != WriteType::Insert
        );
    }
    // Include an explicit duplicate to exercise last-write-wins staging.
    writes.push(writes[0]);
    let mut expected = original.clone();
    for &(key, value) in &writes {
        expected.iter_mut().find(|(k, _)| *k == key).unwrap().1 = value;
    }

    // Discard a speculative batch as Criterion does; the same keys must remain usable.
    let speculative = stage_prepared_writes::<_, C>(db.new_batch(), &writes)
        .merkleize(&db, None)
        .await
        .unwrap();
    let expected_root = speculative.root();
    assert_ne!(initial_root, expected_root);
    drop(speculative);
    assert_eq!(db.root(), initial_root);
    for (key, value) in &original {
        assert_eq!(db.get(key).await.unwrap(), *value);
    }

    let merkleized = stage_prepared_writes::<_, C>(db.new_batch(), &writes)
        .merkleize(&db, None)
        .await
        .unwrap();
    assert_eq!(merkleized.root(), expected_root);
    let (db, _) = db.apply_batch(merkleized).await.unwrap();
    let db = db.commit().await.unwrap();
    assert_eq!(db.root(), expected_root);
    // Check every seeded and insert-range key, including unchanged survivor values.
    for (key, value) in expected {
        assert_eq!(db.get(&key).await.unwrap(), value);
    }
    db.destroy().await.unwrap();
}

#[test]
fn pure_workloads_commit_on_measured_variants() {
    deterministic::Runner::default().start(|context| async move {
        for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Delete] {
            let db = AnyTestDb::<merkle::mmb::Family>::init(
                context.child("any_unordered").child(write_type.name()),
                common::any_fix_cfg(&context),
            )
            .await
            .unwrap();
            assert_workload_commit(db, write_type).await;

            let db = CurrentOrderedTestDb::<merkle::mmb::Family>::init(
                context.child("current_ordered").child(write_type.name()),
                common::cur_fix_cfg(&context),
            )
            .await
            .unwrap();
            assert_workload_commit(db, write_type).await;
        }
    });
}
