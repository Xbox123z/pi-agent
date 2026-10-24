//! OpenAI Chat Completions 兼容 Provider（OpenRouter / Groq / DeepSeek / Ollama / 自建代理通用）。

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{ChatMessage, ChatResponse, ModelProvider, ToolCall, ToolSpec, Usage};

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
}
