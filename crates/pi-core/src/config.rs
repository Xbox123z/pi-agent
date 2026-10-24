//! 分层配置：全局 `~/.pi/config.toml` + 项目 `.pi/config.toml` + 环境变量。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PiConfig {
    /// 默认模型，如 "pool:free-auto" / "openrouter:deepseek/deepseek-chat-v3.1:free"
    #[serde(default)]
    pub default_model: Option<String>,
    /// 单一上游（不使用账号池时的直连配置）
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
    /// 策略：哪些工具免确认、哪些直接拒绝
    #[serde(default)]
    pub policy: PolicyConfig,
    /// 插件配置原文（各插件自行解析）
    #[serde(default)]
    pub plugins: HashMap<String, toml_like::Table>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// 免确认直接执行的工具（默认内置安全工具已在代码中放行）
    #[serde(default)]
    pub allow: Vec<String>,
    /// 始终拒绝的工具
    #[serde(default)]
    pub deny: Vec<String>,
}

/// 极简 TOML 表占位（避免强依赖 toml 解析，插件配置以 JSON 值存储）。
pub mod toml_like {
    pub type Table = serde_json::Map<String, serde_json::Value>;
}

impl PiConfig {
    pub fn global_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".pi").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let mut cfg = PiConfig::default();
        // 1) 全局配置
        if let Some(p) = Self::global_path() {
            if p.exists() {
                cfg = Self::load_file(&p)?;
            }
        }
        // 2) 项目级覆盖
        let proj = PathBuf::from(".pi/config.toml");
        if proj.exists() {
            let local = Self::load_file(&proj)?;
            cfg.merge(local);
        }
        // 3) 环境变量兜底
        if cfg.provider.is_none() {
            if let (Ok(base), Ok(key)) = (
                std::env::var("PI_BASE_URL"),
                std::env::var("PI_API_KEY"),
            ) {
                cfg.provider = Some(ProviderConfig {
                    base_url: base,
                    api_key: key,
                    model: std::env::var("PI_MODEL").ok(),
                });
            }
        }
        if cfg.default_model.is_none() {
            cfg.default_model = std::env::var("PI_MODEL").ok();
        }
        Ok(cfg)
    }

    fn load_file(p: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(p)
            .with_context(|| format!("read config {}", p.display()))?;
        parse_toml(&text).with_context(|| format!("parse config {}", p.display()))
    }

    fn merge(&mut self, other: PiConfig) {
        if other.default_model.is_some() {
            self.default_model = other.default_model;
        }
        if other.provider.is_some() {
            self.provider = other.provider;
        }
        self.policy.allow.extend(other.policy.allow);
        self.policy.deny.extend(other.policy.deny);
        self.plugins.extend(other.plugins);
    }
}

/// 超轻量 TOML 子集解析：支持 `key = "string"`、`key = true`、`[section]`、`key = ["a","b"]`。
/// 足够表达 pi 的配置；复杂场景建议直接使用 JSON 语法的值。
fn parse_toml(text: &str) -> Result<PiConfig> {
    let mut cfg = PiConfig::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = format!("{}.{}", section, k.trim());
        let val = parse_value(v.trim());
        apply(&mut cfg, &key, val);
    }
    Ok(cfg)
}

fn parse_value(v: &str) -> serde_json::Value {
    let v = v.trim();
    if v.starts_with('[') && v.ends_with(']') {
        let inner = &v[1..v.len() - 1];
        let items: Vec<serde_json::Value> = inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .map(serde_json::Value::String)
            .collect();
        return serde_json::Value::Array(items);
    }
    if v == "true" {
        return serde_json::Value::Bool(true);
    }
    if v == "false" {
        return serde_json::Value::Bool(false);
    }
    serde_json::Value::String(v.trim_matches('"').to_string())
}

fn apply(cfg: &mut PiConfig, key: &str, val: serde_json::Value) {
    let s = val.as_str().map(|x| x.to_string());
    match key {
        ".default_model" | "core.default_model" => cfg.default_model = s,
        "provider.base_url" | "provider.base-url" => {
            cfg.provider.get_or_insert(ProviderConfig {
                base_url: String::new(),
                api_key: String::new(),
                model: None,
            }).base_url = s.unwrap_or_default();
        }
        "provider.api_key" | "provider.api-key" => {
            cfg.provider.get_or_insert(ProviderConfig {
                base_url: String::new(),
                api_key: String::new(),
                model: None,
            }).api_key = s.unwrap_or_default();
        }
        "provider.model" => {
            if let Some(p) = cfg.provider.as_mut() {
                p.model = s;
            }
        }
        "policy.allow" => {
            if let serde_json::Value::Array(a) = val {
                cfg.policy.allow = a.into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            }
        }
        "policy.deny" => {
            if let serde_json::Value::Array(a) = val {
                cfg.policy.deny = a.into_iter().filter_map(|v| v.as_str().map(String::from)).collect();
            }
        }
        other => {
            if let Some(rest) = other.strip_prefix("plugins.") {
                let mut parts = rest.splitn(2, '.');
                let plugin = parts.next().unwrap_or_default().to_string();
                let field = parts.next().unwrap_or("value").to_string();
                cfg.plugins
                    .entry(plugin)
                    .or_default()
                    .insert(field, val);
            }
        }
    }
}
