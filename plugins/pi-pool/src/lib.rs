//! pi-pool —— Pi Agent 内置插件：账号池 + 免费模型路由
//!
//! 功能：
//! - 管理多个上游平台的多个 API Key（账号池）
//! - 轮换（round-robin）+ 失败自动切换下一个账号
//! - 429/5xx 自动冷却（429 用 Retry-After，5xx 指数退避）
//! - 特殊模型 `free-auto`：在所有"免费上游"之间按 免费模型 -> 冷却 -> 轮换 顺序路由
//! - 账号保存在 `~/.pi/pool.json`（本机文件，权限自负，勿提交到 git）

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use pi_core::model::{ChatMessage, ChatResponse, ModelProvider, ToolSpec};
use pi_core::openai;
use pi_core::plugins::{Plugin, PluginContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub mod presets;

/// 池中的一个账号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

/// 池的持久化文件结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PoolFile {
    #[serde(default)]
    pub accounts: Vec<Account>,
}

pub fn pool_path() -> PathBuf {
    dirs_home().join(".pi").join("pool.json")
}

fn dirs_home() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn load_pool() -> PoolFile {
    let p = pool_path();
    if !p.exists() {
        return PoolFile::default();
    }
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_pool(pool: &PoolFile) -> Result<()> {
    let p = pool_path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(pool)?)?;
    Ok(())
}

/// 运行时账号状态（冷却等）。
struct AccountState {
    account: Account,
    cooldown_until: Option<Instant>,
    fails: u32,
}

/// 账号池 Provider：对上层表现为一个 provider（id = "pool"）。
pub struct PoolProvider {
    states: Mutex<Vec<AccountState>>,
    rr: AtomicUsize,
    client: reqwest::Client,
}

impl PoolProvider {
    pub fn new(accounts: Vec<Account>) -> Self {
        Self {
            states: Mutex::new(
                accounts.into_iter().map(|a| AccountState { account: a, cooldown_until: None, fails: 0 }).collect(),
            ),
            rr: AtomicUsize::new(0),
            client: reqwest::Client::builder().timeout(Duration::from_secs(180)).build().expect("reqwest client"),
        }
    }

    /// 按 priority 降序 + 轮换次序，返回可用账号索引列表。
    fn order(&self, want_model: Option<&str>) -> Vec<usize> {
        let states = self.states.lock().unwrap();
        let now = Instant::now();
        let start = self.rr.fetch_add(1, Ordering::Relaxed);
        let n = states.len();
        let mut idxs: Vec<usize> = (0..n).map(|i| (start + i) % n).collect();
        idxs.retain(|&i| {
            let s = &states[i];
            let cooled = s.cooldown_until.map(|t| t <= now).unwrap_or(true);
            let model_ok = match want_model {
                None | Some("free-auto") => true,
                Some(m) => s.account.models.is_empty() || s.account.models.iter().any(|x| x == m),
            };
            cooled && model_ok
        });
        idxs.sort_by_key(|&i| -states[i].account.priority);
        idxs
    }

    fn mark_fail(&self, idx: usize, status: u16, retry_after: Option<u64>) {
        let mut states = self.states.lock().unwrap();
        if let Some(s) = states.get_mut(idx) {
            s.fails += 1;
            let cool = match status {
                429 => Duration::from_secs(retry_after.unwrap_or(60).min(3600)),
                401 | 403 => Duration::from_secs(3600), // 鉴权问题：长冷却
                s if s >= 500 => Duration::from_secs((2u64.pow(s.fails.min(6))).min(300)),
                _ => Duration::from_secs(10),
            };
            s.cooldown_until = Some(Instant::now() + cool);
        }
    }

    fn mark_ok(&self, idx: usize) {
        let mut states = self.states.lock().unwrap();
        if let Some(s) = states.get_mut(idx) {
            s.fails = 0;
            s.cooldown_until = None;
        }
    }

