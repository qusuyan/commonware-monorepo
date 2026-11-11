#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports)]
#[path = "../src/qmdb/benches/apply_study.rs"]
mod apply_study;
#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::{Digest, KeySet, WriteSampler, WriteType};
use commonware_cryptography::{Hasher as _, Sha256};
use commonware_parallel::Rayon;
use commonware_runtime::{Runner as _, Supervisor as _, deterministic};
use commonware_storage::{
    merkle::mmb,
    qmdb::any::traits::{DbAny, MerkleizedBatch},
};
use commonware_utils::TestRng;
use std::collections::BTreeMap;

type Any = commonware_storage::qmdb::any::unordered::fixed::BTreeDb<
    mmb::Family,
    deterministic::Context,
    Digest,
    Digest,
    Sha256,
    commonware_storage::translator::EightCap,
    Rayon,
>;
type Current = commonware_storage::qmdb::current::ordered::fixed::Db<
    mmb::Family,
    deterministic::Context,
    Digest,
    Digest,
    Sha256,
    commonware_storage::translator::EightCap,
    32,
    Rayon,
>;

async fn check<C>(db: C, kind: WriteType, skew: f64) -> Digest
where
    C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
{
    let keys = KeySet::generate_parallel(100);
    let db = common::seed_db_with_keys(db, keys.slice(100)).await;
    let mut values = BTreeMap::new();
    let mut rng = TestRng::new(42);
    for &key in keys.slice(100) {
        values.insert(key, Some(common::make_fixed_value(&mut rng)));
    }
    let initial = db.root();
    let sampler = WriteSampler::new(100, skew).unwrap();
    let mut writes = Vec::new();
    // More calls than keys ensures duplicates, including repeated deletions and inserts.
    common::prepare_sampled_writes(&mut writes, 250, &mut TestRng::new(99), kind, &sampler);
    let mut repeated = Vec::new();
    common::prepare_sampled_writes(&mut repeated, 250, &mut TestRng::new(99), kind, &sampler);
    assert_eq!(writes, repeated);
    for &(key, value) in &writes {
        values.insert(key, value);
    }
    let (db, _) = apply_study::apply_prepared(db, &writes).await;
    assert_ne!(db.root(), initial);
    for i in 0..200u64 {
        let key = Sha256::hash(&[&i.to_be_bytes()]);
        assert_eq!(
            db.get(&key).await.unwrap(),
            values.get(&key).copied().flatten()
        );
    }
    let root = db.root();
    db.destroy().await.unwrap();
    root
}

#[test]
fn apply_study_preserves_values_duplicates_and_engine_roots() {
    deterministic::Runner::default().start(|ctx| async move {
        let descriptor = common::merkleize_config::Descriptor::production();
        for kind in [
            WriteType::Overwrite,
            WriteType::Insert,
            WriteType::Delete,
            WriteType::Mixed,
        ] {
            for skew in [0.0, 2.0] {
                let label = if skew == 0.0 { "uniform" } else { "skewed" };
                let any_ctx = ctx.child("any").child(kind.name()).child(label);
                let any = Any::init(
                    any_ctx.child("storage"),
                    descriptor.any(&any_ctx, descriptor.cache(&any_ctx)),
                )
                .await
                .unwrap();
                let first = check(any, kind, skew).await;
                let current_ctx = ctx.child("current").child(kind.name()).child(label);
                let current = Current::init(
                    current_ctx.child("storage"),
                    descriptor.current(&current_ctx, descriptor.cache(&current_ctx)),
                )
                .await
                .unwrap();
                let first_current = check(current, kind, skew).await;
                let repeat_current_ctx =
                    ctx.child("repeat_current").child(kind.name()).child(label);
                let current_repeat = Current::init(
                    repeat_current_ctx.child("storage"),
                    descriptor.current(&repeat_current_ctx, descriptor.cache(&repeat_current_ctx)),
                )
                .await
                .unwrap();
                assert_eq!(first_current, check(current_repeat, kind, skew).await);
                // Independently seeded instances repeat the exact workload and resulting root.
                let repeat_ctx = ctx.child("repeat").child(kind.name()).child(label);
                let any = Any::init(
                    repeat_ctx.child("storage"),
                    descriptor.any(&repeat_ctx, descriptor.cache(&repeat_ctx)),
                )
                .await
                .unwrap();
                assert_eq!(first, check(any, kind, skew).await);
            }
        }
    });
}

