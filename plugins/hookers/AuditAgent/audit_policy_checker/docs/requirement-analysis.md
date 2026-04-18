# xiaoO Agent 动作审计插件 — 需求分析文档

## 1. 项目背景

### 1.1 xiaoO

xiaoO 是一个 AI Agent 框架，其核心流程为：

1. 用户输入 prompt（对应唯一的一个 session）
2. Agent 大模型（LLM）分析 prompt 以及该 session 中的历史执行动作，决定下一步动作 `a_next`
3. 执行 `a_next`
4. 循环 2-3 直到任务完成

xiaoO 在步骤 2 和步骤 3 之间插入了一个审计钩子：在 LLM 返回 `a_next` 但尚未执行时，调用审计插件进行权限校验。

### 1.2 Cerberus 策略体系

Cerberus 是 AgentOS 中的沙箱策略引擎，采用类似 SELinux 的声明式策略（policy），以 TOML 格式定义 agent 动作的执行权限边界。策略涵盖以下维度：

| 维度 | 说明 |
|------|------|
| **文件系统访问** | `path_groups`（预定义路径组）+ `custom_paths`（自定义路径规则） |
| **命名空间隔离** | `namespaces`（mount/pid/network/user 隔离开关） |
| **资源限制** | `resources`（超时/内存/进程数上限） |
| **环境变量** | `environment`（白名单控制） |
| **网络策略** | `network_policy`（细粒度出站规则，需 ebpf feature） |

### 1.3 核心思路

将用户输入的 prompt 语义映射为最小权限的 Cerberus policy，再检查 agent 的下一步动作是否在该 policy 允许范围内，从而实现"最小权限 + 动作审计"的双重保障。

---

## 2. 功能需求

### 2.1 工具定位

- **名称**：`audit_policy_checker`（暂定）
- **角色**：xiaoO 的插件工具，在 agent 执行下一步动作前被调用
- **粒度**：per-session（每个 session 有独立的 session id 和独立的 policy 上下文）

### 2.2 输入定义

| 参数 | 类型 | 说明 |
|------|------|------|
| `session_id` | `str` | 当前会话唯一标识 |
| `prompt_session` | `str` | 该 session 用户输入的原始 prompt |
| `action_history` | `list[dict]` | 该 session 的 agent 历史执行动作序列，格式为 `[{"name": "a1"}, {"name": "a2"}, ...]`，当前每个元素只包含 `name` 字段，后续根据实际情况扩展 |
| `a_next` | `dict` | 下一步待执行动作，包含 `action_type`（动作类型，如 `shell_command`、`file_read`、`file_write`、`network_request` 等）、`action_detail`（动作具体内容，如具体命令、文件路径、URL 等） |
| `reason` | `str` | agent 给出的执行该下一步动作的理由 |

### 2.3 输出定义

| 场景 | 返回字段 | 说明 |
|------|----------|------|
| **Deny** | `{"decision": "Deny", "reason": "详细拒绝原因", "violated_policy": "违背的 policy 条款描述"}` | 动作不在允许的 policy 范围内 |
| **Allow** | `{"decision": "Allow", "policy": "<TOML格式的policy字符串>", "policy_explanation": "policy 各项设置的简要说明"}` | 动作在允许的 policy 范围内，附带生成的最小 policy |

### 2.4 核心处理流程

```
┌─────────────────────────────────────────────────────────────────┐
│                        输入                                      │
│  session_id, prompt_session, action_history, a_next, reason     │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 0: 查询 Policy 缓存                                        │
│                                                                  │
│  检查 session_id 对应的 policy 是否已缓存                        │
│  - 命中缓存 → 跳至 Step 2                                       │
│  - 未命中 → 进入 Step 1                                         │
└───────────────────────────┬─────────────────────────────────────┘
                            │ (未命中缓存)
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 1: PROMPT1 → LLM → 生成最小 policy                         │
│                                                                  │
│  输入: prompt-policy映射文档 + prompt_session                    │
│  调用: GLM-5.1（OpenAI兼容API）                                 │
│  输出: policy_a_next (TOML格式的最小权限policy)                  │
│  缓存: 将生成的 policy 缓存到 session_id 对应的缓存中            │
│  超时: 5秒                                                       │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 2: PROMPT2 → LLM → 判定动作合规性                          │
│                                                                  │
│  输入: policy_a_next + a_next + reason + action_history          │
│  调用: GLM-5.1（OpenAI兼容API）                                 │
│  输出: 合规性判定结果 + 详细分析                                  │
│  超时: 4秒                                                       │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 3: 汇总决策                                                 │
│                                                                  │
│  合规 → Return Allow + policy_a_next                            │
│  违规 → Return Deny + 违背原因                                   │
└─────────────────────────────────────────────────────────────────┘
```

