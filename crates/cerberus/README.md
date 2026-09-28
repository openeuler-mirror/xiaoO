# Cerberus

> Cerberus stands at the threshold, guarding what may pass and what must be denied.

Secure command execution toolkit for Rust and AI agent hosts. It combines policy-based sandboxing, execution filtering, audit hooks, and a CLI that makes safe-by-default command execution usable from local development workflows.

## Highlights

| Feature | What You Get |
|---------|--------------|
| **Policy-Based Sandboxing** | Filesystem, namespace, environment, process, and network controls applied before the command runs |
| **Profiles for Real Workflows** | Built-in `workspace-write-network-on`, `workspace-write-network-off`, and `workspace-write-network-on-dev-env` profiles for trusted commands, locked-down execution, and AI coding assistant use cases |
| **Fail-Closed Enforcement** | Policies can require full enforcement and refuse execution when runtime capabilities are missing |
| **Execution Filters** | Argument, environment, and output filters with configurable violation actions |
| **Audit Surface** | Built-in execution context, event model, observers, and sinks for logging and traceability |
| **CLI + Library Split** | Use `cerberus-core` as an embeddable Rust library or `cerberus` as an operator-facing CLI |
| **Host Scaffolding** | Install integration scaffolding for Claude Code, Codex CLI, and OpenCode workflows |
| **Linux Sandbox Primitives** | Landlock, seccomp, mount isolation, and namespaces are wired into the runtime where supported |

## Workspace Usage

Use the crate(s) you need from this workspace via local path dependencies:

```toml
[dependencies]
cerberus-core = { path = "crates/cerberus/cerberus-core" }

# If you need the CLI library surface
cerberus-cli = { path = "crates/cerberus/cerberus-cli" }
```

> **Important — Cerberus ships outside the main workspace.** The three Cerberus crates
> are commented out of the root workspace (`Cargo.toml:26-29`), and
> `crates/cerberus/` has no `[workspace]` of its own. Their manifests inherit
> `version.workspace = true`, so they cannot resolve a workspace root as checked out and
> are not built by `cargo build --workspace`. To build or install them you must first
> make them workspace members (uncomment the entries in the root `Cargo.toml`) or add a
> `[workspace]` table under `crates/cerberus/`. An eBPF toolchain is additionally required
> for the `ebpf` feature. See `tests/README.md:13` for the same caveat.

## Project Layout

| Path | Purpose |
|------|---------|
| `cerberus-core/` | Core request, policy, execution, filtering, sandbox, result, and audit primitives |
| `cerberus-cli/` | `cerberus` binary, built-in profiles, history storage, rendering, and host integration commands |
| `docs/` | Scenario-driven documentation and verification notes |
| `testcases/` | Manual end-to-end test cases, e.g. `testcases/cli-fs-permissions-e2e.md` |

## Quick Start

### Rust Library

```rust,no_run
use cerberus_core::{execute, ExecRequest, Policy};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request = ExecRequest::new("ls").arg("-la");
    let policy = Policy::minimal();

    let result = execute(request, &policy)?;

    println!("Exit code: {}", result.exit_code);
    println!("Stdout: {}", result.stdout_utf8());
    Ok(())
}
```

### CLI Usage

```bash
# Run a command with the default workspace-write-network-on-dev-env profile
cargo run -p cerberus-cli -- exec -- ls -la

# Use a stricter built-in profile
cargo run -p cerberus-cli -- --profile workspace-write-network-off exec -- cat /etc/hosts

# Inspect available profiles
cargo run -p cerberus-cli -- profile list
```

For a more stable global installation, install Cerberus into Cargo's bin directory:

```bash
cargo install --path crates/cerberus/cerberus-cli
```

> **Prerequisite:** the command above only works once Cerberus is a resolvable workspace —
> either uncomment the three Cerberus entries in the root `Cargo.toml:26-29`, or add a
> `[workspace]` table under `crates/cerberus/`. As checked out, their
> `version.workspace = true` manifests cannot find a workspace root and the install fails
> (same caveat as `tests/README.md:13`). The `cargo run -p cerberus-cli` examples above
> have the same requirement.

If you need `network_policy` enforcement, install Cerberus with the eBPF-enabled core feature:

```bash
cargo install --path crates/cerberus/cerberus-cli --features cerberus-core/ebpf --force
```

This is required for profiles such as `repo-root-python-network-filtered`. Without the `cerberus-core/ebpf` feature, enabled `network_policy` configurations fail closed before execution.

Then make sure your shell can find Cargo-installed binaries:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

### Policy Sources

Cerberus resolves policies in a deterministic order:

1. `--policy-file <PATH>` explicit override
2. `config/cerberus-policies/<name>.toml` discovered project profile
3. Built-in profile fallback: `workspace-write-network-on`, `workspace-write-network-off`, `workspace-write-network-on-dev-env`

