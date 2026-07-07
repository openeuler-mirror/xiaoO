# Plugin Installation and Usage

## Cerberus Plugin (Optional)

Cerberus provides secure command execution with policy-based sandboxing. It is included in the workspace but requires the eBPF toolchain (Linux only).

```bash
# Install with eBPF support (default, requires nightly Rust + eBPF toolchain)
cargo install --path crates/cerberus/cerberus-cli

# Install without eBPF if toolchain is unavailable
cargo install --path crates/cerberus/cerberus-cli --no-default-features -p cerberus-core
```

If `cargo build --release` fails due to Cerberus/eBPF, you can skip it:

```bash
cargo build --release --workspace --keep-going
```

## Plugins

Pre-built hookers and skills are placed in `<your_xiaoO>/plugins`. They are **not installed by default**.

To install hookers, run:

```bash
cd <your_xiaoO>/plugins/hookers
./config.sh
```

You can also develop your own hookers and place them in `<your_xiaoO>/plugins/hookers`. See `how-to-develop-a-plugin-hooker.md` for details.

## Skills

### Built-in Skills

When you run `cargo install --path apps/endside`, builtin skills are automatically installed. They provide security policy enforcement and other built-in capabilities, and are loaded with highest priority by the runtime.

**Installation locations** (automatic fallback):
- **System level** (preferred): `/usr/lib/.xiaoo/skills/` - requires root privileges
- **User level** (fallback): `~/.xiaoo/skills/` - used if system-level installation fails

**Builtin skills** (located in `<xiaoO>/plugins/skills/`):
- `xiaoo-guardian` - Security policy enforcement
- `block-analyzer` - Block analysis capabilities

> **Note**: `cargo build` does NOT install skills. Only `cargo install` triggers skill installation.
>
> **Installation Behavior**:
> - First attempts to install all builtin skills to system-level directory (requires root privileges)
> - If system-level installation fails (e.g., permission denied), automatically falls back to user-level directory
> - Without these skills, security features and other capabilities may be unavailable.
>
> **For system-wide installation** (recommended for multi-user environments):
> - Run `cargo install` with root privileges: `sudo cargo install --path apps/endside`

### Skill Directory Priority (Four Levels)

1. **Project level** (highest): `./.xiaoo/skills/` - Project-specific skills
2. **Config level** (medium): Directories specified in `[skills].dirs` - Team/user shared skills
3. **User level**: `~/.xiaoo/skills/` - Personal skills available everywhere
4. **System level** (lowest): `/usr/lib/.xiaoo/skills/` - Built-in skills only

### Custom Skills

Custom skills can be installed to user-level directory using the `xiaoo --cli skill install` command:

```bash
# Install from local directory (installs to ~/.xiaoo/skills by default)
xiaoo --cli skill install ./my-skill/

# Install from Git repository
xiaoo --cli skill install https://github.com/user/my-skill.git
```

> **Note**: User-installed skills go to `~/.xiaoo/skills/` (user level), NOT `/usr/lib/.xiaoo/skills/` (system level is reserved for built-in skills).

See [skill_usage.md](./skill_usage.md) for detailed skill documentation.

### Uninstalling Skills

```bash
# Remove a user-installed skill
xiaoo --cli skill remove <skill-name>

# Or manually remove from user level
rm -rf ~/.xiaoo/skills/<skill-name>

# Remove built-in guardian skill (requires root)
sudo rm -rf /usr/lib/.xiaoo/skills/xiaoo-guardian
```

To completely uninstall xiaoO and all associated skills:

```bash
# Uninstall the application
cargo uninstall xiaoo-endside

# Remove system-level skills (requires root)
sudo rm -rf /usr/lib/.xiaoo/skills

# Remove user-level skills
rm -rf ~/.xiaoo/skills
```

## RPM 安装环境下的插件管理

通过 RPM 安装 xiaoO-hookers 后，所有插件安装到 `/usr/lib/.xiaoo/hookers/` 目录下。插件可以通过以下两种方式启用和关闭。

### 方式一：使用软链接命令（推荐）

