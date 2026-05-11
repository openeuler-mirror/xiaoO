#!/usr/bin/env python3
"""plugin-hook bridge for auditagent.

plugin hook
-----------------------------------------
xiaoO 在触发 tool-pre hook 时通过 stdin 发送 JSON：

脚本将：
  1. 将 hook payload 转换为 auditagent 输入格式
  2. 调用 audit_action() 执行安全审计
  3. 向 xiaoO 返回 PreHookResult JSON：
       {"result": "allow"}
     或
       {"result": "deny", "reason": "..."}

退出码（直接调用模式）：
  0 → Allow
  1 → Deny
  2 → 输入错误

退出码（hook 模式）：
  0 → 始终为 0，结果通过 stdout JSON 传给 xiaoO
"""

import json
import os
import sys
from datetime import datetime

from audit_policy_checker.main import audit_action

_LOG_PATH = os.environ.get("AUDIT_LOG_PATH", "/dev/null")


def _log(tag: str, payload: object) -> None:
    """将带时间戳的 tag + JSON payload 追加写入日志文件。"""
    try:
        line = f"[{datetime.now().isoformat(timespec='milliseconds')}] [{tag}] {json.dumps(payload, ensure_ascii=False)}\n"
        with open(_LOG_PATH, "a", encoding="utf-8") as f:
            f.write(line)
    except Exception as exc:  # noqa: BLE001
        print(f"[run_audit] log write failed: {exc}", file=sys.stderr)


# ---------------------------------------------------------------------------
# Hook 模式（模式 B）
# ---------------------------------------------------------------------------


def _handle_hook_payload(data: dict) -> int:
    """处理来自 xiaoO 的 plugin-hook payload，返回 PreHookResult JSON。"""
    call = data.get("call", {})
    tool_name: str = call.get("tool_name", "unknown_tool")
    tool_input = call.get("input", {})

    metadata = data.get("metadata", {})
    trace_id = metadata.get("trace_id", "unknown_trace_id")
    span_id = metadata.get("span_id", "unknown_span_id")

    # 将 tool input 序列化为字符串作为 action_detail
    # 对于 Bash 工具，提取 command 字段以正确检测危险命令
    if isinstance(tool_input, dict):
        if tool_name.lower() == "bash" and "command" in tool_input:
            action_detail = str(tool_input.get("command", ""))
        else:
            action_detail = json.dumps(tool_input, ensure_ascii=False)
    else:
        action_detail = str(tool_input)

    a_next = {
        "action_type": tool_name,
        "action_detail": action_detail,
    }

    session_id: str = data.get("session_id") or call.get("call_id", "unknown-session")
    prompt_session: str = data.get("prompt_session", "")
    action_history: list = data.get("action_history", [])
    reason: str = data.get("reason", "")

    _log(
        "HOOK_INPUT",
        {
            "session_id": session_id,
            "tool_name": tool_name,
            "tool_input": tool_input,
            "reason": reason,
            "action_history_len": len(action_history),
        },
    )

    # 调用 auditagent
    result = audit_action(
        session_id=session_id,
        prompt_session=prompt_session,
        action_history=action_history,
        a_next=a_next,
        reason=reason,
    )

    # 向 xiaoO 返回 PreHookResult JSON
    if result["decision"] == "Allow":
        hook_result = {"result": "allow", "reason": result.get("reason", "")}
    else:
        reason_text = result.get("violated_policy") or result.get("reason", "")
        hook_result = {"result": "deny", "reason": reason_text}

    _log(
        "HOOK_OUTPUT",
        {"tool_name": tool_name, "hook_result": hook_result, "audit_result": result},
    )

    print(json.dumps(hook_result, ensure_ascii=False))
    return 0  # hook 模式始终以 0 退出；xiaoO 通过 stdout JSON 获取决策


# ---------------------------------------------------------------------------
# 入口
# ---------------------------------------------------------------------------


def main() -> int:
    raw = sys.stdin.read().strip()
    if not raw:
        err = {"error": "stdin 为空，请传入 JSON 字符串"}
        _log("ERROR", err)
        print(json.dumps(err, ensure_ascii=False))
        return 2

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        err = {"error": f"JSON 解析失败: {exc}"}
        _log("ERROR", err)
        print(json.dumps(err, ensure_ascii=False))
        return 2

    return _handle_hook_payload(data)


if __name__ == "__main__":
    sys.exit(main())
