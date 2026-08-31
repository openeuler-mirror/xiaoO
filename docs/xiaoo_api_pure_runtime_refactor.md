# xiaoo-api Runtime SDK 化提案（RFC）

> 状态：提案草案，未评审。  
> 范围：只讨论 `crates/xiaoo-api` 的公开边界及其直接依赖。  
> 明确不讨论：`apps/*` 的迁移方式、daemon/TUI/HTTP 的组装、控制面如何消费 SDK。

## 0. 结论

`Host` 不是一个应该修补的抽象，而是一个不应存在于 xiaoo SDK 公开面的抽象。

xiaoo-api 对外只需要提供：

1. agent runtime 的构造和驱动；
2. 工具与沙箱实例的接入契约；
3. 对话环路需要的状态、输入、输出和事件契约；
4. 少量、按具体需求开放的 extension 接口。

不需要再建立 `LocalSessionHost` 的替代品，也不需要把现有 L2 装配层公开出来。
`AppBootstrap`、resolver、store、lease、backend manager 等都不是 runtime SDK。

目标调用形状应接近：

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

## 1. 问题不在于 Host 太重，而在于 Host 没有必要

当前 xiaoo-api 把公开门面描述为：

```text
LocalSessionHost -> open_session -> Session -> run_turn
```

这条链路隐含了一个前提：SDK 必须拥有会话服务的进程级宿主。这个前提不成立。

agent runtime 真正需要的只是：

```text
Runtime configuration
    + caller-owned runtime state
    + one turn input
    + injected operation backend
    -> agent loop output
```

`Host` 引入的进程级所有权没有给决策环路增加能力，只把以下服务层概念带入 SDK：

- 多会话容器及其关闭流程；
- `SessionStore` 持久化；
- client id/pid/hostname 租约字段；
- backend 池化、租借、释放与驱逐；
- checkpoint 与 fork lineage；
- bootstrap/resolver/control-plane 装配。

因此改造方向不是把 `LocalSessionHost` 政名为 `XiaooRuntimeHost`，也不是在 Host 下面
再抽一层 service，而是从 xiaoo-api 的 SDK 面删除 Host 这一层。

## 2. Runtime SDK 的边界

采用一个简单判据：

> 决定一次 agent loop 如何运行的依赖属于 runtime；管理多个会话、客户端或执行环境的生命周期不属于 runtime。

### 2.1 SDK 应公开

| 能力 | SDK 形态 |
|---|---|
| runtime 构造 | `Runtime` / `RuntimeBuilder` |
| turn 驱动 | `Runtime::run(&mut RuntimeState, RuntimeInput)` |
| loop 状态 | `RuntimeState`，由调用方持有，可序列化的状态沿用 core 能力 |
| LLM | provider 实例或 provider contract 注入 |
| 工具 | `ToolRegistry` / `ToolSource` 等纯 runtime contract |
| 沙箱 | `Arc<dyn OperationBackend>` 实例注入 |
| skills | `SkillRegistry` 注入 |
| prompt/compact | runtime 所需的 builder 参数与标准实现 |
| 事件/交互 | `LoopEventSink`、`ToolEventSink`、`InteractionHandle` |
| 定制能力 | 窄的、语义明确的 extension 接口 |

### 2.2 SDK 不应公开

| 能力 | 原因 |
|---|---|
| `LocalSessionHost` / `HostInner` | 进程级会话服务容器 |
| `SessionControlPlane` / `SessionService` | 服务控制面 |
| `SessionStore` 及实现 | 持久化基础设施 |
| client lease/reaper/paused/eviction | 多客户端会话治理语义 |
| `BackendManager` / sandbox counter | 执行环境治理 |
| `GatewayBackendConfig` | daemon 的 config-to-instance 词汇，不是 runtime 依赖 |
| `AppBootstrap` / `AppDependencies` | 应用装配入口 |
| `HostedSessionRuntimeResolver` | 服务层的配置解析与恢复入口 |
| `SessionRuntimeResolver` 家族 | L2 组装协议，不是 SDK 使用协议 |

这里没有“把 L2 全部标成 advanced 后继续导出”的折中。L2 不属于 SDK，就不从
xiaoo-api 导出。确有定制需求时，按需求增加 extension，而不是把内部装配结构冻结成公共 API。

## 3. 工具沙箱的正式接入方式

### 3.1 只接受实例，不接受 kind/config

runtime builder 的标准入口是：

```rust
pub fn operation_backend(
    self,
    backend: Arc<dyn OperationBackend>,
) -> Self;
```

不提供以下平行入口：

```rust
backend_kind("e2b")
backend_config(GatewayBackendConfig { ... })
backend_manager(...)
```

原因是 runtime 只需要执行操作，不应知道实例如何创建、是否来自池、如何计费、何时驱逐。
这些差异在传入 `Arc<dyn OperationBackend>` 前已经被调用方解决。

