# AgentMoss 环境变量参考

本文档汇总 AgentMoss 全部环境变量。

> audit_agent 已被 AgentMoss 取代：判定逻辑迁出为独立常驻服务，`AUDIT_*` 变量不再有效，按 §三 的 `AGENT_MOSS_*` 清单配置即可。

---

## 一、audit_agent 迁移说明

audit_agent 已被 AgentMoss 取代，`AUDIT_*` 变量不再有效。

---

## 二、配置优先级

所有通过 `get_agent_moss_setting()` 读取的变量，优先级统一为：

```
环境变量 > agent_moss_settings.json > 默认值
```

而层级开关（`is_layer_enabled`）额外多一层 runtime JSON：

```
AGENT_MOSS_DISABLE_*  env  >  runtime JSON（~/.config/agentmoss/agent_moss_runtime.json）  >  agent_moss_settings.json  >  默认 True
```

> env 永远最高优先级（覆盖一切），runtime JSON / Console 管理的是"未设 env 时的持久化偏好"。Console UI 会展示哪些开关被 env 接管（灰色不可改）。LLM 配置（API key/model/base_url 等）优先级另见下方"LLM 配置"小节。

---

## 三、AgentMoss 完整环境变量清单

### 3.1 层级开关

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_DISABLE_LLM` | 设 `1` 禁用层3 LLM 深度分析（仅跑层1+层2）| 未设（启用 L3）|
| `AGENT_MOSS_DISABLE_HEURISTIC` | 设 `1` 禁用层1 特征匹配静态检测 | 未设（启用 L1）|
| `AGENT_MOSS_DISABLE_LOGIC_RULES` | 设 `1` 禁用层2 逻辑规则检测 | 未设（启用 L2）|
| `AGENT_MOSS_LLM_FAIL_MODE` | LLM 失败策略：`fail_open`（默认，纯靠 L3 时 LLM 挂了→Allow+warn）/ `fail_closed`（→Deny）| `fail_open` |

### 3.2 LLM 调用

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_LLM_API_KEY` | LLM API Key（最高优先级，兼容 fallback `OPENROUTER_API_KEY` / `OPENAI_API_KEY`）| 未设 |
| `AGENT_MOSS_LLM_MODEL` | LLM 模型名称 | Python: `anthropic/claude-3.5-sonnet`；TS: `gpt-4o` |
| `AGENT_MOSS_LLM_BASE_URL` | LLM API 端点 | Python: `https://openrouter.ai/api/v1`；TS: `https://api.openai.com/v1` |
| `AGENT_MOSS_LLM_PROVIDER` | Provider 标识（决定注入哪些 HTTP headers，如 openrouter/xai）| 从 base_url 推断 |
| `AGENT_MOSS_LLM_TEMPERATURE` | 采样温度 | `0.1` |
| `AGENT_MOSS_LLM_MAX_TOKENS` | 最大输出 token 数 | `4096` |
| `AGENT_MOSS_LLM_TIMEOUT` | 单次调用超时（秒）| `300` |
| `AGENT_MOSS_LLM_RETRIES` | 失败重试次数（默认 2 = 共 3 次尝试）| `2` |
| `AGENT_MOSS_ENABLE_POLICY_GEN` | 设 `1` 启用策略生成（Step 2），否则用默认只读策略 | 未设（禁用）|

> **LLM 配置 fallback**：若用户只配了 xiaoo 的 `~/.config/xiaoo/config.toml`，AgentMoss 会自动继承其 `[llm]` 配置（字段名转译 `api_base` ↔ `base_url`），配一份 xiaoo 配置即可。

### 3.3 配置文件路径

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_CONFIG_PATH` | 主配置 YAML 路径 | `~/.config/agentmoss/agent_moss.yaml`（缺失自动创建，并继承 xiaoo config.toml）|
| `AGENT_MOSS_SETTINGS_PATH` | `agent_moss_settings.json` 路径 | 包内 `agent_moss/infra/agent_moss_settings.json` |
| `AGENT_MOSS_RUNTIME_CONFIG_PATH` | runtime JSON 路径（层级开关 + 规则启停持久化）| `~/.config/agentmoss/agent_moss_runtime.json` |
| `AGENT_MOSS_TOKEN_STATS_PATH` | token 用量统计 JSON 路径 | `~/.config/agentmoss/agent_moss_token_stats.json` |

### 3.4 自定义规则

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_CUSTOM_RULES` | 自定义规则 JSON 数组，注入到 L1 检测。格式：`[{"pattern": "kubectl delete namespace", "action": "Deny", "severity": "critical"}]` | `[]` |

