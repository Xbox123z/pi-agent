<div align="center">

# 🦀 Pi Agent (π)

**用 Rust 编写的高性能轻量 AI Agent —— 深度还原 DeepSeek Harness (DSH) 架构，内置免费模型账号池与子代理编排**

[下载二进制 (Releases)](https://github.com/Xbox123z/pi-agent/releases) • [快速开始](#-快速开始) • [DSH架构映射](#-架构对照-deepseek-harness) • [账号池指南](#-免费模型账号池-pi-pool)

[![CI Status](https://github.com/Xbox123z/pi-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/Xbox123z/pi-agent/actions)
[![Release](https://img.shields.io/github/v/release/Xbox123z/pi-agent?color=orange&logo=rust)](https://github.com/Xbox123z/pi-agent/releases)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-brightgreen.svg?logo=rust)](https://www.rust-lang.org/)

</div>

---

## 📖 项目简介

**Pi Agent (π)** 是一个纯 Rust 打造、原生零垃圾回收开销的 AI 智能代理引擎。

设计上完整借鉴并模拟了 **DeepSeek Harness (DSH)** 的核心分层思想：
- **解耦的运行时**：Host Runtime 负责工具调度与确认闸门，LLM 负责逻辑决策。
- **内置账号池插件 (`pi-pool`)**：无需昂贵账单，直接聚合 OpenRouter、Groq、Gemini、Cerebras 等平台的免费模型额度，支持自动轮换与指数避退冷却。
- **子代理委派 (`subagent`)**：支持主代理将子任务委托给拥有独立会话记忆的独立 Agent，杜绝上下文污染与无限递归。
- **原生 SSE 流式输出**：字符级打字机实时刷新，工具增量调用无缝重组。

---

## 🏛️ 架构对照 (DeepSeek Harness)

Pi Agent 严格遵循 DSH 的模块化解耦规范，其核心映射如下：

```text
┌────────────────────────────────────────────────────────────────┐
│                       Pi Agent Runtime                         │
├────────────────────────────────┬───────────────────────────────┤
│    DSH (DeepSeek Harness)      │         Pi Agent (Rust)       │
├────────────────────────────────┼───────────────────────────────┤
│ Host / Orchestrator            │ pi-core::agent::Agent         │
│ Model Router (provider:model)  │ pi-core::router::ModelRouter  │
│ Model Provider Interface       │ pi-core::model::ModelProvider │
│ Policy Guard (allow/ask/deny)  │ pi-core::tools::Policy        │
│ Tool Execution Context         │ pi-core::tools::ToolContext   │
│ Plugin Host & Event Bus        │ pi-core::plugins::PluginHost  │
│ Session & Memory Compactor     │ pi-core::memory::Memory       │
│ Subagent Delegation            │ pi-core::subagent             │
│ Builtin Free Model Pool        │ plugins::pi-pool              │
└────────────────────────────────┴───────────────────────────────┘
```

```text
       ┌───────────────┐
       │   CLI / User  │
       └───────┬───────┘
               │ Goal & Interaction
       ┌───────▼───────────────────────────┐
       │        Agent Loop (pi-core)       │◄──────┐
       └───────┬───────────────────────────┘       │
               │                                   │ Tool
       ┌───────┴───────┐                   ┌───────┴──────┐
       │ Model Router  │                   │ Policy Gate  │
       └───────┬───────┘                   └───────▲──────┘
               │ Resolve                           │ Verify
       ┌───────▼────────────────┐          ┌───────┴──────┐
       │   pi-pool (Plugin)     │          │ Safe Tools   │
       │  • Account Rotation    │          │ • fs_read    │
       │  • Auto Retry-After    │          │ • fs_write   │
       │  • Multi-Platform Free │          │ • shell      │
       └────────────────────────┘          │ • subagent   │
                                           └──────────────┘
```

---

## ✨ 核心特性

- ⚡ **闪电冷启动**：单独立可执行二进制，启动时间仅需几毫秒，内存占用极低。
- 🔄 **智能账号池 (`pi-pool`)**：
  - 支持多平台、多 API Key 配置，轮询调度（Round-Robin）。
  - **429 自动冷却**：精准解析上游 `Retry-After` 头，遭遇限流自动切换下一可用 Key。
  - **智能模型寻址 (`pool:free-auto`)**：开箱即用，优先匹配免额度模型，遇到故障自动转移。
- 🤖 **子代理系统 (`subagent`)**：
  - 继承父代理的工具库与模型能力，但拥有独立的 `Memory` 空间。
  - 三重递归防护机制（深度上限、自动剥离 `subagent` 工具、受控委派）。
- 🛡️ **严格安全策略系统**：
  - `allow`（直通执行，如 `fs_read`）、`ask`（终端交互确认，如 `shell`）、`deny`（绝对禁止）。
  - 支持通配符规则（例如 `fs_*` 全放行）。
- 🌊 **字符级流式体验**：
  - 采用 Server-Sent Events (SSE) 协议解码。
  - 模型边思索边实时打字输出，复杂工具参数分片自动按序号重装。

---

## 🚀 快速开始

### 方式 A：直接下载各平台二进制

从 [Releases 页面](https://github.com/Xbox123z/pi-agent/releases) 获取构建好的二进制：
- **Linux x86_64**: `pi-linux-x86_64`
- **macOS Apple Silicon**: `pi-macos-aarch64`
- **Windows x64**: `pi-windows-x86_64.exe`

下载后直接重命名为 `pi` 或 `pi.exe` 并置入系统环境变量目录即可。

---

### 方式 B：从源码编译

**前置要求**：Rust 1.70+ 及 C/C++ 链接器（Windows 需 Visual C++ Build Tools，macOS 需 Command Line Tools）。

```bash
# 1. 克隆代码库
git clone https://github.com/Xbox123z/pi-agent.git
cd pi-agent

# 2. 编译发布版本
cargo build --release

# 3. 运行体验
./target/release/pi --help
```

---

## 🛠️ 使用指南

### 1. 快速使用免费模型

无需繁琐设置，直接使用预设的免费上游平台：

```bash
# 查看所有支持的免费上游平台及免费模型名称
pi pool presets

# 方式一：设置系统环境变量（免添加，自动识别入池）
# Windows PowerShell:
$env:OPENROUTER_API_KEY = "sk-or-v1-xxxxxx"
# Linux / macOS:
export OPENROUTER_API_KEY="sk-or-v1-xxxxxx"

# 方式二：命令行手动添加密钥到池中
pi pool add openrouter sk-or-v1-xxxxxx
pi pool add groq gsk_xxxxxx

# 健康检查当前池内账号连通性
pi pool check

# 列出池内所有账号及脱敏信息
pi pool list
```

---

### 2. 执行任务 (Run)

```bash
# 运行一次性任务（默认启用 pool:free-auto 智能免费路由）
pi run "阅读当前目录下的 Cargo.toml，并总结这个项目的核心依赖"

# 自动放行需确认的工具操作（免交互确认）
pi run "在当前目录创建一个 hello.txt 并写入欢迎语" -y

# 指定特定模型
pi run "写一个快速排序算法" -m pool:deepseek/deepseek-chat-v3.1:free
```

---

### 3. 交互式终端会话 (Chat)

```bash
# 进入全功能交互式对话
pi chat

# 在会话中可随时使用指令：
π> /models               # 列出当前所有已注册模型
π> /model groq:llama-3.3-70b-versatile  # 实时切换模型
π> /quit                 # 退出会话
```

---

## ⚙️ 配置文件说明

Pi Agent 支持分层配置机制：
1. 全局配置：`~/.pi/config.toml`
2. 项目级覆盖：当前工作区目录 `.pi/config.toml`

```toml
# 默认采用的路由模型
default_model = "pool:free-auto"

# 安全策略规则配置
[policy]
allow = ["fs_read", "fs_list", "fs_grep"]   # 允许静默执行的工具
deny  = []                                  # 严格禁止的工具（如 ["shell"]）

# 自建/直连上游配置（可选）
[provider]
base_url = "https://api.deepseek.com/v1"
api_key  = "sk-xxxxxxxx"
model    = "deepseek-chat"
```

---

## 📦 项目模块结构

```text
pi-agent/
├── Cargo.toml               # 工作区清单 (Workspace Resolver v2)
├── crates/
│   ├── pi-core/             # 运行时核心库
│   │   ├── src/agent.rs     # 主循环调度器 (LLM ⇄ Tools 往返)
│   │   ├── src/router.rs    # 多 Provider 模型解析器
│   │   ├── src/model.rs     # 统一协议抽象 (Messages, Chunks, ToolSpecs)
│   │   ├── src/openai.rs    # OpenAI 兼容层与 SSE 流式解析
│   │   ├── src/tools.rs     # 工具库声明与 Policy 策略闸门
│   │   ├── src/subagent.rs  # 子代理派发与上下文隔离
│   │   ├── src/memory.rs    # 会话记忆维护与智能压缩折叠
│   │   └── src/plugins.rs   # 插件宿主与广播事件总线
│   └── pi-cli/              # 命令行交互前端
│       └── src/main.rs      # CLI 子命令入口与打字机输出终端
└── plugins/
    └── pi-pool/             # 账号池核心插件
        ├── src/lib.rs       # 账号轮换、429冷却与 free-auto 调度
        └── src/presets.rs   # 涵盖各大主流免费服务商端点预设
```

---

## 📄 许可证

本项目依据 [MIT License](LICENSE) 协议完全开源。
