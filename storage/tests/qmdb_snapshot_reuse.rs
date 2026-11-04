#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::{
    AnyUFixBTreeDb, CurOFixDb, KeySet, ScratchDir, any_fix_cfg, clone_directory,
    cur_fix_cfg, make_fixed_value, seed_db_with_keys,
};
use commonware_cryptography::{DigestOf, Hasher as _, Sha256};
use commonware_runtime::{
    Runner as _, Supervisor,
    tokio::{self},
};
use commonware_storage::merkle::mmb;
use commonware_utils::TestRng;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[allow(dead_code)]
type Digest = DigestOf<Sha256>;
type AnyVariant = AnyUFixBTreeDb<mmb::Family>;
type OrderedVariant = CurOFixDb<mmb::Family>;

#[test]
fn test_clone_directory_speed_and_isolation() {
    let temp = TempDir::new("test_clone_iso");
    let src = temp.path().join("master");
    fs::create_dir_all(src.join("subdir")).unwrap();

    fs::write(src.join("file1.bin"), b"master data 1").unwrap();
    fs::write(src.join("subdir/file2.bin"), b"master data 2").unwrap();

    let dst = temp.path().join("clone");
    let start = Instant::now();
    let elapsed = clone_directory(&src, &dst).unwrap();
    println!("clone_directory took {:?} (wall clock: {:?})", elapsed, start.elapsed());

    // Verify cloned contents
    assert_eq!(fs::read(dst.join("file1.bin")).unwrap(), b"master data 1");
    assert_eq!(fs::read(dst.join("subdir/file2.bin")).unwrap(), b"master data 2");

    // Mutate clone and assert master is isolated
    fs::write(dst.join("file1.bin"), b"mutated in clone").unwrap();
    assert_eq!(fs::read(src.join("file1.bin")).unwrap(), b"master data 1");
}

#[test]
fn test_scratch_dir_lifecycle() {
    let temp = TempDir::new("test_scratch_lc");
    let master = temp.path().join("master");
    let scratch_base = temp.path().join("scratch");
    fs::create_dir_all(&master).unwrap();
    fs::write(master.join("test.bin"), b"seed data").unwrap();

    // Test auto-cleanup on drop
    let scratch_path;
    {
        let scratch = ScratchDir::new_from_clone(&master, &scratch_base).unwrap();
        scratch_path = scratch.path().to_path_buf();
        assert!(scratch_path.exists());
        assert_eq!(fs::read(scratch_path.join("test.bin")).unwrap(), b"seed data");
    }
    // ScratchDir was dropped, path should no longer exist
    assert!(!scratch_path.exists());
}

#[test]
fn test_any_unordered_btree_snapshot_reuse_and_root_invariance() {
    let temp = TempDir::new("test_any_btree_reuse");
    let master_dir = temp.path().join("master_any_btree");
    let keyset = KeySet::generate_parallel(1_000);
    let keys = keyset.slice(1_000);

    let initial_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(&master_dir))
        .start(|ctx| async move {
            let cfg = any_fix_cfg(&ctx);
            let db = AnyVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let db = seed_db_with_keys(db, keys).await;
            let root = db.root();
            let db = db.sync().await.unwrap();
            drop(db);
            root
        });

    // 1. Reopen master snapshot directly in place (0 ms setup, 0 copy)
    let reopened_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(&master_dir))
        .start(|ctx| async move {
            let cfg = any_fix_cfg(&ctx);
            let db = AnyVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let root = db.root();
            drop(db);
            root
        });
    assert_eq!(reopened_root, initial_root, "Reopened master DB must have identical root");

    // 2. Clone master to scratch directory
    let scratch_base = temp.path().join("scratch");
    let scratch = ScratchDir::new_from_clone(&master_dir, &scratch_base).unwrap();

    // Verify clone root matches master
    let clone_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(scratch.path()))
        .start(|ctx| async move {
            let cfg = any_fix_cfg(&ctx);
            let db = AnyVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let root = db.root();
            drop(db);
            root
        });
    assert_eq!(clone_root, initial_root, "Cloned DB must have identical root");

    // 3. Mutate clone with new writes and commit
    let mutated_clone_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(scratch.path()))
        .start(|ctx| async move {
            let cfg = any_fix_cfg(&ctx);
            let db = AnyVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let mut rng = TestRng::new(1234);
            let mut batch = db.new_batch();
            let extra_key = Sha256::hash(&[&999_999u64.to_be_bytes()]);
            batch = batch.write(extra_key, Some(make_fixed_value(&mut rng)));
            let merkleized = batch.merkleize(&db, None).await.unwrap();
            let (db, _) = db.apply_batch(merkleized).await.unwrap();
            let db = db.commit().await.unwrap();
            let db = db.sync().await.unwrap();
            let root = db.root();
            drop(db);
            root
        });
    assert_ne!(mutated_clone_root, initial_root, "Mutated clone root must change");

    // 4. Assert master DB root remains completely untouched
    let master_root_after = tokio::Runner::new(tokio::Config::new().with_storage_directory(&master_dir))
        .start(|ctx| async move {
            let cfg = any_fix_cfg(&ctx);
            let db = AnyVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let root = db.root();
            drop(db);
            root
        });
    assert_eq!(master_root_after, initial_root, "Master DB root must remain completely unaffected by clone mutations");
}

#[test]
fn test_current_ordered_snapshot_reuse_and_root_invariance() {
    let temp = TempDir::new("test_cur_ordered_reuse");
    let master_dir = temp.path().join("master_cur_ordered");
    let keyset = KeySet::generate_parallel(1_000);
    let keys = keyset.slice(1_000);

    let initial_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(&master_dir))
        .start(|ctx| async move {
            let cfg = cur_fix_cfg(&ctx);
            let db = OrderedVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let db = seed_db_with_keys(db, keys).await;
            let root = db.root();
            let db = db.sync().await.unwrap();
            drop(db);
            root
        });

    // Reopen master snapshot directly
    let reopened_root = tokio::Runner::new(tokio::Config::new().with_storage_directory(&master_dir))
        .start(|ctx| async move {
            let cfg = cur_fix_cfg(&ctx);
            let db = OrderedVariant::init(ctx.child("storage"), cfg).await.unwrap();
            let root = db.root();
            drop(db);
            root
        });
    assert_eq!(reopened_root, initial_root, "Reopened ordered master DB must have identical root");
}
