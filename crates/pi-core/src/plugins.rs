//! 插件总线：事件广播 + 插件注册（工具、模型 Provider、事件钩子）。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::model::ModelProvider;
use crate::tools::Tool;

/// 插件可监听的事件。
#[derive(Debug, Clone)]
pub enum Event {
    AgentStart { goal: String },
    AgentStep { step: usize },
    ToolBefore { name: String, args: Value },
    ToolAfter { name: String, ok: bool },
    AgentEnd { ok: bool },
    Shutdown,
}

/// 插件上下文：插件初始化时可用的注册接口集合。
pub struct PluginContext {
    pub tools: Vec<Arc<dyn Tool>>,
    pub providers: Vec<Arc<dyn ModelProvider>>,
}

impl PluginContext {
    pub fn new() -> Self {
        Self { tools: vec![], providers: vec![] }
    }
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str {
        self.id()
    }
    fn version(&self) -> &str {
        "0.1.0"
    }
    /// 插件初始化：注册工具/Provider，读取自己的配置段。
    async fn init(&self, config: &serde_json::Map<String, Value>, ctx: &mut PluginContext) -> Result<()>;
    /// 事件钩子（默认忽略）。
    async fn on_event(&self, _event: &Event) -> Result<()> {
        Ok(())
    }
}

/// 插件宿主：持有插件与事件总线。
pub struct PluginHost {
    plugins: Vec<Arc<dyn Plugin>>,
    tx: broadcast::Sender<Event>,
}

impl PluginHost {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(16));
        Self { plugins: vec![], tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn register(&mut self, plugin: Arc<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    /// 初始化全部插件并汇聚它们注册的扩展点。
    pub async fn init_all(
        &self,
        configs: &std::collections::HashMap<String, serde_json::Map<String, Value>>,
    ) -> Result<PluginContext> {
        let mut ctx = PluginContext::new();
        for p in &self.plugins {
            let empty = serde_json::Map::new();
            let cfg = configs.get(p.id()).unwrap_or(&empty);
            p.init(cfg, &mut ctx).await?;
        }
        Ok(ctx)
    }

    pub async fn emit(&self, event: Event) {
        let _ = self.tx.send(event.clone());
        for p in &self.plugins {
            let _ = p.on_event(&event).await;
        }
    }

    pub fn plugins(&self) -> &[Arc<dyn Plugin>] {
        &self.plugins
    }
}
