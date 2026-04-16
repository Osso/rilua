//! Instrumentation counters for `Table::rehash`, enabled by the
//! `rehash-stats` feature. Used to diagnose pre-sizing gaps: if this
//! counter is high during startup, a hot allocation site is hitting
//! the rehash path because its pre-size is too small.

use core::sync::atomic::{AtomicU64, Ordering};

pub static REHASH_COUNT: AtomicU64 = AtomicU64::new(0);
pub static REHASH_FROM_EMPTY: AtomicU64 = AtomicU64::new(0);
pub static REHASH_GROW: AtomicU64 = AtomicU64::new(0);
pub static REHASH_FRAME_BACKED: AtomicU64 = AtomicU64::new(0);
pub static REHASH_NONFRAME: AtomicU64 = AtomicU64::new(0);
/// Histogram by old hash size (power of 2) when new_hash_size == 0.
/// Helps distinguish "first hash entry" (old=0) from "array promotion" (old>0).
pub static REHASH_TO_ZERO_FROM: [AtomicU64; 16] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Histogram buckets by new hash size (power of 2): index i = size 2^i.
/// Index 0 captures resizes to size 0 (array-only tables).
pub static REHASH_NEW_SIZE_BUCKETS: [AtomicU64; 16] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Maps a power-of-2 size to its bucket index: `0 → 0`, `2^i → i`.
/// Clamped to `cap - 1` so out-of-range sizes land in the top bucket.
fn size_to_bucket(size: u32, cap: usize) -> usize {
    if size == 0 {
        0
    } else {
        size.trailing_zeros().min(cap as u32 - 1) as usize
    }
}

#[inline]
pub fn record(old_hash_size: u32, new_hash_size: u32, frame_backed: bool) {
    REHASH_COUNT.fetch_add(1, Ordering::Relaxed);
    if old_hash_size == 0 {
        REHASH_FROM_EMPTY.fetch_add(1, Ordering::Relaxed);
    } else if new_hash_size > old_hash_size {
        REHASH_GROW.fetch_add(1, Ordering::Relaxed);
    }
    if frame_backed {
        REHASH_FRAME_BACKED.fetch_add(1, Ordering::Relaxed);
    } else {
        REHASH_NONFRAME.fetch_add(1, Ordering::Relaxed);
    }
    let new_bucket = size_to_bucket(new_hash_size, REHASH_NEW_SIZE_BUCKETS.len());
    REHASH_NEW_SIZE_BUCKETS[new_bucket].fetch_add(1, Ordering::Relaxed);
    if new_hash_size == 0 {
        let from_bucket = size_to_bucket(old_hash_size, REHASH_TO_ZERO_FROM.len());
        REHASH_TO_ZERO_FROM[from_bucket].fetch_add(1, Ordering::Relaxed);
    }
}

/// Snapshot of rehash counters. Consumers print this at shutdown.
#[derive(Debug, Clone, Copy)]
pub struct RehashStats {
    pub total: u64,
    pub from_empty: u64,
    pub grow: u64,
    pub frame_backed: u64,
    pub nonframe: u64,
    pub by_new_size: [u64; 16],
    pub to_zero_from: [u64; 16],
}

pub fn snapshot() -> RehashStats {
    let mut by_new_size = [0u64; 16];
    for (i, b) in REHASH_NEW_SIZE_BUCKETS.iter().enumerate() {
        by_new_size[i] = b.load(Ordering::Relaxed);
    }
    let mut to_zero_from = [0u64; 16];
    for (i, b) in REHASH_TO_ZERO_FROM.iter().enumerate() {
        to_zero_from[i] = b.load(Ordering::Relaxed);
    }
    RehashStats {
        total: REHASH_COUNT.load(Ordering::Relaxed),
        from_empty: REHASH_FROM_EMPTY.load(Ordering::Relaxed),
        grow: REHASH_GROW.load(Ordering::Relaxed),
        frame_backed: REHASH_FRAME_BACKED.load(Ordering::Relaxed),
        nonframe: REHASH_NONFRAME.load(Ordering::Relaxed),
        by_new_size,
        to_zero_from,
    }
}

pub fn reset() {
    REHASH_COUNT.store(0, Ordering::Relaxed);
    REHASH_FROM_EMPTY.store(0, Ordering::Relaxed);
    REHASH_GROW.store(0, Ordering::Relaxed);
    REHASH_FRAME_BACKED.store(0, Ordering::Relaxed);
    REHASH_NONFRAME.store(0, Ordering::Relaxed);
    for b in &REHASH_NEW_SIZE_BUCKETS {
        b.store(0, Ordering::Relaxed);
    }
    for b in &REHASH_TO_ZERO_FROM {
        b.store(0, Ordering::Relaxed);
    }
}
