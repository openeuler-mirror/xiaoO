# audit_agent 技术方案说明

> **项目名称**: audit_agent (audit-policy-checker)
> **版本**: 0.1.0
> **语言**: Python 3.10+
> **框架**: xiaoO AI Agent 插件体系
> **最后更新**: 2026-06-04

---

## 1. 项目概述

### 1.1 定位

audit_agent 是 xiaoO AI Agent 框架的**前置安全审计插件**，在 Agent 的每一次工具调用执行前进行拦截，通过三层防御体系判断该操作是否安全，并在放行时生成最小权限的 Cerberus 沙箱策略。

### 1.2 核心能力

| 能力 | 说明 |
|------|------|
| **动作安全判定** | 三层防御（启发式 → 逻辑规则 → LLM 深度分析），判定 Allow / Deny |
| **沙箱策略生成** | 对放行动作生成最小权限 TOML 策略（文件系统、网络、命名空间、资源限制） |
| **Prompt 注入防护** | 检测 45+ 种中英文 Prompt 注入模式（指令覆盖、角色劫持、社会工程等） |
| **快速放行优化** | 安全工具（glob/ls/cat 等）跳过 LLM 调用，延迟低至 ~2ms |

### 1.3 插件接入方式

通过 xiaoO 的 `*.Tool.*.pre` Hook 点接入，xiaoO 在每次工具调用前通过 stdin 发送 JSON payload，audit_agent 通过 stdout 返回 `{"result": "allow"}` 或 `{"result": "deny", "reason": "..."}`。

```json
// plugin.json
[{
  "id": "plugin_audit_tool_input",
  "hook_point": "*.Tool.*.pre",
  "command": "PYTHONPATH=audit_policy_checker audit_policy_checker/venv/bin/python3 audit.py"
}]
```

---

## 2. 系统架构

### 2.1 整体数据流

```
┌─────────────────────────────────────────────────────────────────────┐
│                        xiaoO Agent Loop                             │
│  用户 Prompt → LLM 决策 → 选择工具 → [Hook 拦截] → 执行 / 拒绝      │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ stdin JSON
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         audit.py (插件桥接)                          │
│  解析 hook payload → 提取工具字段 → 调用 audit_action()              │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                     main.py (审计编排器)                              │
│                                                                      │
│  ┌───────────────── Step 1: 安全判断 ─────────────────┐              │
│  │                                                     │              │
│  │  Layer 1 ─→ Layer 2 ─→ Layer 3                     │              │
│  │  启发式     逻辑规则   LLM+Skill                    │              │
│  │                                                     │              │
│  │  Deny? ──→ 立即返回 Deny                           │              │
│  │  Allow? ─→ 进入 Step 2                             │              │
│  └─────────────────────────────────────────────────────┘              │
│                                                                      │
│  ┌───────────────── Step 2: 策略生成 ─────────────────┐              │
│  │  缓存命中 → 使用缓存                               │              │
│  │  缓存未命中 → LLM 生成 TOML 策略 → 写入缓存        │              │
│  │  白名单工具 → 使用预定义最小权限策略                 │              │
│  └─────────────────────────────────────────────────────┘              │
│                                                                      │
│  → 返回 {decision, policy, reason}                                   │
└──────────────────────────────┬──────────────────────────────────────┘
                               │ stdout JSON
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                         xiaoO Agent Loop                             │
│  收到 allow → 执行工具 + 应用沙箱策略                                │
│  收到 deny  → 阻止执行 + 向用户展示原因                              │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 目录结构

```
audit_agent/
├── audit.py                          # 插件 Hook 桥接入口
├── plugin.json                       # 插件注册声明
├── audit_settings.json               # 运行时设置
├── install.sh                        # 安装脚本
├── README.md                         # 项目文档
├── SECURITY_RULES.md                 # 安全规则参考手册
└── audit_policy_checker/             # Python 包
    ├── pyproject.toml                # 包定义与依赖
    ├── audit_policy_checker/
    │   ├── __init__.py
    │   ├── main.py                   # 核心编排：audit_action()
    │   ├── cli.py                    # CLI 工具入口
    │   ├── config.py                 # 配置管理
    │   ├── llm_client.py             # LLM 调用客户端
    │   ├── parsers.py                # 响应解析器
    │   ├── policy_cache.py           # LRU + 文件策略缓存
    │   ├── prompt_templates.py       # Prompt 模板管理
    │   ├── logging_utils.py          # 审计日志
    │   ├── security/                 # 安全检测核心
    │   │   ├── audit_agent.py        # xiaoOSecBot 主协调器
    │   │   ├── heuristic_detector.py # Layer 1: 启发式检测
    │   │   ├── logic_rules.py        # Layer 2: 逻辑规则检测
    │   │   ├── llm_analyzer.py       # Layer 3: LLM 深度分析
    │   │   ├── script_content_analyzer.py  # 脚本内容预扫描
    │   │   ├── skill_engine.py       # Skill 匹配引擎
    │   │   └── types.py              # 数据类型定义
    │   ├── templates/                # Prompt 模板
    │   │   ├── prompt1_template.txt
    │   │   ├── prompt2_template.txt
    │   │   ├── security_judge_template.txt
    │   │   └── policy_mapping.md
    │   ├── skills/                   # 12 个安全 Skill 规则文件
    │   └── rules/
    │       └── user_rules.json       # 用户自定义敏感规则

