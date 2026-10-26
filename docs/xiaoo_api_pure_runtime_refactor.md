# xiaoo-api Runtime SDK Architecture

> 范围：本文档描述 `crates/xiaoo-api` 的公开边界、模块布局、Runtime 契约与依赖规则。  
> 明确不涉及：`apps/*` 的组装方式、daemon/TUI/HTTP 的装配、控制面如何消费 SDK。

## 1. 定位

xiaoo-api 是 agent runtime SDK。调用方给它模型、工具和一个可执行操作的沙箱实例，
再用调用方持有的状态驱动 agent loop；除此之外的会话服务与基础设施治理，都不属于它。

SDK 对外只提供：

1. agent runtime 的构造和驱动；
2. 工具与沙箱实例的接入契约；
3. 对话环路需要的状态、输入、输出和事件契约；
4. 少量、按具体需求开放的 extension 接口（当前未冻结，见 §7）。

`AppBootstrap`、resolver、store、lease、backend manager 等服务层概念都不是 runtime SDK，
不从 xiaoo-api 导出。

目标调用形状：

```rust
use std::sync::Arc;
use xiaoo_api::prelude::*;

let runtime = Runtime::builder()
    .llm_provider(llm_provider)
    .operation_backend(Arc::new(my_sandbox))
    .tools(tool_registry)
    .skills(skill_registry)
    .build()?;

let mut state = RuntimeState::new("conversation-id");
let output = runtime
    .run(&mut state, RuntimeInput::new("分析当前工作区"))
    .await?;
```

这里没有 host、open session、client lease 或 backend kind。调用方已经持有一个沙箱实例，
SDK 只把它接进 agent 的工具执行路径。

## 2. SDK 边界

采用一个简单判据：

> 决定一次 agent loop 如何运行的依赖属于 runtime；管理多个会话、客户端或执行环境的生命周期不属于 runtime。

### 2.1 SDK 公开

| 能力 | SDK 形态 |
|---|---|
| runtime 构造 | `Runtime` / `RuntimeBuilder`（`crates/xiaoo-api/src/runtime.rs`） |
| turn 驱动 | `Runtime::run(&mut RuntimeState, RuntimeInput)`（`runtime.rs`） |
| loop 状态 | `RuntimeState`，由调用方持有，可序列化的状态沿用 core 能力 |
| LLM | provider 实例或 provider contract 注入 |
| 工具 | `ToolRegistry` / `ToolSource` 等纯 runtime contract |
| 沙箱 | `Arc<dyn OperationBackend>` 实例注入 |
| skills | `SkillRegistry` 注入 |
| prompt/compact | runtime 所需的 builder 参数与标准实现 |
| 事件/交互 | `LoopEventSink`、`ToolEventSink`、`InteractionHandle` |

### 2.2 SDK 不公开

以下服务层概念全部位于 `apps/shared`，不从 xiaoo-api 导出：

| 能力 | 原因 |
|---|---|
| 进程级会话服务容器（host 形态） | SDK 不拥有会话服务的进程级宿主 |
| `SessionControlPlane` / `SessionService` | 服务控制面 |
| `SessionStore` 及实现 | 持久化基础设施 |
| client lease/reaper/paused/eviction | 多客户端会话治理语义 |
| `BackendManager` / sandbox counter | 执行环境治理 |
| `GatewayBackendConfig` | daemon 的 config-to-instance 词汇，不是 runtime 依赖 |
| `AppBootstrap` / `AppDependencies` | 应用装配入口 |
| `HostedSessionRuntimeResolver` | 服务层的配置解析与恢复入口 |
| `SessionRuntimeResolver` 家族 | L2 组装协议，不是 SDK 使用协议 |

L2 装配层不从 xiaoo-api 导出。确有定制需求时，按需求增加 extension，而不是把内部装配结构冻结成公共 API。

## 3. 模块布局

公开模块（`crates/xiaoo-api/src/lib.rs`）：

```text
xiaoo_api
├── prelude       # 纯 runtime SDK 的便捷导入（prelude.rs）
├── runtime       # Runtime、builder、state、input/output、run（lib.rs）
├── backend       # operation plane contracts（lib.rs）
├── chat          # 会话消息与 runtime 消费的通用小类型（lib.rs）
├── tools         # registry/source contracts与标准实现（lib.rs）
├── skills        # registry contracts与标准实现（lib.rs）
├── llm           # provider contracts与构造辅助（lib.rs）
├── events        # loop/tool event contracts（lib.rs）
└── interaction   # runtime interaction contracts（lib.rs）
```

`chat` 模块承载 `ChatMessage`、`ReasoningEffort`、`FeatureFlags` 等会话消息与配置小类型，
不含会话服务生命周期语义。

wire/client 协议类型位于独立 crate `crates/protocol`。xiaoo-api 不同时扮演 runtime SDK
和 daemon client contract 包。

## 4. prelude 导出

`crates/xiaoo-api/src/prelude.rs` 导出 9 个名字：

- runtime 面：`Runtime`、`RuntimeBuilder`、`RuntimeState`、`RuntimeInput`、`RuntimeOutput`；
- operation plane：`OperationBackend`、`OperationBackendCapabilities`、`OperationError`；
- 构造错误：`RuntimeBuildError`。

## 5. Runtime 公开模型

### 5.1 `Runtime`

`Runtime`（`runtime.rs`）表示可复用的 agent 决策环路配置，而不是会话容器。它持有：

- LLM provider；
- prompt builder；
- compression pipeline 与 token budget policy；
- tool registry；
- skill registry；
- feature flags、system prompt、max turns；
- 可选的 operation backend；
- 默认可见工具集。

