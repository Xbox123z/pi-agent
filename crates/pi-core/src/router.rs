//! 模型路由：把 "provider:model" 或裸模型名解析到具体 Provider。

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Arc;

use crate::model::ModelProvider;

#[derive(Default, Clone)]
pub struct ModelRouter {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
    fallback_order: Vec<String>,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_provider(&mut self, p: Arc<dyn ModelProvider>) {
        let id = p.id().to_string();
        self.fallback_order.push(id.clone());
        self.providers.insert(id, p);
    }

    /// 解析 "provider:model" 或裸模型名（在所有 provider 中查找）。
    pub fn resolve(&self, spec: &str) -> Result<(Arc<dyn ModelProvider>, String)> {
        if let Some((pid, model)) = spec.split_once(':') {
            let p = self
                .providers
                .get(pid)
                .ok_or_else(|| anyhow!("unknown provider `{pid}` (have: {})", self.providers.keys().cloned().collect::<Vec<_>>().join(", ")))?;
            return Ok((p.clone(), model.to_string()));
        }
        for id in &self.fallback_order {
            if let Some(p) = self.providers.get(id) {
                if p.models().iter().any(|m| m == spec) {
                    return Ok((p.clone(), spec.to_string()));
                }
            }
        }
        // 找不到时退给第一个 provider，让上游自行判断（便于代理端动态模型）。
        if let Some(id) = self.fallback_order.first() {
            let p = self.providers[id].clone();
            return Ok((p, spec.to_string()));
        }
        Err(anyhow!("no model providers registered"))
    }

    pub fn list_models(&self) -> Vec<String> {
        let mut out = Vec::new();
        for id in &self.fallback_order {
            if let Some(p) = self.providers.get(id) {
                for m in p.models() {
                    out.push(format!("{id}:{m}"));
                }
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