**总超时**：10 秒
- Step 0（缓存查询）：本地操作，< 1ms
- Step 1（首次生成 policy）：LLM 调用 ≤ 5s
- Step 2（合规性判定）：LLM 调用 ≤ 4s
- Step 3（汇总决策）：本地操作，< 1ms
- 缓存命中时，总耗时 ≈ Step 2 + Step 3 ≤ 5s

---

## 3. Prompt-Policy 映射参考文档

> 以下文档由 Cerberus 官方配置文档和示例策略文件总结而来，将作为 PROMPT1 的上下文传入 LLM，指导其根据用户 prompt 生成合适的最小权限 policy。

### 3.1 策略字段速查

#### 3.1.1 顶层字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `landlock_optional` | bool | false | Landlock 不可用时是否允许继续执行 |
| `mount_isolation_fallback` | bool | false | Landlock 不可用时是否退化到 mount 隔离 |

#### 3.1.2 path_groups — 预定义路径组

| 字段 | true 时允许的访问 | 实际路径 |
|------|-------------------|----------|
| `system_binaries` | 读+执行 | `/usr/bin` |
| `system_libraries` | 读/读+执行 | `/usr/lib`, `/usr/lib64`, `/lib`, `/lib64`, `/usr/share`, `/etc/ld.so.cache`, `/etc/localtime`, `/etc/locale.alias` |
| `temp_directories` | 读+写 | `/tmp` |
| `device_files` | 读+写 | `/dev` |
| `proc_filesystem` | 只读 | `/proc` |
| `network_config` | 只读 | `/etc`（包含 passwd/shadow 等，需谨慎） |
| `wsl_paths` | 只读 | `/mnt/wsl` |

#### 3.1.3 custom_paths — 自定义路径规则

```toml
[[custom_paths]]
path = "/workspace"           # 绝对路径
permission = "readwrite"      # readonly | readwrite | readexecute
```

#### 3.1.4 namespaces — 命名空间隔离

| 字段 | true 含义 | false 含义 |
|------|-----------|------------|
| `mount` | 启用挂载隔离 | 与宿主共享挂载视图 |
| `pid` | 启用 PID 隔离 | 可见宿主进程视图 |
| `network` | 允许联网 | 禁止联网 |
| `user` | 启用 user namespace | 使用真实 UID/GID |

> 注意：`network = true` 时通常需设 `user = false`（user namespace 与网络访问不兼容）。

#### 3.1.5 resources — 资源限制

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeout_secs` | u64 | 30 | 最大执行时长（秒） |
| `max_memory_bytes` | u64 | 268435456 | 地址空间上限（字节） |
| `max_processes` | u32 | 省略=不限 | 最大进程数 |

#### 3.1.6 environment — 环境变量白名单

```toml
[environment]
whitelist = ["PATH", "LANG", "HOME"]
```

#### 3.1.7 network_policy — 细粒度网络规则（可选）

```toml
[network_policy]
enabled = true
mode = "monitor"              # monitor | enforce
default_action = "deny"       # allow | deny