RPM 安装时会创建两个全局软链接，可直接在任意目录执行：

```bash
# 交互式安装/启用插件
xiaoo-hookers-install

# 卸载/禁用插件
xiaoo-hookers-uninstall
```

也支持非交互模式：

```bash
# 启用 audit_agent（不交互，直接写入配置）
xiaoo-hookers-install --non-interactive audit_agent
```

### 方式二：直接编辑 config.toml

手动编辑 `~/.config/xiaoo/config.toml` 的 `[hooker]` 段。

**场景 1：启用所有插件**

```toml
[hooker]
default = "All"
plugins = [
  "/usr/lib/.xiaoo/hookers/audit_agent/plugin.json",
]
```

`default = "All"` 表示所有已注册的插件默认启用，`plugins` 列表中的 plugin.json 会被加载注册。

**场景 2：启用所有插件，但排除个别不需要的**

```toml
[hooker]
default = "All"
plugins = [
  "/usr/lib/.xiaoo/hookers/audit_agent/plugin.json",
  "/usr/lib/.xiaoo/hookers/tool_post_secret_guard/plugin.json",
]
disabled = [
  "pluginguard_secret_like_output",
]
```

`disabled` 列表填写 plugin.json 中定义的 `id` 字段值。

### RPM 场景下插件 install.sh 的变化

RPM 打包时，部分插件自身的 `install.sh` 行为会有所调整。以 audit_agent 为例：

- **开发者场景**（git clone + cargo build）：执行 `install.sh` 时会创建 Python 虚拟环境（venv），并通过 pip 安装依赖
- **RPM 场景**（通过 RPM 安装）：audit_agent 的 Python 依赖已通过 RPM 包提供（`python3-openai`、`python3-httpx`、`python3-pydantic`、`python3-tenacity` 等），因此 `install.sh` **不再创建 venv 和 pip install**，只负责生成 `audit_settings.json` 配置文件

这意味着在 RPM 场景下执行 `xiaoo-hookers-install` 不会因为缺少 pip 或 venv 而出错。

### 验证插件是否生效

```bash
# 查看 xiaoo 运行日志，确认插件已加载
xiaoo run -p "echo hello"

# 查看审计日志（audit_agent）
cat /usr/lib/.xiaoo/hookers/audit_agent/audit_policy_checker.log
```

### 注意事项

- **路径使用绝对路径**：`plugins` 列表中的路径必须是 plugin.json 的完整绝对路径，RPM 安装后路径为 `/usr/lib/.xiaoo/hookers/<plugin>/plugin.json`
- **`disabled` 使用 hooker id**：`disabled` 列表填写的是 plugin.json 中 `id` 字段的值（如 `plugin_audit_tool_input`、`pluginguard_secret_like_output`），不是文件路径
- **修改配置后无需重启**：xiaoo 每次启动时重新读取配置

## Chat & Session Lifecycle Hooks

除 Tool 层 hook（`*.Tool.*.pre/post/error`）外，xiaoo 还提供 **chat 层** 与 **session 层** 两个挂载点，让外部插件能在用户输入、system prompt 组装、会话状态流转等阶段介入。它们与 Tool hook 共用同一套 `plugin.json` + `sh -c` 子进程 + stdin/stdout JSON 协议，可注册在同一个 `~/.xiaoo/hookers/<name>/plugin.json` 数组里。

### Hook points overview

| Hook point | 触发时机 | 插件可返回结果 |
|---|---|---|
| `*.Chat.command.before` | 用户输入命中 `~/.xiaoo/commands/<name>.md` 斜杠命令、模板展开为 body 之后、body 提交为 user turn 之前 | `Allow` / `Transform { body }` / `Deny { reason }` |
| `*.Chat.message.received` | user 消息构造完成、写入消息历史之前 | `Accept` / `Transform { message }` |
| `*.Chat.system.transform` | PromptBuilder 组装完 `system: Vec<String>` 分段、合并成单条 system 消息之前 | `Allow` / `Transform { system }` |
| `*.Session.lifecycle.state` | 一次非错误 root turn 结束（`Complete`/`MaxTurnsReached`/`BudgetExhausted`/`Cancelled` 四种 `Ok` 结局）、会话回到 `idle` 时（fire-and-forget，不阻塞 turn 返回） | `Ack`（事件型，无可变输出） |

