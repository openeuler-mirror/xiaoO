# xiaoO Architecture Overview

Reviewable architecture reference for the code as it stands on master `a58d79f`.

> **Supersedes** `docs/xiaoO系统架构分析.pptx`, which predates the current workspace
> layout: that deck still describes `apps/xiaoo-app`, an eleven-crate-short inventory,
> and the removed `xiaoo-tui` / `xiaoo-app daemon` binaries. Where the deck and this
> document disagree, this document follows the code.

## 1. Applications

Everything user-facing lives under `apps/`. There are four members
(`Cargo.toml:3-6`).

| App | Crate | Role | Evidence |
| --- | --- | --- | --- |
| `apps/endside` | `xiaoo-endside` | Front end: terminal UI, one-shot CLI, and skill management | `apps/endside/Cargo.toml:2` |
| `apps/serverside` | `xiaoo-serverside` | Daemon: HTTP/REST API, channels, cron, and runtime control plane | `apps/serverside/Cargo.toml:2` |
| `apps/shared` | `xiaoo-shared` | Application assembly facade shared by front end and daemon | `apps/shared/Cargo.toml:2` |
| `apps/vault` | `vault` | Secrets/key custody: key providers, AES-GCM whitebox, and optional SDF/HSM backends | `apps/vault/src/lib.rs` |

`apps/shared` is the composition layer: it owns the gateway (`apps/shared/src/lib.rs:6`),
the runtime checkpoint store (`apps/shared/src/lib.rs:12`), channels, cron, MCP and LSP
support, and is the only place that binds configuration to concrete runtime instances.
`apps/endside` depends on it directly (`apps/endside/Cargo.toml:40`).

### Binary names

| Binary | Definition | Purpose |
| --- | --- | --- |
| `xiaoo` | `apps/endside/Cargo.toml:10-12` | Terminal UI by default |
| `xiaoo --cli run -p "..."` | same binary, CLI subcommand | One-shot non-interactive run |
| `xiaoo-daemon` | `apps/serverside/Cargo.toml:11-13` | Long-running daemon |

The binary is `xiaoo`, not `xiaoo-tui` or `xiaoo-app`.

## 2. Workspace Members

The workspace declares 23 members (`Cargo.toml:1-31`). The workspace version is `0.1.0`
(`Cargo.toml:34`).

| Layer | Crates |
| --- | --- |
| Contracts and types | `crates/agent-contracts`, `crates/agent-types` |
| Core runtime | `crates/core`, `crates/compact`, `crates/memory` |
| LLM access | `crates/llm-client`, `crates/agent-llm` |
| Capability plugins | `crates/tool`, `crates/skill`, `crates/subagent`, `crates/prompt`, `crates/hook` |
| Execution substrate | `crates/operation_backend`, `crates/lsp`, `crates/mcp` |
| Observability | `crates/trace`, `crates/trace/src/moirai` |
| SDK and wire | `crates/xiaoo-api`, `crates/protocol` |
| Applications | `apps/shared`, `apps/endside`, `apps/serverside`, `apps/vault` |

### Cerberus is out of the workspace

The Cerberus crates are **commented out** of the workspace
(`Cargo.toml:26-29`), together with their path dependencies (`Cargo.toml:77-78`). They
require an eBPF toolchain and are not built by default. A consequence worth knowing
before following `crates/cerberus/README.md`: because the crates are outside the
workspace and have no `[workspace]` of their own, `cargo install --path
crates/cerberus/cerberus-cli` cannot resolve a workspace root.

## 3. Runtime Layering

The runtime is layered from the pure decision loop outward to application assembly.

### 3.1 The SDK boundary: `xiaoo-api`

`crates/xiaoo-api` is the runtime SDK facade. Its public modules are `backend`, `chat`,
`events`, `interaction`, `llm`, `runtime`, `skills`, `tools`, and `prelude`
(`crates/xiaoo-api/src/lib.rs:11-21`). The crate documentation states the contract
explicitly: the caller owns conversation state and injects any sandbox instance, while
session stores, leases, backend managers, and gateway bootstrapping are deliberately
*not* part of the crate (`crates/xiaoo-api/src/lib.rs:3-6`).

Dependencies confirm the boundary: the manifest pulls only lower-level crates and has no
`apps/*` dependency (`crates/xiaoo-api/Cargo.toml:7-19`).

### 3.2 The decision loop: `crates/core`

