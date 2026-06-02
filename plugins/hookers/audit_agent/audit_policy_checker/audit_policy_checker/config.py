"""配置项管理 — 从 config.json 文件加载配置"""

import json
import os
from dataclasses import dataclass, field
from pathlib import Path

# 默认配置文件路径：与 config.py 同级目录下的 config.json
DEFAULT_CONFIG_PATH = Path(__file__).parent / "config.json"


@dataclass
class LLMConfig:
    """LLM 调用配置（支持多种 Provider）"""

    api_key: str = ""  # API Key
    model: str = "anthropic/claude-3.5-sonnet"  # 模型名称
    temperature: float = 0.1  # 低温度以确保输出稳定
    base_url: str = "https://openrouter.ai/api/v1"  # API 地址
    provider: str = ""  # Provider 名称（如 openrouter, openai, deepseek 等），用于 Headers 适配


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
        provider=llm_data.get("provider", ""),
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


# Provider 默认 base_url 映射（与 xiaoo-app/src/tui/services/provider.rs 保持一致）
PROVIDER_BASE_URLS: dict[str, str] = {
    "openai": "https://api.openai.com/v1",
    "openrouter": "https://openrouter.ai/api/v1",
    "groq": "https://api.groq.com/openai/v1",
    "mistral": "https://api.mistral.ai/v1",
    "together": "https://api.together.xyz/v1",
    "xai": "https://api.x.ai/v1",
    "xai-grok": "https://api.x.ai/v1",
    "deepseek": "https://api.deepseek.com",
    "gitcode": "https://api-ai.gitcode.com/v1",
    "ollama": "http://localhost:11434",
    "zai-coding-plan": "https://api.z.ai/api/coding/paas/v4",
    "zhipu-coding-plan": "https://api.z.ai/api/coding/paas/v4",
    "zhipuai-coding-plan": "https://api.z.ai/api/coding/paas/v4",
    # Zhipu/Z.AI 主站（与 xiaoo provider_registry.rs 保持一致）
    "zhipu": "https://open.bigmodel.cn/api/paas/v4",
    "bigmodel": "https://open.bigmodel.cn/api/paas/v4",
    "zai": "https://open.bigmodel.cn/api/paas/v4",
    "z-ai": "https://open.bigmodel.cn/api/paas/v4",
    "z.ai": "https://open.bigmodel.cn/api/paas/v4",
    "zai-cn": "https://open.bigmodel.cn/api/paas/v4",
    "zai-china": "https://open.bigmodel.cn/api/paas/v4",
    "zai-global": "https://open.bigmodel.cn/api/paas/v4",
    "glm-cn": "https://open.bigmodel.cn/api/paas/v4",
}

# Provider 默认 api_key_env 映射（与 xiaoo-app 保持一致）
PROVIDER_API_KEY_ENVS: dict[str, str] = {
    "openai": "OPENAI_API_KEY",
    "anthropic": "ANTHROPIC_API_KEY",
    "claude": "ANTHROPIC_API_KEY",
    "gemini": "GEMINI_API_KEY",
    "google": "GEMINI_API_KEY",
    "openrouter": "OPENROUTER_API_KEY",
    "groq": "GROQ_API_KEY",
    "mistral": "MISTRAL_API_KEY",
    "xai": "XAI_API_KEY",
    "xai-grok": "XAI_API_KEY",
    "deepseek": "DEEPSEEK_API_KEY",
    "zai": "ZHIPU_API_KEY",
    "zai-global": "ZHIPU_API_KEY",
    "z.ai": "ZHIPU_API_KEY",
    "zai-cn": "ZHIPU_API_KEY",
    "zai-china": "ZHIPU_API_KEY",
    "bigmodel": "ZHIPU_API_KEY",
    "zhipu": "ZHIPU_API_KEY",
    "glm-cn": "ZHIPU_API_KEY",
    "zai-coding-plan": "ZHIPU_API_KEY",
    "zhipu-coding-plan": "ZHIPU_API_KEY",
    "zhipuai-coding-plan": "ZHIPU_API_KEY",
    "glm": "GLM_API_KEY",
    "glm-global": "GLM_API_KEY",
    "gitcode": "GITCODE_API_KEY",
}


