# xiaoO Cron 定时任务 — 实现文档

> **状态（截至 master `a58d79f`）：已实现。**
>
> 本文件原为 *Spec 设计文档*。cron 调度器、配置解析、TUI 编辑器、HTTP/CLI 接口均已落地，
> 因此本文已改写为**按实现描述**，全部结论以 `path:line` 为据。
> 与旧版 Spec 的主要差异：
>
> | 旧版 Spec 描述 | 实际实现 |
> |---|---|
> | `cron` crate v0.15 | `cron = "0.13"`（`Cargo.toml:53`） |
> | 不支持动态添加/删除任务 | TUI 写入 `jobs.toml`；`config render-cron` 生成草稿 |
> | 生命周期 `open_session → submit_turn → close_session` | 单次 `SessionService::run_turn()`（`apps/serverside/src/cron/scheduler.rs:454`） |
> | 手动编辑后发送 SIGHUP 生效 | **无 SIGHUP**；只有 SIGINT/SIGTERM（`apps/serverside/src/main.rs:876-900`） |
> | `scheduler.start()` | `new` / `catalog` / `trigger_now` / `stop` |
> | 无 HTTP/CLI 章节 | §13 新增：`GET /api/v1/cron/jobs`、`POST /api/v1/cron/run`、`config cron` 等 |
>
> 保留的设计意图（cron 表达式规范、`[cron]` 配置语义、重试/超时策略）与实现一致。

## 1. 概述

### 1.1 背景

cron 功能已在 `GatewayEntryKind::ScheduledJob` 枚举变体中预留并**已被真实使用**：
`CronScheduler` 构造 `AppTurnRequest` 时填充该 kind
（`apps/serverside/src/cron/scheduler.rs:419-423`）。

### 1.2 核心设计决策

| 决策 | 选择 | 实现位置 |
|---|---|---|
| 全局设置 | `config.toml` 的 `[cron]` 段 | `apps/serverside/src/daemon_config.rs:1131-1140` |
| 任务定义 | `<cron dir>/jobs.toml` | `apps/serverside/src/cron_management.rs:176` |
| Cron 引擎 | `cron` crate v0.13 | `Cargo.toml:53` |
| 调度模式 | 每个 job 独立 `tokio::spawn` timer | `apps/serverside/src/cron/scheduler.rs:74` |
| 执行复用 | 构造 `AppTurnRequest`，调用 `SessionService::run_turn()` | `apps/serverside/src/cron/scheduler.rs:454` |

### 1.3 目标

- 用户在 `jobs.toml` 中定义 cron 定时任务，到时间自动触发 agent 执行
- 兼容标准 cron 表达式语法（5 字段，可归一化为 6 字段）
- 任务执行结果可追溯（trace/moirai）
- 与现有 Session 体系无缝对接
- 支持任务的启用/禁用、重试、超时控制
- 支持运维面：HTTP 目录/手动触发、CLI 校验/渲染/运行状态

### 1.4 非目标

- 不实现分布式调度（单机 daemon 内调度）
- 不实现任务依赖（DAG 编排）
- 秒级以下精度
- **无 SIGHUP 热重载**：修改 `jobs.toml` 后需重启 daemon 才能生效（§13.4）

---

## 2. 架构设计

### 2.1 整体架构

```
┌──────────────────────────────────────────────────────────────────┐
│                      配置文件                                     │
│                                                                  │
│  ~/.config/xiaoo/config.toml          ~/.config/xiaoo/cron/       │
│  ┌──────────────────────────┐         ┌──────────────────────┐   │
│  │ [cron]                   │         │ jobs.toml             │   │
│  │   enabled = true         │         │ [[job]] name="..."    │   │
│  │   max_concurrent_jobs=3  │         │ [[job]] name="..."    │   │
│  │   default_timeout_secs   │         │ ...                   │   │
│  └──────────────────────────┘         └──────────┬───────────┘   │
│                                                  │               │
├──────────────────────────────────────────────────┼───────────────┤
│                      运行时                        │               │
│                                                  │               │
│  ┌───────────────────────────────────────────────┴──────────┐    │
│  │  DaemonConfig::resolve_cron_jobs()                        │    │
│  │    → 读取 jobs.toml → 解析 cron → 合并全局默认 →          │    │
│  │      Vec<CronJobConfig>                                   │    │
│  │    ⚠ 任一条目非法 → 整个函数 Err → 全部 job 不加载        │    │
│  └──────────────────────────────────────────────────────────┘    │
│                              │                                   │
│                              ▼                                   │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  CronScheduler::new(jobs, max_concurrent, session_svc)    │    │
│  │                                                             │    │
│  │  为每个 enabled job spawn_job_timer()                       │    │
│  │    ├── tokio::select! { sleep until next / cancel }         │    │
│  │    ├── acquire concurrency semaphore permit                 │    │
│  │    └── execute_job_with_retry()                             │    │
│  │          └── execute_job_once()                             │    │
│  │                └── SessionService::run_turn(AppTurnRequest)  │    │
│  │                                                             │    │
│  │  catalog() / trigger_now(name) / stop()                     │    │
│  └──────────────────────────────────────────────────────────┘    │
│                              │                                   │
│                              ▼                                   │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  现有执行路径（无变更）                                      │    │
│  │    SessionSupervisor::run_root_turn()                       │    │
│  │      → SessionWorker::run()                                 │    │
│  │        → core::run_agent_loop()                             │    │
│  └──────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────┘
```

