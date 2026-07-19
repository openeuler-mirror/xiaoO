#!/usr/bin/env python3
"""xiaoO plugin-hook bridge → AgentMoss HTTP service.

替代 xiaoO/plugins/hookers/audit_agent/audit.py 的桥接脚本（audit_agent 是
AgentMoss 的前身，已迁出为独立常驻服务）。

契约（与 audit.py 完全对齐，便于无感替换）：
  - xiaoO 在 tool-pre hook 触发时，经 stdin 发送 JSON payload
  - 本脚本：
      1. 活性检查：探 AgentMoss /api/v1/health，确认服务常驻可达
         （不可达时 fail-closed Deny，并给清晰错误引导，避免"服务挂了静默放行"）
      2. 解析 payload，提取 tool_name/tool_input → action_type/action_detail
         （提取规则与 audit.py 一致：bash→command，file_*→file_path，skill→skill 名）
      3. POST 到 AgentMoss 的 /api/v1/analyze（127.0.0.1:9090）
      4. 将 AnalyzeResponse 映射为 PreHookResult JSON：
           {"result": "allow", "reason": "..."}
           {"result": "deny",  "reason": "..."}
      5. stdout 输出 PreHookResult JSON（退出码始终 0，hook 模式约定）

  - AgentMoss 作为常驻 HTTP 服务（systemd unit / pip 装后自启），比每次
    spawn audit.py 子进程快得多（无 venv 启动/import 开销，无 ThreadPoolExecutor
    卡死风险）。

活性检查设计（需求：用户装好 agent_moss 插件后要能看服务是否开/源码是否在）：
  - 启动时先 GET http://{HOST}:{PORT}/api/v1/health（超时 2s）
  - 200 healthy → 继续走 analyze
  - 连接拒绝（ECONNREFUSED）→ 服务没起，给"启动命令 + systemd 提示"
  - 超时/其他 → 网络或服务异常，给"检查日志/端口"提示
  - 源码缺失检查：env AGENT_MOSS_CHECK_SOURCE=1 时，额外检查
    /usr/lib/agent_moss/ 或 PYTHONPATH 里的 agent_moss 包是否 import 得到
    （openEuler RPM 装在 /usr/lib/agent_moss/，pip 装在 site-packages）

环境变量：
  AGENT_MOSS_HOST  AgentMoss 监听地址（默认 127.0.0.1）
  AGENT_MOSS_PORT  AgentMoss 监听端口（默认 9090）
  AGENT_MOSS_HOOK_LOG  桥接日志路径（默认 空=不记）
  AGENT_MOSS_HOOK_TIMEOUT  HTTP 请求超时秒（默认 60）
  AGENT_MOSS_HEALTH_TIMEOUT  活性检查超时秒（默认 2）
  AGENT_MOSS_CHECK_SOURCE  =1 时额外检查源码可 import（默认 空=不查）

依赖：仅 stdlib（urllib + socket）。xiaoO venv 无需额外安装。
"""

import json
import os
import socket
import sys
import time
import urllib.request
import urllib.error
from datetime import datetime

_HOST = os.getenv("AGENT_MOSS_HOST", "127.0.0.1")
_PORT = os.getenv("AGENT_MOSS_PORT", "9090")
_TIMEOUT = float(os.getenv("AGENT_MOSS_HOOK_TIMEOUT", "60"))
_HEALTH_TIMEOUT = float(os.getenv("AGENT_MOSS_HEALTH_TIMEOUT", "2"))
_LOG_PATH = os.getenv("AGENT_MOSS_HOOK_LOG", "")
_CHECK_SOURCE = os.getenv("AGENT_MOSS_CHECK_SOURCE", "") == "1"


def _log(tag: str, payload: object) -> None:
    if not _LOG_PATH:
        return
    try:
        line = f"[{datetime.now().isoformat(timespec='milliseconds')}] [{tag}] {json.dumps(payload, ensure_ascii=False)}\n"
        with open(_LOG_PATH, "a", encoding="utf-8") as f:
            f.write(line)
    except Exception as exc:
        print(f"[agent_moss_bridge] log write failed: {exc}", file=sys.stderr)


def _check_source_install() -> str | None:
    """检查 AgentMoss 源码是否可 import（openEuler RPM 装在 /usr/lib/agent_moss/，
    pip 装在 site-packages）。返回 None=正常，否则返回缺失原因字符串。
    """
    try:
        import importlib.util
        spec = importlib.util.find_spec("agent_moss")
        if spec is None:
            return "agent_moss Python 包未找到（RPM 未装 / pip 未装 / PYTHONPATH 未含）"
        return None  # 找到了
    except Exception as exc:
        return f"agent_moss 源码检查异常: {exc}"


def _check_service_health() -> tuple[bool, str]:
    """活性检查：探 AgentMoss /api/v1/health。

    Returns:
        (ok, message)：ok=True 服务常驻可达；ok=False 不可达，message 是人可读引导。
    """
    url = f"http://{_HOST}:{_PORT}/api/v1/health"
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=_HEALTH_TIMEOUT) as resp:
            if resp.status == 200:
                data = json.loads(resp.read().decode("utf-8"))
                if data.get("status") == "healthy":
                    return True, f"AgentMoss 服务健康（{data.get('version', '?')}）"
                return False, f"AgentMoss 服务响应非 healthy: {data}"
            return False, f"AgentMoss 服务 HTTP {resp.status}"
    except urllib.error.URLError as exc:
        # 区分连接拒绝（服务没起）vs 超时（网络/服务异常）
        reason = str(exc)
        if isinstance(exc.reason, ConnectionRefusedError) or "Connection refused" in reason:
            return False, (
                f"AgentMoss 服务未启动（{_HOST}:{_PORT} 拒绝连接）。"
                f"请先启动：systemctl start agent-moss（openEuler RPM）"
                f" 或 agent-moss-server（pip 装包），或设 AGENT_MOSS_HOST/PORT 指向远端服务"
            )
        if "timed out" in reason.lower():
            return False, (
                f"AgentMoss 服务超时（{_HEALTH_TIMEOUT}s）：检查 {_HOST}:{_PORT} 是否正确、"
                f"服务日志/端口占用。systemctl status agent-moss 或 journalctl -u agent-moss"
            )
        return False, f"AgentMoss 服务不可达: {exc}"
    except Exception as exc:
        return False, f"AgentMoss 活性检查异常: {exc}"


