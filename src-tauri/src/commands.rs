use crate::session_parser::ParsedSession;
use crate::types::*;
use crate::{claude_auth, codex_discovery, codex_usage, jsonl_parser, session_discovery, session_parser};
use tauri::{AppHandle, Manager};

/// 获取所有活跃的 Claude Code 会话
#[tauri::command]
pub async fn get_active_sessions() -> Vec<ActiveSession> {
    session_discovery::discover_active_sessions()
}

/// 获取所有活跃的 Codex 会话（含解析后的状态）
#[tauri::command]
pub async fn get_active_codex_sessions() -> Vec<ActiveCodexSession> {
    let discovered = codex_discovery::discover_active_codex_sessions();
    discovered
        .into_iter()
        .map(|s| jsonl_parser::parse_codex_session(&s.file_path, &s.session_id, &s.last_modified))
        .collect()
}

/// 获取 Claude 用户认证和用量信息
#[tauri::command]
pub async fn get_claude_user_info() -> ClaudeUserInfo {
    claude_auth::get_claude_user_info().await
}

/// 获取 Codex 额度信息
#[tauri::command]
pub async fn get_codex_usage() -> Option<CodexRateLimits> {
    codex_usage::get_codex_usage().await
}

/// 加载并解析 CC 会话 JSONL 文件（静默模式，失败返回 null）
#[tauri::command]
pub async fn load_session_silent(
    file_path: String,
    sub_agent_files: Option<Vec<String>>,
) -> Option<ParsedSession> {
    let subs = sub_agent_files.unwrap_or_default();
    if subs.is_empty() {
        session_parser::parse_session_file(&file_path)
    } else {
        session_parser::parse_session_with_sub_agents(&file_path, &subs)
    }
}

/// 隐藏主窗口
#[tauri::command]
pub async fn close_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}
