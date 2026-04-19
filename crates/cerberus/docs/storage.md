# Cerberus 历史记录与数据库位置

Cerberus CLI 会把执行历史写入本地 SQLite 数据库。

当前实现里，数据库只用于 `cerberus` CLI 的历史记录功能，不属于 `cerberus-core` 的策略文件配置项，也没有单独的 CLI 参数去改写它的路径。

---

## 哪些命令会访问数据库

当前 CLI 里有两条主路径会打开数据库：

- `cerberus exec -- ...`
  - 命令执行完成后，CLI 会打开数据库并写入一条执行记录
- `cerberus history`
  - CLI 会打开同一个数据库并读取历史记录

对应代码路径：

- `cerberus-cli/src/main.rs`
  - `exec_command()` -> `Storage::open_default()` -> `record_execution(...)`
  - `show_history()` -> `Storage::open_default()` -> `get_records(...)`

---

## 数据库文件落在哪里

当前实现使用：

```rust
dirs::data_local_dir().or_else(dirs::data_dir())
    .join("cerberus")
    .join("cerberus.db")
```

也就是说，Cerberus 会先取平台默认的 **local data directory**，如果取不到，再回退到 `data_dir()`，最后把数据库放在：

```text
<data-dir>/cerberus/cerberus.db
```

### Linux

通常是：

```text
$XDG_DATA_HOME/cerberus/cerberus.db
```

如果 `XDG_DATA_HOME` 没设置，通常会落在：

```text
~/.local/share/cerberus/cerberus.db
```

### macOS

通常是：

```text
~/Library/Application Support/cerberus/cerberus.db
```

### Windows

通常是：

```text
%LOCALAPPDATA%\cerberus\cerberus.db
```

如果 `data_local_dir()` 取不到，才会退回到更通用的 data dir。

---

## 目录和数据库何时创建

当前实现是按需创建：

- 当 CLI 第一次成功走到 `Storage::open_default()`
- 它会先创建父目录 `.../cerberus/`
- 然后打开 `cerberus.db`
- 如果 `executions` 表不存在，就自动建表

所以不需要手动先创建目录。

---

## 数据库存了什么

当前 schema 只有一张表：`executions`

字段包括：

- `id`
- `command`
- `exit_code`
- `duration_ms`
- `timed_out`
- `timestamp`

这意味着当前历史记录主要用于回答：

- 执行了什么命令
- 退出码是多少
- 跑了多久
- 是否超时
- 大致是什么时候执行的

它不是完整审计归档，也不是策略文件仓库。

---

## 当前没有什么能力

下面这些能力，**当前实现没有**：

- 没有 `--db-path` 之类的 CLI 参数
- 没有 `CERBERUS_DB` 之类的专用环境变量覆盖数据库路径
- 没有内置 `history clear` 命令

如果你要清空历史，只能直接删除数据库文件。

---

## 如何自己查看或删除

### 查看数据库文件是否存在

Linux 上可以直接看：

```bash
ls "$XDG_DATA_HOME/cerberus/cerberus.db"
```

如果你没有设置 `XDG_DATA_HOME`，通常可以看：

```bash
ls ~/.local/share/cerberus/cerberus.db
```

### 删除历史记录

直接删除数据库文件即可，例如 Linux 常见路径：

```bash
rm ~/.local/share/cerberus/cerberus.db
```

下次运行 `cerberus exec` 或 `cerberus history` 时，CLI 会重新创建数据库和表。

---

## 和策略文件的关系

数据库位置 **不受 policy TOML 控制**。

也就是说：

- `path_groups`
- `custom_paths`
- `namespaces`
- `resources`
- `environment`
- `network_policy`

这些都不会改变历史数据库的位置。

数据库路径是 CLI 自己的本地运行时选择，不是策略配置的一部分。
