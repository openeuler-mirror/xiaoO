# Cerberus Bash Control Plugin

通过 Cerberus 沙箱控制 bash 命令执行的 xiaoO hooker 插件。

## 功能

- 在 bash 命令执行前，通过 Cerberus 沙箱进行安全检查和策略控制
- 在 bash 命令执行后，检测并记录沙箱拦截事件

## 前提条件

**必须先安装 Cerberus CLI**：

```bash
# 安装（需要 eBPF toolchain）
cargo install --path crates/cerberus/cerberus-cli

# 或不带 eBPF 支持
cargo install --path crates/cerberus/cerberus-cli --no-default-features -p cerberus-core
```

确保 `cerberus` 在 PATH 中，或位于 `~/.cargo/bin/cerberus`。

## 安装

```bash
cd plugins/hookers/cerberus_bash_control
./install.sh
```

安装脚本会：
1. 检查 cerberus 是否已安装
2. 从模板生成 `plugin.json`（填充绝对路径）
3. 将插件路径添加到 `~/.config/xiaoo/config.toml` 的 `[hooker]` 段

## 卸载

```bash
cd plugins/hookers/cerberus_bash_control
./uninstall.sh
```

## 配置

### 告警日志路径

安装时会提示输入告警日志路径，用于记录沙箱拦截事件。默认为 `/tmp/cerberus_alert.log`。

### 策略文件

`policy.toml` 定义了沙箱策略（即 `cerberus-core::policy::Policy` 的 TOML 表达，`deny_unknown_fields`——写了 schema 外的字段会直接解析失败）。可根据需要修改：

```toml
landlock_optional = true
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true

[[custom_paths]]
path = "/path/to/workspace"
permission = "readwrite"

[namespaces]
network = false  # 禁止网络访问

[resources]
timeout_secs = 60
max_memory_bytes = 268435456

[environment]
whitelist = ["PATH", "LANG", "HOME", "TERM"]
```

顶层字段为 `landlock_optional` / `mount_isolation_fallback`；路径访问由 `[path_groups]` 预定义开关 + `[[custom_paths]]`（`path` / `permission`，取值 `readonly` / `readwrite` / `readexecute` / `readwriteexecute`）表达；细粒度网络管控用 `[network_policy]`（需要 eBPF backend，默认构建下启用会 fail-closed）。

完整 schema 与内置 profile 说明参考 `crates/cerberus/docs/configuration.md`，完整示例参考本目录的 `policy.toml`。

### hooker default 模式

安装时会询问 `[hooker]` 段的 `default` 模式。**它决定的是「已注册的 hooker 里哪些处于启用状态」，不是工具层的允许/拦截策略**（`crates/hook/src/framework/registry/builder.rs`）：

- `All`：所有**已注册**的 hooker 默认启用；再用 `disabled` 列表排除个别 hooker
- `None`：所有已注册 hooker 默认不启用；只有 `enabled` 列表里列出的 hooker 会跑

也就是说 `default = "All"` ≠ 「允许所有工具调用」。工具调用是否被拦，取决于跑起来的 hooker 脚本返回什么（这里是 `wrap_bash_with_cerberus.py` 返回 `allow` / `deny` / `transform`），与 `default` 无关。如果一个 hooker 没被启用，它就像不存在一样——既不拦截也不放行任何东西。

> `enabled` / `disabled` / `policies` 里出现**未注册**的 hooker id 会让 xiaoo 启动构建失败（`crates/hook/src/framework/registry/builder.rs`），而不是静默忽略。

## 工作原理

1. **pre hook** (`wrap_bash_with_cerberus.py`)：拦截 `bash` 工具调用，将命令包装为 `cerberus exec -- <command>`
2. **post hook** (`alert_on_cerberus_permission.py`)：检查执行结果中的拦截标记，记录到告警日志

## 文件说明

| 文件 | 说明 |
|------|------|
| `plugin.json.template` | 插件配置模板 |
| `plugin.json` | 生成的插件配置（安装后） |
| `policy.toml` | Cerberus 策略文件 |
| `wrap_bash_with_cerberus.py` | pre hook：命令包装 |
| `alert_on_cerberus_permission.py` | post hook：拦截检测 |
| `config.py` | 安装/卸载脚本 |
