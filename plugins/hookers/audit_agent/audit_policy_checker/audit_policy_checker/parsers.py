"""LLM 返回结果解析 — 从 LLM 响应中提取 TOML policy、JSON 合规性判断和安全判断"""

import json
import re
import sys

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib


def parse_policy_from_llm(response: str) -> str:
    """
    从 LLM 响应中提取 TOML 格式的 policy。

    LLM 可能在 TOML 前后添加 markdown 代码块标记或其他文字，
    本函数负责提取纯净的 TOML 内容。

    Args:
        response: LLM 返回的原始文本

    Returns:
        str: 提取的 TOML 格式 policy 字符串

    Raises:
        ValueError: 无法提取有效的 TOML 内容
    """
    # 策略1：提取 ```toml ... ``` 代码块
    toml_block_pattern = r"```toml\s*\n(.*?)```"
    match = re.search(toml_block_pattern, response, re.DOTALL)
    if match:
        toml_str = match.group(1).strip()
        _validate_toml(toml_str)
        return toml_str

    # 策略2：提取 ``` ... ``` 代码块（无语言标记）
    code_block_pattern = r"```\s*\n(.*?)```"
    match = re.search(code_block_pattern, response, re.DOTALL)
    if match:
        toml_str = match.group(1).strip()
        try:
            _validate_toml(toml_str)
            return toml_str
        except ValueError:
            pass  # 不是有效 TOML，继续尝试其他策略

    # 策略3：将整个响应作为 TOML 解析
    toml_str = response.strip()
    _validate_toml(toml_str)
    return toml_str


def _validate_toml(toml_str: str) -> None:
    """
    验证字符串是否为合法的 TOML 格式。

    Args:
        toml_str: 待验证的 TOML 字符串

    Raises:
        ValueError: TOML 格式非法
    """
    try:
        tomllib.loads(toml_str)
    except Exception as e:
        raise ValueError(f"TOML 解析失败: {e}") from e


def parse_compliance_from_llm(response: str) -> dict:
    """
    从 LLM 响应中解析合规性判断结果（JSON 格式）。

    LLM 可能在 JSON 前后添加 markdown 代码块标记或其他文字，
    本函数负责提取纯净的 JSON 内容并解析。

    Args:
        response: LLM 返回的原始文本

    Returns:
        dict: 包含 compliant (bool)、analysis (str)、violation_detail (str) 的字典

    Raises:
        ValueError: 无法提取有效的 JSON 内容
    """
    # 策略1：提取 ```json ... ``` 代码块
    json_block_pattern = r"```json\s*\n(.*?)```"
    match = re.search(json_block_pattern, response, re.DOTALL)
    if match:
        return _parse_json_object(match.group(1).strip())

    # 策略2：提取 ``` ... ``` 代码块
    code_block_pattern = r"```\s*\n(.*?)```"
    match = re.search(code_block_pattern, response, re.DOTALL)
    if match:
        try:
            return _parse_json_object(match.group(1).strip())
        except ValueError:
            pass

    # 策略3：在整个响应中查找 JSON 对象
    json_object_pattern = r"\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}"
    match = re.search(json_object_pattern, response, re.DOTALL)
    if match:
        try:
            return _parse_json_object(match.group(0).strip())
        except ValueError:
            pass

    # 策略4：将整个响应作为 JSON 解析
    return _parse_json_object(response.strip())


