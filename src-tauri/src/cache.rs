use crate::types::{ClaudeUserInfo, DiskCache};
use std::fs;
use std::path::PathBuf;

fn cache_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("cc-usage-cache.json")
}

/// 从磁盘读取缓存
pub fn load_disk_cache() -> Option<DiskCache> {
    let path = cache_file_path();
    let raw = fs::read_to_string(&path).ok()?;
    let data: DiskCache = serde_json::from_str(&raw).ok()?;
    if data.fetched_at > 0 && data.ttl > 0 {
        Some(data)
    } else {
        None
    }
}

/// 写入磁盘缓存
pub fn save_disk_cache(info: &ClaudeUserInfo, fetched_at: u64, ttl: u64) {
    let path = cache_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let cache = DiskCache {
        info: info.clone(),
        fetched_at,
        ttl,
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = fs::write(&path, json);
    }
}
