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
  AGENT_MOSS_LOG_PATH  桥接全量 hook 日志路径（默认 空=不记）。每次 hook 调用
                       记 HOOK_INPUT/HOOK_OUTPUT（含 tool_input、判定结果），
                       对应 audit_agent 的 AUDIT_LOG_PATH。
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
from pathlib import Path

try:
    import tomllib  # Python 3.11+ stdlib
except ModuleNotFoundError:  # pragma: no cover
    try:
        import tomli as tomllib  # 3.10- fallback
    except ModuleNotFoundError:
        tomllib = None

_HOST = os.getenv("AGENT_MOSS_HOST", "127.0.0.1")
_PORT = os.getenv("AGENT_MOSS_PORT", "9090")
# 完整 URL env（最高优先级，与 OpenDesk hook.ts AGENT_GATE_URL 一致）。
# 形如 http://10.0.0.5:9095，设了就用它，跳过 host/port 拼装和探测。
_URL_ENV = os.getenv("AGENT_MOSS_URL", "")
_TIMEOUT = float(os.getenv("AGENT_MOSS_HOOK_TIMEOUT", "60"))
_HEALTH_TIMEOUT = float(os.getenv("AGENT_MOSS_HEALTH_TIMEOUT", "2"))
_LOG_PATH = os.getenv("AGENT_MOSS_LOG_PATH", "")
_CHECK_SOURCE = os.getenv("AGENT_MOSS_CHECK_SOURCE", "") == "1"

# 探测端口范围（学 OpenDesk hook.ts GATE_PROBE_RANGE，6 个够用且快）。
# AgentMoss 默认 9090，findFreePort 从 9090 往上找空闲，所以探测扫 9090-9095。
_PROBE_PORTS = [9090, 9091, 9092, 9093, 9094, 9095]
# 探测结果缓存：解析出的 service URL 缓存，仅首次 hook 调用有探测延迟。
_cached_service_url: str | None = None
# xiaoo LLM 配置缓存（读 ~/.config/xiaoo/config.toml + 解析 env 拿真实 key）。
# 命中缓存则 per-request 注入，避免每次 hook 都读 config.toml。
# None=未尝试加载，dict=已加载（可能为空 dict 表示 xiaoo 无 [llm] 段或 key 缺失）。
_xiaoo_llm_config_cache: dict | None = None


def _load_xiaoo_llm_config() -> dict:
    """读 xiaoo 的 LLM 配置，组装成 per-request llm_config（供 AgentMoss 服务端用）。

    只在 xiaoo 调用场景生效：bridge.py 读 ~/.config/xiaoo/config.toml 的 [llm] 段，
    字段名转译（xiaoo api_key_env→env 取真实 key / api_base→baseUrl / model / provider），
    组装成 {"apiKey", "baseUrl", "model", "provider"}。AgentMoss 服务端收到后用 per-request
    配置覆盖全局，等于"xiaoo 调用时才用 xiaoo 的 key"——服务端不再无条件读 xiaoo config。

    缺 xiaoo config.toml / 无 [llm] 段 / api_key_env 指向的 env 未设 → 返回空 dict
    （AgentMoss 服务端会用自己的全局 LLM 配置，相当于普通调用）。
    """
    global _xiaoo_llm_config_cache
    if _xiaoo_llm_config_cache is not None:
        return _xiaoo_llm_config_cache

    cfg: dict = {}
    try:
        xiaoo_path = Path.home() / ".config" / "xiaoo" / "config.toml"
        if not xiaoo_path.exists() or tomllib is None:
            _xiaoo_llm_config_cache = cfg
            return cfg
        with open(xiaoo_path, "rb") as f:
            data = tomllib.load(f)
        llm = data.get("llm") or {}
        if not llm:
            _xiaoo_llm_config_cache = cfg
            return cfg
        # xiaoo 用 api_key_env 间接引 env（如 XIAOO_API_KEY），这里解析出真实 key。
        api_key_env = llm.get("api_key_env", "")
        api_key = os.getenv(api_key_env) if api_key_env else llm.get("api_key", "")
        if not api_key:
            # key 拿不到，per-request 注入无意义（服务端会 401），返回空让服务端用全局
            _xiaoo_llm_config_cache = cfg
            return cfg
        base_url = llm.get("base_url") or llm.get("api_base") or ""
        model = llm.get("model", "")
        provider = llm.get("provider", "")
        if model and base_url:
            cfg = {
                "apiKey": api_key,
                "baseUrl": base_url,
                "model": model,
                "provider": provider,
            }
    except Exception:
        cfg = {}
    _xiaoo_llm_config_cache = cfg
    return cfg