[[network_policy.rules]]
action = "allow"
direction = "outbound"        # inbound | outbound
protocol = "tcp"              # tcp | udp
hosts = ["example.com"]
cidrs = ["192.168.1.0/24"]
ports = [[80, 80], [443, 443]]
```

> 注意：`network_policy` 需要 ebpf feature 支持；`namespaces.network = false` 时配置 `network_policy` 会报错。

### 3.2 预置策略 Profile 对照

| Profile | 适用场景 | 超时 | 内存 | 进程数 | 联网 | 环境变量 |
|---------|----------|------|------|--------|------|----------|
| **minimal** | 本地可信命令，偏开发友好 | 120s | 1 GiB | 不限 | 允许 | PATH,LANG,HOME,USER,TERM,SHELL,PWD |
| **strict** | 不可信命令，强调 fail-closed | 30s | 256 MiB | 50 | 禁止 | PATH,LANG,TERM |
| **llm-safe** | AI 编码助手 | 60s | 512 MiB | 100 | 允许 | 含开发工具变量（CARGO_HOME, PYTHONPATH 等） |
| **high-security-agent** | 自主 agent 步骤，最小攻击面 | 45s | 256 MiB | 20 | 禁止 | PATH,LANG,HOME,TERM |
| **process-visible** | 需查看宿主进程的场景 | 120s | 1 GiB | 不限 | 允许 | PATH,LANG,HOME,USER,TERM,SHELL,PWD |

### 3.3 场景 → Policy 映射指南

以下为"用户 prompt 语义 → 推荐 policy 配置"的映射参考，供 LLM 生成 policy 时使用。

#### 场景 A：纯本地文件读取/分析（无需联网）

**典型 prompt**：
- "帮我读取 `/var/log/app.log` 并分析错误"
- "查看 `/workspace/src/main.py` 的代码结构"
- "统计 `/data` 目录下的文件数量"

**推荐 policy 关键配置**：

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = false
device_files = false
proc_filesystem = false
network_config = false
wsl_paths = false

[[custom_paths]]
path = "<用户prompt中提到的具体路径>"
permission = "readonly"

[namespaces]
mount = true
pid = true
network = false     # 无需联网
user = true

[resources]
timeout_secs = 60
max_memory_bytes = 268435456
max_processes = 20

[environment]
whitelist = ["PATH", "LANG", "HOME", "TERM"]
```

**要点**：
- `network = false`：禁止联网
- `custom_paths` 只开放 prompt 中提到的路径，权限为 `readonly`
- 不开放 `device_files`、`proc_filesystem`、`network_config`

#### 场景 B：本地代码编写/修改（无需联网）

**典型 prompt**：
- "在 `/workspace/src/` 下创建一个新的 Python 模块"
- "修改 `/workspace/config.yaml` 中的配置项"
- "重构 `/workspace/src/utils.py`"

**推荐 policy 关键配置**：

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = false
proc_filesystem = false
network_config = false
wsl_paths = false

[[custom_paths]]
path = "<工作目录>"
permission = "readwrite"

[namespaces]
mount = true
pid = true
network = false     # 无需联网
user = true

[resources]
timeout_secs = 120
max_memory_bytes = 536870912
max_processes = 50

[environment]
whitelist = ["PATH", "LANG", "HOME", "USER", "TERM", "SHELL", "PWD",
             "EDITOR", "VISUAL", "PYTHONPATH", "GOPATH"]
```

**要点**：
- `custom_paths` 开放工作目录的 `readwrite` 权限
- 资源限制较场景 A 宽松（更多内存、更长超时、更多进程数）
- 环境变量包含开发工具相关变量
- 仍然禁止联网

#### 场景 C：需要联网的开发任务

**典型 prompt**：
- "安装 Python 包 `requests` 并编写示例代码"
- "从 GitHub 克隆一个仓库到 `/workspace`"
- "查询 API 文档并编写调用代码"
- "使用 pip install 安装依赖"

**推荐 policy 关键配置**：

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = true
proc_filesystem = true
network_config = true     # 联网需要 DNS 解析
wsl_paths = true

[[custom_paths]]
path = "<工作目录>"
permission = "readwrite"

[namespaces]
mount = true
pid = true
network = true           # 允许联网
user = false             # network=true 时 user 必须为 false

[resources]
timeout_secs = 120
max_memory_bytes = 536870912
max_processes = 100

[environment]
whitelist = [
    "PATH", "LANG", "LC_ALL", "HOME", "USER", "TERM", "SHELL", "PWD",
    "EDITOR", "VISUAL", "RUST_BACKTRACE", "RUST_LOG",
    "CARGO_HOME", "RUSTUP_HOME", "NODE_PATH", "NPM_CONFIG",
    "PYTHONPATH", "GOPATH", "GOBIN",
    "HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy",
    "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"
]
```

