# AgentMoss 安全拦截规则说明书

本文档说明 AgentMoss（agent_moss 安全审计服务）实现的安全拦截规则，供 xiaoO 客户参考。
AgentMoss 判定逻辑迁出为独立常驻 HTTP 服务，规则定义见本目录下的 `skills/`、`rules/`、`templates/`。

---

## 三层防御体系

AgentMoss 对每个待执行动作 `a_next`（含 `action_type` + `action_detail`）依次过三层，命中即决策。判断依据来自**当前操作系统 Profile**（Linux/Windows 自动识别，可强制）和**用户运行时配置**（runtime_config，Policy Console 可热改）。

| 层 | 名称 | 机制 | 命中后 |
|----|------|------|--------|
| L1 | 启发式静态检测 | 正则匹配危险命令 + 注入关键词 + 用户敏感规则 + 敏感路径/危险模式/横向移动（**只看 a_next 本身**） | critical/high → Deny（短路） |
| L2 | 逻辑规则检测 | read_before_write、意图一致性、间接文件访问、密码/用户删除授权、提权（**需要 action_history / prompt_session / cwd**） | critical/high → Deny（短路） |
| L3 | LLM + Skill 深度分析 | 13 个安全 Skill 规则（`skills/*_guard.md`）+ LLM 语义判断 | 按风险等级决策 |

### 风险等级与拦截策略

每条规则标注风险等级，不同等级对应不同处理：

| 风险等级 | L1/L2 处理 | L3 处理 |
|---------|-----------|---------|
| **critical** | 立即 Deny（短路） | — |
| **high** | 立即 Deny（短路） | — |
| **medium** | 不拦截，传 L3 | 作为提示注入 LLM prompt，由 LLM 决定 |
| **low** | 不拦截，传 L3 | 作为提示注入 LLM prompt，由 LLM 决定 |

> **fail-closed 原则**：AgentMoss 服务不可达或 L3 LLM 分析失败时，按配置的 `fail_mode` 决策——默认 `fail_open`（前两层未拦截时 Allow + 警告，可用性优先），可配 `fail_closed`（Deny，安全优先）。L3 失败 + 前两层已拦截时无论 fail_mode 都 Deny。

---

## L1：启发式静态检测

L1 由 `HeuristicDetector`（`agent_moss/engine/heuristic.py`）协调，按优先级依次调用子检测器，**短路返回**（命中即返回，不再跑后续）。所有子检测器**只看 a_next 本身**（action_type + action_detail），不看历史动作，这是 L1 与 L2 的分界。

子检测器执行顺序：

| # | 子检测器 | 检测内容 | 风险等级 |
|---|---------|---------|---------|
| 1 | 安全工具白名单跳过 | action_type 精确匹配 profile 的 `safe_action_types`（glob/list_dir 等纯元数据工具）→ 直接放行 | — |
| 2 | 环境变量自定义规则 | `AGENT_MOSS_CUSTOM_RULES` 用户正则，命中 Deny | high |
| 3 | 用户敏感规则 | `rules/user_rules.json` 通配符敏感动作 + 精确敏感工具名 | high |
| 4 | 危险命令正则 | profile 的 `dangerous_commands`，正则 `re.search`（大小写不敏感） | critical/high |
| 5 | Prompt 注入关键词 | profile 的 `injection_keywords`，子串匹配（中英文） | critical/high |
| 6 | 敏感路径访问 | profile 的 `sensitive_paths`，按 `deny_mode` 区分读写、边界匹配 | critical/high |
| 7 | 危险操作模式 | 通配符+删除、重定向覆盖 `/etc`/`/boot`/`/proc` 等关键目录 | high/critical |
| 8 | 横向移动 | ssh/scp/rsync 到非本机主机 | medium（传 L3） |

> 注：敏感路径/危险模式/横向移动三类已从 L2 重分类到 L1（只看 a_next 本身，不需历史上下文）。

### L1：用户敏感规则（`rules/user_rules.json`）

用户可配置的敏感动作与敏感工具白名单，命中即触发 L1 拦截：

### 敏感动作（sensitive_actions）

