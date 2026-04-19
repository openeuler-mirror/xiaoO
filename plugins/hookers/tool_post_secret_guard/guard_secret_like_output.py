import json
import os
import re
import sys


def load_jsonc(path: str) -> dict:
    """加载 JSONC 文件（支持 // 和 /* */ 注释）"""
    with open(path, "r", encoding="utf-8") as f:
        content = f.read()
    # 去掉 /* */ 注释
    content = re.sub(r"/\*.*?\*/", "", content, flags=re.DOTALL)
    # 去掉 // 注释
    content = re.sub(r"//[^\n]*", "", content)
    return json.loads(content)


def parse_sensitive_patterns(raw: list) -> list[dict]:
    """解析 sensitive_patterns 配置，统一为 [{description, pattern, compiled}] 格式

    支持两种格式:
      - 字符串: "sk-[A-Za-z0-9_-]+"
      - 对象: {"description": "...", "pattern": ["sk-...", "..."]}
    """
    result = []
    for item in raw:
        if isinstance(item, str):
            result.append({
                "description": item,
                "pattern": [item],
            })
        elif isinstance(item, dict):
            patterns = item.get("pattern", [])
            if isinstance(patterns, str):
                patterns = [patterns]
            result.append({
                "description": item.get("description", "未命名模式"),
                "pattern": patterns,
            })
    return result


config = load_jsonc(
    os.path.join(os.path.dirname(__file__), "tool_post_secret_guard_config.jsonc")
)

REDACTED_SUFFIX = config.get("REDACTED_SUFFIX", "[REDACTED]")
SENSITIVE_PATTERNS = parse_sensitive_patterns(config.get("sensitive_patterns", []))

# 空 patterns 时跳过检测，避免空正则匹配所有位置
if SENSITIVE_PATTERNS:
    all_regex_strs = []
    for entry in SENSITIVE_PATTERNS:
        for p in entry["pattern"]:
            all_regex_strs.append(p)
    SECRET_LIKE_PATTERN = re.compile(
        "|".join(f"({pattern})" for pattern in all_regex_strs)
    )
else:
    SECRET_LIKE_PATTERN = None


def find_matched_entry(matched_text: str) -> dict | None:
    """根据匹配到的文本，找到对应的配置项"""
    for entry in SENSITIVE_PATTERNS:
        for p in entry["pattern"]:
            if re.search(p, matched_text):
                return entry
    return None


def redact_secret_like_values(text: str) -> str:
    if SECRET_LIKE_PATTERN is None:
        return text

    def replace_with_marker(match):
        matched_text = match.group(0)
        entry = find_matched_entry(matched_text)
        if entry:
            return f"[{entry['description']}{REDACTED_SUFFIX}]"
        return f"[REDACTED]"

    return SECRET_LIKE_PATTERN.sub(replace_with_marker, text)


def contains_secret_like_value(text: str) -> bool:
    if SECRET_LIKE_PATTERN is None:
        return False
    return SECRET_LIKE_PATTERN.search(text) is not None


def get_matched_descriptions(text: str) -> list[str]:
    """获取文本中匹配到的敏感信息描述列表"""
    if SECRET_LIKE_PATTERN is None:
        return []

    matched = []
    for entry in SENSITIVE_PATTERNS:
        for p in entry["pattern"]:
            if re.search(p, text):
                matched.append(entry["description"])
                break
    return list(dict.fromkeys(matched))  # 去重保序


def main() -> int:
    payload = json.load(sys.stdin)
    if payload.get("stage") != "post":
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    call = payload.get("call", {})
    if call.get("tool_name") not in ["bash", "file_read", "read_file"]:
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    outcome = payload.get("outcome", {})
    if outcome.get("type") != "success":
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    output = outcome.get("output")
    if not isinstance(output, str):
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    try:
        bash_output = json.loads(output)
    except json.JSONDecodeError:
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    if not isinstance(bash_output, dict):
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    stdout_value = bash_output.get("stdout")
    stderr_value = bash_output.get("stderr")
    if not isinstance(stdout_value, str) or not isinstance(stderr_value, str):
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    interaction = payload.get("interaction")
    if interaction is not None:
        response = interaction.get("response", {})
        allowed = response.get("allowed") is True
        if allowed:
            json.dump({"action": "final", "result": "accept"}, sys.stdout)
            return 0

        sanitized_output = dict(bash_output)
        if contains_secret_like_value(stdout_value):
            sanitized_output["stdout"] = redact_secret_like_values(stdout_value)
        if contains_secret_like_value(stderr_value):
            sanitized_output["stderr"] = redact_secret_like_values(stderr_value)

        json.dump(
            {
                "action": "final",
                "result": "transform",
                "modified_output": json.dumps(sanitized_output),
            },
            sys.stdout,
        )
        return 0

    fields = []
    if contains_secret_like_value(stdout_value):
        fields.append("stdout")
    if contains_secret_like_value(stderr_value):
        fields.append("stderr")

    if not fields:
        json.dump({"result": "accept"}, sys.stdout)
        return 0

    field_list = ", ".join(fields)
    descriptions = get_matched_descriptions(stdout_value) + get_matched_descriptions(stderr_value)
    desc_list = ", ".join(dict.fromkeys(descriptions))

    json.dump(
        {
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": (
                    f"可疑行为！检测到工具输出的 {field_list} 包含疑似敏感信息（{desc_list}）。"
                    "是否允许保留原始输出给该工具（如不保留则会MASK掉）？"
                ),
            },
            "continuation": {
                "policy": "secret_output_review",
                "matched_fields": fields,
            },
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
