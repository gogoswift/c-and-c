/// CC JSONL 会话解析器
///
/// 解析 Claude Code 的 .jsonl 会话文件，提取：
/// - turns → toolCalls（配对 tool_use ↔ tool_result）
/// - tokenUsage.estimatedCost（基于 input/output tokens）
/// - subAgents（递归解析子代理文件）
///
/// 仅实现 MiniMonitor 需要的最小数据集

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

/// 前端 Session 类型对应
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ParsedSession {
    pub turns: Vec<ParsedTurn>,
    pub sub_agents: Vec<SubAgentRef>,
    pub token_usage: TokenUsage,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ParsedTurn {
    pub tool_calls: Vec<ParsedToolCall>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ParsedToolCall {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub is_error: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentRef {
    pub session: Option<ParsedSession>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub estimated_cost: f64,
}

/// 内部构建用的 pending tool call
struct PendingToolCall {
    turn_idx: usize,
    tool_idx: usize,
}

/// 解析单个 JSONL 会话文件
///
/// 读取整个文件，逐行解析 JSON，构建 Session
pub fn parse_session_file(file_path: &str) -> Option<ParsedSession> {
    let content = read_file_content(file_path)?;
    let session = build_session(&content);
    Some(session)
}

/// 解析会话文件并关联子代理
pub fn parse_session_with_sub_agents(
    file_path: &str,
    sub_agent_files: &[String],
) -> Option<ParsedSession> {
    let mut session = parse_session_file(file_path)?;

    // 递归解析每个子代理文件
    for sa_path in sub_agent_files {
        let sub_session = parse_session_file(sa_path);
        session.sub_agents.push(SubAgentRef {
            session: sub_session,
        });
    }

    Some(session)
}

/// 读取文件内容（大文件限制：最多读取头部 2MB + 尾部 512KB）
fn read_file_content(file_path: &str) -> Option<String> {
    let mut file = fs::File::open(file_path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_len = metadata.len() as usize;

    const MAX_HEAD: usize = 2 * 1024 * 1024; // 2MB
    const MAX_TAIL: usize = 512 * 1024; // 512KB

    if file_len <= MAX_HEAD + MAX_TAIL {
        // 小文件，全部读取
        let mut content = String::new();
        file.read_to_string(&mut content).ok()?;
        Some(content)
    } else {
        // 大文件：读头部 + 尾部
        let mut head_buf = vec![0u8; MAX_HEAD];
        let head_n = file.read(&mut head_buf).ok()?;

        file.seek(SeekFrom::End(-(MAX_TAIL as i64))).ok()?;
        let mut tail_buf = Vec::new();
        file.read_to_end(&mut tail_buf).ok()?;

        let head = String::from_utf8_lossy(&head_buf[..head_n]);
        let tail = String::from_utf8_lossy(&tail_buf);

        // 合并，中间加换行分隔
        Some(format!("{}\n{}", head, tail))
    }
}

/// 从 JSONL 内容构建 Session
fn build_session(content: &str) -> ParsedSession {
    let mut turns: Vec<ParsedTurn> = Vec::new();
    let mut current_turn: Option<ParsedTurn> = None;
    let mut pending: HashMap<String, PendingToolCall> = HashMap::new();
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut model = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let val: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let record_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let timestamp = val
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        match record_type {
            "user" => {
                let message = match val.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let content = message.get("content");

                // 检查是否是 tool_result
                let has_tool_result = content
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    })
                    .unwrap_or(false);

                if has_tool_result {
                    // 配对 tool_result → 已有的 tool_use
                    if let Some(arr) = content.and_then(|c| c.as_array()) {
                        for block in arr {
                            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                                continue;
                            }
                            let tool_use_id = match block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                            {
                                Some(id) => id.to_string(),
                                None => continue,
                            };
                            let is_error = block
                                .get("is_error")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            if let Some(p) = pending.remove(&tool_use_id) {
                                // 更新对应 turn 中的 tool call
                                let turn = if p.turn_idx < turns.len() {
                                    &mut turns[p.turn_idx]
                                } else if let Some(ref mut ct) = current_turn {
                                    ct
                                } else {
                                    continue;
                                };

                                if p.tool_idx < turn.tool_calls.len() {
                                    turn.tool_calls[p.tool_idx].end_time =
                                        timestamp.clone();
                                    turn.tool_calls[p.tool_idx].is_error = is_error;
                                }
                            }
                        }
                    }
                } else {
                    // 普通 user 消息 → 开启新 turn
                    if let Some(t) = current_turn.take() {
                        turns.push(t);
                    }
                    current_turn = Some(ParsedTurn {
                        tool_calls: Vec::new(),
                    });
                }
            }

            "assistant" => {
                // 确保有 turn
                if current_turn.is_none() {
                    current_turn = Some(ParsedTurn {
                        tool_calls: Vec::new(),
                    });
                }

                let message = match val.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                // 提取 model
                if model.is_empty() {
                    if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
                        model = m.to_string();
                    }
                }

                // 累加 token usage
                if let Some(usage) = message.get("usage") {
                    total_input_tokens +=
                        usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    total_output_tokens +=
                        usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    // cache_read_input_tokens 也算 input
                    total_input_tokens += usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                }

                // 提取 tool_use blocks
                if let Some(blocks) = message.get("content").and_then(|c| c.as_array()) {
                    for block in blocks {
                        if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                            continue;
                        }
                        let tool_id =
                            block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();

                        let ct = current_turn.as_mut().unwrap();
                        let tool_idx = ct.tool_calls.len();
                        let turn_idx = turns.len(); // current_turn 的逻辑索引

                        ct.tool_calls.push(ParsedToolCall {
                            start_time: timestamp.clone(),
                            end_time: None,
                            is_error: false,
                        });

                        if !tool_id.is_empty() {
                            pending.insert(
                                tool_id,
                                PendingToolCall {
                                    turn_idx,
                                    tool_idx,
                                },
                            );
                        }
                    }
                }
            }

            _ => {
                // queue-operation, progress, system 等 → 跳过
            }
        }
    }

    // 收尾
    if let Some(t) = current_turn {
        turns.push(t);
    }

    // 计算预估费用（基于模型定价）
    let estimated_cost = estimate_cost(&model, total_input_tokens, total_output_tokens);

    ParsedSession {
        turns,
        sub_agents: Vec::new(),
        token_usage: TokenUsage { estimated_cost },
    }
}

/// 基于模型和 token 数量估算费用（美元）
fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    // 定价：$/百万 token
    let (input_price, output_price) = if model.contains("opus") {
        (15.0, 75.0) // Opus
    } else if model.contains("haiku") {
        (0.25, 1.25) // Haiku
    } else {
        // Sonnet (default)
        (3.0, 15.0)
    };

    let input_cost = (input_tokens as f64 / 1_000_000.0) * input_price;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * output_price;
    input_cost + output_cost
}