### 2.2 与现有架构的集成点

| 集成点 | 位置 | 说明 |
|---|---|---|
| `GatewayEntryKind::ScheduledJob` | `apps/serverside/src/cron/scheduler.rs:420` | 实际填充该 kind |
| `AppTurnRequest` | `apps/serverside/src/cron/scheduler.rs:417-440` | scheduler 构造时填充 |
| `SessionService` trait | `apps/serverside/src/cron/scheduler.rs:32` | scheduler 持有 `Arc<dyn SessionService>` |
| `SessionService::run_turn()` | `apps/serverside/src/cron/scheduler.rs:454` | **单次调用**，不 open/close session |
| `core::run_agent_loop()` | `crates/core/src/agent_loop.rs` | **已有**，无变更 |

### 2.3 `GatewayEntryKind::ScheduledJob` 的使用

`CronScheduler` 每次执行构造 `AppTurnRequest` 时填充 `ScheduledJob`
（`apps/serverside/src/cron/scheduler.rs:417-440`）：

```rust
let request = AppTurnRequest {
    session_id: session_id.clone(),              // format!("cron-{name}-{ts}")
    entry: GatewayEntryContext {
        kind: Some(GatewayEntryKind::ScheduledJob),
        runtime_profile_id: job.config.agent_role.clone(),
        ..Default::default()
    },
    sender_id: format!("cron/{}", job.config.name),
    text: job.config.prompt.clone(),
    client_id: Some(daemon_cron_principal()),
    chain_depth: 0,
    // ...
};
```

> **与旧版 Spec 的差异**：实现**不**调用 `open_session()` / `close_session()`；
> session id 由 `format!("cron-{}-{}", name, ts)` 直接生成
> （`apps/serverside/src/cron/scheduler.rs:413-415`），
> 一次 `run_turn` 即完成整个任务。

---

## 3. Cron 表达式规范

### 3.1 支持的格式

| 格式 | 示例 | 说明 |
|---|---|---|
| 标准 5 字段 | `0 9 * * mon-fri` | 分 时 日 月 周（默认） |
| 6 字段（含秒）| `0 0 9 * * mon-fri` | 秒 分 时 日 月 周 |

### 3.2 字段取值范围

```
┌────────────── 秒 (0-59, 可选)
│ ┌──────────── 分 (0-59)
│ │ ┌────────── 时 (0-23)
│ │ │ ┌──────── 日 (1-31)
│ │ │ │ ┌────── 月 (1-12)
│ │ │ │ │ ┌──── 周 (0-7, sun-sat)
│ │ │ │ │ │
* * * * * *
```

### 3.3 支持的运算符

| 运算符 | 示例 | 说明 |
|---|---|---|
| `*` | `* * * * *` | 每个时间单位 |
| `,` | `0,30 * * * *` | 列表（第 0 分和第 30 分）|
| `-` | `9-17 * * * *` | 范围（9 点到 17 点）|
| `/` | `*/15 * * * *` | 步长（每 15 分钟）|
| 名称 | `mon-fri` | 周名称（周一至周五）|
| 名称 | `jan-dec` | 月份名称 |

### 3.4 常见场景示例

| 场景 | 表达式 |
|---|---|
| 每天 9:00 | `0 9 * * *` |
| 工作日 9:00 | `0 9 * * mon-fri` |
| 每周一 8:30 | `30 8 * * mon` |
| 每月 1 号 0:00 | `0 0 1 * *` |
| 每 5 分钟 | `*/5 * * * *` |
| 每小时整点 | `0 * * * *` |
| 每天 0:00 + 12:00 | `0 0,12 * * *` |

### 3.5 不支持

- `@yearly` / `@monthly` / `@weekly` / `@daily` 宏
- `?` 和 `L` / `W` / `#` 扩展语法

---

## 4. 配置文件设计

### 4.1 文件布局

```
~/.config/xiaoo/
├── config.toml              # 主配置（daemon、LLM、channel 等）
│   └── [cron]               # cron 全局设置（可选）
│       ├── jobs_dir         # jobs.toml 所在目录（可选，有默认值）
│       ├── max_concurrent_jobs
│       └── default_timeout_secs
└── cron/
    └── jobs.toml             # 定时任务定义（[[job]] 数组）
```

### 4.2 `config.toml` — `[cron]` 段

