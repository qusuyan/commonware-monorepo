#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::{
    Digest, WriteSampler, WriteType, prepare_random_writes, prepare_sampled_writes,
    stage_prepared_writes, write_random_updates, write_random_writes, write_sampled_writes,
};
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
        for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Mixed] {
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
        for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Mixed] {
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
