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

# 在导入 audit_policy_checker 之前，先把插件根目录（audit.py 所在目录）
# 注入环境变量。audit.py 始终位于插件根目录（不被 pip 装进 venv），
# 是唯一可靠的"插件根目录锚点"——无论 pip 安装还是 RPM 源码直跑，
# 都能用它定位到统一的 audit_settings.json 位置（插件根目录）。
_PLUGIN_ROOT = os.path.dirname(os.path.abspath(__file__))
os.environ.setdefault("AUDIT_PLUGIN_ROOT", _PLUGIN_ROOT)

from audit_policy_checker.config import get_log_path  # noqa: E402
from audit_policy_checker.main import audit_action  # noqa: E402


def _resolve_log_path() -> str:
    """
    解析日志路径，优先级：环境变量 > audit_settings.json > /dev/null。

    与设置环境变量效果一致：在 audit_settings.json 中配置 AUDIT_LOG_PATH
    即可让 HOOK_INPUT/HOOK_OUTPUT 等全量日志写入指定文件。
    get_log_path 内部已实现"环境变量 > settings > 默认"，此处再兜底 /dev/null。
    """
    try:
        path = get_log_path()
        if not path:
            path = os.environ.get("AUDIT_LOG_PATH", "")
        return path or "/dev/null"
    except (json.JSONDecodeError, OSError) as exc:
        # audit_settings.json 损坏或不可读时，安全降级到 /dev/null，
        # 避免 JSON 解析异常导致 audit.py 在模块导入阶段直接崩溃，
        # 影响面从"日志路径错"扩大到"整个 hook 失效"。
        print(f"[run_audit] audit_settings.json 读取失败，日志降级到 /dev/null: {exc}", file=sys.stderr)
        return os.environ.get("AUDIT_LOG_PATH", "") or "/dev/null"


_LOG_PATH = _resolve_log_path()


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
    # 对于文件操作工具，只提取 file_path 字段，避免 content 中的文本触发敏感路径误报
    # 对于 skill 工具，只提取 skill 名称，避免 args（自然语言描述）中的文本触发敏感路径误报
    if isinstance(tool_input, dict):
        if tool_name.lower() == "bash" and "command" in tool_input:
            action_detail = str(tool_input.get("command", ""))
        elif tool_name.lower() in ("file_write", "file_edit", "file_read") and "file_path" in tool_input:
            action_detail = str(tool_input.get("file_path", ""))
        elif tool_name.lower() == "skill" and "skill" in tool_input:
            action_detail = str(tool_input.get("skill", ""))
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
    code = main()
    # 用 os._exit 而非 sys.exit：跳过解释器清理，不等非守护线程。
    # audit_agent 的 L3 用 ThreadPoolExecutor 调 LLM，worker 是非守护线程；
    # 当 call_llm 卡死（http 超时失效）且 future.result 超时后，worker 仍永久阻塞，
    # sys.exit 会在进程退出阶段等它 → 卡死。os._exit 立即退出，worker 随进程消亡。
    # audit.py 是一次性 hook 脚本，无 atexit/需清理资源，flush stdout 后直接退出安全。
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(code)