# 测试用例（与源码分离，不包含在 RPM 包中）
plugins/tests/hookers/audit_agent/
├── test_fastpass.py                  # 快速放行集成测试
├── TEST_GUIDE.md                     # 测试指南
├── xiaoo/                            # run_rules_tests.py + shell 脚本 + rules/
└── cases/                            # auditagent CLI 测试用例
```

---

## 3. 三层防御体系详解

### 3.1 Layer 1: 启发式静态检测 (heuristic_detector.py)

**目标**: 毫秒级识别已知危险模式，高风险立即拦截。

| 子检测器 | 职责 | 规则数量 |
|----------|------|---------|
| `UserRuleMatcher` | 加载 `user_rules.json`，通配符模式匹配敏感动作 + 敏感工具精确匹配 | 17 条敏感动作 + 3 个敏感工具 |
| `CommandPatternScanner` | 编译正则检测危险命令（rm 变体、chmod 777、sudo、/etc/shadow、docker.sock、curl POST 等） | 20+ 条正则 |
| `InjectionKeywordChecker` | 双语 Prompt 注入关键词检测（指令覆盖、角色劫持、命令注入、社会工程、XSS、假边界、开发者模式等 8 类） | 45+ 关键词 |

**判定逻辑**:
- `risk_level = critical/high` → 立即 Deny（`confidence ≥ 80`）
- `risk_level = medium/low` 或 `confidence < 80` → 不拦截，传递给后续层

**内联脚本假阳性防护**:
对 `python -c "print('/etc/shadow')"` 这类内联脚本命令，`file_access` 类风险不立即 Deny（因为敏感路径可能出现在字符串字面量中），而是交给 Layer 3 LLM 做语义判断。

### 3.2 Layer 2: 逻辑规则检测 (logic_rules.py)

**目标**: 基于上下文的语义规则检查，利用动作历史和用户意图进行判断。

| 规则 | 说明 | 示例 |
|------|------|------|
| `read_before_write` | 写文件前必须先读取（新文件豁免） | 直接 `file_edit` 未读取过的文件 → Deny |
| `intent_consistency` | 用户意图关键词 vs 危险动作关键词一致性 | 用户说"查看日志"但动作是 `rm -rf` → Deny |
| `sensitive_path_access` | 25 个敏感路径分级（critical/high/medium） | `/etc/shadow`（读+写均阻断）、`.xiaoo-guardian/`（写阻断，读允许） |
| `dangerous_patterns` | 通配符+删除组合、重定向覆盖系统目录 | `rm *`、`> /etc/passwd` → Deny |

**敏感路径分级示例**:

| 级别 | 路径 | 读 | 写 |
|------|------|----|----|
| critical | `/etc/shadow`, `/etc/passwd` | ❌ | ❌ |
| critical | `~/.ssh/id_rsa`, `~/.ssh/authorized_keys` | ❌ | ❌ |
| high | `/etc/hosts`, `/etc/resolv.conf` | ✅ | ❌ |
| high | `~/.bashrc`, `~/.zshrc` | ✅ | ❌ |
| medium | `~/.gitconfig`, `~/.npmrc` | ✅ | ⚠️ |

### 3.3 Layer 3: LLM + Skill 深度分析 (llm_analyzer.py)

**目标**: 对前两层未能判定的复杂场景进行语义级安全分析。

**工作流程**:

```
输入: action + heuristic_result + logic_result
        │
        ▼