```toml
# ===== 主配置文件：~/.config/xiaoo/config.toml =====

# ... 其他配置 ...

# Cron 全局设置（可选。不存在此段则 daemon 不启动 cron scheduler）
[cron]

# jobs.toml 所在目录（可选，默认 ~/.config/xiaoo/cron/）
# 支持 ~ 展开
jobs_dir = "~/.config/xiaoo/cron"

# 最大并发 job 数（0 = 无限制，默认 3）
# 当并发数达到上限时，新触发的 job 排队等待
max_concurrent_jobs = 3

# 全局默认超时时间（秒），单个 job 可覆盖
default_timeout_secs = 3600
```

**说明**：

- `[cron]` 段整体可选。如果不存在，daemon 不启动 cron scheduler
- `jobs_dir` 默认为 `~/.config/xiaoo/cron/`
- 如果 `jobs.toml` 文件不存在或为空，scheduler 启动但无任务运行

### 4.3 `jobs.toml` 文件格式

```toml
# ===== Cron 任务定义：~/.config/xiaoo/cron/jobs.toml =====
#
# 此文件可独立编辑，也可由 TUI 的 Cron 编辑器写入
# （apps/endside/src/services/cron_service.rs:144-187）。
# 修改后必须重启 daemon 才能生效：daemon 只在启动时解析一次 jobs.toml
# （apps/serverside/src/main.rs:653-656），且没有 SIGHUP 处理（§13.4）。

[[job]]
# 任务唯一标识（必填）
name = "morning-standup"

# 任务描述（可选，用于日志/UI 展示）
description = "每个工作日早上 9:00 生成团队站会摘要"

# Cron 表达式（必填，5 或 6 字段）
cron = "0 9 * * mon-fri"

# Agent prompt（必填）
prompt = """
请基于本仓库的最新提交记录，生成团队早上站会摘要：
1. 昨日完成的工作
2. 今日计划
3. 阻塞项
结果写入 docs/standup/$(date +%Y-%m-%d).md
"""

# 可选：指定使用的 agent role（对应 config.toml 中 [agent.xxx] 段）
# 不填则使用默认 agent
agent_role = "plan-agent"

# 可选：超时时间（秒），默认继承 [cron].default_timeout_secs
timeout_secs = 1800

# 可选：是否启用该任务（默认 true）
enabled = true

# 可选：失败后重试次数（默认 0 = 不重试）
max_retries = 2

# 可选：重试间隔（秒，默认 60）
retry_delay_secs = 120


[[job]]
name = "weekly-code-review"
description = "每周五下午 17:00 运行代码审查"
cron = "0 17 * * fri"
prompt = "请对本周所有提交进行代码审查，重点关注安全漏洞和性能问题。输出审查报告到 docs/reviews/weekly-$(date +%Y-W%V).md。"
agent_role = "code-reviewer"
timeout_secs = 7200
max_retries = 1
retry_delay_secs = 300


[[job]]
name = "hourly-health-check"
description = "每小时检查系统健康状态"
cron = "0 * * * *"
prompt = "检查系统健康状态：磁盘使用率、内存、CPU负载。如有异常写入 docs/alerts/$(date +%Y-%m-%d_%H).md。"
timeout_secs = 300
enabled = false    # 默认禁用
```

### 4.4 配置字段参考

#### `[cron]` 全局段

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `jobs_dir` | string | `"~/.config/xiaoo/cron"` | jobs.toml 所在目录 |
| `max_concurrent_jobs` | usize | `3` | 最大并发数，0 = 无限制 |
| `default_timeout_secs` | u64 | `3600` | 全局默认超时（秒）|

#### `[[job]]` 段

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|---|---|---|---|---|
| `name` | string | ✅ | — | 任务唯一标识 |
| `description` | string | | `None` | 任务描述 |
| `cron` | string | ✅ | — | cron 表达式 |
| `prompt` | string | ✅ | — | agent prompt |
| `agent_role` | string | | `None` | 指定 agent role |
| `timeout_secs` | u64 | | 继承全局 | 超时时间（秒）|
| `enabled` | bool | | `true` | 是否启用 |
| `max_retries` | u32 | | `0` | 失败后重试次数 |
| `retry_delay_secs` | u64 | | `60` | 重试间隔（秒）|

### 4.5 配置加载优先级

```
job.timeout_secs      >  [cron].default_timeout_secs  >  3600 (硬编码默认)
job.max_retries       >  默认 0（不重试）
job.retry_delay_secs  >  默认 60
```

---

## 5. Rust 数据结构

### 5.1 文件结构（实际）

```
crates/agent-types/src/
└── cron/
    ├── mod.rs
    ├── config.rs                       # CronGlobalConfig, CronJobDef, CronJobConfig
    ├── error.rs                        # CronParseError, CronExecutionError, CronError
    └── expression.rs                   # CronExpression (包装 cron crate)

apps/serverside/src/
├── daemon_config.rs                    # CronSectionRaw, CronJobRaw, resolve_cron_jobs()
├── cron_management.rs                  # CronReport, CronJobsDraft, render_cron_jobs()
├── main.rs                             # 集成 CronScheduler
└── cron/
    ├── mod.rs
    └── scheduler.rs                    # CronScheduler, 执行 + 重试逻辑

apps/endside/src/services/
└── cron_service.rs                     # TUI 侧读写 jobs.toml
```

