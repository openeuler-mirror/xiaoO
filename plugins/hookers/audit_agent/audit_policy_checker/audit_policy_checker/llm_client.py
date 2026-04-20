"""LLM 调用客户端 — 基于 OpenAI SDK 通过 OpenRouter 调用多种大模型"""

from openai import OpenAI

from .config import LLMConfig, get_default_config

# OpenRouter 固定 base_url
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"


def create_client(config: LLMConfig | None = None) -> OpenAI:
    """
    创建 OpenRouter LLM 客户端。

    Args:
        config: LLM 配置，为 None 时使用默认配置

    Returns:
        OpenAI: 客户端实例（兼容 OpenRouter API）
    """
    if config is None:
        config = get_default_config().llm

    base_url = config.base_url or OPENROUTER_BASE_URL

    return OpenAI(
        api_key=config.api_key,
        base_url=base_url,
        timeout=30.0,  # HTTP 连接超时（秒）
    )


def call_llm(
    prompt: str,
    timeout: float = 5.0,
    config: LLMConfig | None = None,
) -> str:
    """
    调用大模型（通过 OpenRouter，兼容 OpenAI SDK）。

    Args:
        prompt: 完整的 prompt 文本
        timeout: 超时时间（秒）
        config: LLM 配置，为 None 时使用默认配置

    Returns:
        str: 模型返回的文本

    Raises:
        TimeoutError: 调用超时
        Exception: API 调用失败
    """
    if config is None:
        config = get_default_config().llm

    base_url = config.base_url or OPENROUTER_BASE_URL

    client = OpenAI(
        api_key=config.api_key,
        base_url=base_url,
        timeout=timeout,
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
        if cleaned.startswith("\ufeff"):
            cleaned = cleaned[1:]

        return cleaned

    except Exception as e:
        error_msg = str(e).lower()
        if "timeout" in error_msg or "timed out" in error_msg:
            raise TimeoutError(f"LLM 调用超时（{timeout}s）: {e}") from e
        raise
