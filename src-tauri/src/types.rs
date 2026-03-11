use serde::{Deserialize, Serialize};

// ── Claude Code 用量 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserType {
    #[serde(rename = "not_logged_in")]
    NotLoggedIn,
    #[serde(rename = "api_user")]
    ApiUser,
    #[serde(rename = "subscriber")]
    Subscriber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageTier {
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeUsage {
    pub five_hour: UsageTier,
    pub seven_day: UsageTier,
    pub seven_day_opus: Option<UsageTier>,
    pub seven_day_sonnet: Option<UsageTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUserInfo {
    pub user_type: UserType,
    pub subscription_type: Option<String>,
    pub account_type: Option<String>,
    pub usage: Option<ClaudeUsage>,
    pub error: Option<String>,
}

// ── Codex 额度 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRateLimitTier {
    pub used_percent: f64,
    pub window_minutes: u32,
    pub resets_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRateLimits {
    pub primary: CodexRateLimitTier,
    pub secondary: CodexRateLimitTier,
    pub plan_type: Option<String>,
    pub source: String,
}

// ── CC 活跃会话 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSession {
    pub session_id: String,
    pub project_hash: String,
    pub file_path: String,
    pub last_modified: String,
    pub sub_agent_files: Vec<String>,
    pub title: Option<String>,
    pub slug: Option<String>,
    pub cwd: Option<String>,
}

// ── CX 活跃会话 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveCodexSession {
    pub session_id: String,
    pub file_path: String,
    pub last_modified: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub start_time: Option<String>,
    pub has_active_tool: bool,
    pub has_error: bool,
    pub tool_count: u32,
    pub total_tokens: u64,
}

// ── 磁盘缓存 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskCache {
    pub info: ClaudeUserInfo,
    pub fetched_at: u64,
    pub ttl: u64,
}
