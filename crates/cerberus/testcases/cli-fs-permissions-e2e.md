# Cerberus CLI 端到端测试剧本：Filesystem Permissions

本文档定义 **CLI 侧端到端预期行为**，重点覆盖 `custom_paths.permission` 的解析、展示、以及实际执行语义。

范围只包括：

- `cerberus profile show`
- `cerberus exec`
- `--policy-file`
- file-backed policy 中的相对路径解析
- `readonly` / `readwrite` / `readexecute` / `readwriteexecute`

不包括：

- network policy 的 eBPF 行为
- `/etc` 路径组历史兼容
- host adapter 安装流程

---

## 统一约定

- 测试环境：Linux
- `WORKDIR`：临时目录，例如 `/tmp/cerberus-e2e-<id>`
- `POLICY_FILE`：临时 policy 文件路径
- `CERBERUS`：CLI 可执行入口，例如 `cargo run -p cerberus-cli --`
- 除非特别说明，命令退出码 `0` 代表成功，非 `0` 代表失败
- 如果某条剧本依赖 execute 语义而当前运行时后端无法精确 enforce，**预期行为是 fail closed**，而不是静默降级放过

---

## Fixture A：最小 file-backed policy 模板

```toml
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
network = false
user = true

[resources]
timeout_secs = 30

[environment]
whitelist = ["PATH", "LANG", "HOME", "USER", "TERM", "SHELL", "PWD"]
```

---

## TC-001：`profile show` 对 built-in profile 仍然只注入 workspace `readwrite`

### 目的

确认 built-in profile 的 CLI 注入行为没有因为新增 `readwriteexecute` 而被扩大。

### 步骤

1. 在任意仓库工作目录下运行：

```bash
${CERBERUS} profile show workspace-write-network-on
```

### 期望

- 输出中出现当前 workspace 路径
- 该路径显示为 `readwrite`
- 输出中**不应**把 workspace 默认显示为 `readexecute`
- 输出中**不应**把 workspace 默认显示为 `readwriteexecute`

---

## TC-002：`profile show` 能正确展示 file-backed policy 里的 `readwriteexecute`

### 步骤

1. 创建 `POLICY_FILE`：

```toml
# 追加到 Fixture A 后面
[[custom_paths]]
path = "/tmp/cerberus-e2e/bin"
permission = "readwriteexecute"
```

2. 运行：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" profile show custom
```

### 期望

- CLI 成功解析 policy
- 输出中包含 `/tmp/cerberus-e2e/bin`
- 权限展示为 `readwriteexecute`
- 不能被错误显示成 `readwrite` 或 `readexecute`

---

## TC-003：`readwriteexecute` 允许“生成后立刻执行”

### 目的

验证真实存在的 `R + W + X` use case。

### 准备

1. 创建目录：

```bash
mkdir -p "${WORKDIR}/bin"
```

2. policy 追加：

```toml
[[custom_paths]]
path = "./bin"
permission = "readwriteexecute"
```

### 步骤

1. 在 `${WORKDIR}` 下执行：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc 'printf "#!/bin/sh\necho generated-ok\n" > ./bin/gen.sh && chmod +x ./bin/gen.sh && ./bin/gen.sh'
```

### 期望

- 命令成功
- stdout 包含 `generated-ok`
- 不应因为“可写但不可执行”被拒绝

---

## TC-004：`readwrite` 不允许执行

### 准备

1. 创建目录：

```bash
mkdir -p "${WORKDIR}/bin"
printf "#!/bin/sh\necho should-not-run\n" > "${WORKDIR}/bin/noexec.sh"
chmod +x "${WORKDIR}/bin/noexec.sh"
```

2. policy 追加：

```toml
[[custom_paths]]
path = "./bin"
permission = "readwrite"
```

### 步骤

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc './bin/noexec.sh'
```

### 期望

- 命令失败
- 失败原因体现为 execute 权限不足，或者 sandbox capability / filesystem isolation 无法满足 execute 语义
- **不能**因为 mount fallback 粗糙实现就把它放过去

---

## TC-005：`readexecute` 不允许写

### 准备

1. 创建目录和脚本：

```bash
mkdir -p "${WORKDIR}/tool"
printf "#!/bin/sh\necho rx-ok\n" > "${WORKDIR}/tool/run.sh"
chmod +x "${WORKDIR}/tool/run.sh"
```

2. policy 追加：

```toml
[[custom_paths]]
path = "./tool"
permission = "readexecute"
```

### 步骤

1. 先执行已有脚本：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc './tool/run.sh'
```

