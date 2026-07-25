# AgentMoss 安全策略管控台（Policy Console）— 功能总结

## 一、需求背景

针对「安全策略需要可视化管控、动态增删/开关各层规则、适配不同落地场景」的需求，AgentMoss 提供了一套 Web 可视化管理平台。原有机制下规则定义在源码/配置中，修改需重启；新平台支持逐条开关、分类管控、可视化界面，且**热生效无需重启**。

> 相比旧 audit_agent 的 Dashboard（独立常驻 FastAPI 子进程 + 子进程独立读 JSON），AgentMoss 的 Console 是**服务内置 sub-router**——管控台和判定服务同进程，直接改 runtime JSON，`is_layer_enabled` 每次判定现读，零延迟生效。

## 二、核心设计原理

**文件级配置传递 + runtime JSON 持久化**，实现零侵入热生效：

- **配置存储**：`~/.config/agentmoss/agent_moss_runtime.json`（`AGENT_MOSS_RUNTIME_CONFIG_PATH` 覆盖）
- **配置写入**：Console Web 界面操作 → Console API → 写入 runtime JSON
- **配置读取**：AgentMoss 服务判定时现读 runtime JSON（`is_layer_enabled` 每次判定读最新）
- **热生效**：改配置 → 下次判定立即生效，无需重启服务、无需信号通知

遵循 **「seed → 本地副本」** 模型：
- 源码常量 = 出厂默认（seed，不可变）
- 用户本地 runtime JSON = 用户副本（可增删改，持久化）
- 首次加载自动从 seed 生成本地副本；版本升级时新规则自动合并（指纹去重，保留用户开关）；删除本地文件可恢复出厂

```
┌─────────────────────────────────────────────────────────────┐
│  浏览器界面（用户操作）                                        │
│  开关三层、增删规则、禁用 skill、改 deny_mode、看 token 用量    │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTP /console/api/*
┌──────────────────────────▼──────────────────────────────────┐
│  AgentMoss 服务进程（常驻，FastAPI，含 Console sub-router）    │
│  收到请求 → runtime_config CRUD → 写 runtime JSON             │
└──────────────────────────┬──────────────────────────────────┘
                           │ 现读
┌──────────────────────────▼──────────────────────────────────┐
│  ~/.config/agentmoss/agent_moss_runtime.json                 │
│  三层开关 / 规则启停 / deny_mode / skip_l3 / skill 开关       │
└─────────────────────────────────────────────────────────────┘
```

## 三、访问与鉴权

- **访问**：服务启动后浏览器打开 `http://127.0.0.1:9090/console`（端口随服务实际监听，默认 9090；bridge 探测 9090-9095）。
- **鉴权**：默认本机（127.0.0.1）免鉴权，便于 iframe 嵌入；远程访问设 `AGENT_MOSS_CONSOLE_TOKEN`，请求带 `Authorization: Bearer <token>`。

## 四、Console API

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/console` | 返回 SPA index.html |
| `GET` / `PUT` | `/console/api/layers` | 查/改三层开关（L1/L2/L3 启停）|
| `GET` | `/console/api/rules` | 查全量规则（L1/L2，builtin + 自定义）|
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
| `GET` | `/console/api/config` | 查完整 runtime config + 路径 |
| `GET` | `/console/api/env-overrides` | 查被环境变量接管的开关（灰色不可改）|
| `POST` | `/console/api/reset` | 重置为出厂默认 |
| `GET` | `/console/api/token-stats` | token 用量统计 |
| `GET` | `/console/api/token-stats/recent` | 最近 N 条调用记录 |

## 五、配置优先级

`AGENT_MOSS_DISABLE_*` env > runtime JSON > `agent_moss_settings.json` > 默认。env 永远最高（Console 会把被 env 接管的开关展示为灰色不可改）。

## 六、与旧 audit_agent Dashboard 的差异

| 维度 | 旧 audit_agent Dashboard | AgentMoss Console |
|------|--------------------------|-------------------|
| 进程模型 | 独立常驻 FastAPI 子进程，子进程独立读 JSON | 服务内置 sub-router，同进程改 runtime JSON |
| 生效方式 | 子进程重启读 JSON | 服务现读，零延迟 |
| 配置位置 | `~/.config/xiaoo/audit_runtime.json` | `~/.config/agentmoss/agent_moss_runtime.json` |
| 访问 | 独立端口 | 服务端口 `/console` |

---

*详见 [README.md](README.md) 的 "Policy Console" 与 "runtime_config" 章节。规则定义见 [SECURITY_RULES.md](SECURITY_RULES.md)。*
