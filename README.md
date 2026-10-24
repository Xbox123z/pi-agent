# Pi Agent (π)

> 用 Rust 写的开源轻量 AI Agent —— 架构上尽可能模拟 [DSH (DeepSeek Harness)](https://github.com/) 的分层设计，内置**账号池插件**接入各大平台的**免费模型**。

## 特性

- 🦀 **纯 Rust**，单二进制，冷启动毫秒级
- 🧩 **插件架构**：事件总线 + 插件注册工具/模型 Provider（内置 `pi-pool` 插件）
- 🔄 **账号池**：多平台多 Key，自动轮换、429/5xx 冷却、失败转移
- 🆓 **免费模型**：内置 OpenRouter(:free) / Groq / Gemini / Cerebras / GitHub Models / Mistral / Ollama 预设，`pool:free-auto` 一键自动路由
- 🛠️ **工具系统**：`fs_read` / `fs_list` / `fs_grep`（自动放行）、`fs_write` / `shell` / `web_fetch`（确认制）
- 🧠 **记忆**：会话历史 + 自动摘要压缩，JSON 持久化
- ⚡ **流式输出**：SSE 增量解析，打字机效果（工具调用增量同步累积）
- 🛡️ **策略闸门**：deny > allow > 工具默认策略，支持通配（如 `fs_*`）

## 架构（对照 DSH）

| DSH 概念 | Pi Agent 实现 |
|---|---|
| Host runtime | `pi-core::agent::Agent` 主循环 |
| 模型路由 | `pi-core::router::ModelRouter`（`provider:model` 寻址） |
| Provider | `pi-core::model::ModelProvider` trait + OpenAI 兼容实现 |
| 工具 + 审批 | `pi-core::tools`（JSON Schema + allow/ask/deny 策略） |
| 插件系统 | `pi-core::plugins::PluginHost`（事件总线 + 扩展点注册） |
| 会话/记忆 | `pi-core::session` / `pi-core::memory`（摘要压缩） |
| Harness 账号池插件 | `plugins/pi-pool`（轮换、冷却、失败转移、free-auto） |

## 快速开始

**编译前提**：Rust 1.70+，且需要 C/C++ linker（Windows 装
[VS Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)；
macOS `xcode-select --install`；Linux `build-essential`）。

```bash
cargo build --release
./target/release/pi pool presets        # 看看支持哪些免费上游
./target/release/pi pool add openrouter sk-or-xxxx   # 加一个 key
./target/release/pi run "读取 Cargo.toml 并总结这个项目" -y
./target/release/pi chat                 # 交互模式
```

也可以直接用环境变量（无需 add）：

```bash
$env:OPENROUTER_API_KEY="sk-or-..."   # Windows PowerShell
pi run "hello" -y                      # 自动导入到池中
```

默认模型是 `pool:free-auto`：在所有池内账号间轮换，优先选名字带 `free` 的模型，遇到 429/5xx 自动冷却并切换下一个账号。

## 账号池命令

```bash
pi pool presets            # 列出预设上游（含申请地址、环境变量名）
pi pool list               # 列出账号（key 脱敏）
pi pool add <preset> <key> [--base-url <url>]
pi pool remove <id>
pi pool check              # 健康检查
```

池数据存于 `~/.pi/pool.json`（**不要提交到 git**，已内置 .gitignore 规则示例）。

## 切换模型

```bash
pi run "..." -m pool:free-auto                          # 默认
pi run "..." -m pool:deepseek/deepseek-chat-v3.1:free   # 指定模型
pi run "..." -m direct:my-model                         # 直连 provider（config 里配的）
```

chat 模式中用 `/model <name>` 切换，`/models` 列出全部。

## 配置

全局 `~/.pi/config.toml` + 项目 `.pi/config.toml`（项目覆盖全局）：

```toml
default_model = "pool:free-auto"

[provider]              # 可选：不走池的直连上游
base_url = "https://api.deepseek.com/v1"
api_key = "sk-..."
model = "deepseek-chat"

[policy]
allow = ["fs_*"]        # 免确认
deny  = ["shell"]       # 永拒
```

环境变量兜底：`PI_BASE_URL` / `PI_API_KEY` / `PI_MODEL`。

## 路线图

- [x] 流式输出（SSE）—— `ModelProvider::stream` + `hooks.on_delta`
- [ ] 子代理（subagent）与工作流编排
- [ ] WASM 沙箱工具
- [ ] PHP 绑定（通过 FFI / 内嵌 CLI）
- [ ] TUI 界面（ratatui）

## License

MIT
