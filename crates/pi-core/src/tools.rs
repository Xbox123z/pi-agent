//! 工具系统：声明式 JSON Schema + 策略闸门 + 内置安全工具集。

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::model::ToolSpec;

/// 工具执行策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// 直接执行
    Allow,
    /// 需要用户确认
    Ask,
    /// 拒绝执行
    Deny,
}

/// 工具执行上下文。
pub struct ToolContext {
    pub cwd: std::path::PathBuf,
    pub policy: crate::config::PolicyConfig,
    /// 子代理派发器（由 Agent 注入；支持 `subagent` 工具）。
    /// 用 trait object 而非直接持有 Agent，避免循环依赖与 async 跨线程问题。
    pub subagent: Option<std::sync::Arc<dyn crate::subagent::SubagentInvoker>>,
    /// 当前 Agent 深度（顶层 = 0），用于限制子代理递归。
    pub depth: u32,
}

impl ToolContext {
    pub fn new(cwd: std::path::PathBuf, policy: crate::config::PolicyConfig) -> Self {
        Self { cwd, policy, subagent: None, depth: 0 }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    /// 该工具的基础安全级别（可被配置覆盖）。
    fn base_policy(&self) -> Policy {
        Policy::Ask
    }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String>;

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }
}

/// 工具注册表 + 策略解析。
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    /// 移除一个工具（子代理用它去掉 `subagent`，防止无限递归）。
    pub fn remove(&mut self, name: &str) {
        self.tools.remove(name);
    }
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }
    /// 解析工具最终策略：deny 列表 > allow 列表 > 工具基础策略。
    pub fn policy_for(&self, name: &str, cfg: &crate::config::PolicyConfig) -> Policy {
        if cfg.deny.iter().any(|p| glob_match(p, name)) {
            return Policy::Deny;
        }
        if cfg.allow.iter().any(|p| glob_match(p, name)) {
            return Policy::Allow;
        }
        self.get(name).map(|t| t.base_policy()).unwrap_or(Policy::Deny)
    }
}

/// `fs_*` 这类简单前缀通配。
pub fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    pattern == name
}

/// 内置安全工具集（只读为主，写操作需要确认）。
pub fn builtin_tools() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(ReadFile));
    r.register(Arc::new(WriteFile));
    r.register(Arc::new(ListDir));
    r.register(Arc::new(Grep));
    r.register(Arc::new(Shell));
    r.register(Arc::new(WebFetch));
    r.register(Arc::new(crate::subagent::SubagentTool));
    r
}

fn arg_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("missing string argument `{key}`"))
}

// ---------------- read ----------------
struct ReadFile;
#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str { "fs_read" }
    fn description(&self) -> &str { "Read a UTF-8 text file. Args: path, optional offset/limit (lines)." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "offset": {"type": "integer"},
                "limit": {"type": "integer"}
            },
            "required": ["path"]
        })
    }
    fn base_policy(&self) -> Policy { Policy::Allow }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let path = ctx.cwd.join(arg_str(&args, "path")?);
        let text = tokio::fs::read_to_string(&path).await?;
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;
        let out: Vec<String> = text
            .lines()
            .skip(offset - 1)
            .take(limit)
            .enumerate()
            .map(|(i, l)| format!("{}: {l}", offset + i))
            .collect();
        Ok(out.join("\n"))
    }
}

// ---------------- write ----------------
struct WriteFile;
#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &str { "fs_write" }
    fn description(&self) -> &str { "Write (create or overwrite) a UTF-8 text file. Args: path, content." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"]
        })
    }
    fn base_policy(&self) -> Policy { Policy::Ask }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let path = ctx.cwd.join(arg_str(&args, "path")?);
        let content = arg_str(&args, "content")?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &content).await?;
        Ok(format!("wrote {} bytes to {}", content.len(), path.display()))
    }
}

// ---------------- list ----------------
struct ListDir;
#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &str { "fs_list" }
    fn description(&self) -> &str { "List directory entries. Args: path (default \".\")." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        })
    }
    fn base_policy(&self) -> Policy { Policy::Allow }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let path = ctx.cwd.join(args.get("path").and_then(|v| v.as_str()).unwrap_or("."));
        let mut rd = tokio::fs::read_dir(&path).await?;
        let mut out = Vec::new();
        while let Some(e) = rd.next_entry().await? {
            let suffix = if e.file_type().await?.is_dir() { "/" } else { "" };
            out.push(format!("{}{suffix}", e.file_name().to_string_lossy()));
        }
        out.sort();
        Ok(out.join("\n"))
    }
}

// ---------------- grep ----------------
struct Grep;
#[async_trait]
impl Tool for Grep {
    fn name(&self) -> &str { "fs_grep" }
    fn description(&self) -> &str { "Search files under a path for a substring. Args: pattern, optional path." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string"}
            },
            "required": ["pattern"]
        })
    }
    fn base_policy(&self) -> Policy { Policy::Allow }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let pattern = arg_str(&args, "pattern")?;
        let root = ctx.cwd.join(args.get("path").and_then(|v| v.as_str()).unwrap_or("."));
        let mut matches = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { continue };
            while let Ok(Some(e)) = rd.next_entry().await {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(text) = tokio::fs::read_to_string(&p).await {
                    for (i, line) in text.lines().enumerate() {
                        if line.contains(&pattern) {
                            matches.push(format!("{}:{}: {}", p.display(), i + 1, line.trim()));
                            if matches.len() >= 200 {
                                return Ok(matches.join("\n"));
                            }
                        }
                    }
                }
            }
        }
        Ok(matches.join("\n"))
    }
}

// ---------------- shell ----------------
struct Shell;
#[async_trait]
impl Tool for Shell {
    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str { "Run a shell command in the workspace. Args: command. Requires confirmation." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        })
    }
    fn base_policy(&self) -> Policy { Policy::Ask }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
        let cmd = arg_str(&args, "command")?;
        let mut command = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", &cmd]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &cmd]);
            c
        };
        let out = command.current_dir(&ctx.cwd).output().await?;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.is_empty() {
            text.push_str("\n[stderr]\n");
            text.push_str(&err);
        }
        if text.len() > 16_000 {
            text.truncate(16_000);
            text.push_str("\n...[truncated]");
        }
        Ok(text)
    }
}

// ---------------- web ----------------
struct WebFetch;
#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str { "Fetch an HTTP(S) URL and return decoded text (truncated). Args: url." }
    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"url": {"type": "string"}},
            "required": ["url"]
        })
    }
    fn base_policy(&self) -> Policy { Policy::Ask }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<String> {
        let url = arg_str(&args, "url")?;
        let resp = reqwest::Client::new()
            .get(&url)
            .header("User-Agent", "pi-agent/0.1")
            .send()
            .await?;
        let mut text = resp.text().await?;
        if text.len() > 24_000 {
            text.truncate(24_000);
            text.push_str("\n...[truncated]");
        }
        Ok(text)
    }
}
