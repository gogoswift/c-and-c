use crate::types::ActiveCodexSession;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{SystemTime, UNIX_EPOCH};

const HEAD_BYTES: usize = 50 * 1024;
const TAIL_BYTES: usize = 80 * 1024;

/// 轻量级解析 Codex rollout JSONL，提取 MiniMonitor 所需状态
///
/// Codex rollout JSONL 格式：
///   line 1: {"type":"session_meta","payload":{"id":"<uuid>","cwd":"...","timestamp":"..."}}
///   line N: {"type":"response_item","payload":{"type":"function_call","call_id":"call_xxx",...}}
///   line N: {"type":"response_item","payload":{"type":"function_call_output","call_id":"call_xxx",...}}
///   line N: {"type":"event_msg","payload":{"type":"task_started",...}}
///   line N: {"type":"event_msg","payload":{"type":"task_complete",...}}
///   line N: {"type":"event_msg","payload":{"type":"user_message","message":"..."}}
///   line N: {"type":"event_msg","payload":{"type":"token_count","info":{...}}}
pub fn parse_codex_session(
    file_path: &str,
    session_id: &str,
    modified: &SystemTime,
) -> ActiveCodexSession {
    let mut cwd = None;
    let mut title = None;
    let mut start_time = None;
    let mut has_active_tool = false;
    let mut has_error = false;
    let mut tool_count: u32 = 0;
    let mut total_tokens: u64 = 0;

    // ── 读头部：提取 cwd、startTime、title ──
    if let Ok(head) = read_chunk(file_path, 0, HEAD_BYTES) {
        for line in head.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                // session_meta: 第一行，包含 cwd 和 timestamp
                if msg_type == "session_meta" {
                    if let Some(payload) = val.get("payload") {
                        if cwd.is_none() {
                            cwd = payload
                                .get("cwd")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                        if start_time.is_none() {
                            start_time = payload
                                .get("timestamp")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                }

                // 第一条 user_message 作为 title
                if title.is_none() && msg_type == "event_msg" {
                    if let Some(payload) = val.get("payload") {
                        if payload.get("type").and_then(|v| v.as_str()) == Some("user_message") {
                            if let Some(msg) = payload.get("message").and_then(|v| v.as_str()) {
                                let clean: String =
                                    msg.split_whitespace().collect::<Vec<_>>().join(" ");
                                if !clean.is_empty() {
                                    let chars: Vec<char> = clean.chars().collect();
                                    title = Some(if chars.len() > 80 {
                                        let truncated: String = chars[..77].iter().collect();
                                        format!("{}...", truncated)
                                    } else {
                                        clean
                                    });
                                }
                            }
                        }
                    }
                }

                // startTime 兜底：用第一行的 timestamp
                if start_time.is_none() {
                    if let Some(ts) = val.get("timestamp").and_then(|v| v.as_str()) {
                        start_time = Some(ts.to_string());
                    }
                }

                if cwd.is_some() && title.is_some() && start_time.is_some() {
                    break;
                }
            }
        }
    }

    // ── 读尾部：提取活跃工具状态、错误、token ──
    if let Ok(file_len) = std::fs::metadata(file_path).map(|m| m.len() as usize) {
        if file_len > 0 {
            let offset = if file_len > TAIL_BYTES {
                file_len - TAIL_BYTES
            } else {
                0
            };
            if let Ok(tail) = read_chunk(file_path, offset, TAIL_BYTES) {
                let mut pending_calls: HashSet<String> = HashSet::new();
                let mut last_tool_error = false;

                for line in tail.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");

                        match msg_type {
                            "response_item" => {
                                if let Some(payload) = val.get("payload") {
                                    let p_type = payload
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");

                                    if p_type == "function_call" {
                                        if let Some(call_id) =
                                            payload.get("call_id").and_then(|v| v.as_str())
                                        {
                                            pending_calls.insert(call_id.to_string());
                                        }
                                        tool_count += 1;
                                    }

                                    if p_type == "function_call_output" {
                                        if let Some(call_id) =
                                            payload.get("call_id").and_then(|v| v.as_str())
                                        {
                                            pending_calls.remove(call_id);
                                        }
                                        // 检测错误：output 包含非零退出码
                                        if let Some(output) =
                                            payload.get("output").and_then(|v| v.as_str())
                                        {
                                            if let Some(pos) =
                                                output.find("Process exited with code ")
                                            {
                                                let after = &output
                                                    [pos + "Process exited with code ".len()..];
                                                let code: String = after
                                                    .chars()
                                                    .take_while(|c| c.is_ascii_digit())
                                                    .collect();
                                                last_tool_error = code != "0" && !code.is_empty();
                                            } else {
                                                last_tool_error = false;
                                            }
                                        }
                                    }
                                }
                            }
                            "event_msg" => {
                                if let Some(payload) = val.get("payload") {
                                    let p_type = payload
                                        .get("type")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");

                                    match p_type {
                                        "task_complete" => {
                                            pending_calls.clear();
                                        }
                                        "task_started" => {
                                            pending_calls.clear();
                                            last_tool_error = false;
                                        }
                                        "token_count" => {
                                            if let Some(t) = payload
                                                .get("info")
                                                .and_then(|i| i.get("total_token_usage"))
                                                .and_then(|u| u.get("total_tokens"))
                                                .and_then(|v| v.as_u64())
                                            {
                                                total_tokens = t;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                has_active_tool = !pending_calls.is_empty();
                has_error = last_tool_error;
            }
        }
    }

    let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    let dt = chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0).unwrap_or_default();

    ActiveCodexSession {
        session_id: session_id.to_string(),
        file_path: file_path.to_string(),
        last_modified: dt.to_rfc3339(),
        cwd,
        title,
        start_time,
        has_active_tool,
        has_error,
        tool_count,
        total_tokens,
    }
}

fn read_chunk(path: &str, offset: usize, max_bytes: usize) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset as u64))?;
    }
    let mut buf = vec![0u8; max_bytes];
    let n = file.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf[..n]).to_string())
}
