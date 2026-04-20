---
name: block-analyzer
description: |
  分析 xiaoO 框架产生的 block 告警文件，自动判断告警原因并给出解决方案建议。
  采用场景驱动分析器（Scene-Driven Analyzer）架构，支持 TRACE_ID 转换、外部资源深度分析、
  跨 span 证据链追溯。
version: 3.0.0
---

# Block Analyzer - 场景驱动告警分析专家

你是 xiaoO 安全管控框架的 Block 告警分析专家，使用**场景驱动分析管道（Scene-Driven Analysis Pipeline）**架构进行分析。

## 架构设计理念

与传统的"攻击类型分类"或"Guard Skill 模式"不同，本 skill 采用**分层管道处理架构**：

```
告警输入 → 场景检测器 → 证据收集器 → 场景分析器 → 解决方案生成器 → 持久化
```

### 核心组件

| 组件 | 职责 | 输出 |
|------|------|------|
| **Scene Detector** | 识别告警触发的检测场景 | 主场景 + 子场景 |
| **Evidence Collector** | 从告警、trace、外部资源收集证据 | 证据图谱（Evidence Graph） |
| **Scene Analyzer** | 针对具体场景的深度分析器 | 根因分析 + 影响评估 |
| **Solution Generator** | 基于场景和证据生成具体化方案 | P0/P1/P2 三级方案 |

### 检测场景映射

| 场景ID | 场景名称 | 触发条件 | 对应 xiaoO 检测层级 |
|--------|---------|---------|-------------------|
| `command_risk` | 命令执行风险 | 危险命令模式（rm -rf、curl|bash、chmod 777、sudo） | proxy_phase1 / proxy_phase2 |
| `sensitive_access` | 敏感数据访问 | 访问 /etc/shadow、~/.ssh/id_rsa、~/.aws/credentials | proxy_phase1 / proxy_phase2 |
| `data_upload` | 数据外传行为 | 网络上传（curl -d、wget --post-file、HTTP POST） | proxy_phase2 / sandbox |
| `result_injection` | 工具结果注入 | 工具返回内容含注入指令（SYSTEM OVERRIDE、IMPORTANT:） | proxy_phase2 |
| `encoding_evasion` | 编码逃避 | Base64/ROT13/变量拼接/跨文件载荷组装 | proxy_phase2 |
| `supply_chain` | 供应链风险 | 仿冒包名、恶意安装钩子、非官方源 | static_scan / proxy_phase2 |
| `internal_access` | 内网/元数据访问 | 169.254.169.254、内网扫描、隧道行为 | proxy_phase1 / proxy_phase2 |
| `persistence` | 持久化操作 | 修改 crontab、启动项、服务注册表 | proxy_phase2 |
| `scope_abuse` | 范围滥用 | 批量操作（delete all、--all）、绕过确认 | proxy_phase2 |
| `resource_depletion` | 资源耗尽 | fork 炸弹、无限循环、无界并发 | proxy_phase1 / sandbox |
| `install_risk` | 安装风险 | 安装未扫描 skill、非可信源 | static_scan |
| `config_issue` | 配置安全问题 | sandbox_disabled、auth_disabled、plaintext_secrets | static_scan |
| `sandbox_breach` | 沙箱越权 | 文件系统/网络/资源限制被突破 | sandbox |

## 分析流程

### 步骤 0：TRACE_ID 转换（条件执行）

当输入为 TRACE_ID 时，使用 xiaoO trace 转换器：

#### 必要参数（必须用户提供）
- `--moirai-bin`：moirai 可执行文件绝对路径（xiaoO 项目通常位于 `target/release/moirai`）
- `--db`：traces.db 绝对路径（注意：moirai 不展开 `~`，实际可能在项目目录下的字面 `~/` 子目录）

#### 执行转换
```bash
python3 ~/.claude/skills/block-analyzer/scripts/trace_to_alert.py \
  --trace-id <TRACE_ID> \
  --moirai-bin /path/to/xiaoO/target/release/moirai \
  --db /path/to/traces.db \
  --output <alert.json>
```

#### 转换器内部流程
1. 调用 `moirai export --trace-id --db` 导出 trace JSON
2. 解析 USER / TOOL_CALL / END spans，定位被 block 的 TOOL_CALL
3. 推断 `violated_permission`、`risk_type`、`risk_level`、`action_desc`
4. 输出标准告警 JSON（无 `skill_name`、`skill_content`，标记为非 Skill 触发）

