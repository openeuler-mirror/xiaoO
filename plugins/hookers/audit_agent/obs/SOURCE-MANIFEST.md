# audit-agent 源码清单

源码压缩包 `audit-agent-0.1.0.tar.gz` 由 `build-source-tarball.sh` 生成。

## 文件清单

以下路径相对于 `plugins/hookers/audit_agent/`：

### 根目录文件

| 文件 | 说明 |
|------|------|
| `audit.py` | 插件入口脚本，读取 xiaoO stdin JSON 调用审计引擎 |
| `audit_settings.json.example` | 运行时配置模板（LLM 开关、超时、日志路径等） |
| `README.md` | 插件说明文档 |
| `SECURITY_RULES.md` | 安全规则说明文档 |

### audit_policy_checker/ — Python 包构建文件

| 文件 | 说明 |
|------|------|
| `audit_policy_checker/pyproject.toml` | Python 包元数据和依赖声明 |
| `audit_policy_checker/requirements.txt` | pip 依赖清单 |

### audit_policy_checker/audit_policy_checker/ — Python 源码

| 文件 | 说明 |
|------|------|
| `__init__.py` | 包初始化 |
| `main.py` | 核心入口 `audit_action()`，协调三层检测 |
| `config.py` | 配置加载（config.json + audit_settings.json + xiaoo config.toml fallback） |
| `llm_client.py` | OpenAI SDK 封装 |
| `logging_utils.py` | 日志工具（stdlib logging） |
| `cli.py` | CLI 入口（`auditagent` 命令） |
| `parsers.py` | 输入解析工具 |
| `policy_cache.py` | session 级 policy 缓存 |
| `prompt_templates.py` | LLM prompt 模板加载 |
| `config.json.example` | 默认配置模板 |

### audit_policy_checker/audit_policy_checker/security/ — 安全检测子包

| 文件 | 说明 |
|------|------|
| `__init__.py` | 子包初始化 |
| `audit_agent.py` | 主协调器，三层防御逻辑 |
| `heuristic_detector.py` | 第一层：启发式静态检测 |
| `logic_rules.py` | 第二层：逻辑规则检测（read_before_write、敏感路径等） |
| `llm_analyzer.py` | 第三层：LLM 深度分析 |
| `script_content_analyzer.py` | 脚本内容预扫描（可疑关键词检测） |
| `skill_engine.py` | Skill 规则引擎 |
| `types.py` | 类型定义 |

### audit_policy_checker/audit_policy_checker/templates/ — LLM prompt 模板

| 文件 | 说明 |
|------|------|
| `prompt1_template.txt` | 第一轮 LLM prompt 模板 |
| `prompt2_template.txt` | 第二轮 LLM prompt 模板 |
| `security_judge_template.txt` | 安全判断 prompt 模板 |
| `policy_mapping.md` | 策略映射说明 |

### audit_policy_checker/audit_policy_checker/skills/ — 安全 Skill 规则

| 文件 | 说明 |
|------|------|
| `browser_web_access_guard.md` | 浏览器/网页访问安全规则 |
| `data_exfiltration_guard.md` | 数据外泄防护规则 |
| `email_operation_guard.md` | 邮件操作安全规则 |
| `file_access_guard.md` | 文件访问安全规则 |
| `general_tool_risk_guard.md` | 通用工具风险规则 |
| `intent_deviation_guard.md` | 意图偏离检测规则 |
| `lateral_movement_guard.md` | 横向移动检测规则 |
| `persistence_backdoor_guard.md` | 持久化/后门检测规则 |
| `resource_exhaustion_guard.md` | 资源耗尽检测规则 |
| `script_execution_guard.md` | 脚本执行安全规则 |
| `skill_installation_guard.md` | Skill 安装安全规则 |
| `supply_chain_guard.md` | 供应链攻击检测规则 |

### audit_policy_checker/audit_policy_checker/rules/ — 用户规则

| 文件 | 说明 |
|------|------|
| `user_rules.json` | 用户自定义规则 |

## 排除的文件

以下文件/目录不包含在源码压缩包中：

| 路径 | 原因 |
|------|------|
| `audit_policy_checker/venv/` | Python 虚拟环境，RPM 使用系统 Python |
| `audit_policy_checker/build/` | setuptools 构建产物 |
| `audit_policy_checker/dist/` | pip 构建产物 |
| `audit_policy_checker/__pycache__/` | Python 字节码缓存 |
| `audit_policy_checker/*.egg-info/` | setuptools 元数据 |
| `audit_policy_checker/.pytest_cache/` | pytest 缓存 |
| `audit_policy_checker/audit_policy_checker.log` | 运行时日志 |
| `audit_policy_checker/audit_policy_checker/__pycache__/` | Python 字节码缓存 |
| `plugin.json` | RPM 版本由 spec 文件重新生成（command 路径不同） |
| `install.sh` | 手动安装脚本，RPM 不需要 |
| `audit_settings.json` | 运行时生成的配置，RPM %post 脚本从 .example 生成 |