    /// 解析 Retry-After 头（秒）。
    fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
        headers
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
    }

    /// 池状态报告（CLI `pi pool list` 用）。
    pub fn report(&self) -> Vec<(String, String, bool, u32)> {
        let states = self.states.lock().unwrap();
        let now = Instant::now();
        states
            .iter()
            .map(|s| {
                let cooled = s.cooldown_until.map(|t| t <= now).unwrap_or(true);
                (s.account.id.clone(), s.account.provider.clone(), cooled, s.fails)
            })
            .collect()
    }

    /// free-auto 路由：把请求依次尝试到每个可用账号，
    /// 每个账号用它的第一个模型（若池里配了多个模型，优先含 "free" 的）。
    async fn complete_auto(&self, messages: &[ChatMessage], tools: &[ToolSpec]) -> Result<ChatResponse> {
        let idxs = self.order(None);
        if idxs.is_empty() {
            return Err(anyhow!("pool: no available accounts (all cooling down or empty). Run `pi pool list` / `pi pool add`."));
        }
        let mut last_err = anyhow!("pool: no attempt made");
        for idx in idxs {
            let (base, key, model) = {
                let states = self.states.lock().unwrap();
                let a = &states[idx].account;
                let model = a
                    .models
                    .iter()
                    .find(|m| m.contains("free"))
                    .or_else(|| a.models.first())
                    .cloned()
                    .unwrap_or_else(|| "auto".to_string());
                (a.base_url.clone(), a.api_key.clone(), model)
            };
            let body = openai::build_body(&model, messages, tools);
            match self.try_account(idx, &base, &key, &body).await {
                Ok(resp) => return Ok(resp),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    async fn try_account(&self, idx: usize, base: &str, key: &str, body: &serde_json::Value) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let mut req = self.client.post(&url).json(body);
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }
        req = req.header("HTTP-Referer", "https://github.com/Xbox123z/pi-agent").header("X-Title", "Pi Agent");
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                self.mark_fail(idx, 500, None);
                return Err(anyhow!("account #{idx} network error: {e}"));
            }
        };
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let ra = Self::retry_after(resp.headers());
            let text = resp.text().await.unwrap_or_default();
            self.mark_fail(idx, status, ra);
            let snippet: String = text.chars().take(300).collect();
            return Err(anyhow!("account #{idx} HTTP {status}: {snippet}"));
        }
        let text = resp.text().await.unwrap_or_default();
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                self.mark_fail(idx, 500, None);
                return Err(anyhow!("account #{idx} invalid JSON: {e}"));
            }
        };
        match openai::parse_response(&v) {
            Ok(r) => {
                self.mark_ok(idx);
                Ok(r)
            }
            Err(e) => {
                self.mark_fail(idx, 500, None);
                Err(anyhow!("account #{idx} bad response: {e}"))
            }
        }
    }
}

#[async_trait]
impl ModelProvider for PoolProvider {
    fn id(&self) -> &str {
        "pool"
    }

    fn models(&self) -> Vec<String> {
        let mut out = vec!["free-auto".to_string()];
        let states = self.states.lock().unwrap();
        let mut seen = std::collections::HashSet::new();
        for s in states.iter() {
            for m in &s.account.models {
                if seen.insert(m.clone()) {
                    out.push(m.clone());
                }
            }
        }
        out
    }

