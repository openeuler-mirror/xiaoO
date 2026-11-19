#!/usr/bin/env python3
"""xiaoO plugin-hook bridge → AgentMoss HTTP service.

契约：
  - xiaoO 在 tool-pre hook 触发时，经 stdin 发送 JSON payload
  - 本脚本：
      1. 活性检查：探 AgentMoss /api/v1/health，确认服务常驻可达
         （不可达时 fail-closed Deny，并给清晰错误引导，避免"服务挂了静默放行"）
      2. 解析 payload，提取 tool_name/tool_input → action_type/action_detail
         （提取规则：bash→command，file_*→file_path，skill→skill 名）
      3. POST 到 AgentMoss 的 /api/v1/analyze（127.0.0.1:9090）
      4. 将 AnalyzeResponse 映射为 PreHookResult JSON：
           {"result": "allow", "reason": "..."}
           {"result": "deny",  "reason": "..."}
      5. stdout 输出 PreHookResult JSON（正常路径退出码 0；空 stdin / JSON
         解析失败时退出码 2）

  - AgentMoss 作为常驻 HTTP 服务（systemd unit / pip 装后自启），避免每次
    hook 都 spawn 子进程的开销（venv 启动/import、进程池管理）。

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
                       记 HOOK_INPUT/HOOK_OUTPUT（含 tool_input、判定结果）；
                       一旦设置，hook 日志与 LLM prompt 都写同一文件。
  AGENT_MOSS_HOOK_TIMEOUT  HTTP 请求超时秒（默认 60）
  AGENT_MOSS_HEALTH_TIMEOUT  活性检查超时秒（默认 2）
  AGENT_MOSS_CHECK_SOURCE  =1 时额外检查源码可 import（默认 空=不查）

依赖：仅 stdlib（urllib + socket）。xiaoO venv 无需额外安装。
"""

import json
import os
import re
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

#: 写工具历史里单个文本字段（content/old/new/patch…）保留的最大字符数。
#: RBW-11 只需要判断「脚本/写入内容是否提到目标文件名」，不需要整份内容；
#: 无界保留会让 analyze 请求体与 L3 prompt 随文件体积线性膨胀。
_HISTORY_TEXT_LIMIT = 2000
#: 从写工具内容里提取的「引用文件名」上限（紧凑数组，供 RBW-11 匹配）。
_HISTORY_REF_NAME_LIMIT = 64
_HISTORY_REF_NAME_MAXLEN = 128
#: 文件名（含扩展名）粗提取：覆盖 `out.csv`、`dir/折旧表.csv`、`C:\x\a.py` 等。
_HISTORY_REF_NAME_RE = re.compile(r"[^\s'\"`<>|;:,(){}\[\]]+\.[A-Za-z0-9]{1,8}")

# AgentMoss 0.16.14 起默认开启鉴权（auto 生成 token）：不带 Bearer 会 401，
# 而 bridge 的失败分支是 fail-closed，等于所有工具调用都被拒。
# 解析顺序：显式 env > env 指定的文件 > 约定路径（install.sh 生成到
# ~/.config/agentmoss/xiaoo_token，0600；systemd 以 root 运行也能读，
# nohup 以当前用户运行同样可读）。
def _resolve_auth_token() -> str:
    tok = os.getenv("AGENT_MOSS_AUTH_TOKEN", "").strip()
    if tok:
        return tok
    for candidate in (
        os.getenv("AGENT_MOSS_AUTH_TOKEN_FILE", "").strip(),
        str(Path.home() / ".config" / "agentmoss" / "xiaoo_token"),
    ):
        if not candidate:
            continue
        try:
            val = Path(candidate).read_text(encoding="utf-8").strip()
            if val:
                return val
        except OSError:
            continue
    return ""

# 归属实例标识：bridge 只认自己起的 agentmoss（instance 字段 == _EXPECT_INSTANCE），
# 避免漂移到同机其他 agent（如 OpenDesk）spawn 的 agentmoss——后者会用错 agent 规则判定 xiaoo 请求。
# 值来自 env AGENT_MOSS_INSTANCE（install.sh 注入=xiaoo），默认 xiaoo。
# 用户显式设 AGENT_MOSS_URL/PORT 时跳过探测，此值不生效（尊重显式配置）。
_EXPECT_INSTANCE = os.getenv("AGENT_MOSS_INSTANCE", "xiaoo")