> **This repository ships no policy files.** There is no `config/` directory at the repo
> root and no `cerberus-policies` directory anywhere in the tree, so level 2 never
> resolves for a fresh clone — only `--policy-file` and the built-ins are actually
> available out of the box. Level 2 is a convention for *your own* project: create
> `config/cerberus-policies/<name>.toml` in the directory you run from, and the CLI
> discovers it there.

For CLI behavior, two runtime rules matter:

- `cerberus exec` defaults to `workspace-write-network-on-dev-env` when you do not pass `--profile`
- Built-in profiles (`workspace-write-network-on`, `workspace-write-network-off`, `workspace-write-network-on-dev-env`) auto-inject the current workspace as a `readwrite` custom path during `exec` and `profile show`; `--policy-file` remains file-defined and is not auto-modified
- Legacy aliases `minimal`, `strict`, and `llm-safe` still resolve for compatibility, but `profile list` and docs now prefer the explicit names

There are **no checked-in file-backed presets** in this repository. The
`repo-root-write-*` names (`repo-root-write-network-off`, `repo-root-write-network-on`,
`repo-root-write-network-on-dev-env`) are examples of file-backed policies that target a
repository root explicitly; create them yourself under `config/cerberus-policies/` if you
want that behavior. They are **not** built-ins and will not appear in a fresh clone's
`profile list`. Their intended scope differs from the built-ins: for example, a
`repo-root-write-network-off` policy keeps the repo root `readwrite`, blocks network, and
still allows `/dev`, `/proc`, and `/mnt/wsl` reads, where the built-in
`workspace-write-network-off` stays narrower.

### Built-in Profiles

| Profile | Intended Use | Current Defaults |
|---------|--------------|------------------|
| `workspace-write-network-on` | Trusted local commands with workspace write access, network allowed, and fallback-friendly enforcement | 120s timeout, 1 GiB memory limit, permissive environment whitelist, network allowed, Landlock optional plus mount downgrade allowed |
| `workspace-write-network-off` | Untrusted commands with workspace write access, network blocked, and fail-closed isolation | 30s timeout, 256 MiB memory limit, process cap of 50, reduced environment whitelist, network blocked, no fallback |
| `workspace-write-network-on-dev-env` | AI coding assistants that need workspace write access, network, and development-tool environment variables | 60s timeout, 512 MiB memory limit, process cap of 100, broader dev-tool env whitelist, network allowed, no fallback |

Built-in profiles now share one CLI-specific filesystem convenience: the current workspace is injected as `readwrite` so common repo-local reads and writes work out of the box. This is intentionally narrower than execution permission; `readwrite` does **not** imply `readexecute`, so running `./script` from the workspace still requires an explicit executable path rule in a policy file. If a path genuinely needs all three capabilities, Cerberus also supports `readwriteexecute`, but that should stay exceptional rather than becoming the default.

## CLI Commands

| Command | Description |
|---------|-------------|
| `cerberus exec <cmd...>` | Execute a command under the resolved policy |
| `cerberus history` | Show stored execution history |
| `cerberus profile list` | List built-in and discovered profiles |
| `cerberus profile show <name>` | Inspect a profile, runtime capabilities, enforcement level, and any built-in workspace injection that applies at CLI runtime |
| `cerberus init --claude|--codex|--opencode` | Install, inspect, or uninstall host scaffolding for supported agent hosts |

## Core Library Surface

`cerberus-core` exposes the building blocks needed to embed secure execution in another Rust application:

- **Requests**: `ExecRequest`, `StdinPolicy`
- **Policies**: `Policy`, `PolicyBuilder`, filesystem and network rule types, namespace and resource configs
- **Execution**: `execute`, `execute_shell`, sandbox setup and spawn options
- **Filters**: argument, environment, and output filters with explicit violation results
- **Audit**: execution context, observers, sinks, and event types for recording execution lifecycle data

## Documentation

| Guide | Description |
|-------|-------------|
| [Configuration Guide](docs/configuration.md) | 中文配置说明：策略文件写法、字段含义、内置 profile 差异与常见陷阱 |
| [Runtime Capability Matrix](docs/runtime-capability-matrix.md) | 不同运行环境下的编译能力、运行门禁、配置差异与安全防护保留情况 |
| [History Storage](docs/storage.md) | 执行历史的存储位置与排查方式 |

## Platform Notes

- Linux is the primary target for sandbox enforcement.
- `cerberus-core` integrates Landlock, seccomp, mount isolation, and namespace setup where the runtime supports them.
- `namespaces.network` uses positive public semantics: `true` means network allowed, `false` means network blocked.
- `network_policy` is feature-gated: once enabled, Cerberus routes execution through the runtime-gated sandbox path; in the default build it fails closed when the required eBPF backend is unavailable, while builds with `--features ebpf` include a real network matcher/enforcer path.
- Enforcement strength is runtime-dependent; `cerberus profile show <name>` reports detected capabilities and whether a policy is fully enforced, degraded, or unsupported.
