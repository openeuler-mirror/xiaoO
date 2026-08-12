# AgentMoss

AgentMoss 是一个**可被任意 AI Agent 调用的独立通用安全分析服务**。基于 OS Profile 机制，自动适配 Linux/Windows 的 syscall 检测模式，提供三层防御安全分析（特征匹配 → 逻辑规则 → LLM 语义分析）。

> v2 更新：新增 OS Profile 系统（Linux/Windows 自适应）、两级快速放行白名单、rm 命令分级检测、脚本内容预扫描、LLM 结果缓存、Provider 自动识别与 Header 注入、fail-closed 安全原则。
>
> v0.4.0 新增：非交互式密码修改检测（passwd/chpasswd/newusers/lnewusers 等）、密码修改授权检测（ask_user_question 前置检查）、内置安全 Skill 白名单（xiaoo-guardian 直接放行）、系统信息命令快速放行（whoami/id/hostname 等 12 个命令）、shell 重定向写入检测、AcTrail 执法事件上报。
>
> v0.5.0 新增：L1/L2 规则收窄减少误杀（sudo 只拦截危险命令、rm -rf /tmp 不拦截、dd/mkfs/format 收窄、/etc/passwd 仅从 L1 移除、git push --force 降级为 medium）、nmap/masscan/zmap 网络扫描检测、lnewusers 批量用户添加检测、L2 通配符删除排除临时目录、用户/组删除授权检测、chgpasswd/lpasswd/gpasswd 检测。

## 目录

