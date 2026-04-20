"""LLM 深度分析器 — 结合 Skill 规则进行安全判断

将匹配到的 Skill 检测规则注入 LLM prompt，指导 LLM 进行安全判断。
轻量化实现：直接注入 skill 规则到 prompt，进行深度语义分析。
"""

import json
import logging
import time

from ..config import Config
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

        # 5. 调用 LLM（带重试）
        max_retries = config.retry.max_retries
        retry_interval = config.retry.retry_interval

        for attempt in range(1, max_retries + 1):
            try:
                llm_response = call_llm(
                    prompt=judge_prompt,
                    timeout=config.timeout.prompt2_timeout,  # 复用 prompt2 超时配置
                    config=config.llm,
                )
                judgment = self._parse_llm_response(llm_response)
                judgment.source = "llm"
                return judgment

            except (TimeoutError, ValueError, Exception) as e:
                logger.warning(
                    "LLM 安全分析第 %d 次调用失败: %s", attempt, e
                )
                if attempt < max_retries:
                    time.sleep(retry_interval)
                else:
                    # LLM 分析全部失败
                    raise LLMAnalysisFailure(
                        f"LLM 安全分析全部失败（{max_retries} 次）"
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