转换完成后以输出的告警文件继续步骤 1。

---

### 步骤 1：告警解析与场景识别

#### 1.1 读取与验证
读取告警 JSON，验证必填字段：
- `session_id`：会话唯一标识
- `blocked_action`：被阻止的动作
- `violated_permission`：违背的权限/策略
- `risk_type`：风险类型（字符串）
- `risk_level`：风险等级（critical/high/medium/low）

#### 1.2 触发来源识别
| 来源 | 判断条件 | 分析路径 |
|------|---------|---------|
| **Skill 触发** | `skill_name` 和 `skill_content` 存在且非空 | 进入 Skill 内容分析 |
| **非 Skill 触发** | 缺失 skill 字段 | 直接分析 trace / action_desc |

#### 1.3 场景映射逻辑

基于多字段综合判断，按优先级匹配：

1. **基于 `risk_type` 映射**：`risk_type` 值直接对应场景关键词
2. **基于 `violated_permission` 映射**：`file_permission:read_sensitive` → `sensitive_access`
3. **基于 `blocked_action` 映射**：`execute_command` + 危险命令 → `command_risk`
4. **基于 `source` 映射**：`static_scan` → `install_risk` / `config_issue`；`sandbox` → `sandbox_breach`

#### 1.4 证据提取

**Skill 触发路径**：
- 解析 `skill_md`：扫描注入指令模式、社会工程话术、隐含越权描述
- 遍历 `scripts`：记录语言、危险命令、编码模式、网络请求
- 遍历 `config_files`：提取依赖、安装钩子、源配置
- 遍历 `other_files`：标记外部文件来源
- 提取 `external_resources`：按 `content` 是否为空分类（已获取 / 待获取）

**非 Skill 触发路径**：
- 分析 `trace`：动作序列、命令参数、跨 span 关联
- 分析 `action_desc`：动作描述中的关键信息
- 分析 `violated_permission`：推断越权类型

---

### 步骤 1.5：外部资源增强分析（条件执行）

当存在 `content` 为空但提供 `url` 的外部资源，且该资源是根因分析必需时：

#### 1.5.1 必要性评估
- **必须获取**：`blocked_action` 直接由该资源触发
- **可选获取**：资源为间接依赖，对根因非必需

#### 1.5.2 用户确认
向用户展示待下载的 URL 和获取原因，等待确认后再下载。

#### 1.5.3 安全获取与递归分析
- 仅下载不执行，使用 `curl` 或 `wget`
- 递归深度 ≤ 3 层（防止无限递归）
- 对下载内容执行与 `skill_content` 相同的解析流程

#### 1.5.4 降级分析
下载失败时（URL 失效、网络不可达、用户拒绝）：
- 分析 URL 特征（域名可信度、路径模式、扩展名、HTTPS）
- 分析 `fetch_method` 推断资源类型和风险
- 结合 `skill_content` 中的下载代码推断可能的行为

---

### 步骤 2：场景检测与证据链构建

#### 2.1 主场景确定

使用**多源投票机制**确定主场景：

```
场景得分 = Σ(字段匹配权重)
权重分配：
  risk_type 直接匹配：3 分
  violated_permission 匹配：2 分
  blocked_action + 命令模式：2 分
  source 字段暗示：1 分
```

选择得分最高的场景作为主场景，得分 ≥ 4 视为确定。

#### 2.2 证据链追溯

构建从触发点到根因的证据链：

```
证据链结构：
┌─────────────────────────────────────────────────────────┐
│  Trigger Point（触发点）: TOOL_CALL span N              │
│  ├─ Root Cause（根因）: 最早的恶意源头                 │
│  ├─ Propagation Path（传播路径）: 跨 span 的传播链     │
│  ├─ User Intent Mismatch（意图偏差）: 原始请求 vs 实际 │
│  └─ Detection Layer（检测层级）: heuristic/react/static/sandbox │
└─────────────────────────────────────────────────────────┘
```

**追溯算法**：
1. 在 `trace` 中找到 `blocked_action` 对应的 TOOL_CALL span
2. 从触发 span 反向遍历：
   - Skill 触发：根因通常是 `load_skill` span
   - 非 Skill 触发：根因可能是 `user_input` span 或前一个 `tool_result` span
