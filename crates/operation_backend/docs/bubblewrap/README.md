# Linux Bubblewrap Isolation

The local operation backend can run tool commands through Linux `bubblewrap`
(`bwrap`). This adds an OS-level mount and namespace sandbox on top of XiaoO's
local path policy, so Bash commands and their child processes see only the
configured filesystem roots.

## Scope

Bubblewrap isolation is available only for the local operation backend on Linux.
Non-Linux builds reject `kind = "linux_bubblewrap"` during backend construction.
Linux builds also require `bwrap` to be available in `PATH`.

The capability applies to:

- `bash` and other local exec calls, by wrapping the process with `bwrap`
- local file read, write, search, and export operations, by checking paths before
  the backend performs them
- runtime permission grants, by adding temporary read/write roots to the active
  Bubblewrap bind list

## Configuration

Set the operation backend to `local` and configure `options.isolation`:

```toml
[operation_backend]
kind = "local"

[operation_backend.options]
home_dir = "/home/alice"
temp_root = "/tmp"
default_shell = "/bin/bash"

[operation_backend.options.isolation]
kind = "linux_bubblewrap"
allow_network = false
readable_roots = ["/home/alice/project"]
writable_roots = ["/home/alice/project/.xiaoo-tmp"]
```

If `readable_roots` is omitted, the workspace root is readable. If
`writable_roots` is omitted, the workspace root and `temp_root` are writable.
Every writable root is also treated as readable.

`allow_network` defaults to `true`; set it to `false` to run commands in a new
network namespace with `--unshare-net`. Domain-level allow and deny rules are
not implemented by Bubblewrap isolation.

## Runtime Behavior

When the policy is enabled, the backend builds a Bubblewrap command with:

- a private mount namespace and process-related namespaces
- read-only binds for common runtime paths such as `/usr`, `/bin`, `/lib`, and
  selected `/etc` files when they exist
- read-only binds for configured readable roots
- read/write binds for configured writable roots
- optional network isolation based on `allow_network`

If a tool tries to access a blocked path through the backend APIs, the backend
returns a sandbox policy denial and the gateway can ask the user whether to
grant extra permission. Read and write grants are added to the active bind list;
a write grant also allows reads for the same root. A one-shot exec-runtime grant
temporarily bypasses the Bubblewrap wrapper for the retry.

## Notes

- Paths must be absolute host paths. Existing paths are canonicalized during
  policy construction and access checks.
- Bubblewrap denies unbound paths by making them absent from the sandbox view.
  Bash command stderr may therefore show normal command errors such as "No such
  file or directory" or "Read-only file system".
- Distro-specific runtime paths are not bound by default. Add them to
  `readable_roots` if a command or toolchain needs them.
