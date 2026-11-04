//! Shared storage and execution contract for Merkleization and saved seeds.
#![allow(dead_code)]
use super::{INIT_CACHE_SIZE, WRITE_BUFFER_SIZE};
use commonware_parallel::Rayon;
use commonware_runtime::{BufferPooler, Strategizer, buffer::paged::CacheRef};
use commonware_storage::{
    journal::contiguous::fixed::Config as JournalConfig,
    merkle::full::Config as MerkleConfig,
    qmdb::{any::FixedConfig as AnyConfig, current::FixedConfig as CurrentConfig},
    translator::EightCap,
};
use commonware_utils::{NZU16, NZU64, NZUsize};
use serde::{Deserialize, Serialize};
use std::num::{NonZeroU16, NonZeroU64, NonZeroUsize};
pub const PARTITION: &str = "bench-merkleize";
pub const ITEMS_PER_BLOB: NonZeroU64 = NZU64!(10_000_000);
pub const PAGE_SIZE: NonZeroU16 = NZU16!(4096);
pub const CACHE_PAGES: NonZeroUsize = NZUsize!(16_384);
pub const THREADS: NonZeroUsize = NZUsize!(8);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSettings {
    pub partition: String,
    pub items_per_blob: u64,
    pub merkle_family: String,
    pub key_bytes: usize,
    pub value_bytes: usize,
    pub translator: String,
    pub current_chunk_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSettings {
    pub page_bytes: u16,
    pub cache_pages: usize,
    pub write_buffer_bytes: usize,
    pub strategy_threads: usize,
    pub init_cache_size: usize,
    pub init_buffer_bytes: usize,
    pub init_concurrency: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub version: u32,
    pub storage: StorageSettings,
    pub execution: ExecutionSettings,
}
impl Descriptor {
    pub fn production() -> Self {
        Self::with_blob_capacity(ITEMS_PER_BLOB.get())
    }
    pub fn with_blob_capacity(items_per_blob: u64) -> Self {
        Self {
            version: 1,
            storage: StorageSettings {
                partition: PARTITION.into(),
                items_per_blob,
                merkle_family: "mmb".into(),
                key_bytes: 32,
                value_bytes: 32,
                translator: "EightCap".into(),
                current_chunk_bytes: super::CHUNK_SIZE,
            },
            execution: ExecutionSettings {
                page_bytes: PAGE_SIZE.get(),
                cache_pages: CACHE_PAGES.get(),
                write_buffer_bytes: WRITE_BUFFER_SIZE.get(),
                strategy_threads: THREADS.get(),
                init_cache_size: INIT_CACHE_SIZE.unwrap().get(),
                init_buffer_bytes: 1 << 21,
                init_concurrency: "sequential".into(),
            },
        }
    }
    pub fn fingerprint(&self) -> String {
        super::keys::checksum(&serde_json::to_vec(self).expect("descriptor encoding"))
    }
    pub fn validate(&self) -> std::io::Result<()> {
        if self.storage.items_per_blob == 0
            || *self != Self::with_blob_capacity(self.storage.items_per_blob)
        {
            return Err(super::artifact_io::invalid(
                "unsupported benchmark configuration",
            ));
        }
        Ok(())
    }
    pub fn cache(&self, ctx: &impl BufferPooler) -> CacheRef {
        CacheRef::from_pooler(
            ctx,
            NonZeroU16::new(self.execution.page_bytes).unwrap(),
            NonZeroUsize::new(self.execution.cache_pages).unwrap(),
        )
    }
    pub fn merkle(
        &self,
        ctx: &(impl BufferPooler + Strategizer),
        pc: CacheRef,
    ) -> MerkleConfig<Rayon> {
        MerkleConfig {
            journal_partition: format!("journal-{}", self.storage.partition),
            metadata_partition: format!("metadata-{}", self.storage.partition),
            items_per_blob: NonZeroU64::new(self.storage.items_per_blob).unwrap(),
            write_buffer: WRITE_BUFFER_SIZE,
            strategy: ctx.strategy(THREADS),
            page_cache: pc,
        }
    }
    pub fn journal(&self, pc: CacheRef) -> JournalConfig {
        JournalConfig {
            partition: format!("log-journal-{}", self.storage.partition),
            items_per_blob: NonZeroU64::new(self.storage.items_per_blob).unwrap(),
            page_cache: pc,
            write_buffer: WRITE_BUFFER_SIZE,
        }
    }
    pub fn any(
        &self,
        ctx: &(impl BufferPooler + Strategizer),
        pc: CacheRef,
    ) -> AnyConfig<EightCap, Rayon> {
        AnyConfig {
            merkle_config: self.merkle(ctx, pc.clone()),
            journal_config: self.journal(pc),
            translator: EightCap,
            init_cache_size: INIT_CACHE_SIZE,
            init_buffer: NZUsize!(1 << 21),
            init_concurrency: (),
        }
    }
    pub fn current(
        &self,
        ctx: &(impl BufferPooler + Strategizer),
        pc: CacheRef,
    ) -> CurrentConfig<EightCap, Rayon> {
        CurrentConfig {
            merkle_config: self.merkle(ctx, pc.clone()),
            journal_config: self.journal(pc),
            grafted_metadata_partition: format!("grafted-metadata-{}", self.storage.partition),
            translator: EightCap,
            init_cache_size: INIT_CACHE_SIZE,
            init_buffer: NZUsize!(1 << 21),
            init_concurrency: (),
        }
    }
}