def _extract_action_detail(tool_name: str, tool_input) -> str:
    """与 audit.py 一致的 action_detail 提取规则。

    bash → command 字段；file_write/edit/read → file_path；skill → skill 名；
    其他 → 整个 input 的 JSON。
    """
    if isinstance(tool_input, dict):
        tn = tool_name.lower()
        if tn == "bash" and "command" in tool_input:
            return str(tool_input.get("command", ""))
        if tn in ("file_write", "file_edit", "file_read") and "file_path" in tool_input:
            return str(tool_input.get("file_path", ""))
        if tn == "skill" and "skill" in tool_input:
            return str(tool_input.get("skill", ""))
        return json.dumps(tool_input, ensure_ascii=False)
    return str(tool_input)


def _handle_hook_payload(data: dict) -> int:
    # === Step 0: 活性检查 ===
    # AgentMoss 不可达时 fail-closed Deny，避免"服务挂了静默放行所有危险操作"。
    # 先探服务；再（可选）探源码。两步分报，让用户一眼看清是服务没起还是源码没装。
    health_ok, health_msg = _check_service_health()
    if not health_ok:
        details = [health_msg]
        if _CHECK_SOURCE:
            src_issue = _check_source_install()
            if src_issue:
                details.append(f"源码检查: {src_issue}")
        reason = "；".join(details)
        hook_result = {
            "result": "deny",
            "reason": f"[AgentMoss 活性检查失败] {reason}。按 fail-closed 原则拒绝",
        }
        _log("HOOK_HEALTH_FAIL", {"host": _HOST, "port": _PORT, "msg": reason})
        print(json.dumps(hook_result, ensure_ascii=False))
        return 0
    _log("HOOK_HEALTH_OK", {"host": _HOST, "port": _PORT})

    # === Step 1: payload 解包 ===
    call = data.get("call", {})
    tool_name: str = call.get("tool_name", "unknown_tool")
    tool_input = call.get("input", {})

    action_detail = _extract_action_detail(tool_name, tool_input)
    a_next = {"action_type": tool_name, "action_detail": action_detail}

    session_id: str = data.get("session_id") or call.get("call_id", "unknown-session")
    prompt_session: str = data.get("prompt_session", "")
    action_history: list = data.get("action_history", [])
    reason: str = data.get("reason", "")

    request_body = {
        "session_id": session_id,
        "prompt_session": prompt_session,
        "action_history": [
            {"action_type": h.get("name", h.get("action_type", "")),
             "action_detail": h.get("action_detail", "")}
            for h in action_history if isinstance(h, dict)
        ],
        "a_next": a_next,
        "reason": reason,
        "os_type": "",      # 让 AgentMoss 自动检测
        "cwd": data.get("cwd", ""),
        "agent_id": "xiaoo",  # 告知 AgentMoss 加载 xiaoO 专属规则
    }

    _log("HOOK_INPUT", {
        "session_id": session_id, "tool_name": tool_name,
        "reason": reason, "action_history_len": len(action_history),
    })

    # === Step 2: POST /api/v1/analyze ===
    url = f"http://{_HOST}:{_PORT}/api/v1/analyze"
    req = urllib.request.Request(
        url,
        data=json.dumps(request_body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as resp:
            result = json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as exc:
        # 活性检查过了但 analyze 不可达：服务在 analyze 阶段挂了或超时。
        hook_result = {
            "result": "deny",
            "reason": f"AgentMoss /api/v1/analyze 不可达（{exc}），按 fail-closed 原则拒绝",
        }
        _log("HOOK_OUTPUT", {"tool_name": tool_name, "hook_result": hook_result, "error": str(exc)})
        print(json.dumps(hook_result, ensure_ascii=False))
        return 0
    except Exception as exc:
        hook_result = {"result": "deny", "reason": f"AgentMoss 调用异常: {exc}"}
        _log("HOOK_OUTPUT", {"tool_name": tool_name, "hook_result": hook_result, "error": str(exc)})
        print(json.dumps(hook_result, ensure_ascii=False))
        return 0

    # === Step 3: 映射 PreHookResult ===
    decision = result.get("decision", "Deny")
    if decision == "Allow":
        hook_result = {"result": "allow", "reason": result.get("reason", "")}
    else:
        reason_text = result.get("violated_policy") or result.get("reason", "")
        hook_result = {"result": "deny", "reason": reason_text}

    _log("HOOK_OUTPUT", {"tool_name": tool_name, "hook_result": hook_result, "audit_result": result})
    print(json.dumps(hook_result, ensure_ascii=False))
    return 0


def main() -> int:
    raw = sys.stdin.read().strip()
    if not raw:
        print(json.dumps({"error": "stdin 为空，请传入 JSON 字符串"}, ensure_ascii=False))
        return 2
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        print(json.dumps({"error": f"JSON 解析失败: {exc}"}, ensure_ascii=False))
        return 2
    return _handle_hook_payload(data)


if __name__ == "__main__":
    code = main()
    sys.stdout.flush()
    sys.stderr.flush()
    # 用 os._exit：hook 脚本无 atexit 资源，flush 后直接退出。
    os._exit(code)