┌─── SkillEngine.match_skills() ───┐
│  从 12 个 Skill 文件中匹配       │
│  Top-3 相关规则（关键词评分）     │
└───────────────┬──────────────────┘
                │
                ▼
┌─── script_content_analyzer ──────┐
│  提取脚本路径 → 读取 ≤500 行     │
│  扫描 18 种高风险正则模式         │
│  评估关键词组合风险               │
└───────────────┬──────────────────┘
                │
                ▼
┌─── 组装结构化 Prompt ────────────┐
│  = security_judge_template       │
│  + 匹配的 Skill 规则文本         │
│  + Layer 1/2 检测提示            │
│  + 脚本预扫描结果                │
└───────────────┬──────────────────┘
                │
                ▼
┌─── LLM 调用 (带超时+重试) ──────┐
│  解析 JSON → SecurityJudgment    │
└──────────────────────────────────┘
```

**12 个安全 Skill 领域**:

| Skill 文件 | 覆盖场景 |
|------------|---------|
| `file_access_guard.md` | 文件访问风险 |
| `script_execution_guard.md` | 脚本执行风险 |
| `data_exfiltration_guard.md` | 数据外泄 |
| `supply_chain_guard.md` | 供应链攻击 |
| `persistence_backdoor_guard.md` | 持久化后门 |
| `lateral_movement_guard.md` | 横向移动 |
| `resource_exhaustion_guard.md` | 资源耗尽 |
| `browser_web_access_guard.md` | 浏览器/网络访问 |
| `email_operation_guard.md` | 邮件操作 |
| `intent_deviation_guard.md` | 意图偏离 |
| `skill_installation_guard.md` | Skill 安装风险 |
| `general_tool_risk_guard.md` | 通用工具风险兜底 |

**容错机制**:
- LLM 调用超时/异常 + 前序层有违规 → Deny（fail-closed）
- LLM 调用超时/异常 + 前序层无违规 → Allow（warn-allow）

**超时与卡死自愈（两层兜底）**:

L3 用 `ThreadPoolExecutor` 包装 `call_llm` 加超时，但 worker 线程是非守护线程。当 `call_llm` 卡死（HTTP 超时失效）且 `future.result(timeout)` 超时后，worker 仍永久阻塞，`ThreadPoolExecutor.__exit__` 的 `shutdown(wait=True)` 会等它 → audit.py 进程退出阶段也要等非守护线程 → **永久卡死**。为此设计两层兜底，任一生效即不卡死：

| 层 | 位置 | 机制 | 触发时机 |
|----|------|------|---------|
| ① audit 侧 | `audit.py` 末尾 | `os._exit` 代替 `sys.exit`，跳过解释器清理，不等非守护 worker，进程立即退出 | L3 超时（`AUDIT_LLM_TIMEOUT`，默认 300s）后 |
| ② hooker 侧 | `crates/hook` plugin adaptor | `tokio::process::Command` + `kill_on_drop(true)` + `tokio::time::timeout(600s)`，超时 SIGKILL 子进程 | audit.py 子进程 600s 不退出时 |

正常情况 ① 先生效（audit.py 在 L3 超时后自愈退出）；① 失效时 ② 兜底强杀。audit.py 无 atexit/需清理资源，`os._exit` 前 `flush` stdout 即安全。

### 3.4 快速放行优化 (Fast-Pass)

为避免安全工具产生不必要的 LLM 调用开销，设计了两级快速放行：

| 级别 | 工具列表 | 行为 | 延迟 |
|------|---------|------|------|
| **Tier 1 (完全安全)** | `glob`, `ls`, `list_dir`, `ask_user_question`, `count_text_length` + 安全 bash 命令 (`echo`, `pwd`, `which`, `date`, `whoami` 等) | 跳过 Layer 2 + Layer 3 | ~2ms |
| **Tier 2 (只读敏感)** | `read`, `file_read`, `head`, `tail`, `grep` + 只读 bash (`cat`, `wc`, `diff`, `find` 等) | 跳过 Layer 3，保留 Layer 2 | ~5ms |

> **前提条件**: Layer 1 启发式检测未发现 high/critical 风险。

---

## 4. 策略生成 (Step 2)

### 4.1 流程

1. 检查 LRU + 文件缓存（`/tmp/audit_policy_cache/`）
2. 缓存命中 → 直接使用
3. 缓存未命中 + Policy 生成已启用 → LLM 调用 PROMPT1 生成 TOML 策略
4. 缓存未命中 + Policy 生成已禁用 → 使用预定义最小权限策略
5. 白名单工具 → 始终使用预定义最小权限策略

### 4.2 最小权限策略示例 (TOML)

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

[namespaces]
mount = true
pid = false
network = true
user = false

[resources]
timeout_secs = 30
max_memory_bytes = 268435456   # 256MB

[environment]
whitelist = ["PATH", "LANG", "HOME", "USER", "HTTP_PROXY", "HTTPS_PROXY"]
```