**要点**：
- `network = true`，同时 `user = false`
- `network_config = true`（开放 `/etc` 只读，用于 DNS 解析）
- 环境变量包含代理设置
- 资源限制与 `llm-safe` profile 对齐

#### 场景 D：系统运维/管理任务

**典型 prompt**：
- "检查系统磁盘使用情况"
- "查看当前运行的进程列表"
- "检查系统网络连接状态"
- "查看系统日志 `/var/log/syslog`"

**推荐 policy 关键配置**：

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = true
proc_filesystem = true       # 需要读取 /proc
network_config = true        # 可能需要读取 /etc 中的配置
wsl_paths = false

[[custom_paths]]
path = "<指定的系统路径，如 /var/log>"
permission = "readonly"

[namespaces]
mount = true
pid = false                  # 需要看到宿主进程
network = true               # 可能需要检查网络状态
user = false

[resources]
timeout_secs = 60
max_memory_bytes = 268435456
max_processes = 50

[environment]
whitelist = ["PATH", "LANG", "HOME", "USER", "TERM", "SHELL", "PWD"]
```

**要点**：
- `pid = false`：需要看到宿主进程信息
- `proc_filesystem = true`：需要读取 `/proc`
- `custom_paths` 对系统路径只给 `readonly`
- 是否允许联网视具体任务而定

#### 场景 E：不可信/高风险任务

**典型 prompt**：
- "执行用户上传的脚本"
- "运行第三方提供的可执行文件"
- "处理来源不明的数据文件"

**推荐 policy 关键配置**：

```toml
landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = false
proc_filesystem = false
network_config = false
wsl_paths = false

[namespaces]
mount = true
pid = true
network = false              # 禁止联网
user = true

[resources]
timeout_secs = 30            # 最短超时
max_memory_bytes = 134217728 # 128 MiB，最严格
max_processes = 10           # 最少进程数

[environment]
whitelist = ["PATH", "LANG", "TERM"]
```

**要点**：
- 最严格的资源限制
- 禁止联网，不暴露系统配置
- 最小环境变量集
- 参照 `strict` + `high-security-agent` 的组合

### 3.4 Policy 生成原则

1. **最小权限**：只开放 prompt 任务所需的最低权限，不预先授权可能用到的权限
2. **默认拒绝**：`path_groups` 默认全部 `false`，按需开启；`network` 默认 `false`
3. **路径精确**：`custom_paths` 优先使用 prompt 中提到的具体路径，避免开放过大父目录
4. **权限精确**：只需读取的路径给 `readonly`，只需执行的给 `readexecute`，确需写入的才给 `readwrite`
5. **资源适度**：根据任务复杂度设置合理的资源限制，简单的读取任务不需要 1GiB 内存
6. **环境最小**：环境变量白名单只包含任务所需的最小集合
7. **联网审慎**：`network_config = true` 会开放整个 `/etc` 只读，仅在明确需要联网时开启
8. **命名空间兼容**：`network = true` 时 `user` 必须设为 `false`

---

## 4. Prompt 模板设计

### 4.1 PROMPT1 模板：根据用户 prompt 生成最小 policy

```
你是一个 Cerberus 安全策略专家。根据用户输入的 prompt，生成该 prompt 允许执行的最小权限策略（policy）。

## Cerberus Policy 格式说明

策略以 TOML 格式输出，包含以下字段：

### 顶层字段
- landlock_optional (bool): Landlock 不可用时是否允许继续执行，默认 false
- mount_isolation_fallback (bool): Landlock 不可用时是否退化到 mount 隔离，默认 false

