import json
import sys
from pathlib import Path

CONFIG_KEY_ALERT_LOG_PATH = "alert_log_path"
CONFIG_KEY_DENIAL_MARKERS = "denial_markers"
DEFAULT_ALERT_LOG_PATH = "/tmp/alert.log"

DEFAULT_DENIAL_MARKERS = (
    "Error: Execution error: Sandbox setup error:",
    "Error: Execution error: Filter error:",
    "Sandbox capability error:",
    "Violation triggered",
    "Landlock setup failed:",
    "Seccomp setup failed:",
    "Mount isolation failed:",
    "Namespace setup failed:",
    "Permission denied",
    "/etc/shadow",
    "Could not resolve host",
)


def find_matched_output_lines(
    stdout_value: str, stderr_value: str, denial_markers: tuple[str, ...]
) -> list[tuple[str, str]]:
    matched_output: list[tuple[str, str]] = []

    for line in stderr_value.splitlines():
        for marker in denial_markers:
            if marker in line:
                matched_output.append((marker, line))
                break

    for line in stdout_value.splitlines():
        for marker in denial_markers:
            if marker in line:
                matched_output.append((marker, line))
                break

    return matched_output


def build_security_notice(
    matched_output_lines: list[tuple[str, str]],
    trace_id: str | None,
    span_id: str | None,
) -> str:
    rendered_lines = "\n".join(line for _, line in matched_output_lines)
    rendered_markers = ", ".join(
        dict.fromkeys(marker for marker, _ in matched_output_lines)
    )

    return (
        '<SYSTEM ALERT type="security_notice">\n'
        "SUSPICIOUS BEHAVIOR is blocked\n"
        "POTENTIAL SECURITY RISK DETECTED. SUSPICIOUS BEHAVIORS block records:\n"
        f"{rendered_lines}\n"
        f"matched_markers={rendered_markers}\n"
        f"trace_id={trace_id or 'unknown'}\n"
        f"span_id={span_id or 'unknown'}\n"
        "**MUST DO**: Stop current task and report to user immediately.\n"
        "remind user of possible vulnerability or misconfiguration in the executed command.\n"
        "</SYSTEM ALERT>"
    )


def extract_bash_output(payload: dict) -> dict | None:
    outcome = payload.get("outcome", {})
    if outcome.get("type") != "success":
        return None

    output = outcome.get("output")
    if not isinstance(output, str):
        return None

    try:
        parsed = json.loads(output)
    except json.JSONDecodeError:
        return None

    return parsed if isinstance(parsed, dict) else None


def resolve_denial_markers(config: dict) -> tuple[str, ...]:
    configured = config.get(CONFIG_KEY_DENIAL_MARKERS)
    if not isinstance(configured, list):
        return DEFAULT_DENIAL_MARKERS

    markers = [marker for marker in configured if isinstance(marker, str) and marker]
    return tuple(markers) if markers else DEFAULT_DENIAL_MARKERS


def append_alert(
    log_path: str,
    trace_id: str | None,
    span_id: str | None,
    matched_output_lines: list[tuple[str, str]],
    stdout_value: str,
    stderr_value: str,
    exit_code: object,
) -> None:
    rendered_markers = ", ".join(
        dict.fromkeys(marker for marker, _ in matched_output_lines)
    )
    Path(log_path).parent.mkdir(parents=True, exist_ok=True)
    with Path(log_path).open("a", encoding="utf-8") as handle:
        handle.write("alert_begin\n")
        handle.write(f"trace_id={trace_id or 'unknown'}\n")
        handle.write(f"span_id={span_id or 'unknown'}\n")
        handle.write(f"matched_markers={rendered_markers}\n")
        handle.write("matched_lines<<EOF\n")
        for _, line in matched_output_lines:
            handle.write(f"{line}\n")
        handle.write("EOF\n")
        handle.write(f"exit_code={exit_code}\n")
        handle.write("stderr<<EOF\n")
        handle.write(stderr_value)
        if not stderr_value.endswith("\n"):
            handle.write("\n")
        handle.write("EOF\n")
        handle.write("stdout<<EOF\n")
        handle.write(stdout_value)
        if not stdout_value.endswith("\n"):
            handle.write("\n")
        handle.write("EOF\n")
        handle.write("alert_end\n")


def main() -> int:
    payload = json.load(sys.stdin)
    if payload.get("stage") != "post":
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    call = payload.get("call", {})
    if call.get("tool_name") != "bash":
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    bash_output = extract_bash_output(payload)
    if bash_output is None:
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    stdout_value = bash_output.get("stdout")
    stderr_value = bash_output.get("stderr")
    exit_code = bash_output.get("exit_code")
    if not isinstance(stdout_value, str) or not isinstance(stderr_value, str):
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    definition = payload.get("definition", {})
    config = definition.get("config", {})
    denial_markers = resolve_denial_markers(config)
    matched_output_lines = find_matched_output_lines(
        stdout_value, stderr_value, denial_markers
    )
    if not matched_output_lines:
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    metadata = payload.get("metadata", {})
    log_path = config.get(CONFIG_KEY_ALERT_LOG_PATH, DEFAULT_ALERT_LOG_PATH)
    trace_id = metadata.get("trace_id")
    span_id = metadata.get("span_id")

    append_alert(
        log_path,
        trace_id,
        span_id,
        matched_output_lines,
        stdout_value,
        stderr_value,
        exit_code,
    )
    notice = build_security_notice(matched_output_lines, trace_id, span_id)
    transformed_output = dict(bash_output)
    transformed_output["stdout"] = "\n".join(
        [
            # stdout_value,
            notice,
        ]
    )
    transformed_output["stderr"] = "\n".join(
        [
            # stderr_value,
            notice,
        ]
    )

    json.dump(
        {
            "result": "transform",
            "modified_output": json.dumps(transformed_output),
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