它不持有 session store、lease table、backend pool 或后台 reaper。`Runtime::builder()`
（`runtime.rs`）是构造入口；`Runtime` 是对 `xiaoo_core::AgentRuntime` 的稳定 facade，
不是另一份 runtime 实现。

### 5.2 `RuntimeState`

`RuntimeState` 是单条对话环路的可变状态，对应 core 的 `LoopState`：messages、turn count、
token usage、compression metadata、kv cache 等。

状态由调用方创建和持有：

```rust
let mut state = RuntimeState::new(conversation_id);
runtime.run(&mut state, input).await?;
```

SDK 公开 core 已有的 snapshot 转换，但不提供 `SessionStore`。序列化结果放在哪里、
何时保存、如何做版本迁移是调用方的持久化策略，不反向塑造 runtime API。

### 5.3 `RuntimeInput` / `RuntimeOutput`

`RuntimeInput` 是 agent loop 的单次调用输入，包含：

- user message；
- event sink；
- interaction handle；
- reasoning effort；
- visible tools 或工具过滤策略；
- cancellation / pending input 等纯 loop 能力。

`RuntimeOutput` 沿用 core 的 complete/suspend/error 语义，不映射成 daemon 的 turn DTO。

`Runtime::run`（`runtime.rs`）以 `(&mut RuntimeState, RuntimeInput)` 驱动一轮。
工具可见集完全由 `input.visible_tools` 决定：空集表示本轮无工具；需要完整注册表时
由调用方经 `Runtime::visible_tools` 显式注入。

### 5.4 Builder 的层级

builder 只表达 runtime 依赖：

```rust
Runtime::builder()
    .llm_provider(...)
    .operation_backend(...)
    .tool_registry(...)
    .skill_registry(...)
    .prompt_builder(...)
    .compression_pipeline(...)
    .token_budget(...)
    .build()
```

`RuntimeBuilder`（`runtime.rs`）为 prompt、compact、empty tools/skills 提供标准默认
实现，但不自动进行服务发现、读取应用配置、创建沙箱或恢复持久化会话。

## 6. 工具沙箱的接入方式

### 6.1 只接受实例，不接受 kind/config

runtime builder 的标准入口是：

```rust
pub fn operation_backend(
    self,
    backend: Arc<dyn OperationBackend>,
) -> Self;
```

（`runtime.rs`）

不提供 `backend_kind("e2b")`、`backend_config(GatewayBackendConfig { ... })`、
`backend_manager(...)` 等平行入口。runtime 只需要执行操作，不需要知道实例如何创建、
是否来自池、如何计费、何时驱逐——这些差异在传入 `Arc<dyn OperationBackend>` 之前
已经由调用方解决。

### 6.2 公开 operation plane，不公开 lifecycle plane

`xiaoo_api::backend` 从 `agent-contracts` 重导出运行工具所需的合同：

- `OperationBackend` / `OperationBackendCapabilities`；
- path/filesystem/search/exec/export 五组能力 trait 与请求/响应类型；
- `OperationError` / `ExecutionState`；
- 权限询问所需的 permission contract。

不重导出：

- `BackendLifecycle` / `BackendProvider`；
- create/load/pause/delete/inspect 等 lifecycle DTO；
- manager、quota、counter、checkpoint 类型；
- config-to-instance builder 合同。

这条分界使「工具如何操作沙箱」成为稳定 SDK 合同，同时不把「沙箱如何被治理」带进来。

### 6.3 backend 所有权

runtime 共享使用 backend，但不拥有其外部生命周期：

- runtime 不创建 backend；
- runtime 不替调用方池化或驱逐 backend；
- runtime drop 不隐式销毁远端沙箱；
- runtime 不自动调用 `OperationBackend::shutdown`；
- 相同 backend 是否跨多个 `RuntimeState` 共享由调用方决定。

## 7. Extension

extension 的目的，是让特定集成添加特定能力，同时保持基础 runtime API 小而稳定。

原则：

1. extension 必须围绕 runtime 语义命名，不能成为 `AppBootstrap` 的新名字；
2. 不公开 store、lease、resolver、manager 等服务层对象；
3. 不提供无类型的 service locator 或任意 JSON 装配袋；
4. 优先增加窄接口，例如 tool source、runtime view decorator、event observer；
5. 只有出现真实使用者时才增加新的 extension capability。

LLM、operation backend、tools、skills、prompt、compact 都是 runtime 的正常依赖，
有明确 builder 方法，不塞进 extension。extension 只承载无法成为通用核心参数的定制能力。

当前没有真实 extension 消费者，因此 `lib.rs` 中没有 `extension` 模块——空泛的
trait 不会被提前冻结；extension capability 随第一个真实使用者一起引入，安装入口收敛为
窄的 `RuntimeExtensionRegistrar`（`add_tool_source` / `decorate_runtime_view` /
`observe_events` 一类方法），不暴露内部 assembly。

## 8. 依赖纪律

最重要的可机械验证规则：

> `crates/xiaoo-api` 不得依赖 `apps/shared` 或任何 `apps/*` crate。

runtime SDK 的依赖方向只能指向可复用的 lower-level crates。`crates/xiaoo-api/Cargo.toml`
的依赖为：

- 仓内 crate：`xiaoo-core`、`agent-types`、`agent-contracts`、`llm-client`、`compact`、
  `tool`、`skill`、`prompt`；
- 基础依赖：`anyhow`、`thiserror`、`tokio`。

没有 `xiaoo-shared`，也没有任何 `apps/*` 依赖。

## 9. 一句话架构定义

> xiaoo-api 是 agent runtime SDK：调用方给它模型、工具和一个可执行操作的沙箱实例，
> 再用调用方持有的状态驱动 agent loop；除此之外的会话服务与基础设施治理，都不属于它。
