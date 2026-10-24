//! pi —— Pi Agent 命令行入口
//!
//! 子命令：
//!   pi run "<goal>"       一次性任务
//!   pi chat               交互式会话
//!   pi models             列出可用模型（含 pool:free-auto）
//!   pi pool list|add|remove|check|presets   账号池管理

use anyhow::Result;
use clap::{Parser, Subcommand};
use pi_core::agent::{Agent, AgentHooks};
use pi_core::config::PiConfig;
use pi_core::memory::Memory;
use pi_core::plugins::PluginHost;
use pi_core::router::ModelRouter;
use pi_core::session::Session;
use pi_core::tools;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "pi", version, about = "Pi Agent — 开源轻量 AI Agent（Rust，内置免费模型账号池）")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 运行一次性任务
    Run {
        /// 任务目标
        goal: String,
        /// 模型（默认 pool:free-auto）
        #[arg(short, long)]
        model: Option<String>,
        /// 自动批准所有工具确认
        #[arg(short = 'y', long)]
        yes: bool,
        /// 最大步数
        #[arg(long, default_value_t = 24)]
        max_steps: usize,
    },
    /// 交互式会话
    Chat {
        #[arg(short, long)]
        model: Option<String>,
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 列出可用模型
    Models,
    /// 账号池管理
    Pool {
        #[command(subcommand)]
        sub: PoolCmd,
    },
}

