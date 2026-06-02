# macOS Seatbelt Isolation

The local operation backend can run tool commands through macOS `sandbox-exec`.
This adds an OS-level Seatbelt profile on top of XiaoO's local path policy, so
file reads, writes, and command execution are constrained to configured roots.

## Scope

Seatbelt isolation is available only for the local operation backend on macOS.
It is not used on Linux or Windows. Non-macOS builds reject
`kind = "macos_seatbelt"` during backend construction.

The capability applies to:

- `bash` and other local exec calls, by wrapping the process with `sandbox-exec`
- local file read, write, search, and export operations, by checking paths before
  the backend performs them
- runtime permission grants, by adding temporary read/write roots to the active
  Seatbelt profile

## Configuration

Set the operation backend to `local` and configure `options.isolation`:

```toml
[operation_backend]
kind = "local"

[operation_backend.options]
home_dir = "/Users/alice"
temp_root = "/tmp"
default_shell = "/bin/zsh"

[operation_backend.options.isolation]
kind = "macos_seatbelt"
allow_network = false
readable_roots = ["/Users/alice/project"]
writable_roots = ["/Users/alice/project/.xiaoo-tmp"]
```

If `readable_roots` is omitted, the workspace root is readable. If
`writable_roots` is omitted, the workspace root and `temp_root` are writable.
Every writable root is also treated as readable.

`allow_network` defaults to `true`; set it to `false` when a local session should
not make outbound network connections.

## Runtime Behavior

When the policy is enabled, the backend builds a Seatbelt profile with:

- deny-by-default file access
- read access to system paths needed to start normal macOS commands
- read access to configured readable roots
- read/write access to configured writable roots
- optional network access based on `allow_network`

If a tool tries to access a blocked path, the backend returns a sandbox policy
denial. The gateway can ask the user whether to grant extra permission. Read and
write grants are added to the current profile; a write grant also allows reads
for the same root. A one-shot exec-runtime grant temporarily disables the
Seatbelt wrapper for the retry when macOS reports an executable/runtime access
failure without a precise path.

## Notes

- Paths must be absolute host paths. Existing paths are canonicalized during
  policy construction and access checks.
- Seatbelt is a macOS-only guardrail. Keep the normal local backend path policy
  in place; it provides consistent denial reporting before and around
  `sandbox-exec`.
- The TUI sandbox selector can enable this by setting the local backend
  isolation to `macos_seatbelt`.
