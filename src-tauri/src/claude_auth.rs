use crate::cache;
use crate::types::{ClaudeUsage, ClaudeUserInfo, UsageTier, UserType};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
const CACHE_TTL: u64 = 10 * 60 * 1000; // 10 分钟
const BACKOFF_TTL: u64 = 30 * 60 * 1000; // 30 分钟
const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_REFRESH_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const OAUTH_CLIENT_ID: &str = "5f84f21a-4a56-40de-9626-87e60b3b817a";

struct AuthState {
    cached_info: Option<ClaudeUserInfo>,
    last_fetch: u64,
    current_ttl: u64,
}

static AUTH_STATE: std::sync::LazyLock<Mutex<AuthState>> = std::sync::LazyLock::new(|| {
    Mutex::new(AuthState {
        cached_info: None,
        last_fetch: 0,
        current_ttl: CACHE_TTL,
    })
});

/// inflight 去重：避免并发请求同时调 API 触发 429
static INFLIGHT: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 获取 Claude 用户信息（带三层缓存）
///
/// 缓存策略：
/// 1. 内存缓存（10分钟/30分钟退避）
/// 2. 磁盘缓存（进程重启后恢复）
/// 3. 实时查询（Keychain + OAuth API 或 CLI fallback）
pub async fn get_claude_user_info() -> ClaudeUserInfo {
    let now = now_ms();

    // 层 1：内存缓存命中
    {
        let state = AUTH_STATE.lock().unwrap();
        if let Some(ref info) = state.cached_info {
            if (now - state.last_fetch) < state.current_ttl {
                return info.clone();
            }
        }
    }

    // 层 2：首次调用 - 尝试从磁盘恢复缓存，避免立即调 API
    {
        let state = AUTH_STATE.lock().unwrap();
        if state.cached_info.is_none() {
            if let Some(disk) = cache::load_disk_cache() {
                if (now - disk.fetched_at) < disk.ttl {
                    let info = disk.info.clone();
                    drop(state);
                    let mut state = AUTH_STATE.lock().unwrap();
                    state.cached_info = Some(info.clone());
                    state.last_fetch = disk.fetched_at;
                    state.current_ttl = disk.ttl;
                    eprintln!("[claude_auth] restored from disk cache (ttl remaining: {}s)", (disk.ttl - (now - disk.fetched_at)) / 1000);
                    return info;
                }
            }
        }
    }

    // 层 3：调 API（加锁去重，避免并发请求触发 429）
    let _guard = INFLIGHT.lock().await;

    // 拿到锁后再检查一次缓存（可能另一个请求刚刚更新了）
    {
        let state = AUTH_STATE.lock().unwrap();
        if let Some(ref info) = state.cached_info {
            if (now_ms() - state.last_fetch) < state.current_ttl {
                return info.clone();
            }
        }
    }

    let info = fetch_claude_user_info().await;
    let now = now_ms();
    {
        let mut state = AUTH_STATE.lock().unwrap();
        state.cached_info = Some(info.clone());
        state.last_fetch = now;
        cache::save_disk_cache(&info, now, state.current_ttl);
    }
    info
}