def _parse_json_object(json_str: str) -> dict:
    """
    解析 JSON 字符串并校验必要字段。

    Args:
        json_str: JSON 字符串

    Returns:
        dict: 解析后的字典

    Raises:
        ValueError: JSON 解析失败或缺少必要字段
    """
    try:
        result = json.loads(json_str)
    except json.JSONDecodeError as e:
        raise ValueError(f"JSON 解析失败: {e}") from e

    if not isinstance(result, dict):
        raise ValueError(f"JSON 解析结果不是对象，而是 {type(result).__name__}")

    # 校验必要字段
    if "compliant" not in result:
        raise ValueError("JSON 缺少必要字段 'compliant'")

    # 将 compliant 转换为 bool
    compliant_value = result["compliant"]
    if isinstance(compliant_value, str):
        compliant_lower = compliant_value.lower()
        if compliant_lower in ("true", "是"):
            result["compliant"] = True
        elif compliant_lower in ("false", "否"):
            result["compliant"] = False
        else:
            raise ValueError(f"'compliant' 字段值无法转为布尔: {compliant_value}")
    elif not isinstance(compliant_value, bool):
        result["compliant"] = bool(compliant_value)

    # 补充缺失字段
    result.setdefault("analysis", "")
    result.setdefault("violation_detail", "")

    return result


def parse_security_judgment(response: str) -> "SecurityJudgment":
    """
    从 LLM 响应中解析安全判断结果（JSON 格式）。

    期望的 JSON 格式：
    {
      "allowed": true/false,
      "reason": "具体原因",
      "risk_level": "low|medium|high|critical",
      "risk_type": "风险类别",
      "confidence": 0-100,
      "action_desc": "动作描述"
    }

    Args:
        response: LLM 返回的原始文本

    Returns:
        SecurityJudgment: 安全判断结果

    Raises:
        ValueError: 无法提取有效的 JSON 内容
    """
    from .security.types import SecurityJudgment

    # 策略1：提取 ```json ... ``` 代码块
    json_block_pattern = r"```json\s*\n(.*?)```"
    match = re.search(json_block_pattern, response, re.DOTALL)
    if match:
        return _parse_security_json(match.group(1).strip())

    # 策略2：提取 ``` ... ``` 代码块
    code_block_pattern = r"```\s*\n(.*?)```"
    match = re.search(code_block_pattern, response, re.DOTALL)
    if match:
        try:
            return _parse_security_json(match.group(1).strip())
        except ValueError:
            pass

    # 策略3：在整个响应中查找 JSON 对象
    json_object_pattern = r"\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}"
    match = re.search(json_object_pattern, response, re.DOTALL)
    if match:
        try:
            return _parse_security_json(match.group(0).strip())
        except ValueError:
            pass

    # 策略4：将整个响应作为 JSON 解析
    return _parse_security_json(response.strip())


def _parse_security_json(json_str: str) -> "SecurityJudgment":
    """
    解析安全判断 JSON 并构建 SecurityJudgment。

    Args:
        json_str: JSON 字符串

    Returns:
        SecurityJudgment: 安全判断结果

    Raises:
        ValueError: JSON 解析失败
    """
    from .security.types import SecurityJudgment

    try:
        result = json.loads(json_str)
    except json.JSONDecodeError as e:
        raise ValueError(f"安全判断 JSON 解析失败: {e}") from e

    if not isinstance(result, dict):
        raise ValueError(f"安全判断 JSON 解析结果不是对象，而是 {type(result).__name__}")

    # 解析 allowed 字段
    allowed_value = result.get("allowed", False)
    if isinstance(allowed_value, str):
        allowed_lower = allowed_value.lower()
        allowed = allowed_lower in ("true", "是", "yes", "1")
    elif isinstance(allowed_value, bool):
        allowed = allowed_value
    else:
        allowed = bool(allowed_value)

    # 解析 risk_level
    risk_level = result.get("risk_level", "low")
    if risk_level not in ("low", "medium", "high", "critical"):
        risk_level = "low"

    # 解析 confidence
    confidence = result.get("confidence", 50)
    try:
        confidence = int(confidence)
        confidence = max(0, min(100, confidence))
    except (ValueError, TypeError):
        confidence = 50

    return SecurityJudgment(
        allowed=allowed,
        reason=result.get("reason", ""),
        risk_level=risk_level,
        risk_type=result.get("risk_type", ""),
        confidence=confidence,
        source="llm",
        action_desc=result.get("action_desc", ""),
    )