### path_groups — 预定义路径组
- system_binaries: 读+执行 /usr/bin
- system_libraries: 读/读+执行 /usr/lib, /lib 等
- temp_directories: 读+写 /tmp
- device_files: 读+写 /dev
- proc_filesystem: 只读 /proc
- network_config: 只读 /etc（注意：包含 passwd/shadow，需谨慎）
- wsl_paths: 只读 /mnt/wsl

### custom_paths — 自定义路径规则
- path: 绝对路径
- permission: readonly | readwrite | readexecute

### namespaces — 命名空间隔离
- mount: 启用挂载隔离
- pid: 启用 PID 隔离（false 则可见宿主进程）
- network: true=允许联网, false=禁止联网
- user: 启用 user namespace（注意：network=true 时 user 必须为 false）

### resources — 资源限制
- timeout_secs: 最大执行时长（秒）
- max_memory_bytes: 地址空间上限（字节）
- max_processes: 最大进程数（省略=不限）

### environment — 环境变量白名单
- whitelist: 允许透传的变量名列表

### network_policy — 细粒度网络规则（可选）
- enabled: 是否启用
- mode: monitor | enforce
- default_action: allow | deny
- rules: 包含 action, direction, protocol, hosts, cidrs, ports

## 场景 → Policy 映射参考

{prompt_policy_mapping_doc}

## Policy 生成原则

1. 最小权限：只开放 prompt 任务所需的最低权限
2. 默认拒绝：path_groups 默认全部 false，按需开启；network 默认 false
3. 路径精确：custom_paths 使用 prompt 中提到的具体路径
4. 权限精确：只需读取给 readonly，只需执行给 readexecute，确需写入才给 readwrite
5. 资源适度：根据任务复杂度设置合理的资源限制
6. 环境最小：环境变量白名单只包含任务所需的最小集合
7. 联网审慎：network_config=true 会开放整个 /etc 只读，仅在明确需要联网时开启
8. 命名空间兼容：network=true 时 user 必须设为 false

## 用户 Prompt

{prompt_session}

## 输出要求

请直接输出 TOML 格式的策略文件内容，不要输出任何解释性文字。策略应该是该 prompt 允许执行的最小权限集合。
```

### 4.2 PROMPT2 模板：判定动作是否在 policy 允许范围内

````
你是一个 Cerberus 安全策略审计员。你的任务是判断一个待执行的动作是否在给定的策略（policy）允许范围内。

## 当前生效的策略（policy）

```toml
{policy_a_next}
```

## 待执行的动作

- 动作类型: {action_type}
- 动作详情: {action_detail}

## 执行理由

{reason}

## 历史动作序列

{action_history_formatted}

## 审计要点

请从以下维度分析该动作是否违背策略：

1. **文件系统访问**：动作是否需要访问策略未授权的路径？访问模式（读/写/执行）是否与策略中的 permission 一致？
2. **网络访问**：动作是否需要联网？策略是否允许（network=true）？如果策略包含 network_policy，动作的目标主机/端口是否在允许范围内？
3. **命名空间**：动作是否需要查看宿主进程（pid=false 时可见）？策略的 PID 隔离是否支持？
4. **资源消耗**：动作是否可能超出策略规定的超时、内存或进程数限制？
5. **环境变量**：动作是否依赖策略未包含的环境变量？
6. **动作合理性**：结合执行理由和历史动作序列，该动作是否与完成用户 prompt 的目标一致？是否存在"借任务之名行越权之实"的风险？

## 输出格式

请以 JSON 格式输出，包含以下字段：

```json
{
  "compliant": true/false,
  "analysis": "详细分析过程，逐一说明各审计维度的判断结果",
  "violation_detail": "如果不合规，具体违背了哪些 policy 条款；如果合规，此字段为空字符串"
}
```

请严格按上述 JSON 格式输出，不要输出其他内容。
````

---

## 5. 接口设计

### 5.1 主入口函数

```python
def audit_action(
    session_id: str,
    prompt_session: str,
    action_history: list[dict],
    a_next: dict,
    reason: str,
) -> dict:
    """
    审计 agent 的下一步动作是否在用户 prompt 允许的最小 policy 范围内。

    Args:
        session_id: 会话唯一标识
        prompt_session: 用户输入的原始 prompt
        action_history: 历史动作序列，格式为 [{"name": "a1"}, {"name": "a2"}, ...]
        a_next: 下一步动作，含 action_type 和 action_detail
        reason: 执行该动作的理由

    Returns:
        dict: {"decision": "Allow"|"Deny", "policy": "...", "reason": "..."}
    """
