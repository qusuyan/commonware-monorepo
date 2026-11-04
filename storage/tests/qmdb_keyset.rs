#![cfg(feature = "test-traits")]

#[allow(dead_code, unused_imports, unused_macros)]
#[path = "../src/qmdb/benches/common.rs"]
mod common;

use common::KeySet;
use commonware_cryptography::{Hasher as _, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    fs,
    path::{Path, PathBuf},
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

#[test]
fn test_keyset_generation_and_boundary_verification() {
    let count = 10_000;
    let keyset = KeySet::generate_parallel(count);
    assert_eq!(keyset.len(), count);
    assert!(!keyset.is_empty());

    // Boundary digests check
    let k0 = Sha256::hash(&[&0u64.to_be_bytes()]);
    let k_last = Sha256::hash(&[&(count as u64 - 1).to_be_bytes()]);
    assert_eq!(keyset.slice(1)[0], k0);
    assert_eq!(keyset.slice(count)[count - 1], k_last);
}

#[test]
fn test_keyset_prefix_slicing_invariance() {
    let count_large = 2_000;
    let count_small = 1_000;

    let keyset_large = KeySet::generate_parallel(count_large);
    let keyset_small = KeySet::generate_parallel(count_small);

    let prefix_slice = keyset_large.slice(count_small);
    assert_eq!(prefix_slice.len(), count_small);
    assert_eq!(prefix_slice, keyset_small.slice(count_small));
}

#[test]
fn test_keyset_save_and_load_roundtrip() {
    let temp = TempDir::new("test_keyset_roundtrip");
    let file_path = temp.path().join("test_keys.bin");

    let count = 5_000;
    let saved = KeySet::generate_and_save(&file_path, count).unwrap();
    assert_eq!(saved.len(), count);

    // Assert exact file size: count * 32 bytes
    let metadata = fs::metadata(&file_path).unwrap();
    assert_eq!(metadata.len(), (count * 32) as u64);

    // Load back and compare
    let loaded = KeySet::load_from_file(&file_path).unwrap();
    assert_eq!(loaded.len(), count);
    assert_eq!(saved.slice(count), loaded.slice(count));
}

#[test]
fn test_keyset_load_or_generate() {
    let temp = TempDir::new("test_keyset_load_or_gen");
    let file_path = temp.path().join("lazy_keys.bin");

    // First call: file does not exist, generates and saves
    let keyset1 = KeySet::load_or_generate(&file_path, 1_000).unwrap();
    assert_eq!(keyset1.len(), 1_000);
    assert!(file_path.exists());

    // Second call: file exists, loads from disk directly
    let keyset2 = KeySet::load_or_generate(&file_path, 1_000).unwrap();
    assert_eq!(keyset2.len(), 1_000);
    assert_eq!(keyset1.slice(1_000), keyset2.slice(1_000));
}

#[test]
fn test_interior_corruption_and_legacy_import() {
    let temp = TempDir::new("keyset_corruption");
    let path = temp.path().join("keys.bin");
    let keys = KeySet::generate_and_save(&path, 10).unwrap();
    KeySet::load_from_file(&path).unwrap();
    let original_mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let mut raw = fs::read(&path).unwrap();
    raw[32..64].copy_from_slice(keys.slice(1)[0].as_ref());
    fs::write(&path, &raw).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    assert!(KeySet::load_from_file(&path).is_err());
    fs::remove_file(KeySet::sidecar(&path)).unwrap();
    assert!(KeySet::load_from_file(&path).is_err());
    assert!(!KeySet::sidecar(&path).exists());
}

#[test]
fn test_legacy_import_idempotence_and_insufficient_file_preservation() {
    let temp = TempDir::new("keyset_import");
    let path = temp.path().join("keys.bin");
    let keys = KeySet::generate_parallel(10);
    let raw: Vec<u8> = keys
        .slice(10)
        .iter()
        .flat_map(|k| k.as_ref().iter().copied())
        .collect();
    fs::write(&path, &raw).unwrap();
    assert_eq!(
        KeySet::load_from_file(&path).unwrap().slice(10),
        keys.slice(10)
    );
    assert!(KeySet::sidecar(&path).exists());
    assert!(KeySet::load_or_generate(&path, 11).is_err());
    assert!(KeySet::generate_and_save(&path, 11).is_err());
    assert_eq!(fs::read(&path).unwrap(), raw);
    for (i, key) in keys.slice(10).iter().enumerate() {
        assert_eq!(*key, Sha256::hash(&[&(i as u64).to_be_bytes()]));
    }
}

#[test]
fn test_creation_locks_and_truncated_files() {
    let temp = TempDir::new("keyset_lock");
    let path = temp.path().join("keys.bin");
    let lock = common::artifact_io::CreationLock::acquire(&path).unwrap();
    assert!(KeySet::generate_and_save(&path, 10).is_err());
    assert!(!path.exists());
    drop(lock);
    KeySet::generate_and_save(&path, 10).unwrap();
    fs::write(&path, [0u8; 31]).unwrap();
    assert!(KeySet::load_from_file(&path).is_err());
}
