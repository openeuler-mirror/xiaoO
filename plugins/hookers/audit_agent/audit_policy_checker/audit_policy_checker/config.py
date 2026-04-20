"""配置项管理 — 从 config.json 文件加载配置"""

import json
import os
from dataclasses import dataclass, field
from pathlib import Path

# 默认配置文件路径：与 config.py 同级目录下的 config.json
DEFAULT_CONFIG_PATH = Path(__file__).parent / "config.json"


@dataclass
class LLMConfig:
    """LLM 调用配置（通过 OpenRouter 调用多种大模型）"""

    api_key: str = ""  # OpenRouter API Key
    model: str = "anthropic/claude-3.5-sonnet"  # 模型名称（OpenRouter 格式）
    temperature: float = 0.1  # 低温度以确保输出稳定
    base_url: str = "https://openrouter.ai/api/v1"  # OpenRouter API 地址


@dataclass
class TimeoutConfig:
    """超时配置"""

    total_timeout: float = 10.0  # 总超时
    prompt1_timeout: float = 5.0  # PROMPT1 LLM 调用超时
    prompt2_timeout: float = 4.0  # PROMPT2 LLM 调用超时
    step_interval: float = 0.0  # Step1 与 Step2 之间的间隔（秒），避免 API 限流


@dataclass
class CacheConfig:
    """缓存配置"""

    enabled: bool = True  # 是否启用 session 级别 policy 缓存
    max_size: int = 1000  # 最大缓存条目数


@dataclass
class RetryConfig:
    """重试配置"""

    max_retries: int = 3  # LLM 调用最大重试次数
    retry_interval: float = 2.0  # 重试间隔（秒）


@dataclass
class SecurityConfig:
    """安全检测配置"""

    enabled: bool = True  # 是否启用安全检测
    heuristic_enabled: bool = True  # 启发式检测开关
    logic_rules_enabled: bool = True  # 逻辑规则检测开关
    llm_analysis_enabled: bool = True  # LLM 深度分析开关
    rules_path: str = ""  # 用户规则文件路径（空则使用默认路径）
    skills_dir: str = ""  # Skills 目录路径（空则使用默认路径）


@dataclass
class Config:
    """全局配置"""

    llm: LLMConfig = field(default_factory=LLMConfig)
    timeout: TimeoutConfig = field(default_factory=TimeoutConfig)
    cache: CacheConfig = field(default_factory=CacheConfig)
    retry: RetryConfig = field(default_factory=RetryConfig)
    security: SecurityConfig = field(default_factory=SecurityConfig)
    log_level: str = "INFO"


def load_config(config_path: str | Path | None = None) -> Config:
    """
    从 config.json 文件加载配置。

    加载优先级：
    1. 指定的 config_path
    2. 环境变量 AUDIT_CONFIG_PATH 指定的路径
    3. 默认路径：audit_policy_checker/config.json

    对于 LLM 的 api_key，额外支持环境变量 OPENROUTER_API_KEY 覆盖文件中的值。

    Args:
        config_path: 配置文件路径，为 None 时按优先级自动查找

    Returns:
        Config: 加载后的配置对象

    Raises:
        FileNotFoundError: 配置文件不存在
        json.JSONDecodeError: 配置文件 JSON 格式错误
    """
    # 确定配置文件路径
    if config_path is not None:
        path = Path(config_path)
    elif os.getenv("AUDIT_CONFIG_PATH"):
        path = Path(os.getenv("AUDIT_CONFIG_PATH"))
    else:
        path = DEFAULT_CONFIG_PATH

    if not path.exists():
        raise FileNotFoundError(f"配置文件不存在: {path}。请创建 config.json 文件，可参考 config.json.example。")

    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)

    # 构建 LLMConfig
    llm_data = data.get("llm", {})
    llm_config = LLMConfig(
        api_key=llm_data.get("api_key", ""),
        model=llm_data.get("model", "anthropic/claude-3.5-sonnet"),
        temperature=llm_data.get("temperature", 0.1),
        base_url=llm_data.get("base_url", "https://openrouter.ai/api/v1"),
    )

    # 环境变量覆盖 api_key（优先级最高）
    env_api_key = os.getenv("OPENROUTER_API_KEY")
    if env_api_key:
        llm_config.api_key = env_api_key

    # 构建 TimeoutConfig
    timeout_data = data.get("timeout", {})
    timeout_config = TimeoutConfig(
        total_timeout=timeout_data.get("total_timeout", 10.0),
        prompt1_timeout=timeout_data.get("prompt1_timeout", 5.0),
        prompt2_timeout=timeout_data.get("prompt2_timeout", 4.0),
        step_interval=timeout_data.get("step_interval", 0.0),
    )

    # 构建 CacheConfig
    cache_data = data.get("cache", {})
    cache_config = CacheConfig(
        enabled=cache_data.get("enabled", True),
        max_size=cache_data.get("max_size", 1000),
    )

    # 构建 RetryConfig
    retry_data = data.get("retry", {})
    retry_config = RetryConfig(
        max_retries=retry_data.get("max_retries", 3),
        retry_interval=retry_data.get("retry_interval", 2.0),
    )

    # 构建 SecurityConfig
    sec_data = data.get("security", {})
    security_config = SecurityConfig(
        enabled=sec_data.get("enabled", True),
        heuristic_enabled=sec_data.get("heuristic_enabled", True),
        logic_rules_enabled=sec_data.get("logic_rules_enabled", True),
        llm_analysis_enabled=sec_data.get("llm_analysis_enabled", True),
        rules_path=sec_data.get("rules_path", ""),
        skills_dir=sec_data.get("skills_dir", ""),
    )

    # 构建 Config
    return Config(
        llm=llm_config,
        timeout=timeout_config,
        cache=cache_config,
        retry=retry_config,
        security=security_config,
        log_level=data.get("log_level", "INFO"),
    )


# 全局默认配置实例（延迟加载，首次访问时从 config.json 读取）
_default_config: Config | None = None


def get_default_config() -> Config:
    """获取全局默认配置（延迟加载）"""
    global _default_config
    if _default_config is None:
        _default_config = load_config_with_xiaoo_fallback()
    return _default_config


def load_config_with_xiaoo_fallback() -> Config:
    """
    加载配置，自动从 xiaoo 配置文件中 fallback model。

    加载优先级：
    1. audit_policy_checker/config.json（完整配置）
    2. 环境变量 AUDIT_CONFIG_PATH 指定的路径
    3. xiaoo ~/.config/xiaoo/config.toml 中的 model 字段覆盖

    对于 LLM 的 api_key，优先使用环境变量 OPENROUTER_API_KEY（已有逻辑）。

    Returns:
        Config: 加载后的配置对象
    """
    try:
        cfg = load_config()
    except FileNotFoundError:
        cfg = Config()

    xiaoo_config = Path.home() / ".config" / "xiaoo" / "config.toml"
    if xiaoo_config.exists():
        try:
            import tomllib
            data = tomllib.loads(xiaoo_config.read_text())
            if llm := data.get("llm"):
                if model := llm.get("model"):
                    cfg.llm.model = model
        except Exception:
            pass

    return cfg
