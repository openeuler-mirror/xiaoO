# Linux dyn-sandbox Isolation

The local operation backend can run tool commands through the custom `dyn-sandbox`
sandbox. This adds an OS-level mount sandbox on top of XiaoO's local path policy,
so Bash commands and their child processes see only the configured filesystem
roots.

## Scope

dyn-sandbox isolation is available only for the local operation backend on Linux.
Non-Linux builds reject `kind = "linux_dynsandbox"` during backend construction.
Linux builds also require the `dyn-sandbox` binary to be available in `PATH`.

The capability applies to:

- `bash` and other local exec calls, by wrapping the process with `dyn-sandbox`

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
kind = "linux_dynsandbox"
readable_roots = ["/home/alice/project"]
writable_roots = ["/home/alice/project/.xiaoo-tmp"]
```

If `readable_roots` is omitted, the workspace root is readable. If
`writable_roots` is omitted, the workspace root and `temp_root` are writable.
Every writable root is also treated as readable.

## Runtime Behavior

When the policy is enabled, the backend builds a `dyn-sandbox` command that:

- binds configured readable roots read-only and writable roots read/write in a
  private mount namespace
- runs the tool command with the sandbox working directory set to the command cwd

Read and write grants are added to the active mount list; a write grant also
allows reads for the same root. A one-shot exec-runtime grant temporarily bypasses
the `dyn-sandbox` wrapper for the retry, running the command directly on the host.

For paths that only appear inside a Bash command and are not visible in the
sandbox mount namespace, the command fails with the usual shell errors ("No such
file or directory", "Read-only file system").

## Notes

- Paths must be absolute host paths. Existing paths are canonicalized during
  policy construction and access checks.
- Distro-specific runtime paths are not bound by default. Add them to
  `readable_roots` if a command or toolchain needs them.