| 模式 | 说明 |
|------|------|
| 写入系统文件 | 写入 /etc, /usr, /boot 等系统目录 |
| 发送外部邮件 | 通过 email 工具发邮件到外部 |
| 危险 shell 命令 | rm -rf, chmod 777, sudo 等 |
| 修改全局配置 | 修改系统级配置文件或环境变量 |
| 支付操作 | 涉及支付、转账等金融操作 |
| 访问密钥文件 | 访问 .ssh/id_rsa, .env, credentials |
| 网络数据外传 | 通过 HTTP/wget/curl 等发送数据到外部 |
| `curl * -d *` | curl 发送数据（可能数据外传） |
| `curl * POST *` | curl POST 发送数据 |
| `wget *--post-data*` | wget POST 发送数据 |
| `crontab -e` | 编辑定时任务（持久化后门） |
| `crontab -r` | 删除定时任务（清除审计痕迹） |
| `* \| crontab` | 管道方式改定时任务（持久化后门） |
| `iptables -` | 改防火墙规则（关安全防护） |
| `ssh * -o StrictHostKeyChecking=no` | SSH 跳过主机密钥验证（横向移动） |
| `base64 -d * bash` | Base64 解码后执行（编码绕过） |
| `curl * \| bash` | 下载远程脚本并执行（RCE） |

### 敏感工具（sensitive_tools）

- `eval`、`exec`、`system`

完整定义见 [`rules/user_rules.json`](rules/user_rules.json)。

---

## L2：逻辑规则检测

基于上下文和行为链的逻辑分析，超越简单模式匹配（需要 action_history / prompt_session / cwd 之一或多个）：

| 规则 | 说明 |
|------|------|
| read_before_write | 写已存在文件前必须先读 |
| intent_consistency | 动作与 prompt 意图一致性 |
| indirect_file_access | 内联脚本动态遍历目录读凭据（listdir/glob 等 + 文件读写） |
| 密码修改授权 | 非交互式密码修改（passwd/chpasswd 等需确认） |
| 用户/组删除授权 | userdel/groupdel 等需确认 |
| privilege_escalation | sudo/su 提权检测（sudo 后跟包管理/服务管理命令直接放行，非运维命令查历史上下文） |

> 注：敏感路径/危险模式/横向移动三类已从 L2 重分类到 L1（只看 a_next 本身，不需历史上下文）。

---

## L3：安全 Skill 规则（`skills/*_guard.md`）

13 个安全 Skill，每个针对一类风险，由 LLM 按规则做深度语义判断：

| Skill | 检测内容 | 文档 |
|-------|---------|------|
| script_execution_guard | 命令/脚本执行（rm -rf、sudo、curl\|bash、反弹 Shell） | [skills/script_execution_guard.md](skills/script_execution_guard.md) |
| file_access_guard | 敏感文件访问与路径滥用 | [skills/file_access_guard.md](skills/file_access_guard.md) |
| indirect_access_guard | 间接文件访问（目录遍历绕过） | [skills/indirect_access_guard.md](skills/indirect_access_guard.md) |
| data_exfiltration_guard | 数据外传 | [skills/data_exfiltration_guard.md](skills/data_exfiltration_guard.md) |
| lateral_movement_guard | 横向移动（nmap、ssh 隧道） | [skills/lateral_movement_guard.md](skills/lateral_movement_guard.md) |
| persistence_backdoor_guard | 持久化与后门（crontab、git hooks） | [skills/persistence_backdoor_guard.md](skills/persistence_backdoor_guard.md) |
| resource_exhaustion_guard | 资源耗尽（fork 炸弹、dd 填充） | [skills/resource_exhaustion_guard.md](skills/resource_exhaustion_guard.md) |
| supply_chain_guard | 供应链（typosquatting、恶意包） | [skills/supply_chain_guard.md](skills/supply_chain_guard.md) |
| skill_installation_guard | Skill/插件安装 | [skills/skill_installation_guard.md](skills/skill_installation_guard.md) |
| browser_web_access_guard | 浏览器/Web 访问 | [skills/browser_web_access_guard.md](skills/browser_web_access_guard.md) |
| email_operation_guard | 邮件操作 | [skills/email_operation_guard.md](skills/email_operation_guard.md) |
| intent_deviation_guard | 意图偏离 | [skills/intent_deviation_guard.md](skills/intent_deviation_guard.md) |
| general_tool_risk_guard | 通用工具风险 | [skills/general_tool_risk_guard.md](skills/general_tool_risk_guard.md) |

---

## L3 决策：Prompt → Policy 映射（`templates/policy_mapping.md`）

对放行的动作，AgentMoss 按场景生成最小权限策略参考，见 [`templates/policy_mapping.md`](templates/policy_mapping.md)。

---

## 规则管控

规则可通过 Policy Console（`http://127.0.0.1:9090/console`）动态增删/开关，详见 [README.md](README.md) 的 "Policy Console" 与 "runtime_config" 章节，及 [`POLICY_CONSOLE_SUMMARY.md`](POLICY_CONSOLE_SUMMARY.md)。

---

*规则来源：AgentMoss 服务仓库（`~/gitcode/AgentMoss/`）。本文档为 xiaoO 插件目录的客户参考副本，规则定义随 AgentMoss 服务版本更新。*
