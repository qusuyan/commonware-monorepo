#![cfg(feature = "test-traits")]
#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;
#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/merkleize.rs"]
mod merkleize;
use common::{
    AnyUFixBTreeDb, CurOFixDb, Digest, KeySet, ScratchDir, WriteType,
    merkleize_config::Descriptor,
    snapshots::{self, ReuseMode, Variant},
};
use commonware_bench::Workload;
use commonware_cryptography::Sha256;
use commonware_runtime::{Runner as _, Supervisor as _, buffer::paged::CacheRef, tokio};
use commonware_storage::{
    merkle::mmb,
    qmdb::any::traits::{DbAny, MerkleizedBatch},
};
use std::{fs, path::Path};

async fn exercise<C>(
    ctx: tokio::Context,
    db: C,
    pc: CacheRef,
    variant: merkleize::Variant,
    manifest: &snapshots::Manifest,
    new_child: fn(&C::Merkleized) -> C::Batch,
) -> Vec<Digest>
where
    C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>,
    C::Merkleized: MerkleizedBatch<Digest = Digest>,
{
    snapshots::check_database(&db, manifest).unwrap();
    let mut rng = commonware_utils::TestRng::new(42);
    use commonware_cryptography::{Hasher as _, Sha256};
    for i in 0..manifest.key_count {
        let key = Sha256::hash(&[&i.to_be_bytes()]);
        assert_eq!(
            db.get(&key).await.unwrap(),
            Some(common::make_fixed_value(&mut rng))
        );
    }
    use commonware_storage::qmdb::any::traits::UnmerkleizedBatch as _;
    let mut batch_roots = Vec::new();
    for write_type in [WriteType::Overwrite, WriteType::Insert, WriteType::Mixed] {
        for skew in [0.0, 2.0] {
            let mut rng = commonware_utils::TestRng::new(99);
            let sampler = common::WriteSampler::new(manifest.key_count, skew).unwrap();
            let batch =
                common::write_sampled_writes(db.new_batch(), 17, &mut rng, write_type, &sampler);
            batch_roots.push(batch.merkleize(&db, None).await.unwrap().root());
        }
    }
    // Each batch is speculative; resetting RNG here makes the fresh/reused streams identical.
    let options = merkleize::BenchOptions {
        variant,
        num_keys: manifest.key_count,
        fixed_updates: Some(17),
        write_type: Some(WriteType::Overwrite),
        skew: None,
        chained: false,
        seed_sync: true,
        clear_cache: false,
        snapshot_dir: Some("scratch".into()),
    };
    let mut workload =
        merkleize::MerkleizeWorkload::<mmb::Family, C>::new(ctx, db, pc, options, new_child);
    workload.setup().await;
    let mut roots = batch_roots;
    for _ in 0..3 {
        workload.before_iter().await;
        roots.push(workload.iter().await);
    }
    workload.teardown().await;
    roots
}
fn benchmark_roots(path: &Path, variant: Variant, manifest: &snapshots::Manifest) -> Vec<Digest> {
    tokio::Runner::new(tokio::Config::new().with_storage_directory(path)).start(|ctx| async move {
        let pc = Descriptor::production().cache(&ctx);
        match variant {
            Variant::AnyUnorderedBtree => {
                let db = AnyUFixBTreeDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    merkleize::any_fix_cfg_with_cache(&ctx, pc.clone()),
                )
                .await
                .unwrap();
                exercise::<AnyUFixBTreeDb<mmb::Family>>(
                    ctx,
                    db,
                    pc,
                    merkleize::Variant::AnyUnorderedBTreeFixedMmb,
                    manifest,
                    |parent| parent.new_batch::<Sha256>(),
                )
                .await
            }
            Variant::CurrentOrdered => {
                let db = CurOFixDb::<mmb::Family>::init(
                    ctx.child("storage"),
                    merkleize::cur_fix_cfg_with_cache(&ctx, pc.clone()),
                )
                .await
                .unwrap();
                exercise::<CurOFixDb<mmb::Family>>(
                    ctx,
                    db,
                    pc,
                    merkleize::Variant::CurrentOrderedFixed32Mmb,
                    manifest,
                    |parent| parent.new_batch::<Sha256>(),
                )
                .await
            }
        }
    })
}
#[test]
fn creator_to_actual_benchmark_and_repeated_reuse() {
    for variant in [Variant::AnyUnorderedBtree, Variant::CurrentOrdered] {
        let base = ScratchDir::empty(&std::env::temp_dir()).unwrap();
        let master = base.path().join("master");
        let keys = KeySet::generate_parallel(129);
        let descriptor = Descriptor::production();
        let manifest = snapshots::create(&master, variant, &descriptor, keys.slice(129)).unwrap();
        let before = snapshots::inventory(&master).unwrap();
        snapshots::inspect(&master).unwrap();
        let idempotent = snapshots::create(&master, variant, &descriptor, keys.slice(129)).unwrap();
        assert_eq!(manifest.merkle_root, idempotent.merkle_root);
        let mut expected = None;
        for mode in [
            ReuseMode::Off,
            ReuseMode::Required,
            ReuseMode::Required,
            ReuseMode::Auto,
        ] {
            let prepared = snapshots::prepare_at(variant, 129, mode, &master).unwrap();
            assert_ne!(prepared.scratch.path(), master);
            let roots = benchmark_roots(prepared.scratch.path(), variant, &prepared.manifest);
            if let Some(prior) = &expected {
                assert_eq!(prior, &roots);
            } else {
                expected = Some(roots);
            }
            let path = prepared.scratch.path().to_path_buf();
            drop(prepared);
            assert!(!path.exists());
        }
        assert_eq!(snapshots::inventory(&master).unwrap(), before);
    }
}
#[test]
fn small_blob_boundary_roundtrip() {
    for variant in [Variant::AnyUnorderedBtree, Variant::CurrentOrdered] {
        let base = ScratchDir::empty(&std::env::temp_dir()).unwrap();
        let master = base.path().join("master");
        let descriptor = Descriptor::with_blob_capacity(16);
        let keys = KeySet::generate_parallel(65);
        let manifest = snapshots::create(&master, variant, &descriptor, keys.slice(65)).unwrap();
        snapshots::validate(&master, variant, 65, &descriptor).unwrap();
        let scratch = ScratchDir::new_from_clone(&master, base.path()).unwrap();
        snapshots::verify_open(scratch.path(), &manifest, Some(keys.slice(65))).unwrap();
        assert!(snapshots::validate(&master, variant, 65, &Descriptor::production()).is_err());
    }
}
#[test]
fn invalid_snapshot_is_rejected_without_mutating_master() {
    let base = ScratchDir::empty(&std::env::temp_dir()).unwrap();
    let master = base.path().join("master");
    let variant = Variant::AnyUnorderedBtree;
    let keys = KeySet::generate_parallel(33);
    let descriptor = Descriptor::production();
    snapshots::create(&master, variant, &descriptor, keys.slice(33)).unwrap();
    let manifest_path = master.join(snapshots::MANIFEST);
    let original = fs::read(&manifest_path).unwrap();
    let mut manifest = snapshots::read_manifest(&master).unwrap();
    manifest.merkle_root = "0".repeat(64);
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(snapshots::inspect(&master).is_err());
    assert!(snapshots::prepare_at(variant, 33, ReuseMode::Auto, &master).is_err());
    fs::write(&manifest_path, original).unwrap();
    let file = master.join(manifest.files.keys().next().unwrap());
    let old = fs::read(&file).unwrap();
    fs::write(&file, &old[..old.len().saturating_sub(1)]).unwrap();
    assert!(snapshots::inspect(&master).is_err());
    assert!(snapshots::create(&master, variant, &descriptor, keys.slice(33)).is_err());
}
#[test]
fn modes_missing_files_schema_and_concurrent_scratch() {
    assert_eq!(ReuseMode::parse(None).unwrap(), ReuseMode::Required);
    for (value, expected) in [
        ("required", ReuseMode::Required),
        ("auto", ReuseMode::Auto),
        ("1", ReuseMode::Auto),
        ("off", ReuseMode::Off),
        ("0", ReuseMode::Off),
    ] {
        assert_eq!(ReuseMode::parse(Some(value)).unwrap(), expected);
    }
    assert!(ReuseMode::parse(Some("false")).is_err());
    let base = ScratchDir::empty(&std::env::temp_dir()).unwrap();
    let missing = base.path().join("missing");
    assert!(
        snapshots::prepare_at(Variant::AnyUnorderedBtree, 5, ReuseMode::Required, &missing)
            .is_err()
    );
    assert!(!missing.exists());
    let fresh =
        snapshots::prepare_at(Variant::AnyUnorderedBtree, 5, ReuseMode::Auto, &missing).unwrap();
    assert_eq!(fresh.source, "fresh");
    fs::create_dir(&missing).unwrap();
    fs::write(missing.join(snapshots::MANIFEST), b"{\"schema_version\":1}").unwrap();
    assert!(snapshots::inspect(&missing).is_err());
    let dirs: Vec<_> = std::thread::scope(|scope| {
        (0..8)
            .map(|_| {
                scope.spawn(|| {
                    ScratchDir::new_from_clone(fresh.scratch.path(), base.path()).unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect()
    });
    let mut names: Vec<_> = dirs.iter().map(|d| d.path().to_path_buf()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 8);
    drop(dirs);
    assert!(names.iter().all(|p| !p.exists()));
}

#[test]
fn scratch_mutations_and_failed_clones_preserve_sources() {
    use commonware_cryptography::{Hasher as _, Sha256};
    use commonware_storage::qmdb::any::traits::UnmerkleizedBatch as _;
    async fn mutate<C: DbAny<mmb::Family, Key = Digest, Value = Digest, Digest = Digest>>(db: C) {
        let key = Sha256::hash(&[&0u64.to_be_bytes()]);
        let batch = db.new_batch().write(key, Some(Sha256::hash(&[b"changed"])));
        let batch = batch.merkleize(&db, None).await.unwrap();
        let (db, _) = db.apply_batch(batch).await.unwrap();
        db.commit().await.unwrap().sync().await.unwrap();
    }
    for variant in [Variant::AnyUnorderedBtree, Variant::CurrentOrdered] {
        let base = ScratchDir::empty(&std::env::temp_dir()).unwrap();
        let master = base.path().join("master");
        let keys = KeySet::generate_parallel(65);
        snapshots::create(&master, variant, &Descriptor::production(), keys.slice(65)).unwrap();
        let before = snapshots::inventory(&master).unwrap();
        let scratch = ScratchDir::new_from_clone(&master, base.path()).unwrap();
        tokio::Runner::new(tokio::Config::new().with_storage_directory(scratch.path())).start(
            |ctx| async move {
                let descriptor = Descriptor::production();
                let pc = descriptor.cache(&ctx);
                match variant {
                    Variant::AnyUnorderedBtree => {
                        mutate(
                            AnyUFixBTreeDb::<mmb::Family>::init(
                                ctx.child("storage"),
                                descriptor.any(&ctx, pc),
                            )
                            .await
                            .unwrap(),
                        )
                        .await
                    }
                    Variant::CurrentOrdered => {
                        mutate(
                            CurOFixDb::<mmb::Family>::init(
                                ctx.child("storage"),
                                descriptor.current(&ctx, pc),
                            )
                            .await
                            .unwrap(),
                        )
                        .await
                    }
                }
            },
        );
        assert_eq!(snapshots::inventory(&master).unwrap(), before);
        snapshots::inspect(&master).unwrap();
        assert_ne!(snapshots::inventory(scratch.path()).unwrap(), before);
        let path = scratch.path().to_path_buf();
        drop(scratch);
        assert!(!path.exists());
        assert!(ScratchDir::new_from_clone(&base.path().join("missing"), base.path()).is_err());
        assert_eq!(fs::read_dir(base.path()).unwrap().count(), 1);
    }
}