# 探测端口范围（学 OpenDesk hook.ts GATE_PROBE_RANGE，6 个够用且快）。
# AgentMoss 默认 9090，findFreePort 从 9090 往上找空闲；具体探测端口以
# _PROBE_PORTS 数组为单一事实源。
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


def _version_key(v: str) -> tuple[int, ...]:
    """把版本号转成可比较的整数元组（0.17.33 > 0.17.22）。"""
    parts = [int(x) for x in re.findall(r"\d+", v or "")]
    return tuple(parts) or (0,)


def _healthy_instance(port: int) -> dict | None:
    """GET 某端口的 health；healthy 且 instance 匹配才返回响应，否则 None。"""
    url = f"http://{_HOST}:{port}/api/v1/health"
    try:
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=_HEALTH_TIMEOUT) as resp:
            if resp.status != 200:
                return None
            data = json.loads(resp.read().decode("utf-8"))
            if data.get("status") != "healthy":
                return None
            # 归属过滤：未带 instance（旧版/未配置）或不匹配 → 跳过。
            # 宁可探测不到走 fail-closed，也不漂移到别人的进程用错规则。
            if data.get("instance", "") != _EXPECT_INSTANCE:
                return None
            return data
    except Exception:
        return None


def _port_file_service_url() -> str | None:
    """优先读 AgentMoss 启动时写的 port 文件，避免同机旧实例占着 9090。

    AgentMoss 端口漂移后会写 ``<数据目录>/agent_moss_server.port.json``。
    若只按 9090-9095 顺序探测，同机残留的旧版本实例（同 instance 名）会先命中，
    token 体系不一致 → 401 → fail-closed 拒绝所有工具调用（2026-11 实测）。
    """
    root = os.getenv("AGENT_MOSS_DATA_DIR", "").strip()
    base = Path(root) if root else Path.home() / ".config" / "agentmoss"
    # 新版本按 instance 分子目录；旧版本仍写共享路径（兼容回退，靠 health 校验 instance）
    candidates = [
        base / _EXPECT_INSTANCE / "agent_moss_server.port.json",
        base / "agent_moss_server.port.json",
    ]
    for path in candidates:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            port = int(data.get("port") or 0)
        except Exception:
            continue
        if port > 0 and _healthy_instance(port) is not None:
            return f"http://{_HOST}:{port}"
    return None