### 5.2 `agent-types::cron::expression`

```rust
use std::str::FromStr;

/// 已验证的 cron 表达式
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpression(String);

impl CronExpression {
    pub fn parse(raw: &str) -> Result<Self, CronParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CronParseError::Empty);
        }
        cron::Schedule::from_str(trimmed)
            .map_err(|e| CronParseError::InvalidSyntax {
                raw: trimmed.to_string(),
                message: e.to_string(),
            })?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn next_after(
        &self,
        after: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let schedule = cron::Schedule::from_str(&self.0).ok()?;
        schedule.upcoming(chrono::Utc).next()
    }

    pub fn as_str(&self) -> &str { &self.0 }
}
```

### 5.3 `agent-types::cron::config`

```rust
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CronGlobalConfig {
    pub jobs_dir: PathBuf,
    pub max_concurrent_jobs: usize,
    pub default_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CronJobDef {
    pub name: String,
    pub description: Option<String>,
    pub cron: CronExpression,
    pub prompt: String,
    pub agent_role: Option<String>,
    pub timeout_secs: Option<u64>,
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}

/// 合并全局默认后的完整 job 配置（运行时使用）
#[derive(Debug, Clone)]
pub struct CronJobConfig {
    pub name: String,
    pub description: Option<String>,
    pub cron: CronExpression,
    pub prompt: String,
    pub agent_role: Option<String>,
    pub timeout_secs: u64,     // 已合并全局默认
    pub enabled: bool,
    pub max_retries: u32,
    pub retry_delay_secs: u64,
}
```

### 5.4 `agent-types::cron::error`

```rust
#[derive(Debug, thiserror::Error)]
pub enum CronParseError {
    #[error("empty cron expression")]
    Empty,
    #[error("invalid cron expression '{raw}': {message}")]
    InvalidSyntax { raw: String, message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CronExecutionError {
    #[error("job '{job_name}' timed out after {timeout_secs}s")]
    Timeout { job_name: String, timeout_secs: u64 },
    #[error("job '{job_name}' session error: {error}")]
    Session { job_name: String, error: String },
    #[error("job '{job_name}' is disabled")]
    Disabled { job_name: String },
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("parse error: {0}")]
    Parse(#[from] CronParseError),
    #[error("execution error: {0}")]
    Execution(#[from] CronExecutionError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
}
```

### 5.5 `daemon_config` 新增类型

```rust
// config.toml 中 [cron] 段的反序列化
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CronSectionRaw {
    #[serde(default = "default_cron_jobs_dir")]
    pub jobs_dir: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_jobs: usize,
    #[serde(default = "default_cron_timeout")]
    pub default_timeout_secs: u64,
}

// jobs.toml 顶层结构
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CronJobsFileRaw {
    #[serde(default)]
    pub job: Vec<CronJobRaw>,
}

// jobs.toml 中单个 [[job]]
#[derive(Debug, Clone, Deserialize)]
pub struct CronJobRaw {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub cron: String,
    pub prompt: String,
    #[serde(default)]
    pub agent_role: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default = "default_retry_delay")]
    pub retry_delay_secs: u64,
}

fn default_cron_jobs_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.config/xiaoo/cron", home)
}
fn default_max_concurrent() -> usize { 3 }
fn default_cron_timeout() -> u64 { 3600 }
fn default_true() -> bool { true }
fn default_retry_delay() -> u64 { 60 }
```

### 5.6 `DaemonConfig` 新增方法

```rust
impl DaemonConfig {
    /// 如果 config.toml 有 [cron] 段则返回，否则 None
    pub fn cron_section(&self) -> Option<&CronSectionRaw> {
        self.app.cron.as_ref()
    }

    /// 加载 jobs.toml → 解析 cron → 合并全局默认 → 返回 CronJobConfig 列表
    pub fn resolve_cron_jobs(&self) -> Result<Vec<CronJobConfig>> {
        let global = match self.cron_section() {
            Some(cfg) => cfg,
            None => return Ok(Vec::new()),
        };

        let jobs_dir = shellexpand::tilde(&global.jobs_dir).into_owned();
        let jobs_file = Path::new(&jobs_dir).join("jobs.toml");

        if !jobs_file.exists() {
            tracing::info!(path = %jobs_file.display(), "jobs.toml not found, no jobs loaded");
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&jobs_file)?;
        let raw: CronJobsFileRaw = toml::from_str(&content)?;

        let mut jobs = Vec::with_capacity(raw.job.len());
        for raw_job in raw.job {
            let cron = CronExpression::parse(&raw_job.cron)
                .map_err(|e| anyhow::anyhow!("job '{}': {}", raw_job.name, e))?;
            let timeout_secs = raw_job.timeout_secs.unwrap_or(global.default_timeout_secs);
            jobs.push(CronJobConfig {
                name: raw_job.name,
                description: raw_job.description,
                cron,
                prompt: raw_job.prompt,
                agent_role: raw_job.agent_role,
                timeout_secs,
                enabled: raw_job.enabled,
                max_retries: raw_job.max_retries,
                retry_delay_secs: raw_job.retry_delay_secs,
            });
        }
        Ok(jobs)
    }
}
```

