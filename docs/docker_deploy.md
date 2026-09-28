# Docker 容器化部署

> ## ⚠️ 当前 `Dockerfile` 无法构建成功（2026-09 核对）
>
> **`docker build` 今天会失败。本节文档描述的是镜像的设计意图与修复方向，不是
> 可直接复现的现状。** 已确认三处缺陷（本次变更按用户决定**不修改 `Dockerfile`**，
> 仅在此记录；修复 Dockerfile 属于独立变更）：
>
> | # | 位置 | 缺陷 | 修复方向 |
> | --- | --- | --- | --- |
> | 1 | `Dockerfile:56` | `COPY crates/llm-client-cli/Cargo.toml` 指向**已被删除**的 crate（删除提交 `a507b74` "refactor: remove llm-client-cli crate, remove related tests"；`crates/` 下现无 `llm-client-cli`）→ 构建在此中止 | 删除该行 |
> | 2 | `Dockerfile:45-66` | 逐层 COPY 的 workspace 成员清单**遗漏** `crates/xiaoo-api` 与 `crates/protocol`，而两者都是 workspace 成员（`Cargo.toml:30-31`）→ 后续 `cargo fetch`（`Dockerfile:89`）无法解析整个 workspace | 补两行 `COPY crates/xiaoo-api/Cargo.toml` / `COPY crates/protocol/Cargo.toml` |
> | 3 | `Dockerfile:118-126` | 对 `plugins/hookers/audit_agent/` 执行 `rm -f .../install.sh` 并向该目录写入 `plugin.json`，但 `audit_agent` 已在 `1f08b49` "移除 audit_agent，由 agent_moss 完全取代" 中被删除（`plugins/hookers/` 现无此目录）→ `&&` 链在该步失败 | 删除这两步，或改为对 `agent_moss` 做等价处理 |
>
> 附带（不单独致命，但同属 drift）：`Dockerfile:111-115` 的注释与 `:148-161` 的
> pip/dnf 依赖清单仍围绕被删除的 `audit_agent` / `audit_dashboard` 编写，见下文 §1。
>
> 下文除"构建镜像"章节外，其余内容（镜像元信息、运行方式、环境变量）描述的是
> **镜像成功构建后**的形态，可用于编写与评审 `Dockerfile` 修复，但**不要**据此
> 认为镜像今天可用。

xiaoO 提供官方 `Dockerfile`，设计目标是从代码仓构建出一个包含 `xiaoo`、
`xiaoo-daemon` 和 `moirai` 三个程序的镜像。镜像默认拉起 `xiaoo-daemon`，
对外暴露 HTTP API（`18080`）与只读 dashboard（`28081`）。

> **镜像不内置 `config.toml`**：运行时挂载配置文件，同一镜像可服务
> OpenRouter / OpenAI / 智谱 / 本地模型 / Feishu bot 等任何场景，只需换
> 挂载的配置和环境变量。通用配置参考 [通用配置指南](./config_file_guide.md)，
> daemon 专用配置参考 [Daemon 配置](./daemon_config.md)。

---

## 1. 镜像内容

基于 `openeuler/openeuler:24.03-lts-sp3`，两阶段构建：builder 阶段用 dnf
自带的 Rust 1.90 编译三个二进制并打包到 `/staging/`，runtime 阶段只装运行时
依赖并拷入 staged 产物，最终镜像不带任何编译工具链。

### 文件布局

| 路径 | 内容 |
| --- | --- |
| `/usr/bin/xiaoo` | CLI / TUI 二进制 |
| `/usr/bin/xiaoo-daemon` | HTTP daemon 二进制（默认启动） |
| `/usr/bin/moirai` | moirai tracing 工具 |
| `/usr/bin/xiaoo-hookers-install` / `-uninstall` | → hookers `install.sh` / `uninstall.sh` 的软链 |
| `/usr/lib/.xiaoo/skills/` | 内置 skills |
| `/usr/lib/.xiaoo/hookers/` | hookers 插件（随仓发布的四个：`agent_moss`、`cerberus_bash_control`、`llm_pre_secret_guard`、`tool_post_secret_guard`） |
| `/usr/share/doc/xiaoO/` | `README.md`、`README.zh-CN.md`、`docs/*.md` |
| `/usr/share/licenses/xiaoO/` | `LICENSE` |
| `/root/.xiaoo/` | 运行时数据（trace.db 等），`VOLUME`；即 `~/.xiaoo` |
| `/root/.config/xiaoo/` | 默认配置目录（空，挂载 `config.toml` 到此即走默认解析） |

### 预装运行时依赖（全部 dnf 安装，无 pip）

| 用途 | 包 |
| --- | --- |
| 主二进制 | `xclip`、`git`、`ca-certificates`、`curl`（HEALTHCHECK） |
| hookers（`agent_moss` 等 Python 脚本） | `python3` + `python3-tomli`（`agent_moss/bridge.py:57-60` 优先用 3.11+ 标准库 `tomllib`，降级用 `tomli`） |

> **依赖清单已失配（known gap）**：`Dockerfile:148-161` 仍安装 `python3-openai` /
> `httpx` / `pydantic` / `tenacity` / `fastapi` / `uvicorn` / `starlette`，这些是
> 已删除的 `audit_agent` / `audit_dashboard` 所需。实际随镜像发布的 hooker 脚本
> 只用标准库：对 `plugins/hookers/*/` 下全部 `*.py` 取 import 并集，第三方依赖
> 仅 `tomli`（`agent_moss/bridge.py:60`；`llm_pre_secret_guard` /
> `tool_post_secret_guard` / `cerberus_bash_control` 的脚本无第三方 import）。
> `agent_moss/install.sh:116-143` 走的是 `pip install agent-moss` 的独立 venv 流程，
> 与镜像内的系统级 preinstall 无关。修复 Dockerfile 时应据此精简该清单。