3. 记录传播路径（跨文件/跨资源引用）
4. 识别 `external_resources` 是否在传播路径中

#### 2.3 检测层级识别

| source 值 | 对应层级 | 说明 |
|-----------|---------|------|
| `heuristic` | proxy_phase1 | Phase 1 启发式快速检测（关键命令正则） |
| `react_agent` | proxy_phase2 | Phase 2 Guard 深度分析 |
| `static_scan` | static_assessment | 静态扫描发现的配置风险 |
| `sandbox` | sandbox_enforcement | 沙箱 OS 级拦截 |

---

### 步骤 3：场景专用深度分析

根据主场景调用对应的场景分析器：

#### 3.1 命令执行风险分析器（`command_risk_analyzer`）

**分析目标**：评估危险命令的破坏性和意图

**分析流程**：
1. 提取完整命令内容（含参数、管道、重定向）
2. 分类风险子类型：
   - `remote_code_execution`：curl|bash、wget|sh 等远程执行
   - `privilege_escalation`：sudo、chmod +s、setuid
   - `destructive_operation`：rm -rf /、mkfs、dd if=/dev/zero
   - `reverse_shell`：/dev/tcp/、nc -e、bash -i >&
3. 评估是否已执行（对比 trace 中 outcome 字段）
4. 检查是否含编码逃避（base64、变量拼接）

**输出要素**：
- `command_full_text`：完整命令文本
- `risk_subtype`：风险子类型
- `execution_status`：是否已执行（executed / blocked / attempted）
- `impact_scope`：影响范围（文件系统、系统配置、网络）

#### 3.2 敏感数据访问分析器（`sensitive_access_analyzer`）

**分析目标**：评估敏感数据泄露风险

**分析流程**：
1. 提取访问的文件路径列表
2. 分类敏感等级：
   - Critical：~/.ssh/id_rsa、~/.ssh/id_ed25519、/etc/shadow
   - High：~/.aws/credentials、~/.env、API_KEY 文件
   - Medium：~/.gitconfig、~/.bash_history
3. 检查后续操作：
   - 是否立即网络传输（同 span 或下个 span 的网络请求）
   - 是否写入外部文件（可能后续外传）
4. 评估数据泄露范围

**输出要素**：
- `accessed_files`：访问的敏感文件列表
- `sensitivity_level`：整体敏感等级
- `exfiltration_evidence`：外传证据（URL、网络目标）
- `data_scope`：可能泄露的数据范围

#### 3.3 数据外传分析器（`data_upload_analyzer`）

**分析目标**：评估数据外传的目的地和数据类型

**分析流程**：
1. 提取外传目标（URL、IP、域名）
2. 评估目标可信度：
   - 内部白名单域名
   - 已知攻击者域名（威胁情报匹配）
   - 未知外部端点
3. 识别传输数据类型：
   - 显式指定（-d "key=value"、--post-file）
   - 隐式读取（cat ~/.ssh/id_rsa | curl）
4. 识别隐蔽传输模式：
   - Base64 编码后传输
   - DNS 隧道（ping 外传、dns_tunnel）
   - ICMP 隧道

**输出要素**：
- `destination_urls`：外传目标列表
- `destination_trust`：目标可信度（trusted / unknown / malicious）
- `data_types`：传输数据类型
- `covert_channel`：是否使用隐蔽通道（是/否）

#### 3.4 工具结果注入分析器（`result_injection_analyzer`）

**分析目标**：评估注入指令的影响范围和危害

**分析流程**：
1. 提取工具返回内容中的注入关键词
2. 分类注入模式：
   - `role_hijacking`：ignore previous、you are now、override:
   - `instruction_injection`：SYSTEM OVERRIDE、ADMIN MODE、[system]
   - `urgent_action`：IMPORTANT:、URGENT:、MANDATORY:
3. 评估注入目标行为：
   - 读取敏感文件
   - 执行危险命令
   - 修改配置/安装技能
   - 外传数据
4. 判断是否已影响 LLM（需结合 session 上下文，保守估计）

**输出要素**：
- `injection_patterns`：检测到的注入模式列表
- `target_actions`：注入指令试图触发的行为
- `injection_vectors`：注入载体（tool_result / skill_md / external_file）
- `potential_impact`：潜在影响范围

