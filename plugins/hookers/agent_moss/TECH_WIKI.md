# AgentMoss（agent_moss）技术 Wiki

本文档描述 AgentMoss 安全分析服务的当前架构，以及 xiaoO 通过 `plugins/hookers/agent_moss/` 桥接插件接入它的工作原理。权威规则与源码在 AgentMoss 仓库（PyPI 包 `agent-moss`，本仓 install.sh 固定安装版本 `0.15.1`）。

---

## 目录

1. [总体架构](#1-总体架构)
2. [三层检测引擎](#2-三层检测引擎)
3. [xiaoO 桥接插件（bridge.py）](#3-xiaoO-桥接插件bridgepy)
4. [服务 API](#4-服务-api)
5. [失败策略](#5-失败策略)
6. [环境变量](#6-环境变量)
7. [安装与 systemd 部署](#7-安装与-systemd-部署)
8. [附录：参考文件路径](#8-附录参考文件路径)

---

## 1. 总体架构

AgentMoss 是一个**可被任意 AI Agent 调用的独立常驻 HTTP 安全分析服务**（FastAPI，systemd 管理），基于 OS Profile 机制自动适配 Linux/Windows 规则。xiaoO 不内嵌任何判定逻辑：工具调用前，由插件子进程 `bridge.py` 把动作转发给 AgentMoss 的 `POST /api/v1/analyze`，拿回 Allow/Deny 决策。

```
┌──────────────────────────────────────────────────────────────┐
│  xiaoO Agent 执行循环                                         │
│                                                              │
│  Agent LLM → 决定下一步动作 (a_next + reason)                  │
│       │                                                      │
│       ▼  *.Tool.*.pre hook                                   │
│  ┌─────────────────────────────────────────┐                 │
│  │  agent_moss 插件 (bridge.py)            │                 │
│  │  stdin JSON → 活性检查 → 字段映射        │                 │
│  └───────────────┬─────────────────────────┘                 │
│                  │ POST /api/v1/analyze (127.0.0.1:9090-9095)│
│                  ▼                                            │
│  ┌─────────────────────────────────────────┐                 │
│  │  AgentMoss 服务 (systemd: agent-moss)    │                 │
│  │  OS Profile (Linux/Windows)             │                 │
│  │  两级快速放行白名单                       │                 │
│  │  层1 特征匹配 → 层2 逻辑规则 → 层3 LLM+Skill │              │
│  └───────────────┬─────────────────────────┘                 │
│                  │ AnalyzeResponse                           │
│                  ▼                                            │
│  {"result": "allow"} / {"result": "deny", "reason": "..."}   │
└──────────────────────────────────────────────────────────────┘
```

AgentMoss 作为常驻 HTTP 服务（systemd unit / pip 装后自启），比每次 spawn 一个完整分析子进程快得多（无 venv 启动/import 开销，无线程池卡死风险）。多实例并存时（如本机 OpenDesk 的 `instance=opendesk` 与 xiaoO 的 `instance=xiaoo`），bridge 按健康检查返回的 `instance` 字段过滤，只连自己的实例。

---

## 2. 三层检测引擎

AgentMoss 对每个待执行动作 `a_next`（`action_type` + `action_detail`）依次过三层，命中即决策：

| 层 | 名称 | 机制 | 命中后 |
|----|------|------|--------|
| L1 | 启发式静态检测 | 正则匹配危险命令 + 注入关键词 + 用户敏感规则 + **敏感路径/危险模式/横向移动**（只看 a_next 本身） | critical/high → Deny（短路） |
| L2 | 逻辑规则检测 | read_before_write、意图一致性、间接文件访问、密码修改授权、用户/组删除授权、提权（需要 action_history / prompt_session / cwd） | critical/high → Deny（短路） |
| L3 | LLM + Skill 深度分析 | 13 个安全 Skill 规则（`skills/*_guard.md`，按关键词相关度取 top-3 注入 prompt）+ LLM 语义判断 | 按风险等级决策 |

- **L1 规则分 OS**：Linux/Windows 各一份（`agent_moss/profiles/linux.py` / `windows.py`），规则数量以 AgentMoss 仓库为准。
- **两级快速放行**：完全安全命令（ls/pwd/echo 等）跳过 L2+L3；只读敏感命令（cat/grep 等）跳过 L3；内置安全 Skill（如 xiaoo-guardian）直接放行。白名单放行前扫描完整命令管道尾部，防止 `echo ... | passwd` 类绕过。
- **L3 输入有界**：每次调用 = 固定模板 + 本次 a_next + 本次 session 的历史动作摘要 + ≤3 个 Skill，不随 session 长度累积上下文；结果缓存（200 条 FIFO）消减重复调用。
- **runtime_config**：三层开关、规则启停、skill 开关持久化在 `~/.config/agentmoss/agent_moss_runtime.json`，Policy Console（`http://127.0.0.1:<port>/console`）可视化热改。

### L3 安全 Skill（13 个）

| Skill | 检测内容 |
|-------|---------|
| script_execution_guard | 命令/脚本执行（rm -rf、sudo、curl\|bash、反弹 Shell） |
| file_access_guard | 敏感文件访问与路径滥用 |
| indirect_access_guard | 间接文件访问（目录遍历绕过） |
| data_exfiltration_guard | 数据外传 |
| lateral_movement_guard | 横向移动（nmap、ssh 隧道） |
| persistence_backdoor_guard | 持久化与后门（crontab、git hooks） |
| resource_exhaustion_guard | 资源耗尽（fork 炸弹、dd 填充） |
| supply_chain_guard | 供应链（typosquatting、恶意包） |
| skill_installation_guard | Skill/插件安装 |
| browser_web_access_guard | 浏览器/Web 访问 |
| email_operation_guard | 邮件操作 |
| intent_deviation_guard | 意图偏离 |
| general_tool_risk_guard | 通用工具风险 |

本仓 `plugins/hookers/agent_moss/skills/` 内是这 13 个文件的参考副本。

---

## 3. xiaoO 桥接插件（bridge.py）

`plugins/hookers/agent_moss/bridge.py` 是 xiaoO 的 `*.Tool.*.pre` hook 子进程，仅依赖 Python stdlib（urllib + socket），无需 venv。每次 hook 调用：

1. **活性检查**：解析服务 URL 后 `GET /api/v1/health`（默认超时 2s）。
   - URL 解析优先级：`AGENT_MOSS_URL` > 显式 `AGENT_MOSS_PORT` > **探测 9090-9095**（找返回 healthy 且 `instance` 匹配的端口，结果缓存）> 默认 9090。AgentMoss 端口被占时自动 findFreePort 往上找，所以 bridge 扫一段端口。
   - 探测按 `instance == AGENT_MOSS_INSTANCE`（默认 `xiaoo`）过滤，避免漂移到同机其他 agent 起的 agentmoss（bridge.py）。
2. **payload 解包与映射**：从 stdin JSON 提取 `call.tool_name` / `call.input` → `a_next`（bash→command、file_\*→file_path、skill→skill 名）；透传 `session_id`、`prompt_session`、`prompt_history`、`action_history`。
   - **工作区字段**：xiaoO 宿主 payload 的工作区字段是 `workspace`（`crates/hook/src/hookers/plugin/tool/adaptor.rs`），AgentMoss 接口字段是 `cwd`。bridge 以 `data.get("cwd") or data.get("workspace", "")` 取值（bridge.py），xiaoO 场景下 L2「间接文件访问检测」可正常拿到工作区。
   - bridge 不做任何鉴权/认证流程——hook 调用是本机 loopback HTTP，AgentMoss 的 `/api/v1/analyze` 无需 AUTH。
3. **转发分析**：`POST /api/v1/analyze`（bridge.py），请求带 `agent_id: "xiaoo"`（服务端据此加载 xiaoO 专属规则）和 `metadata.llm_config`（bridge 读 `~/.config/xiaoo/config.toml` 解析出的 per-request LLM 配置，拿不到则空 dict、服务端走全局配置）。
4. **结果映射**：`AnalyzeResponse.decision` → `{"result": "allow"}` / `{"result": "deny", "reason": "[触发层: ...] ..."}` 写到 stdout，退出码 0。

插件注册（`plugin.json`）：

```json
[
  {
    "id": "plugin_agent_moss_tool_input",
    "hook_point": "*.Tool.*.pre",
    "command": "python3 bridge.py",
    "raw_command": "python3 bridge.py"
  }
]
```

（安装时 `plugins/hookers/install.sh` 会把相对 command 解析为绝对路径。）

---

## 4. 服务 API

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/health` | 健康检查（含 `status`/`version`/`instance` 字段；bridge.py） |
| `POST` | `/api/v1/analyze` | 安全分析，返回 Allow/Deny 决策（bridge.py） |

**AnalyzeRequest 主要字段**：

```json
{
  "session_id": "会话ID (必填)",
  "prompt_session": "原始任务描述 (可选，L1 注入检测扫描)",
  "prompt_history": [{"text": "...", "actions": [...]}],
  "action_history": [{"action_type": "...", "action_detail": "..."}],
  "a_next": {"action_type": "bash", "action_detail": "cat /etc/passwd"},
  "reason": "执行理由 (可选)",
  "os_type": "",
  "cwd": "/home/user/project",
  "agent_id": "xiaoo",
  "metadata": {"llm_config": {...}, "llm_log_path": "..."}
}
```

**AnalyzeResponse 主要字段**：`decision`（Allow/Deny）、`reason`、`risk_level`（low/medium/high/critical）、`risk_type`、`violated_layers`（如 `["1.1"]`）、`violated_policy`（Deny 时）、`policy`（Allow 时附带的 Cerberus TOML 策略参考）、`confidence`、`analysis_duration_ms`。

完整 API 与 Console 路由见 AgentMoss 仓库 README。

---

## 5. 失败策略

失败策略分两层，方向相反：

| 层面 | 失败场景 | 行为 |
|------|----------|------|
| **bridge 活性检查** | AgentMoss 服务不可达/非 healthy/analyze 请求失败 | **fail-closed Deny**（bridge.py）：返回 `{"result":"deny","reason":"[AgentMoss 活性检查失败] ..."}` 并附启动引导，避免「服务挂了静默放行危险操作」 |
| **服务端 L3 LLM** | 纯靠 L3 判定时 LLM 调用失败 | 默认 **fail_open**（前两层未拦截则 Allow + 警告），可用 `AGENT_MOSS_LLM_FAIL_MODE=fail_closed` 切换为 Deny；L3 失败 + 前两层已拦截时无论 fail_mode 都 Deny |

超时层级：`AGENT_MOSS_HOOK_TIMEOUT`（默认 60s）限制 bridge 等 analyze 响应；`AGENT_MOSS_LLM_TIMEOUT`（默认 300s）限制服务端等远端 LLM；xiaoO hook 机制 `PLUGIN_HOOKER_TIMEOUT_MS`（600s/10 分钟，`crates/hook/src/hookers/plugin/tool/adaptor.rs`）兜底整个 hook 子进程，超时 kill。

---

## 6. 环境变量

完整清单见 [`docs/ENV_VARS.md`](docs/ENV_VARS.md)。要点（默认值以 ENV_VARS.md 为准）：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `AGENT_MOSS_URL` | 完整服务 URL（最高优先级，设了跳过探测） | 未设 |
| `AGENT_MOSS_HOST` / `AGENT_MOSS_PORT` | 服务地址/端口（显式设 PORT 则不探测） | `127.0.0.1` / `9090` |
| `AGENT_MOSS_INSTANCE` | 归属实例标识（install.sh 注入 `xiaoo`，探测时过滤） | `xiaoo` |
| `AGENT_MOSS_HOOK_TIMEOUT` | analyze 请求超时（秒） | `60` |
| `AGENT_MOSS_HEALTH_TIMEOUT` | 活性检查超时（秒） | `2` |
| `AGENT_MOSS_CHECK_SOURCE` | =1 时额外检查 agent_moss 包可 import | 未设 |
| `AGENT_MOSS_LOG_PATH` | **全量 hook 日志 + LLM prompt 日志**路径（bridge 记 HOOK_INPUT/OUTPUT，L3 记 prompt，写同一文件） | 未设（不记录） |
| `AGENT_MOSS_CONFIG_PATH` | 主配置 YAML 路径 | `~/.config/agentmoss/agent_moss.yaml`（缺失自动创建，并继承 xiaoo config.toml 的 LLM 配置） |
| `AGENT_MOSS_LLM_API_KEY` / `MODEL` / `BASE_URL` | LLM 配置（env 最高优先级） | 未设 |
| `AGENT_MOSS_LLM_FAIL_MODE` | LLM 失败策略 | `fail_open` |
| `AGENT_MOSS_DISABLE_LLM` / `DISABLE_HEURISTIC` / `DISABLE_LOGIC_RULES` | =1 单独禁用 L3/L1/L2 | 未设 |
| `AGENT_MOSS_LLM_TIMEOUT` | LLM 单次调用超时（秒） | `300` |

---

## 7. 安装与 systemd 部署

### 安装 AgentMoss 服务

```bash
bash plugins/hookers/agent_moss/install.sh [--enable-llm|--disable-llm]
```

install.sh 会：

1. 创建 venv（`/opt/agent_moss/venv`）并**从 PyPI 安装 `agent-moss==0.15.1`**（install.sh；已有同版本 venv 则跳过）
2. 写入 `/etc/agent_moss/agent_moss.env`（注入 `AGENT_MOSS_INSTANCE=xiaoo`）
3. 注册并启动 systemd service（`agent-moss`，`agent-moss install --enable`；无 sudo 时 nohup 后台启动）
4. 探测 9090-9095 验证 `instance==xiaoo` 的健康检查

### 在 xiaoO 侧注册插件

```bash
bash plugins/hookers/install.sh --non-interactive agent_moss
```

（交互模式下传插件名无效，`--non-interactive` 才接受插件名参数。）

### systemd 管理

| 命令 | 说明 |
|------|------|
| `sudo systemctl start/stop/restart agent-moss` | 启动/停止/重启 |
| `sudo systemctl status agent-moss` | 查看状态 |
| `journalctl -u agent-moss -f` | 查看日志 |
| `sudo systemctl disable agent-moss` | 取消开机启动 |

### 卸载

`bash plugins/hookers/uninstall.sh` 选择移除 agent_moss 时只从 `~/.config/xiaoo/config.toml` 移除插件条目（本插件目录没有子级 uninstall.sh）。AgentMoss 常驻服务需手动移除（install.sh 的卸载提示）：

```bash
sudo systemctl disable --now agent-moss
sudo rm -rf /opt/agent_moss
sudo rm /etc/systemd/system/agent-moss.service
sudo systemctl daemon-reload
```

---

## 8. 附录：参考文件路径

AgentMoss 服务的源码与规则在 AgentMoss 仓库（不在本仓）：

| 文件 | 路径（AgentMoss 仓库内） |
|------|------|
| CLI 主入口 | `agent_moss/cli.py` |
| 分析入口 | `agent_moss/engine/analyzer.py` |
| 三层防御协调器 | `agent_moss/engine/coordinator.py` |
| 层1 特征匹配 | `agent_moss/engine/heuristic.py` + `agent_moss/profiles/linux.py`（规则） |
| 层2 逻辑规则 | `agent_moss/engine/logic_rules.py` |
| 层3 LLM 分析 | `agent_moss/engine/llm_analyzer.py` |
| 类型定义 | `agent_moss/engine/types.py` |
| 服务 README | AgentMoss 仓库 `README.md` |

xiaoO 侧文件：`plugins/hookers/agent_moss/bridge.py`（桥接脚本，本插件目录）。