/// 实际获取 Claude 用户信息
///
/// 优先路径：Keychain -> Usage API（快速、精确）
/// Fallback: claude auth status CLI
///
/// 注意：不自行刷新 token，依赖 Claude Code 自身维护 Keychain 中的凭据。
async fn fetch_claude_user_info() -> ClaudeUserInfo {
    // 1. 尝试 Keychain → Usage API
    if let Some(cred) = get_keychain_credential() {
        eprintln!(
            "[claude_auth] keychain OK: subscriptionType={:?}",
            cred.subscription_type
        );
        let sub_type = cred
            .subscription_type
            .as_deref()
            .map(|s| s.to_lowercase());

        // 只有明确是 free 用户才跳过 API；subscriptionType 为 None 也尝试
        let is_free = sub_type.as_deref() == Some("free");
        if !is_free {
            let mut token = cred.access_token.clone();
            let is_expired = cred.expires_at.map_or(false, |e| now_ms() > e);

            // token 过期 → 先刷新
            if is_expired {
                eprintln!("[claude_auth] token expired, attempting refresh...");
                if let Some(rt) = &cred.refresh_token {
                    if let Some(new_token) = refresh_access_token(rt).await {
                        token = new_token;
                    }
                }
            }

            eprintln!("[claude_auth] calling usage API...");
            let usage = fetch_usage_api(&token).await;

            // 401 → 再尝试一次 refresh
            let usage = if usage.is_none() && !is_expired {
                if let Some(rt) = &cred.refresh_token {
                    eprintln!("[claude_auth] 401, attempting refresh...");
                    if let Some(new_token) = refresh_access_token(rt).await {
                        token = new_token;
                        fetch_usage_api(&token).await
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                usage
            };

            eprintln!("[claude_auth] usage API result: has_data={}", usage.is_some());
            let ttl = if usage.is_some() {
                CACHE_TTL
            } else {
                BACKOFF_TTL
            };
            {
                let mut state = AUTH_STATE.lock().unwrap();
                state.current_ttl = ttl;
            }
            // API 失败时保留旧数据
            let stale_usage = AUTH_STATE
                .lock()
                .unwrap()
                .cached_info
                .as_ref()
                .and_then(|i| i.usage.clone());

            let api_ok = usage.is_some();
            return ClaudeUserInfo {
                user_type: UserType::Subscriber,
                subscription_type: sub_type.clone(),
                account_type: sub_type,
                usage: usage.or(stale_usage),
                error: if api_ok { None } else { Some("usage_api_failed".to_string()) },
            };
        }
    }

    // Keychain 失败
    eprintln!("[claude_auth] keychain not found, trying CLI fallback...");

    // 2. Fallback: claude auth status CLI
    let auth = get_auth_status_cli();
    eprintln!("[claude_auth] CLI: logged_in={}, subscriptionType={:?}, apiProvider={:?}",
        auth.logged_in, auth.subscription_type, auth.api_provider);

    if !auth.logged_in {
        return ClaudeUserInfo {
            user_type: UserType::NotLoggedIn,
            subscription_type: None,
            account_type: None,
            usage: None,
            error: None,
        };
    }

    let sub_type = auth.subscription_type.map(|s| s.to_lowercase());
    let is_subscriber = sub_type.as_ref().map_or(false, |s| s != "free")
        || auth.api_provider.as_deref() == Some("firstParty");

    if !is_subscriber {
        return ClaudeUserInfo {
            user_type: UserType::ApiUser,
            subscription_type: sub_type.clone(),
            account_type: sub_type,
            usage: None,
            error: None,
        };
    }

    // 订阅用户但没有 keychain token（非 macOS 或 token 过期），无法调 API
    ClaudeUserInfo {
        user_type: UserType::Subscriber,
        subscription_type: sub_type.clone(),
        account_type: sub_type,
        usage: None,
        error: Some("no_keychain_token".to_string()),
    }
}

// ── Keychain 凭据读取 ──

struct KeychainCredential {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    subscription_type: Option<String>,
}

/// 从 macOS Keychain 获取 OAuth 凭据
fn get_keychain_credential() -> Option<KeychainCredential> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let oauth = parsed.get("claudeAiOauth")?;

    Some(KeychainCredential {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        refresh_token: oauth.get("refreshToken").and_then(|v| v.as_str()).map(|s| s.to_string()),
        expires_at: oauth.get("expiresAt").and_then(|v| v.as_u64()),
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

// ── OAuth Usage API ──

/// 调用 Anthropic OAuth Usage API 获取精确用量
async fn fetch_usage_api(access_token: &str) -> Option<ClaudeUsage> {
    let client = reqwest::Client::new();
    let resp = client
        .get(USAGE_API_URL)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        eprintln!("[claude_auth] usage API returned status: {}", resp.status());
        return None;
    }

    let data: serde_json::Value = resp.json().await.ok()?;
    eprintln!("[claude_auth] usage API response: {}", data);

    let parse_tier = |tier: &serde_json::Value| -> UsageTier {
        UsageTier {
            utilization: tier
                .get("utilization")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            resets_at: tier
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        }
    };

    Some(ClaudeUsage {
        five_hour: parse_tier(&data["five_hour"]),
        seven_day: parse_tier(&data["seven_day"]),
        seven_day_opus: data.get("seven_day_opus").map(|t| parse_tier(t)),
        seven_day_sonnet: data.get("seven_day_sonnet").map(|t| parse_tier(t)),
    })
}

// ── Token Refresh ──

/// 用 refresh_token 获取新的 access_token，成功后写回 Keychain
async fn refresh_access_token(refresh_token: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_REFRESH_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        eprintln!("[claude_auth] refresh failed: {} body={}", status, body);
        return None;
    }

    let data: serde_json::Value = resp.json().await.ok()?;
    let new_access = data.get("access_token")?.as_str()?;
    let new_refresh = data.get("refresh_token").and_then(|v| v.as_str());
    let expires_in = data.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let new_expires_at = now_ms() + expires_in * 1000;

    eprintln!("[claude_auth] refresh success, expires_in={}s", expires_in);

    // 写回 Keychain
    update_keychain_token(new_access, new_refresh, new_expires_at);

    Some(new_access.to_string())
}

/// 更新 Keychain 中的 OAuth token
fn update_keychain_token(access_token: &str, refresh_token: Option<&str>, expires_at: u64) {
    // 读取现有 keychain 数据
    let output = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output();

    let mut parsed: serde_json::Value = match output {
        Ok(ref o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            serde_json::from_str(stdout.trim()).unwrap_or_else(|_| serde_json::json!({}))
        }
        _ => serde_json::json!({}),
    };

    // 更新 OAuth 字段
    if let Some(oauth) = parsed.get_mut("claudeAiOauth") {
        oauth["accessToken"] = serde_json::json!(access_token);
        oauth["expiresAt"] = serde_json::json!(expires_at);
        if let Some(rt) = refresh_token {
            oauth["refreshToken"] = serde_json::json!(rt);
        }
    }

    let json_str = serde_json::to_string(&parsed).unwrap_or_default();

    // 先删再加（macOS security 不支持直接更新）
    let _ = Command::new("security")
        .args(["delete-generic-password", "-s", "Claude Code-credentials"])
        .output();

    let result = Command::new("security")
        .args([
            "add-generic-password",
            "-s", "Claude Code-credentials",
            "-a", "",
            "-w", &json_str,
            "-U",
        ])
        .output();

    match result {
        Ok(o) if o.status.success() => {
            eprintln!("[claude_auth] keychain updated successfully");
        }
        Ok(o) => {
            eprintln!("[claude_auth] keychain update failed: {}", String::from_utf8_lossy(&o.stderr));
        }
        Err(e) => {
            eprintln!("[claude_auth] keychain update error: {}", e);
        }
    }
}

// ── CLI Fallback ──

struct CliAuthStatus {
    logged_in: bool,
    subscription_type: Option<String>,
    api_provider: Option<String>,
}

/// Fallback: 调用 claude auth status CLI
fn get_auth_status_cli() -> CliAuthStatus {
    let home = dirs::home_dir().unwrap_or_default();
    let extended_path = format!(
        "{}:/usr/local/bin:/opt/homebrew/bin:{}/.local/bin:{}/.claude/local",
        std::env::var("PATH").unwrap_or_default(),
        home.display(),
        home.display()
    );

    let output = Command::new("claude")
        .args(["auth", "status"])
        .env("PATH", &extended_path)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
                return CliAuthStatus {
                    logged_in: val
                        .get("loggedIn")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    subscription_type: val
                        .get("subscriptionType")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    api_provider: val
                        .get("apiProvider")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
            }
            CliAuthStatus {
                logged_in: false,
                subscription_type: None,
                api_provider: None,
            }
        }
        _ => CliAuthStatus {
            logged_in: false,
            subscription_type: None,
            api_provider: None,
        },
    }
}
