use crate::types::{CodexRateLimitTier, CodexRateLimits};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_TTL: u64 = 5 * 60 * 1000; // 5 分钟
const USAGE_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const TAIL_BYTES: usize = 80 * 1024;

struct CacheState {
    data: Option<CodexRateLimits>,
    ts: u64,
}

static CACHE: std::sync::LazyLock<Mutex<CacheState>> =
    std::sync::LazyLock::new(|| Mutex::new(CacheState { data: None, ts: 0 }));

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 获取 Codex 最新额度信息（带 5 分钟缓存）
///
/// 策略：API 优先 -> JSONL 回退
pub async fn get_codex_usage() -> Option<CodexRateLimits> {
    let now = now_ms();
    {
        let cache = CACHE.lock().unwrap();
        if cache.data.is_some() && (now - cache.ts) < CACHE_TTL {
            return cache.data.clone();
        }
    }

    log::info!("[codex_usage] fetching usage...");

    // 通道 1：ChatGPT Usage API
    let mut result = None;
    match fetch_from_api().await {
        Ok(Some(r)) => {
            log::info!(
                "[codex_usage] API success: primary={}%, secondary={}%",
                r.primary.used_percent,
                r.secondary.used_percent
            );
            result = Some(r);
        }
        Ok(None) => {
            log::info!("[codex_usage] API returned no rate_limit data");
        }
        Err(e) => {
            log::warn!("[codex_usage] API failed: {}", e);
        }
    }

    // 通道 2：JSONL 扫描回退
    if result.is_none() {
        match scan_jsonl_rate_limits() {
            Some(r) => {
                log::info!(
                    "[codex_usage] JSONL fallback success: primary={}%, secondary={}%",
                    r.primary.used_percent,
                    r.secondary.used_percent
                );
                result = Some(r);
            }
            None => {
                log::info!("[codex_usage] JSONL also has no data");
            }
        }
    }

    {
        let mut cache = CACHE.lock().unwrap();
        cache.data = result.clone();
        cache.ts = now_ms();
    }

    result
}

// =============================================================================
// 通道 1：ChatGPT Usage API
// =============================================================================

/// 从 ~/.codex/auth.json 读取 access_token
fn read_access_token() -> Option<String> {
    let path = dirs::home_dir()?.join(".codex").join("auth.json");
    let raw = fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&raw).ok()?;
    val.get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 用 access_token 调用 ChatGPT Usage API 获取实时额度
async fn fetch_from_api() -> Result<Option<CodexRateLimits>, String> {
    let token = read_access_token().ok_or_else(|| "no access_token in auth.json".to_string())?;
    let client = reqwest::Client::new();
    let resp = client
        .get(USAGE_API_URL)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "*/*")
        .header("Referer", "https://chatgpt.com/")
        .header("Origin", "https://chatgpt.com")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("API {}: {}", resp.status(), resp.status()));
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("json parse failed: {}", e))?;

    Ok(parse_api_response(&data))
}

/// 解析 ChatGPT Usage API 响应
///
/// 响应格式：
/// {
///   "rate_limit": {
///     "primary_window": {
///       "used_percent": 1.0,
///       "reset_at": 1772822743 | "2026-03-07T02:45:43Z",
///       "reset_after_seconds": 18000,
///       "limit_window_seconds": 18000
///     },
///     "secondary_window": { ... }
///   }
/// }
fn parse_api_response(data: &serde_json::Value) -> Option<CodexRateLimits> {
    let rate = data.get("rate_limit")?;
    let pw = rate.get("primary_window");
    let sw = rate.get("secondary_window");

    if pw.is_none() && sw.is_none() {
        return None;
    }

    Some(CodexRateLimits {
        primary: parse_window_tier(pw, 300),
        secondary: parse_window_tier(sw, 10080),
        plan_type: data
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        source: "api".to_string(),
    })
}