### 3.2 xiaoo-api 公开 operation plane，不公开 lifecycle plane

`xiaoo_api::backend` 应从 `agent-contracts` 重导出运行工具所需的合同：

- `OperationBackend` / `OperationBackendCapabilities`；
- path/filesystem/search/exec/export 五组能力 trait 与请求/响应类型；
- `OperationError` / `ExecutionState`；
- 权限询问所需的 permission contract。

不重导出：

- `BackendLifecycle` / `BackendProvider`；
- create/load/pause/delete/inspect 等 lifecycle DTO；
- manager、quota、counter、checkpoint 类型；
- config-to-instance builder 合同，除非未来有明确的 SDK 消费场景。

这条分界使“工具如何操作沙箱”成为稳定 SDK 合同，同时不把“沙箱如何被治理”带进来。

### 3.3 backend 所有权

建议采用明确合同：runtime 共享使用 backend，但不拥有其外部生命周期。

- runtime 不创建 backend；
- runtime 不替调用方池化或驱逐 backend；
- runtime drop 不隐式销毁远端沙箱；
- `OperationBackend::shutdown` 是否继续存在于 operation contract 可单独评审，但
  runtime 不应自动调用它；
- 相同 backend 是否跨多个 `RuntimeState` 共享由调用方决定。

## 4. Runtime 公开模型

### 4.1 `Runtime`

`Runtime` 表示可复用的 agent 决策环路配置，而不是会话容器。它持有：

- LLM provider；
- prompt builder；
- compression pipeline 与 token budget policy；
- tool registry；
- skill registry；
- feature flags、system prompt、max turns；
- 可选的 operation backend；
- 已注册的 runtime extensions。

它不持有 session store、lease table、backend pool 或后台 reaper。

现有 `xiaoo_core::AgentRuntime` 已经非常接近这个形状。优先方案是由 xiaoo-api 提供
稳定、收敛的 facade/re-export，而不是复制一份新的 runtime 实现。

### 4.2 `RuntimeState`

`RuntimeState` 是单条对话环路的可变状态，对应 core 的 `LoopState`：messages、turn count、
token usage、compression metadata、kv cache 等。

状态由调用方创建和持有：

```rust
let mut state = RuntimeState::new(conversation_id);
runtime.run(&mut state, input).await?;
```

SDK 可以公开 core 已有的 snapshot 转换，但不提供 `SessionStore`。序列化结果放在哪里、
何时保存、如何做版本迁移是调用方的持久化策略，不应反向塑造 runtime API。

### 4.3 `RuntimeInput` / `RuntimeOutput`

`RuntimeInput` 应是 agent loop 的单次调用输入，包含：

- user message；
- event sink；
- interaction handle；
- reasoning effort；
- visible tools 或工具过滤策略；
- cancellation / pending input 等纯 loop 能力。

`RuntimeOutput` 沿用 core 的 complete/suspend/error 语义，不映射成 daemon 的 turn DTO。
若现有 `AgentLoopInput` / `LoopRunResult` 已足够稳定，xiaoo-api 直接收敛重导出即可，
不要再增加一套同义 DTO。

### 4.4 Builder 的层级

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
    .extension(...)
    .build()