def _log(tag: str, payload: object) -> None:
    if not _LOG_PATH:
        return
    try:
        line = f"[{datetime.now().isoformat(timespec='milliseconds')}] [{tag}] {json.dumps(payload, ensure_ascii=False)}\n"
        with open(_LOG_PATH, "a", encoding="utf-8") as f:
            f.write(line)
    except Exception as exc:
        print(f"[agent_moss_bridge] log write failed: {exc}", file=sys.stderr)


def _probe_service_url() -> str | None:
    """探测本地端口找 AgentMoss（返回 healthy 的那个）。

    扫 _PROBE_PORTS，GET /api/v1/health，status=='healthy' 即命中。
    找不到返回 None（调用方走默认 9090，让 fail-closed 逻辑生效）。
    与 OpenDesk hook.ts probeGateUrl 一致。
    """
    for port in _PROBE_PORTS:
        url = f"http://{_HOST}:{port}/api/v1/health"
        try:
            req = urllib.request.Request(url, method="GET")
            with urllib.request.urlopen(req, timeout=_HEALTH_TIMEOUT) as resp:
                if resp.status == 200:
                    data = json.loads(resp.read().decode("utf-8"))
                    if data.get("status") == "healthy":
                        return f"http://{_HOST}:{port}"
        except Exception:
            # 端口未监听或非 AgentMoss，继续探测下一个
            continue
    return None


