"""Session 级别 Policy 缓存管理

同一 session 中同一 prompt 对应的 policy 只生成一次，后续从缓存读取。
预留缓存失效接口，供后续版本支持"历史动作序列中存在用户重新指定 prompt 时重新生成 policy"。
"""

import threading
from collections import OrderedDict

from .config import CacheConfig, get_default_config


class PolicyCache:
    """Session 级别的 policy 缓存，支持 LRU 淘汰"""

    def __init__(self, config: CacheConfig | None = None):
        self._config = config or get_default_config().cache
        self._cache: OrderedDict[str, str] = OrderedDict()  # session_id -> policy_toml_str
        self._lock = threading.Lock()

    def _cache_key(self, session_id: str, prompt_session: str) -> str:
        """生成缓存键：基于 session_id 和 prompt 内容"""
        return f"{session_id}::{prompt_session}"

    def get(self, session_id: str, prompt_session: str) -> str | None:
        """
        从缓存中获取 policy。

        Args:
            session_id: 会话唯一标识
            prompt_session: 用户输入的原始 prompt

        Returns:
            str | None: 缓存的 policy（TOML 格式字符串），未命中返回 None
        """
        if not self._config.enabled:
            return None

        key = self._cache_key(session_id, prompt_session)
        with self._lock:
            if key in self._cache:
                # LRU：移动到末尾
                self._cache.move_to_end(key)
                return self._cache[key]
            return None

    def put(self, session_id: str, prompt_session: str, policy: str) -> None:
        """
        将生成的 policy 存入缓存。

        Args:
            session_id: 会话唯一标识
            prompt_session: 用户输入的原始 prompt
            policy: TOML 格式的 policy 字符串
        """
        if not self._config.enabled:
            return

        key = self._cache_key(session_id, prompt_session)
        with self._lock:
            if key in self._cache:
                self._cache.move_to_end(key)
            self._cache[key] = policy

            # LRU 淘汰
            while len(self._cache) > self._config.max_size:
                self._cache.popitem(last=False)

    def invalidate(self, session_id: str, prompt_session: str) -> None:
        """
        使指定 session + prompt 的 policy 缓存失效。

        TODO: 预留接口 — 当历史动作序列中存在用户重新指定 prompt 时，
        需要使缓存失效并重新生成 policy。当前版本暂不实现该逻辑的自动触发，
        后续版本可根据 action_history 中的特定标记
        （如 action_type == "user_new_prompt"）调用此接口。

        Args:
            session_id: 会话唯一标识
            prompt_session: 用户输入的原始 prompt
        """
        key = self._cache_key(session_id, prompt_session)
        with self._lock:
            self._cache.pop(key, None)

    def invalidate_session(self, session_id: str) -> None:
        """
        使指定 session 的所有 policy 缓存失效。

        Args:
            session_id: 会话唯一标识
        """
        with self._lock:
            keys_to_remove = [k for k in self._cache if k.startswith(f"{session_id}::")]
            for key in keys_to_remove:
                del self._cache[key]

    def clear(self) -> None:
        """清空所有缓存"""
        with self._lock:
            self._cache.clear()

    def size(self) -> int:
        """返回当前缓存条目数"""
        with self._lock:
            return len(self._cache)


# 全局默认缓存实例（延迟初始化）
_default_cache: PolicyCache | None = None


def get_default_cache() -> PolicyCache:
    """获取全局默认缓存实例（延迟初始化）"""
    global _default_cache
    if _default_cache is None:
        _default_cache = PolicyCache()
    return _default_cache
