"""日志工具 — 记录每次审计调用的输入、LLM 交互、决策结果和耗时"""

import time
import functools
from typing import Any

from loguru import logger

from .config import get_default_config


def setup_logger(log_level: str | None = None) -> None:
    """
    初始化日志配置。

    Args:
        log_level: 日志级别，为 None 时使用默认配置
    """
    level = log_level or get_default_config().log_level
    logger.remove()
    logger.add(
        sink="audit_policy_checker.log",
        level=level,
        format="{time:YYYY-MM-DD HH:mm:ss.SSS} | {level:<8} | {name}:{function}:{line} - {message}",
        rotation="10 MB",
        retention="7 days",
        encoding="utf-8",
    )
    logger.add(
        sink=lambda msg: print(msg, end=""),
        level=level,
        format="<green>{time:YYYY-MM-DD HH:mm:ss.SSS}</green> | <level>{level:<8}</level> | {message}",
        colorize=True,
    )


class AuditLogger:
    """审计日志记录器，记录单次审计调用的完整过程"""

    def __init__(self, session_id: str):
        self.session_id = session_id
        self.start_time = time.monotonic()
        self.step_times: dict[str, float] = {}
        self._step_start: float | None = None
        self._current_step: str | None = None

    def log_input(self, prompt_session: str, action_history: list[dict], a_next: dict, reason: str) -> None:
        """记录工具输入"""
        logger.info(
            f"[{self.session_id}] 审计输入 | "
            f"prompt={prompt_session[:100]}... | "
            f"历史动作数={len(action_history)} | "
            f"下一步动作={a_next.get('action_type', '?')}:{a_next.get('action_detail', '?')[:80]} | "
            f"理由={reason[:80]}"
        )

    def start_step(self, step_name: str) -> None:
        """开始计时某个步骤"""
        self._current_step = step_name
        self._step_start = time.monotonic()
        logger.debug(f"[{self.session_id}] 开始步骤: {step_name}")

    def end_step(self, step_name: str, detail: str = "") -> None:
        """结束计时某个步骤"""
        if self._step_start is not None:
            elapsed = time.monotonic() - self._step_start
            self.step_times[step_name] = elapsed
            detail_str = f" | {detail}" if detail else ""
            logger.info(f"[{self.session_id}] 步骤完成: {step_name} | 耗时={elapsed:.3f}s{detail_str}")
            self._step_start = None
            self._current_step = None

    def log_llm_interaction(self, step: str, prompt_summary: str, response_summary: str) -> None:
        """记录 LLM 交互内容"""
        logger.debug(
            f"[{self.session_id}] LLM交互 | 步骤={step} | "
            f"PROMPT摘要={prompt_summary[:200]} | "
            f"响应摘要={response_summary[:200]}"
        )

    def log_result(self, decision: str, reason: str, policy: str = "") -> None:
        """记录最终决策结果"""
        total_time = time.monotonic() - self.start_time
        steps_info = ", ".join(f"{k}={v:.3f}s" for k, v in self.step_times.items())
        logger.info(
            f"[{self.session_id}] 审计结果 | 决策={decision} | "
            f"理由={reason[:150]} | "
            f"步骤耗时=[{steps_info}] | 总耗时={total_time:.3f}s"
        )
        if policy:
            logger.debug(f"[{self.session_id}] 生成的Policy:\n{policy}")

    def log_error(self, step: str, error: Exception) -> None:
        """记录错误"""
        logger.error(f"[{self.session_id}] 步骤异常: {step} | 错误={type(error).__name__}: {error}")