---

## 6. CronScheduler 核心实现

### 6.1 数据结构

```rust
// apps/serverside/src/cron/scheduler.rs

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

pub struct CronScheduler {
    cancel_token: CancellationToken,
    concurrency_limiter: Arc<Semaphore>,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

struct CronJob {
    config: CronJobConfig,
    session_service: Arc<dyn SessionService>,
    cancel_token: CancellationToken,
    concurrency_limiter: Arc<Semaphore>,
    next_run: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
    running: AtomicBool,
    last_result: Mutex<Option<CronLastResult>>,
    /// 触发次数（含手动 trigger_now）
    trigger_count: AtomicU64,
    /// 成功执行次数
    success_count: AtomicU64,
    /// 重试耗尽后仍失败的次数
    failure_count: AtomicU64,
}
```

字段定义见 `apps/serverside/src/cron/scheduler.rs:140-154`。

> **与旧版 Spec 的差异**：不存在 `last_run` / `run_count`；实际字段是
> `next_run` / `running` / `last_result` / `trigger_count` / `success_count` / `failure_count`。

### 6.2 构造与生命周期

公开 API 为 `new` / `catalog` / `trigger_now` / `stop`
（`apps/serverside/src/cron/scheduler.rs:29,83,93,112`）——**没有 `start()` 方法**；
`new()` 内部即为每个 enabled job spawn timer。

```rust
impl CronScheduler {
    /// 构造并立即为每个 enabled job spawn timer。
    pub fn new(
        jobs: Vec<CronJobConfig>,
        max_concurrent: usize,
        session_service: Arc<dyn SessionService>,
    ) -> Self {
        let cancel_token = CancellationToken::new();
        let limit = if max_concurrent > 0 { max_concurrent } else { usize::MAX };
        let concurrency_limiter = Arc::new(Semaphore::new(limit));

        let mut handles = Vec::new();
        let mut runtime_jobs = HashMap::new();
        for config in jobs {
            if !config.enabled {
                tracing::info!(job = %config.name, "cron job disabled, skipping");
                continue;
            }
            let job = Arc::new(CronJob { /* … 见上 */ });
            handles.push(Self::spawn_job_timer(job.clone()));
            runtime_jobs.insert(job.config.name.clone(), job);
        }

        Self { cancel_token, handles: Mutex::new(handles), jobs: runtime_jobs }
    }

    /// 返回所有 enabled job 的实时状态，按 name 排序。
    pub async fn catalog(&self) -> Vec<CronJobRuntimeResponse> { /* :94-101 */ }

    /// 立即触发一个 enabled job；不等待 agent turn 完成。
    /// 找不到 → TriggerError::NotFound；已在运行 → TriggerError::AlreadyRunning。
    pub async fn trigger_now(&self, name: &str)
        -> Result<CronJobRuntimeResponse, TriggerError> { /* :103-122 */ }

    /// 取消全部 timer 并等待其退出。
    pub async fn stop(&self) { /* :124-138 */ }
}
```

`trigger_now` 的错误类型见 `apps/serverside/src/cron/scheduler.rs:129-137`
（`TriggerError::{NotFound, AlreadyRunning}`）。

### 6.3 Timer 主循环

```rust
impl CronScheduler {
    fn spawn_job_timer(job: Arc<CronJob>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tracing::info!(job = %job.config.name, cron = %job.config.cron, "timer started");

            loop {
                // 1. 计算下一次触发时间
                let now = chrono::Utc::now();
                let Some(next) = job.config.cron.next_after(now) else {
                    tracing::error!(job = %job.config.name, "no future match, stopping");
                    break;
                };
                *job.next_run.lock().await = Some(next);

                let wait = match (next - now).to_std() {
                    Ok(d) if d > Duration::ZERO => d,
                    _ => Duration::ZERO,
                };

                tracing::info!(
                    job = %job.config.name,
                    next = %next.format("%Y-%m-%dT%H:%M:%SZ"),
                    wait_secs = wait.as_secs(),
                    "waiting"
                );

                // 2. 等待或取消
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = job.cancel_token.cancelled() => {
                        tracing::info!(job = %job.config.name, "cancelled");
                        break;
                    }
                }

                // 3. 获取并发许可
                let _permit = match job.concurrency_limiter.acquire().await {
                    Ok(p) => p,
                    Err(_) => break,
                };

                // 4. 执行
                execute_job_with_retry(&job).await;
            }
        })
    }
}
```

### 6.4 执行与重试

