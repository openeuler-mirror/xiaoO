# audit-agent OBS 打包说明

## 目录结构

```
obs/
├── audit-agent.spec          # RPM spec 文件
├── audit-agent-0.1.0.tar.gz  # 源码压缩包
└── README.md                  # 本文件
```

## OBS 构建流程

1. 在 OBS 中创建项目（如 `home:kenhkl:xiaoo`）
2. 创建包 `audit-agent`
3. 上传 `audit-agent.spec` 和 `audit-agent-0.1.0.tar.gz`
4. OBS 会自动根据 spec 文件中的 `Requires` 从 openEuler 24.03-LTS-SP3 源下载依赖并构建

## 依赖清单

所有依赖均可在 openEuler 24.03-LTS-SP3 官方源中找到：

| 依赖包 | openEuler RPM 包名 | 版本 | 仓库位置 |
|--------|-------------------|------|---------|
| openai | python3-openai | 1.77.0 | EPOL/main |
| httpx | python3-httpx | 0.27.0 | everything |
| pydantic | python3-pydantic | 2.10.6 | everything |
| pydantic-core | python3-pydantic-core | 2.27.2 | everything |
| tenacity | python3-tenacity | 8.2.3 | everything |
| tomli | python3-tomli | 2.0.1 | everything |

注：loguru 已移除，改用 Python 标准库 logging。

## 安装后配置

### 1. 安装 RPM

```bash
# 先安装 xiaoo RPM
rpm -i xiaoo-0.1.0-1.rpm

# 再安装 audit-agent RPM
rpm -i audit-agent-0.1.0-1.rpm
```

### 2. 启用插件

**方式一：CLI 命令**
```bash
xiaoo plugin enable plugin_audit_tool_input
```

**方式二：TUI 命令**
```
/plugin enable plugin_audit_tool_input
```

**方式三：手动编辑配置文件**

编辑 `~/.config/xiaoo/config.toml`，添加：

```toml
[hooker]
default = "None"
plugins = ["/usr/lib/xiaoo/plugins/audit_agent/plugin.json"]
enabled = ["plugin_audit_tool_input"]
```

### 3. 配置 LLM（可选）

audit_agent 的第三层 LLM 分析需要配置 LLM API。在 `~/.config/xiaoo/config.toml` 中配置：

```toml
[llm]
provider = "zhipu"
model = "glm-4-flash"
api_key_env = "XIAOO_API_KEY"
```

或设置环境变量：
```bash
export XIAOO_API_KEY="your-api-key"
```

如不需要 LLM 分析层，可禁用：
```bash
# 编辑 /usr/lib/xiaoo/plugins/audit_agent/audit_settings.json
# 设置 "AUDIT_DISABLE_LLM_LAYER3": "1"
```

## RPM 安装后的文件布局

```
/usr/lib/xiaoo/plugins/audit_agent/
├── audit.py                      # 插件入口脚本
├── plugin.json                   # hooker 注册文件（command 使用系统 Python）
├── audit_settings.json           # 运行时配置（%post 从 .example 生成）
└── audit_settings.json.example   # 配置模板

/usr/lib/python3.11/site-packages/audit_policy_checker/   # Python 包
├── __init__.py
├── main.py
├── config.py
├── logging_utils.py
├── llm_client.py
├── cli.py
├── parsers.py
├── policy_cache.py
├── prompt_templates.py
├── security/
│   ├── audit_agent.py
│   ├── heuristic_detector.py
│   ├── logic_rules.py
│   ├── llm_analyzer.py
│   ├── script_content_analyzer.py
│   ├── skill_engine.py
│   └── types.py
├── templates/
├── skills/
└── rules/
```

## 与 xiaoo RPM 的关系

- xiaoo RPM 先构建，提供 `xiaoo` 和 `xiaoo-tui` 二进制
- audit-agent RPM 后构建，仅提供 Python 插件代码
- 两者通过 `plugin.json` 的绝对路径关联
- audit-agent RPM 不依赖 xiaoo RPM（可以独立安装），但只有在 xiaoo 配置中启用后才生效