### 镜像元信息

| 指令 | 值 |
| --- | --- |
| `CMD` | `["xiaoo-daemon"]`（无 `ENTRYPOINT`；`docker run` 追加的参数整体覆盖 `CMD`） |
| `EXPOSE` | `18080 28081` |
| `VOLUME` | `["/root/.xiaoo"]` |
| `HEALTHCHECK` | `curl -fsS http://127.0.0.1:18080/api/v1/health`（`/api/v1/health` 绕过 bearer） |
| `ENV` | `RUST_LOG=info HOME=/root`（`XIAOO_CONFIG` 故意不设，留给运行时） |

> 镜像不设 `ENTRYPOINT`，直接靠 `CMD` 覆盖分流：`docker run xiaoo` →
> `xiaoo-daemon`（默认）；`docker run xiaoo xiaoo` → TUI；`docker run xiaoo
> xiaoo --cli run -p "..."` → 一次性 CLI；`docker run -it xiaoo bash` →
> 进 shell（`xiaoo` / `xiaoo-daemon` 均在 `PATH` 中可直接执行）。

---

## 2. 构建镜像

> ⚠️ **本命令当前会失败**，原因见文首缺陷表（`Dockerfile:56`、`:45-66`、`:118-126`）。
> 修复 Dockerfile 后再执行；在此之前不要期望构建产物存在。

```bash
docker build -t xiaoo:latest .
```

构建约 10–20 分钟（主要耗时在 `cargo fetch` + `cargo build --release`）。
国内网络通常无需任何 build-arg：`.cargo/config.toml` 走华为云 crates.io 镜像
（`Dockerfile:37` 将该文件拷为全局 cargo 配置）、Python 依赖走 openEuler dnf 仓库
（`Dockerfile:14-28` builder 工具链同样由 dnf 的华为云镜像提供）。

---

## 3. 使用镜像

镜像被设计成"一个镜像覆盖所有场景"。所有示例假设你已准备好 `my.toml`
配置文件。

### 配置挂载（二选一）

```bash
# A. 挂到默认路径（最简，无需环境变量）
-v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro

# B. 任意路径 + XIAOO_CONFIG
-v $PWD/my.toml:/cfg.toml:ro -e XIAOO_CONFIG=/cfg.toml
```

### Daemon 模式（默认）

```bash
docker run --rm -d -p 18080:18080 -p 28081:28081 \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... \
  xiaoo:latest
```

启动后：API `http://localhost:18080/api/v1/...`（除 `/api/v1/health` 外需
bearer token），dashboard `http://localhost:28081/`（只读，无需 token）。

daemon flags（显式写 `xiaoo-daemon` 覆盖默认 `CMD`）：

```bash
docker run --rm -d -p 18099:18080 -p 28081:28081 \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... \
  xiaoo:latest xiaoo-daemon --host 0.0.0.0 --dashboard-host 0.0.0.0
```

宿主机端口冲突时优先用 `-p <HOST_PORT>:18080` 端口映射，容器内 daemon 仍监听
默认 18080，镜像内置 `HEALTHCHECK`（探 `127.0.0.1:18080`）自动生效。

若确实要用 `--port` 改 daemon 监听端口，须同步用 `--health-cmd` 覆盖
`HEALTHCHECK`，否则 Docker 在 retries 耗尽后会把容器标 `unhealthy`：

```bash
docker run --rm -d -p 18099:18099 -p 28081:28081 \
  --health-cmd "curl -fsS http://127.0.0.1:18099/api/v1/health || exit 1" \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... \
  xiaoo:latest xiaoo-daemon --host 0.0.0.0 --port 18099 --dashboard-host 0.0.0.0
```

### TUI 模式（需 `-it`）

```bash
docker run --rm -it \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... \
  xiaoo:latest xiaoo
```

### CLI 一次性执行

```bash
docker run --rm \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... \
  xiaoo:latest xiaoo --cli run -p "Count the characters in hello world"
```

### 进容器排障

镜像里装了 `bash`、`curl`、`git`、`python3`，可直接 exec 进去或起一次性 shell：

```bash
# 起常驻容器后 exec 进去
docker run -d --name xiaoo-box -p 18080:18080 -p 28081:28081 \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -v xiaoo-data:/root/.xiaoo \
  -e OPENROUTER_API_KEY=sk-or-... xiaoo:latest
docker exec -it xiaoo-box bash

# 一次性 shell（覆盖默认 CMD）；`xiaoo` / `xiaoo-daemon` 均在 PATH 中
docker run --rm -it \
  -v $PWD/my.toml:/root/.config/xiaoo/config.toml:ro \
  -e OPENROUTER_API_KEY=sk-or-... xiaoo:latest bash
```

### 环境变量

| 变量 | 默认 | 用途 |
| --- | --- | --- |
| `XIAOO_CONFIG` | 未设置（走 `~/.config/xiaoo/config.toml`） | 显式指定配置文件 |
| `RUST_LOG` | `info` | 日志级别 |
| `HOME` | `/root` | 影响 `~/.xiaoo` / `~/.config/xiaoo` 解析 |
| provider 凭证 | — | `OPENROUTER_API_KEY` / `OPENAI_API_KEY` / `ZAI_API_KEY` / `FEISHU_APP_SECRET` 等，与配置中 `[llm].api_key_env` 对应 |
