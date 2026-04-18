# Cerberus 配置指南

`cerberus` 的策略文件本质上是 `cerberus-core::policy::Policy` 的 TOML 表达。本文档只描述当前代码里已经实现并可被解析的字段，不扩展任何“未来可能支持”的配置。

## 配置方式

Cerberus 的策略来源按以下优先级解析：

1. `--policy-file <PATH>`：显式指定文件，优先级最高
2. `config/cerberus-policies/<name>.toml`：按 profile 名称向上查找项目配置目录
3. 内置 profile：`workspace-write-network-on`、`workspace-write-network-off`、`workspace-write-network-on-dev-env`

这意味着：

- 如果你传了 `--policy-file`，就不会再看 `--profile`
- 如果 `config/cerberus-policies/workspace-write-network-off.toml` 存在，它会覆盖同名内置 profile
- 如果文件存在但解析失败，Cerberus 会直接报错，不会悄悄退回内置 profile

当前仓库还额外签入了三份 **repo-root** 作用域的 file-backed profile：

- `repo-root-write-network-off`
- `repo-root-write-network-on`
- `repo-root-write-network-on-dev-env`

它们不是内置 profile，而是 `config/cerberus-policies/` 下的普通 TOML 文件，因此：

- `profile list` 会把它们显示为 `[file: ...]`
- 它们的行为完全由文件字面内容决定
- 它们的名字带 `repo-root`，是为了明确表示这些规则写死了当前仓库根路径，不是通用的 workspace 预设

其中 `repo-root-write-network-off` 也比内置 `workspace-write-network-off` 更宽：它同样禁网，但保留了 repo-root 的 `readwrite`，以及 `/dev`、`/proc`、`/mnt/wsl` 的访问。

### CLI 默认行为（当前实现）

- `cerberus exec` 在未显式指定 `--profile` 时，默认使用 `workspace-write-network-on-dev-env`
- 对内置 profile（`workspace-write-network-on`、`workspace-write-network-off`、`workspace-write-network-on-dev-env`），CLI 会在 `exec` 和 `profile show` 时自动把当前工作目录追加为一条 `readwrite` 的 `custom_paths`
- 这种自动注入只作用于内置 profile 的 CLI 体验，不会改写 `--policy-file` 指向的文件，也不会改写 `config/cerberus-policies/*.toml` 里的静态内容
- 旧名字 `minimal`、`strict`、`llm-safe` 仍然可用，但文档与 `profile list` 默认展示新的显式名称

---

## 最小可用示例

下面这个示例描述的是**纯策略文件本身**。如果你直接写 TOML 文件，业务目录仍然要自己通过 `[[custom_paths]]` 显式声明；只有 CLI 使用内置 profile 执行 `exec` 或查看 `profile show` 时，才会自动注入当前 workspace。

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = false
proc_filesystem = false
wsl_paths = false

[namespaces]
mount = true
pid = true
network = true
user = true

[resources]
timeout_secs = 30
max_memory_bytes = 268435456
# max_processes 省略表示不限数量

[environment]
whitelist = ["PATH", "LANG", "HOME"]

