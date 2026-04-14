# xiaoo-app

Application-layer package for XiaoO.

## Current role

- Own the app assembly layer for gateway, TUI, channel ingress, and process bootstrap.
- Depend on `crates/*` for runtime, memory, contracts, and shared types.
- Keep transport concerns out of `crates/core`.