/// A deliberately slow stand-in isolates each phase of the actual apply-study helper.
mod timing_probe {
    use super::*;
    use commonware_runtime::Handle;
    use commonware_storage::{
        merkle::Location,
        qmdb::{
            Error,
            any::traits::{ApplyBatchResult, BatchableDb, UnmerkleizedBatch},
        },
    };
    use std::{
        ops::Range,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct Delays {
        excluded: Mutex<Duration>,
        applied: Mutex<Duration>,
    }
    impl Delays {
        fn pause(&self, applying: bool) {
            let start = Instant::now();
            std::thread::sleep(Duration::from_millis(15));
            let elapsed = start.elapsed();
            let counter = if applying {
                &self.applied
            } else {
                &self.excluded
            };
            *counter.lock().unwrap() += elapsed;
        }
    }
    struct Db {
        delays: Arc<Delays>,
        root: Digest,
    }
    struct Batch {
        delays: Arc<Delays>,
        root: Digest,
    }
    struct Root {
        delays: Arc<Delays>,
        root: Digest,
    }

    impl Drop for Db {
        fn drop(&mut self) {
            self.delays.pause(false);
        }
    }
    impl MerkleizedBatch for Root {
        type Digest = Digest;
        fn root(&self) -> Digest {
            self.delays.pause(false);
            self.root
        }
    }
    impl UnmerkleizedBatch<Db> for Batch {
        type Family = mmb::Family;
        type K = Digest;
        type V = Digest;
        type Metadata = Digest;
        type Merkleized = Root;
        fn write(self, _: Digest, _: Option<Digest>) -> Self {
            self.delays.pause(false);
            self
        }
        async fn merkleize(self, _: &Db, _: Option<Digest>) -> Result<Root, Error<mmb::Family>> {
            self.delays.pause(false);
            Ok(Root {
                delays: self.delays,
                root: self.root,
            })
        }
    }
    impl BatchableDb for Db {
        type Family = mmb::Family;
        type K = Digest;
        type V = Digest;
        type Merkleized = Root;
        type Batch = Batch;
        fn new_batch(&self) -> Batch {
            self.delays.pause(false);
            Batch {
                delays: Arc::clone(&self.delays),
                root: self.root,
            }
        }
        async fn apply_batch(self, batch: Root) -> ApplyBatchResult<Self> {
            assert_eq!(batch.root, self.root);
            self.delays.pause(true);
            Ok((self, Location::new(0)..Location::new(1)))
        }
    }
    impl DbAny<mmb::Family> for Db {
        type Key = Digest;
        type Value = Digest;
        type Digest = Digest;
        fn root(&self) -> Digest {
            self.delays.pause(false);
            self.root
        }
        fn bounds(&self) -> Range<Location<mmb::Family>> {
            unreachable!()
        }
        async fn get(&self, _: &Digest) -> Result<Option<Digest>, Error<mmb::Family>> {
            unreachable!()
        }
        async fn get_many(&self, _: &[&Digest]) -> Result<Vec<Option<Digest>>, Error<mmb::Family>> {
            unreachable!()
        }
        async fn get_metadata(&self) -> Result<Option<Digest>, Error<mmb::Family>> {
            unreachable!()
        }
        async fn prune(self, _: Location<mmb::Family>) -> Result<Self, Error<mmb::Family>> {
            unreachable!()
        }
        async fn start_sync(self) -> Result<(Self, Handle<()>), Error<mmb::Family>> {
            panic!("apply study must not start syncing")
        }
        async fn commit(self) -> Result<Self, Error<mmb::Family>> {
            panic!("apply study must not commit")
        }
        async fn sync(self) -> Result<Self, Error<mmb::Family>> {
            panic!("apply study must not sync")
        }
        async fn destroy(self) -> Result<(), Error<mmb::Family>> {
            unreachable!()
        }
        fn inactivity_floor_loc(&self) -> Location<mmb::Family> {
            unreachable!()
        }
        fn sync_boundary(&self) -> Location<mmb::Family> {
            unreachable!()
        }
    }

    #[test]
    fn actual_helper_times_apply_and_excludes_preparation_validation_and_drop() {
        deterministic::Runner::default().start(|_| async {
            let delays = Arc::new(Delays::default());
            let root = Sha256::hash(&[b"timer probe"]);
            let db = Db { delays: Arc::clone(&delays), root };
            let wall_start = Instant::now();
            let (db, measured) = apply_study::apply_prepared(db, &[(root, Some(root))]).await;
            drop(db);
            let wall = wall_start.elapsed();
            let applied = *delays.applied.lock().unwrap();
            let excluded = *delays.excluded.lock().unwrap();
            // Compare observed nested intervals, avoiding machine-dependent absolute limits.
            assert!(measured >= applied, "apply delay missing from reported latency");
            assert!(wall.saturating_sub(measured) >= excluded, "non-apply phases leaked into latency");
            println!("apply timing probe: wall={wall:?}, reported={measured:?}, apply_delay={applied:?}, excluded_delay={excluded:?}");
        });
    }
}
