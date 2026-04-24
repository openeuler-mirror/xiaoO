"""LLM 深度分析器 — 结合 Skill 规则进行安全判断

将匹配到的 Skill 检测规则注入 LLM prompt，指导 LLM 进行安全判断。
轻量化实现：直接注入 skill 规则到 prompt，进行深度语义分析。
"""

import json
import logging
import time
from concurrent.futures import ThreadPoolExecutor, TimeoutError as FuturesTimeoutError
from datetime import datetime

from ..config import Config, get_log_path, get_llm_timeout
from ..llm_client import call_llm
from .skill_engine import SkillEngine
from .types import HeuristicResult, LogicRuleResult, SecurityJudgment

logger = logging.getLogger(__name__)


class LLMAnalysisFailure(Exception):
    """LLM 分析失败的异常"""
    pass


class LLMAnalyzer:
    """LLM 深度分析器 — 结合 Skill 规则进行安全判断"""

    def __init__(self, skill_engine: SkillEngine | None = None):
        self._skill_engine = skill_engine or SkillEngine()

    def analyze(
        self,
        prompt_session: str,
        action_history: list[dict[str, object]],
        a_next: dict[str, str],
        reason: str,
        heuristic_result: HeuristicResult,
        logic_result: LogicRuleResult,
        config: Config,
    ) -> SecurityJudgment:
        """
        使用 LLM + Skill 进行深度安全分析。

        Args:
            prompt_session: 用户输入的原始 prompt
            action_history: 历史动作序列
            a_next: 下一步动作
            reason: 执行理由
            heuristic_result: 启发式检测结果（作为提示信息注入）
            logic_result: 逻辑规则检测结果（作为提示信息注入）
            config: 全局配置

        Returns:
            SecurityJudgment: 安全判断结果
        """
        from ..prompt_templates import get_security_judge_prompt

        # 1. 匹配相关 Skill
        matched_skills = self._skill_engine.match_skills(a_next, reason)
        skills_text = self._skill_engine.format_skills_for_prompt(matched_skills)

        # 2. 格式化历史动作
        history_text = self._format_action_history(action_history)

        # 3. 格式化启发式/逻辑规则提示
        hints_text = self._format_hints(heuristic_result, logic_result)

        # 4. 生成安全判断 prompt
        judge_prompt = get_security_judge_prompt(
            prompt_session=prompt_session,
            action_history=history_text,
            a_next=a_next,
            reason=reason,
            skills_text=skills_text,
            hints_text=hints_text,
        )

        # 4.5 打印 prompt 到日志文件（如果配置了 AUDIT_LOG_PATH）
        log_path = get_log_path()
        if log_path:
            self._log_prompt(judge_prompt, a_next, log_path)

        # 5. 调用 LLM（带超时和重试）
        max_retries = config.retry.max_retries
        retry_interval = config.retry.retry_interval
        _call_start = time.monotonic()  # 记录调用开始时间，用于精确计算耗时
        llm_timeout = get_llm_timeout()

        # 根据 LLM 超时时间自动计算单次 call_llm 超时时间
        # 公式：总超时 = N 次调用 + (N-1) 次重试间隔
        # 所以：单次超时 = (总超时 - 重试间隔 * (max_retries - 1)) / max_retries
        single_call_timeout = (
            llm_timeout - retry_interval * (max_retries - 1)
        ) / max_retries
        # 确保单次超时至少 10 秒
        single_call_timeout = max(single_call_timeout, 10.0)

        for attempt in range(1, max_retries + 1):
            try:
                # 使用 ThreadPoolExecutor 包装 LLM 调用，支持超时控制
                with ThreadPoolExecutor(max_workers=1) as executor:
                    future = executor.submit(
                        call_llm,
                        prompt=judge_prompt,
                        timeout=single_call_timeout,
                        config=config.llm,
                    )
                    llm_response = future.result(timeout=llm_timeout)

                judgment = self._parse_llm_response(llm_response)
                judgment.source = "llm"
                return judgment

            except FuturesTimeoutError:
                # ThreadPoolExecutor 超时：LLM 调用在 llm_timeout 内未返回
                elapsed = time.monotonic() - _call_start
                logger.warning(
                    "LLM 安全分析超时（耗时 %.1fs，上限 %ds），第 %d/%d 次尝试",
                    elapsed,
                    llm_timeout,
                    attempt,
                    max_retries,
                )
                if attempt < max_retries:
                    time.sleep(retry_interval)
                else:
                    raise LLMAnalysisFailure(
                        f"LLM 安全分析超时（耗时 {elapsed:.1f}s，上限 {llm_timeout}s，已重试 {max_retries} 次）"
                    )

            except TimeoutError as e:
                # call_llm 抛出的真正超时（网络/读取超时）
                logger.warning(
                    "LLM 调用网络超时: %s，第 %d/%d 次尝试",
                    e,
                    attempt,
                    max_retries,
                )
                if attempt < max_retries:
                    time.sleep(retry_interval)
                else:
                    raise LLMAnalysisFailure(
                        f"LLM 调用网络超时（已重试 {max_retries} 次）: {e}"
                    )

            except Exception as e:
                # API 错误（如并发限制 500006、400/500 错误等），非超时
                error_type = type(e).__name__
                logger.warning(
                    "LLM 安全分析第 %d/%d 次调用失败 [%s]: %s",
                    attempt,
                    max_retries,
                    error_type,
                    e,
                )
                if attempt < max_retries:
                    time.sleep(retry_interval)
                else:
                    raise LLMAnalysisFailure(
                        f"LLM 安全分析失败（{max_retries} 次）[{error_type}]: {e}"
                    )

    def _parse_llm_response(self, response: str) -> SecurityJudgment:
        """解析 LLM 返回的安全判断结果"""
        from ..parsers import parse_security_judgment

        return parse_security_judgment(response)

    @staticmethod
    def _format_action_history(action_history: list[dict[str, object]]) -> str:
        """格式化历史动作序列"""
        if not action_history:
            return "（无历史动作）"

        lines = []
        for i, action in enumerate(action_history, 1):
            if isinstance(action, dict):
                name = action.get("name", action.get("action_type", "未知"))
                detail = action.get("action_detail", "")
                if detail:
                    lines.append(f"  {i}. {name}: {detail}")
                else:
                    lines.append(f"  {i}. {name}")
            else:
                lines.append(f"  {i}. {action}")
        return "\n".join(lines)

    @staticmethod
    def _format_hints(
        heuristic_result: HeuristicResult,
        logic_result: LogicRuleResult,
    ) -> str:
        """格式化启发式/逻辑规则检测提示"""
        hints = []

        if heuristic_result.hit:
            hints.append(
                f"⚠️ 启发式检测发现风险 [风险等级: {heuristic_result.risk_level}]:\n"
                f"   类型: {heuristic_result.risk_type}\n"
                f"   原因: {heuristic_result.reason}\n"
                f"   匹配模式: {', '.join(heuristic_result.matched_patterns)}"
            )

        if logic_result.hit:
            hints.append(
                f"⚠️ 逻辑规则检测发现风险 [风险等级: {logic_result.risk_level}]:\n"
                f"   违反规则: {logic_result.violated_rule}\n"
                f"   类型: {logic_result.risk_type}\n"
                f"   原因: {logic_result.reason}"
            )

        if not hints:
            return "启发式检测和逻辑规则检测均未发现风险。"

        return "\n\n".join(hints)

    @staticmethod
    def _log_prompt(prompt: str, a_next: dict[str, str], log_path: str) -> None:
        """将 LLM prompt 写入日志文件。"""
        if not log_path:
            return
        try:
            action_type = a_next.get("action_type", "unknown")
            action_detail = a_next.get("action_detail", "")[:100]
            log_line = (
                f"[{datetime.now().isoformat(timespec='milliseconds')}] "
                f"[LLM_PROMPT] action_type={action_type}, action_detail={action_detail}... "
                f"prompt_length={len(prompt)}\n"
                f"{prompt}\n"
                f"--- END PROMPT ---\n"
            )
            with open(log_path, "a", encoding="utf-8") as f:
                f.write(log_line)
        except Exception as e:
            logger.warning("Failed to write LLM prompt to log: %s", e)
