# E2B Workspace 与 Skills Bootstrap

本文说明 E2B runtime 的 API 级 workspace 与 skills 初始化机制。该机制用于消除
daemon 宿主机上下文与 E2B sandbox 文件系统之间的“双源”问题。

## 功能状态

当前版本提供单向、首次初始化的 bootstrap：

- API 调用方在首次 `open` 或首次 `input` 时指定 daemon 宿主机上的 workspace
  和 skill 搜索根。
- daemon 创建不可变归档并安装到新 E2B sandbox。
- 安装成功后，E2B 内的文件成为该 runtime 的唯一真源。
- 后续轮次、checkpoint、pause/resume 和 checkout 不再重新读取宿主机目录。

该机制只适用于 E2B backend。local 等其他 backend 保持原有的 daemon 配置行为。

> `workspace` 和 `skills` 都是 daemon 所在宿主机的路径，不是 HTTP API 客户端
> 所在机器的路径。

## 要解决的问题

在没有 bootstrap 的情况下，runtime 可能同时使用两份不一致的数据：

- 系统提示、`AGENTS.md`、repo map 和 skill registry 从 daemon 宿主机生成。
- 文件、shell 等 backend 工具实际操作 `/home/user/workspace` 中的 E2B 文件。

模型因此可能“看到”宿主机最新代码，却让工具操作空目录、旧模板或另一份代码。
本功能把 runtime 构建顺序调整为先复制和安装，再从 E2B 文件系统生成上下文。

```mermaid
flowchart LR
    A["首次 open / input"] --> B["校验并绑定宿主路径"]
    B --> C["构建临时 tar 与 SHA-256"]
    C --> D["创建 E2B sandbox"]
    D --> E["流式上传、校验、解压到 staging"]
    E --> F["替换远端 workspace / skills"]
    F --> G["写入 bootstrap manifest"]
    G --> H["从 E2B 生成 AGENTS、repo map、skills"]
    H --> I["执行 runtime"]
```

## API

以下两个请求增加了相同的可选字段：

| Endpoint | Request 类型 |
| --- | --- |
| `POST /api/v1/runtimes/open` | `RuntimeOpenRequest` |
| `POST /api/v1/runtimes/input` | `RuntimeTurnRequest` |

字段定义：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `workspace` | `string \| null` | 要复制到 E2B 的 daemon 宿主机目录 |
| `skills` | `string[] \| null` | 有序的 daemon 宿主机 skill 搜索根列表；每个根的一级子目录为候选 skill |

`open` 示例：

```json
{
  "runtime_id": "runtime-1",
  "conversation_id": "conversation-1",
  "sender_id": "user-1",
  "workspace": "/home/cz",
  "skills": [
    "/home/cz/.xiaoo/skills",
    "/opt/company/skills"
  ]
}
```

直接 `input` 也可以完成首次绑定：

```json
{
  "runtime_id": "runtime-1",
  "conversation_id": "conversation-1",
  "sender_id": "user-1",
  "text": "检查当前项目",
  "mentions": [],
  "workspace": "/home/cz",
  "skills": [
    "/home/cz/.xiaoo/skills"
  ]
}
```

### 路径要求

所有非空输入路径都必须：

- 是绝对路径；
- 在 daemon 宿主机上存在；
- 可读；
- 是目录。

daemon 会先对路径执行 canonicalize。符号路径与其规范路径会绑定为同一个路径。
当前版本信任 API 调用方，不限制 allowed roots。

`skills` 中的每一项都是“搜索根目录”，不是 skill 名称、单个 skill 目录或
`SKILL.md` 文件路径。列表顺序也是同名 skill 的优先级顺序。

## Runtime 绑定语义

workspace 和 skill roots 在 runtime 级别不可变绑定。

| 请求场景 | `workspace` | `skills` | 结果 |
| --- | --- | --- | --- |
| 新 runtime | 省略或 `null` | 省略或 `null` | 绑定空 workspace、空 skills |
| 新 runtime | 指定目录 | 指定列表 | canonicalize、归档并绑定 |
| 新 runtime | 任意 | `[]` | 显式绑定空 skills |
| 已有 runtime | 省略或 `null` | 省略或 `null` | 继承已有绑定 |
| 已有 runtime | 与绑定相同 | 与绑定相同 | 接受；不会重新复制 |
| 已有 runtime | 与绑定不同 | 与绑定不同 | 返回 `409 Conflict` |

skill roots 的顺序属于绑定身份的一部分。相同目录但不同顺序会被视为冲突。
绑定完成后不能通过传 `null` 或 `[]` 清空已有内容；需要创建新的 runtime。

已有 handle 的 `open` 快速返回路径也会先执行一致性校验，不能借此绕过绑定检查。
同一 daemon 进程内的 runtime 初始化锁保证并发首次请求只执行一次归档与
bootstrap。

## 远端目录布局