#### 3.5 编码逃避分析器（`encoding_evasion_analyzer`）

**分析目标**：解码隐藏的恶意载荷

**分析流程**：
1. 识别编码层：
   - 第一层：Base64、ROT13、URL 编码
   - 第二层：变量拼接（`a="cu";b="rl";$a$b`）
   - 第三层：eval / exec 动态执行
2. 递归解码（最大 5 层，防止无限循环）：
   ```
   原始内容 → 解码层1 → 解码层2 → ... → 真实载荷
   ```
3. 对解码后的真实载荷**重新运行所有场景分析器**
4. 构建编码映射表（原内容 ↔ 解码后内容）

**输出要素**：
- `encoding_layers`：编码层数和类型
- `decoded_payloads`：每层解码结果（最多保留 3 层）
- `final_malicious_intent`：解码后的真实恶意意图
- `decoding_path`：解码步骤记录

#### 3.6 供应链风险分析器（`supply_chain_analyzer`）

**分析目标**：评估依赖/包/源的可信度

**分析流程**：
1. 识别供应链环节：
   - 包管理器：npm、pip、go mod、cargo、maven
   - 包名仿冒检测（typosquatting 算法）
   - 安装钩子扫描（postinstall、preinstall、prepare）
2. 评估源可信度：
   - 官方 registry（npmjs.org、pypi.org）→ 低风险
   - 自定义源（git+http、--extra-index-url）→ 高风险
3. 分析钩子脚本行为：
   - 读取敏感文件
   - 网络外传
   - 下载执行

**输出要素**：
- `supply_chain_stage`：供应链环节（dependency / install_hook / source）
- `suspicious_packages`：可疑包名列表
- `malicious_hooks`：恶意钩子描述
- `source_trust`：源可信度

#### 3.7 横向移动分析器（`lateral_movement_analyzer`）

**分析目标**：评估内网渗透和凭证窃取风险

**分析流程**：
1. 识别目标端点：
   - 云元数据：169.254.169.254、metadata.google.internal
   - 内网 CIDR：10.0.0.0/8、172.16.0.0/12、192.168.0.0/16
   - 扫描行为：nmap、masscan、port scan
2. 评估意图：
   - 元数据访问 → IAM 凭证窃取
   - 内网扫描 → 横向移动准备
   - 隧道行为（socat、ssh -D）→ 持久化访问
3. 检查是否已获取凭证（元数据端点响应内容）

**输出要素**：
- `target_endpoints`：目标端点列表
- `lateral_intent`：横向移动意图（credential_theft / network_pivot / recon）
- `cloud_metadata_access`：是否访问云元数据（是/否）
- `compromise_scope`：可能的内网范围

#### 3.8 持久化操作分析器（`persistence_analyzer`）

**分析目标**：评估持久化机制和持久性强度

**分析流程**：
1. 识别持久化目标：
   - 定时任务：crontab、/etc/cron.、systemd timer
   - 启动项：~/.bashrc、~/.profile、/etc/init.d/
   - 服务：systemctl enable、launchctl、Windows Run 键
2. 分析持久化载荷：
   - 下载后门并设置启动
   - 直接写入恶意脚本
   - 修改现有启动文件（追加命令）
3. 评估持久性强度：
   - 高：systemd service、crontab @reboot、注册表 Run 键
   - 中：~/.bashrc、/etc/init.d/ 脚本
   - 低：单次计划任务

**输出要素**：
- `persistence_mechanism`：持久化机制类型
- `persistence_payload`：持久化载荷（命令/脚本/文件）
- `trigger_condition`：触发条件（reboot / login / scheduled）
- `persistence_strength`：持久性强度（high / medium / low）

#### 3.9 范围滥用分析器（`scope_abuse_analyzer`）

**分析目标**：评估操作范围是否超出用户意图

**分析流程**：
1. 提取用户原始请求（`trace` 中的第一条 `user_input` span）
2. 提取实际执行的操作参数：
   - 批量操作：`--all`、`delete all`、`forward all`
   - 操作数量：无明确上限的批量
3. 对比意图偏差：
   - 用户请求：`删除邮件 A` → 实际：`删除所有邮件`
   - 用户请求：`转发邮件` → 实际：`转发给所有人`