def load_config_with_xiaoo_fallback() -> Config:
    """
    加载配置，自动从 xiaoo 配置文件中 fallback LLM 配置。

    加载优先级：
    1. audit_policy_checker/config.json（完整配置）
    2. 环境变量 AUDIT_CONFIG_PATH 指定的路径
    3. xiaoo ~/.config/xiaoo/config.toml 中的 LLM 配置覆盖

    对于 LLM 的配置：
    - model: 从 xiaoo config.toml 读取
    - provider: 从 xiaoo config.toml 读取，用于确定默认 base_url
    - base_url: 优先使用 xiaoo config.toml 中的 api_base，否则用 provider 默认值
    - api_key: 通过 api_key_env 指定的环境变量获取

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
                # 覆盖 model
                if model := llm.get("model"):
                    cfg.llm.model = model

                # 获取 provider（用于确定默认 base_url 和 Headers 适配）
                provider = llm.get("provider", "").lower()
                if provider:
                    cfg.llm.provider = provider

                # 覆盖 base_url：优先使用 api_base，否则用 provider 默认值
                if api_base := llm.get("api_base"):
                    cfg.llm.base_url = api_base
                elif provider:
                    default_base = PROVIDER_BASE_URLS.get(provider, "")
                    if default_base:
                        cfg.llm.base_url = default_base

                # 通过 api_key_env 环境变量获取 api_key
                api_key_env = llm.get("api_key_env", "")
                if api_key_env:
                    env_api_key = os.getenv(api_key_env)
                    if env_api_key:
                        cfg.llm.api_key = env_api_key
                elif provider:
                    # 使用 provider 默认的 api_key_env
                    default_key_env = PROVIDER_API_KEY_ENVS.get(provider, "")
                    if default_key_env:
                        env_api_key = os.getenv(default_key_env)
                        if env_api_key:
                            cfg.llm.api_key = env_api_key
        except Exception:
            pass

    return cfg


# ========== audit_settings.json 加载逻辑 ==========
# 优先级：环境变量 > audit_settings.json > 默认值

# audit_settings.json 默认路径：与 config.py 同级目录
DEFAULT_AUDIT_SETTINGS_PATH = Path(__file__).parent / "audit_settings.json"


def load_audit_settings(settings_path: str | Path | None = None) -> dict:
    """
    从 audit_settings.json 加载审计配置。

    Args:
        settings_path: 配置文件路径，为 None 时使用默认路径

    Returns:
        dict: 审计配置字典

    Raises:
        FileNotFoundError: 配置文件不存在
        json.JSONDecodeError: 配置文件 JSON 格式错误
    """
    if settings_path is not None:
        path = Path(settings_path)
    else:
        path = DEFAULT_AUDIT_SETTINGS_PATH

    if not path.exists():
        return {}

    with open(path, "r", encoding="utf-8") as f:
        # 过滤掉 _comment 键
        data = json.load(f)
        return {k: v for k, v in data.items() if not k.startswith("_")}


# 全局审计设置缓存（延迟加载）
_audit_settings: dict | None = None


def get_audit_settings() -> dict:
    """获取全局审计设置（延迟加载）"""
    global _audit_settings
    if _audit_settings is None:
        _audit_settings = load_audit_settings()
    return _audit_settings


def get_audit_setting(key: str, default: str | int | bool = "") -> str:
    """
    获取审计配置值，优先级：环境变量 > audit_settings.json > 默认值

    Args:
        key: 配置键名（如 AUDIT_DISABLE_LLM_LAYER3）
        default: 默认值

    Returns:
        str: 配置值（环境变量优先，否则从 audit_settings.json 读取，否则返回默认值）
    """
    # 1. 环境变量优先
    env_value = os.environ.get(key)
    if env_value is not None:
        return env_value

    # 2. 从 audit_settings.json 读取
    settings = get_audit_settings()
    if key in settings:
        return str(settings[key])

    # 3. 返回默认值
    return str(default)


def is_llm_layer3_enabled() -> bool:
    """判断是否启用层3 LLM 深度分析"""
    disable_value = get_audit_setting("AUDIT_DISABLE_LLM_LAYER3", "")
    return disable_value != "1"


def get_llm_timeout() -> int:
    """获取 LLM 超时时间（秒）"""
    timeout = get_audit_setting("AUDIT_LLM_TIMEOUT", 300)
    try:
        return int(timeout)
    except ValueError:
        return 300


def get_log_path() -> str:
    """获取 LLM prompt 日志路径"""
    return get_audit_setting("AUDIT_LOG_PATH", "")


def is_policy_gen_enabled() -> bool:
    """判断是否启用策略生成"""
    enable_value = get_audit_setting("AUDIT_ENABLE_POLICY_GEN", "")
    return enable_value == "1"