[[custom_paths]]
path = "/workspace"
permission = "readwrite"
```

---

## 顶层字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `landlock_optional` | `bool` | `false` | Landlock 不可用时，是否允许继续执行 |
| `mount_isolation_fallback` | `bool` | `false` | Landlock 不可用时，是否允许退化到 mount 隔离 |
| `path_groups` | table | 全部为 `false` | 预定义文件系统访问组 |
| `custom_paths` | array<table> | 空数组 | 自定义文件系统路径规则 |
| `namespaces` | table | 全部为 `true` | Linux namespace 隔离开关 |
| `resources` | table | `timeout_secs=30`、`max_memory_bytes=268435456`、`max_processes` 省略 | 资源限制 |
| `environment` | table | 见下文 | 允许透传到子进程的环境变量白名单 |
| `network_policy` | table | 不启用 | 细粒度网络规则对象。默认构建下会先做校验并在缺少 backend 时 fail-closed；启用 `ebpf` feature 时会接入真实监控/拦截路径 |

---

## `path_groups`：预定义路径组

```toml
[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = false
proc_filesystem = false
wsl_paths = false
```

| 字段 | `true` 时允许的访问 | 实际路径 |
|------|-------------------|----------|
| `system_binaries` | 读 + 执行 | `/usr/bin` |
| `system_libraries` | 读 / 读+执行 | `/usr/lib`、`/usr/lib64`、`/lib`、`/lib64`、`/usr/share`、`/etc/ld.so.cache`、`/etc/localtime`、`/etc/locale.alias` |
| `temp_directories` | 读 + 写 | `/tmp` |
| `device_files` | 读 + 写 | `/dev` |
| `proc_filesystem` | 只读 | `/proc` |
| `wsl_paths` | 只读 | `/mnt/wsl` |

### 什么时候用它

- 想快速打开一类常见系统路径时，用 `path_groups`
- 想补充业务路径时，再配合 `[[custom_paths]]`

### 重要提醒

当前实现**没有**“打开整个 `/etc`”的 `path_groups` 开关。像 DNS 解析、`/etc/hosts`、TLS trust store 这种需求，应该通过 `[[custom_paths]]` 显式列出真正需要的只读路径。

---

## `custom_paths`：自定义路径规则

```toml
[[custom_paths]]
path = "/workspace"
permission = "readwrite"

[[custom_paths]]
path = "/usr/local/bin/my-tool"
permission = "readexecute"

[[custom_paths]]
path = "/workspace/generated-tool"
permission = "readwriteexecute"
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `path` | `string` | 是 | 规则对应的绝对路径；在 file-backed policy 中也可以写相对路径 |
| `permission` | `string` | 是 | `readonly`、`readwrite`、`readexecute`、`readwriteexecute` |

| 权限值 | 含义 |
|--------|------|
| `readonly` | 可读，不可写，不可执行 |
| `readwrite` | 可读可写，可创建/替换/删除，不可执行 |
| `readexecute` | 可读可执行，不可写 |
| `readwriteexecute` | 可读可写可执行，也可创建/替换/删除 |

### 建议

- 优先写绝对路径
- 只给真正需要的目录，不要直接开放过大的父目录
- 如果只是想让命令可执行，通常应该用 `readexecute` 而不是 `readwrite`
- `readwriteexecute` 只应用在确实需要“生成后立刻执行”这类路径上，不要把它当成默认值
- 联网命令如果需要 resolver / hosts / CA bundle，应显式补充这些只读路径，而不是依赖某个隐式的“network”文件系统组

### file-backed policy 的相对路径规则

对 file-backed policy（包括 `--policy-file` 和 `config/cerberus-policies/*.toml`），`custom_paths.path` 里的相对路径会在 **CLI 解析期** 按启动 `cerberus` 时的当前工作目录展开。

例如，如果你在 `/work/project` 下运行：

```bash
cerberus --policy-file ./policy.toml exec -- ...
```

那么：

- `path = "."` → `/work/project`
- `path = "./plugins"` → `/work/project/plugins`
- `path = "plugins"` → `/work/project/plugins`
- `path = "../shared"` → `/work/shared`

这里要特别区分两层语义：

- 这是 **file-backed policy 的当前目录相对路径语义**
- 它不等于 built-in profile 的自动 workspace 注入
- `cerberus-core` 仍然只看到展开后的绝对路径

### CLI 对内置 profile 的自动 workspace 注入

当前 CLI 会对内置 profile（`workspace-write-network-on`、`workspace-write-network-off`、`workspace-write-network-on-dev-env`）自动追加一条以当前工作目录为目标的 `readwrite` 规则，用来支持常见的工作区读取与写入操作，例如在仓库根目录执行 `ls`、读取源码、写入构建产物等。

这里要特别区分两层语义：

- 这是 **CLI 对内置 profile 的运行时补充**，不是 TOML 文件里的隐式默认值
- 注入的是 `readwrite`，**不是** `readexecute`，所以它不等价于允许直接执行 workspace 里的 `./script` 或本地二进制

---

## `namespaces`：隔离语义

```toml
[namespaces]
mount = true
pid = true
network = true
user = true
```

| 字段 | `true` 的含义 | `false` 的含义 |
|------|---------------|----------------|
| `mount` | 启用挂载隔离 | 与宿主共享挂载视图 |
| `pid` | 启用 PID 隔离 | 可见宿主进程视图 |
| `network` | 允许网络访问，不创建隔离网络视图 | 阻止网络访问，运行时会走禁网隔离路径 |
| `user` | 启用 user namespace | 使用真实 UID/GID |

### `namespaces.network` 的公开语义

这里的 `network` 现在就是字面意思，表示“是否允许网络访问”：

- `network = true`：允许联网
- `network = false`：禁止联网

CLI 里的 `profile show` 也按这个公开语义展示，例如 `workspace-write-network-off` 会显示 `network: false (network blocked)`，`workspace-write-network-on-dev-env` 会显示 `network: true (network allowed)`。

### 与 `user` 的关系

当前仓库里的允许联网预设，`workspace-write-network-on` 与 `workspace-write-network-on-dev-env` 都使用 `user = false`。这不是 `network = true` 的语义前提，只是当前内置 profile 的已验证组合。如果你自定义策略，可以按需求单独设置 `user`，但要确认目标运行环境真的支持你要求的 namespace 组合。

### fail-closed 行为

在 Linux 上，`pid`、`user`、`network` 这些被代码视为关键 namespace：

- 如果策略要求它们开启，但当前运行环境不支持，Cerberus 会直接拒绝执行
- `mount` 缺失则可能进入 `degraded`，不一定立刻失败

---

## `resources`：资源限制

```toml
[resources]
timeout_secs = 60
max_memory_bytes = 536870912
max_processes = 100
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeout_secs` | `u64` | `30` | 最大执行时长，超时后进程会被终止 |
| `max_memory_bytes` | `u64` / 省略 | `268435456` | 地址空间上限，单位字节 |
| `max_processes` | `u32` / 省略 | 省略 | 最大进程数，用于限制 fork bomb |

### 不限资源的写法

当前实现已经修复 `0` 的歧义：

- 最推荐的写法仍然是**省略字段**，例如省略 `max_processes`
- 如果旧文件里写了 `0`，解析时会被规范化为“不限”，不会再把 `0` 直接当成运行时硬限制传给 `setrlimit`

换句话说，`max_memory_bytes` 和 `max_processes` 要表达“不限”，优先用“省略字段”这个写法。

---

## `environment`：环境变量白名单

```toml
[environment]
whitelist = ["PATH", "LANG", "HOME", "USER"]
```

当前默认白名单来自 `EnvironmentConfig::default()`：

| 默认透传变量 |
|--------------|
| `PATH` |
| `LANG` |
| `HOME` |
| `USER` |
| `HTTP_PROXY` |
| `HTTPS_PROXY` |
| `http_proxy` |
| `https_proxy` |
| `ALL_PROXY` |
| `all_proxy` |
| `NO_PROXY` |
| `no_proxy` |

### 行为说明

- 只有在白名单里的变量才会传给沙箱子进程
- 如果你把白名单收得过窄，像 `HOME`、`TERM`、`SHELL` 这类工具常用变量会消失
- `workspace-write-network-off` / `workspace-write-network-on` / `workspace-write-network-on-dev-env` 三个内置 profile 都会覆盖这里的默认白名单

---

## `network_policy`：网络规则结构

```toml
[network_policy]
enabled = true
mode = "monitor"
default_action = "deny"

[[network_policy.rules]]
action = "allow"
direction = "outbound"
protocol = "tcp"
hosts = ["example.com"]
cidrs = ["192.168.1.0/24"]
ports = [[80, 80], [443, 443]]
```

### 顶层字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | `bool` | `true` | 是否启用该网络策略对象 |
| `mode` | `"monitor" \| "enforce"` | `monitor` | 监控还是强制执行 |
| `default_action` | `"allow" \| "deny"` | `deny` | 没有命中规则时的默认动作 |
| `rules` | array<table> | 空数组 | 具体规则列表 |

### 单条规则字段

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `action` | `"allow" \| "deny"` | 是 | 命中规则后的动作 |
| `direction` | `"inbound" \| "outbound"` | 否 | 方向，不写表示都匹配 |
| `protocol` | `"tcp" \| "udp"` | 否 | 协议，不写表示都匹配 |
| `hosts` | `string[]` | 否 | 域名列表 |
| `cidrs` | `string[]` | 否 | CIDR 网段列表 |
| `ports` | `[[u16, u16]]` | 否 | 端口范围，闭区间 |

### 校验规则

- 端口不能是 `0`
- 端口范围必须满足 `start <= end`
- `cidrs` 必须带 `/`，前缀长度必须在 `0..=32`
- `direction` 只能是 `inbound` 或 `outbound`
- `protocol` 只能是 `tcp` 或 `udp`
- 空规则是合法的，含义接近“匹配全部”

### 当前代码里的实际地位

从当前仓库代码路径看，`network_policy` 目前已经具备：

- TOML 解析
- 字段校验
- `profile show` 输出展示
- 与 `namespaces.network` 的冲突校验
- 运行前能力门禁
- `ebpf` feature 下的 matcher / enforcer 初始化与事件处理路径

但它的真实运行状态要分开理解：

1. **总开关仍然是 `namespaces.network`**
   - `network = false` 时，策略已经是禁网态
   - 这时如果还配置 `network_policy`，解析会直接报错，因为“先禁网，再配细粒度联网规则”本身就是自相矛盾

2. **默认构建与 `ebpf` 构建的行为不同**
   - 只要 `network_policy.enabled = true`，Cerberus 就会在执行前检查 eBPF backend 是否可用
   - 只要 `network_policy.enabled = true`，运行时也会强制走带 sandbox/runtime gating 的执行路径，不会因为策略里没开其他 namespace / 资源限制就退回 direct execution
   - **默认构建 / backend 不可用**：`monitor` 和 `enforce` 都会 fail-closed，拒绝执行；`mode = "enforce"` 的报错会更明确，直接说明缺少 eBPF enforcement backend
   - **启用 `ebpf` feature 的构建**：存在真实的 matcher / enforcer / audit backend 路径，`monitor` 与 `enforce` 不再只是展示字段，而会进入实际网络事件处理流程

3. **`enabled = false` 时只是保留配置对象**
   - 这种情况下对象会被解析和展示
   - 但不会进入真正的运行时拦截路径

所以，今天应把 `network_policy` 理解成一条**带 feature gate 的真实能力链路**：

- 没开 `ebpf` 时，它体现为严格的 fail-closed 门禁
- 开了 `ebpf` 时，它才进入真实的 host / CIDR / port 匹配与 monitor / enforce 流程

如果你在默认构建里看到 `network_policy` 被拒绝，不是“文档保守”，而是当前 build 本身就没有打开那条 backend。

---

## 内置 profile 对照

| Profile | 适用场景 | 超时 | 内存 | 进程数 | 联网 | fallback |
|---------|----------|------|------|--------|------|----------|
| `workspace-write-network-on` | 本地可信命令，工作区可写、允许联网、偏开发友好 | 120s | 1 GiB | 不限 | 允许 | 仅放宽 Landlock 缺失和 mount 降级路径，不能绕过 namespace 或 `network_policy` 门禁 |
| `workspace-write-network-off` | 工作区可写、禁止联网、强调 fail-closed | 30s | 256 MiB | 50 | 禁止 | 全部关闭 |
| `workspace-write-network-on-dev-env` | 工作区可写、允许联网、保留开发环境变量 | 60s | 512 MiB | 100 | 允许 | 全部关闭 |

### CLI 对内置 profile 的额外运行时语义

- `cerberus exec` 默认使用 `workspace-write-network-on-dev-env`
- 对内置 profile 执行 `exec` 或 `profile show` 时，CLI 会额外注入当前 workspace 的 `readwrite` 访问
- 这意味着 `workspace-write-network-off` 在 CLI 的“有效执行策略”里，仍然会保留它原本的禁网、收紧环境变量、强 namespace/fail-closed 倾向，但**不会**再默认拒绝当前 workspace 的读写
- 如果你需要一个**完全由文件决定、没有 CLI 自动 workspace 注入**的策略边界，请使用 `--policy-file` 或显式的 `config/cerberus-policies/*.toml`

### 内置 profile 的环境变量差异

- `workspace-write-network-on`：`PATH`、`LANG`、`HOME`、`USER`、`TERM`、`SHELL`、`PWD`
- `workspace-write-network-off`：`PATH`、`LANG`、`TERM`
- `workspace-write-network-on-dev-env`：在常规变量之外，还会加入 `LC_ALL`、`EDITOR`、`VISUAL`、`RUST_BACKTRACE`、`RUST_LOG`、`CARGO_HOME`、`RUSTUP_HOME`、`NODE_PATH`、`NPM_CONFIG`、`PYTHONPATH`、`GOPATH`、`GOBIN`

---

## 常见陷阱

### 1. 继续按旧语义理解 `network`

错。当前公开语义已经改成“是否允许联网”。

- `true` = 允许联网
- `false` = 禁止联网

### 2. 在配置里写不存在的字段

当前实现会直接报错，不会悄悄忽略。

### 3. 以为配置文件解析失败会自动回退

不会。文件存在但有误时，Cerberus 会直接报错。

### 4. 把 `0` 写成资源“不限”

当前实现下不安全。更稳妥的做法是直接省略该字段。

### 5. 把默认构建下的 fail-closed 门禁，误解成“`network_policy` 完全没实现”

也不对。当前实现是分 build 的：

- 默认构建下，启用 `network_policy` 会因为缺少 eBPF backend 而 fail-closed
- 启用 `ebpf` feature 后，存在真实的监控 / 强制执行路径

### 6. 把“内置 profile 会自动注入 workspace”理解成“所有策略都会自动注入 workspace”

也不对。当前自动注入只发生在 CLI 使用内置 profile 时。

- `cerberus --profile workspace-write-network-off exec -- ...`：会注入当前 workspace 的 `readwrite`
- `cerberus --policy-file ./my-policy.toml exec -- ...`：不会注入，行为完全由文件内容决定

### 7. 把 workspace 的 `readwrite` 注入理解成“可以执行 workspace 里的文件”

也不对。`readwrite` 只表示可读可写，**不包含执行权限**。

- 想让工作区里的脚本或二进制可执行，仍然要显式给对应路径 `readexecute`
- 内置 profile 的自动注入只解决“当前目录可读写”的问题，不覆盖“当前目录可执行”

---

## 自定义策略示例

### 1. 类似 `workspace-write-network-off` 的自定义策略

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = true
proc_filesystem = true
wsl_paths = false

[namespaces]
mount = true
pid = true
network = false
user = true

[resources]
timeout_secs = 30
max_memory_bytes = 268435456
max_processes = 50

[environment]
whitelist = ["PATH", "LANG", "TERM"]
```

### 2. 类似 `workspace-write-network-on-dev-env` 的联网开发策略

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = true
proc_filesystem = true
wsl_paths = true

[[custom_paths]]
path = "/etc/resolv.conf"
permission = "readonly"

[[custom_paths]]
path = "/etc/hosts"
permission = "readonly"

[[custom_paths]]
path = "/etc/nsswitch.conf"
permission = "readonly"

[[custom_paths]]
path = "/etc/ssl/certs"
permission = "readonly"

[namespaces]
mount = true
pid = true
network = true
user = false

[resources]
timeout_secs = 60
max_memory_bytes = 536870912
max_processes = 100

[environment]
whitelist = [
  "PATH", "LANG", "LC_ALL", "HOME", "USER", "TERM", "SHELL", "PWD",
  "EDITOR", "VISUAL", "RUST_BACKTRACE", "RUST_LOG", "CARGO_HOME", "RUSTUP_HOME",
  "NODE_PATH", "NPM_CONFIG", "PYTHONPATH", "GOPATH", "GOBIN"
]
```

---

## 相关命令

```bash
# 查看可用 profile
cerberus profile list

# 查看某个 profile 的展开结果、运行时能力与 enforcement level
cerberus profile show workspace-write-network-off

# 使用显式配置文件执行
cerberus --policy-file ./config/cerberus-policies/my-policy.toml exec -- ls -la

# 查看已记录的执行历史
cerberus history
```

## 历史记录与数据库位置

`cerberus` CLI 会把执行历史写入本地 SQLite 数据库。

- `cerberus exec -- ...`：执行完成后写入一条记录
- `cerberus history`：从同一个数据库读取记录

当前实现下，数据库路径不是策略文件配置项，而是 CLI 自己按平台默认 data directory 解析出来的：

```text
<data-dir>/cerberus/cerberus.db
```

常见情况：

- Linux：`$XDG_DATA_HOME/cerberus/cerberus.db`
- Linux 未设置 `XDG_DATA_HOME`：`~/.local/share/cerberus/cerberus.db`
- macOS：`~/Library/Application Support/cerberus/cerberus.db`
- Windows：`%LOCALAPPDATA%\cerberus\cerberus.db`

它和 policy TOML 无关：`path_groups`、`custom_paths`、`namespaces` 等字段都不会改变这个数据库位置。

更完整的说明见下文档：

- [History Storage](./storage.md)

---

## 相关文档

- [Cerberus README](../README.md)
- [Runtime Capability Matrix](./runtime-capability-matrix.md)
- [History Storage](./storage.md)
- [Scenario Comparisons](./03.scenario-comparisons.md)
