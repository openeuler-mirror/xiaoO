# tool_post_secret_guard

xiaoO hooker 插件，在工具执行后拦截输出，检测并脱敏疑似敏感信息（API Key、Token、私钥等）。

## 工作流程

1. 拦截 `bash`、`file_read`、`read_file` 工具的 post 阶段输出
2. 扫描 `stdout` / `stderr` 是否匹配敏感信息正则模式
3. 若检测到敏感信息：
   - 弹出确认提示，询问用户是否允许保留原始输出
   - 用户拒绝或未交互时，将敏感内容替换为 `[描述REDACTED]`
4. 无敏感信息则直接放行

## 文件说明

| 文件 | 说明 |
|------|------|
| `guard_secret_like_output.py` | 核心检测与脱敏逻辑 |
| `config.py` | 安装/卸载脚本，管理 `plugin.json` 生成与 `config.toml` 注册 |
| `tool_post_secret_guard_config.jsonc` | 敏感信息正则模式配置 |
| `plugin.json.template` | 插件描述模板，安装时自动填充绝对路径生成 `plugin.json` |
| `install.sh` / `uninstall.sh` | 快捷安装/卸载入口 |

## 安装与卸载

```bash
# 安装（生成 plugin.json，注册到 ~/.config/xiaoo/config.toml）
bash install.sh
# 或
python3 config.py install

# 卸载（移除注册并删除 plugin.json）
bash uninstall.sh
# 或
python3 config.py uninstall
```

安装时若 `config.toml` 中缺少 `[hooker]` 段或必要字段，脚本会交互式引导补全。

## 配置

编辑 `tool_post_secret_guard_config.jsonc` 自定义检测规则：

- `REDACTED_SUFFIX` — 脱敏后缀标记，默认 `[REDACTED]`
- `sensitive_patterns` — 敏感信息正则列表，支持两种格式：

```jsonc
"sensitive_patterns": [
  // 简写：直接写正则字符串
  "sk-[A-Za-z0-9_-]+",

  // 完整写：带描述和多个模式
  {
    "description": "OpenAI-compatible API Key",
    "pattern": [
      "sk-[A-Za-z0-9_-]+",
      "sk-api-[A-Za-z0-9_-]+"
    ]
  }
]
```

内置模式覆盖：OpenAI API Key、AWS Access Key、GitHub/GitLab Token、Slack Token/Webhook、JWT、PEM 私钥。列表为空时跳过检测。