def _resolve_service_url() -> str:
    """解析 AgentMoss 服务 URL（优先级学 OpenDesk hook.ts resolveGateUrl）。

    优先级：
      1. AGENT_MOSS_URL env（完整 URL，最高优先级）
      2. AGENT_MOSS_HOST/AGENT_MOSS_PORT env（用户显式指定端口，直接用）
      3. 探测 9090-9095 找返回 healthy 的端口（缓存）
      4. 默认 http://127.0.0.1:9090（探测失败兜底，让 fail-closed 生效）

    缓存：结果存 _cached_service_url，仅首次 hook 调用有探测延迟。
    仅当用户没显式设 URL/port 时才探测（env 指定了就不探测，尊重显式配置）。
    """
    global _cached_service_url
    if _cached_service_url:
        return _cached_service_url
    # 1. 完整 URL env
    if _URL_ENV:
        _cached_service_url = _URL_ENV
        return _cached_service_url
    # 2. 用户显式设了 PORT（非默认），直接用，不探测
    if os.getenv("AGENT_MOSS_PORT") is not None:
        _cached_service_url = f"http://{_HOST}:{_PORT}"
        return _cached_service_url
    # 3. 探测
    probed = _probe_service_url()
    if probed:
        _cached_service_url = probed
        return _cached_service_url
    # 4. 兜底默认 9090
    _cached_service_url = f"http://{_HOST}:{_PORT}"
    return _cached_service_url


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
    """活性检查：解析服务 URL（含探测兜底）后探 /api/v1/health。

    _resolve_service_url 已经做了探测缓存（env 指定则不探测，直接用；否则扫
    9090-9095 找 healthy）。这里对解析出的 URL 再探一次确认（resolve 的探测结果
    已缓存，此处多数命中缓存 URL 即 healthy，仅兜底 9090 场景会真失败）。

    Returns:
        (ok, message)：ok=True 服务常驻可达；ok=False 不可达，message 是人可读引导。
    """
    base_url = _resolve_service_url()
    url = f"{base_url}/api/v1/health"
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=_HEALTH_TIMEOUT) as resp:
            if resp.status == 200:
                data = json.loads(resp.read().decode("utf-8"))
                if data.get("status") == "healthy":
                    return True, f"AgentMoss 服务健康（{data.get('version', '?')}，{base_url}）"
                return False, f"AgentMoss 服务响应非 healthy: {data}"
            return False, f"AgentMoss 服务 HTTP {resp.status}"
    except urllib.error.URLError as exc:
        # 区分连接拒绝（服务没起）vs 超时（网络/服务异常）
        reason = str(exc)
        if isinstance(exc.reason, ConnectionRefusedError) or "Connection refused" in reason:
            return False, (
                f"AgentMoss 服务未启动（{base_url} 拒绝连接，已探测 9090-9095 无 healthy）。"
                f"请先启动：systemctl start agent-moss（openEuler RPM）"
                f" 或 python3 -m uvicorn agent_moss.server.app:create_app --factory"
                f"，或设 AGENT_MOSS_URL/AGENT_MOSS_PORT 指向远端服务"
            )
        if "timed out" in reason.lower():
            return False, (
                f"AgentMoss 服务超时（{_HEALTH_TIMEOUT}s）：检查 {base_url} 是否正确、"
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


def _normalize_history_item(h: dict) -> dict:
    """把 xiaoO action_history 的一项规范化成 AnalyzeRequest 要求的结构。

    问题：xiaoO 传的 history 项 action_detail 可能是 dict（如 ask_user_question
    的 {"questions": [...]}、skill 的 {"skill": "..."}、bash 的 {"command": "..."}），
    而 AnalyzeRequest.action_detail 要求 str，dict 直传触发 422。

    解法：跟 a_next 一样，按 tool_name 提取（bash→command / file_*→file_path /
    skill→skill 名 / 其他→JSON）。tool_name 已是 str 时直接用。
    """
    tool_name = str(h.get("name") or h.get("action_type") or "unknown_tool")
    raw_detail = h.get("action_detail", h.get("input", ""))
    if raw_detail == "":
        raw_detail = h.get("input", "")
    action_detail = _extract_action_detail(tool_name, raw_detail)
    return {"action_type": tool_name, "action_detail": action_detail}


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
            _normalize_history_item(h)
            for h in action_history if isinstance(h, dict)
        ],
        "a_next": a_next,
        "reason": reason,
        "os_type": "",      # 让 AgentMoss 自动检测
        "cwd": data.get("cwd", ""),
        "agent_id": "xiaoo",  # 告知 AgentMoss 加载 xiaoO 专属规则
        # per-request 注入 xiaoo 的 LLM 配置 + 日志路径（只在 xiaoo 调用场景生效）。
        # bridge 读 xiaoo config.toml + 解析 env 拿真实 key，服务端用它覆盖全局，
        # 等于"xiaoo 调时用 xiaoo 的 key"，服务端不再无条件读 xiaoo config。
        # 拿不到 key（config 缺/key env 未设）则空 dict，服务端走全局配置。
        # llm_log_path 同理 per-request 注入：bridge 子进程有 AGENT_MOSS_LOG_PATH env
        # 但服务进程不一定有（可能别的终端起的），通过 metadata 传给服务端写 LLM_PROMPT，
        # 跟 audit_agent "LOG_PATH 一设，hook 日志 + LLM prompt 都写同文件" 体验一致。
        "metadata": {
            "llm_config": _load_xiaoo_llm_config(),
            "llm_log_path": _LOG_PATH,
        },
    }

    _log("HOOK_INPUT", {
        "session_id": session_id, "tool_name": tool_name,
        "reason": reason, "action_history_len": len(action_history),
    })

    # === Step 2: POST /api/v1/analyze ===
    # 用解析出的服务 URL（含探测兜底，跟 health 同一 URL）。
    url = f"{_resolve_service_url()}/api/v1/analyze"
    req = urllib.request.Request(
        url,
        data=json.dumps(request_body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as resp:
            result = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        # HTTP 错误（如 422 请求体校验失败、500 内部错误）：读响应体拿服务端 detail。
        # 不读响应体会丢失关键定位信息（422 的 detail 说明哪个字段错了）。
        body = ""
        try:
            body = exc.read().decode("utf-8", errors="replace")
        except Exception:
            pass
        hook_result = {
            "result": "deny",
            "reason": f"AgentMoss /api/v1/analyze 返回 HTTP {exc.code}：{body or exc.reason}，按 fail-closed 原则拒绝",
        }
        _log("HOOK_OUTPUT", {"tool_name": tool_name, "hook_result": hook_result, "error": str(exc), "resp_body": body})
        print(json.dumps(hook_result, ensure_ascii=False))
        return 0
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