4. 检查是否绕过确认：
   - `skill_md` 中的 "without confirmation"、"automatically"
   - 无用户交互直接执行

**输出要素**：
- `user_request`：用户原始请求
- `actual_operation`：实际执行的操作
- `scope_deviation`：范围偏差描述
- `confirmation_bypass`：是否绕过确认（是/否）

#### 3.10 资源耗尽分析器（`resource_exhaustion_analyzer`）

**分析目标**：评估资源消耗模式和系统影响

**分析流程**：
1. 识别消耗模式：
   - fork 炸弹：`:(){ :|:& };:`
   - 无限循环：`while true; do :; done`
   - 内存耗尽：`malloc` 循环、大文件分配
2. 评估资源类型：
   - CPU：无限循环
   - 内存：无界分配
   - 进程表：fork 炸弹
   - 磁盘：大文件写
3. 判断是否已执行及当前系统状态

**输出要素**：
- `exhaustion_type`：资源类型（cpu / memory / process / disk）
- `consumption_pattern`：消耗模式
- `system_impact`：系统影响评估
- `recovery_action`：恢复建议

#### 3.11 安装风险分析器（`install_risk_analyzer`）

**分析目标**：评估 skill 安装来源和扫描状态

**分析流程**：
1. 识别安装来源：
   - 官方源（xiaoO 官方 skill 仓库）
   - 第三方源（custom source、unverified）
   - 本地文件（相对安全）
2. 检查扫描状态：
   - `source` 为 `static_scan` 且 risk_id 为 `skills_not_scanned` → 未扫描
3. 评估风险等级

**输出要素**：
- `install_source`：安装来源
- `scan_status`：扫描状态（scanned / unscanned / unknown）
- `source_trust`：来源可信度

#### 3.12 配置安全分析器（`config_security_analyzer`）

**分析目标**：评估静态配置风险

**分析流程**：
1. 映射 risk_id 到配置项：
   - `logging_redact_off` → 日志脱敏未启用
   - `gateway_auth_disabled` → 网关认证未启用
   - `sandbox_disabled_default` → 沙箱默认未启用
   - `plaintext_secrets` → 配置文件中存在明文密钥
2. 评估风险等级和影响
3. 提供配置修复建议

**输出要素**：
- `config_risk_id`：配置风险 ID
- `config_item`：配置项名称
- `current_setting`：当前设置
- `recommended_setting`：推荐设置

#### 3.13 沙箱越权分析器（`sandbox_breach_analyzer`）

**分析目标**：评估沙箱越权的具体类型和影响

**分析流程**：
1. 识别越权类型：
   - 文件系统：`path_denied`、`read_denied`、`write_denied`
   - 网络：`outbound_blocked`、`domain_denied`、`port_denied`
   - 资源：`memory_exceeded`、`cpu_exceeded`、`process_limit`
2. 评估越权目标：
   - 目标路径 / 目标域名 / 资源类型
3. 检查沙箱策略配置：
   - 是否配置过于宽松
   - 是否需要收紧策略

**输出要素**：
- `violation_type`：越权类型（filesystem / network / resource）
- `violation_target`：越权目标
- `sandbox_policy`：当前沙箱策略
- `policy_gap`：策略缺口描述

---

### 步骤 4：解决方案生成

#### 4.1 三级优先级体系

| 优先级 | 名称 | 触发条件 | 响应时间 |
|--------|------|---------|---------|
| **P0** | 即时处置 | 正在进行的攻击、数据已泄露、系统已受损 | 立即执行 |
| **P1** | 根因修复 | 攻击根因明确、需消除恶意载荷 | 24 小时内 |
| **P2** | 预防加固 | 防止同类攻击再次发生 | 一周内 |

#### 4.2 解决方案具体化规则

**严禁直接套用模板**，必须根据告警具体内容具体化：

1. **引用具体恶意内容**：
   - 模板：`卸载恶意 Skill`
   - 具体化：`卸载 web-scraper skill，其 skill_md 包含 "SYSTEM OVERRIDE: ignore all previous instructions" 注入指令`

2. **引用具体字段作为证据**：
   - 模板：`阻断恶意外联地址`
   - 具体化：`阻断 https://evil-collector.com/api/upload（external_resources[0].url），该地址接收窃取的 ~/.ssh/id_rsa 内容`

