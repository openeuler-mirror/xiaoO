# 遗留缺陷记录 — agent_moss bridge 未读取 `workspace`

> **状态**：未修复（本次为纯文档变更，不改代码）
> **类型**：字段名契约不一致（宿主 payload ↔ bridge）
> **影响面**：agent_moss 钩子在 xiaoO 接入场景下的**层2「间接文件访问检测」完全失效**
> **发现于**：2026-09 文档对齐（`docs/sync-with-code-2026-09`）
> **记录位置**：本文件；另在三处文档留下指向性说明（见文末「相关文档」）

---

## 1. 一句话结论

xiaoO 的 tool pre payload 用 **`workspace`** 字段携带工作区，而
`plugins/hookers/agent_moss/bridge.py` 读的是 **`cwd`** 字段。二者名字不匹配，
bridge 每次转发给 AgentMoss 的 `cwd` 都是**空串**，因此依赖 `cwd` 的检测规则拿不到
工作区，实际不触发。

## 2. 证据链

| 环节 | 位置 | 事实 |
|---|---|---|
| 宿主发送字段名 | `crates/hook/src/hookers/plugin/tool/adaptor.rs:292-304` | tool pre payload 组装为 `session_id` / **`workspace`** / `prompt_session` / `prompt_history` / `action_history` / `call` —— **没有 `cwd`** |
| 字段值来源 | `crates/hook/src/hookers/plugin/core.rs:250-255` | `workspace` 由 `serialize_workspace_root(runtime)` 生成 |
| bridge 读取字段名 | `plugins/hookers/agent_moss/bridge.py:358` | `"cwd": data.get("cwd", ""),` |
| 后果 | 同上 | `data` 中不存在 `cwd` → 恒取默认值 `""` → 转发给 AgentMoss 的 `cwd` 永远是空串 |
| 受害规则 | `plugins/hookers/agent_moss/docs/agent-moss-integration-design.md:539` | 规则表第 12 条「间接文件访问」：`os.listdir`/`os.walk` 遍历 `cwd` 以间接读取 `.env` 凭据 |

### 最小复现

```bash
# 1. 确认宿主 payload 字段名（应输出 workspace，无 cwd）
grep -n '"workspace"\|"cwd"' crates/hook/src/hookers/plugin/tool/adaptor.rs

# 2. 确认 bridge 读取的字段名（应输出 cwd）
grep -n 'data.get("cwd"' plugins/hookers/agent_moss/bridge.py

# 3. 构造宿主真实 payload（无 cwd）喂给 bridge 的字段解析逻辑，观察 cwd 结果
python3 - <<'PY'
data = {"session_id": "s", "workspace": "/home/user/workspace", "call": {"tool": "bash"}}
print("forwarded cwd =", repr(data.get("cwd", "")))   # -> ''
print("available workspace =", repr(data.get("workspace")))  # -> '/home/user/workspace'
PY
```

第 3 步输出 `forwarded cwd = ''`，而复现出「空串」即证明该字段被静默丢弃。

## 3. 影响评估

- **在 xiaoO 接入场景下**：层2「间接文件访问检测」失效。攻击者用
  `os.listdir`/`os.walk` 遍历工作区间接读取 `.env` 类凭据时，该规则不会命中。
- **层1 / 层3**：不受影响（不依赖 `cwd`）。
- **AgentMoss 作为独立服务**：不受影响。其他直接调用 analyze 接口、自带 `cwd` 的调用方
  （如 OpenDesk）行为正常。
- **是否会静默失败**：会。字段名不匹配不会产生任何报错或警告，`data.get(..., "")`
  的默认值把缺陷完全吞掉——这也是它长期未被发现的原因。

## 4. 修复方向

一行改动即可，把读取的键名改为宿主实际契约：

```diff
-        "cwd": data.get("cwd", ""),
+        "cwd": data.get("workspace", ""),
```

`bridge.py:358` 的 key 名（`cwd`）是 **AgentMoss analyze 接口** 的入参名，**不要改**；
只改它从宿主 payload 里取值的字段名（`cwd` → `workspace`）。

修复时建议一并处理：

1. **兼容旧**：若需兼容仍传 `cwd` 的调用方，可写成
   `data.get("workspace") or data.get("cwd", "")`。
2. **补测试**：给 bridge 加一条断言「宿主 payload（含 `workspace`、无 `cwd`）→ 转发
   `cwd` 等于该 workspace」，防止再次静默回退。
3. **契约对齐**：考虑在 `crates/hook` 的 payload 里同时提供 `cwd` 别名，或在
   `how-to-develop-a-plugin-hooker.md` 的 payload schema 中明确「工作区字段为
   `workspace`，第三方 hooker 请勿假设 `cwd`」——目前该文档已具备此说明。

## 5. 为何本次未修复

本次变更是**纯文档对齐**（分支 `docs/sync-with-code-2026-09`，不含任何 `.rs`/`.py`
改动）。按项目约定，代码与文档冲突且代码本身可疑时，文档记录**实际行为**并标注
known gap，不顺手改代码。该缺陷应作为独立变更修复，以便单独评审、测试与回归。

## 6. 相关文档

本缺陷在三处文档中以「已知缺口」形式记录了实际行为（均**不**声称已修复）：

- [`plugins/hookers/agent_moss/README.md`](../README.md) — payload 字段说明与字段表 `cwd` 行
- [`plugins/hookers/agent_moss/TECH_WIKI.md`](../TECH_WIKI.md) — 层2 流程图下方的缺口说明
- [`plugins/hookers/agent_moss/docs/agent-moss-integration-design.md`](agent-moss-integration-design.md) — 规则表第 12 条及其缺口说明

hooker payload 契约的通用说明见
[`plugins/hookers/how-to-develop-a-plugin-hooker.md`](../../how-to-develop-a-plugin-hooker.md)。
