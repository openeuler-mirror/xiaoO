# llm_pre_secret_guard

xiaoO hooker 插件，在 LLM 请求发送前检测请求中的敏感信息，向用户确认。

## 工作流程

1. 拦截 LLM `pre` 阶段
2. 扫描 `request.messages` 中的敏感信息（API Key、Token、私钥等）
3. 若检测到敏感信息：
   - 弹出确认提示，询问用户是否允许原样发送
   - 用户允许 → 放行原请求
   - 用户拒绝 → 脱敏后发送（敏感内容替换为 `[描述REDACTED]`）
4. 无敏感信息则直接放行

## 文件说明

| 文件 | 说明 |
|------|------|
| `guard_secret_like_request.py` | 核心检测与脱敏逻辑 |
| `config.py` | 安装/卸载脚本 |
| `llm_pre_secret_guard_config.jsonc` | 敏感信息正则模式配置 |
| `plugin.json.template` | 插件描述模板 |
| `install.sh` / `uninstall.sh` | 快捷安装/卸载入口 |

## 安装与卸载

```bash
# 安装
bash install.sh
# 或
python3 config.py install

# 卸载
bash uninstall.sh
# 或
python3 config.py uninstall
```

## 配置

编辑 `llm_pre_secret_guard_config.jsonc` 自定义检测规则：

- `REDACTED_SUFFIX` — 脱敏后缀标记，默认 `[REDACTED]`
- `sensitive_patterns` — 敏感信息正则列表，与 `tool_post_secret_guard` 格式相同

内置模式覆盖：OpenAI API Key、AWS Access Key、GitHub/GitLab Token、Slack Token/Webhook、JWT、PEM 私钥。列表为空时跳过检测。