```

可以为 prompt、compact、empty tools/skills 提供标准默认实现，但不自动进行服务发现、
读取应用配置、创建沙箱或恢复持久化会话。

## 5. Extension，而不是公开 L2

extension 的目的，是让特定集成添加特定能力，同时保持基础 runtime API 小而稳定。

### 5.1 原则

1. extension 必须围绕 runtime 语义命名，不能成为 `AppBootstrap` 的新名字；
2. 不公开 store、lease、resolver、manager 等服务层对象；
3. 不提供无类型的 service locator 或任意 JSON 装配袋；
4. 优先增加窄接口，例如 tool source、runtime view decorator、event observer；
5. 只有出现真实使用者时才增加新的 extension capability。

### 5.2 初始接口建议

第一版只需要一个安装入口：

```rust
pub trait RuntimeExtension: Send + Sync {
    fn install(
        &self,
        registrar: &mut RuntimeExtensionRegistrar<'_>,
    ) -> Result<(), ExtensionError>;
}
```

`RuntimeExtensionRegistrar` 不暴露整个内部 assembly，只提供经过评审的窄方法，例如：

```rust
registrar.add_tool_source(...);
registrar.decorate_runtime_view(...);
registrar.observe_events(...);
```

具体方法应随首个真实 extension 一起确定。若当前没有真实 extension 消费者，阶段一可以
只预留 `.extension(...)` 的设计位置而暂不提交空泛 trait，避免提前冻结错误接口。

### 5.3 标准依赖不是 extension

LLM、operation backend、tools、skills、prompt、compact 都是 runtime 的正常依赖，
应该有明确 builder 方法，不应被塞进 extension。extension 只承载无法成为通用核心参数的
定制能力。

## 6. xiaoo-api 模块重划

建议公开模块收敛为：

```text
xiaoo_api
├── prelude
├── runtime       # Runtime、builder、state、input/output、run
├── backend       # operation plane contracts
├── tools         # registry/source contracts与标准实现
├── skills        # registry contracts与标准实现
├── llm           # provider contracts与构造辅助
├── events        # loop/tool event contracts
├── interaction   # runtime interaction contracts
└── extension     # 有真实需求后开放的窄扩展点
```

以下模块不应继续作为 runtime SDK 公共模块：

```text
host
session
memory                 # 当前是服务连接与健康管理语义
runtime::wire
runtime 中的 bootstrap/resolver exports
backend 中的 BackendManager/GatewayBackendConfig exports
sse                    # HTTP client protocol，不属于 runtime SDK
```

如果 wire/client API 仍需独立发布，应进入单独的 protocol/client crate；不要因为历史兼容
继续让 xiaoo-api 同时扮演 runtime SDK 和 daemon client contract 包。

## 7. 依赖纪律

最重要的可机械验证规则：

> `crates/xiaoo-api` 不得依赖 `apps/shared` 或任何 `apps/*` crate。

runtime SDK 的依赖方向只能指向可复用的 lower-level crates，例如：

- `xiaoo-core`；
- `agent-contracts` / `agent-types`；
- `llm-client`；
- `tool` / `skill` / `prompt` / `compact`；
- 其他明确属于 runtime 的基础 crate。

只要 xiaoo-api 的 `Cargo.toml` 还包含：

```toml
xiaoo-shared = { path = "../../apps/shared" }
```

就说明 API 边界仍然被应用/服务层反向塑造，改造尚未完成。

## 8. 建议落地顺序（仅 xiaoo-api）

### 阶段 1：公开纯 runtime 最小面

- 从 `runtime` 模块重导出/封装 core 的 runtime、state、input/output 和 run；
- 从 `backend` 模块公开 `OperationBackend` operation plane；
- 提供 backend 实例注入入口；
- 添加一个自定义 backend 的 compile test；
- 不添加 extension，除非已有具体扩展需求和调用示例。

验收：不经过 `LocalSessionHost`、`SessionStore`、`BackendManager` 即可执行一轮带工具调用的 agent loop。

### 阶段 2：收敛 facade

- 为常用 prompt/compact/tool/skill 组装提供 runtime builder 默认值；
- `prelude` 只导出纯 runtime 用例实际需要的名字；
- 用一个可运行示例替换现有 `minimal_chat` 的 host/session 流程。

验收：示例中不存在 host、open/close session、lease 或 backend kind/config。

### 阶段 3：移除服务层公开面

- 删除 `pub mod host`；
- 删除 session/control-plane/store exports；
- 删除 `AppBootstrap`、hosted resolver 等 L2 exports；
- 删除 `BackendManager` / `GatewayBackendConfig` exports；
- 删除 `xiaoo-shared` 依赖；
- wire/client 模块另行归位，不在本 RFC 中设计其新位置。

验收：`cargo tree -p xiaoo-api` 中没有 `xiaoo-shared`，公开文档中没有 Host、lease、store、eviction、control plane。

### 阶段 4：按实际需求增加 extension

- 收集第一个无法由标准 builder 参数表达的真实定制需求；
- 为该需求增加最窄 extension capability；
- 用外部实现测试证明不需要访问内部 assembly 或 L2 resolver。

验收：extension API 中没有 service locator、gateway config 或 daemon 生命周期类型。

## 9. 公开面验收清单

- [ ] `xiaoo-api` 不依赖 `apps/*`；
- [ ] 最小用例从 `Runtime` 开始，而不是从 `Host` 开始；
- [ ] 调用方能直接传入 `Arc<dyn OperationBackend>`；
- [ ] runtime 不根据 kind/config 创建或治理 sandbox；
- [ ] 对话状态由调用方持有；
- [ ] SDK 不公开 SessionStore、lease、paused、eviction、checkpoint；
- [ ] SDK 不公开 AppBootstrap 或 resolver 家族；
- [ ] 标准 runtime 依赖使用明确 builder 方法；
- [ ] 定制能力通过窄 extension 开放，不通过 L2 泄漏；
- [ ] 没有为了替代 `LocalSessionHost` 而新造另一个 Host。

## 10. 一句话架构定义

> xiaoo-api 是 agent runtime SDK：调用方给它模型、工具和一个可执行操作的沙箱实例，
> 再用调用方持有的状态驱动 agent loop；除此之外的会话服务与基础设施治理，都不属于它。