> 前三个 chat hook 是「可变 hook」——插件可以改写或拒绝输入；第四个 session hook 是「事件型观察者」——只能确认收到事件，没有 `transform`/`deny` 路径。

### 与 Tool hook 的差异

- **执行模型相同**：都是 `sh -c <command>`，stdin 写入一次 JSON payload、stdout 读取一次 JSON 结果，非零退出视为失败。
- **`payload.stage` 不同**：chat/session hook 的 stage 字符串是 `command_before` / `chat_message` / `system_transform` / `session_state`（不是 `pre`/`post`/`error`），脚本据此分发。
- **调度差异**：chat hook 在 agent loop 同步执行，单个插件报错只 `tracing::warn!` 不中断整轮，只有 `command.before` 的 `Deny` 会短路；session state hook 在 gateway 后台 `tokio::spawn` 执行（fire-and-forget），错误走 `tracing::warn!`，绝不影响主流程或 `run_turn` 返回值。
- **交互机制**：三个 chat hook 还支持 `action: "ask_user"`——插件可发起 `Confirm` / `TextInput` / `Choice` 交互，用户回答后 xiaoo 会带着 `interaction` 字段再次调用同一命令，直到插件返回 `final`。session state hook 不支持该机制。

### Minimal plugin.json

`~/.xiaoo/hookers/js-test/plugin.json` 一次性注册四个 hooker，全指向同一个脚本：

```json
[
  { "id": "js_test_system_transform", "hook_point": "*.Chat.system.transform",    "command": "node ~/.xiaoo/hookers/js-test/xiaoo-hook-test.js" },
  { "id": "js_test_chat_message",     "hook_point": "*.Chat.message.received",   "command": "node ~/.xiaoo/hookers/js-test/xiaoo-hook-test.js" },
  { "id": "js_test_command_before",   "hook_point": "*.Chat.command.before",     "command": "node ~/.xiaoo/hookers/js-test/xiaoo-hook-test.js" },
  { "id": "js_test_session_state",    "hook_point": "*.Session.lifecycle.state", "command": "node ~/.xiaoo/hookers/js-test/xiaoo-hook-test.js" }
]
```

### stage 取值表

`payload.stage` 是 xiaoo 在构造 payload 时硬编码写入的判别字符串，**不等于** `hook_point` 的最后一段：

| `hook_point`（plugin.json 里写的） | payload 里的 `stage` |
|---|---|
| `*.Chat.system.transform` | `system_transform` |
| `*.Chat.message.received` | `chat_message` |
| `*.Chat.command.before` | `command_before` |
| `*.Session.lifecycle.state` | `session_state` |

### session_id 获取

插件需要当前会话 id 时，直接读 `payload.session_id` 即可，**无需区分 local / remote 模式**：

- **local 模式**：TUI 进程内跑 agent loop，hooker 子进程由 TUI 拉起，`payload.session_id` 就是 TUI 的 `state.session_id`（UUID）。
- **remote 模式**：TUI 把自己的 `session_id` 通过 `RuntimeTurnRequest` 发给 daemon，daemon 原样使用；hooker 子进程由 daemon 拉起，`payload.session_id` 与 local 模式下是**同一个值**。

因此插件不必判断"我现在是被 TUI 还是 daemon 拉起的"，`payload.session_id` 永远是「当前会话」的正确 id。

各 hook 点 `payload.session_id` 可用性：

| hook_point | payload.session_id | 类型 |
|---|---|---|
| `*.Chat.command.before` | ✓ | String |
| `*.Chat.message.received` | ✓ | String |
| `*.Chat.system.transform` | ✓ | String \| null |
| `*.Session.lifecycle.state` | ✓ | String |
| `*.Tool.*.pre` | ✓ | String |
| `*.Tool.*.post` / `error` | ✗ | payload 不带 session_id |
| `*.Llm.*.pre` / `post` / `error` | ✗ | payload 不带 session_id |