def _probe_service_url() -> str | None:
    """探测本地端口找 AgentMoss（healthy + 归属匹配；多实例取版本最高的）。

    与 OpenDesk hook.ts probeGateUrl 探测范围一致，但增加归属过滤与版本优先：
    同机可能残留旧实例（如 0.17.22 占 9090、新 0.17.33 漂移到 9091），
    按端口顺序取第一个会选到旧版，token/规则都不对。
    """
    best_url: str | None = None
    best_key: tuple[int, ...] | None = None
    for port in _PROBE_PORTS:
        data = _healthy_instance(port)
        if data is None:
            continue
        key = (_version_key(str(data.get("version", ""))), -port)
        if best_key is None or key > best_key:
            best_key = key
            best_url = f"http://{_HOST}:{port}"
    return best_url


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
    # 2. 用户设了 AGENT_MOSS_PORT（不论是否为默认值），直接用，不探测
    if os.getenv("AGENT_MOSS_PORT") is not None:
        _cached_service_url = f"http://{_HOST}:{_PORT}"
        return _cached_service_url
    # 3. AgentMoss 启动时写的 port 文件（端口漂移后仍指向当前实例）
    port_file = _port_file_service_url()
    if port_file:
        _cached_service_url = port_file
        return _cached_service_url
    # 4. 探测
    probed = _probe_service_url()
    if probed:
        _cached_service_url = probed
        return _cached_service_url
    # 5. 兜底默认 9090
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
    """action_detail 提取规则：取工具输入里最能代表意图的字段。

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


def _referenced_names(text: str) -> set[str]:
    """从内容里粗提取文件名（basename），供 AgentMoss RBW-11 判断脚本产物。"""
    names: set[str] = set()
    for match in _HISTORY_REF_NAME_RE.finditer(text or ""):
        token = match.group(0).strip(".,;:()[]{}<>\"'")
        name = token.replace("\\", "/").rsplit("/", 1)[-1]
        if 1 <= len(name) <= _HISTORY_REF_NAME_MAXLEN:
            names.add(name)
    return names


def _bounded_history_input(tool_input: dict) -> str:
    """写工具历史：保留有界内容片段 + 引用文件名，避免整份文件进历史/L3。

    - content/old/new/patch 等文本字段截断到 _HISTORY_TEXT_LIMIT；
    - 从原始内容提取 basename 放进 `referenced_names`（最多 64 个），
      RBW-11 仍能命中脚本里提到的目标文件名；
    - 其余参数（file_path/mode 等）原样保留。
    """
    data = dict(tool_input)
    names: set[str] = set()
    text_keys = (
        "content", "old", "new", "oldString", "newString",
        "old_string", "new_string", "patch", "text",
    )
    for key in text_keys:
        value = data.get(key)
        if not isinstance(value, str):
            continue
        names.update(_referenced_names(value))
        if len(value) > _HISTORY_TEXT_LIMIT:
            data[key] = value[:_HISTORY_TEXT_LIMIT] + "…[truncated]"
    if names:
        data["referenced_names"] = sorted(names)[:_HISTORY_REF_NAME_LIMIT]
    return json.dumps(data, ensure_ascii=False)


def _normalize_history_item(h: dict) -> dict:
    """把 xiaoO action_history 的一项规范化成 AnalyzeRequest 要求的结构。

    问题：xiaoO 传的 history 项 action_detail 可能是 dict（如 ask_user_question
    的 {"questions": [...]}、skill 的 {"skill": "..."}、bash 的 {"command": "..."}），
    而 AnalyzeRequest.action_detail 要求 str，dict 直传触发 422。

    解法：
    - 写工具（file_write/file_edit）优先取 `input` 字典（即使同时带了字符串
      action_detail），交给 `_bounded_history_input`：保留有界内容片段 + 引用
      文件名，供 AgentMoss RBW-11 判断脚本产物；不把整份文件塞进历史/L3；
    - 其他工具：bash→command / file_read→file_path / skill→skill 名 / 其他→JSON。
    """
    tool_name = str(h.get("name") or h.get("action_type") or "unknown_tool")
    tn = tool_name.lower()
    raw_input = h.get("input")
    raw_detail = h.get("action_detail", raw_input)
    if raw_detail == "" or raw_detail is None:
        raw_detail = raw_input
    if tn in ("file_write", "file_edit"):
        if isinstance(raw_input, dict):
            action_detail = _bounded_history_input(raw_input)
        elif isinstance(raw_detail, dict):
            action_detail = _bounded_history_input(raw_detail)
        else:
            action_detail = _extract_action_detail(tool_name, raw_detail)
    else:
        action_detail = _extract_action_detail(tool_name, raw_detail)
    item = {"action_type": tool_name, "action_detail": action_detail}
    # 工具返回内容：ask_user_question 的 output 是用户回答，AgentMoss 的授权类
    # 规则（改密码 / 删用户）靠它判断"用户是否已确认"。不传就会被判成未授权。
    output = h.get("output", h.get("result", ""))
    if output == "" or output is None:
        output = h.get("tool_result", "")
    if isinstance(output, (dict, list)):
        try:
            output = json.dumps(output, ensure_ascii=False)
        except (TypeError, ValueError):
            output = str(output)
    item["output"] = str(output or "")
    # called_at：该次调用发生的时刻。AgentMoss 的 L2 授权时效窗
    # （CONSENT_TTL_SECONDS，默认 300s）拿它和 a_next.called_at 相减，判断
    # 「用户确认后多久才执行」——一小时前的「是」不能给今天的删用户背书。
    # 宿主 payload 里给的是毫秒时间戳（called_at_ms），这里转 ISO 8601；
    # 缺了就跳过时间窗，由「最近 5 条」距离兜底（不伪造时刻）。
    called_at_ms = h.get("called_at_ms")
    if called_at_ms:
        try:
            item["called_at"] = datetime.fromtimestamp(float(called_at_ms) / 1000).isoformat()
        except (TypeError, ValueError, OSError, OverflowError):
            pass
    return item


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
    a_next = {
        "action_type": tool_name,
        "action_detail": action_detail,
        # 当前动作就是此刻发生的：AgentMoss 拿它与历史里 ask_user_question 的
        # called_at 相减，判断「用户刚才的确认是否仍在时效窗内」。
        "called_at": datetime.now().isoformat(),
    }

    session_id: str = data.get("session_id") or call.get("call_id", "unknown-session")
    prompt_session: str = data.get("prompt_session", "")
    action_history: list = data.get("action_history", [])
    reason: str = data.get("reason", "")

    request_body = {
        "session_id": session_id,
        "prompt_session": prompt_session,
        "prompt_history": [
            {
                "text": t.get("text", ""),
                "actions": [
                    _normalize_history_item(a)
                    for a in t.get("actions", []) if isinstance(a, dict)
                ],
            }
            for t in data.get("prompt_history", []) if isinstance(t, dict)
        ],
        "action_history": [
            _normalize_history_item(h)
            for h in action_history if isinstance(h, dict)
        ],
        "a_next": a_next,
        "reason": reason,
        "os_type": "",      # 让 AgentMoss 自动检测
        # 宿主 payload 里工作区字段叫 `workspace`（crates/hook/.../tool/adaptor.rs 的
        # build_pre_payload）。老写法只读 `cwd`，而宿主从不发这个字段，于是这里的取值
        # 恒为空串 —— AgentMoss 的层2「间接文件访问检测」因此在 xiaoO 场景下静默失效
        # （见 TECH_WIKI.md「关于 cwd 的已知缺口」）。优先 `cwd` 以兼容仍自带该字段的
        # 调用方，缺失时回落到 `workspace`。
        "cwd": data.get("cwd") or data.get("workspace", ""),
        "agent_id": "xiaoo",  # 告知 AgentMoss 加载 xiaoO 专属规则
        # per-request 注入 xiaoo 的 LLM 配置 + 日志路径（只在 xiaoo 调用场景生效）。
        # bridge 读 xiaoo config.toml + 解析 env 拿真实 key，服务端用它覆盖全局，
        # 等于"xiaoo 调时用 xiaoo 的 key"，服务端不再无条件读 xiaoo config。
        # 拿不到 key（config 缺/key env 未设）则空 dict，服务端走全局配置。
        # llm_log_path 同理 per-request 注入：bridge 子进程有 AGENT_MOSS_LOG_PATH env
        # 但服务进程不一定有（可能别的终端起的），通过 metadata 传给服务端写 LLM_PROMPT，
        # 维持"LOG_PATH 一设，hook 日志 + LLM prompt 都写同文件"的行为契约。
        "metadata": {
            "llm_config": _load_xiaoo_llm_config(),
            "llm_log_path": _LOG_PATH,
        },
    }

    _log("HOOK_INPUT", {
        "session_id": session_id, "tool_name": tool_name,
        "tool_input": tool_input, "reason": reason,
        "action_history_len": len(action_history),
    })

    # === Step 2: POST /api/v1/analyze ===
    # 用解析出的服务 URL（含探测兜底，跟 health 同一 URL）。
    url = f"{_resolve_service_url()}/api/v1/analyze"
    _headers = {"Content-Type": "application/json"}
    _token = _resolve_auth_token()
    if _token:
        _headers["Authorization"] = f"Bearer {_token}"
    req = urllib.request.Request(
        url,
        data=json.dumps(request_body).encode("utf-8"),
        headers=_headers,
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
    # AgentMoss 命中的违规层（L1.1/L1.2/L2.1/L2.2/L2.3/L3.1 + brain_L1/brain_L2，按真实执行顺序）。
    # xiaoO 本体 PreHookResult 暂只携带 reason（无独立层字段），因此把层数拼进 reason 让用户可见，
    # 同时 stdout 附带 violated_layers 字段——xiaoO 本体后续支持后可无感读取。
    raw_layers = result.get("violated_layers") or []
    if not isinstance(raw_layers, list):
        raw_layers = [raw_layers]
    layers = [str(l) for l in raw_layers if str(l)]
    layers_text = ",".join(layers)
    if decision == "Allow":
        hook_result = {"result": "allow", "reason": result.get("reason", "")}
    else:
        reason_text = result.get("violated_policy") or result.get("reason", "")
        if layers_text:
            reason_text = f"[触发层: {layers_text}] {reason_text}"
        hook_result = {"result": "deny", "reason": reason_text}
        if layers_text:
            hook_result["violated_layers"] = layers

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
