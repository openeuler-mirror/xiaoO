"""LLM 调用客户端 — 基于 OpenAI SDK 调用多种大模型（支持不同 Provider 的 Headers）

新增功能：返回 LLM 调用的 token 用量统计（prompt_tokens, completion_tokens, total_tokens），
用于 audit-agent 的 token 消耗追踪和 Dashboard 展示。
"""

from __future__ import annotations

from dataclasses import dataclass
from openai import OpenAI

from .config import LLMConfig, get_default_config


@dataclass
class LLMResult:
    """LLM 调用结果 — 包含返回文本和 token 用量"""
    content: str                # LLM 返回的文本内容
    prompt_tokens: int = 0      # 输入 prompt 的 token 数
    completion_tokens: int = 0  # LLM 生成的 token 数
    total_tokens: int = 0       # 总 token 数（prompt + completion）
    model: str = ""             # 实际使用的模型名称（可能被 Provider 路径修改）

    def to_usage_dict(self) -> dict:
        """转换为 token 用量字典（用于统计记录）"""
        return {
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total_tokens,
            "model": self.model,
        }


# OpenRouter 固定 base_url（向后兼容）
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"

# ── Provider 特殊 Headers 配置 ──────────────────────────────────────────────
# 键为 provider 名称（由 detect_provider 返回），值为需要注入的额外 headers。
#
# OpenRouter: HTTP-Referer + X-OpenRouter-Title 用于优先级路由和应用标识
#   参考: https://openrouter.ai/docs/api-reference/overview/header
#
# 注意：以下 provider 通过 OpenAI SDK（兼容模式）调用时需要额外 headers：
# - OpenRouter: 必须添加 HTTP-Referer 和 X-OpenRouter-Title，否则被低优先级路由
# - xAI: 建议添加 HTTP-Referer 用于来源标识
#
# 以下 provider 使用 OpenAI 兼容 API，无需额外 headers：
# - OpenAI, DeepSeek, Groq, Mistral, Together, GitCode, Minimax, Zhipu/Z.AI
#
# 以下 provider 有专门的 API 协议，不走 OpenAI SDK：
# - Anthropic (x-api-key + anthropic-version)
# - Gemini (key 作为 query 参数)
# - Ollama (本地，无认证)
PROVIDER_HEADERS: dict[str, dict[str, str]] = {
    "openrouter": {
        "HTTP-Referer": "shturl.cc/LlVD5pyWViHjM",
        "X-OpenRouter-Title": "xiaoO-audit-agent",
    },
    "xai": {
        "HTTP-Referer": "shturl.cc/LlVD5pyWViHjM",
    },
}


def detect_provider(base_url: str, provider_hint: str = "") -> str:
    """
    从 base_url 或 provider 提示推测 provider 类型。

    Args:
        base_url: API base URL
        provider_hint: 可选的 provider 名称提示（优先于 URL 推测）

    Returns:
        str: provider 标识符（如 openrouter, openai, deepseek 等）
    """
    # 优先使用显式 provider 提示
    if provider_hint:
        hint = provider_hint.lower()
        # 规范化别名（与 xiaoO provider_registry.rs 保持一致）
        if hint in ("claude", "anthropic"):
            return "anthropic"
        if hint in ("google", "gemini"):
            return "gemini"
        if hint in ("zai-coding-plan", "zhipu-coding-plan", "zhipuai-coding-plan"):
            return "zai-coding-plan"
        if hint in ("zai", "z-ai", "z.ai", "zai-cn", "zai-china", "zhipu", "glm-cn", "bigmodel"):
            return "zhipu"
        if hint in ("xai", "xai-grok"):
            return "xai"
        if hint in ("minimax", "minimax-openai"):
            return "minimax"
        if hint in ("minimax-anthropic"):
            return "minimax-anthropic"
        if hint in ("other", "custom", "local"):
            return "openai-compatible"
        return hint

    # 回退到 URL 推测
    url = base_url.lower()

    if "openrouter.ai" in url:
        return "openrouter"
    if "api.openai.com" in url:
        return "openai"
    if "anthropic.com" in url or "api.anthropic.com" in url:
        return "anthropic"
    if "generativelanguage.googleapis.com" in url or "gemini" in url:
        return "gemini"
    if "api.deepseek.com" in url:
        return "deepseek"
    if "api.groq.com" in url:
        return "groq"
    if "api.mistral.ai" in url:
        return "mistral"
    if "api.together.xyz" in url:
        return "together"
    if "api.x.ai" in url:
        return "xai"
    if "api.minimaxi.com" in url:
        return "minimax"
    if "api-ai.gitcode.com" in url:
        return "gitcode"
    if "api.z.ai" in url:
        return "zai-coding-plan"
    if "open.bigmodel.cn" in url or "bigmodel" in url:
        return "zhipu"
    if ":11434" in url:
        return "ollama"

    return "openai-compatible"


