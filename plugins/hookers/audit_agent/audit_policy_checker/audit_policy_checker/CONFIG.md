# config.json 配置说明

将 `config.json.example` 复制为 `config.json`，修改对应字段即可。

```bash
cp config.json.example config.json
```

## 字段说明

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `llm.api_key` | OpenRouter API Key。也可通过环境变量 `OPENROUTER_API_KEY` 设置（环境变量优先级更高） | 空 |
| `llm.model` | 模型名称（OpenRouter 格式，如 `anthropic/claude-3.5-sonnet`、`openai/gpt-4o`） | `anthropic/claude-3.5-sonnet` |
| `llm.temperature` | 生成温度，0.1 表示低随机性、更稳定的输出 | `0.1` |
| `llm.base_url` | API 地址，默认为 OpenRouter | `https://openrouter.ai/api/v1` |
| `timeout.total_timeout` | 整个审计流程的总超时（秒） | `60.0` |
| `timeout.prompt1_timeout` | PROMPT1（生成 policy）的 LLM 调用超时（秒） | `30.0` |
| `timeout.prompt2_timeout` | PROMPT2（合规性判定）的 LLM 调用超时（秒） | `20.0` |
| `timeout.step_interval` | Step1 与 Step2 之间的间隔（秒），设为 0 则不等待。用于避免连续两次 LLM 调用触发 API 限流 | `5.0` |
| `cache.enabled` | 是否启用 session 级别 policy 缓存 | `true` |
| `cache.max_size` | 最大缓存条目数（LRU 淘汰） | `1000` |
| `retry.max_retries` | LLM 调用最大重试次数 | `3` |
| `retry.retry_interval` | 重试间隔（秒） | `2.0` |
| `security.enabled` | 是否启用安全检测 | `true` |
| `security.heuristic_enabled` | 是否启用启发式静态检测 | `true` |
| `security.logic_rules_enabled` | 是否启用逻辑规则检测 | `true` |
| `security.llm_analysis_enabled` | 是否启用 LLM 深度分析 | `true` |
| `security.rules_path` | 用户规则文件路径（空则使用默认路径） | 空 |
| `security.skills_dir` | Skills 目录路径（空则使用默认路径） | 空 |
| `log_level` | 日志级别（DEBUG/INFO/WARNING/ERROR） | `INFO` |

## 常用模型

通过 OpenRouter 可使用多种大模型，修改 `llm.model` 即可切换：

| 模型名称 | 说明 |
|----------|------|
| `anthropic/claude-3.5-sonnet` | Claude 3.5 Sonnet（默认，推荐） |
| `openai/gpt-4o` | GPT-4o |
| `google/gemini-2.0-flash-001` | Gemini 2.0 Flash |
| `deepseek/deepseek-chat` | DeepSeek V3 |
| `meta-llama/llama-3.3-70b-instruct` | Llama 3.3 70B |

完整模型列表见 [OpenRouter Models](https://openrouter.ai/models)。

## 配置加载优先级

1. 代码中显式指定的 `config_path` 参数
2. 环境变量 `AUDIT_CONFIG_PATH` 指定的路径
3. 默认路径：`audit_policy_checker/config.json`

## 环境变量

| 环境变量 | 说明 | 优先级 |
|----------|------|--------|
| `OPENROUTER_API_KEY` | 覆盖 config.json 中的 `llm.api_key` | 最高 |
| `AUDIT_CONFIG_PATH` | 指定配置文件路径 | 高于默认路径 |