```

### 5.2 Policy 缓存接口

```python
def get_or_generate_policy(session_id: str, prompt_session: str) -> str:
    """
    获取或生成 session 对应的 policy。
    同一 session 中同一 prompt 的 policy 只生成一次，后续从缓存读取。

    Args:
        session_id: 会话唯一标识
        prompt_session: 用户输入的原始 prompt

    Returns:
        str: TOML 格式的 policy 字符串
    """

# TODO: 预留接口 — 当历史动作序列中存在用户重新指定 prompt 时，需要使缓存失效并重新生成 policy。
# 当前版本暂不实现该逻辑，后续版本可根据 action_history 中的特定标记
# （如 action_type == "user_new_prompt"）触发 policy 重新生成。
# 示例接口：
# def invalidate_policy_cache(session_id: str) -> None:
#     """使指定 session 的 policy 缓存失效，下次调用时重新生成"""
```

### 5.3 Network Policy 预留接口

```python
# TODO: 预留接口 — 细粒度 network_policy 生成。
# 当前版本只使用 namespaces.network 布尔开关控制联网权限，
# 不生成细粒度的 network_policy（含 hosts/cidrs/ports 规则）。
# 后续版本可在此接口中实现基于 prompt 语义的细粒度网络规则生成。
# 示例接口：
# def generate_network_policy(prompt_session: str) -> dict | None:
#     """
#     根据用户 prompt 生成细粒度的 network_policy。
#     返回 None 表示不需要细粒度规则，使用 namespaces.network 布尔开关即可。
#     返回 dict 表示需要细粒度规则，格式与 Cerberus network_policy 一致。
#     """
```

### 5.4 LLM 调用接口

```python
def call_llm(prompt: str, timeout: float = 5.0) -> str:
    """
    调用 GLM-5.1 大模型（通过 OpenAI 兼容 API 协议）。

    Args:
        prompt: 完整的 prompt 文本
        timeout: 超时时间（秒）

    Returns:
        str: 模型返回的文本

    Raises:
        TimeoutError: 超时
    """
```

### 5.5 辅助函数

```python
def format_action_history(action_history: list[dict]) -> str:
    """将动作历史格式化为可读文本，每个元素当前只包含 name 字段"""

def parse_policy_from_llm(response: str) -> str:
    """从 LLM 响应中提取 TOML 格式的 policy"""

def parse_compliance_from_llm(response: str) -> dict:
    """从 LLM 响应中解析合规性判断结果"""
```

---

## 6. 非功能需求

### 6.1 性能要求

| 指标 | 要求 |
|------|------|
| 总处理时间 | ≤ 10 秒（含 2 次 LLM 调用） |
| PROMPT1 LLM 调用超时 | 5 秒 |
| PROMPT2 LLM 调用超时 | 4 秒 |
| 本地处理时间 | ≤ 1 秒 |

### 6.2 可靠性要求

| 场景 | 处理策略 |
|------|----------|
| LLM 调用超时 | 返回 Deny，理由为"策略生成/审计超时，出于安全考虑拒绝执行" |
| LLM 返回格式错误 | 尝试解析，解析失败则返回 Deny，理由为"策略解析失败" |
| LLM 返回的 policy TOML 格式非法 | 返回 Deny，理由为"生成的策略格式无效" |
| 网络异常 | 返回 Deny，理由为"LLM 服务不可用" |

> **安全原则**：任何异常情况默认 Deny（fail-closed），宁可误拒不可误放。

### 6.3 可扩展性

- PROMPT1 和 PROMPT2 模板以文件形式存储，支持热更新
- prompt-policy 映射文档以文件形式存储，支持增量更新
- LLM 模型可配置（当前默认 GLM-5.1，后续可切换）

### 6.4 日志要求

每次调用记录以下信息：
- `session_id`
- 输入参数摘要
- PROMPT1 和 PROMPT2 的完整内容（调试用）
- LLM 返回结果
- 最终决策及理由
- 各步骤耗时

---

## 7. 项目结构（建议）

```
audit_policy_checker/
├── __init__.py
├── main.py                  # 主入口，audit_action 函数
├── llm_client.py            # LLM 调用封装（OpenAI 兼容 API，调用 GLM-5.1）
├── prompt_templates.py      # PROMPT1/PROMPT2 模板定义
├── parsers.py               # LLM 返回结果解析
├── policy_cache.py          # Session 级别 policy 缓存管理
├── config.py                # 配置项（LLM API endpoint、超时、API key 等）
├── policy_mapping_doc.py    # prompt-policy 映射文档（嵌入代码或加载文件）
├── logging_utils.py         # 日志工具
└── templates/
    ├── prompt1_template.txt  # PROMPT1 模板文件
    ├── prompt2_template.txt  # PROMPT2 模板文件
    └── policy_mapping.md     # prompt-policy 映射参考文档
