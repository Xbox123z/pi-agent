//! 子代理（subagent）：把子任务委派给一个独立的 Agent 实例。
//!
//! - 每个子代理有**独立的 Memory**（不污染父会话）
//! - 共享父级的 ModelRouter / ToolRegistry / 插件（能力一致）
//! - 通过 depth 限制递归深度，防止无限自我调用
//! - 子代理工具集自动移除 `subagent`，杜绝无限递归
//!
//! 父 Agent 通过 [`SubagentInvoker`] trait object 注入到 `ToolContext`，
//! 避免循环依赖，也不依赖 thread-local（async 任务可能跨线程执行）。

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::agent::{Agent, AgentHooks};
use crate::memory::Memory;
use crate::router::ModelRouter;
use crate::session::Session;
use crate::tools::{Policy, Tool, ToolContext, ToolRegistry};

/// 子代理默认最大递归深度（顶层 agent 为 depth 0）。
pub const MAX_DEPTH: u32 = 2;

/// 派发接口：由 Agent 实现，供 `subagent` 工具调用。
#[async_trait]
pub trait SubagentInvoker: Send + Sync {
    async fn spawn(
        &self,
        prompt: &str,
        depth: u32,
        tools: Option<Vec<String>>,
        max_steps: usize,
    ) -> Result<String>;
}

/// 创建子代理所需的共享依赖（从父 Agent 克隆而来）。
#[derive(Clone)]
pub struct AgentDeps {
    pub router: ModelRouter,
    pub tools: ToolRegistry,
    pub plugins: Arc<crate::plugins::PluginHost>,
    pub policy: crate::config::PolicyConfig,
    pub model: String,
    pub cwd: std::path::PathBuf,
}

impl AgentDeps {
    pub fn from_agent(a: &Agent) -> Self {
        Self {
            router: a.router.clone(),
            tools: a.tools.clone(),
            plugins: a.plugins.clone(),
            policy: a.policy.clone(),
            model: a.model.clone(),
            cwd: a.session.cwd.clone(),
        }
    }
}

/// Agent 的派发器实现。
pub struct AgentInvoker {
    pub deps: AgentDeps,
}

#[async_trait]
impl SubagentInvoker for AgentInvoker {
    async fn spawn(
        &self,
        prompt: &str,
        depth: u32,
        tools: Option<Vec<String>>,
        max_steps: usize,
    ) -> Result<String> {
        spawn_with(&self.deps, prompt, depth, tools, max_steps).await
    }
}

/// 静默 hooks：子代理不向终端打印中间过程，只返回最终结果。
struct SilentHooks;

#[async_trait]
impl AgentHooks for SilentHooks {
    async fn confirm(&self, _tool: &str, _args: &Value) -> bool {
        true // 子代理由父级授权，内部自动批准
    }
    async fn on_delta(&self, _text: &str) {}
    async fn on_text(&self, _text: &str) {}
    async fn on_tool(&self, _name: &str, _ok: bool, _preview: &str) {}
}

/// 用给定依赖派生子代理并执行。
pub async fn spawn_with(
    deps: &AgentDeps,
    prompt: &str,
    depth: u32,
    tool_filter: Option<Vec<String>>,
    max_steps: usize,
) -> Result<String> {
    if depth >= MAX_DEPTH {
        return Err(anyhow!(
            "subagent recursion limit reached (depth {depth} >= {MAX_DEPTH})"
        ));
    }

    // 子代理工具集：默认继承父级；始终移除 `subagent` 防无限递归
    let mut tools = match tool_filter {
        Some(names) => {
            let mut reg = ToolRegistry::new();
            for n in names {
                if let Some(t) = deps.tools.get(&n) {
                    reg.register(t);
                }
            }
            reg
        }
        None => deps.tools.clone(),
    };
    tools.remove("subagent");

    let mut session = Session::new(deps.cwd.clone());
    session.auto_yes = true;
    session.max_steps = max_steps;

    let mut child = Agent {
        router: deps.router.clone(),
        tools,
        plugins: deps.plugins.clone(),
        session,
        memory: Memory::new(),
        model: deps.model.clone(),
        system_prompt: subagent_system_prompt(&deps.cwd.to_string_lossy()),
        policy: deps.policy.clone(),
        depth: depth + 1,
    };

    child.run(prompt, Arc::new(SilentHooks)).await
}

fn subagent_system_prompt(cwd: &str) -> String {
    format!(
        "You are a focused sub-agent working on ONE well-scoped subtask.\n\
         Working directory: {cwd}\n\
         Rules:\n\
         - Complete the subtask using tools, then return a concise result.\n\
         - Do NOT delegate further; do NOT ask questions; just do the work.\n\
         - Be precise and minimal. Reply in the same language as the task."
    )
}

/// `subagent` 工具：让模型派生子代理处理子任务。
pub struct SubagentTool;

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }
    fn description(&self) -> &str {
        "Delegate a self-contained subtask to a fresh sub-agent with its own context. \
         Args: prompt (required), optional tools (array of tool names), max_steps. \
         Useful for research or scoped work that would otherwise bloat the main conversation."
    }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "The subtask for the sub-agent" },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional subset of tool names to expose"
                },
                "max_steps": { "type": "integer", "description": "Max agent steps (default 12)" }
            },
            "required": ["prompt"]
        })
    }
    fn base_policy(&self) -> Policy {
        Policy::Allow // 子代理是受控的：独立上下文 + 深度上限 + 自动批准
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let invoker = ctx
            .subagent
            .as_ref()
            .ok_or_else(|| anyhow!("subagent is not available in this context"))?;

        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing `prompt`"))?;
        let tools: Option<Vec<String>> = args
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
        let max_steps = args
            .get("max_steps")
            .and_then(|v| v.as_u64())
            .unwrap_or(12) as usize;

        // 深度从父 Agent 传入（见 agent.rs：ctx 里携带 depth）
        let depth = ctx.depth;
        invoker.spawn(prompt, depth, tools, max_steps).await
    }
}
