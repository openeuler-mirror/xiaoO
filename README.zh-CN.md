<div align="center">
  <img src="./img/logo.jpeg" width="180" alt="xiaoO" style="border-radius: 6px;">
</div>

# xiaoO

[English](./README.md) | [中文](./README.zh-CN.md)

AgentOS 的开源智能中枢。

[![License](https://img.shields.io/badge/license-MulanPSL--2.0-blue.svg)](./License)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-v0.1.0-red.svg)](https://gitcode.com/openeuler/xiaoO)

## xiaoO 是什么？

xiaoO 是 AgentOS 的智能中枢，提供面向系统管理、智能体编排、工具执行、记忆、上下文压缩和多渠道接入的自治 Agent 运行时。

它把操作系统变成 Agent 可以稳定工作的环境：文件、Shell 命令、Git、Web 访问、LSP 诊断、技能、Hook、渠道接入和运行时遥测，都通过统一的 Agent Loop 协同起来。

xiaoO 的运行时还内置了分层记忆和自适应上下文压缩系统，让长对话、高频工具调用和多 Agent 协作不再被原始历史无限增长拖垮。

## 核心特性

- Agent 运行时中枢：支持 CLI、TUI、Daemon、HTTP API 和渠道集成。
- 完整工具能力：文件操作、Shell 执行、Git、Web 搜索/浏览、补丁应用、子 Agent 和可扩展工具清单。
- 自适应上下文管理：Token 预算跟踪、配置化压缩、上下文超限后的强制恢复和 prefix-cache 遥测。
- 流式推理展示：在模型工作时展示 provider 返回的 reasoning/thinking 增量。
- 推理强度分级：支持 `off`、`high`、`max`；TUI 中可用 `Shift+Tab` 循环切换。
- 会话管理：支持保存和恢复长时间运行的任务。
- LSP 诊断：编辑后通过 `rust-analyzer`、`pyright`、`typescript-language-server`、`gopls`、`clangd` 等服务展示错误和警告。
- Skills 技能系统：从本地目录或 Git 来源安装可复用的指令包。
- Hook 与插件系统：在 Agent 创建、LLM 调用前后、工具调用前后提供扩展点，可用于审计、策略、追踪和自定义扩展。
- 可观测性：支持实时 token/cost 统计，并通过 `noop`、`stdout` 或 `moirai-sqlite` 存储 trace。
- 定时和触发式任务：支持接入长期运行的自动化工作流。
- 本地化 UI：提供适合日常 Agent 工作的终端界面。

## 前置要求

- 已安装 Rust 工具链和 Cargo。
- 可用的 LLM provider 账号，或本地模型端点。
- 通过环境变量或 xiaoO 配置文件提供 provider 凭证。

## 从源码安装

```bash
git clone https://gitcode.com/openeuler/xiaoO.git
cd xiaoO
cargo build --release
cargo install --path apps/xiaoo-app
```

安装后应用二进制会位于 `~/.cargo/bin`。请确认 `~/.cargo/bin` 已加入 `PATH`。

如果希望构建时出现交互式安全插件安装提示，可以使用：

```bash
./build.sh --release
```

该构建脚本可以安装 `audit_agent` hooker，用于审计工具执行中的高风险操作。插件安装与使用请参考 [docs/plugins.md](./docs/plugins.md)。

## 快速开始

创建 `~/.config/xiaoo/config.toml`：

```toml
[llm]
provider = "openrouter"              # openai, anthropic, ollama, openrouter, deepseek, zai, minimax, kimi, minimax-coding-plan, kimi-coding-plan
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"   # 从这个环境变量读取 API 密钥
max_tokens = 128000                  # 可选，每次响应的最大输出 token 数
context_window = 128000              # 可选，显式指定总上下文预算上限
reasoning_effort = "off"             # 可选: off, high, 或 max

# 预定义 subagent 角色（CLI/TUI/Daemon 均支持） ⭐
[subagent.code_reviewer]
description = "代码审查专家"
prompt = "你是代码审查专家，专注于代码质量和最佳实践。"
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
read = true
glob = true
grep = true

[trace]
storage_backend = "moirai-sqlite"    # noop, stdout, 或 moirai-sqlite
db_path = "~/.xiaoo/traces.db"       # 当 storage_backend 为 moirai-sqlite 时使用
```

设置 provider 凭证：

```bash
export OPENROUTER_API_KEY="sk-or-..."
```

运行 xiaoO：

```bash
# 终端 UI
xiaoo-tui

# 单次 CLI 调用
xiaoo run -p "Count the characters in hello world"
```

CLI 输出示例：

```text
"hello world" has 11 characters.
```

## 上下文窗口

`[llm].context_window` 是可选项，用于显式设置 token 预算和上下文压缩使用的总上下文大小。xiaoO 会按以下顺序解析最终值：

1. 用户显式配置：`[llm].context_window`
2. 动态模型查询，目前支持 `gemini`、`anthropic` 和 `ollama`
3. 本地兜底默认值：
   - OpenAI-compatible、Ollama 和智谱系列默认为 `128000`
   - Anthropic 默认为 `200000`
   - Gemini 默认为 `1000000`

更多说明请查看 [Memory & Context Compression](./docs/memory_context_system.md)。

## 推理强度

`[llm].reasoning_effort` 用于控制 provider 侧支持的 thinking 或 reasoning 级别。

| 值 | 含义 | TUI 颜色 |
| --- | --- | --- |
| `off` | 在支持的 provider 中关闭额外推理控制 | 灰色 |
| `high` | 使用更强的推理/思考设置 | 黄色 |
| `max` | 使用最强的推理/思考设置 | 红色 |

TUI 状态栏会显示当前值：`Think off/high/max`。按 `Shift+Tab` 可按 `off -> high -> max -> off` 为下一轮切换强度。CLI 模式可使用：

```bash
xiaoo run --reasoning-effort high -p "Explain this repository"
```

Provider 映射采用 best-effort 策略：OpenAI-compatible provider 在 `high` 和 `max` 时接收 `reasoning_effort`；Anthropic 接收 `thinking.budget_tokens`；Gemini 接收 `thinkingConfig.thinkingBudget`；不支持该能力的 provider 会忽略此设置。`off` 会省略 provider 专用推理字段，使默认请求保留各 provider 的原生行为。

## Skills 技能

xiaoO 默认从 `~/.xiaoo/skills` 加载技能。每个技能都是一个由 `SKILL.md` 或 `SKILL.toml` 描述的可复用指令包。

```bash
xiaoo skill list
xiaoo skill show <name>
xiaoo skill audit <path>
xiaoo skill install ./my-skill/
xiaoo skill install https://github.com/user/my-skill.git
xiaoo skill remove <name>
```

完整技能工作流请参考 [docs/skill_usage.md](./docs/skill_usage.md)。

## Daemon 模式

xiaoO 可以作为 daemon 运行，并为 Feishu、Telegram 或自定义服务等外部系统提供 REST API。

```bash
# 默认监听地址：0.0.0.0:18080
xiaoo-app daemon

# 指定配置文件、监听地址和端口
xiaoo-app daemon --config /path/to/config.toml --host 127.0.0.1 --port 18080
```

HTTP 请求可在 JSON body 中通过 `agent` 选择 Agent 角色预设：

```json
{
  "text": "Review this patch for security issues",
  "channel": "http",
  "sender_id": "demo-user",
  "conversation_id": "demo-conv",
  "agent": "code-reviewer"
}
```

更多 daemon 配置请参考 [docs/daemon_config.md](./docs/daemon_config.md)。

## 更多文档

- [Memory & Context Compression](./docs/memory_context_system.md)
- [Plugin System](./docs/plugins.md)
- [Skill Usage](./docs/skill_usage.md)
- [Custom Agents](./docs/custom_agent.md)
- [Remote TUI](./docs/remote_tui.md)
- [Feishu Deployment](./docs/feishu_deploy.md)
- [Telegram Deployment](./docs/telegram_deploy.md)

## 许可证

xiaoO 使用 [MulanPSL-2.0](./License) 许可证。
