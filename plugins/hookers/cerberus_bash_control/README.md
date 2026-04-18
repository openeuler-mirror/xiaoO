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

`policy.toml` 定义了沙箱策略。可根据需要修改：

```toml
[filesystem]
readwrite = ["/path/to/workspace"]

[namespaces]
network = false  # 禁止网络访问
```

详细配置参考 `crates/cerberus/docs/configuration.md`。

### hooker default 模式

安装时会询问 `[hooker]` 段的 `default` 模式：

- `All`：默认允许所有工具调用，仅对 plugins 列表中的插件进行拦截
- `None`：默认拦截所有工具调用，仅对 plugins 列表中的插件放行

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