```rust
async fn execute_job_with_retry(job: &CronJob) {
    let max_attempts = job.config.max_retries.saturating_add(1);

    for attempt in 1..=max_attempts {
        match execute_job_once(job).await {
            Ok(result) => {
                tracing::info!(
                    job = %job.config.name, attempt,
                    session = %result.session_id,
                    tokens = %result.total_tokens,
                    duration_ms = %result.duration_ms,
                    "completed"
                );
                return;
            }
            Err(error) if attempt < max_attempts => {
                tracing::warn!(
                    job = %job.config.name, attempt, max_attempts,
                    error = %error,
                    "failed, retrying in {}s", job.config.retry_delay_secs
                );
                job.failure_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(job.config.retry_delay_secs)) => {}
                    _ = job.cancel_token.cancelled() => return;
                }
            }
            Err(error) => {
                tracing::error!(
                    job = %job.config.name, attempt, max_attempts,
                    error = %error,
                    "permanently failed"
                );
                job.failure_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    }
}

struct JobRunResult {
    session_id: String,
    total_tokens: u64,
    duration_ms: u64,
}

async fn execute_job_once(job: &CronJob) -> Result<JobRunResult, CronExecutionError> {
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let session_id = format!("cron-{}-{}", job.config.name, ts);
    let conversation_id = format!("cron-{}-conv", job.config.name);

    // 单次 run_turn：不存在 open_session / close_session 步骤
    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext {
            kind: Some(GatewayEntryKind::ScheduledJob),
            runtime_profile_id: job.config.agent_role.clone(),
            ..Default::default()
        },
        channel: None,
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        conversation_id,
        sender_id: format!("cron/{}", job.config.name),
        text: job.config.prompt.clone(),
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: vec![],
        reasoning_effort: None,
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: Some(daemon_cron_principal()),
    };

    let start = std::time::Instant::now();

    let result = tokio::time::timeout(
        Duration::from_secs(job.config.timeout_secs),
        job.session_service.run_turn(request),
    )
    .await
    .map_err(|_| CronExecutionError::Timeout {
        job_name: job.config.name.clone(),
        timeout_secs: job.config.timeout_secs,
    })?
    .map_err(|e| CronExecutionError::Session {
        job_name: job.config.name.clone(),
        error: e.to_string(),
    })?;

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(JobRunResult {
        session_id,
        reply: result.reply,
        total_tokens: result.total_tokens,
        duration_ms,
    })
}
```

---

## 7. `main.rs` 集成

```rust
// apps/serverside/src/main.rs

// ---- 启动 Cron Scheduler ----
// CronScheduler::new() 内部已为每个 enabled job spawn timer，
// 不存在单独的 start() 方法。
let cron_scheduler = match config.resolve_cron_jobs() {
    Ok(jobs) if !jobs.is_empty() => {
        let enabled_count = jobs.iter().filter(|j| j.enabled).count();
        if enabled_count > 0 {
            let scheduler = Arc::new(CronScheduler::new(
                jobs,
                config.app.cron.max_concurrent_jobs,
                session_service.clone(),
            ));
            tracing::info!(enabled = enabled_count, "cron scheduler initialized");
            Some(scheduler)
        } else {
            tracing::info!("cron section present but no jobs configured");
            None
        }
    }
    Ok(_) => None,
    Err(error) => {
        // ⚠ 任一 job 非法都会走到这里：整个 cron 被禁用，无任何 job 加载
        tracing::error!(%error, "failed to load cron jobs, cron disabled");
        None
    }
};
```

对应实现位置：`apps/serverside/src/main.rs:629-656`（resolve 与错误分支）；
`CronScheduler::new` 定义于 `apps/serverside/src/cron/scheduler.rs:29-68`。

### 7.1 优雅关闭

Daemon 只响应 **SIGINT / SIGTERM**（`apps/serverside/src/main.rs:876-900`），
在该路径上调用 `CronScheduler::stop()` 取消全部 timer 并等待其退出
（`apps/serverside/src/main.rs:746-747`；实现见
`apps/serverside/src/cron/scheduler.rs:112-125`）。

> **无 SIGHUP**：代码中不存在 `SignalKind::hangup()`。修改 `jobs.toml` 后唯一生效方式是
> **重启 daemon**。

---

## 8. Trace / 可观测性

### 8.1 Span 层级

```
cron:scheduler
└── cron:job:{name}
    ├── cron:attempt:1
    │   └── session:run_turn       # 单次调用，无 open/close
    │       └── agent_loop         # 现有 trace
    │           ├── turn:1
    │           └── turn:2 ...
    └── cron:attempt:2             # 重试
        └── ...
```

### 8.2 日志格式

```
[cron:scheduler] loading jobs from ~/.config/xiaoo/cron/jobs.toml
[cron:scheduler] 3 jobs loaded, 2 enabled
[cron:morning-standup] timer started, next run in 8h 23m (2026-01-15T09:00:00Z)
[cron:morning-standup] triggered
[cron:morning-standup] opened session cron-morning-standup-20260115T090000
[cron:morning-standup] attempt 1 completed: 3200 tokens, 3 turns, 45s
[cron:morning-standup] next run in 23h 59m (2026-01-16T09:00:00Z)
```

---

## 9. 依赖

在 workspace `Cargo.toml` 添加：

