//! 会话记忆：消息历史 + 简易摘要压缩 + JSONL 持久化。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::model::{ChatMessage, Role};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    pub messages: Vec<ChatMessage>,
    /// 历史摘要（压缩早期对话用）。
    #[serde(default)]
    pub summary: Option<String>,
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
    }

    /// 供模型消费的视图：摘要 + 最近 N 条。
    pub fn view(&self, keep_recent: usize) -> Vec<ChatMessage> {
        let mut out = Vec::new();
        if let Some(s) = &self.summary {
            out.push(ChatMessage::system(format!("[memory summary] {s}")));
        }
        let start = self.messages.len().saturating_sub(keep_recent);
        out.extend(self.messages[start..].iter().cloned());
        out
    }

    /// 超简易压缩：把较早的 user/assistant 消息折叠成摘要行。
    pub fn compact(&mut self, keep_recent: usize) {
        if self.messages.len() <= keep_recent + 4 {
            return;
        }
        let cut = self.messages.len() - keep_recent;
        let mut notes = Vec::new();
        for m in &self.messages[..cut] {
            if matches!(m.role, Role::User | Role::Assistant) {
                if let Some(c) = &m.content {
                    let line: String = c.chars().take(120).collect();
                    notes.push(format!("{:?}: {line}", m.role));
                }
            }
        }
        let merged = match &self.summary {
            Some(prev) => format!("{prev}\n{}", notes.join("\n")),
            None => notes.join("\n"),
        };
        // 摘要限长，避免无限增长。
        let mut chars: String = merged.chars().take(4000).collect();
        if merged.chars().count() > 4000 {
            chars.push_str(" ...[summary truncated]");
        }
        self.summary = Some(chars);
        self.messages.drain(..cut);
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn default_path(session_id: &str) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".pi")
            .join("sessions")
            .join(format!("{session_id}.json"))
    }
}