### 4.3 缓存策略

- **内存**: LRU 缓存，默认最大 1000 条
- **持久化**: 文件系统缓存于 `/tmp/audit_policy_cache/`
- **键**: `(session_id, prompt_session)` 组合

---

## 5. 配置体系

### 5.1 配置优先级

```
环境变量 > audit_settings.json > config.json > 默认值
```

### 5.2 config.json (LLM 与安全配置)

| 分组 | 字段 | 默认值 | 说明 |
|------|------|--------|------|
| `llm` | `api_key` | `""` | API Key（环境变量 `OPENROUTER_API_KEY` 可覆盖） |
| `llm` | `model` | `anthropic/claude-3.5-sonnet` | LLM 模型 |
| `llm` | `temperature` | `0.1` | 生成温度 |
| `llm` | `base_url` | `https://openrouter.ai/api/v1` | API 端点 |
| `llm` | `provider` | `""` | Provider 名称（用于自动注入请求头） |
| `timeout` | `total_timeout` | `10.0` | 总超时（秒） |
| `timeout` | `step_interval` | `0.0` | Step 1 与 Step 2 之间间隔 |
| `cache` | `enabled` | `true` | 启用策略缓存 |
| `cache` | `max_size` | `1000` | 最大缓存条目 |
| `retry` | `max_retries` | `3` | LLM 调用重试次数 |
| `security` | `enabled` | `true` | 安全检测总开关 |
| `security` | `heuristic_enabled` | `true` | Layer 1 开关 |
| `security` | `logic_rules_enabled` | `true` | Layer 2 开关 |
| `security` | `llm_analysis_enabled` | `true` | Layer 3 开关 |

### 5.3 audit_settings.json (运行时设置)

