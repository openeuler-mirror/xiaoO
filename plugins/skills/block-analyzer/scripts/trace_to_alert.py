#!/usr/bin/env python3
"""
trace_to_alert.py - 将 xiaoO moirai trace 数据转换为 block-analyzer 标准告警 JSON 格式。

使用方式:
    python3 trace_to_alert.py --trace-id <TRACE_ID> --db <DB_PATH> --moirai-bin <MOIRAI_BIN> --output <OUTPUT_PATH>

流程:
    1. 调用 moirai export 导出 trace.json
    2. 解析 trace 数据，提取 span 信息
    3. 定位被 block 的动作（result_kind=blocked 或 outcome 非 Ok）
    4. 推断 violated_permission / risk_type / risk_level 等字段
    5. 输出标准告警 JSON 文件
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile


# ── 工具名到 blocked_action 的映射 ──────────────────────────────────

TOOL_ACTION_MAP = {
    "bash": "execute_command",
    "shell": "execute_command",
    "execute": "execute_command",
    "read_file": "read_file",
    "file_read": "read_file",
    "write_file": "write_file",
    "edit_file": "write_file",
    "replace_in_file": "write_file",
    "file_write": "write_file",
    "file_edit": "write_file",
    "search_content": "search_content",
    "search_file": "search_file",
    "list_dir": "list_dir",
    "web_fetch": "web_fetch",
    "web_search": "web_search",
    "install_binary": "install_binary",
    "execute_command": "execute_command",
}


# ── 危险命令模式 → (risk_type, risk_level, violated_permission) ─────

DANGEROUS_PATTERNS = [
    # (pattern_regex, risk_type, risk_level, violated_permission)
    (r"rm\s+-rf\s+/", "malicious_script", "critical", "shell_permission:execute_dangerous"),
    (r"rm\s+-rf\s", "malicious_script", "critical", "shell_permission:execute_dangerous"),
    (r"curl\s+.*\|\s*bash", "remote_code_execution", "critical", "shell_permission:execute_dangerous"),
    (r"curl\s+.*\|\s*sh", "remote_code_execution", "critical", "shell_permission:execute_dangerous"),
    (r"wget\s+.*\|\s*bash", "remote_code_execution", "critical", "shell_permission:execute_dangerous"),
    (r"/etc/shadow", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"\.ssh/id_rsa", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"\.ssh/id_ed25519", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"chmod\s+777", "malicious_script", "high", "shell_permission:execute_dangerous"),
    (r"chmod\s+u\+x", "malicious_script", "medium", "shell_permission:execute"),
    (r"sudo\s+", "privilege_escalation", "high", "shell_permission:execute_dangerous"),
    (r"iptables\s+-F", "malicious_script", "critical", "shell_permission:execute_dangerous"),
    (r"crontab", "persistence_backdoor", "high", "shell_permission:execute"),
    (r"169\.254\.169\.254", "lateral_movement", "high", "network_permission:internal_access"),
    (r"mkfifo", "reverse_shell", "critical", "shell_permission:execute_dangerous"),
    (r"nc\s+-.*-e\s+", "reverse_shell", "critical", "shell_permission:execute_dangerous"),
    (r"/dev/tcp/", "reverse_shell", "critical", "shell_permission:execute_dangerous"),
    (r"fork\s*\(\)\s*\{.*\}", "resource_exhaustion", "critical", "shell_permission:execute_dangerous"),
    (r":\(\)\{\s*:\|:&\s*\}", "resource_exhaustion", "critical", "shell_permission:execute_dangerous"),
]

# 敏感文件路径模式
SENSITIVE_PATH_PATTERNS = [
    (r"/etc/shadow", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"/etc/passwd", "data_exfiltration", "medium", "file_permission:read_sensitive"),
    (r"\.ssh/id_", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"\.env", "data_exfiltration", "medium", "file_permission:read_sensitive"),
    (r"\.aws/credentials", "data_exfiltration", "high", "file_permission:read_sensitive"),
    (r"\.gitconfig", "data_exfiltration", "low", "file_permission:read"),
]


def find_moirai_binary():
    """查找 moirai 可执行文件"""
    candidates = [
        "./target/release/moirai",
        "./target/debug/moirai",
        "moirai",
    ]
    for c in candidates:
        if os.path.isfile(c) and os.access(c, os.X_OK):
            return os.path.abspath(c)
    try:
        result = subprocess.run(["which", "moirai"], capture_output=True, text=True)
        if result.returncode == 0:
            return os.path.abspath(result.stdout.strip())
    except Exception:
        pass
    return None


def _resolve_db_path(db_path, moirai_bin=None):
    """
    解析 moirai db 路径为绝对路径。

    moirai 不会做 ~ 展开，传入 ~ 时会在 cwd 下创建字面的 ~/ 子目录存放 db。
    因此需要同时检查两种位置：
      1. 展开 ~ 后的绝对路径（如 /home/user/.moirai/traces.db）
      2. moirai 在历史 cwd 下创建的字面 ~/ 子目录中的 db

    当路径以 ~ 开头时，字面 ~ 路径下的候选优先（moirai 实际使用的位置）。
    同优先级下选文件最大的（更可能包含数据）。
    """
    expanded = os.path.expanduser(db_path)
    if os.path.isabs(expanded):
        abs_expanded = expanded
    else:
        abs_expanded = os.path.abspath(expanded)

    literal_candidates = []
    expanded_candidates = []

    if os.path.isfile(abs_expanded):
        expanded_candidates.append(abs_expanded)

    if db_path.startswith("~"):
        search_dirs = set()
        search_dirs.add(os.getcwd())
        if moirai_bin:
            moirai_dir = os.path.dirname(os.path.abspath(moirai_bin))
            for _ in range(5):
                search_dirs.add(moirai_dir)
                parent = os.path.dirname(moirai_dir)
                if parent == moirai_dir:
                    break
                moirai_dir = parent
        p = os.getcwd()
        for _ in range(5):
            if os.path.isfile(os.path.join(p, "target/release/moirai")):
                search_dirs.add(p)
                break
            parent = os.path.dirname(p)
            if parent == p:
                break
            p = parent
        for d in search_dirs:
            alt_path = os.path.join(d, db_path)
            alt_abs = os.path.abspath(alt_path)
            if os.path.isfile(alt_abs) and alt_abs not in expanded_candidates and alt_abs not in literal_candidates:
                literal_candidates.append(alt_abs)

    if literal_candidates:
        literal_candidates.sort(key=lambda p: os.path.getsize(p), reverse=True)
        return literal_candidates[0]

    if expanded_candidates:
        return expanded_candidates[0]

    return abs_expanded


def export_trace(trace_id, db_path, moirai_bin=None):
    """调用 moirai export 导出 trace 数据"""
    if moirai_bin is None:
        moirai_bin = find_moirai_binary()
    if moirai_bin is None:
        print(
            "错误: 找不到 moirai 可执行文件。\n"
            "请通过 --moirai-bin 参数指定 moirai 的路径，例如：\n"
            "  python3 scripts/trace_to_alert.py --trace-id <TRACE_ID> \\\n"
            "    --moirai-bin /path/to/target/release/moirai \\\n"
            "    --db ~/.moirai/traces.db \\\n"
            "    --output <alert_output_path>",
            file=sys.stderr,
        )
        sys.exit(1)

    resolved_db = _resolve_db_path(db_path, moirai_bin=moirai_bin)
    if not os.path.isfile(resolved_db):
        print(
            f"错误: 找不到 traces.db 文件 (尝试路径: {resolved_db})。\n"
            "请通过 --db 参数指定 moirai traces.db 的实际路径，例如：\n"
            "  python3 scripts/trace_to_alert.py --trace-id <TRACE_ID> \\\n"
            "    --moirai-bin /path/to/target/release/moirai \\\n"
            "    --db ~/.moirai/traces.db \\\n"
            "    --output <alert_output_path>",
            file=sys.stderr,
        )
        sys.exit(1)

    cmd = [moirai_bin, "export", "--trace-id", trace_id, "--db", resolved_db]
    print(f"执行: {' '.join(cmd)}", file=sys.stderr)

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        if result.returncode != 0:
            print(f"moirai export 失败 (exit code {result.returncode}): {result.stderr}", file=sys.stderr)
            sys.exit(1)
        return json.loads(result.stdout)
    except subprocess.TimeoutExpired:
        print("错误: moirai export 超时", file=sys.stderr)
        sys.exit(1)
    except json.JSONDecodeError as e:
        print(f"错误: trace 数据 JSON 解析失败: {e}", file=sys.stderr)
        sys.exit(1)


def load_trace_from_file(filepath):
    """从本地文件加载 trace 数据（用于测试）"""
    with open(filepath) as f:
        return json.load(f)


def parse_trace(trace_data):
    """解析 trace 数据，分类为 USER / TOOL_CALL / END spans"""
    user_span = None
    tool_calls = []
    end_span = None

    for span in trace_data:
        span_type = span.get("span_type", "")
        if span_type == "USER":
            user_span = span
        elif span_type == "TOOL_CALL":
            tool_calls.append(span)
        elif span_type == "END":
            end_span = span

    return user_span, tool_calls, end_span


def find_blocked_tool_call(tool_calls):
    """
    定位被 block 的 TOOL_CALL span。
    优先级:
      1. result_kind in ("blocked", "denied")
      2. outcome in ("Blocked", "Denied") 且 phase 含 deny/blocked
      3. outcome != "Ok" 且 phase != "completed"
      4. outcome != "Ok"
      5. None（无 blocked，返回 None）
    """
    # 优先匹配 result_kind
    for tc in tool_calls:
        extras = tc.get("extras", {})
        rk = extras.get("result_kind", "")
        if rk in ("blocked", "denied"):
            return tc

    for tc in tool_calls:
        extras = tc.get("extras", {})
        outcome = extras.get("outcome", "")
        phase = extras.get("phase", "")
        if outcome in ("Blocked", "Denied") and ("deny" in phase or "block" in phase):
            return tc

    for tc in tool_calls:
        extras = tc.get("extras", {})
        outcome = extras.get("outcome", "Ok")
        phase = extras.get("phase", "completed")
        if outcome != "Ok" and phase != "completed":
            return tc

    for tc in tool_calls:
        extras = tc.get("extras", {})
        if extras.get("outcome", "Ok") != "Ok":
            return tc

    # 无明确 blocked span
    return None


def extract_command(tool_call):
    """从 TOOL_CALL 中提取命令文本"""
    extras = tool_call.get("extras", {})
    effective_input = extras.get("effective_input", {})

    # bash/shell 工具 → command 字段
    if "command" in effective_input:
        return effective_input["command"]

    # write_file/file_write/edit_file → 读取 path/file_path
    path = effective_input.get("path") or effective_input.get("file_path")
    if path:
        content = effective_input.get("content", "")
        snippet = content[:100] + "..." if len(content) > 100 else content
        return f"{path}: {snippet}"

    # read_file/search 等 → 各种字段
    parts = []
    for key in ("path", "file_path", "pattern", "query", "url", "command", "content"):
        if key in effective_input:
            parts.append(f"{key}={effective_input[key]}")
    return " ".join(parts) if parts else str(effective_input)


def infer_risk(command, tool_name=""):
    """根据命令内容和工具名推断 risk_type, risk_level, violated_permission"""
    cmd_lower = command.lower()

    # 匹配危险命令模式
    for pattern, risk_type, risk_level, violated_perm in DANGEROUS_PATTERNS:
        if re.search(pattern, cmd_lower):
            return risk_type, risk_level, violated_perm

    # 如果是文件操作工具，检查敏感路径
    if tool_name in ("read_file", "write_file", "edit_file", "file_read", "file_write", "file_edit"):
        for pattern, risk_type, risk_level, violated_perm in SENSITIVE_PATH_PATTERNS:
            if re.search(pattern, cmd_lower):
                return risk_type, risk_level, violated_perm

    # 默认推断
    if tool_name in ("bash", "shell", "execute"):
        return "malicious_script", "medium", "shell_permission:execute"
    if tool_name in ("read_file", "file_read"):
        return "data_exfiltration", "low", "file_permission:read"
    if tool_name in ("write_file", "edit_file", "file_write", "file_edit"):
        return "scope_escalation", "medium", "file_permission:write"

    return "scope_escalation", "medium", "unknown_permission"


def build_action_desc(tool_name, command, extras):
    """构建 action_desc 描述"""
    outcome = extras.get("outcome", "Ok")
    result_kind = extras.get("result_kind", "")

    parts = []
    if tool_name == "bash":
        parts.append(f"执行命令: {command}")
    elif tool_name in ("read_file", "file_read"):
        parts.append(f"读取文件: {command}")
    elif tool_name in ("write_file", "edit_file", "file_write", "file_edit"):
        parts.append(f"修改文件: {command}")
    else:
        parts.append(f"工具调用 {tool_name}: {command}")

    if result_kind == "blocked":
        parts.append("，被安全策略拦截")
    elif outcome != "Ok":
        parts.append(f"，执行结果: {outcome}")

    execution_error = extras.get("execution_error")
    if execution_error:
        parts.append(f"，错误: {execution_error}")

    return "".join(parts)


def _format_timestamp(ms_timestamp):
    """将毫秒时间戳转为 ISO 格式"""
    import datetime
    if ms_timestamp is None:
        return None
    try:
        dt = datetime.datetime.fromtimestamp(ms_timestamp / 1000, tz=datetime.timezone.utc)
        return dt.strftime("%Y-%m-%dT%H:%M:%SZ")
    except Exception:
        return str(ms_timestamp)


def _build_trace_chain(trace_data):
    """从 trace 数据构建标准 trace 链"""
    trace_chain = []
    for span in trace_data:
        span_type = span.get("span_type", "")
        ts_str = _format_timestamp(span.get("start_time"))

        if span_type == "USER":
            trace_chain.append({
                "action": "user_input",
                "content": f"session 启动 (trace_id: {span.get('trace_id', '')})",
                "timestamp": ts_str,
            })
        elif span_type == "TOOL_CALL":
            extras = span.get("extras", {})
            tn = extras.get("tool_name", "unknown")
            cmd = extract_command(span)
            trace_entry = {
                "action": TOOL_ACTION_MAP.get(tn, tn),
                "timestamp": ts_str,
            }
            # 根据工具类型添加不同字段
            if tn in ("bash", "shell", "execute"):
                trace_entry["command"] = cmd
            else:
                trace_entry["content"] = cmd
            trace_chain.append(trace_entry)
        elif span_type == "END":
            trace_chain.append({
                "action": "session_end",
                "timestamp": ts_str,
            })
    return trace_chain


def trace_to_alert(trace_data, trace_id=None):
    """
    将 trace JSON 数据转换为标准告警 JSON 格式。
    
    参数:
        trace_data: moirai export 输出的 trace JSON（list of spans）
        trace_id: 可选的 trace_id 覆盖值
    
    返回:
        符合 block-analyzer 输入格式的告警 dict
    """
    user_span, tool_calls, end_span = parse_trace(trace_data)

    # 提取 session_id
    session_id = trace_id
    if user_span:
        session_id = user_span.get("trace_id", trace_id)
        # 优先从 TOOL_CALL 的 session_id 获取（更精确）
        for tc in tool_calls:
            sid = tc.get("extras", {}).get("session_id")
            if sid:
                session_id = sid
                break

    # 提取 user
    user = "unknown"
    if user_span:
        # USER span 没有 agent_id，从第一个 TOOL_CALL 获取
        for tc in tool_calls:
            agent_id = tc.get("extras", {}).get("agent_id")
            if agent_id:
                user = agent_id
                break

    # 定位被 block 的动作
    blocked_tc = find_blocked_tool_call(tool_calls)
    if blocked_tc is None:
        # 无明确 blocked 动作：可能是正常 session 或空 trace
        # 构建 trace 链
        trace_chain = _build_trace_chain(trace_data)

        if not tool_calls:
            # 无 TOOL_CALL 的空 trace
            return {
                "session_id": session_id or "unknown",
                "trace": trace_chain,
                "user": user,
                "blocked_action": "unknown",
                "violated_permission": "unknown_permission",
                "timestamp": None,
                "risk_level": "info",
                "risk_type": "no_block",
                "action_desc": "无 TOOL_CALL 记录，session 无 block 事件",
                "source": "heuristic",
                "no_block": True,
            }

        # 有 TOOL_CALL 但全部成功（正常 session）
        return {
            "session_id": session_id or "unknown",
            "trace": trace_chain,
            "user": user,
            "blocked_action": "unknown",
            "violated_permission": "none",
            "timestamp": None,
            "risk_level": "info",
            "risk_type": "no_block",
            "action_desc": f"session 正常结束，共 {len(tool_calls)} 次工具调用均成功，无 block 事件",
            "source": "heuristic",
            "no_block": True,
        }

    blocked_extras = blocked_tc.get("extras", {})
    blocked_tool_name = blocked_extras.get("tool_name", "unknown")
    blocked_command = extract_command(blocked_tc)

    # 映射 blocked_action
    blocked_action = TOOL_ACTION_MAP.get(blocked_tool_name, blocked_tool_name)

    # 推断风险
    risk_type, risk_level, violated_perm = infer_risk(blocked_command, blocked_tool_name)

    # 构建动作描述
    action_desc = build_action_desc(blocked_tool_name, blocked_command, blocked_extras)

    # 推断 source
    source = "heuristic"
    if blocked_extras.get("pre_hook_count", 0) > 0 or blocked_extras.get("post_hook_count", 0) > 0:
        source = "react_agent"
    if blocked_extras.get("result_kind") == "blocked":
        source = "react_agent"

    # 构建 trace 链
    trace_chain = _build_trace_chain(trace_data)

    # 构建 timestamp
    timestamp = _format_timestamp(blocked_tc.get("start_time"))

    alert = {
        "session_id": session_id or "unknown",
        "trace": trace_chain,
        "user": user,
        "blocked_action": blocked_action,
        "violated_permission": violated_perm,
        "timestamp": timestamp,
        "risk_level": risk_level,
        "risk_type": risk_type,
        "action_desc": action_desc,
        "source": source,
    }

    return alert


def main():
    parser = argparse.ArgumentParser(
        description="将 moirai trace 数据转换为 block-analyzer 标准告警 JSON 格式"
    )
    parser.add_argument(
        "--trace-id", required=False,
        help="要导出的 TRACE_ID"
    )
    parser.add_argument(
        "--db", default="~/.moirai/traces.db",
        help="moirai traces.db 路径 (默认: ~/.moirai/traces.db)"
    )
    parser.add_argument(
        "--trace-file", required=False,
        help="直接从本地 trace JSON 文件读取（用于测试，跳过 moirai export）"
    )
    parser.add_argument(
        "--moirai-bin", required=False,
        help="moirai 可执行文件路径 (默认自动查找)"
    )
    parser.add_argument(
        "--output", "-o", required=True,
        help="输出的告警 JSON 文件路径"
    )

    args = parser.parse_args()

    if not args.trace_id and not args.trace_file:
        parser.error("必须指定 --trace-id 或 --trace-file")

    if args.trace_file:
        args.trace_file = os.path.abspath(os.path.expanduser(args.trace_file))
    if args.moirai_bin:
        args.moirai_bin = os.path.abspath(os.path.expanduser(args.moirai_bin))
    args.output = os.path.abspath(os.path.expanduser(args.output))

    if args.trace_id and not args.moirai_bin:
        moirai_found = find_moirai_binary()
        if moirai_found is None:
            print(
                "提示: 使用 --trace-id 模式需要 moirai 可执行文件，但未找到。\n"
                "请通过 --moirai-bin 参数指定，例如：\n"
                "  --moirai-bin /path/to/target/release/moirai\n"
                "\n"
                "同时请确认 --db 参数指向正确的 traces.db 文件路径，例如：\n"
                "  --db ~/.moirai/traces.db",
                file=sys.stderr,
            )
            sys.exit(1)
        args.moirai_bin = moirai_found

    # 加载 trace 数据
    if args.trace_file:
        print(f"从文件加载 trace 数据: {args.trace_file}", file=sys.stderr)
        trace_data = load_trace_from_file(args.trace_file)
    else:
        trace_data = export_trace(args.trace_id, args.db, args.moirai_bin)

    # 转换
    alert = trace_to_alert(trace_data, trace_id=args.trace_id)

    # 写入输出文件
    output_path = args.output
    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(alert, f, ensure_ascii=False, indent=2)

    print(f"转换完成: {output_path}", file=sys.stderr)
    print(f"  session_id: {alert['session_id']}", file=sys.stderr)
    print(f"  blocked_action: {alert['blocked_action']}", file=sys.stderr)
    print(f"  risk_type: {alert['risk_type']}", file=sys.stderr)
    print(f"  risk_level: {alert['risk_level']}", file=sys.stderr)


if __name__ == "__main__":
    main()