- [架构](#架构)
- [快速开始](#快速开始)
- [LLM 配置](#llm-配置)
- [AcTrail 集成](#actrail-集成)
- [openEuler 部署](#openeuler-部署)
- [API 参考](#api-参考)
- [Policy Console（策略管控台）](#policy-console策略管控台)
- [runtime_config（运行时持久化配置）](#runtime_config运行时持久化配置)
- [配置](#配置)
- [项目结构](#项目结构)
- [测试](#测试)
- [环境变量参考](docs/ENV_VARS.md)

## 架构

```
┌─────────────────────────────────────────────────────────────┐
│          调用入口 (HTTP / Unix Socket / Hook)                 │
│  POST /api/v1/analyze                                        │
│  GET  /api/v1/health                                         │
├─────────────────────────────────────────────────────────────┤
│               ObservableAdapter                               │
│  AgentOS 可观测服务 syscall → 标准格式                       │
├─────────────────────────────────────────────────────────────┤
│                OS Profile 选择                                │
│     ┌──────────┐    ┌──────────┐                             │
│     │  Linux   │    │ Windows  │                             │
│     │ Profile  │    │ Profile  │                             │
│     └──────────┘    └──────────┘                             │
├─────────────────────────────────────────────────────────────┤
│          两级快速放行白名单                                    │
│  完全安全 (ls/pwd/echo/whoami/hostname...) → 跳过 L2+L3     │
│  只读敏感 (cat/grep/head...) → 跳过 L3，保留 L2              │
│  内置安全 Skill (xiaoo-guardian) → 跳过 L2+L3                │
│  ★ agent 定制白名单 (按 agent_id 加载，如 opendesk 的       │
│    memory-*/todomgr-* 纯数据操作) → 跳过 L2+L3               │
│  ★ 安全兜底: 完整命令管道尾部扫描，防止白名单绕过             │
├─────────────────────────────────────────────────────────────┤
│          安全分析引擎 (三层防御)                               │
│                                                              │
│  层1: 特征匹配静态检测 (<1ms)                                   │
│    ├── 用户自定义规则匹配                                     │
│    ├── 危险命令正则 (38+ 模式，rm 分级检测，sudo 只拦截危险命令) │
│    ├── 非交互式密码修改检测 (| passwd/chpasswd/newusers/lnewusers 等) │
│    ├── → high/critical → 直接 Deny                            │
│    ├── → 内联脚本 file_access 转层3                            │
│    │     （避免 'python -c "cat /etc/shadow"' 假阳性）         │
│    └── Prompt 注入检测 (62 个中英关键词，9 类)                 │
│                                                              │
│  层2: 逻辑规则检测 (<1ms)                                     │
│    ├── Rule 1: read-before-write 原则 (含 shell 重定向检测)    │
│    ├── Rule 2: 意图偏离检测                                   │
│    ├── Rule 3: 敏感路径访问 (含凭据文件 5 条目,               │
│    │          \b 边界匹配避免部分匹配误报)                     │
│    ├── Rule 4: 危险操作模式 (通配符删除/重定向覆盖, 临时目录排除) │
│    ├── Rule 5: 密码修改授权检测 (非交互式需 ask_user_question) │
│    ├── Rule 6: 用户/组删除授权检测 (需 ask_user_question)     │
│    ├── Rule 7: 提权检测 (sudo+危险命令/su/chmod u+s)          │
│    └── Rule 8: 横向移动检测 (ssh/scp/rsync)                   │
│                                                              │
│  层3: LLM + Skill 深度分析 (2-5s)                            │
│    ├── 脚本内容预扫描 (18 个可疑模式，4 组合风险)              │
│    ├── Skill 规则匹配 (12 个 Skill，关键词加权评分)            │
│    ├── LLM 结果缓存 (200 条 FIFO)                            │
│    ├── Provider 自动识别 (15+ Provider)                       │
│    └── LLM 语义安全判断 (fail-closed)                         │
├─────────────────────────────────────────────────────────────┤
│          AcTrail 事件上报 (fire-and-forget)                    │
│  Allow/Deny 决策 → AcTrail SQLite Enforcement 事件            │
└─────────────────────────────────────────────────────────────┘
```

### 设计原则

- **适配在 OS 层，不在 Agent 层** — 不同 Agent 最终都会调用操作系统的 syscall，这是确定性的
- **标准化 I/O 契约** — 任何 Agent 遵循 API 格式即可接入，无需关心内部实现
- **fail-closed 安全原则** — LLM 调用失败时默认拒绝，不确定时拒绝
- **两级快速放行** — 完全安全命令跳过 L2+L3，只读敏感命令跳过 L3，降低性能开销
- **安全兜底** — 白名单放行前扫描完整命令管道尾部，防止 `echo ... | passwd` 等绕过
- **内置 Skill 信任** — 系统安全 Skill（如 xiaoo-guardian）加载时直接放行，不走 LLM 审计
- **rm 命令分级检测** — 根据 flag 危险度 + 目标路径敏感度分级，`/tmp` 等临时目录不再拦截
- **sudo 收窄** — 不再一刀切拦截所有 sudo，只拦截 sudo 后跟的危险命令（rm/chmod/dd/mkfs 等）
- **内联脚本假阳性防护** — `python -c "x = 'cat /etc/shadow'; print(x)"` 等字符串字面量中的敏感路径不立即拦截，转层3 LLM 语义判断
- **目录遍历动态文件访问检测** — 检测 `os.listdir()` + `open()` 等通过目录遍历动态发现文件并读写的行为，防止绕过静态路径匹配
- **敏感路径读写区分** — 凭据文件（credentials.yml、.env、SSH 密钥等）读写均拦截；系统配置文件（/etc/hosts、/boot/ 等）仅写拦截；特殊设备（/dev/random）仅读拦截
- **Shell 重定向智能识别** — 识别 `2>/dev/null` 等丢弃写法不计入写操作，`lsblk`/`blockdev` 等只读系统工具后的重定向也不判定为写入
- **凭据文件边界匹配** — `credentials.yml`、`.env` 等使用 `\b` 边界匹配避免非文件名拼接（如 `something_credentials_yml`）误报
- **密码修改授权** — 非交互式密码修改（`| passwd`、`chpasswd` 等）必须有 `ask_user_question` 用户授权
- **Shell 重定向写入检测** — `echo data > file` 等重定向操作识别为写入，纳入 read-before-write 规则

## 快速开始

### 安装

**Python (PyPI)**：
```bash
pip install agent-moss
```

**TypeScript (npm)**：
```bash
npm install @kenhkl/agent-moss
```

如需从源码安装：

```bash
git clone git@gitcode.com:kenhkl/AgentMoss.git
cd AgentMoss

# Python 版
pip install -e .

# TypeScript 版
cd ts && npm install && npm run build
```

### CLI 使用

```bash
# 生成输入模板 (v2: 含 os_type 和 cwd 字段)
agent-moss init -o input.json

# 运行安全分析
agent-moss analyze input.json

# 安装 systemd service（有 sudo 时）
# 自动创建 /etc/systemd/system/agent-moss.service，--enable 开机自启
agent-moss install --enable

# 无 sudo 时跳过 systemd，提示手动启动：
#   agent-moss server --port 0
agent-moss install

# 查看服务状态（端口、版本、Console URL、三层开关）
agent-moss status

# 启动 HTTP 服务 (TCP)
# 端口被占时自动 findFreePort 往上找空闲端口（9090→9091→…，扫 100 个）
agent-moss server --port 0
# 消费方（xiaoO bridge.py）探测 9090-9095 的 /api/v1/health 自动连上实际端口。

# 启动 Unix Domain Socket 服务（同机部署推荐）
agent-moss server --mode socket --socket /var/run/agent_moss/agent_moss.sock

# 指定配置文件
agent-moss server --mode socket --config /etc/agent_moss/agent_moss.yaml
```

### API 调用

**HTTP TCP 方式：**

```bash
curl -X POST http://127.0.0.1:9090/api/v1/analyze \
  -H 'Content-Type: application/json' \
  -d '{
    "session_id": "test-001",
    "prompt_session": "列出系统文件",
    "action_history": [],
    "a_next": {
      "action_type": "bash",
      "action_detail": "ls -la /tmp"
    },
    "reason": "查看临时目录",
    "os_type": "",
    "cwd": "/home/user"
  }'
```

**Unix Domain Socket 方式（同机低延迟）：**

```bash
curl --unix-socket /var/run/agent_moss/agent_moss.sock \
  -X POST http://localhost/api/v1/analyze \
  -H 'Content-Type: application/json' \
  -d '{
    "session_id": "test-001",
    "a_next": {
      "action_type": "bash",
      "action_detail": "ls -la /tmp"
    }
  }'
```

**响应示例 (Allow)**：

```json
{
  "decision": "Allow",
  "risk_level": "low",
  "risk_type": "",
  "violated_layers": [],
  "confidence": 100,
  "analysis_duration_ms": 10.8
}
```

### TypeScript / Node.js 调用

```typescript
import { createApp } from '@kenhkl/agent-moss';

const app = createApp();
// 通过 Hono app.fetch 在 Node.js / Bun / Electron 中调用
// 或启动独立服务: npx agent-moss server --port 9090
```

也可以直接 HTTP 调用：

```typescript
const resp = await fetch('http://127.0.0.1:9090/api/v1/analyze', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
        session_id: 'sess-001',
        a_next: { action_type: 'bash', action_detail: 'ls -la' },
        os_type: 'linux',
        cwd: '/home/user',
    }),
});
const result = await resp.json();
// → { decision: 'Allow'|'Deny', risk_level: 'low'|'...', ... }
```

**响应示例 (Deny)**：

```json
{
  "decision": "Deny",
  "reason": "检测到递归强制删除关键路径 (rm -rf /...)",
  "risk_level": "critical",
  "risk_type": "script_execution",
  "violated_layers": ["1.1"],
  "confidence": 95,
  "analysis_duration_ms": 1.2
}
```

## LLM 配置

AgentMoss 的三层防御中，层3（LLM 语义分析）是可选的。层1+层2 的静态规则可以独立运行。

### L3 输入构成与 token 消耗（为什么不会上下文爆炸）

客户常有的顾虑：最坏情况下一个 session 里的每个 tool call 都会走到 L3 LLM 分析，会不会像 Agent 主循环那样把历史会话不断叠加导致上下文爆炸？**不会**。AgentMoss 的 L3 每次调用都是**有界的独立请求**，输入只包含当前这次判定的必要上下文，不随 session 长度无限累积。

**每次 L3 调用的输入构成**（固定模板 + 当前判定要素）：

| 组成部分 | 内容 | 是否有界 |
|---------|------|---------|
| 固定 prompt 模板 | 安全审计员角色 + 判定维度 + JSON 输出格式 | ✓ 固定 |
| 原始用户 prompt | `prompt_session`（本次 session 的用户意图） | 有界（用户单次输入） |
| 待执行动作 | `action_type` + `action_detail`（当前这一次 tool call） | ✓ 单次动作 |
| 执行理由 | `reason` | ✓ 当前动作的理由 |
| 历史动作序列 | `action_history`（本次 session 已执行的动作，每条仅 name + action_detail） | 有界（本次 session 的有限动作） |
| 安全检测规则 | **最多 3 个 Skill**（按关键词相关度从全集选 top-3，每个 Skill 29-39 行） | ✓ 最多 3 个 |
| 前置检测结果 | L1/L2 的命中提示（仅命中时才有） | ✓ 按需 |
| 脚本内容分析 | 内联脚本/管道的静态分析（仅脚本类动作才有） | ✓ 按需 |

**关键边界**：

- **Skill 最多 3 个**：L3 从全部安全 Skill 中按关键词相关度评分取 top-3 注入 prompt（无匹配时兜底 `general_tool_risk_guard` 1 个），不会把全部 Skill 都塞进去。每个 Skill 文件 29-39 行。
- **每次调用独立**：L3 是"判定单个待执行动作 a_next"的独立请求，**不是**把 agent 的全部历史对话叠加进上下文。每次输入 = 固定模板 + 本次 a_next + 本次 session 的历史动作 + ≤3 个 Skill，彼此之间无累积。
- **实测输入大小**：典型 L3 调用 prompt 长度约 **3500-3700 字符（~1k-1.5k token）**，多次调用稳定在该量级，不随 session 延长而膨胀。
- **与 Agent 主循环的区别**：Agent 主循环调 LLM 时通常把全部历史对话（user/assistant 往返）叠加进上下文，session 越长 token 越大；AgentMoss L3 每次只判一个动作，历史只传"已执行动作的摘要"（name + action_detail），且本次判定完即丢弃，不在 L3 侧累积。

> 最坏情况（session 内每个 tool call 都走 L3）下，**单次 L3 的输入 token 仍有界**（~1.5k token 量级），代价是调用次数 × 单次 token，而非单次 token 随历史爆炸。结果缓存（200 条 FIFO）会进一步消减重复调用的实际 LLM 消耗。

### 启用 LLM

**方式 1：环境变量（推荐）**

```bash
export AGENT_MOSS_LLM_API_KEY="sk-your-key"
# 可选：自定义模型和 API 端点
export AGENT_MOSS_LLM_MODEL="glm-5.0"
export AGENT_MOSS_LLM_BASE_URL="https://api.nextapi.store/v1"
```

**方式 2：配置文件**

首次运行 AgentMoss 时，会自动在 `~/.config/agentmoss/config.json` 创建默认配置文件。编辑该文件：

```json
{
  "llm": {
    "api_key": "sk-your-key",
    "model": "glm-5.0",
    "base_url": "https://api.nextapi.store/v1"
  }
}
```

**配置文件优先级**：
1. 环境变量（`AGENT_MOSS_LLM_API_KEY` 等）— 最高优先级
2. `AGENT_MOSS_CONFIG_PATH` 环境变量指定的路径
3. `~/.config/agentmoss/config.json` — 首次运行自动创建
4. 包内默认配置

**方式 3：YAML 配置文件**

通过 `--config` 或 `AGENT_MOSS_CONFIG_PATH` 指定 YAML 文件：

```bash
agent-moss server --config /etc/agent_moss/agent_moss.yaml
```

编辑 `config/agent_moss.yaml`：

```yaml
llm:
  provider: "zhipu"
  model: "glm-5.0"
  base_url: "https://api.nextapi.store/v1"
  api_key_env: "AGENT_MOSS_LLM_API_KEY"
  temperature: 0.1
  max_tokens: 4096

security:
  llm_analysis:
    enabled: true
```

### 禁用 LLM（仅静态规则）

```bash
export AGENT_MOSS_DISABLE_LLM=1
```

### 支持的 LLM Provider

| Provider | URL 匹配 | Provider Hint | 说明 |
|----------|---------|---------------|------|
| OpenAI | `api.openai.com` | `openai` | 默认模型 |
| Anthropic | `anthropic.com` | `claude` / `anthropic` | Claude 系列 |
| Google | `generativelanguage.googleapis.com` | `google` / `gemini` | Gemini 系列 |
| DeepSeek | `api.deepseek.com` | `deepseek` | DeepSeek 系列 |
| 智谱 (GLM) | `open.bigmodel.cn` | `zhipu` / `glm-cn` / `bigmodel` | GLM / CogView |
| 智谱 Coding Plan | `api.z.ai` | `zai-coding-plan` | 智谱编程套餐 |
| OpenRouter | `openrouter.ai` | `openrouter` | 多模型中转 |
| Groq | `api.groq.com` | `groq` | 高速推理 |
| Mistral | `api.mistral.ai` | `mistral` | Mistral 系列 |
| Together | `api.together.xyz` | `together` | 开源模型聚合 |
| xAI | `api.x.ai` | `xai` / `xai-grok` | Grok 系列 |
| MiniMax | `api.minimaxi.com` | `minimax` | MiniMax 系列 |
| GitCode | `api-ai.gitcode.com` | `gitcode` | GitCode AI |
| Ollama | `:11434` | `ollama` | 本地模型 |
| 任意兼容 | — | `other` / `custom` / `local` | 任何 OpenAI 兼容 API |

自动识别 Provider 后，OpenRouter 和 xAI 会注入特定 HTTP Header（Referer、Title），确保平台正确计费。

## AcTrail 集成

AgentMoss 可以将每次安全分析的 allow/deny 决策作为 Enforcement 事件写入 [AcTrail](https://gitcode.com/openeuler/AcTrail) 的存储，实现统一的 agent 行为审计。

### 启用

```bash
# 环境变量方式
export ACTRAIL_ENABLED=1
export ACTRAIL_STORAGE_PATH=/tmp/actrail.sqlite  # 默认路径

# 启动 AgentMoss
agent-moss server --port 9090
```

### Policy Console（策略管控台）

AgentMoss 自带浏览器策略管控台，可视化调整三层开关、规则启停、deny_mode、skill 开关，查看 token 用量统计。Python 版和 TS 版功能对等（同一套前端 SPA，同一套 HTTP API 契约）。

**访问**：服务启动后，浏览器打开 `http://127.0.0.1:9090/console`（端口随服务实际监听端口）。

**鉴权**：默认本机（127.0.0.1）免鉴权，便于 iframe 嵌入；如需远程访问，设 `AGENT_MOSS_CONSOLE_TOKEN` 环境变量，请求带 `Authorization: Bearer <token>`。

**Console API**（前端 SPA 调用，也可直接 curl）：

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/console` | 返回 SPA index.html |
| `GET` / `PUT` | `/console/api/layers` | 查/改三层开关（L1/L2/L3 启停）|
| `GET` | `/console/api/rules` | 查全量规则（L1/L2，含 builtin + 自定义）|
| `PUT` | `/console/api/rules/enabled` | 翻转规则启停 |
| `PUT` | `/console/api/rules/deny_mode` | 改 deny_mode（deny_write/deny_read/deny_both）|
| `PUT` | `/console/api/rules/skip_l3` | 改规则禁用时是否跳过 L3 |
| `PUT` | `/console/api/categories/enabled` | 改分类开关 |
| `POST` / `DELETE` | `/console/api/rules` | 增/删自定义规则（builtin 不可删）|
| `GET` | `/console/api/skills` | 查 L3 skill 全集 |
| `PUT` | `/console/api/skills/enabled` | 翻转 skill 启停 |
| `PUT` | `/console/api/skill-categories/enabled` | 改 skill 分类开关 |
| `POST` / `DELETE` | `/console/api/skills` | 增/删自定义 skill（写 markdown 文件）|
| `GET` | `/console/api/skills/{id}/content` | 查 skill markdown 内容 |
| `GET` | `/console/api/agents` | 查 agent 定制规则（按 agent_id 分区，含 enabled 状态）|
| `PUT` | `/console/api/agents/rules/enabled` | 翻转某 agent 某条规则启停 |
| `POST` / `DELETE` | `/console/api/agents/rules` | 增/删 agent 定制规则条目 |
| `GET` | `/console/api/config` | 查完整 runtime config + 路径 |
| `GET` | `/console/api/env-overrides` | 查被环境变量接管的开关（灰色不可改）|
| `POST` | `/console/api/reset` | 重置为出厂默认 |
| `GET` | `/console/api/token-stats` | token 用量统计 |
| `GET` | `/console/api/token-stats/recent` | 最近 N 条调用记录 |
| `GET` | `/console/api/token-stats/trend` | 趋势（日期×模型）|
| `POST` | `/console/api/token-stats/reset` | 清空统计 |

> Console UI 采用 OpenDesk `--ag-` 设计 token + 暗色模式，为后续原生嵌入 OpenDesk 安全网关新页面预留视觉对齐。

## runtime_config（运行时持久化配置）

runtime config 是 AgentMoss 的**运行时策略持久化层**，区别于启动时读的 YAML 配置：

- **存储**：`~/.config/agentmoss/agent_moss_runtime.json`（`AGENT_MOSS_RUNTIME_CONFIG_PATH` 覆盖）
- **内容**：三层开关、L1/L2 规则启停状态、deny_mode、skip_l3、L3 skill 开关
- **seed**：首次加载时若文件不存在，自动从源码常量 seed 出厂默认（含全部 builtin 规则）并落盘
- **merge**：每次加载时与源码默认 merge——保留用户开关，补入新 builtin，源码已删的 builtin 标记 `source_removed`+禁用（指纹去重，跨 id 变更识别同一规则）
- **优先级**：`AGENT_MOSS_DISABLE_*` env > runtime JSON > `agent_moss_settings.json` > 默认。env 永远最高（Console 会展示被 env 接管的开关为灰色不可改）
- **热生效**：`is_layer_enabled` 每次判定现读 runtime JSON，Console 改完立刻生效，无需重启服务
- **query 写操作**：Python 和 TS 均实现全套（seed/merge/CRUD/update_*/`_or_default` 回退），双栈行为一致

Console UI 即是对 runtime config 的可视化操作层；直接编辑 JSON 或用 env 覆盖是另外两个入口，三者最终都落到同一份 runtime JSON。

## 配置

| 环境变量 | 说明 | 默认值 |
|---------|------|--------|
| `ACTRAIL_ENABLED` | 设为 `1` 启用上报 | 未设置（禁用） |
| `ACTRAIL_STORAGE_PATH` | AcTrail SQLite 数据库路径 | `/tmp/actrail.sqlite` |
| `ACTRAIL_TIMEOUT_MS` | 写入超时（毫秒） | `100` |

### 事件格式

AgentMoss 写入 AcTrail 的 `events` 表，payload 格式为 `Enforcement` 类型：

```json
{
  "backend": "agent-moss",
  "operation": "tool_call",
  "decision": "allow|deny",
  "result": "allowed|denied",
  "tool.name": "bash",
  "tool.command": "cat /var/log/syslog",
  "agent.session_id": "session-001",
  "agent.prompt": "read log file",
  "risk.level": "low",
  "risk.type": "",
  "reason": "...",
  "violated_layers": "1,2",
  "confidence": "100",
  "duration_ms": "2.3"
}
```

### 架构

```
AgentMoss (Python HTTP)          AcTrail (eBPF daemon)
┌─────────────────┐             ┌─────────────────┐
│ POST /analyze   │             │                 │
│   ↓             │             │  SQLite 存储     │
│ 三层防御分析     │             │  (events 表)    │
│   ↓             │  fire-and-  │                 │
│ Allow/Deny      │──forget────▶│ Enforcement     │
│   ↓             │  写入 SQLite │ 事件落库        │
│ 返回给调用方     │             │                 │
└─────────────────┘             └─────────────────┘
                                        ↓
                               actrailviewer / actrailweb
                               统一查看 agent 行为审计
```

## openEuler 部署

### 前提

- openEuler 22.03 LTS+
- Python 3.10+
- root 权限

### 一键安装

```bash
sudo dnf install -y python3 python3-pip python3-devel
git clone <repo-url> agent_moss
cd agent_moss
sudo bash scripts/install.sh
```

### 配置 LLM（生产环境）

```bash
# 写入 API Key（文件权限 600）
echo "AGENT_MOSS_LLM_API_KEY=sk-your-key" | sudo tee /etc/agent_moss/agent_moss.env
sudo chmod 600 /etc/agent_moss/agent_moss.env

# 编辑 LLM model / base_url
sudo vim /etc/agent_moss/agent_moss.yaml
```

### 启动方式

```bash
# HTTP TCP 模式（默认，适合跨机/调试）
sudo systemctl enable --now agent-moss

# Unix Socket 模式（同机部署推荐，更低延迟）
# 编辑 /etc/agent_moss/agent_moss.yaml 中 server.mode: "socket"
# 或修改 /etc/systemd/system/agent-moss.service 中的 ExecStart
```

### 启动与验证

```bash
sudo systemctl enable --now agent-moss

# HTTP 模式验证
curl http://127.0.0.1:9090/api/v1/health
# {"status":"healthy","version":"<当前版本，见 pyproject.toml / ts/package.json>"}

# Socket 模式验证
curl --unix-socket /var/run/agent_moss/agent_moss.sock \
  http://localhost/api/v1/health
```

### 管理命令

| 命令 | 说明 |
|------|------|
| `systemctl start agent-moss` | 启动 |
| `systemctl stop agent-moss` | 停止 |
| `systemctl restart agent-moss` | 重启 |
| `systemctl status agent-moss` | 查看状态 |
| `journalctl -u agent_moss -f` | 查看日志 |
| `systemctl disable agent-moss` | 取消开机启动 |

### 防火墙（HTTP 模式需要）

```bash
sudo firewall-cmd --add-port=9090/tcp --permanent
sudo firewall-cmd --reload
```

### RPM 安装（离线交付，推荐）

AgentMoss 提供 RPM 离线包（源码包建议在 AgentSecurity 仓构建 RPM）。RPM 安装与上面的源码安装等价，但依赖通过 RPM `Requires` 自动解析，无需联网。

**安装后产物**：

| 路径 | 内容 |
|------|------|
| `/usr/lib/.xiaoo/hookers/agent_moss/agent_moss/` | Python 源码 |
| `/usr/lib/.xiaoo/hookers/agent_moss/scripts/` | systemd service 模板 |
| `/usr/lib/.xiaoo/hookers/agent_moss/hooks/` | bridge 脚本（xiaoO 用） |
| `/usr/bin/agent-moss` | CLI 入口（install/status/server） |
| `/usr/lib/systemd/system/agent-moss.service` | systemd unit（daemon 子包） |
| `/var/lib/agent_moss/` | 运行时数据（token_stats、user_rules） |
| `/var/log/agent_moss/` | 日志目录 |

> ⚠️ **注意**：`/usr/lib/.xiaoo/hookers/` 是 **AgentMoss 与 xiaoO 共用**的目录。AgentMoss 源码装在其子目录 `agent_moss/` 下。**不要手动 `rm -rf /usr/lib/.xiaoo/` 清理残留**——这样会连带删掉 AgentMoss 源码，导致 `/usr/bin/agent-moss` 报 `ModuleNotFoundError: No module named 'agent_moss'`。清理旧插件（如 audit_agent）应只删对应子目录，或通过 `dnf remove` 卸载。

**完整安装顺序（AgentMoss + xiaoO）**：AgentMoss 是 xiaoO 的安全审计运行时依赖，需先装 AgentMoss 再装 xiaoO。

```bash
# 1. 装 AgentMoss 主包 + daemon 子包（提供安全分析服务）
sudo dnf install ./AgentMoss-xxx.noarch.rpm ./AgentMoss-daemon-xxx.noarch.rpm

# 2. 启动服务（http://127.0.0.1:9090）
sudo systemctl enable --now agent-moss

# 3. 装 xiaoO 主包 + hookers 子包
sudo dnf install ./xiaoO-xxx.noarch.rpm ./xiaoO-hookers-xxx.noarch.rpm

# 4. 注册 agent_moss bridge 到 xiaoO config.toml
xiaoo-hookers-install --non-interactive agent_moss
```

> 未运行 AgentMoss 时，agent_moss bridge fail-close（拒绝工具执行）。

> 💡 若 `/usr/bin/agent-moss` 报 `No module named 'agent_moss'`（源码被手工删除或未装好），重新安装 AgentMoss 主包即可恢复源码：
> ```bash
> sudo dnf reinstall ./AgentMoss-xxx.noarch.rpm
> ```

## API 参考

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/api/v1/health` | 健康检查 |
| `POST` | `/api/v1/analyze` | 安全分析 |

### POST /api/v1/analyze

**请求体 (v2)**：

```json
{
  "session_id": "会话ID (必填)",
  "prompt_session": "原始任务描述 (可选，用于注入检测)",
  "action_history": [{"action_type": "...", "action_detail": "..."}],
  "a_next": {
    "action_type": "bash",
    "action_detail": "cat /etc/passwd"
  },
  "reason": "执行理由 (可选)",
  "os_type": "",
  "cwd": "/home/user/project",
  "metadata": {"agent_id": "...", "sandbox_id": "..."}
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `session_id` | string | 是 | 会话唯一标识 |
| `prompt_session` | string | 否 | 原始任务描述（层1 注入检测会扫描此字段） |
| `action_history` | array | 否 | 历史动作序列 |
| `a_next.action_type` | string | 是 | 动作类型 (bash/file_read/file_write 等) |
| `a_next.action_detail` | string | 是 | 命令/动作详情 |
| `reason` | string | 否 | 执行理由 |
| `os_type` | string | 否 | `"linux"` / `"windows"`，留空自动检测 |
| `cwd` | string | 否 | 当前工作目录 |
| `agent_id` | string | 否 | 调用方 agent 标识（如 `xiaoo` / `opendesk`）。留空走通用模式。详见[三方 agent 定制规则](#三方-agent-定制规则) |
| `metadata` | object | 否 | 扩展元数据（`llm_config`/`llm_log_path` per-request LLM 配置） |

**响应字段**：

| 字段 | 类型 | 说明 |
|------|------|------|
| `decision` | string | `Allow` / `Deny` |
| `reason` | string | 决策原因 |
| `risk_level` | string | `low` / `medium` / `high` / `critical` |
| `risk_type` | string | 风险类别 (file_access, script_execution, data_exfiltration, prompt_injection, privilege_escalation, lateral_movement, ...) |
| `violated_layers` | array | 触发的检测层，如 `["1", "2"]` |
| `violated_policy` | string | 违反的具体条款 (Deny 时) |
| `policy` | string | Cerberus TOML 策略 (Allow 时) |
| `confidence` | int | 置信度 0-100 |
| `analysis_duration_ms` | float | 分析耗时（毫秒） |

## 配置

优先级：环境变量 > YAML 配置文件 > 内置默认值

### 环境变量

> 完整清单（含 audit_agent 迁移对照、bridge.py 端口变量、配置优先级）见 [`docs/ENV_VARS.md`](docs/ENV_VARS.md)。

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `AGENT_MOSS_LLM_API_KEY` | LLM API Key（兼容 fallback `OPENROUTER_API_KEY` / `OPENAI_API_KEY`）| 未设置 |
| `AGENT_MOSS_LLM_MODEL` | LLM 模型名称 | `gpt-4o` (TS) / `anthropic/claude-3.5-sonnet` (Python) |
| `AGENT_MOSS_LLM_BASE_URL` | LLM API 端点 | `https://api.openai.com/v1` (TS) / `https://openrouter.ai/api/v1` (Python) |
| `AGENT_MOSS_LLM_PROVIDER` | Provider 标识（决定注入 HTTP headers）| 从 base_url 推断 |
| `AGENT_MOSS_LLM_TEMPERATURE` | LLM 采样温度 | `0.1` |
| `AGENT_MOSS_LLM_MAX_TOKENS` | LLM 最大输出 token 数 | `4096` |
| `AGENT_MOSS_LLM_TIMEOUT` | LLM 单次调用超时（秒）| `300` |
| `AGENT_MOSS_LLM_RETRIES` | LLM 调用重试次数 | `2`（共 3 次尝试） |
| `AGENT_MOSS_LLM_FAIL_MODE` | LLM 失败策略：`fail_open`（默认 Allow+warn）/ `fail_closed`（Deny）| `fail_open` |
| `AGENT_MOSS_DISABLE_LLM` | 设为 `1` 禁用层3 LLM | 未设置 |
| `AGENT_MOSS_DISABLE_HEURISTIC` | 设为 `1` 禁用层1 特征匹配 | 未设置 |
| `AGENT_MOSS_DISABLE_LOGIC_RULES` | 设为 `1` 禁用层2 逻辑规则 | 未设置 |
| `AGENT_MOSS_LOG_PATH` | 全量 hook 日志 + LLM prompt 日志（bridge 记 HOOK_INPUT/OUTPUT，L3 记 prompt，写同文件；对应 audit_agent `AUDIT_LOG_PATH`）| 未设置（不记录）|
| `AGENT_MOSS_CUSTOM_RULES` | 自定义规则 JSON 数组 | `[]` |
| `AGENT_MOSS_ENABLE_POLICY_GEN` | 设为 `1` 启用 Policy 生成 | 未设置 |
| `AGENT_MOSS_CONFIG_PATH` | 主配置 YAML 路径 | `~/.config/agentmoss/agent_moss.yaml` |
| `AGENT_MOSS_RUNTIME_CONFIG_PATH` | runtime JSON（层级开关+规则启停持久化）| `~/.config/agentmoss/agent_moss_runtime.json` |
| `AGENT_MOSS_TOKEN_STATS_PATH` | token 用量统计 JSON | `~/.config/agentmoss/agent_moss_token_stats.json` |
| `AGENT_MOSS_SETTINGS_PATH` | `agent_moss_settings.json` 路径 | 包内默认 |
| `AGENT_MOSS_CONSOLE_TOKEN` | Console Bearer 鉴权 token | 未设（本机免鉴权）|
| `AGENT_MOSS_URL` / `HOST` / `PORT` | bridge.py 连服务用（端口被占自动 findFreePort）| `127.0.0.1:9090` |
| `ACTRAIL_ENABLED` | 设为 `1` 启用 AcTrail 事件上报 | 未设置（禁用） |
| `ACTRAIL_STORAGE_PATH` | AcTrail SQLite 数据库路径 | `/tmp/actrail.sqlite` |
| `ACTRAIL_TIMEOUT_MS` | AcTrail 写入超时（毫秒） | `100` |

### 配置文件

参考 `config/agent_moss.yaml`。

### 自定义规则

通过 `AGENT_MOSS_CUSTOM_RULES` 环境变量注入自定义正则规则：

```bash
export AGENT_MOSS_CUSTOM_RULES='[
  {"pattern": "kubectl delete namespace", "action": "Deny", "severity": "critical"},
  {"pattern": "docker rm -f", "action": "Deny", "severity": "high"}
]'
```

### 配置模板

```yaml
# OS Profile 自动选择
os_profile:
  type: ""         # 留空自动检测，可手动指定 linux / windows

# 服务调用模式
server:
  mode: "http"     # "http" (TCP) 或 "socket" (Unix Domain Socket)
  socket_path: "/var/run/agent_moss/agent_moss.sock"
```

## 项目结构

```
AgentMoss/
├── agent_moss/               # Python 实现 (PyPI: agent-moss)
│   ├── __init__.py
│   ├── cli.py                # 命令行工具 (init/analyze/server)
│   ├── __version__.py
│   ├── profiles/             # OS Profile 系统
│   │   ├── base.py           # OSProfile 抽象基类
│   │   ├── linux.py          # LinuxProfile
│   │   └── windows.py        # WindowsProfile
│   ├── engine/               # 安全分析引擎
│   │   ├── analyzer.py       # 分析入口
│   │   ├── coordinator.py    # 三层协调器 + L1.5/L2.5 递归脚本链 + fail-open
│   │   ├── heuristic.py      # 层1: 特征匹配检测 (危险命令 + 注入关键词，热重载 runtime_config)
│   │   ├── logic_rules.py    # 层2: 逻辑规则 (敏感路径/意图/密码/用户删除，credential 区分)
│   │   ├── llm_analyzer.py   # 层3: LLM + Skill 深度分析 (skip_l3_hints + token 记录)
│   │   ├── script_analyzer.py        # 脚本预扫描
│   │   ├── script_content_analyzer.py # L1.5/L2.5 递归脚本链内容分析 (移植自 audit_agent)
│   │   ├── inline_analyzer.py        # 内联脚本/文本区分
│   │   ├── skill_engine.py   # Skill 匹配引擎 (热重载 runtime_config 禁用 skill)
│   │   └── types.py
│   ├── infra/                # 基础设施
│   │   ├── config.py         # 配置管理 (YAML + 环境变量 + 继承 xiaoo config.toml)
│   │   ├── runtime_config.py # runtime JSON：seed/merge/CRUD/层级开关持久化 (51 函数)
│   │   ├── token_stats.py   # token 用量统计 (JSON 持久化)
│   │   ├── llm_client.py     # LLM 客户端 (流式回退 + Provider header 适配)
│   │   ├── actrail.py        # AcTrail 事件上报
│   │   ├── logging.py / parsers.py / policy_cache.py / prompt_templates.py
│   ├── server/               # HTTP API 服务层 (FastAPI)
│   │   ├── app.py            # 应用 + run_server + _find_free_port (端口动态分配)
│   │   ├── routes.py         # /api/v1/* 路由 (health/analyze/brain)
│   │   ├── models.py / middleware.py / socket_server.py
│   ├── console/              # Policy Console (FastAPI sub-router, 23 路由)
│   │   ├── router.py         # /console/api/* (layers/rules/skills/config/env/reset/token-stats)
│   │   └── static/index.html # 自包含 SPA (--ag- 风格 + 暗色模式)
│   ├── adapters/  # 适配器层 (observable)
│   ├── skills/    # 安全 Skill 规则 (Markdown)
│   ├── templates/ # Prompt 模板 + policy_mapping
│   ├── agents/    # 三方 agent 定制化扩展 (xiaoO / opendesk) — rules.json 单一事实源，py+ts 共享
│   └── brain/     # 规则自学习/存储
│
├── ts/                       # TypeScript 实现 (npm: @kenhkl/agent-moss) — 功能与 Python 对等
│   ├── src/
│   │   ├── cli.ts            # CLI 入口 (init/analyze/server)
│   │   ├── config.ts         # 配置 (环境变量 + fail-open)
│   │   ├── models.ts         # Zod 数据模型
│   │   ├── routes.ts         # Hono API 路由 (health/analyze)
│   │   ├── server.ts         # Hono HTTP + findFreePort + 挂载 Console
│   │   ├── actrail.ts / package-json.ts
│   │   ├── engine/           # 三层防御引擎 (与 py 对等)
│   │   │   ├── coordinator.ts / heuristic.ts / logic-rules.ts
│   │   │   ├── llm-analyzer.ts / script-analyzer.ts / script-content-analyzer.ts
│   │   │   ├── inline-analyzer.ts / skill-loader.ts / template-loader.ts
│   │   │   ├── token-stats.ts / types.ts
│   │   ├── infra/runtime-config.ts  # runtime JSON 全功能 (seed/merge/CRUD/query，对齐 py)
│   │   └── console/          # Policy Console (Hono sub-app, 23 路由 1:1 对齐 py)
│   │       ├── router.ts / index.ts
│   │       └── static/index.html  # 复用 py 的同一份 SPA
│   └── package.json
│
├── hooks/xiaoO/             # xiaoO hook 桥接 (bridge.py + plugin.json)
│   └── bridge.py             # stdin JSON → POST /api/v1/analyze → stdout {result,reason}；探测兜底
├── config/agent_moss.yaml   # YAML 配置模板
├── docs/
│   ├── design.md             # 架构需求设计
│   ├── ENV_VARS.md           # 环境变量参考 + audit_agent 迁移对照
│   └── ...
└── tests/
    ├── conftest.py / test_all_cases.py / test_agents_xiaoo.py / test_brain.py
    ├── test_find_free_port.py  # 端口动态分配单测
    └── cases/                  # 测试用例 JSON
```

## 测试

双栈（Python + TypeScript）行为一致性测试，每个功能单测 py/ts 各一份：

```bash
# Python 版
python3 -m pytest tests/ -v                # 全量（含 find_free_port、all_cases、agents_xiaoo 等）

# 包含 LLM 层（需 API Key，会触发 L3 真调用）
AGENT_MOSS_LLM_API_KEY="sk-xxx" python3 -m pytest tests/ -v

# TypeScript 版
cd ts && npx vitest run                     # 全量（含 find-free-port、runtime-config-seed、console-routes 等）
cd ts && npx tsc --noEmit                   # 类型检查
```