| 字段 | 默认值 | 说明 |
|------|--------|------|
| `AUDIT_DISABLE_LLM_LAYER3` | `""` (启用) | 设为 `"1"` 禁用 Layer 3 |
| `AUDIT_LLM_TIMEOUT` | `300` | LLM 超时（秒） |
| `AUDIT_LOG_PATH` | `""` | 调试日志路径（记录 HOOK_INPUT/HOOK_OUTPUT 全量日志与 LLM prompt 日志） |
| `AUDIT_ENABLE_POLICY_GEN` | `""` (禁用) | 设为 `"1"` 启用 LLM 策略生成 |
| `AUDIT_CONFIG_PATH` | `""` | 自定义 config.json 路径，指向的文件存在时生效 |
| `AUDIT_RUNTIME_CONFIG_PATH` | `""` | 自定义运行时热加载配置路径 |

**文件位置**：audit_settings.json 统一放在**插件根目录**（与 `audit.py` 同级）。无论 pip 安装（venv 形态）还是 RPM 源码直跑形态，`audit.py` 启动时都会把自身所在目录注入 `AUDIT_PLUGIN_ROOT` 环境变量，`config.py` 据此定位到统一的 audit_settings.json。`AUDIT_LOG_PATH` 在此配置即让 HOOK_INPUT/HOOK_OUTPUT 全量日志写入指定文件，效果与设置环境变量一致。

### 5.4 支持的 LLM Provider

OpenRouter、DeepSeek、智谱 (Zhipu)、xAI、OpenAI、Anthropic 等 15+ Provider，通过 URL 自动识别并注入 Provider 特定请求头。

---

## 6. 输入输出协议

### 6.1 输入 (xiaoO → audit_agent)

```json
{
  "call": {
    "tool_name": "bash",
    "input": {"command": "rm -rf /tmp/test"}
  },
  "metadata": {
    "trace_id": "xxx",
    "span_id": "yyy"
  },
  "session_id": "session-001",
  "prompt_session": "帮我清理临时文件",
  "action_history": [
    {"name": "bash", "input": "ls /tmp"}
  ],
  "reason": "用户要求清理临时目录",
  "policy": {
    "llm_analysis": true
  }
}
```

### 6.2 输出 (audit_agent → xiaoO)

**放行**:
```json
{"result": "allow", "reason": "安全的只读 bash 命令: ls /tmp"}
```

**拒绝**:
```json
{"result": "deny", "reason": "[command_execution] 检测到危险命令: rm -rf 递归删除"}
```

### 6.3 内部返回结构 (audit_action)

```python
{
    "decision": "Allow" | "Deny",
    "policy": "<TOML 字符串>",
    "reason": "判断原因",
    "violated_policy": "违规描述（Deny 时）",
    "violated_layers": ["1.1", "1.2"]  # 违反的层号
}
```

---

## 7. 依赖项

| 包 | 版本 | 用途 |
|----|------|------|
| `openai` | ≥1.0 | OpenAI 兼容 API 客户端 |
| `httpx` | ≥0.25 | HTTP 客户端 |
| `pydantic` | ≥2.0 | 数据校验 |
| `tenacity` | ≥8.0 | 重试机制 |
| `loguru` | ≥0.7 | 结构化日志 |
| `tomli` | ≥2.0 | TOML 解析（Python <3.11） |

---

## 8. 部署与安装

### 8.1 安装方式

```bash
# 方式 1: 通过 xiaoO 构建系统
./build.sh --release

# 方式 2: 通过插件安装脚本
./plugins/hookers/install.sh audit_agent --enable-llm

# 方式 3: 手动安装
cd audit_policy_checker
python -m venv venv
source venv/bin/activate
pip install -e .
```

### 8.2 CLI 使用

```bash
# 生成输入模板
auditagent --init

# 执行审计
auditagent input.json

# 指定输出目录
auditagent input.json -o ./results

# 仅输出到终端
auditagent input.json --no-file

# 控制 API 限流
auditagent input.json --step-interval 10
```

**退出码**: `0` = Allow, `1` = Deny, `2` = 输入错误。

### 8.3 Python API