/// 解析单个 window 对象
/// reset_at 可能是 Unix 秒时间戳或 ISO 字符串
fn parse_window_tier(win: Option<&serde_json::Value>, default_window_minutes: u32) -> CodexRateLimitTier {
    let win = match win {
        Some(w) => w,
        None => {
            return CodexRateLimitTier {
                used_percent: 0.0,
                window_minutes: default_window_minutes,
                resets_at: 0,
            }
        }
    };

    // reset_at: 可能是数字（Unix 秒）或字符串（ISO 8601）
    let resets_at = if let Some(n) = win.get("reset_at").and_then(|v| v.as_u64()) {
        n
    } else if let Some(s) = win.get("reset_at").and_then(|v| v.as_str()) {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.timestamp() as u64)
            .unwrap_or(0)
    } else {
        0
    };

    // window_minutes: 从 limit_window_seconds 计算，或用默认值
    let window_minutes = win
        .get("limit_window_seconds")
        .and_then(|v| v.as_u64())
        .map(|secs| (secs / 60) as u32)
        .unwrap_or(default_window_minutes);

    CodexRateLimitTier {
        used_percent: win
            .get("used_percent")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        window_minutes,
        resets_at,
    }
}

// =============================================================================
// 通道 2：本地 JSONL 扫描（回退）
// =============================================================================

/// 扫描所有 Codex session 文件，找到最新的非 null rate_limits
fn scan_jsonl_rate_limits() -> Option<CodexRateLimits> {
    let base = dirs::home_dir()?.join(".codex").join("sessions");
    if !base.exists() {
        return None;
    }

    // 收集最近的 JSONL 文件，按 mtime 降序
    let mut files: Vec<(std::path::PathBuf, SystemTime)> = Vec::new();
    collect_jsonl_files(&base, &mut files, 0, 3);
    files.sort_by(|a, b| b.1.cmp(&a.1));

    for (path, _) in files.iter().take(10) {
        if let Some(limits) = extract_rate_limits_from_tail(path) {
            return Some(limits);
        }
    }
    None
}

/// 递归收集 sessions 目录下所有 .jsonl 文件
fn collect_jsonl_files(
    dir: &std::path::Path,
    files: &mut Vec<(std::path::PathBuf, SystemTime)>,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files, depth + 1, max_depth);
        } else if path.extension().map_or(false, |e| e == "jsonl") {
            if let Ok(meta) = fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    files.push((path, modified));
                }
            }
        }
    }
}

/// 从文件尾部提取最新的非 null rate_limits
fn extract_rate_limits_from_tail(path: &std::path::Path) -> Option<CodexRateLimits> {
    let mut file = fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len() as usize;
    if file_len == 0 {
        return None;
    }

    let offset = if file_len > TAIL_BYTES {
        file_len - TAIL_BYTES
    } else {
        0
    };
    file.seek(SeekFrom::Start(offset as u64)).ok()?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let content = String::from_utf8_lossy(&buf);

    // 从后向前搜索含 limit_id 的行
    for line in content.lines().rev() {
        let line = line.trim();
        if line.is_empty() || !line.contains("\"limit_id\"") {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(rl) = val.get("payload").and_then(|p| p.get("rate_limits")) {
                if rl.get("limit_id").is_some()
                    && rl.get("primary").is_some()
                    && rl.get("secondary").is_some()
                {
                    let parse = |w: &serde_json::Value| -> CodexRateLimitTier {
                        CodexRateLimitTier {
                            used_percent: w
                                .get("used_percent")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                            window_minutes: w
                                .get("window_minutes")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(300)
                                as u32,
                            resets_at: w.get("resets_at").and_then(|v| v.as_u64()).unwrap_or(0),
                        }
                    };
                    return Some(CodexRateLimits {
                        primary: parse(&rl["primary"]),
                        secondary: parse(&rl["secondary"]),
                        plan_type: rl
                            .get("plan_type")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        source: "jsonl".to_string(),
                    });
                }
            }
        }
    }
    None
}
