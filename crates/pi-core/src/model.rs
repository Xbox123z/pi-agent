//! 统一的 LLM 抽象层。

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// 对话中的一条消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// assistant 发起工具调用时携带。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// role == Tool 时对应的调用 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self { role: Role::System, content: Some(text.into()), tool_calls: vec![], tool_call_id: None }
    }
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, content: Some(text.into()), tool_calls: vec![], tool_call_id: None }
    }
    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self { role: Role::Tool, content: Some(text.into()), tool_calls: vec![], tool_call_id: Some(call_id.into()) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 一次工具调用（由模型产出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON 字符串参数。
    pub arguments: String,
}

/// 模型一次完整响应。
#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// 流式响应分片。
#[derive(Debug, Clone)]
pub enum StreamChunk {
    Delta(String),
    ToolCall(ToolCall),
    Done,
}

/// 提供给模型的工具描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 模型 Provider 抽象：任何兼容 OpenAI Chat Completions 的端点都可实现。
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider 标识，如 "openrouter" / "groq" / "pool:xxx"。
    fn id(&self) -> &str;
    /// 该 Provider 可用模型列表。
    fn models(&self) -> Vec<String>;
    /// 非流式补全。
    async fn complete(
        &self,
        model: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ChatResponse>;
    /// 流式补全（可选实现，默认回退为非流式整包推送）。
    async fn stream(
        &self,
        model: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        tx: mpsc::Sender<StreamChunk>,
    ) -> Result<ChatResponse> {
        let resp = self.complete(model, messages, tools).await?;
        if !resp.content.is_empty() {
            let _ = tx.send(StreamChunk::Delta(resp.content.clone())).await;
        }
        for tc in &resp.tool_calls {
            let _ = tx.send(StreamChunk::ToolCall(tc.clone())).await;
        }
        let _ = tx.send(StreamChunk::Done).await;
        Ok(resp)
    }
}
