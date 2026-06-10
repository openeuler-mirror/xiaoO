# Plugin Installation and Usage

## Cerberus Plugin (Optional)

Cerberus provides secure command execution with policy-based sandboxing. It is included in the workspace but requires the eBPF toolchain (Linux only).

```bash
# Install with eBPF support (default, requires nightly Rust + eBPF toolchain)
cargo install --path crates/cerberus/cerberus-cli

# Install without eBPF if toolchain is unavailable
cargo install --path crates/cerberus/cerberus-cli --no-default-features -p cerberus-core
```

If `cargo build --release` fails due to Cerberus/eBPF, you can skip it:

```bash
cargo build --release --workspace --keep-going
```

## Plugins

Pre-built hookers and skills are placed in `<your_xiaoO>/plugins`. They are **not installed by default**.

To install hookers, run:

```bash
cd <your_xiaoO>/plugins/hookers
./config.sh
```

You can also develop your own hookers and place them in `<your_xiaoO>/plugins/hookers`. See `how-to-develop-a-plugin-hooker.md` for details.

## Skills

### Built-in Skills

When you run `cargo install --path apps/endside`, builtin skills are automatically installed. They provide security policy enforcement and other built-in capabilities, and are loaded with highest priority by the runtime.

**Installation locations** (automatic fallback):
- **System level** (preferred): `/usr/lib/.xiaoo/skills/` - requires root privileges
- **User level** (fallback): `~/.xiaoo/skills/` - used if system-level installation fails

**Builtin skills** (located in `<xiaoO>/plugins/skills/`):
- `xiaoo-guardian` - Security policy enforcement
- `block-analyzer` - Block analysis capabilities

> **Note**: `cargo build` does NOT install skills. Only `cargo install` triggers skill installation.
>
> **Installation Behavior**:
> - First attempts to install all builtin skills to system-level directory (requires root privileges)
> - If system-level installation fails (e.g., permission denied), automatically falls back to user-level directory
> - Without these skills, security features and other capabilities may be unavailable.
>
> **For system-wide installation** (recommended for multi-user environments):
> - Run `cargo install` with root privileges: `sudo cargo install --path apps/endside`

### Skill Directory Priority (Four Levels)

1. **Project level** (highest): `./.xiaoo/skills/` - Project-specific skills
2. **Config level** (medium): Directories specified in `[skills].dirs` - Team/user shared skills
3. **User level**: `~/.xiaoo/skills/` - Personal skills available everywhere
4. **System level** (lowest): `/usr/lib/.xiaoo/skills/` - Built-in skills only

### Custom Skills

Custom skills can be installed to user-level directory using the `xiaoo --cli skill install` command:

```bash
# Install from local directory (installs to ~/.xiaoo/skills by default)
xiaoo --cli skill install ./my-skill/

# Install from Git repository
xiaoo --cli skill install https://github.com/user/my-skill.git
```

> **Note**: User-installed skills go to `~/.xiaoo/skills/` (user level), NOT `/usr/lib/.xiaoo/skills/` (system level is reserved for built-in skills).

See [skill_usage.md](./skill_usage.md) for detailed skill documentation.

### Uninstalling Skills

```bash
# Remove a user-installed skill
xiaoo --cli skill remove <skill-name>

# Or manually remove from user level
rm -rf ~/.xiaoo/skills/<skill-name>

# Remove built-in guardian skill (requires root)
sudo rm -rf /usr/lib/.xiaoo/skills/xiaoo-guardian
```

To completely uninstall xiaoO and all associated skills:

```bash
# Uninstall the application
cargo uninstall xiaoo-endside

# Remove system-level skills (requires root)
sudo rm -rf /usr/lib/.xiaoo/skills

# Remove user-level skills
rm -rf ~/.xiaoo/skills
```
