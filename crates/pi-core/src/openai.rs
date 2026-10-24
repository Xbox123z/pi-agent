//! OpenAI Chat Completions 兼容 Provider（OpenRouter / Groq / DeepSeek / Ollama / 自建代理通用）。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{ChatMessage, ChatResponse, ModelProvider, ToolCall, ToolSpec, Usage};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct OpenAiCompat {
    pub id: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
    pub extra_headers: Vec<(String, String)>,
    client: reqwest::Client,
}

impl OpenAiCompat {
    pub fn new(id: impl Into<String>, base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            models: vec![],
            extra_headers: vec![],
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("reqwest client"),
        }
    }

    pub fn with_models(mut self, models: Vec<String>) -> Self {
        self.models = models;
        self
    }

    pub fn with_header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.extra_headers.push((k.into(), v.into()));
        self
    }
}

/// 构造请求体（供账号池插件复用）。
pub fn build_body(model: &str, messages: &[ChatMessage], tools: &[ToolSpec]) -> Value {
    let mut body = json!({ "model": model, "messages": messages, "stream": false });
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
                }))
                .collect(),
        );
        body["tool_choice"] = json!("auto");
    }
    body
}

/// 解析响应体（供账号池插件复用）。
pub fn parse_response(v: &Value) -> Result<ChatResponse> {
    if let Some(err) = v.get("error") {
        return Err(anyhow!("upstream error: {err}"));
    }
    let msg = v
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("malformed response: {v}"))?;
    let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or_default().to_string();
    let tool_calls = msg
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, tc)| {
                    let f = tc.get("function")?;
                    Some(ToolCall {
                        id: tc.get("id").and_then(|x| x.as_str()).map(String::from).unwrap_or_else(|| format!("call_{i}")),
                        name: f.get("name")?.as_str()?.to_string(),
                        arguments: match f.get("arguments") {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => "{}".into(),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let usage = v.get("usage").map(|u| Usage {
        prompt_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
        completion_tokens: u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
    });
    Ok(ChatResponse { content, tool_calls, usage })
}

/// HTTP 层错误分类，供账号池做冷却/轮换决策。
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let body: String = self.body.chars().take(400).collect();
        write!(f, "HTTP {}: {}", self.status, body)
    }
}
impl std::error::Error for HttpError {}

/// 发送一次 **流式** chat/completions 请求，解析 SSE `data:` 行并推送增量。
///
/// 返回累积出的完整 ChatResponse（含 tool_calls）。
pub async fn post_chat_stream(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    headers: &[(String, String)],
    body: &Value,
    tx: &tokio::sync::mpsc::Sender<crate::model::StreamChunk>,
) -> Result<ChatResponse> {
    use crate::model::{StreamChunk, ToolCall};
    use futures_util::StreamExt;

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut body = body.clone();
    body["stream"] = json!(true);

    let mut req = client.post(&url).json(&body).header("Accept", "text/event-stream");
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req.send().await.with_context(|| format!("POST {url}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        return Err(HttpError { status, body: text }.into());
    }

    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    // tool_calls 增量按 index 累积：index -> (id, name, arguments)
    let mut partial: std::collections::HashMap<usize, (String, String, String)> =
        std::collections::HashMap::new();
    let mut usage = None;

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading stream chunk")?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        // 逐行处理完整行
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(err) = v.get("error") {
                return Err(anyhow!("upstream stream error: {err}"));
            }
            if let Some(u) = v.get("usage") {
                usage = Some(Usage {
                    prompt_tokens: u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                    completion_tokens: u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
                });
            }
            let Some(choice) = v.pointer("/choices/0") else { continue };
            if let Some(s) = choice.pointer("/delta/content").and_then(|c| c.as_str()) {
                if !s.is_empty() {
                    content.push_str(s);
                    let _ = tx.send(StreamChunk::Delta(s.to_string())).await;
                }
            }
            // 工具调用增量
            if let Some(arr) = choice.pointer("/delta/tool_calls").and_then(|t| t.as_array()) {
                for tc in arr {
                    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let entry = partial.entry(idx).or_insert_with(|| (String::new(), String::new(), String::new()));
                    if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                        entry.0.push_str(id);
                    }
                    if let Some(f) = tc.get("function") {
                        if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                            entry.1.push_str(n);
                        }
                        if let Some(a) = f.get("arguments").and_then(|x| x.as_str()) {
                            entry.2.push_str(a);
                        }
                    }
                }
            }
        }
    }

    // 汇总工具调用（按 index 顺序）
    let mut idxs: Vec<usize> = partial.keys().copied().collect();
    idxs.sort_unstable();
    for i in idxs {
        let (id, name, args) = &partial[&i];
        if name.is_empty() {
            continue;
        }
        let call = ToolCall {
            id: if id.is_empty() { format!("call_{i}") } else { id.clone() },
            name: name.clone(),
            arguments: if args.is_empty() { "{}".to_string() } else { args.clone() },
        };
        let _ = tx.send(StreamChunk::ToolCall(call.clone())).await;
        tool_calls.push(call);
    }
    let _ = tx.send(StreamChunk::Done).await;

    Ok(ChatResponse { content, tool_calls, usage })
}

/// 发送一次 chat/completions 请求。
pub async fn post_chat(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    headers: &[(String, String)],
    body: &Value,
) -> Result<ChatResponse> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut req = client.post(&url).json(body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.with_context(|| format!("POST {url}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(HttpError { status, body: text }.into());
    }
    let v: Value = serde_json::from_str(&text).with_context(|| format!("invalid JSON from {url}"))?;
    parse_response(&v)
}

#[async_trait]
impl ModelProvider for OpenAiCompat {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> Vec<String> {
        self.models.clone()
    }
    async fn complete(&self, model: &str, messages: &[ChatMessage], tools: &[ToolSpec]) -> Result<ChatResponse> {
        let body = build_body(model, messages, tools);
        post_chat(&self.client, &self.base_url, &self.api_key, &self.extra_headers, &body).await
    }

    async fn stream(
        &self,
        model: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<crate::model::StreamChunk>,
    ) -> Result<ChatResponse> {
        let body = build_body(model, messages, tools);
        post_chat_stream(&self.client, &self.base_url, &self.api_key, &self.extra_headers, &body, &tx).await
    }
}