2. 再尝试写入：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc 'printf x > ./tool/forbidden.txt'
```

### 期望

- 第一步成功，stdout 包含 `rx-ok`
- 第二步失败
- 失败原因体现为写权限不足

---

## TC-006：`readonly` 只允许读，不允许写，也不允许执行普通脚本

### 准备

1. 创建目录和文件：

```bash
mkdir -p "${WORKDIR}/ro"
printf "hello" > "${WORKDIR}/ro/input.txt"
printf "#!/bin/sh\necho should-not-run\n" > "${WORKDIR}/ro/no.sh"
chmod +x "${WORKDIR}/ro/no.sh"
```

2. policy 追加：

```toml
[[custom_paths]]
path = "./ro"
permission = "readonly"
```

### 步骤

1. 读取文件：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- cat ./ro/input.txt
```

2. 写入文件：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc 'printf fail > ./ro/out.txt'
```

3. 执行脚本：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" exec -- sh -lc './ro/no.sh'
```

### 期望

- 第一步成功，并输出 `hello`
- 第二步失败
- 第三步失败

---

## TC-007：file-backed policy 的相对路径解析对 `readwriteexecute` 同样成立

### 准备

1. 在 `${WORKDIR}` 下写入 policy：

```toml
[[custom_paths]]
path = "."
permission = "readwriteexecute"
```

### 步骤

1. 从 `${WORKDIR}` 启动：

```bash
${CERBERUS} --policy-file "./policy.toml" profile show custom
```

### 期望

- policy 成功解析
- 相对路径 `.` 被展开为 `${WORKDIR}`
- 展示权限为 `readwriteexecute`

---

## TC-008：未知 permission 字符串必须直接报错

### 步骤

1. 创建非法 policy：

```toml
[[custom_paths]]
path = "/tmp/x"
permission = "rwx"
```

2. 运行：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" profile show custom
```

### 期望

- CLI 失败退出
- 报错应明确指向 policy parse / invalid permission
- 不能静默接受，也不能偷偷映射成别的权限

---

## TC-009：built-in profile 的 workspace 注入与 file-backed policy 的显式 `readwriteexecute` 不应混淆

### 场景

- built-in profile：workspace 是 CLI 注入的 `readwrite`
- file-backed policy：workspace 可以被作者显式声明为 `readwriteexecute`

### 步骤

1. 对 built-in profile 执行：

```bash
${CERBERUS} profile show workspace-write-network-on-dev-env
```

2. 对 file-backed policy 执行：

```bash
${CERBERUS} --policy-file "${POLICY_FILE}" profile show custom
```

### 期望

- 两者都能展示 workspace / custom path
- built-in 仍显示 `readwrite`
- file-backed 可显示 `readwriteexecute`
- CLI 不应把 built-in 的默认注入偷偷扩大成 `readwriteexecute`

---

## TC-010：如果 execute 语义无法被当前后端精确 enforce，则 CLI 必须 fail closed

### 目的

防止 `readexecute` / `readwriteexecute` 在不支持精确 execute 隔离的 fallback 下被错误放行。

### 前提

- 当前运行环境无法使用 Landlock
- mount fallback 仍可用，但不能精确表达 execute 维度

### 步骤

1. 使用包含 `readexecute` 或 `readwriteexecute` 的 policy
2. 执行依赖该路径 execute 权限的命令

### 期望

- CLI 失败退出
- 报错应说明当前 runtime / backend 无法满足所请求的 filesystem execute 语义
- **不能**继续执行
- **不能**静默退化为“只要能挂载就算成功”

---

## 最小回归矩阵

| Case | profile show | exec-read | exec-write | exec-execute |
|------|--------------|-----------|------------|--------------|
| `readonly` | 成功显示 | 允许 | 拒绝 | 拒绝 |
| `readwrite` | 成功显示 | 允许 | 允许 | 拒绝 |
| `readexecute` | 成功显示 | 允许 | 拒绝 | 允许 |
| `readwriteexecute` | 成功显示 | 允许 | 允许 | 允许 |

---

## 审阅重点

审阅时重点确认以下问题：

1. `readwriteexecute` 是否应该作为正式公开权限值
2. built-in workspace 注入是否坚持保持 `readwrite`，而不是自动放大
3. execute 相关场景是否一律要求 exact enforcement，不能接受 silent downgrade
4. CLI 展示是否继续使用单字符串风格，还是后续改成 capability list