def get_provider_headers(provider: str) -> dict[str, str]:
    """
    获取 provider 特定的 headers。
    """
    return PROVIDER_HEADERS.get(provider, {})


def create_client(config: LLMConfig | None = None) -> OpenAI:
    """
    创建 LLM 客户端（根据 Provider 自动添加特殊 Headers）。
    """
    if config is None:
        config = get_default_config().llm

    base_url = config.base_url or OPENROUTER_BASE_URL
    provider = detect_provider(base_url, config.provider)
    extra_headers = get_provider_headers(provider)

    return OpenAI(
        api_key=config.api_key,
        base_url=base_url,
        timeout=30.0,
        default_headers=extra_headers if extra_headers else None,
    )


def call_llm(
    prompt: str,
    timeout: float = 5.0,
    config: LLMConfig | None = None,
) -> LLMResult:
    """
    调用大模型（根据 Provider 自动添加特殊 Headers）。

    Args:
        prompt: 完整的 prompt 文本
        timeout: 超时时间（秒）
        config: LLM 配置，为 None 时使用默认配置

    Returns:
        LLMResult: 包含返回文本和 token 用量的结果对象

    Raises:
        TimeoutError: 调用超时
        Exception: API 调用失败
    """
    if config is None:
        config = get_default_config().llm

    base_url = config.base_url or OPENROUTER_BASE_URL
    provider = detect_provider(base_url, config.provider)
    extra_headers = get_provider_headers(provider)

    client = OpenAI(
        api_key=config.api_key,
        base_url=base_url,
        timeout=timeout,
        default_headers=extra_headers if extra_headers else None,
    )

    try:
        response = client.chat.completions.create(
            model=config.model,
            messages=[
                {"role": "user", "content": prompt},
            ],
            temperature=config.temperature,
        )

        message = response.choices[0].message
        content = message.content
        finish_reason = response.choices[0].finish_reason

        if not content:
            if finish_reason == "length":
                raise ValueError(
                    "LLM 推理耗尽 token（finish_reason=length），无实际输出"
                )
            raise ValueError("LLM 返回内容为空")

        # 清理 BOM 和前后空白
        cleaned = content.strip()
        if cleaned.startswith("﻿"):
            cleaned = cleaned[1:]

        # 提取 token 用量统计
        usage = response.usage
        prompt_tokens = usage.prompt_tokens if usage else 0
        completion_tokens = usage.completion_tokens if usage else 0
        total_tokens = usage.total_tokens if usage else 0
        model_used = response.model if response.model else config.model

        return LLMResult(
            content=cleaned,
            prompt_tokens=prompt_tokens,
            completion_tokens=completion_tokens,
            total_tokens=total_tokens,
            model=model_used,
        )

    except Exception as e:
        error_msg = str(e).lower()
        if "timeout" in error_msg or "timed out" in error_msg:
            raise TimeoutError(f"LLM 调用超时（{timeout}s）: {e}") from e
        raise
