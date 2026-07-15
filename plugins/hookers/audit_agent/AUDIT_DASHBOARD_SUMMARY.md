# Audit-Agent 安全策略可视化管理平台 — 功能总结

## 一、需求背景

针对客户提出的「安全策略需要可视化管控台,动态管理增删拦截各层安全策略,适配不同落地场景」的需求,我们为 audit-agent 设计并实现了一套 Web 可视化管理平台。原有机制下,所有规则硬编码在 Python 源码中,修改需改代码并重启,无法逐条开关、无法分类管控、无可视化界面。新平台解决了这些痛点。

## 二、核心设计原理

采用 **「文件级配置传递 + 子进程独立读取」** 机制,实现零侵入热生效:

- **配置存储**:`~/.config/xiaoo/audit_runtime.json`(用户本地副本,每个用户独立)
- **配置写入**:Dashboard Web 界面操作 → 后端 API → 写入本地 JSON 文件
- **配置读取**:audit-agent 每次 tool call 以独立子进程启动,启动时读取最新 JSON
- **热生效**:修改配置 → 下次调用立即生效,无需重启、无需信号通知

遵循 **「种子 → 本地副本」** 模型:
- Python 源码中的规则 = 出厂默认(种子,不可变)
- 用户本地 JSON = 用户副本(可增删改,持久化)
- 首次运行自动从种子生成本地副本;版本升级时新规则自动合并,不影响用户已有开关设置;删除本地文件即可恢复出厂

```
┌─────────────────────────────────────────────────────────────┐
│  浏览器界面 (用户操作)                                         │
│  点开关、新增规则、禁用 skill...                              │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTP 请求
┌──────────────────────────▼──────────────────────────────────┐
│  xiaoo-audit-dashboard 进程 (常驻, FastAPI)                  │
│  收到请求 → 调用 runtime_config.py 的函数                    │
│  → 写入 ~/.config/xiaoo/audit_runtime.json                  │
│  → 写入 ~/.config/xiaoo/audit_skills/*.md (自定义 skill)    │
│  → 写入 ~/.config/xiaoo/audit_token_stats.json (token 统计) │
└──────────────────────────┬──────────────────────────────────┘
                           │ 文件落盘
                           ▼
              ~/.config/xiaoo/audit_runtime.json  ← 共享文件
                           │
                           │ audit-agent 每次被调用时读
                           ▼
┌─────────────────────────────────────────────────────────────┐
│  audit.py 子进程 (xiaoO 每次 tool call 触发,短命)            │
│  启动 → load_runtime_config() 读最新 JSON                    │
│  → 按 enabled/category_enabled 过滤规则                       │
│  → 跳过 disabled 的 skill                                    │
│  → 执行检测 → 返回 allow/deny                                 │
└─────────────────────────────────────────────────────────────┘
```

## 三、已实现功能清单

### 1. 三层分析层级全局开关

| 功能 | 说明 |
|---|---|
| L1/L2/L3 独立开关 | 可任意组合启用某层、某两层或全部三层 |
| 环境变量覆盖 | `AUDIT_DISABLE_L1`/`AUDIT_DISABLE_L2`/`AUDIT_DISABLE_LLM_LAYER3` 优先级最高,紧急运维场景硬性覆盖,界面会提示"环境变量强制覆盖" |

### 2. 规则分类与逐条管控(L1 + L2)

| 功能 | 说明 |
|---|---|
| 规则外置化 | 所有原硬编码规则提取为 JSON 配置(L1: 91 条,L2: 44 条) |
| 分类管理 | L1 分 4 类(危险命令/注入检测/用户敏感动作/自定义),L2 分 6 类(敏感路径/意图一致性/密码修改授权/用户删除授权/读写一致性/危险操作模式) |
| 分类批量开关 | 一键启用/禁用整个分类 |
| 单条规则开关 | 每条规则独立启用/禁用 |
| 新增自定义规则 | 支持正则模式、关键词、敏感路径等多种类型,带风险等级/类型/原因字段 |
| 删除自定义规则 | 仅允许删除用户自定义规则(builtin 标记保护出厂规则) |

### 3. L3 Skill 规则管理

| 功能 | 说明 |
|---|---|
| Skill 列表展示 | 12 个内置 Skill,分 6 类(文件操作/网络安全/持久化供应链/意图通用/资源/自定义) |
| 单个 Skill 开关 | 独立启用/禁用 |
| 分类批量开关 | 按分类整体启用/禁用 |
| 新增自定义 Skill | 上传 Markdown 内容 + 触发关键词,写入用户目录 `~/.config/xiaoo/audit_skills/` |
| 删除自定义 Skill | 仅允许删除用户自定义 Skill |
| 双目录加载 | 内置 skills 目录(只读)+ 用户 skills 目录(可写),用户同名可覆盖内置 |

### 4. Token 用量统计

| 功能 | 说明 |
|---|---|
| 自动记录 | 每次 LLM 调用自动记录 token 用量(prompt/completion/total tokens、model、step、时间戳) |
| 多维统计 | 按步骤(L3 安全判断/策略生成)、按模型、按日期分组汇总 |
| 可视化展示 | 概览页柱状图(输入/输出 Token 按日期分布)、Token 详情页(分组进度条 + 最近调用记录表) |
| 时间范围筛选 | 今日/近7天/近30天/全部 |
| 容量控制 | 最多保留 10000 条记录,自动裁剪;支持一键清除 |

