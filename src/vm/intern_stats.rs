//! Instrumentation for `intern_string`, enabled by the `intern-stats`
//! feature. Records call counts per string so we can rank hot interning
//! sites and decide which deserve migration to `intern_string_static`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Per-content call counter. Keyed by the interned bytes (small strings
/// dominate; storing the content is cheap at the volumes we see).
static COUNTS: OnceLock<Mutex<HashMap<Vec<u8>, u64>>> = OnceLock::new();

fn counts() -> &'static Mutex<HashMap<Vec<u8>, u64>> {
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn record(data: &[u8]) {
    if let Ok(mut map) = counts().lock() {
        *map.entry(data.to_vec()).or_insert(0) += 1;
    }
}

/// Snapshot: `(content, count)` sorted by count descending.
pub fn snapshot_top(n: usize) -> Vec<(Vec<u8>, u64)> {
    let Ok(map) = counts().lock() else {
        return Vec::new();
    };
    let mut entries: Vec<(Vec<u8>, u64)> =
        map.iter().map(|(k, &v)| (k.clone(), v)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(n);
    entries
}

pub fn total_calls() -> u64 {
    counts()
        .lock()
        .map(|m| m.values().sum())
        .unwrap_or(0)
}

pub fn unique_strings() -> usize {
    counts().lock().map(|m| m.len()).unwrap_or(0)
}

pub fn reset() {
    if let Ok(mut map) = counts().lock() {
        map.clear();
    }
}
