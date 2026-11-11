//! Offline translation cache: accepted commands keyed by the raw request text,
//! served into the confirmation flow when the provider is unreachable.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{atomic_write, ensure_config_dir};

const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;
const CACHE_FILE: &str = "translation-cache.json";

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CachedCommand>,
}

#[derive(Serialize, Deserialize)]
struct CachedCommand {
    command: String,
    created_at: u64,
}

/// The cache is best-effort on every path: an unresolvable config dir or an
/// unreadable file yields an empty cache, never a failed run.
fn cache_path() -> Option<PathBuf> {
    Some(ensure_config_dir().ok()?.join(CACHE_FILE))
}

/// Returns the cached command for `request`, pruning expired entries to disk.
pub fn lookup(request: &str) -> Option<String> {
    let path = cache_path()?;
    let cache = load(&path);
    cache.entries.get(request).map(|entry| entry.command.clone())
}

/// Records `command` under the key `request`; re-recording refreshes the entry.
pub fn record(request: &str, command: &str) {
    // An emptied edit buffer can reach acceptance; an empty command is never a
    // translation and must never be served offline.
    if command.trim().is_empty() {
        return;
    }
    let Some(path) = cache_path() else { return };
    let mut cache = load(&path);
    cache.entries.insert(request.to_string(), CachedCommand { command: command.to_string(), created_at: now_secs() });
    if let Ok(serialized) = serde_json::to_string(&cache) {
        let _ = atomic_write(&path, &serialized);
    }
}

/// Loads the cache, persisting the file when pruning dropped expired entries.
fn load(path: &Path) -> CacheFile {
    if let Ok(raw) = fs::read_to_string(path)
        && let Ok(mut cache) = serde_json::from_str::<CacheFile>(&raw)
    {
        let now = now_secs();
        let before = cache.entries.len();
        cache.entries.retain(|_, entry| now.saturating_sub(entry.created_at) <= MAX_AGE_SECS);
        if cache.entries.len() < before
            && let Ok(serialized) = serde_json::to_string(&cache)
        {
            let _ = atomic_write(path, &serialized);
        }
        return cache;
    }
    CacheFile { version: 1, entries: HashMap::new() }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_secs())
}
