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

## RPM 安装环境下的插件管理

通过 RPM 安装 xiaoO-hookers 后，所有插件安装到 `/usr/lib/.xiaoo/hookers/` 目录下。插件可以通过以下两种方式启用和关闭。

### 方式一：使用软链接命令（推荐）

RPM 安装时会创建两个全局软链接，可直接在任意目录执行：

```bash
# 交互式安装/启用插件
xiaoo-hookers-install

# 卸载/禁用插件
xiaoo-hookers-uninstall
```

也支持非交互模式：

```bash
# 启用 audit_agent（不交互，直接写入配置）
xiaoo-hookers-install --non-interactive audit_agent
```

### 方式二：直接编辑 config.toml

手动编辑 `~/.config/xiaoo/config.toml` 的 `[hooker]` 段。

**场景 1：启用所有插件**

```toml
[hooker]
default = "All"
plugins = [
  "/usr/lib/.xiaoo/hookers/audit_agent/plugin.json",
]
```

`default = "All"` 表示所有已注册的插件默认启用，`plugins` 列表中的 plugin.json 会被加载注册。

**场景 2：启用所有插件，但排除个别不需要的**

```toml
[hooker]
default = "All"
plugins = [
  "/usr/lib/.xiaoo/hookers/audit_agent/plugin.json",
  "/usr/lib/.xiaoo/hookers/tool_post_secret_guard/plugin.json",
]
disabled = [
  "pluginguard_secret_like_output",
]
```

`disabled` 列表填写 plugin.json 中定义的 `id` 字段值。

### RPM 场景下插件 install.sh 的变化

RPM 打包时，部分插件自身的 `install.sh` 行为会有所调整。以 audit_agent 为例：

- **开发者场景**（git clone + cargo build）：执行 `install.sh` 时会创建 Python 虚拟环境（venv），并通过 pip 安装依赖
- **RPM 场景**（通过 RPM 安装）：audit_agent 的 Python 依赖已通过 RPM 包提供（`python3-openai`、`python3-httpx`、`python3-pydantic`、`python3-tenacity` 等），因此 `install.sh` **不再创建 venv 和 pip install**，只负责生成 `audit_settings.json` 配置文件

这意味着在 RPM 场景下执行 `xiaoo-hookers-install` 不会因为缺少 pip 或 venv 而出错。

### 验证插件是否生效

```bash
# 查看 xiaoo 运行日志，确认插件已加载
xiaoo run -p "echo hello"

# 查看审计日志（audit_agent）
cat /usr/lib/.xiaoo/hookers/audit_agent/audit_policy_checker.log
```

### 注意事项

- **路径使用绝对路径**：`plugins` 列表中的路径必须是 plugin.json 的完整绝对路径，RPM 安装后路径为 `/usr/lib/.xiaoo/hookers/<plugin>/plugin.json`
- **`disabled` 使用 hooker id**：`disabled` 列表填写的是 plugin.json 中 `id` 字段的值（如 `plugin_audit_tool_input`、`pluginguard_secret_like_output`），不是文件路径
- **修改配置后无需重启**：xiaoo 每次启动时重新读取配置
