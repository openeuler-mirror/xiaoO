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
    """解析 sensitive_patterns 配置，统一为 [{description, pattern, compiled}] 格式"""
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


config_path = os.path.join(os.path.dirname(__file__), "llm_pre_secret_guard_config.jsonc")
config = load_jsonc(config_path)

REDACTED_SUFFIX = config.get("REDACTED_SUFFIX", "[REDACTED]")
SENSITIVE_PATTERNS = parse_sensitive_patterns(config.get("sensitive_patterns", []))

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
    return list(dict.fromkeys(matched))


# 一个块里可能承载文本的字段。ContentBlock（crates/agent-types/src/llm/message.rs）
# 有 text / tool_use / tool_result / image / document 五种，除了 text 之外，
# tool_result 的 output、image 与 document 的 description 也都是正文，
# tool_use 的 input 则是任意 JSON（命令行参数常在里面）。
# 只认 text 时，这些位置里的密钥既不脱敏也不询问，直接随请求发出去。
TEXT_BEARING_FIELDS = ("text", "output", "description")


def block_texts(block: dict) -> list[str]:
    """取出一个块里所有可能含密钥的文本"""
    texts = [
        block[field]
        for field in TEXT_BEARING_FIELDS
        if isinstance(block.get(field), str)
    ]
    if block.get("type") == "tool_use" and block.get("input") is not None:
        texts.append(json.dumps(block["input"], ensure_ascii=False))
    return texts


def redact_value(value):
    """递归脱敏任意 JSON（tool_use.input 是 Value，不是一个字符串）"""
    if isinstance(value, str):
        return redact_secret_like_values(value)
    if isinstance(value, list):
        return [redact_value(v) for v in value]
    if isinstance(value, dict):
        return {k: redact_value(v) for k, v in value.items()}
    return value


def extract_text_from_content(content) -> str:
    """从 message content 提取文本

    ChatMessage 序列化格式:
    - role: str
    - blocks: [{type: "text", text: "..."}, ...]
    - 没有 content 字段！
    """
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        texts = []
        for block in content:
            if isinstance(block, dict):
                texts.extend(block_texts(block))
        return " ".join(texts)
    return ""


def redact_content(content):
    """对 content (blocks 数组) 进行脱敏"""
    if isinstance(content, str):
        return redact_secret_like_values(content)
    if isinstance(content, list):
        result = []
        for block in content:
            if isinstance(block, dict):
                new_block = dict(block)
                for field in TEXT_BEARING_FIELDS:
                    if isinstance(new_block.get(field), str):
                        new_block[field] = redact_secret_like_values(new_block[field])
                if new_block.get("type") == "tool_use" and new_block.get("input") is not None:
                    new_block["input"] = redact_value(new_block["input"])
                result.append(new_block)
            else:
                result.append(block)
        return result
    return content


def check_messages_for_secrets(messages: list) -> tuple[bool, list[str]]:
    """检查 messages 是否包含敏感信息，返回 (是否包含, 描述列表)"""
    if SECRET_LIKE_PATTERN is None:
        return False, []

    found = False
    descriptions = []

    for msg in messages:
        blocks = msg.get("blocks", [])
        text = extract_text_from_content(blocks)
        if contains_secret_like_value(text):
            found = True
            descriptions.extend(get_matched_descriptions(text))

    return found, list(dict.fromkeys(descriptions))


def redact_messages(messages: list) -> list:
    """对 messages 中的敏感内容进行脱敏"""
    result = []
    for msg in messages:
        new_msg = dict(msg)
        if "blocks" in new_msg:
            new_msg["blocks"] = redact_content(new_msg["blocks"])
        result.append(new_msg)
    return result


def main() -> int:
    payload = json.load(sys.stdin)
    if payload.get("stage") != "pre":
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    request = payload.get("request", {})
    messages = request.get("messages", [])

    if not messages:
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    has_secrets, descriptions = check_messages_for_secrets(messages)

    if not has_secrets:
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    interaction = payload.get("interaction")
    if interaction is not None:
        response = interaction.get("response", {})
        allowed = response.get("allowed") is True
        if allowed:
            json.dump({"action": "final", "result": "allow"}, sys.stdout)
            return 0

        # 用户拒绝，脱敏后发送
        redacted_request = dict(request)
        redacted_request["messages"] = redact_messages(messages)

        json.dump(
            {
                "action": "final",
                "result": "transform",
                "modified_request": redacted_request,
            },
            sys.stdout,
        )
        return 0

    # 首次检测到敏感信息，询问用户
    desc_list = ", ".join(descriptions)
    json.dump(
        {
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": (
                    f"检测到即将发送的 LLM 请求包含疑似敏感信息（{desc_list}）。"
                    "是否允许原样发送？（拒绝将脱敏后发送）"
                ),
            },
            "continuation": {
                "policy": "secret_request_review",
            },
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
