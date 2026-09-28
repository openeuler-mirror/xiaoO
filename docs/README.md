# xiaoO 文档索引

本目录（`docs/`）是所有项目文档的唯一入口。**新增文档必须登记到本索引**，
否则 `scripts/check-docs.sh` 门禁会失败（防止出现无人可达的孤儿文档）。

- 文档基线：master `a58d79f`（2026-09-24）
- 门禁：`bash scripts/check-docs.sh`（相对链接可达 + 索引覆盖 + 双语 README 对称）
- 已接入 CI：`bash scripts/ci.sh` 第 3 步

## 状态徽标说明

| 徽标 | 含义 |
| --- | --- |
| ✅ `current` | 与当前代码一致，可作为参考依据 |
| 🔧 `refreshed` | 本次（2026-09）按代码核对并修订过 |
| 📐 `design: implemented` | 设计文档，所述能力**已实现**，代码为准 |
| 🧩 `design: partial` | 设计文档，仅**部分实现**，未实现部分已标注 |
| 🗄️ `obsolete` | 已过期，仅作历史留存，不作为依据 |

---

## 配置（Configuration）

配置类文档按"共用 / 分模式"组织，四篇互相链接，请按运行模式选择。

| 文档 | 说明 | 状态 |
| --- | --- | --- |
| [config_file_guide.md](./config_file_guide.md) | **共用配置项**：`[llm]`、`[subagent]`、`[skills]`、`[compact]`、`[trace]`、`[hooker]`、`[vault]`、`[mcp]`、`[memory_automation]` 等 | 🔧 `refreshed` |
| [cli_config.md](./cli_config.md) | CLI（`xiaoo --cli run`）专用配置与命令行参数 | 🔧 `refreshed` |
| [tui_config.md](./tui_config.md) | TUI 专用配置：`[tui.remote]`、`[lsp]`、`[agent]`，以及**键位说明** | 🔧 `refreshed` |
| [daemon_config.md](./daemon_config.md) | Daemon 专用配置：`[agents]`、`[channels]`、`[http]`、`[cron]`、`[mcp_server]`，以及 Runtime API / SSE 事件 | 🔧 `refreshed` |

> ⚠️ **配置解析重要提示**：除 `.mcp.json` 外，所有 TOML 配置结构体**没有**
> `deny_unknown_fields`——**写错键名不会报错，只会被静默忽略**。改动配置后请用
> `xiaoo-daemon config validate --config <path>` 校验。

## 架构与运行时（Architecture & Runtime）

| 文档 | 说明 | 状态 |
| --- | --- | --- |
| [architecture_overview.md](./architecture_overview.md) | **架构总览**：apps 分层、workspace crate、二进制、agent loop 与运行时分层。取代下方过期的 pptx | ✅ `current` |
| [xiaoo_api_pure_runtime_refactor.md](./xiaoo_api_pure_runtime_refactor.md) | `xiaoo-api` Runtime SDK 化改造说明；**已落地**，含验收证据 | 📐 `design: implemented` |
| [memory_context_system.md](./memory_context_system.md) | 记忆与上下文压缩：分层记忆、压缩阶段、`[memory_automation]`、失败 ingest 队列 | 🔧 `refreshed` |
| [runtime_checkpoint.md](./runtime_checkpoint.md) | Runtime 检查点：暂停 / 恢复 / checkout、租约（lease）、检查点目录 | 🔧 `refreshed` |
| [super_gateway_control_plane_design.md](./super_gateway_control_plane_design.md) | Super Gateway 控制面、Runtime API 与 Workspace 设计；**已实现**，代码为准 | 📐 `design: implemented` |
| [kvcache-coordination-design.md](./kvcache-coordination-design.md) | 语义感知 KV Cache 协同；**部分实现且跨仓库**（Rust 客户端在本仓，LMCache-Ascend 服务端不在） | 🧩 `design: partial` |
| [xiaoO系统架构分析.pptx](./xiaoO系统架构分析.pptx) | ⚠️ **历史材料，已过期**：仍写 `apps/xiaoo-app`、`xiaoo-tui` 等已不存在的名字，crate 清单缺 11 项。请改读 [architecture_overview.md](./architecture_overview.md) | 🗄️ `obsolete` |

## 能力扩展（Extending xiaoO）

| 文档 | 说明 | 状态 |
| --- | --- | --- |
| [plugins.md](./plugins.md) | 插件安装与使用：hooker 安装、内置 skills、chat/session 生命周期 hook、`stage` 取值 | 🔧 `refreshed` |
| [how-to-develop-a-plugin-hooker.md](../plugins/hookers/how-to-develop-a-plugin-hooker.md) | **Hooker 开发指南**：`plugin.json` 字段、四类 hook 家族（Tool/Chat/Llm/Session）、payload schema、超时 | 🔧 `refreshed` |
| [skill_usage.md](./skill_usage.md) | Skills 使用：目录优先级、CLI 子命令、禁用与审计配置、frontmatter 字段 | 🔧 `refreshed` |
| [custom_agent.md](./custom_agent.md) | 自定义 agent 角色：`[agent.<name>]`、工具可见性掩码语义 | 🔧 `refreshed` |
| [custom_tools.md](./custom_tools.md) | 声明式自定义工具：manifest 字段、`[effect]`、stdin/stdout 模式 | 🔧 `refreshed` |