| Module | Responsibility |
| --- | --- |
| `agent_loop.rs` | `run_agent_loop` — prompt build, LLM call, tool dispatch, compression, and hook invocation (`crates/core/src/lib.rs:11`) |
| `runtime.rs` | `AgentRuntime` and `AgentRuntimeBuilder` — reusable loop configuration (`crates/core/src/lib.rs:15`) |
| `loop_state.rs` | `LoopState` — mutable per-conversation state: messages, turn count, token usage, compression metadata, KV cache (`crates/core/src/loop_state.rs:11-24`) |
| `snapshot.rs` | `RuntimeSnapshot` — the frozen, per-turn view the loop reads (`crates/core/src/lib.rs:19`) |

`AgentRuntime` holds only loop configuration — LLM provider, tool registry, skill
registry, prompt builder, system prompt, and feature flags (`crates/core/src/runtime.rs:11-18`).
Mutations go through `RuntimePatch` / `replace_all` (`crates/core/src/runtime.rs:83-102`).
It holds no session store, lease table, backend pool, or reaper.

The loop reads a `RuntimeSnapshot` per turn, which is where the compression pipeline,
max turns, and token-budget policy are frozen
(`crates/core/src/snapshot.rs:10-19`).

### 3.3 Intermediate crates

- `crates/compact` implements staged context compression: `apply_microcompact` for stale
  tool pairs (`crates/compact/src/microcompact.rs:21`) and the `ContextManager` pipeline.
- `crates/memory` implements layered memory: live snapshots, session summaries, durable
  memory, and the SQLite + FTS5 + embedding semantic store.
- `crates/operation_backend` implements the sandbox/execution substrate behind
  `OperationBackend`.

## 4. Gateway Runtime Resolution

When the daemon admits a request, the gateway must turn configuration plus a session
record into a concrete, runnable agent runtime. That work lives in
`apps/shared/src/gateway/session_runtime/` and the adjacent resolver.

| Component | Path | Responsibility |
| --- | --- | --- |
| Descriptor and resolver trait | `apps/shared/src/gateway/session_runtime/resolver.rs` | `SessionRuntimeDescriptor` and `SessionRuntimeResolver` define *what* must be built |
| Runtime factory | `apps/shared/src/gateway/session_runtime/factory.rs` | `AppRuntimeFactory` assembles the concrete `Runtime`, tool registry, compression pipeline, and hooks |
| Binding projection | `apps/shared/src/gateway/session_runtime/bindings.rs` | `SessionRuntimeBindings` exposed from the module (`apps/shared/src/gateway/session_runtime/mod.rs:3`) |
| Hosted resolver | `apps/shared/src/gateway/hosted_runtime_resolver.rs` | The production resolver: composes workspace prompts, repo map, skills table, subagent roles, and tool sources into a resolvable runtime |

The factory is the seam where the SDK is actually used: it imports `Runtime` and related
types from `xiaoo_api::runtime` (`apps/shared/src/gateway/session_runtime/factory.rs:22-26`),
which is what keeps the SDK free of gateway knowledge while still powering it.

Note the real module names: `session_runtime/{factory,resolver}.rs` and
`hosted_runtime_resolver.rs`. There is no `runtime_factory` or `runtime_resolver` module,
contrary to the obsolete architecture deck.

The resolved runtime drives execution through the session supervisor, which runs a root
turn and any spawned subagent lanes (`apps/shared/src/gateway/session_supervisor.rs`), and
`session_worker.rs`, which constructs the per-lane `MemoryManager` and `RuntimeInput`
(`apps/shared/src/gateway/session_worker.rs:133-152`).

## 5. Request Flow

```text
Client (TUI / CLI / HTTP / channel)
        |
        v
apps/{endside,serverside}   entry + transport
        |
        v
apps/shared                 gateway: session admission, leases, checkpointing
        |                   session_runtime resolution -> ResolvedSessionRuntime
        v
crates/xiaoo-api            Runtime facade (pure runtime contracts)
        |
        v
crates/core                 run_agent_loop over LoopState + RuntimeSnapshot
        |
        +--> crates/llm-client      model calls
        +--> crates/tool            tool dispatch via OperationBackend
        +--> crates/compact         context compression
        +--> crates/memory          memory snapshot and recall
        +--> crates/hook            lifecycle hook invocation
        +--> crates/trace           telemetry
```

## 6. Related Documents

- [Runtime checkpoint control](./runtime_checkpoint.md)
- [Runtime SDK refactor (as built)](./xiaoo_api_pure_runtime_refactor.md)
- [Memory & context compression](./memory_context_system.md)