```toml
[workspace.dependencies]
cron = "0.13"
shellexpand = "3"
```

`agent-types/Cargo.toml`：
```toml
[dependencies]
cron.workspace = true
chrono.workspace = true
```

`apps/serverside/Cargo.toml`：
```toml
[dependencies]
cron.workspace = true
shellexpand.workspace = true
```

---

## 10. 错误处理策略

| 阶段 | 情况 | 行为 |
|---|---|---|
| 启动 | `[cron]` 段不存在 | daemon 正常启动，不启用 cron；`Ok(vec![])`（`apps/serverside/src/daemon_config.rs:963-966`） |
| 启动 | `jobs.toml` 不存在 | 日志 info，`Ok(vec![])`，daemon 正常启动（`apps/serverside/src/daemon_config.rs:971-974`） |
| 启动 | `jobs.toml` 语法错误 | `Err` → 日志 error，**cron 全部禁用**，daemon 正常启动（`apps/serverside/src/daemon_config.rs:978-979`） |
| 启动 | 单个 job cron 表达式无效 | `Err` → **整个 resolve_cron_jobs 失败**，全部 job 不加载（`apps/serverside/src/daemon_config.rs:989-990`）⚠ |
| 启动 | job 名称重复 | 同上，`bail!("duplicate cron job name")`（`apps/serverside/src/daemon_config.rs:985-987`）⚠ |
| 运行 | Job 执行超时 | 标记 Timeout，按配置重试（`apps/serverside/src/cron/scheduler.rs:347-405`） |
| 运行 | Job 并发达到上限 | Semaphore 排队等待（`apps/serverside/src/cron/scheduler.rs:278`） |
| 运行 | 手动触发未找到/已在运行 | `TriggerError::NotFound` / `AlreadyRunning`（`apps/serverside/src/cron/scheduler.rs:129-137`） |
| 关闭 | SIGINT/SIGTERM | CancellationToken 取消所有 timer（`apps/serverside/src/cron/scheduler.rs:112-125`） |

**核心原则**：Fail Open — cron 解析失败不阻止 daemon 启动。

> ⚠ **已知缺口**：**单个坏 job 并非"跳过该 job"**。旧 Spec 写的「日志 error，跳过该 job」
> 不成立：`resolve_cron_jobs()` 对任一条目非法都直接 `bail!`，导致**所有** job 都不加载。
> 排障请先运行 `xiaoo-daemon config cron` 查看 `errors` 字段（§13.5）。

---

## 11. 实现计划

| 阶段 | 任务 | 预估 |
|---|---|---|
| P1 | 添加 cron / shellexpand 依赖 | 小 |
| P2 | `agent-types::cron` 模块（expression + config + error）| 中 |
| P3 | `daemon_config` 扩展（CronSectionRaw + resolve_cron_jobs）| 中 |
| P4 | `CronScheduler` 实现（timer 循环 + 重试 + 信号量）| 大 |
| P5 | `main.rs` 集成 | 小 |
| P6 | 单元测试 + 集成测试 | 中 |

---

## 12. 快速开始示例

```bash
# 1. 创建 jobs.toml
mkdir -p ~/.config/xiaoo/cron
cat > ~/.config/xiaoo/cron/jobs.toml << 'EOF'
[[job]]
name = "daily-summary"
description = "每天 18:00 生成工作总结"
cron = "0 18 * * *"
prompt = "生成今日工作总结，写入 docs/daily/$(date +%Y-%m-%d).md"
EOF

# 2. 在主配置中启用 cron
cat >> ~/.config/xiaoo/config.toml << 'EOF'

[cron]
max_concurrent_jobs = 2
default_timeout_secs = 1800
EOF

# 3. 启动 daemon
xiaoo-daemon
# [cron:scheduler] loading jobs from ~/.config/xiaoo/cron/jobs.toml
# [cron:scheduler] 1 jobs loaded, 1 enabled
# [cron:daily-summary] timer started, next run in 4h 15m

# 4. 修改任务
vim ~/.config/xiaoo/cron/jobs.toml            # 编辑
systemctl restart xiaoo-daemon                # 重启使其生效
# 注意：SIGHUP 不会触发重载 —— daemon 仅处理 SIGINT/SIGTERM，
# 且只在启动时解析一次 jobs.toml（apps/serverside/src/main.rs:629-656, 876-900）。
```

### 12.1 通过 HTTP 查看 / 手动触发

```bash
# 查看 job 目录与运行状态
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:18080/api/v1/cron/jobs

# 立即触发一个 enabled job
curl -X POST -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' \
     -d '{"name":"daily-summary"}' \
     http://127.0.0.1:18080/api/v1/cron/run
```

### 12.2 通过 CLI 校验 / 渲染

```bash
# job 配置报告（含 valid / errors）
xiaoo-daemon config cron

# 校验并渲染草稿 JSON 为 jobs.toml 文本
echo '{"jobs":[{"name":"daily","cron":"0 18 * * *","prompt":"摘要"}]}' \
  | xiaoo-daemon config render-cron
```