## 集成与部署（Integration & Deployment）

| 文档 | 说明 | 状态 |
| --- | --- | --- |
| [mcp.md](./mcp.md) | MCP 支持：客户端配置、`/mcp/agent` 与 `/mcp/chat` 工具语义、诊断命令 | 🔧 `refreshed` |
| [vault_secrets_design.md](./vault_secrets_design.md) | 密钥存储：WhiteBox / SDF 密文格式、`config vault` 与 `set-secret` CLI、引用模型 | 🔧 `refreshed` |
| [cron_scheduled_jobs.md](./cron_scheduled_jobs.md) | 定时任务：`[cron]` 配置、调度器行为、HTTP/CLI 控制面 | 🔧 `refreshed` |
| [e2b_workspace_skills_bootstrap.md](./e2b_workspace_skills_bootstrap.md) | E2B 远端 workspace 与 skills 引导 | 🔧 `refreshed` |
| [docker_deploy.md](./docker_deploy.md) | 容器化部署：镜像内容、运行方式、环境变量。⚠️ **含构建当前不可用的说明** | 🔧 `refreshed` |
| [remote_tui.md](./remote_tui.md) | 远程 TUI：连接 daemon、端点清单、`/remote` 命令族 | 🔧 `refreshed` |
| [feishu_deploy.md](./feishu_deploy.md) | 飞书渠道部署：webhook、配置项、验证与排障 | 🔧 `refreshed` |
| [telegram_deploy.md](./telegram_deploy.md) | Telegram 渠道部署：webhook、配置项、验证与排障 | 🔧 `refreshed` |

## 测试与工程约定

| 文档 | 说明 |
| --- | --- |
| [tests/README.md](../tests/README.md) | 全仓唯一测试根目录 `tests/` 的目录结构、命名规则与接入方式 |
| [crates/cerberus/README.md](../crates/cerberus/README.md) | Cerberus 安全执行组件（未入 workspace，需自行准备构建环境） |

---

## 改代码时该同步改哪篇文档

改完代码请对照下表更新文档；不确定改哪篇时，以「配置键 → 共用配置指南」为起点。

| 改动内容 | 需要同步的文档 |
| --- | --- |
| TOML 配置键 / 默认值 | [config_file_guide.md](./config_file_guide.md) + 对应模式文档（[cli](./cli_config.md) / [tui](./tui_config.md) / [daemon](./daemon_config.md)） |
| CLI 参数 / 子命令 | [cli_config.md](./cli_config.md) |
| TUI 键位 / 斜杠命令 | [tui_config.md](./tui_config.md) |
| HTTP 路由 / SSE 事件 / `config` 子命令 | [daemon_config.md](./daemon_config.md) |
| Hook 点 / hook payload schema | [plugins.md](./plugins.md) + [how-to-develop-a-plugin-hooker.md](../plugins/hookers/how-to-develop-a-plugin-hooker.md) |
| 内置工具名 / 工具可见性语义 | [config_file_guide.md](./config_file_guide.md) + [custom_agent.md](./custom_agent.md) |
| skills 加载与配置 | [skill_usage.md](./skill_usage.md) + [plugins.md](./plugins.md) |
| workspace crate / 二进制名 / 分层 | [architecture_overview.md](./architecture_overview.md) |
| 渠道配置与部署步骤 | [feishu_deploy.md](./feishu_deploy.md) / [telegram_deploy.md](./telegram_deploy.md) |

## 校验与门禁

```bash
bash scripts/check-docs.sh          # 文档门禁：相对链接 + 索引覆盖 + 双语 README 对称
bash scripts/ci.sh                  # 统一 CI：门面纪律 + tests-hygiene + 文档门禁
./target/debug/xiaoo-daemon config validate --config <path>   # 校验配置示例
```

> **写文档的硬性约定**（2026-09 文档对齐中确立，请沿用）：
> 1. 只写代码里能指到 `path:line` 的行为；不确定就标注 `unverified`，不要写"看起来应该"的行为。
> 2. 代码与文档冲突且代码本身可疑时，文档写**实际行为**并标注 known gap，**不要顺手改代码**。
> 3. 配置示例必须能过 `config validate`；`.mcp.json` 示例必须能过 MCP 配置校验。
> 4. 文档中引用内置工具名一律用真实名（`file_read`/`file_write`/`file_edit`/`bash`/`glob`/`grep`/…），
>    `read`/`write`/`edit` 这类名字不存在，写错会被静默忽略。
> 5. `README.md` 与 `README.zh-CN.md` 必须成对更新，事实保持一致。