> `system_transform` 的 session_id 理论上可能为 `null`（`ChatSystemTransformInput.session_id` 是 `Option<String>`），脚本里建议 `payload.session_id || "(unknown)"` 兜底。tool post/error 与 LLM hook 的 payload 目前不带 session_id；如需在这些钩子里拿会话 id，可考虑改用 `*.Tool.*.pre` 或 chat 层钩子记录映射。

### Minimal demo script

最简 demo（`.js`）：每个 hook 都演示**如何获取 session id** 并做一次**用户可感知**的改写——`[sid=<session_id>]` 标记会出现在用户消息、system 指令和 `/tmp/xiaoo-demo.log` 日志里，便于核对每个钩子各自拿到的会话 id（`command_before` 额外拼 `payload.command`/`payload.arguments`，`chat_message` 额外拼 `payload.prior_message_count` 判定首条消息）：

```js
#!/usr/bin/env node
// xiaoo hooker demo (js-test): 在每个 hook 点演示如何获取当前 session id。
// 直接读 payload.session_id 即可，无需区分 local/remote 模式（两种模式下
// 是同一个值——TUI 把 session_id 发给 daemon，daemon 原样使用）。
// stdout 只写结果 JSON；日志走 stderr 或文件。

const fs = require("fs");
const payload = JSON.parse(fs.readFileSync(0, "utf8") || "{}");

// 统一取 session_id：system_transform 的该字段可能为 null，做兜底。
const SID = payload.session_id || "(unknown)";

// 每个 hook 触发时都往日志写一行，便于核对四个钩子各自拿到的 sid。
fs.appendFileSync("/tmp/xiaoo-demo.log",
  `[${new Date().toISOString()}] stage=${payload.stage} sid=${SID}\n`);

let result;
switch (payload.stage) {
  case "command_before": {
    // payload.session_id = 当前会话 id
    // payload.command    = 斜杠命令名（如 "hook-test"），仅斜杠命令入口进此 stage
    // payload.arguments  = 原始参数串（如 "aaa"，无参为空串）
    // payload.body       = 模板展开后的 body
    const cmd = payload.command || "";
    const args = payload.arguments || "";
    const tag = cmd ? `[HOOK:command_before:${cmd}${args ? ` ${args}` : ""}]` : "[HOOK:command_before]";
    result = { result: "transform", body: `${tag} [sid=${SID}] ${payload.body || ""}` };
    break;
  }
  case "chat_message": {
    // payload.session_id = 当前会话 id
    // payload.prior_message_count = 当前用户消息落库前的历史消息数：
    //   全新 session 首条 = 0；首轮完成后再发 = 2；只回放了 user 的恢复 = 1。
    //   用 <= 1 判定「会话首条有效输入」（兼容 retry/中断恢复）。
    const count = payload.prior_message_count ?? 0;
    const tag = count <= 1
      ? `[HOOK:chat_message:first#${count}]`
      : `[HOOK:chat_message:follow-up#${count}]`;
    const blocks = (payload.message?.blocks || []).map((b) =>
      b.type === "text" ? { ...b, text: `${tag} [sid=${SID}] ${b.text}` } : b
    );
    result = { result: "transform", message: { ...payload.message, blocks } };
    break;
  }
  case "system_transform": {
    // payload.session_id = 当前会话 id（可能为 null，已在顶部兜底）
    // 往 system 数组追加一条指令 → LLM 回复会以 [HOOK:system_transform] 开头
    const sys = Array.isArray(payload.system) ? payload.system : [];
    result = { result: "transform", system: [...sys, `回复必须以 [HOOK:system_transform] 开头（sid=${SID}）`] };
    break;
  }
  case "session_state": {
    // payload.session_id = 当前会话 id（事件型、只读）
    // payload.state      = "idle"（一轮非错误结束）
    // payload.outcome    = complete / max_turns_reached / budget_exhausted / cancelled
    if (payload.state === "idle" && payload.outcome === "max_turns_reached") {
      const newSession = `cont-${Date.now()}`;
      result = {
        result: "ack",
        actions: [
          { kind: "create_session", session_id: newSession },
          { kind: "switch_session", session_id: newSession }
        ]
      };
    } else {
      result = { result: "ack" };
    }
    break;
  }
  default:
    result = { result: "allow" };
}

