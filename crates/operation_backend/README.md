# operation_backend

Operation backends for tool execution. The crate implements the
`agent-contracts` backend contracts — `OperationBackend` for exec, filesystem,
search, and export operations, plus the `BackendLifecycle` /
`BackendProvider` control plane — so a host can create, use, and delete
backends through one uniform lifecycle API.

## Local backend

The `local` backend runs tool operations on the host machine:

- exec: `bash` and other commands, wrapped by the configured isolation if any
- filesystem: read, write, and temp-path operations
- search: glob and grep
- file export
- LSP: `local_lsp_backend()` builds a minimal backend for daemon-level LSP
  singletons

`OperationBackendBuilderImpl` builds the backend from an
`OperationBackendConfig` with `kind = "local"`; `local_backend` and
`local_backend_with_isolation` construct one directly, and
`LocalBackendProvider` / `local_backend_provider` expose it through the
`BackendProvider` / `BackendLifecycle` control plane.

## Isolation

The local backend can run commands inside an OS-level sandbox, selected via
`options.isolation`:

- [Linux Bubblewrap](docs/bubblewrap/README.md) — wraps commands with `bwrap`
  for mount and namespace isolation on Linux.
- [Linux dyn-sandbox](docs/dyn_sandbox/README.md) — wraps commands with the
  custom `dyn-sandbox` binary for mount isolation on Linux, with a streaming
  AUTH control channel for runtime permission prompts.
- [macOS Seatbelt](docs/seatbelt/README.md) — constrains commands with a
  `sandbox-exec` Seatbelt profile on macOS.

## backend_cli

The `xiaoo-backend-cli` binary (source under `src/bin/backend_cli.rs`) builds
a backend from a config file or a stdin envelope and executes a single JSON
operation request, so backend operations can be externalized into a
subprocess.

## Tests

```bash
bash tests/run.sh -p operation_backend
```