```

---

## 8. 依赖项

| 依赖 | 用途 | 版本建议 |
|------|------|----------|
| `openai` | OpenAI 兼容 API 客户端，调用 GLM-5.1 | ≥ 1.0 |
| `httpx` | 异步 HTTP 客户端（openai 底层依赖） | ≥ 0.25 |
| `toml` / `tomli` | TOML 解析与验证 | ≥ 2.0 |
| `pydantic` | 数据模型与校验 | ≥ 2.0 |
| `tenacity` | 重试机制 | ≥ 8.0 |
| `loguru` | 日志记录 | ≥ 0.7 |

---

## 9. 风险与应对

| 风险 | 影响 | 应对措施 |
|------|------|----------|
| LLM 生成不合规的 policy（权限过大） | 安全风险 | PROMPT2 进行二次校验；PROMPT1 中强调最小权限原则 |
| LLM 误判动作合规性 | 安全风险 | PROMPT2 提供详细审计维度；记录日志供事后审计 |
| 10 秒超时限制导致频繁 Deny | 用户体验 | 优化 prompt 长度；选择响应速度快的 LLM；预缓存常见 prompt 的 policy |
| prompt 歧义导致 policy 不准确 | 误拒/误放 | PROMPT1 中要求 LLM 对歧义按更严格方向解读 |
| TOML 解析失败 | 功能异常 | 增加格式修复逻辑；失败时 fail-closed |

---

## 10. 已确认事项

1. **LLM API 接入方式**：采用 OpenAI 兼容的 API 协议调用 GLM-5.1（即使用 OpenAI SDK 的接口格式，指向 GLM-5.1 的 endpoint）。
2. **a_next 的动作类型枚举**：当前版本以 `shell_command`、`file_read`、`file_write`、`network_request` 为基础类型，后续根据实际情况扩增。
3. **action_history 的数据格式**：历史动作序列为 `list[dict]`，格式为 `[{"name": "a1"}, {"name": "a2"}, ...]`，每个动作的 JSON 暂时只包含 `name` 字段，后续根据实际情况添加更多字段。
4. **session 级别 policy 缓存**：同一 session 中同一 prompt 对应的 policy 缓存生成一次即可，避免重复调用 LLM。预留重新生成 policy 的接口（当历史动作序列中存在用户重新指定 prompt 时需重新生成），当前版本暂不实现该接口逻辑，但代码中预留接口并注释说明。
5. **与 xiaoO 的集成方式**：当前版本按 Python 脚本被调用的方式处理，后续可能扩展为 SDK 调用等方式。
6. **network_policy 处理**：当前版本只依赖 `namespaces.network` 的布尔开关，不生成细粒度的 `network_policy`。代码中预留 `network_policy` 的生成接口，并注释说明后续可扩展。
7. **并发安全**：当前版本假设同一 session 的动作执行序列为顺序执行，暂不增加并发安全机制，后续如有需要再添加。

---

*文档版本：v0.2 | 更新日期：2026-04-11*