E2B 使用固定路径，不继承 daemon 配置中的本地 workspace 或 skills 目录：

```text
/home/user/workspace/                 # workspace 快照
/home/user/.xiaoo/skills/
├── 0/skill-00000/                    # 第 0 个搜索根中的最终启用 skill
├── 0/skill-00001/
└── 1/skill-00002/                    # 第 1 个搜索根中的最终启用 skill
/home/user/.xiaoo/bootstrap/
└── manifest.json                     # 已安装内容的版本和摘要
```

远端 skill 目录名是稳定归档序号，不承诺与宿主机目录名相同。对模型公开的
`Skill.location`、skill 目录表、prompt indicator 和 `[Skill directory]` 都使用
远端路径。

即使请求没有指定 workspace 或 skills，bootstrap 仍会安装空目录，以清除 E2B
模板可能残留的旧文件。

## Workspace 快照规则

指定 workspace 表示复制整个目录，包括：

- 普通文件和所有子目录；
- 隐藏文件；
- `.git`；
- 空目录；
- Unix 权限和可执行位。

owner、xattr 和时间戳不保留。归档头使用确定性元数据，便于得到稳定摘要。

符号链接按以下规则处理：

- 指向同一输入根内部的相对链接会被保留；
- 指向同一输入根内部的绝对链接会重写为对应的 E2B 绝对路径；
- 越过输入根、无法解析或指向外部的链接会使整个请求失败。

socket、device、FIFO 等特殊文件也会使整个请求失败。归档期间会比较文件、目录
和符号链接的前后元数据；检测到变化时终止请求并返回可重试冲突，避免生成明显
混合的快照。对于频繁写入的项目，调用方应尽量选择稳定时点创建 runtime。

## Skill 选择与复制

daemon 按以下流程处理 API 传入的 skill roots：

1. 按 API 列表顺序处理搜索根。
2. 对每个根的一级子目录进行确定性排序扫描。
3. 沿用现有的 skill 解析、脚本策略和安全审计配置。
4. 无效 skill 跳过；同名 skill 由第一个有效候选胜出。
5. 只复制最终启用的 skill，但复制其完整目录，包括 `scripts/`、`assets/` 和其他
   配套文件。

runtime finalize 时从 E2B 文件重新解析这些 skill manifest，而不是回退到宿主机
registry。远端 manifest 后续若被修改为无效内容，该 skill 会被跳过。

未指定 `skills` 时，E2B skill registry 为空。daemon 配置、用户目录和系统目录中
的默认 skills 都不会泄漏到该 E2B runtime。local backend 的多级 skill 发现规则
不受影响。

## AGENTS.md、repo map 与运行时真源

bootstrap 成功后：

- `descriptor.workspace_root` 和 AgentContext 均为 `/home/user/workspace`；
- 只读取 `/home/user/workspace/AGENTS.md`，不读取指定根之外的父级
  `AGENTS.md`；
- repo map 通过 E2B backend 枚举和读取远端 workspace；
- skill registry 通过 E2B backend 读取远端 skill 文件；
- 后续轮次不会用宿主机内容覆盖 E2B 内的修改。

因此，agent 在 E2B 内新增、删除或修改源码后，下一次 runtime 解析生成的 repo map
能够反映远端状态。未指定 workspace 时，不生成 workspace `AGENTS.md` 段落或 repo
map。

## 归档与安装

bootstrap payload 是 daemon 本地磁盘上的临时、未压缩 tar：

- 内容以流式方式写入归档，不把整个 workspace 读入内存；
- 完成后计算 SHA-256，并将本地临时文件权限收紧为只读；
- 上传使用流式 HTTP body；
- E2B 端先校验 SHA-256，再解压到 staging 目录；
- 安装通过目录 rename 和失败回滚替换 workspace/skills；
- `manifest.json` 最后写入，作为安装完成标记。

finalize 会校验远端 manifest 的版本和摘要是否与 session 中的绑定一致。摘要或版本
不一致时返回冲突，不在未知文件状态上继续执行。