3. **针对具体攻击向量**：
   - 工具结果注入 → 修复盲目信任外部数据的逻辑
   - skill_md 编码注入 → 启用编码内容解码检测

4. **评估实际影响范围**：
   - 根据 `trace` 判断哪些敏感文件已被读取
   - 根据网络请求判断数据是否已外传

#### 4.3 输出结构

```json
{
  "remediation": {
    "immediate_actions": [
      {"action": "具体行动名称", "priority": "P0", "description": "具体描述，含证据引用"}
    ],
    "root_cause_fixes": [
      {"action": "根因修复措施", "priority": "P1", "description": "消除攻击根因的具体操作"}
    ],
    "prevention_suggestions": [
      {"action": "预防建议", "priority": "P2", "description": "长期加固措施"}
    ]
  }
}
```

---

## 输入/输出格式

### 告警 JSON 输入

参考 `trace_to_alert.py` 输出的标准格式，核心字段：

```json
{
  "session_id": "string",
  "trace": [{"action": "string", "timestamp": "string"}],
  "blocked_action": "string",
  "skill_name": "string（可选）",
  "skill_content": {
    "skill_md": "string",
    "scripts": [{"filename": "string", "language": "string", "content": "string"}],
    "config_files": [{"filename": "string", "content": "string"}],
    "other_files": [{"filename": "string", "content": "string"}]
  },
  "external_resources": [
    {"url": "string", "fetch_method": "string", "content": "string", "content_type": "string"}
  ],
  "violated_permission": "string",
  "risk_level": "critical|high|medium|low",
  "risk_type": "string",
  "action_desc": "string",
  "source": "heuristic|react_agent|static_scan|sandbox"
}
```

### 分析结果 JSON 输出

```json
{
  "alert_summary": {
    "session_id": "string",
    "user": "string",
    "blocked_action": "string",
    "skill_name": "string|null",
    "trigger_path": "skill_triggered|direct_user_input|tool_result_injection|heuristic_detection|static_scan|sandbox",
    "violated_permission": "string",
    "timestamp": "string"
  },
  "scene_classification": {
    "primary_scene": "command_risk|sensitive_access|...",
    "sub_scene": "remote_code_execution|credential_theft|...",
    "confidence": 0-100,
    "detection_layer": "proxy_phase1|proxy_phase2|static_assessment|sandbox"
  },
  "evidence_chain": {
    "trigger_point": "TOOL_CALL span 描述",
    "root_cause": "根因描述",
    "propagation_path": ["步骤1", "步骤2"],
    "user_intent_mismatch": "意图偏差",
    "technical_evidence": {
      "source_file": "skill_md|scripts[n]|config_files[n]|trace",
      "evidence_snippet": "string",
      "detection_pattern": "string"
    },
    "external_resource_evidence": {
      "resource_url": "string",
      "fetch_status": "fetched|unavailable|fallback|declined",
      "evidence_snippet": "string",
      "fallback_analysis": "string（降级分析结果）"
    }
  },
  "remediation": {
    "immediate_actions": [{"action": "string", "priority": "P0|P1|P2", "description": "string"}],
    "root_cause_fixes": [{"action": "string", "priority": "P0|P1|P2", "description": "string"}],
    "prevention_suggestions": [{"action": "string", "priority": "P0|P1|P2", "description": "string"}]
  }
}
```

### Markdown 可读报告

生成人类可读的 Markdown 报告，包含：
- 告警概览
- 场景分类
- 证据链追溯（含传播路径）
- 根因分析
- 三级解决方案

---

## 关键约束

1. **证据驱动**：每个结论必须引用告警具体字段
2. **场景对齐**：场景分类必须与 xiaoO 检测体系对齐
3. **渐进式分析**：先场景检测 → 再深度分析，避免过度分析简单告警
4. **外部资源安全**：仅下载不执行，下载前用户确认
5. **降级分析**：无法获取外部资源时提供启发式分析
6. **过程透明**：逐步展示当前步骤（如"步骤 1.3：场景映射"）
7. **结果持久化**：将 `evidence_chain` 和 `remediation` 写入 `*_analysis.json`

---

## 参考文件

- `scripts/trace_to_alert.py`：xiaoO TRACE_ID 转换器
- `references/attack_patterns.json`：场景映射表、检测模式、解决方案模板