### 3.5 Console（策略管控台）

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_CONSOLE_TOKEN` | Console Bearer 鉴权 token | 未设（本机 127.0.0.1 免鉴权，便于 iframe 嵌入）|

### 3.6 bridge.py（xiaoO 消费方连服务）

这些变量由 `plugins/hookers/agent_moss/bridge.py` 读取，用于 xiaoO hook 子进程定位并连接 AgentMoss 服务：

| 变量 | 说明 | 默认值 |
|---|---|---|
| `AGENT_MOSS_URL` | 完整服务 URL（最高优先级，如 `http://10.0.0.5:9095`）| 未设 |
| `AGENT_MOSS_HOST` | 服务监听地址 | `127.0.0.1` |
| `AGENT_MOSS_PORT` | 服务端口（被占时服务自动 findFreePort 往上找，bridge 探测 9090-9095 兜底）| `9090` |
| `AGENT_MOSS_HOOK_TIMEOUT` | analyze 请求超时（秒）| `60` |
| `AGENT_MOSS_HEALTH_TIMEOUT` | 活性检查超时（秒）| `2` |
| `AGENT_MOSS_INSTANCE` | **本机归属实例标识**。bridge 探测 `/api/v1/health` 时要求返回的 `instance` 字段等于该值，不等则跳过该端口（宁可 fail-closed 也不连到别的 agent 的 agentmoss），见 `plugins/hookers/agent_moss/bridge.py`。`install.sh` 安装时注入为 `xiaoo`（systemd 模式写 `/etc/agent_moss/agent_moss.env`，nohup 模式 export），见 `plugins/hookers/agent_moss/install.sh`。**用户显式设了 `AGENT_MOSS_URL` / `AGENT_MOSS_PORT` 时跳过探测，此值不生效**（尊重显式配置，`bridge.py`）。| `xiaoo` |
| `AGENT_MOSS_LOG_PATH` | **全量 hook 日志 + LLM prompt 日志路径**。bridge.py 记 HOOK_HEALTH_OK/HOOK_INPUT/HOOK_OUTPUT（每次 hook 调用都记，含 tool_input 和判定结果）；llm_analyzer.py 记 LLM 判定 prompt。两处写同一个文件 | 未设（不记）|
| `AGENT_MOSS_CHECK_SOURCE` | 设 `1` 时额外检查 agent_moss 包是否可 import | 未设 |

> 上表是 bridge.py 实际读取的**全部** env（`plugins/hookers/agent_moss/bridge.py`）。bridge 之外，还有一类由 bridge 从 xiaoo 配置读取后 **per-request 转发**给服务的变量：LLM 相关配置（如 API key）不在 bridge 进程 env 里读取，而是 bridge 解析 `~/.config/xiaoo/config.toml` + 相关 env 后随请求注入 `metadata.llm_config`（`bridge.py`）。

> 端口解析优先级（学 OpenDesk hook.ts resolveGateUrl）：`AGENT_MOSS_URL` > `AGENT_MOSS_PORT` 显式指定 > 探测 9090-9095 找 `/api/v1/health` 返回 healthy **且 `instance` 匹配 `AGENT_MOSS_INSTANCE`** > 默认 9090。

> 注：`AGENT_MOSS_LOG_PATH` 不依赖 L3 是否命中——即使命令在层1/层2 就被拦（或服务返回 422 fail-closed），hook 全量日志照常记录，便于排查。

### 3.7 安装源（install.sh）

AgentMoss 服务由 `plugins/hookers/agent_moss/install.sh` 安装：创建 venv（`/opt/agent_moss/venv`）并**从 PyPI 安装固定版本 `agent-moss==0.15.1`**（install.sh）。不存在 `AGENT_MOSS_HOME` / `pip install -e` 本地源码安装路径。

### 3.8 AcTrail（事件上报，可选）

| 变量 | 说明 | 默认值 |
|---|---|---|
| `ACTRAIL_ENABLED` | 设 `1` 启用 AcTrail 事件上报 | 未设（禁用）|
| `ACTRAIL_STORAGE_PATH` | AcTrail SQLite 路径 | `/tmp/actrail.sqlite` |
| `ACTRAIL_TIMEOUT_MS` | 写入超时（毫秒）| `100` |

---

## 四、使用示例

```bash
# 禁用层3 LLM（仅用层1+层2，最快）
export AGENT_MOSS_DISABLE_LLM=1

# LLM 失败时 fail-closed（安全优先，而非默认 fail-open）
export AGENT_MOSS_LLM_FAIL_MODE=fail_closed

# 调整 LLM 超时 + 重试
export AGENT_MOSS_LLM_TIMEOUT=60
export AGENT_MOSS_LLM_RETRIES=3

# 记录全量 hook 日志 + LLM prompt 到文件（排查 xiaoo→AgentMoss 调用问题首选）
export AGENT_MOSS_LOG_PATH=/tmp/agentmoss.log

# 启用策略生成（Step 2，默认禁用）
export AGENT_MOSS_ENABLE_POLICY_GEN=1

# 注入自定义规则
export AGENT_MOSS_CUSTOM_RULES='[{"pattern":"kubectl delete namespace","action":"Deny","severity":"critical"}]'

# 指向远端 AgentMoss 服务（不在本机起）
export AGENT_MOSS_URL=http://10.0.0.5:9095

# 本机多 agentmoss 并存时，声明自己该连哪个实例
export AGENT_MOSS_INSTANCE=xiaoo
```

---

> 补充：Policy Console（`http://127.0.0.1:9090/console`）可视化改层级开关/规则，比改 env/settings 更直观；env 仍作最高优先级覆盖手段保留。
