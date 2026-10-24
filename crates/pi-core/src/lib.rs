//! pi-core — Pi Agent 核心运行时
//!
//! 一个用 Rust 编写的轻量 AI Agent 框架，设计上借鉴 DSH(DeepSeek Harness) 的架构：
//!
//! - [`config`]   —— 分层配置（全局 ~/.pi/config.toml + 项目 .pi/config.toml）
//! - [`model`]    —— 统一的 LLM 抽象（消息、流式分片、Provider 接口）
//! - [`router`]   —— 模型路由：按名称从多个 Provider 中解析目标模型
//! - [`tools`]    —— 工具系统：JSON Schema 声明 + 策略闸门（allow/ask/deny）
//! - [`plugins`]  —— 插件总线：事件广播、插件级工具注册
//! - [`agent`]    —— Agent 主循环：模型 ⇄ 工具 往返直到给出最终回复
//! - [`memory`]   —— 会话记忆与持久化
//! - [`session`]  —— 会话/运行时上下文

pub mod agent;
pub mod config;
pub mod memory;
pub mod model;
pub mod openai;
pub mod plugins;
pub mod router;
pub mod session;
pub mod subagent;
pub mod tools;
