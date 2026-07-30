# AgentMoss（agent_moss）需求设计文档

> 版本：v1.0  
> 日期：2026-05-18  
> 状态：待确认

---

## 目录

1. [背景与动机](#1-背景与动机)
   - [1.1 项目背景](#11-项目背景)
   - [1.1.1 当前阶段约束](#111-当前阶段约束)
   - [1.2 为什么要从 audit_agent 改造为 agent_moss](#12-为什么要从-audit_agent-改造为-agent_moss)
   - [1.3 核心目标](#13-核心目标)
2. [现状分析：audit_agent 架构回顾](#2-现状分析audit_agent-架构回顾)
3. [目标架构设计](#3-目标架构设计)
   - [3.1 openEuler 上的独立部署架构（当前阶段）](#31-openeuler-上的独立部署架构当前阶段)
   - [3.2 agent_moss 内部架构](#32-agent_moss-内部架构)
   - [3.3 数据流设计](#33-数据流设计)
4. [详细模块设计](#4-详细模块设计)
5. [接口协议设计](#5-接口协议设计)
6. [配置体系设计](#6-配置体系设计)
7. [迁移改造计划](#7-迁移改造计划)
8. [风险与缓解措施](#8-风险与缓解措施)
9. [附录](#9-附录)
   - [9.3 待确认事项（已结合 openEuler 独立部署约束给出建议方案）](#93-待确认事项已结合-openeuler-独立部署约束给出建议方案)

---

## 1. 背景与动机

### 1.1 项目背景

**AgentOS** 是 openEuler 的一个发行版，其设计目标是构建一个以 AI Agent 为核心的操作系统。在 AgentOS 中：

- **xiaoO** 作为权限最高的系统管家（Agent Manager），负责管理 Agent 的生命周期，包括拉起沙箱、启动 Agent、监控 Agent 行为等。
- 其他 Agent 在 xiaoO 创建的沙箱中运行，执行各类任务。
- AgentOS 在系统服务层提供**"可观测"服务（Observability Service）**，负责收集所有 Agent 在实际运行中产生的系统调用（syscall）数据，并对其他服务提供数据查询接口。

**AgentMoss（agent_moss）** 作为 AgentOS 的安全模块，在收到从"可观测"服务获取的 Agent 行为数据后，执行多层安全分析，并将安全判定结果返回给"可观测"服务，供其做出允许/阻断决策。

### 1.1.1 当前阶段约束

**AgentOS 框架代码尚未就绪**。因此 agent_moss 的第一阶段目标是在 **openEuler 操作系统上作为独立的系统服务运行**，提供标准的 HTTP API 供外部调用（包括未来的"可观测"服务、CLI 工具、或其他 AgentOS 组件）。架构设计上预留适配器层，确保未来 AgentOS 框架就绪后可平滑接入。

当前阶段的核心原则：
- **独立可运行**：不依赖 AgentOS、xiaoO 或任何尚未就绪的组件
- **接口前向兼容**：API 设计与未来 AgentOS "可观测"服务对接需求一致
- **配置自包含**：使用独立的 YAML 配置文件，不依赖 xiaoO config.toml
- **部署标准化**：遵循 openEuler 系统服务规范（systemd unit、FHS 路径）

### 1.2 为什么要从 audit_agent 改造为 agent_moss？

| 维度 | audit_agent（现状） | agent_moss（目标） |
|------|---------------------|---------------------|
| **定位** | xiaoO 的应用层插件 | AgentOS 的系统服务层安全模块 |
| **部署层级** | 用户空间插件（Python venv） | 系统服务（systemd / daemon） |
| **触发方式** | xiaoO Hook 机制（stdin JSON） | "可观测"服务主动调用 API |
| **输入来源** | xiaoO 在工具调用前传入 | "可观测"服务收集的 agent syscall 数据 |
| **输出目标** | xiaoO（PreHookResult JSON） | "可观测"服务（结构化安全分析结果） |
| **生命周期** | 随 xiaoO 调用启停 | 常驻系统服务，持续运行 |
| **配置管理** | 依赖 xiaoO config.toml | 独立的系统级配置文件 |
| **xiaoO 特定逻辑** | 大量 xiaoO 专属检测规则 | 移除 xiaoO 特有，增加 AgentOS 通用规则 |

### 1.3 核心目标

1. **代码复用**：最大化复用现有三层安全分析引擎（启发式 + 逻辑规则 + LLM+Skill），避免重复开发。
2. **层级下沉**：将安全分析能力从应用层插件下沉到系统服务层，成为 AgentOS 基础设施的一部分。
3. **接口标准化**：定义清晰的服务间通信协议，使"可观测"服务及其他 AgentOS 组件可以标准方式调用。
4. **通用化**：移除 xiaoO 专属逻辑，使其适用于任何 AgentOS 中运行的 Agent。

---

## 2. 现状分析：audit_agent 架构回顾

### 2.1 当前架构总览

```
┌──────────────────────────────────────────────────────────────┐
│  xiaoO Agent 执行循环                                         │
│                                                              │
│  Agent LLM → 决定下一步动作 (a_next + reason)                  │
│       │                                                      │
│       ▼                                                      │
│  ┌─────────────────────────────────────────┐                 │
│  │  audit_agent (xiaoO Plugin)             │                 │
│  │  ┌───────────────────────────────────┐  │                 │
│  │  │ audit.py (xiaoO Hook Bridge)      │  │                 │
│  │  │  stdin JSON → a_next 格式转换      │  │                 │
│  │  └───────────────┬───────────────────┘  │                 │
│  │                  │                       │                 │
│  │  ┌───────────────▼───────────────────┐  │                 │
│  │  │ main.py :: audit_action()         │  │                 │
│  │  │                                   │  │                 │
│  │  │  Step 1: 安全判断 (judge_security) │  │                 │
│  │  │  Step 2: Policy 生成 + 缓存        │  │                 │
│  │  └───────────────┬───────────────────┘  │                 │
│  │                  │                       │                 │
│  │  ┌───────────────▼───────────────────┐  │                 │
│  │  │ security/ (三层防御引擎)           │  │                 │
│  │  │  ├── audit_agent.py (协调器)      │  │                 │
│  │  │  ├── heuristic_detector.py (层1)  │  │                 │
│  │  │  ├── logic_rules.py (层2)         │  │                 │
│  │  │  ├── llm_analyzer.py (层3)        │  │                 │
│  │  │  ├── skill_engine.py              │  │                 │
│  │  │  ├── script_content_analyzer.py   │  │                 │
│  │  │  └── types.py                     │  │                 │
│  │  └───────────────────────────────────┘  │                 │
│  └─────────────────────────────────────────┘                 │
│       │                                                      │
│       ▼ stdout JSON                                          │
│  {"result": "allow"} / {"result": "deny", "reason": "..."}   │
└──────────────────────────────────────────────────────────────┘
```

> **注**：上图是改造前的 audit_agent 旧架构（xiaoO 插件形态，stdin/stdout 触发），
> 仅供回顾迁移起点。改造后的 agentmoss 实际结构见 [3.2 节](#32-agent_moss-内部架构)：
> HTTP 服务形态，模块 `audit_agent.py`/`heuristic_detector.py`/`main.py` 已分别
> 改为 `engine/coordinator.py`/`engine/heuristic.py`/`engine/analyzer.py`。

### 2.2 三层安全分析引擎（核心资产，需保留）

```
a_next (待执行动作)
    │
    ▼
┌─────────────────────────────────────────────┐
│ 层1: 启发式静态检测 (HeuristicDetector)       │
│ ├── UserRuleMatcher: 用户自定义规则匹配       │
│ ├── CommandPatternScanner: 危险命令正则       │
│ └── InjectionKeywordChecker: Prompt注入检测   │
│     → high/critical → 直接 Deny (短路)        │
│     → 内联脚本 file_access 转层3              │
│       （避免 'cat /etc/shadow' 假阳性）       │
│     → 凭据文件（credentials.yml 等）不转层3   │
│       （直接 Deny，不允许 LLM 覆盖）          │
└──────────────┬──────────────────────────────┘
               │ not high/critical
               ▼
┌─────────────────────────────────────────────┐
│ 白名单只读工具快速放行                         │
│ (READONLY_SAFE_TOOLS + SAFE_ACTION_TYPES)    │
│     → 安全 → 直接 Allow                       │
└──────────────┬──────────────────────────────┘
               │ 非白名单工具
               ▼
┌─────────────────────────────────────────────┐
│ 层2: 逻辑规则检测 (LogicRulesChecker)         │
│ ├── read_before_write 原则                   │
│ ├── 意图一致性检测                            │
│ ├── 敏感路径访问检测（含凭据文件，\b边界匹配）  │
│ └── 危险操作模式检测                          │
│     → high/critical → 直接 Deny (短路)        │
│     → 内联脚本 file_access 转层3              │
│     → 凭据文件（credentials.yml 等）不转层3   │
└──────────────┬──────────────────────────────┘
               │ not high/critical
               ▼
┌─────────────────────────────────────────────┐
│ 层3: LLM + Skill 深度分析 (LLMAnalyzer)      │
│ ├── SkillEngine.match_skills() 规则匹配       │
│ ├── script_content_analyzer 脚本预扫描        │
│ └── call_llm() 语义安全判断                   │
│     → 返回 SecurityJudgment                   │
│     Fail: fail-closed + warn-allow           │
└─────────────────────────────────────────────┘
```

### 2.3 当前输入格式

```json
{
    "session_id": "会话唯一标识",
    "prompt_session": "用户原始 prompt",
    "action_history": [
        {"name": "a1", "action_detail": "..."},
        {"name": "a2", "action_detail": "..."}
    ],
    "a_next": {
        "action_type": "bash",
        "action_detail": "cat /etc/passwd"
    },
    "reason": "执行该动作的理由"
}
```

### 2.4 当前输出格式

```python
# Allow
{"decision": "Allow", "policy": "<TOML>", "reason": "...", "violated_policy": ""}

# Deny
{"decision": "Deny", "policy": "", "reason": "...", "violated_policy": "[risk_type] ..."}
```

### 2.5 当前需要移除/改造的部分

| 组件 | 处理方式 | 原因 |
|------|----------|------|
| `audit.py` (xiaoO Hook Bridge) | **移除** | 不再通过 xiaoO hook 机制触发 |
| `plugin.json` | **移除** | 不再作为 xiaoO 插件 |
| `install.sh` | **重写** | 改为系统服务安装脚本 |
| `~/.config/xiaoo/config.toml` 依赖 | **移除** | 使用独立配置文件 |
| xiaoO 专属检测规则（xiaoo-guardian, xiaoo.env 等） | **移除/通用化** | 不再在 xiaoO 上下文中运行 |
| `audit_settings.json` | **改造** | 改为 agent_moss 服务配置文件 |
| `config.json` (LLM 配置) | **保留并改造** | 核心 LLM 配置保留，移除 xiaoO fallback |
| 三层安全引擎 (`security/`) | **核心保留** | 核心分析能力完整保留 |
| Skill 规则 (`skills/`) | **保留并扩展** | 增加 AgentOS 沙箱相关 Skill |
| 用户规则 (`rules/`) | **保留** | 通用的用户自定义规则机制 |
| Prompt 模板 (`templates/`) | **保留并适配** | 调整提示词上下文描述 |
| Policy 生成 (Step 2) | **保留为可选** | 沙箱场景下仍可能需要 Cerberus Policy |

---

## 3. 目标架构设计

### 3.1 openEuler 上的独立部署架构（当前阶段）

由于 AgentOS 框架代码尚未就绪，agent_moss 第一阶段作为 **openEuler 上的独立系统服务**运行，通过 HTTP API 对外提供安全分析能力。未来 AgentOS "可观测"服务及其他组件就绪后，通过适配器层平滑接入。

```
┌─────────────────────────────────────────────────────────────────┐
│                     openEuler 操作系统                            │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │                    应用层 (User Space)                      │ │
│  │                                                             │ │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────────────────────┐ │ │
│  │  │  CLI 工具 │  │ 测试脚本  │  │ 其他调用方（未来可观测服务）│ │ │
│  │  └────┬─────┘  └────┬─────┘  └────────────┬─────────────┘ │ │
│  │       │             │                     │                │ │
│  │       │             ▼                     │                │ │
│  │       │    POST /v1/analyze (JSON)        │                │ │
│  │       └──────────────┬────────────────────┘                │ │
│  └──────────────────────┼──────────────────────────────────────┘ │
│                         │                                       │
│  ┌──────────────────────▼──────────────────────────────────────┐ │
│  │              系统服务层 (System Services)                    │ │
│  │                                                             │ │
│  │  ┌───────────────────────────────────────────────────────┐ │ │
│  │  │              agent_moss (systemd service)              │ │ │
│  │  │                                                       │ │ │
│  │  │  ┌─────────────────────────────────────────────────┐  │ │ │
│  │  │  │  HTTP Server (FastAPI)                          │  │ │ │
│  │  │  │  · POST /api/v1/analyze                         │  │ │ │
│  │  │  │  · GET  /api/v1/health                          │  │ │ │
│  │  │  │  · GET  /api/v1/metrics                         │  │ │ │
│  │  │  └───────────────────┬─────────────────────────────┘  │ │ │
│  │  │                      │                                 │ │ │
│  │  │  ┌───────────────────▼─────────────────────────────┐  │ │ │
│  │  │  │  适配器层 (预留)                                  │  │ │ │
│  │  │  │  · ObservableAdapter  (未来 AgentOS 可观测服务)   │  │ │ │
│  │  │  │  · DirectAdapter     (当前直接 JSON 调用)        │  │ │ │
│  │  │  └───────────────────┬─────────────────────────────┘  │ │ │
│  │  │                      │                                 │ │ │
│  │  │  ┌───────────────────▼─────────────────────────────┐  │ │ │
│  │  │  │  安全分析引擎 (三层防御)                          │  │ │ │
│  │  │  │  启发式 → 逻辑规则 → LLM+Skill                   │  │ │ │
│  │  │  └─────────────────────────────────────────────────┘  │ │ │
│  │  │                                                       │ │ │
│  │  │  配置: /etc/agent_moss/agent_moss.yaml                │ │ │
│  │  │  日志: /var/log/agent_moss/                            │ │ │
│  │  │  运行: /var/run/agent_moss/                            │ │ │
│  │  └───────────────────────────────────────────────────────┘ │ │
│  └───────────────────────────────────────────────────────────┘ │
│                                                                 │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │                   内核层 (Kernel)                            │ │
│  │   eBPF / seccomp / Landlock / ...                          │ │
│  └───────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**未来演进路径**：当 AgentOS 框架就绪后，agent_moss 仅需：
1. 添加 `ObservableAdapter` 适配"可观测"服务的数据格式
2. 将 socket 路径或 HTTP 端点注册到 AgentOS 服务发现
3. 核心安全引擎代码**零改动**

### 3.2 agent_moss 内部架构

```
┌──────────────────────────────────────────────────────────────┐
│                    agent_moss 服务                            │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │              对外接口层 (API Layer)                      │  │
│  │                                                        │  │
│  │  · Unix Domain Socket / gRPC / HTTP REST               │  │
│  │  · POST /v1/analyze     — 提交安全分析请求              │  │
│  │  · GET  /v1/health      — 健康检查                      │  │
│  │  · GET  /v1/config      — 查询当前配置                   │  │
│  └──────────────────────┬─────────────────────────────────┘  │
│                         │                                     │
│  ┌──────────────────────▼─────────────────────────────────┐  │
│  │              请求适配层 (Adapter Layer)                  │  │
│  │                                                        │  │
│  │  · 请求反序列化与校验 (Pydantic models)                  │  │
│  │  · 格式转换：可观测服务格式 → 内部 a_next 格式            │  │
│  │  · 响应序列化                                            │  │
│  └──────────────────────┬─────────────────────────────────┘  │
│                         │                                     │
│  ┌──────────────────────▼─────────────────────────────────┐  │
│  │         安全分析引擎 (Security Engine) [核心复用]         │  │
│  │                                                        │  │
│  │  ┌──────────────────────────────────────────────────┐  │  │
│  │  │  analyzer.py :: analyze()  对外分析入口              │  │  │
│  │  │                                                    │  │  │
│  │  │  Step 1: 安全判断 (coordinator 三层防御协调)        │  │  │
│  │  │  Step 2: [可选] Policy 生成 + 缓存                  │  │  │
│  │  └────────────────────┬─────────────────────────────┘  │  │
│  │                       │                                 │  │
│  │  ┌────────────────────▼─────────────────────────────┐  │  │
│  │  │  engine/ (三层防御引擎)                            │  │  │
│  │  │  ├── coordinator.py     协调器（三层串联）         │  │  │
│  │  │  ├── heuristic.py       层1 特征匹配静态检测        │  │  │
│  │  │  ├── logic_rules.py      层2 逻辑规则检测           │  │  │
│  │  │  ├── llm_analyzer.py     层3 LLM+Skill 深度分析    │  │  │
│  │  │  ├── skill_engine.py     Skill 匹配                │  │  │
│  │  │  ├── script_content_analyzer.py  脚本内容预扫描    │  │  │
│  │  │  ├── inline_analyzer.py  内联脚本语义判定          │  │  │
│  │  │  └── types.py          类型定义                   │  │  │
│  │  └──────────────────────────────────────────────────┘  │  │
│  └──────────────────────┬─────────────────────────────────┘  │
│                         │                                     │
│  ┌──────────────────────▼─────────────────────────────────┐  │
│  │              基础设施层 (Infrastructure)                 │  │
│  │                                                        │  │
│  │  · llm_client.py       — LLM 客户端 (保留)              │  │
│  │  · parsers.py          — 响应解析 (保留)                │  │
│  │  · prompt_templates.py — Prompt 模板 (保留)             │  │
│  │  · policy_cache.py     — 策略缓存 (保留)                │  │
│  │  · logging_utils.py    — 审计日志 (保留并增强)          │  │
│  │  · config.py           — 配置管理 (改造)                │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │              服务治理层 (Service Governance)             │  │
│  │                                                        │  │
│  │  · 服务生命周期管理 (daemon)                            │  │
│  │  · 健康检查端点                                         │  │
│  │  · 优雅关闭 (SIGTERM handler)                           │  │
│  │  · 配置热加载 (SIGHUP handler)                          │  │
│  │  · 指标暴露 (Prometheus metrics)                       │  │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

### 3.3 数据流设计

```
Agent 沙箱中运行
      │
      │ 实际 syscall
      ▼
┌─────────────────┐
│  "可观测" 服务    │  收集 syscall 数据，转换为结构化动作描述
│  (Observability) │
└────────┬────────┘
         │ 1. POST /v1/analyze (安全分析请求)
         │    Content-Type: application/json
         │    Body: AnalyzeRequest
         ▼
┌─────────────────┐
│   agent_moss    │
│                 │
│  2. 请求校验     │  Pydantic 模型校验
│  3. 格式适配     │  Observable格式 → 内部 AnalyzeRequest 格式
│  4. 三层安全分析  │  启发式 → 逻辑规则 → LLM+Skill
│  5. Policy生成   │  [可选] Cerberus 沙箱 Policy
│  6. 审计日志     │  记录完整分析过程
│                 │
└────────┬────────┘
         │ 7. Response: AnalyzeResponse
         │    {decision, risk_level, risk_type, violated_layers, reason, policy}
         ▼
┌─────────────────┐
│  "可观测" 服务    │
│                 │
│  Allow → 放行    │
│  Deny  → 阻断    │
└─────────────────┘
```

---

## 4. 详细模块设计

### 4.1 目录结构

```
agent_moss/
├── setup.py / pyproject.toml      # Python 包配置
├── requirements.txt               # 依赖列表
├── README.md                      # 项目文档
├── Makefile                       # 构建/安装/测试
│
├── config/
│   ├── agent_moss.yaml            # 主配置文件（YAML，替代 audit_settings.json + config.json）
│   └── agent_moss.yaml.example    # 配置示例
│
├── agent_moss/                    # 主 Python 包
│   ├── __init__.py                # 包初始化
│   ├── __version__.py             # 版本号
│   │
│   ├── server/                    # 服务层（新增）
│   │   ├── __init__.py
│   │   ├── app.py                 # 服务主入口（FastAPI / gRPC server）
│   │   ├── routes.py              # API 路由定义
│   │   ├── models.py              # Pydantic 请求/响应模型
│   │   ├── middleware.py          # 中间件（日志、限流、认证）
│   │   └── health.py              # 健康检查
│   │
│   ├── adapters/                  # 适配器层（新增）
│   │   ├── __init__.py
│   │   ├── base.py                # 适配器基类
│   │   └── observable.py          # "可观测"服务数据格式适配器
│   │
│   ├── engine/                    # 安全分析引擎（核心复用，原 main.py + security/）
│   │   ├── __init__.py
│   │   ├── analyzer.py            # 核心入口 analyze()（重命名自 main.py :: audit_action()）
│   │   ├── coordinator.py         # 安全判断协调器（重命名自 security/audit_agent.py）
│   │   ├── heuristic.py           # 层1: 启发式检测（原 heuristic_detector.py）
│   │   ├── logic_rules.py         # 层2: 逻辑规则检测
│   │   ├── llm_analyzer.py        # 层3: LLM 深度分析
│   │   ├── skill_engine.py        # Skill 引擎
│   │   ├── script_analyzer.py     # 脚本内容分析
│   │   └── types.py               # 类型定义
│   │
│   ├── skills/                    # 安全 Skill 规则（保留 + 扩展）
│   │   ├── file_access_guard.md
│   │   ├── script_execution_guard.md
│   │   ├── data_exfiltration_guard.md
│   │   ├── browser_web_access_guard.md
│   │   ├── email_operation_guard.md
│   │   ├── lateral_movement_guard.md
│   │   ├── persistence_backdoor_guard.md
│   │   ├── resource_exhaustion_guard.md
│   │   ├── skill_installation_guard.md
│   │   ├── supply_chain_guard.md
│   │   ├── intent_deviation_guard.md
│   │   ├── general_tool_risk_guard.md
│   │   └── sandbox_escape_guard.md        # 新增：沙箱逃逸检测
│   │
│   ├── rules/                     # 用户自定义规则
│   │   └── user_rules.json
│   │
│   ├── templates/                 # Prompt 模板
│   │   ├── prompt1_template.txt
│   │   ├── prompt2_template.txt
│   │   ├── security_judge_template.txt
│   │   └── policy_mapping.md
│   │
│   ├── infra/                     # 基础设施（原平铺的模块）
│   │   ├── __init__.py
│   │   ├── config.py              # 配置管理（改造）
│   │   ├── llm_client.py          # LLM 客户端
│   │   ├── parsers.py             # 响应解析
│   │   ├── prompt_templates.py    # Prompt 模板
│   │   ├── policy_cache.py        # 策略缓存
│   │   └── logging.py             # 审计日志
│   │
│   └── cli.py                     # 命令行工具（保留，增强）
│
├── tests/                         # 测试
│   ├── unit/
│   │   ├── test_heuristic.py
│   │   ├── test_logic_rules.py
│   │   ├── test_llm_analyzer.py
│   │   └── test_adapters.py
│   ├── integration/
│   │   ├── test_api.py
│   │   └── test_e2e.py
│   └── cases/                     # 测试用例 JSON（保留）
│
├── scripts/                       # 运维脚本
│   ├── install.sh                 # 系统服务安装
│   ├── uninstall.sh               # 卸载
│   └── agent_moss.service         # systemd unit 文件
│
└── docs/
    ├── design.md                  # 本文档
    ├── api.md                     # API 文档
    └── deployment.md              # 部署文档
```

### 4.2 核心类/函数重命名映射

| 原名 | 新名 | 位置 |
|------|------|------|
| `audit_action()` | `analyze()` | `engine/analyzer.py` |
| `judge_security()` | `judge_security()` | `engine/coordinator.py` |
| `xiaoOSecBot` | `AgentMossBot` | `engine/coordinator.py` |
| `HeuristicDetector` | `HeuristicDetector` | `engine/heuristic.py` |
| `LogicRulesChecker` | `LogicRulesChecker` | `engine/logic_rules.py` |
| `LLMAnalyzer` | `LLMAnalyzer` | `engine/llm_analyzer.py` |
| `SkillEngine` | `SkillEngine` | `engine/skill_engine.py` |
| `SecurityJudgment` | `SecurityJudgment` | `engine/types.py` |
| `PolicyCache` | `PolicyCache` | `infra/policy_cache.py` |
| `AuditLogger` | `AuditLogger` → `MossLogger` | `infra/logging.py` |
| `Config` | `Config` | `infra/config.py` |
| `SAFE_ACTION_TYPES` | `SAFE_ACTION_TYPES` | `engine/heuristic.py` |
| `READONLY_SAFE_TOOLS` | `READONLY_SAFE_TOOLS` | `engine/coordinator.py` |

### 4.3 需要移除的 xiaoO 特定逻辑

| 位置 | 内容 | 处理方式 |
|------|------|----------|
| `heuristic_detector.py` L72-90 | `xiaoo.env`, `xiaoo.toml`, `llm_secrets.json` 检测 | **移除** |
| `heuristic_detector.py` L80-91 | `xiaoo-guardian` 目录保护检测 | **移除** |
| `logic_rules.py` L43 | `~/.xiaoo/skills/xiaoo-guardian/` 路径 | **移除** |
| `logic_rules.py` L252-254 | `xiaoo-guardian` 区分读写逻辑 | **移除** |
| `audit.py` 全文件 | xiaoO Hook Bridge | **移除** |
| `plugin.json` | xiaoO 插件注册 | **移除** |
| `config.py` xiaoO fallback | `~/.config/xiaoo/config.toml` 读取 | **移除** |
| `config.py` provider headers | OpenRouter/xAI 特殊 Headers | **保留**（通用 LLM Provider 适配） |
| `main.py :: POLICY_OUTPUT_DIR` | `/tmp/audit_policy_checker` | **改名** `/tmp/agent_moss` |
| `main.py :: DEFAULT_READONLY_POLICY` | 默认 Cerberus Policy | **保留** |

### 4.4 需要新增的模块

#### 4.4.1 `server/` — 服务层

**目标**：提供标准的服务间通信接口，使 agent_moss 作为常驻系统服务运行。

**技术选型建议**：

| 方案 | 优点 | 缺点 | 建议 |
|------|------|------|------|
| **Unix Domain Socket + JSON** | 零网络开销，简单可靠，与现有 JSON 兼容 | 不支持跨机调用 | **推荐**（同为 AgentOS 本地服务） |
| gRPC | 强类型，高性能，流式支持 | 引入 protobuf 编译链，较重 | 备选 |
| HTTP REST (FastAPI) | 开发快，生态好，易于调试 | 网络开销略大 | 备选 |

**推荐方案**：Unix Domain Socket + JSON（与"可观测"服务通信），同时可选暴露 HTTP REST 管理端点。

```python
# server/models.py — 请求/响应 Pydantic 模型

from pydantic import BaseModel, Field
from typing import Optional

class ActionItem(BaseModel):
    """单个动作描述"""
    action_type: str = Field(..., description="动作类型，如 bash, file_read, file_write 等")
    action_detail: str = Field(..., description="动作详情/命令内容")

class AnalyzeRequest(BaseModel):
    """安全分析请求"""
    session_id: str = Field(..., description="会话/Agent 唯一标识")
    prompt_session: str = Field(default="", description="用户/上游原始 prompt 或任务描述")
    action_history: list[ActionItem] = Field(default_factory=list, description="历史动作序列")
    a_next: ActionItem = Field(..., description="待执行的下一个动作")
    reason: str = Field(default="", description="执行该动作的理由")
    metadata: Optional[dict] = Field(default=None, description="扩展元数据（Agent ID, sandbox ID 等）")

class AnalyzeResponse(BaseModel):
    """安全分析响应"""
    decision: str = Field(..., description="Allow / Deny")
    reason: str = Field(..., description="决策原因")
    risk_level: str = Field(default="low", description="low / medium / high / critical")
    risk_type: str = Field(default="", description="风险类型")
    violated_layers: list[str] = Field(default_factory=list, description="触发的检测层")
    policy: str = Field(default="", description="Cerberus Policy TOML（Allow 时附带）")
    violated_policy: str = Field(default="", description="违反的策略条款（Deny 时附带）")
    confidence: int = Field(default=100, description="置信度 0-100")
    analysis_duration_ms: float = Field(default=0, description="分析耗时（毫秒）")
```

#### 4.4.2 `adapters/` — 适配器层

**目标**：将"可观测"服务的 syscall 数据格式转换为 agent_moss 内部格式。设计为可插拔的适配器模式，未来支持不同数据源。

```python
# adapters/base.py

from abc import ABC, abstractmethod
from server.models import AnalyzeRequest

class InputAdapter(ABC):
    """输入适配器基类"""
    
    @abstractmethod
    def adapt(self, raw_data: dict) -> AnalyzeRequest:
        """将原始数据转换为标准 AnalyzeRequest"""
        ...

# adapters/observable.py

class ObservableAdapter(InputAdapter):
    """"可观测"服务 syscall 数据适配器"""
    
    def adapt(self, raw_data: dict) -> AnalyzeRequest:
        """
        将"可观测"服务的 syscall 数据转换为标准 AnalyzeRequest。
        
        理论上与现有 audit_agent 输入格式保持一致，
        适配器主要负责字段映射和默认值填充。
        """
        # 字段映射：可观测服务格式 → AnalyzeRequest
        session_id = raw_data.get("session_id") or raw_data.get("agent_id", "unknown")
        
        # a_next 格式兼容
        a_next_raw = raw_data.get("a_next", {})
        a_next = ActionItem(
            action_type=a_next_raw.get("action_type", "unknown"),
            action_detail=a_next_raw.get("action_detail", ""),
        )
        
        # action_history 格式兼容
        history_raw = raw_data.get("action_history", [])
        action_history = [
            ActionItem(
                action_type=h.get("name", h.get("action_type", "")),
                action_detail=h.get("action_detail", ""),
            )
            for h in history_raw
        ]
        
        return AnalyzeRequest(
            session_id=session_id,
            prompt_session=raw_data.get("prompt_session", ""),
            action_history=action_history,
            a_next=a_next,
            reason=raw_data.get("reason", ""),
            metadata=raw_data.get("metadata"),
        )
```

### 4.5 保留不动的核心模块

以下模块**原则上不做逻辑修改**，仅做文件重命名/路径调整：

| 模块 | 说明 |
|------|------|
| `engine/heuristic.py` | 移除 xiaoO 特定正则后，其余完整保留 |
| `engine/logic_rules.py` | 移除 xiaoO-guardian 特定逻辑后保留 |
| `engine/llm_analyzer.py` | 完整保留，无需修改 |
| `engine/skill_engine.py` | 保留，扩展 Skill 目录路径 |
| `engine/script_analyzer.py` | 完整保留 |
| `engine/types.py` | 完整保留 |
| `infra/llm_client.py` | 保留，移除 xiaoO config fallback |
| `infra/parsers.py` | 完整保留 |
| `infra/policy_cache.py` | 完整保留 |
| `infra/prompt_templates.py` | 保留，模板内容做适配调整 |
| `skills/*.md` | 保留并可能扩展 |
| `rules/user_rules.json` | 保留 |
| `templates/*.txt` | 保留，内容做适配调整 |

---

## 5. 接口协议设计

### 5.1 与"可观测"服务的通信协议

#### 5.1.1 Unix Domain Socket (推荐)

```
Socket 路径: /var/run/agent_moss/agent_moss.sock
协议: JSON over Unix Domain Socket (Stream)
```

**请求格式**：
```json
{
    "session_id": "agent-sandbox-001",
    "prompt_session": "分析 /var/log/syslog 中的异常",
    "action_history": [
        {
            "name": "read_file",
            "action_detail": "cat /var/log/syslog"
        }
    ],
    "a_next": {
        "action_type": "bash",
        "action_detail": "rm -rf /var/log/*"
    },
    "reason": "清理旧日志文件",
    "metadata": {
        "agent_id": "agent-001",
        "sandbox_id": "sb-001",
        "trace_id": "trace-abc123"
    }
}
```

**响应格式**：
```json
{
    "decision": "Deny",
    "reason": "检测到递归强制删除命令，风险等级: critical",
    "risk_level": "critical",
    "risk_type": "script_execution",
    "violated_layers": ["1.1"],
    "policy": "",
    "violated_policy": "[script_execution] 检测到递归强制删除关键路径 (rm -rf /...)",
    "confidence": 95,
    "analysis_duration_ms": 1.2
}
```

#### 5.1.2 HTTP REST (备选)

```
Base URL: http://localhost:9090/api/v1
```

| 方法 | 路径 | 说明 |
|------|------|------|
| `POST` | `/v1/analyze` | 提交安全分析请求 |
| `GET` | `/v1/health` | 健康检查 |
| `GET` | `/v1/metrics` | Prometheus 指标 |
| `POST` | `/v1/reload` | 配置热加载（需认证） |

### 5.2 输入格式兼容性说明

agent_moss 的输入格式与 audit_agent **保持高度兼容**，核心字段不变：

| 字段 | 类型 | 必填 | 说明 | 兼容性 |
|------|------|------|------|--------|
| `session_id` | string | 是 | Agent 会话标识 | **完全兼容** |
| `prompt_session` | string | 否 | 原始任务描述 | **完全兼容** |
| `action_history` | array | 否 | 历史动作列表 | **完全兼容** |
| `a_next.action_type` | string | 是 | 动作类型 | **完全兼容** |
| `a_next.action_detail` | string | 是 | 动作详情 | **完全兼容** |
| `reason` | string | 否 | 执行理由 | **完全兼容** |
| `metadata` | object | 否 | 扩展元数据 | **新增字段**，不影响兼容 |

> **设计原则**：agent_moss 的输入格式是 audit_agent 输入格式的**严格超集**。所有现有字段保持不变，仅新增可选的 `metadata` 字段用于 AgentOS 扩展。现有使用 audit_agent 的调用方可以无缝迁移。

### 5.3 输出格式

输出格式在现有基础上增加分析耗时和触发层信息：

| 字段 | 类型 | 说明 | 变更 |
|------|------|------|------|
| `decision` | string | `Allow` / `Deny` | 不变 |
| `reason` | string | 决策原因 | 不变 |
| `risk_level` | string | `low` / `medium` / `high` / `critical` | 不变 |
| `risk_type` | string | 风险类型 | 不变 |
| `violated_layers` | list | 触发的检测层，如 `["1.1", "1.2"]` | 不变 |
| `policy` | string | Cerberus Policy TOML | 不变 |
| `violated_policy` | string | 违反条款 | 不变 |
| `confidence` | int | 置信度 0-100 | 不变 |
| `analysis_duration_ms` | float | **新增**：分析耗时（毫秒） | 新增 |

---

## 6. 配置体系设计

### 6.1 配置文件：`agent_moss.yaml`

将原有的 `audit_settings.json`（运行审计配置）和 `config.json`（LLM 配置）合并为一个 YAML 文件：

```yaml
# agent_moss.yaml — agent_moss 服务配置

# ========== 服务配置 ==========
server:
  # 运行模式: http (当前阶段) / socket (未来优化)
  mode: "http"
  http_host: "127.0.0.1"
  http_port: 9090
  # Unix Socket 路径（mode: socket 时使用）
  socket_path: "/var/run/agent_moss/agent_moss.sock"
  # 工作线程数
  workers: 4

# ========== 安全检测配置 ==========
security:
  enabled: true
  
  # 层1: 启发式检测
  heuristic:
    enabled: true
    rules_path: "/etc/agent_moss/rules/user_rules.json"
    
  # 层2: 逻辑规则检测
  logic_rules:
    enabled: true
    
  # 层3: LLM 深度分析
  llm_analysis:
    enabled: true
    # 可通过环境变量 AGENT_MOSS_DISABLE_LLM=1 禁用
    skills_dir: "/etc/agent_moss/skills"

# ========== LLM 配置 ==========
llm:
  provider: "openai"           # openai / openrouter / xai / deepseek / local
  model: "gpt-4o"
  api_key_env: "AGENT_MOSS_LLM_API_KEY"
  base_url: "https://api.openai.com/v1"
  temperature: 0.1
  max_tokens: 4096

# ========== 超时与重试 ==========
timeout:
  # 总最长耗时（含重试）—— AbortController 强制执行，
  # 超过此时间后所有 LLM 调用立即中断，返回 risk_type='analysis_timeout'
  total_timeout: 90
  # 单次 LLM 调用超时
  prompt1_timeout: 40
  # Policy 合规判定 LLM 调用超时（policy 生成默认关闭时不触发）
  prompt2_timeout: 20
  # Step1/Step2 之间的间隔（避免 API 限流）
  step_interval: 0

retry:
  max_retries: 2
  retry_interval: 2

# ========== 缓存配置 ==========
cache:
  enabled: true
  max_size: 1000
  ttl_secs: 3600

# ========== Policy 生成 ==========
policy:
  # 是否启用 LLM Policy 生成
  llm_generation_enabled: false
  # 默认只读 Policy
  default_policy: |
    landlock_optional = false
    mount_isolation_fallback = false
    [path_groups]
    system_binaries = true
    system_libraries = true
    temp_directories = true
    ...

# ========== 日志配置 ==========
logging:
  level: "INFO"                 # DEBUG / INFO / WARNING / ERROR
  dir: "/var/log/agent_moss"
  # LLM prompt 调试日志（生产环境建议关闭）
  llm_prompt_log_enabled: false

# ========== 可观测服务适配器配置 ==========
adapter:
  # 当前使用的适配器
  type: "observable"
  # 适配器特定配置
  observable:
    # 是否对输入做额外校验
    strict_validation: true
```

### 6.2 环境变量

| 环境变量 | 说明 | 默认值 |
|----------|------|--------|
| `AGENT_MOSS_CONFIG_PATH` | 配置文件路径 | `/etc/agent_moss/agent_moss.yaml` |
| `AGENT_MOSS_DISABLE_LLM` | 设为 `1` 禁用层3 LLM | 未设置 |
| `AGENT_MOSS_LLM_API_KEY` | LLM API Key | 未设置 |
| `AGENT_MOSS_LLM_TIMEOUT` | 层3 LLM 超时（秒） | `300` |
| `AGENT_MOSS_LOG_PATH` | LLM prompt 日志路径 | 未设置 |
| `AGENT_MOSS_ENABLE_POLICY_GEN` | 启用 Policy 生成 | 未设置 |

---

## 7. 迁移改造计划

### 7.1 总体策略

采用**渐进式重构**策略，分 4 个阶段完成：

```
Phase 1: 代码提取与重命名     (文件级操作，不改逻辑)
Phase 2: 模块重组与接口定义   (结构调整 + 新增模块)
Phase 3: 服务化改造           (添加 server 层 + 适配器)
Phase 4: 测试、文档、部署     (质量保障 + 上线)
```

### 7.2 Phase 1：代码提取与重命名（预计改动范围：命名 + 配置文件）

**目标**：将 audit_agent 核心代码复制到 AgentMoss 仓库，完成文件级重命名和 xiaoO 特定代码移除，保持功能不变。

**具体任务**：

1. 将 `audit_policy_checker/audit_policy_checker/` 下的源码复制到 `agent_moss/agent_moss/engine/`
2. 执行类名/函数名/文件名重命名（见 [4.2 节](#42-核心类函数重命名映射)）
3. 移除 xiaoO 特定代码（见 [4.3 节](#43-需要移除的-xiaoO-特定逻辑)）
4. 移除 `audit.py`、`plugin.json`、`install.sh` 等 xiaoO 集成文件
5. 创建新的 `config/agent_moss.yaml` 配置文件
6. 更新所有 import 路径
7. **确保所有现有单元测试可以通过**

**验证标准**：
```bash
cd agent_moss
python -m pytest tests/unit/ -v  # 所有单元测试通过
python -m agent_moss.cli --help   # CLI 可正常启动
```

### 7.3 Phase 2：模块重组与接口定义（预计改动范围：结构调整 + API 定义）

**目标**：重构模块组织结构，定义服务接口协议，设计适配器层。

**具体任务**：

1. 完成 [4.1 节](#41-目录结构) 的目录结构重组
2. 创建 `server/models.py`：定义 Pydantic 请求/响应模型
3. 创建 `adapters/base.py` + `adapters/observable.py`：定义适配器接口
4. 重构 `engine/analyzer.py`（原 `main.py`）：
   - 将 `audit_action()` 重命名为 `analyze()`
   - 入参改为 `AnalyzeRequest` Pydantic 模型
   - 出参改为 `AnalyzeResponse` Pydantic 模型
5. 将 `engine/coordinator.py` 中 `xiaoOSecBot` 重命名为 `AgentMossBot`
6. 更新所有模板和 Prompt 文本中的名称引用

**验证标准**：
```bash
# API 模型可正常序列化/反序列化
python -c "
from agent_moss.server.models import AnalyzeRequest, AnalyzeResponse
import json
req = AnalyzeRequest(
    session_id='test',
    a_next={'action_type': 'bash', 'action_detail': 'ls -la'}
)
print(req.model_dump_json())
"
```

### 7.4 Phase 3：服务化改造（预计改动范围：新增 server 层）

**目标**：添加服务层，使 agent_moss 作为常驻系统服务运行。

**具体任务**：

1. 创建 `server/app.py`：基于 FastAPI / aiohttp 的 HTTP 服务入口
2. 创建 `server/routes.py`：实现 `/v1/analyze`、`/v1/health`、`/v1/metrics` 路由
3. 实现 Unix Domain Socket 通信支持
4. 实现配置热加载（SIGHUP 信号处理）
5. 实现优雅关闭（SIGTERM 信号处理）
6. 创建 `scripts/agent_moss.service` systemd unit 文件
7. 创建 `scripts/install.sh` 安装脚本

**验证标准**：
```bash
# 启动服务
agent_moss server --config /etc/agent_moss/agent_moss.yaml

# 健康检查
curl -s http://localhost:9090/api/v1/health | jq .

# 发送分析请求
curl -s -X POST http://localhost:9090/api/v1/analyze \
  -H "Content-Type: application/json" \
  -d @tests/cases/TC-ALLOW-01.json | jq .
```

### 7.5 Phase 4：测试、文档、部署

**目标**：完善测试覆盖，编写文档，准备上线。

**具体任务**：

1. 补充集成测试：模拟"可观测"服务调用 agent_moss
2. 编写 API 文档（`docs/api.md`）
3. 编写部署文档（`docs/deployment.md`）
4. 性能基准测试：对比迁移前后分析耗时
5. 安全审计：确认 xiaoO 特定逻辑已完全移除
6. 在 AgentOS 测试环境中部署验证

---

## 8. 风险与缓解措施

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|----------|
| **重命名导致 import 路径断裂** | 功能不可用 | 高 | Phase 1 后立即运行全量单元测试 |
| **xiaoO 规则误删** | 通用规则被连带移除 | 中 | Code Review：逐条确认删除的规则确实为 xiaoO 专属 |
| **LLM Prompt 模板适配不完整** | 层3 LLM 判断质量下降 | 中 | Phase 2 后用人造用例做 A/B 对比测试 |
| **Unix Socket 权限问题** | 可观测服务无法连接 | 低 | 安装脚本设置正确的 socket 文件权限和 SELinux 上下文 |
| **服务常驻内存泄漏** | 长时间运行后 OOM | 低 | 添加 Prometheus metrics 监控内存，压测验证 |
| **"可观测"服务输入格式不兼容** | 安全分析失败 | 低 | 适配器层设计时就保持格式超集兼容；如有差异通过适配器映射解决 |
| **LLM 服务依赖可用性** | 层3 不可用导致误拦截 | 中 | 已有的 fail-closed + warn-allow 机制；层1+层2 可独立运行 |
| **Policy 生成不再适用 AgentOS 沙箱** | Policy 格式不兼容 | 低 | Step 2 默认关闭，Policy 格式保持 Cerberus TOML 标准 |

---

## 9. 附录

### 9.1 术语对照

| 术语 | 说明 |
|------|------|
| **AgentOS** | openEuler 的 AI Agent 发行版 |
| **xiaoO** | AgentOS 中权限最高的 Agent Manager，负责 Agent 生命周期管理 |
| **可观测服务 (Observability)** | AgentOS 系统服务，收集 Agent syscall 数据 |
| **agent_moss** | AgentOS 安全模块（原 audit_agent） |
| **Cerberus** | xiaoO 的沙箱策略引擎 |
| **三层防御** | 启发式检测 → 逻辑规则检测 → LLM+Skill 深度分析 |
| **Skill** | 用 Markdown 编写的安全检测规则，注入 LLM Prompt |
| **Policy** | Cerberus 沙箱策略（TOML 格式），定义 Agent 的权限边界 |

### 9.2 参考文件路径

> 下表为 AgentMoss 服务仓库（`~/gitcode/AgentMoss/`）内的对应文件路径。旧 audit_agent 源码已随 xiaoO 移除 audit_agent 一并删除，判定逻辑迁至 AgentMoss 仓库。

| 文件 | 路径（AgentMoss 仓库内） |
|------|------|
| CLI 主入口 | `agent_moss/cli.py` |
| 分析入口 | `agent_moss/engine/analyzer.py` |
| 三层防御协调器 | `agent_moss/engine/coordinator.py` |
| 类型定义 | `agent_moss/engine/types.py` |
| 启发式检测 | `agent_moss/engine/heuristic.py` |
| 逻辑规则检测 | `agent_moss/engine/logic_rules.py` |
| LLM 分析器 | `agent_moss/engine/llm_analyzer.py` |
| 服务 README | `~/gitcode/AgentMoss/README.md` |
| xiaoO Hook Bridge | `plugins/hookers/agent_moss/bridge.py`（xiaoO 仓库内，本插件目录） |

### 9.3 待确认事项（已结合 openEuler 独立部署约束给出建议方案）

| # | 事项 | 建议方案 | 理由 |
|---|------|----------|------|
| 1 | **通信协议选型** | **HTTP REST (FastAPI)**，后续可叠加 Unix Socket | openEuler 上无 AgentOS 框架，HTTP 最易测试和集成；Unix Socket 作为后续优化方案 |
| 2 | **Policy 生成（Step 2）** | **保留为可选**，默认关闭 | 引擎代码完整保留，未来沙箱需要时开启即可；当前默认使用内置最小权限 Policy |
| 3 | **输入格式** | **保持与 audit_agent 一致** | 向后兼容，降低迁移成本；新增 `metadata` 字段为可选扩展 |
| 4 | **配置文件格式** | **YAML** | 可读性好，openEuler 系统服务常用格式 |
| 5 | **部署路径** | `/etc/agent_moss/`、`/var/run/agent_moss/`、`/var/log/agent_moss/` | 遵循 FHS 标准，符合 openEuler 系统服务规范 |
| 6 | **API 认证** | **当前阶段不需要**，预留 middleware 接口 | 本地服务间通信无需认证；API 设计上预留认证中间件接口，后续可直接插入 |
| 7 | **LLM Provider** | **沿用 OpenAI SDK 兼容接口**，配置文件指定 provider | 保持与现有代码最大兼容；支持 openai / openrouter / deepseek 等多 provider |

---

> **文档状态**：待确认  
> **下一步**：请审阅本文档，确认设计方案后进入 Phase 1 开发。