    async fn complete(&self, model: &str, messages: &[ChatMessage], tools: &[ToolSpec]) -> Result<ChatResponse> {
        if model == "free-auto" {
            return self.complete_auto(messages, tools).await;
        }
        // 指定模型：在支持该模型的账号间轮换
        let idxs = self.order(Some(model));
        if idxs.is_empty() {
            return Err(anyhow!("pool: no available account serves model `{model}`"));
        }
        let body = openai::build_body(model, messages, tools);
        let mut last_err = anyhow!("pool: no attempt made");
        for idx in idxs {
            let (base, key) = {
                let states = self.states.lock().unwrap();
                let a = &states[idx].account;
                (a.base_url.clone(), a.api_key.clone())
            };
            match self.try_account(idx, &base, &key, &body).await {
                Ok(r) => return Ok(r),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }
}

/// 插件本体：把 PoolProvider 注册进运行时，并同步预置上游的环境变量 key。
pub struct PoolPlugin;

#[async_trait]
impl Plugin for PoolPlugin {
    fn id(&self) -> &str {
        "pi-pool"
    }
    fn name(&self) -> &str {
        "Account Pool + Free Models"
    }

    async fn init(&self, config: &serde_json::Map<String, serde_json::Value>, ctx: &mut PluginContext) -> Result<()> {
        // 1) 读 ~/.pi/pool.json
        let mut pool = load_pool();

        // 2) 自动导入环境变量中的 key（不覆盖已存在的账号）
        for preset in presets::PRESETS {
            if let Ok(key) = std::env::var(preset.env_key) {
                if key.is_empty() {
                    continue;
                }
                let acc_id = format!("{}:env", preset.id);
                if pool.accounts.iter().any(|a| a.id == acc_id) {
                    continue;
                }
                pool.accounts.push(Account {
                    id: acc_id,
                    provider: preset.id.to_string(),
                    base_url: preset.base_url.to_string(),
                    api_key: key,
                    models: preset.models.iter().map(|s| s.to_string()).collect(),
                    priority: 0,
                });
            }
        }

        // 3) 插件配置里内联的账号（pi.toml 的 [plugins.pi-pool]）
        if let Some(serde_json::Value::Array(arr)) = config.get("accounts") {
            for v in arr {
                if let Ok(acc) = serde_json::from_value::<Account>(v.clone()) {
                    if !pool.accounts.iter().any(|a| a.id == acc.id) {
                        pool.accounts.push(acc);
                    }
                }
            }
        }

        if pool.accounts.is_empty() {
            eprintln!("[pi-pool] warning: pool is empty — run `pi pool add <preset> <api-key>` or set an env key like OPENROUTER_API_KEY");
        }

        ctx.providers.push(std::sync::Arc::new(PoolProvider::new(pool.accounts)));
        Ok(())
    }
}

/// CLI 用的便捷函数：加一个账号并持久化。
pub fn add_account(provider: &str, api_key: &str, custom_base: Option<&str>) -> Result<Account> {
    let preset = presets::find(provider);
    let (base, models) = match (preset, custom_base) {
        (Some(p), None) => (p.base_url.to_string(), p.models.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        (Some(p), Some(b)) => (b.to_string(), p.models.iter().map(|s| s.to_string()).collect()),
        (None, Some(b)) => (b.to_string(), vec![]),
        (None, None) => return Err(anyhow!("unknown provider `{provider}` and no --base-url given. Known presets: {}", presets::PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", "))),
    };
    let mut pool = load_pool();
    let n = pool.accounts.iter().filter(|a| a.provider == provider).count() + 1;
    let acc = Account {
        id: format!("{provider}:{n}"),
        provider: provider.to_string(),
        base_url: base,
        api_key: api_key.to_string(),
        models,
        priority: 0,
    };
    pool.accounts.push(acc.clone());
    save_pool(&pool)?;
    Ok(acc)
}

pub fn remove_account(id: &str) -> Result<bool> {
    let mut pool = load_pool();
    let before = pool.accounts.len();
    pool.accounts.retain(|a| a.id != id);
    let removed = pool.accounts.len() != before;
    if removed {
        save_pool(&pool)?;
    }
    Ok(removed)
}

/// 供 CLI 展示：id -> (provider, masked key, models)
pub fn masked_list() -> Vec<(String, String, String, Vec<String>)> {
    load_pool()
        .accounts
        .into_iter()
        .map(|a| {
            let key = if a.api_key.len() > 8 {
                format!("{}...{}", &a.api_key[..4], &a.api_key[a.api_key.len() - 4..])
            } else {
                "***".to_string()
            };
            (a.id, a.provider, key, a.models)
        })
        .collect()
}

/// 健康检查：对每个账号发一个极小请求，返回 (id, ok, 信息)。
pub async fn check_all() -> Vec<(String, bool, String)> {
    let pool = load_pool();
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().expect("client");
    let mut out = Vec::new();
    for a in pool.accounts {
        let model = a.models.first().cloned().unwrap_or_else(|| "auto".into());
        let mut msgs = HashMap::new();
        msgs.insert("model", serde_json::Value::String(model.clone()));
        msgs.insert(
            "messages",
            serde_json::json!([{"role": "user", "content": "hi"}]),
        );
        msgs.insert("max_tokens", serde_json::json!(1));
        let url = format!("{}/chat/completions", a.base_url.trim_end_matches('/'));
        let res = client.post(&url).bearer_auth(&a.api_key).json(&msgs).send().await;
        match res {
            Ok(r) if r.status().is_success() => out.push((a.id, true, format!("ok ({model})"))),
            Ok(r) => out.push((a.id, false, format!("HTTP {}", r.status()))),
            Err(e) => out.push((a.id, false, format!("network: {e}"))),
        }
    }
    out
}
