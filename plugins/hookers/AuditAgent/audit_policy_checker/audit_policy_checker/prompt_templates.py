"""Prompt 模板管理 — 加载和实例化 PROMPT1/PROMPT2 模板"""

import os
from pathlib import Path

# 模板文件目录
TEMPLATES_DIR = Path(__file__).parent / "templates"


def _load_template(filename: str) -> str:
    """从 templates 目录加载模板文件内容"""
    filepath = TEMPLATES_DIR / filename
    if not filepath.exists():
        raise FileNotFoundError(f"模板文件不存在: {filepath}")
    return filepath.read_text(encoding="utf-8")


def _load_policy_mapping_doc() -> str:
    """加载 prompt-policy 映射参考文档"""
    return _load_template("policy_mapping.md")


# 缓存已加载的模板
_template_cache: dict[str, str] = {}


def get_prompt1(prompt_session: str) -> str:
    """
    生成 PROMPT1 的完整内容：根据用户 prompt 生成最小 policy。

    Args:
        prompt_session: 用户输入的原始 prompt

    Returns:
        str: 实例化后的 PROMPT1 完整文本
    """
    if "prompt1_template.txt" not in _template_cache:
        _template_cache["prompt1_template.txt"] = _load_template("prompt1_template.txt")

    template = _template_cache["prompt1_template.txt"]
    mapping_doc = _load_policy_mapping_doc()

    return template.format(
        prompt_policy_mapping_doc=mapping_doc,
        prompt_session=prompt_session,
    )


def get_prompt2(
    policy_a_next: str,
    action_type: str,
    action_detail: str,
    reason: str,
    action_history: list[dict],
) -> str:
    """
    生成 PROMPT2 的完整内容：判定动作是否在 policy 允许范围内。

    Args:
        policy_a_next: 当前生效的 policy（TOML 格式字符串）
        action_type: 下一步动作类型
        action_detail: 下一步动作详情
        reason: 执行该动作的理由
        action_history: 历史动作序列，格式为 [{"name": "a1"}, ...]

    Returns:
        str: 实例化后的 PROMPT2 完整文本
    """
    if "prompt2_template.txt" not in _template_cache:
        _template_cache["prompt2_template.txt"] = _load_template("prompt2_template.txt")

    template = _template_cache["prompt2_template.txt"]

    # 格式化历史动作序列
    if not action_history:
        history_text = "（无历史动作）"
    else:
        lines = []
        for i, action in enumerate(action_history, 1):
            name = action.get("name", "未知动作")
            lines.append(f"  {i}. {name}")
        history_text = "\n".join(lines)

    return template.format(
        policy_a_next=policy_a_next,
        action_type=action_type,
        action_detail=action_detail,
        reason=reason,
        action_history_formatted=history_text,
    )


def get_security_judge_prompt(
    prompt_session: str,
    action_history: str,
    a_next: dict,
    reason: str,
    skills_text: str,
    hints_text: str,
) -> str:
    """
    生成安全判断 prompt 的完整内容。

    Args:
        prompt_session: 用户输入的原始 prompt
        action_history: 已格式化的历史动作序列文本
        a_next: 下一步动作
        reason: 执行该动作的理由
        skills_text: 匹配到的安全检测规则文本
        hints_text: 前置检测结果提示文本

    Returns:
        str: 实例化后的安全判断 prompt 完整文本
    """
    if "security_judge_template.txt" not in _template_cache:
        _template_cache["security_judge_template.txt"] = _load_template(
            "security_judge_template.txt"
        )

    template = _template_cache["security_judge_template.txt"]

    return template.format(
        prompt_session=prompt_session,
        action_history=action_history,
        action_type=a_next.get("action_type", "unknown"),
        action_detail=a_next.get("action_detail", ""),
        reason=reason,
        skills_text=skills_text,
        hints_text=hints_text,
    )