#[derive(Subcommand)]
enum PoolCmd {
    /// 列出池内账号（脱敏）
    List,
    /// 添加账号：pi pool add openrouter sk-or-xxx [--base-url ...]
    Add {
        provider: String,
        api_key: String,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// 删除账号：pi pool remove openrouter:1
    Remove { id: String },
    /// 健康检查所有账号
    Check,
    /// 列出支持的免费上游预设
    Presets,
}

/// CLI 的 AgentHooks：工具确认、打印进度。
struct CliHooks { auto_yes: bool }

#[async_trait::async_trait]
impl AgentHooks for CliHooks {
    async fn confirm(&self, tool: &str, args: &serde_json::Value) -> bool {
        if self.auto_yes {
            println!("\x1b[90m[auto-approve] {tool}\x1b[0m");
            return true;
        }
        let pretty = serde_json::to_string_pretty(args).unwrap_or_default();
        let preview: String = pretty.chars().take(500).collect();
        println!("\x1b[33m⚠ tool `{tool}` wants to run with args:\x1b[0m\n{preview}");
        print!("Approve? [y/N/a(all)] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return false;
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => true,
            "a" | "all" => true, // 简化：本次放行；完整版可设会话级 auto_yes
            _ => false,
        }
    }
    async fn on_delta(&self, text: &str) {
        // 打字机：逐分片写出并立即刷新
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    async fn on_text(&self, text: &str) {
        println!("{text}");
    }
    async fn on_tool(&self, name: &str, ok: bool, preview: &str) {
        let icon = if ok { "\x1b[32m✓\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
        let pv: String = preview.chars().take(120).collect();
        println!("{icon} \x1b[36m{name}\x1b[0m \x1b[90m{pv}\x1b[0m");
    }
}

const SYSTEM_PROMPT: &str = r#"You are Pi Agent (π), an open-source lightweight AI agent written in Rust, architecturally inspired by DSH (DeepSeek Harness).

Capabilities:
- You have tools: fs_read, fs_list, fs_grep (read-only, auto-approved), fs_write, shell, web_fetch (need confirmation unless auto-approved),
  and subagent — delegate a self-contained subtask to a fresh sub-agent with its own context (keeps the main conversation short).
- Prefer reading before writing. Keep changes minimal and precise.
- When a task needs multiple steps, work through them with tools, then give a concise final answer.
- Reply in the same language the user uses.

Style: concise, technical, no filler."#;

async fn build_agent(model: Option<String>, yes: bool, max_steps: usize) -> Result<Agent> {
    let cfg = PiConfig::load().unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // 插件宿主 + 内置 pi-pool 插件
    let mut host = PluginHost::new(64);
    host.register(Arc::new(pi_pool::PoolPlugin));
    let ctx = host.init_all(&cfg.plugins).await?;

    // 模型路由：插件 provider + （可选）直连 provider
    let mut router = ModelRouter::new();
    for p in ctx.providers {
        router.add_provider(p);
    }
    if let Some(p) = &cfg.provider {
        let prov = pi_core::openai::OpenAiCompat::new("direct", &p.base_url, &p.api_key)
            .with_models(p.model.clone().into_iter().collect());
        router.add_provider(Arc::new(prov));
    }

    // 工具注册表：内置 + 插件注册
    let mut registry = tools::builtin_tools();
    for t in ctx.tools {
        registry.register(t);
    }

    let mut session = Session::new(cwd);
    session.auto_yes = yes;
    session.max_steps = max_steps;

    let memory = Memory::new();

    Ok(Agent {
        router,
        tools: registry,
        plugins: Arc::new(host),
        session,
        memory,
        model: model
            .or(cfg.default_model)
            .unwrap_or_else(|| "pool:free-auto".to_string()),
        system_prompt: SYSTEM_PROMPT.to_string(),
        policy: cfg.policy,
        depth: 0,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { goal, model, yes, max_steps } => {
            let mut agent = build_agent(model, yes, max_steps).await?;
            let hooks = std::sync::Arc::new(CliHooks { auto_yes: yes });
            // on_text 钩子已实时打印模型输出，这里只取最终返回值以传播错误
            let _ = agent.run(&goal, hooks).await?;
        }
        Cmd::Chat { model, yes } => {
            let mut agent = build_agent(model.clone(), yes, 64).await?;
            let hooks = std::sync::Arc::new(CliHooks { auto_yes: yes });
            println!("\x1b[1mPi Agent\x1b[0m — interactive chat. \x1b[90m/model <name> 切换模型, /quit 退出\x1b[0m");
            println!("\x1b[90mcurrent model: {}\x1b[0m", agent.model);
            let mut rl = rustyline::DefaultEditor::new()?;
            loop {
                let line = rl.readline("\x1b[1mπ>\x1b[0m ");
                let input = match line {
                    Ok(l) => l.trim().to_string(),
                    Err(_) => break,
                };
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&input);
                match input.as_str() {
                    "/quit" | "/exit" => break,
                    "/models" => {
                        for m in agent.router.list_models() {
                            println!("  {m}");
                        }
                        continue;
                    }
                    s if s.starts_with("/model ") => {
                        agent.model = s.trim_start_matches("/model ").trim().to_string();
                        println!("\x1b[90mmodel -> {}\x1b[0m", agent.model);
                        continue;
                    }
                    _ => {}
                }
                match agent.run(&input, hooks.clone()).await {
                    Ok(_) => {}
                    Err(e) => eprintln!("\x1b[31merror:\x1b[0m {e:#}"),
                }
            }
        }
        Cmd::Models => {
            let agent = build_agent(None, true, 1).await?;
            let models = agent.router.list_models();
            if models.is_empty() {
                println!("(no models — configure a provider or add pool accounts)");
            }
            for m in models {
                println!("{m}");
            }
        }
        Cmd::Pool { sub } => match sub {
            PoolCmd::List => {
                let rows = pi_pool::masked_list();
                if rows.is_empty() {
                    println!("(pool empty — `pi pool add <preset> <api-key>` or `pi pool presets`)");
                }
                for (id, provider, key, models) in rows {
                    println!("{id}\t{provider}\t{key}\t{} model(s)", models.len());
                }
            }
            PoolCmd::Add { provider, api_key, base_url } => {
                let acc = pi_pool::add_account(&provider, &api_key, base_url.as_deref())?;
                println!("added account `{}` ({} model(s))", acc.id, acc.models.len());
            }
            PoolCmd::Remove { id } => {
                if pi_pool::remove_account(&id)? {
                    println!("removed `{id}`");
                } else {
                    println!("no account `{id}`");
                }
            }
            PoolCmd::Check => {
                println!("checking accounts...");
                for (id, ok, info) in pi_pool::check_all().await {
                    let icon = if ok { "\x1b[32m✓\x1b[0m" } else { "\x1b[31m✗\x1b[0m" };
                    println!("{icon} {id}: {info}");
                }
            }
            PoolCmd::Presets => {
                println!("{:<12} {:<34} {:<22} models", "id", "signup", "env var");
                for p in pi_pool::presets::PRESETS {
                    println!("{:<12} {:<34} {:<22} {}", p.id, p.signup, p.env_key, p.models.len());
                }
            }
        },
    }
    Ok(())
}
