# xiaoo-shared

Application-layer package for XiaoO.

## Current role

- Own the app assembly layer for gateway, TUI, channel ingress, and process bootstrap.
- Depend on `crates/*` for runtime, memory, contracts, and shared types.
- Keep transport concerns out of `crates/core`.

## Runtime/session/backend layout

- Runtime checkpoint and checkout API types live in `src/runtime_checkpoint.rs`
  and are re-exported from the crate root.
- Session-scoped runtime resolution and assembly live in
  `src/gateway/session_runtime/`.
- Backend manager types live in `src/backend/`, with shared backend
  request/result shapes grouped in `src/backend/base.rs`.

The public control plane should talk in runtime/checkpoint terms. In the current
v1 implementation, `runtime_id` is backed by the internal `session_id`, while
backend ids remain internal manager state. See
[`docs/runtime_checkpoint_control.md`](../../docs/runtime_checkpoint_control.md)
for the current checkpoint layering.