采用内部 tar bootstrap 是为了减少目录逐文件上传的请求数量。E2B 的通用文件上传
能力参见 [E2B 文件上传说明](https://e2b.dev/docs/quickstart/upload-download-files)。

## Session、Checkpoint 与恢复

`RuntimeBootstrapBinding` 持久化在 session runtime snapshot 中，包含：

- canonical 宿主 workspace；
- canonical 宿主 skill roots；
- bootstrap 内容 SHA-256；
- 远端 workspace 和 skill roots；
- 最终启用的远端 skill manifest 表；
- bootstrap manifest 版本。

宿主路径只在首次创建 sandbox 时读取。checkpoint、pause/resume 和 checkout 继承
snapshot 中的 binding 和 E2B provider snapshot，不重新从宿主机复制。checkout 创建
的子 runtime 继承同一份 bootstrap 身份，但随后可在自己的 E2B 文件系统内独立演进。

## 容量限制

限制对 workspace 与最终复制的 skills 合并计算：

| 限制 | 最大值 |
| --- | ---: |
| 条目总数 | 100,000 |
| 单个普通文件 | 128 MiB |
| 普通文件内容总量 | 1 GiB |

条目包括目录、文件和符号链接。1 GiB 限制是普通文件内容之和；tar header 会让实际
临时归档略大。安装期间 daemon 和 E2B 都需要额外的 staging 空间。

## 错误行为

| HTTP 状态 | 条件 | 调用方建议 |
| --- | --- | --- |
| `400 Bad Request` | 路径非绝对、不存在、不可读、不是目录，或链接/文件类型不合法 | 修正输入目录 |
| `409 Conflict` | 已绑定参数不同、归档期间源发生变化、远端 manifest 不匹配 | 源变化时可重试；绑定变化时创建新 runtime |
| `413 Payload Too Large` | 任一容量限制被超过 | 缩小 workspace 或 skill 集合 |
| `500 Internal Server Error` | 归档 I/O、上传、解压、provider 或 finalize 失败 | 查看 daemon/E2B 日志后重试 |

新 sandbox 的 bootstrap 任一阶段失败时，daemon 会删除 sandbox、释放容量预留，并且
不保存新 session。调用方不会得到一个已经注册但内容不完整的 runtime。

## Custom Tools

E2B runtime 默认关闭 declarative/plugin filesystem custom tool source：

- workspace 中的 `.xiaoo/tools` 会作为普通 workspace 内容被复制；
- 这些 manifest 不会注册到 E2B tool manifest；
- daemon 全局 declarative custom tools 也不会注册到 E2B；
- built-in tools 保持可用；MCP 工具不属于该 filesystem custom-tool 开关。

local backend 的 custom tools 行为保持不变。

## 安全与运维注意事项

- API 调用方被视为受信任主体。当前没有 allowed-roots 或目录白名单。
- 指定整个目录意味着授权复制其中的隐藏文件、`.git`、密钥文件和其他潜在敏感
  内容。调用方应在传入前确认目录边界。
- daemon 需要有足够的本地临时磁盘容纳归档；E2B 需要同时容纳 staging 和目标
  内容。
- bootstrap 是一次性快照，不是同步协议。宿主机后续修改不会进入已有 runtime，
  E2B 修改也不会写回宿主机。
- channel、cron、remote TUI 等未传新字段的 E2B runtime 会得到空 workspace 和空
  skills，不回退到 daemon 默认目录。

## v1 非目标

当前版本不提供：

- 宿主机与 E2B 的双向同步；
- 已绑定 runtime 的手动 refresh 或重新导入；
- E2B 修改写回宿主机；
- API 路径 allowed-roots；
- E2B declarative custom tools 启用开关；
- local backend 的 API 级 workspace/skills 覆盖。

## 验证范围

自动化测试覆盖以下关键行为：

- 旧请求 serde 兼容、`open`/直接 `input` 首次绑定和显式空 skills；
- canonical 路径一致性、已有 handle 快速路径冲突和 HTTP 状态映射；
- 隐藏文件、`.git`、空目录、执行权限、内部相对/绝对符号链接；
- 越界链接、FIFO、条目/单文件/总容量限制；
- skill root 优先级、同名首胜、远端 skill 路径；
- 不同宿主 workspace 的 runtime 隔离；
- 远端 `AGENTS.md`、repo map 更新和 manifest 不匹配；
- E2B custom tool 关闭与 local backend 行为隔离。

完整 workspace 测试可通过以下命令执行：

```bash
cargo test --workspace
```

真实 E2B smoke test 需要配置 `E2B_API_KEY`，默认仍标记为 ignored，不包含在普通
workspace 测试中。

## 实现位置

| 模块 | 职责 |
| --- | --- |
| `apps/shared/src/backend/e2b/bootstrap.rs` | 路径校验、skill 选择、归档、摘要和容量限制 |
| `apps/shared/src/backend/e2b/provider.rs` | 流式上传、远端校验、staging 安装与失败清理 |
| `apps/serverside/src/daemon_runtime.rs` | E2B API 参数解析、绑定校验和 runtime 静态构建 |
| `apps/shared/src/gateway/e2b_runtime.rs` | 远端 manifest、AGENTS、repo map 和 skill finalize |
| `apps/shared/src/gateway/session_record.rs` | `RuntimeBootstrapBinding` snapshot 数据结构 |
| `apps/shared/src/gateway/session_service_impl.rs` | 初始化锁、backend 生命周期与 session 保存顺序 |
| `crates/skill/src/loading/loader.rs` | 确定性 skill 扫描、解析、审计和去重 |
| `crates/tool/src/impl/source_loader.rs` | E2B declarative custom tool source gating |