---

## 13. HTTP / CLI 运维面

本章描述的接口在原 Spec 中完全缺失，属于已实现的功能。

### 13.1 HTTP 路由

两条路由都挂在 **bearer 保护**的路由组下
（`apps/serverside/src/httpserver/router.rs:611-612`，组定义 `:575`）：

| 方法 | 路径 | 处理函数 | 说明 |
|------|------|---------|------|
| `GET` | `/api/v1/cron/jobs` | `handle_cron_catalog` | 返回 job 目录与实时运行状态 |
| `POST` | `/api/v1/cron/run` | `handle_cron_run` | 立即触发一个 enabled job |

请求体（`POST /api/v1/cron/run`）为 `CronRunRequest`
（`crates/protocol/src/wire.rs:485-490`）：

```json
{ "name": "daily-summary" }
```

`name` 必须匹配 `jobs.toml` 中一个 **enabled** 的 job；不存在返回 `404`，
已在运行返回 `409`（对应 `TriggerError::{NotFound, AlreadyRunning}`，
`apps/serverside/src/cron/scheduler.rs:129-137`）。

> 触发是**非阻塞**的：`trigger_now()` 立即返回 job 的运行状态快照，
> 不等待 agent turn 结束（`apps/serverside/src/cron/scheduler.rs:93-110`）。

### 13.2 CLI

| 命令 | 行为 | 位置 |
|------|------|------|
| `xiaoo-daemon config cron [--config <path>]` | 打印 `CronReport` JSON：`configured`/`valid`/`jobs_file`/`jobs`/`errors` | `apps/serverside/src/main.rs:467-475` |
| `xiaoo-daemon config render-cron < draft.json` | 从 stdin 读草稿 JSON，校验并渲染为 jobs.toml 文本 | `apps/serverside/src/main.rs:476-486` |

用法文本见 `apps/serverside/src/main.rs:1336-1337`。

`render-cron` 是**任务编辑的官方入口**：它接收 `CronJobsDraft`，逐条校验
（名称非空、名称唯一、cron 表达式合法等），返回
`CronRenderReport { valid, content, errors }`
（`apps/serverside/src/cron_management.rs:75-143`）。

### 13.3 Capability 域

cron 在 daemon 的 capability 清单中声明为独立域，支持的操作为
**`list` / `validate` / `render` / `runtime_status` / `run_now`**
（`apps/serverside/src/management_capabilities.rs:137-144`）。可用
`GET /api/v1/capabilities` 查询（`apps/serverside/src/httpserver/router.rs:619`）。

### 13.4 动态增删任务（实际能力）

原 Spec 的「不支持动态添加/删除任务」已不准确：

- **TUI 编辑器**：daemon 模式下 TUI 的 Cron 服务可直接写入 `jobs.toml`
  （`apps/endside/src/services/cron_service.rs:144-187`）。
- **CLI 草稿渲染**：`config render-cron` 生成规范的 jobs.toml 文本（§13.2）。
- **HTTP 手动触发**：`POST /api/v1/cron/run` 可在不重启的情况下触发任务（§13.1）。

> **已知缺口**：但**加载仍是一次性的**。`resolve_cron_jobs()` 只在 daemon 启动时执行，
> 且没有 SIGHUP/热重载路径（`apps/serverside/src/main.rs:629-656,876-900`）。
> 因此新增或修改 job 后仍需**重启 daemon**；`trigger_now` 只能触发启动时已加载的 job。

### 13.5 配置错误处理（重要）

`resolve_cron_jobs()` 是**全有或全无**：任一 job 条目非法都会使整个函数返回 `Err`
（重复名称或 cron 表达式无法解析即 `bail!`，
`apps/serverside/src/daemon_config.rs:980-989`），daemon 随后记录
`failed to load cron jobs, cron disabled` 并**禁用全部 cron job**
（`apps/serverside/src/main.rs:653-656`）。

> **已知缺口**：单个坏 job 会导致**所有** job 静默停摆。排障请先运行
> `xiaoo-daemon config cron` 查看 `errors` 字段。

---

## 附录：关键类型速查

| 类型 | 位置 | 说明 |
|---|---|---|
| `CronExpression` | `agent-types::cron::expression` | 已验证的 cron 表达式 |
| `CronParseError` | `agent-types::cron::error` | 解析错误 |
| `CronExecutionError` | `agent-types::cron::error` | 执行错误 |
| `CronJobConfig` | `agent-types::cron::config` | 合并后的运行时配置 |
| `CronGlobalConfig` | `agent-types::cron::config` | 全局设置 |
| `CronSectionRaw` | `daemon_config` | `[cron]` 段反序列化 |
| `CronJobsFileRaw` | `daemon_config` | `jobs.toml` 顶层结构 |
| `CronJobRaw` | `daemon_config` | 单个 `[[job]]` 反序列化 |
| `CronScheduler` | `app/cron/scheduler` | 调度器 |
| `CronJob` | `app/cron/scheduler` | 运行时 job 状态 |
