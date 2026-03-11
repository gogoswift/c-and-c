use chrono::{Local, NaiveDate};
use regex::Regex;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const ACTIVE_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// 发现的 Codex 会话（内部中间类型）
pub struct DiscoveredCodexSession {
    pub session_id: String,
    pub file_path: String,
    pub last_modified: SystemTime,
}

/// 发现所有活跃的 Codex 会话（5 分钟内修改过）
///
/// 扫描 ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
/// 只看最近 2 天目录
pub fn discover_active_codex_sessions() -> Vec<DiscoveredCodexSession> {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".codex").join("sessions"),
        None => return vec![],
    };
    if !base.exists() {
        return vec![];
    }

    let now = SystemTime::now();
    let today = Local::now().date_naive();
    let yesterday = today - chrono::Duration::days(1);
    let date_dirs = vec![
        format_date_path(&base, &today),
        format_date_path(&base, &yesterday),
    ];

    // 从文件名提取 UUID: rollout-<timestamp>-<uuid>.jsonl
    let uuid_re = Regex::new(
        r"([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$",
    )
    .unwrap();

    let mut sessions = Vec::new();

    for dir in date_dirs {
        if !dir.exists() {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let metadata = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified = match metadata.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if now.duration_since(modified).unwrap_or(Duration::MAX) > ACTIVE_THRESHOLD {
                continue;
            }

            let session_id = uuid_re
                .captures(&name)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| name.trim_end_matches(".jsonl").to_string());

            sessions.push(DiscoveredCodexSession {
                session_id,
                file_path: path.to_string_lossy().to_string(),
                last_modified: modified,
            });
        }
    }
    sessions
}

fn format_date_path(base: &PathBuf, date: &NaiveDate) -> PathBuf {
    base.join(date.format("%Y").to_string())
        .join(date.format("%m").to_string())
        .join(date.format("%d").to_string())
}
