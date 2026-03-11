use crate::types::ActiveSession;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ACTIVE_THRESHOLD: Duration = Duration::from_secs(5 * 60);
const HEAD_BYTES: usize = 200 * 1024;
const TAIL_BYTES: usize = 8 * 1024;

/// 发现所有活跃的 Claude Code 会话
///
/// 扫描 ~/.claude/projects/<projectHash>/<sessionId>.jsonl
/// 过滤 5 分钟内修改的文件
pub fn discover_active_sessions() -> Vec<ActiveSession> {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("projects"),
        None => return vec![],
    };
    if !base.exists() {
        return vec![];
    }

    let now = SystemTime::now();
    let mut sessions = Vec::new();

    // 遍历 projectHash 目录
    let project_dirs = match fs::read_dir(&base) {
        Ok(rd) => rd,
        Err(_) => return vec![],
    };

    for project_entry in project_dirs.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_hash = project_entry.file_name().to_string_lossy().to_string();

        let entries = match fs::read_dir(&project_path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".jsonl") || !path.is_file() {
                continue;
            }

            // 检查修改时间
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

            let session_id = name.trim_end_matches(".jsonl").to_string();
            let last_modified = system_time_to_iso(&modified);

            // 发现子代理文件
            let sub_agent_files = discover_sub_agents(&project_path, &session_id);

            // 读取元信息
            let (title, cwd) = read_head_meta(&path);
            let slug = read_slug_from_tail(&path);

            sessions.push(ActiveSession {
                session_id,
                project_hash: project_hash.clone(),
                file_path: path.to_string_lossy().to_string(),
                last_modified,
                sub_agent_files,
                title,
                slug,
                cwd,
            });
        }
    }

    // 按修改时间降序排序
    sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    sessions
}

/// 发现 <sessionId>/subagents/agent-*.jsonl 子代理文件
fn discover_sub_agents(project_path: &PathBuf, session_id: &str) -> Vec<String> {
    let sub_dir = project_path.join(session_id).join("subagents");
    if !sub_dir.exists() {
        return vec![];
    }
    match fs::read_dir(&sub_dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("agent-") && name.ends_with(".jsonl")
            })
            .map(|e| e.path().to_string_lossy().to_string())
            .collect(),
        Err(_) => vec![],
    }
}

/// 从 JSONL 文件头部读取 cwd 和第一条用户消息文本。
/// 限制读取前 200KB，避免大文件卡顿。
fn read_head_meta(path: &PathBuf) -> (Option<String>, Option<String>) {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, None),
    };
    let mut buf = vec![0u8; HEAD_BYTES];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return (None, None),
    };
    let content = String::from_utf8_lossy(&buf[..n]);

    let mut title = None;
    let mut cwd = None;

    for line in content.lines() {
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            // 提取 cwd
            if cwd.is_none() {
                if let Some(c) = val.get("cwd").and_then(|v| v.as_str()) {
                    cwd = Some(c.to_string());
                }
            }
            // 提取 title：第一条 user 消息
            if title.is_none() {
                let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if msg_type == "human" || msg_type == "user" {
                    title = extract_user_text(&val);
                }
            }
            if title.is_some() && cwd.is_some() {
                break;
            }
        }
    }

    (title, cwd)
}

/// 从 user 记录的 message 中提取纯文本内容（跳过 IDE 自动生成的标签）。
/// 截取前 80 个字符作为标题。
fn extract_user_text(val: &serde_json::Value) -> Option<String> {
    let message = val.get("message")?;

    let raw = if let Some(s) = message.as_str() {
        s.to_string()
    } else if let Some(content) = message.get("content") {
        if let Some(s) = content.as_str() {
            s.to_string()
        } else if let Some(arr) = content.as_array() {
            let mut text = String::new();
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        let trimmed = t.trim();
                        if trimmed.starts_with('<') {
                            continue;
                        }
                        text = trimmed.to_string();
                        break;
                    }
                }
            }
            text
        } else {
            return None;
        }
    } else {
        return None;
    };

    if raw.is_empty() {
        return None;
    }

    // 合并空白
    let clean: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return None;
    }

    let chars: Vec<char> = clean.chars().collect();
    if chars.len() > 80 {
        let truncated: String = chars[..77].iter().collect();
        Some(format!("{}...", truncated))
    } else {
        Some(clean)
    }
}

/// 从 JSONL 文件尾部读取 slug。
/// 读取文件末尾 8KB 数据，解析最后几行 JSON。
fn read_slug_from_tail(path: &PathBuf) -> Option<String> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return None,
    };
    let file_len = match file.metadata() {
        Ok(m) => m.len() as usize,
        Err(_) => return None,
    };

    if file_len == 0 {
        return None;
    }

    let offset = if file_len > TAIL_BYTES {
        file_len - TAIL_BYTES
    } else {
        0
    };
    if file.seek(SeekFrom::Start(offset as u64)).is_err() {
        return None;
    }

    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return None;
    }
    let content = String::from_utf8_lossy(&buf);

    // 从后向前找 slug
    for line in content.lines().rev() {
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(slug) = val.get("slug").and_then(|v| v.as_str()) {
                return Some(slug.to_string());
            }
        }
    }
    None
}

fn system_time_to_iso(time: &SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let dt = chrono::DateTime::from_timestamp(secs as i64, 0).unwrap_or_default();
    dt.to_rfc3339()
}