### 5. 配置管理

| 功能 | 说明 |
|---|---|
| 完整配置查看 | 一键查看完整 runtime JSON |
| 环境变量覆盖状态 | 展示当前哪些层级被环境变量强制覆盖 |
| 重置出厂默认 | 删除本地副本,下次运行自动从源码默认值重新生成 |

## 四、部署形态(适配 openEuler RPM 安装)

| 项 | 说明 |
|---|---|
| 依赖来源 | 全部使用 openEuler 24.03-LTS-SP3 官方源 RPM 包:`python3-fastapi`、`python3-uvicorn`、`python3-starlette`(已实测确认在官方源),不依赖 pip install |
| 安装方式 | `dnf install audit-agent-*.rpm`,依赖自动从官方源拉取 |
| 代码位置 | Python 包安装到系统 site-packages;启动命令 `/usr/bin/xiaoo-audit-dashboard` |
| 启动方式 | 按需运行 `xiaoo-audit-dashboard`,前台启动,浏览器访问 `http://localhost:9765` |
| 安全绑定 | 默认仅监听 127.0.0.1,仅本地访问;可选 Bearer token 认证(`AUDIT_DASHBOARD_TOKEN` 环境变量) |
| 用户数据隔离 | 配置存 `~/.config/xiaoo/`,每用户独立,不共享 |

### RPM 安装后文件布局

```
/usr/bin/xiaoo-audit-dashboard              # 启动命令(打包时生成)
/usr/lib/xiaoo/plugins/audit_agent/         # audit.py + plugin.json
/usr/lib/python3.X/site-packages/
├── audit_policy_checker/                   # audit-agent 核心 Python 包
└── audit_dashboard/                        # 控制面板 Python 包
    ├── __init__.py
    ├── app.py                              # FastAPI 后端
    └── static/index.html                   # 前端 Web 界面
```

### 用户使用方式

```bash
# 1. 安装(依赖会被 dnf 自动从官方源拉取)
sudo dnf install ./audit-agent-0.1.0-1.oe2403.aarch64.rpm

# 2. 启动控制面板(按需,前台运行)
xiaoo-audit-dashboard
# 或指定端口
AUDIT_DASHBOARD_PORT=9765 xiaoo-audit-dashboard

# 3. 浏览器访问
# http://localhost:9765
```

## 五、待实现 / 后续可扩展项

| 项 | 说明 |
|---|---|
| systemd 常驻服务 | 当前为按需手动启动,如需开机自启可补充 systemd unit 文件 |
| 审计拦截统计 | 当前有 Token 用量统计,但未做 allow/deny 拦截分布统计(可基于 audit log 解析实现) |
| 多用户/远程管理 | 当前面向单机本地用户,如需多租户或远程集中管理需另设计 |
| 规则导入导出 | 支持配置文件的批量导入/导出,便于跨环境迁移策略 |
| 规则版本管理 | 配置变更历史追溯(目前删除即丢失,可加版本快照) |

## 六、技术栈

| 组件 | 技术 | 说明 |
|---|---|---|
| 后端 | Python + FastAPI | 与 audit-agent 同语言,复用 runtime_config 模块 |
| 前端 | 原生 HTML + CSS + JavaScript | 单页面应用,无构建依赖,零前端工具链 |
| Web 服务器 | uvicorn | ASGI 服务器,FastAPI 标准配套 |
| 配置存储 | 本地 JSON 文件 | `~/.config/xiaoo/` 下,无数据库依赖 |
| 依赖来源 | openEuler 官方源 RPM | fastapi/uvicorn/starlette 均在官方源,符合 RPM 打包约束 |

## 七、API 接口一览

| 方法 | 路径 | 功能 |
|---|---|---|
| GET | `/api/layers` | 获取 L1/L2/L3 开关状态 |
| PUT | `/api/layers` | 设置层级开关 |
| GET | `/api/rules` | 获取规则列表(可按层/分类筛选) |
| PUT | `/api/rules/enabled` | 开关单条规则 |
| PUT | `/api/categories/enabled` | 开关整个分类 |
| POST | `/api/rules` | 新增自定义规则 |
| DELETE | `/api/rules` | 删除自定义规则 |
| GET | `/api/skills` | 获取所有 Skill 列表 |
| PUT | `/api/skills/enabled` | 开关单个 Skill |
| PUT | `/api/skill-categories/enabled` | 开关 Skill 分类 |
| POST | `/api/skills` | 新增自定义 Skill |
| DELETE | `/api/skills` | 删除自定义 Skill |
| GET | `/api/config` | 获取完整配置 |
| GET | `/api/env-overrides` | 获取环境变量覆盖状态 |
| POST | `/api/reset` | 重置到出厂默认 |
| GET | `/api/token-stats` | 获取 token 用量统计(支持 days 参数) |
| GET | `/api/token-stats/recent` | 获取最近调用记录 |
| POST | `/api/token-stats/reset` | 清除 token 统计 |

---

**总结**:本次实现完整覆盖了客户提出的可视化管控、动态增删规则、层级灵活组合、热生效等核心需求,且严格遵循 openEuler 官方源 RPM 依赖约束,可直接通过现有 OBS 打包流程交付。Token 用量统计作为附加能力,帮助客户量化 audit-agent 的 LLM 消耗成本。
