//! Agent 主循环：模型 ⇄ 工具往返，直到模型给出不含工具调用的回复。

use anyhow::{anyhow, Result};
use std::sync::Arc;

use crate::memory::Memory;
use crate::model::{ChatMessage, Role, ToolCall};
use crate::plugins::{Event, PluginHost};
use crate::router::ModelRouter;
use crate::session::Session;
use crate::tools::{Policy, ToolContext, ToolRegistry};

/// 每步执行结果的回调（CLI 用它打印进度 / 请求确认）。
#[async_trait::async_trait]
pub trait AgentHooks: Send + Sync {
    /// 工具需要确认时调用；返回 true 表示放行。
    async fn confirm(&self, tool: &str, args: &serde_json::Value) -> bool;
    /// 模型产出文本增量（完整段）。
    async fn on_text(&self, _text: &str) {}
    /// 工具执行完成。
    async fn on_tool(&self, _name: &str, _ok: bool, _preview: &str) {}
}

pub struct Agent {
    pub router: ModelRouter,
    pub tools: ToolRegistry,
    pub plugins: Arc<PluginHost>,
    pub session: Session,
    pub memory: Memory,
    pub model: String,
    pub system_prompt: String,
    pub policy: crate::config::PolicyConfig,
}

impl Agent {
    pub async fn run(&mut self, goal: &str, hooks: &dyn AgentHooks) -> Result<String> {
        if self.router.is_empty() {
            return Err(anyhow!(
                "no model provider configured — set PI_BASE_URL/PI_API_KEY or enable the pi-pool plugin"
            ));
        }
        self.plugins.emit(Event::AgentStart { goal: goal.to_string() }).await;
        self.memory.push(ChatMessage::user(goal));

        let mut final_text = String::new();
        for step in 0..self.session.max_steps {
            self.plugins.emit(Event::AgentStep { step }).await;

            // 组装上下文
            let mut messages = vec![ChatMessage::system(self.system_prompt.clone())];
            messages.extend(self.memory.view(self.session.keep_recent));

            let (provider, model) = self.router.resolve(&self.model)?;
            let specs = self.tools.specs();
            let resp = provider.complete(&model, &messages, &specs).await;

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    // 出错时把错误反馈给模型一次，仍失败则终止。
                    self.memory.push(ChatMessage::system(format!("[provider error] {e:#}")));
                    let (provider, model) = self.router.resolve(&self.model)?;
                    provider.complete(&model, &messages, &specs).await.map_err(|e2| {
                        anyhow!("provider failed twice: {e:#} ; {e2:#}")
                    })?
                }
            };

            // 记录 assistant 消息
            self.memory.push(ChatMessage {
                role: Role::Assistant,
                content: if resp.content.is_empty() { None } else { Some(resp.content.clone()) },
                tool_calls: resp.tool_calls.clone(),
                tool_call_id: None,
            });

            if !resp.content.is_empty() {
                hooks.on_text(&resp.content).await;
                final_text = resp.content.clone();
            }

            if resp.tool_calls.is_empty() {
                self.plugins.emit(Event::AgentEnd { ok: true }).await;
                return Ok(final_text);
            }

            // 执行工具调用
            for call in &resp.tool_calls {
                let result = self.execute_tool(call, hooks).await;
                let (text, ok) = match &result {
                    Ok(t) => (t.clone(), true),
                    Err(e) => (format!("[tool error] {e:#}"), false),
                };
                let preview: String = text.chars().take(300).collect();
                hooks.on_tool(&call.name, ok, &preview).await;
                self.plugins.emit(Event::ToolAfter { name: call.name.clone(), ok }).await;
                self.memory.push(ChatMessage::tool_result(&call.id, text));
            }

            // 定期压缩记忆
            self.memory.compact(self.session.keep_recent);
        }

        self.plugins.emit(Event::AgentEnd { ok: false }).await;
        Ok(format!("{final_text}\n[stopped: reached max steps {}]", self.session.max_steps))
    }

    async fn execute_tool(&self, call: &ToolCall, hooks: &dyn AgentHooks) -> Result<String> {
        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .unwrap_or_else(|_| serde_json::json!({}));
        self.plugins.emit(Event::ToolBefore { name: call.name.clone(), args: args.clone() }).await;

        let policy = self.tools.policy_for(&call.name, &self.policy);
        match policy {
            Policy::Deny => return Err(anyhow!("tool `{}` denied by policy", call.name)),
            Policy::Ask if !self.session.auto_yes && !hooks.confirm(&call.name, &args).await => {
                return Err(anyhow!("tool `{}` rejected by user", call.name));
            }
            _ => {}
        }

        let tool = self.tools.get(&call.name)
            .ok_or_else(|| anyhow!("unknown tool `{}`", call.name))?;
        let ctx = ToolContext { cwd: self.session.cwd.clone(), policy: self.policy.clone() };
        tool.execute(args, &ctx).await
    }
}