// 当某个 case 被注释掉、result 仍为 undefined 时，按 stage 回退到对应
// 的 no-op 结果，避免 JSON.stringify(undefined) → process.stdout.write(undefined)
// 抛 ERR_INVALID_ARG_TYPE 让整个 hooker 退出状态 1、被宿主当成失败跳过。
if (result === undefined) {
  const noop = {
    command_before: { result: "allow" },
    chat_message: { result: "accept" },
    system_transform: { result: "allow" },
    session_state: { result: "ack" },
  };
  result = noop[payload.stage] ?? { result: "allow" };
}
process.stdout.write(JSON.stringify(result));
```

运行后核对 session id：

```bash
# 四个钩子每次触发都会写一行，可直接看到各钩子拿到的 sid
cat /tmp/xiaoo-demo.log
# [2026-...Z] stage=command_before sid=550e8400-e29b-...
# [2026-...Z] stage=chat_message sid=550e8400-e29b-...
# [2026-...Z] stage=system_transform sid=550e8400-e29b-...
# [2026-...Z] stage=session_state sid=550e8400-e29b-...
```

> local 与 remote 模式下 `sid` 值一致；remote 模式下日志写在 **daemon 所在机器**（hooker 由 daemon 进程拉起），不是 TUI 机器。

完整的 payload 字段、各 hook 合法输出 JSON 形状、`ask_user` 交互协议细节，请参考 [`plugins/hookers/how-to-develop-a-plugin-hooker.md`](../plugins/hookers/how-to-develop-a-plugin-hooker.md) 中的 Chat hook 与 Session lifecycle state hook 章节。

### Plugin-requested session actions

`*.Session.lifecycle.state` hook 的响应里除了 `result: "ack"`，还可以携带一个 `actions` 数组，让插件请求宿主在 turn 结束之后执行副作用——目前支持两种：

| `kind` | 必填字段 | daemon 侧行为 | TUI 侧行为 |
|---|---|---|---|
| `create_session` | `session_id` | 调用 `open_session`（幂等恢复）打开/恢复该会话 | 切换焦点到该会话并恢复历史 transcript |
| `switch_session` | `session_id` | 调用 `open_session`（幂等恢复）确保目标会话存在 | 切换焦点到该会话并恢复历史 transcript |

响应 JSON 形状：

```json
{
  "result": "ack",
  "actions": [
    { "kind": "create_session", "session_id": "debug-1" },
    { "kind": "switch_session", "session_id": "debug-1" }
  ]
}
```

要点：

- 只有 `*.Session.lifecycle.state` hook 的 adaptor 会解析 `actions`；chat/tool hook 的响应里写 `actions` 会被忽略。
- session state hook 从 fire-and-forget 改为 **awaited**：dispatcher 会等所有 hooker 跑完、收集 `actions` 后才把 `Done` 事件发给 TUI，所以慢的 hooker 会延迟 `Done`——`actions`-emitting hooker 务必保持快。
- daemon 侧先 `open_session`，失败的 action 被 `tracing::warn!` 记录并过滤，不会转发到 TUI；TUI 侧再异步切换焦点并恢复 transcript。
- 整条链路 best-effort：任何一层失败都不会向 hook 调用者回传错误。
- `kind` 用 `snake_case`，未知 `kind` 或缺字段的条目会被静默跳过——单个坏条目不会污染数组其它条目。
- 宿主强制 `max action depth = 3` 防 `hook→action→hook` 递归：超过上限的批次会被截断（保留前 3 条，多余的条目被 `tracing::warn!` 记录后丢弃）。插件作者请按此硬上限设计。

完整字段定义、daemon/TUI 两端执行流程、最小示例与排错清单，请参考 [`plugins/hookers/how-to-develop-a-plugin-hooker.md`](../plugins/hookers/how-to-develop-a-plugin-hooker.md) 第 16 章。