```python
from audit_policy_checker.main import audit_action

result = audit_action(
    session_id="session-001",
    prompt_session="帮我查看日志文件",
    action_history=[],
    a_next={"action_type": "bash", "action_detail": "cat /var/log/syslog"},
    reason="用户要求查看系统日志",
)
# result = {"decision": "Allow", "policy": "...", "reason": "...", ...}
```

---

## 9. 测试体系

### 9.1 测试层次

| 层次 | 工具 | 覆盖范围 |
|------|------|---------|
| 单元测试 | pytest | 各模块独立测试 |
| 集成测试 | `plugins/tests/hookers/audit_agent/test_fastpass.py` | 27 个快速放行场景（Tier 1/Tier 2 工具、敏感路径、危险命令） |
| 规则测试 | `plugins/tests/hookers/audit_agent/xiaoo/run_rules_tests.py` | 51 个 JSON 测试用例（Level 1: 33, Level 2: 4, Level 3: 13, Cerberus: 1） |
| 端到端测试 | Shell 脚本 | 30+ 个场景脚本（allow/deny 分类） |

### 9.2 规则测试用例分布

| Level | 数量 | 覆盖内容 |
|-------|------|---------|
| Level 1 | 33 | Layer 1 启发式规则：sudo、rm -rf、chmod 777、/etc/shadow、SSH 密钥、curl POST、crontab、Docker socket、Prompt 注入关键词等 |
| Level 2 | 4 | Layer 2 逻辑规则：read_before_write、intent_consistency、sensitive_paths、dangerous_patterns |
| Level 3 | 13 | Layer 3 深度分析：反向 Shell (nc -e, /dev/tcp)、供应链 typosquatting、数据外泄、Fork Bomb、磁盘填充、SSH 隧道 |
| Cerberus | 1 | Guardian 目录保护 |

---

## 10. 关键设计决策

### 10.1 Fail-Closed 策略

审计过程中的任何未预期异常均返回 Deny，确保安全兜底。LLM 调用失败时的降级策略：
- 前序层有违规记录 → Deny（fail-closed）
- 前序层无违规记录 → Allow + 告警（warn-allow）

### 10.2 内联脚本假阳性防护

`python -c "print('/etc/shadow')"` 中的 `/etc/shadow` 是字符串字面量而非实际文件操作。纯正则无法区分，因此对 `python -c`、`perl -e` 等内联脚本命令的 `file_access` 风险不立即 Deny，交给 Layer 3 LLM 做语义判断。

### 10.3 两级快速放行

安全工具（glob、ls、cat、grep 等）不具备写入或执行能力，跳过 LLM 调用可将延迟从秒级降至毫秒级，显著降低 Agent 循环的整体延迟。

### 10.4 Skill 引导的 LLM 分析

通过关键词匹配从 12 个领域 Skill 文件中选取 Top-3 注入 LLM Prompt，相比通用 Prompt，能显著提升对特定安全场景（数据外泄、供应链攻击、持久化后门等）的识别准确率。

---

## 11. 性能指标

| 场景 | 典型延迟 |
|------|---------|
| Tier 1 快速放行 (glob/ls/echo) | ~2ms |
| Tier 2 快速放行 (cat/grep) | ~5ms |
| Layer 1+2 静态分析 | ~10-50ms |
| Layer 3 LLM 分析 | 1-10s（取决于 Provider） |
| 策略生成 (LLM) | 2-15s |

---

## 12. 后续演进方向

- **动态规则热加载**: 支持运行时更新 `user_rules.json` 和 Skill 文件，无需重启
- **审计日志可视化**: 提供 Web Dashboard 展示审计历史、拦截统计、风险趋势
- **自定义 Skill 扩展**: 开放 Skill 文件编写规范，支持业务方按需添加安全规则
- **多模型协同**: 对 Layer 3 引入多模型投票机制，提升判断准确率
- **策略模板库**: 预置常见场景的策略模板，减少 LLM 调用频率
