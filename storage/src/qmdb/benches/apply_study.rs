//! Apply-only study of Any unordered B-Tree versus Current ordered.
//!
//! Every iteration opens an independent scratch seed. Input preparation and Merkleization
//! precede the timer; only application is timed. No commit or sync follows application.

use crate::common::{
    AnyUFixBTreeDb, CurOFixDb, Digest, WriteSampler, WriteType,
    merkleize_config::Descriptor,
    parse_write_skew, prepare_sampled_writes,
    snapshots::{self, ReuseMode, Variant},
    stage_prepared_writes,
};
use commonware_runtime::{Runner as _, Supervisor as _, tokio};
use commonware_storage::{
    merkle::mmb,
    qmdb::any::traits::{DbAny, MerkleizedBatch, UnmerkleizedBatch as _},
};
use commonware_utils::TestRng;
use criterion::{Criterion, criterion_group};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

/// Stage and Merkleize outside timing, then time publication of the prepared batch.
pub(crate) async fn apply_prepared<C>(db: C, writes: &[(Digest, Option<Digest>)]) -> (C, Duration)
where
    C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
{
    let batch = stage_prepared_writes(db.new_batch(), writes);
    let batch = batch.merkleize(&db, None).await.unwrap();
    let expected = batch.root();
    let start = Instant::now();
    let (db, _) = db.apply_batch(batch).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(db.root(), expected, "applied root must match prepared root");
    black_box(&db);
    (db, elapsed)
}

fn positive_env(name: &str, default: u64) -> u64 {
    let n = std::env::var(name).map_or(default, |s| s.parse().expect("positive u64"));
    assert!(n > 0, "{name} must be positive");
    n
}

fn bench_apply_study(c: &mut Criterion) {
    // Retain the existing workload environment names so both studies use the same runners.
    let keys = positive_env("QMDB_MERKLEIZE_KEYS", 4_000_000);
    let updates = positive_env("QMDB_MERKLEIZE_UPDATES", 10_000);
    let write_type = std::env::var("QMDB_MERKLEIZE_WRITE_TYPE")
        .ok()
        .map(|s| match s.as_str() {
            "overwrite" => WriteType::Overwrite,
            "insert" => WriteType::Insert,
            "delete" => WriteType::Delete,
            "mixed" => WriteType::Mixed,
            _ => panic!("QMDB_MERKLEIZE_WRITE_TYPE must be overwrite, insert, delete, or mixed"),
        });
    let skew = std::env::var("QMDB_MERKLEIZE_SKEW")
        .ok()
        .map(|s| parse_write_skew(&s).expect("valid skew"));
    let sampler = WriteSampler::new(keys, skew.unwrap_or(0.0)).expect("valid workload");
    for (variant, selector) in [
        (
            Variant::AnyUnorderedBtree,
            "any::unordered_btree::fixed::mmb",
        ),
        (
            Variant::CurrentOrdered,
            "current::ordered::fixed::mmb chunk=32",
        ),
    ] {
        let mut name = format!("qmdb::apply_study/v={selector} k={keys} u={updates}");
        if let Some(w) = write_type {
            name.push_str(&format!(" w={}", w.name()));
        }
        if let Some(s) = skew {
            name.push_str(&format!(" zipf={s}"));
        }
        name.push_str(" ch=false s=true cc=false");
        c.bench_function(&name, |b| {
            let mode = ReuseMode::parse(std::env::var("QMDB_SNAPSHOT_REUSE").ok().as_deref())
                .expect("valid reuse mode");
            let mut writes = Vec::new();
            prepare_sampled_writes(
                &mut writes,
                updates,
                &mut TestRng::new(99),
                write_type.unwrap_or(WriteType::Overwrite),
                &sampler,
            );
            let writes = &writes;
            b.iter_custom(|iters| {
                assert!(
                    iters <= 2,
                    "apply_study: requested iteration count ({iters}) is too large for database snapshotting. \
                    Pass '--warm-up-time 0.05 --measurement-time 0.05' or ensure sampling_mode is Flat."
                );
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let prepared =
                        snapshots::prepare_scratch(variant, keys, mode).expect("prepare validated seed");
                    let mut provenance = prepared.provenance(&name);
                    provenance["iteration_policy"] = "independent_snapshot_fixed_inputs".into();
                    println!("{provenance}");
                    let descriptor = Descriptor::production();
                    let manifest = &prepared.manifest;
                    let descriptor = &descriptor;
                    let elapsed = tokio::Runner::new(
                        tokio::Config::new().with_storage_directory(prepared.scratch.path()),
                    )
                    .start(|ctx| async move {
                        let pc = descriptor.cache(&ctx);
                        match variant {
                            Variant::AnyUnorderedBtree => {
                                let db = AnyUFixBTreeDb::<mmb::Family>::init(
                                    ctx.child("storage"),
                                    descriptor.any(&ctx, pc),
                                )
                                .await
                                .unwrap();
                                snapshots::check_database(&db, manifest).unwrap();
                                let (db, elapsed) = apply_prepared(db, writes).await;
                                drop(db);
                                elapsed
                            }
                            Variant::CurrentOrdered => {
                                let db = CurOFixDb::<mmb::Family>::init(
                                    ctx.child("storage"),
                                    descriptor.current(&ctx, pc),
                                )
                                .await
                                .unwrap();
                                snapshots::check_database(&db, manifest).unwrap();
                                let (db, elapsed) = apply_prepared(db, writes).await;
                                drop(db);
                                elapsed
                            }
                        }
                    });
                    total += elapsed;
                    // Runtime and database handles have dropped before scratch cleanup.
                    drop(prepared);
                }
                total
            });
        });
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(20))
        .measurement_time(Duration::from_millis(10));
    targets = bench_apply_study
}
