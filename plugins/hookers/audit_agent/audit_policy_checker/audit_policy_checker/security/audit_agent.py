"""xiaoO Audit Agent 主协调器 — 三层防御安全判断

整体流程：
1. 启发式静态检测（关键命令 + 注入检测 + 用户敏感规则）→ high/critical 直接 Deny
2. 逻辑规则检测（read_before_write + 意图一致性 + 敏感路径 + 危险模式）→ high/critical 直接 Deny
3. LLM + Skill 深度分析（启发式和逻辑规则结果作为提示注入）

优化：白名单只读工具（grep/read/glob等）在启发式检测未命中 high/critical 时，跳过后续检测。
"""

import logging
from pathlib import Path

from ..config import Config
from .heuristic_detector import HeuristicDetector, SAFE_ACTION_TYPES, is_safe_bash_command
from .llm_analyzer import LLMAnalyzer
from .logic_rules import LogicRulesChecker
from .skill_engine import SkillEngine
from .types import HeuristicResult, SecurityJudgment

logger = logging.getLogger(__name__)

# 白名单只读工具：这些工具在启发式检测未命中 high/critical 时可快速放行
READONLY_SAFE_TOOLS = frozenset({
    "ask_user_question", "glob", "grep", "list_dir", "count_text_length",
    "read", "file_read", "head", "tail", "ls",
})


class xiaoOSecBot:
    """xiaoO Audit Agent 安全检测 Agent — 三层防御体系"""

    def __init__(
        self,
        rules_path: str | Path | None = None,
        skills_dir: str | Path | None = None,
    ):
        self._heuristic_detector = HeuristicDetector(rules_path=rules_path)
        self._logic_rules_checker = LogicRulesChecker()
        self._skill_engine = SkillEngine(skills_dir=skills_dir)
        self._llm_analyzer = LLMAnalyzer(skill_engine=self._skill_engine)

    def judge(
        self,
        prompt_session: str,
        action_history: list[dict[str, object]],
        a_next: dict[str, str],
        reason: str,
        config: Config,
    ) -> SecurityJudgment:
        """
        执行三层防御安全判断。

        Args:
            prompt_session: 用户输入的原始 prompt
            action_history: 历史动作序列
            a_next: 下一步动作
            reason: 执行理由
            config: 全局配置

        Returns:
            SecurityJudgment: 安全判断结果
        """
        security_cfg = config.security

        # 如果安全检测被禁用，默认 Allow
        if not security_cfg.enabled:
            logger.info("安全检测已禁用，默认允许")
            return SecurityJudgment(
                allowed=True,
                reason="安全检测已禁用",
                risk_level="low",
                source="disabled",
            )

        violated_layers: list[str] = []

        # ========== 层1: 启发式静态检测 ==========
        if security_cfg.heuristic_enabled:
            heuristic_result = self._heuristic_detector.detect(a_next, reason)
            logger.debug(
                "启发式检测结果: hit=%s, risk_level=%s, reason=%s",
                heuristic_result.hit,
                heuristic_result.risk_level,
                heuristic_result.reason,
            )
            if heuristic_result.hit:
                violated_layers.append("1.1")
                if heuristic_result.risk_level in ("high", "critical"):
                    logger.info(
                        "启发式检测拦截: risk_level=%s, reason=%s",
                        heuristic_result.risk_level,
                        heuristic_result.reason,
                    )
                    return SecurityJudgment(
                        allowed=False,
                        reason=heuristic_result.reason,
                        risk_level=heuristic_result.risk_level,
                        risk_type=heuristic_result.risk_type,
                        confidence=95,
                        source="heuristic",
                        action_desc=a_next.get("action_detail", ""),
                        violated_layers=violated_layers,
                    )
        else:
            heuristic_result = HeuristicResult(hit=False)

        # ========== 白名单只读工具快速放行 ==========
        action_type = a_next.get("action_type", "").lower()
        action_detail = a_next.get("action_detail", "").lower()
        is_readonly_safe = action_type in READONLY_SAFE_TOOLS or action_type in SAFE_ACTION_TYPES

        # 扩展：检查安全的 bash 子命令
        is_safe_bash = (
            action_type == "bash"
            and is_safe_bash_command(action_detail)
            and heuristic_result.risk_level not in ("high", "critical")
        )

        if is_readonly_safe and heuristic_result.risk_level not in ("high", "critical"):
            logger.info(
                "白名单只读工具快速放行: action_type=%s, heuristic_risk=%s",
                action_type,
                heuristic_result.risk_level or "none",
            )
            return SecurityJudgment(
                allowed=True,
                reason=f"白名单只读工具且未检测到敏感路径访问: {action_type}",
                risk_level=heuristic_result.risk_level or "low",
                source="whitelist_bypass",
                action_desc=action_detail,
            )

        if is_safe_bash:
            logger.info(
                "安全 bash 子命令快速放行: command=%s",
                action_detail[:50],
            )
            return SecurityJudgment(
                allowed=True,
                reason=f"安全的只读 bash 命令: {action_detail[:100]}",
                risk_level="low",
                source="whitelist_bypass",
                action_desc=action_detail,
            )

        # ========== 层2: 逻辑规则检测 ==========
        if security_cfg.logic_rules_enabled:
            logic_result = self._logic_rules_checker.check(
                prompt_session, action_history, a_next, reason
            )
            logger.debug(
                "逻辑规则检测结果: hit=%s, risk_level=%s, reason=%s",
                logic_result.hit,
                logic_result.risk_level,
                logic_result.reason,
            )
            if logic_result.hit:
                violated_layers.append("1.2")
                if logic_result.risk_level in ("high", "critical"):
                    logger.info(
                        "逻辑规则检测拦截: violated_rule=%s, reason=%s",
                        logic_result.violated_rule,
                        logic_result.reason,
                    )
                    return SecurityJudgment(
                        allowed=False,
                        reason=logic_result.reason,
                        risk_level=logic_result.risk_level,
                        risk_type=logic_result.risk_type,
                        confidence=90,
                        source="logic_rule",
                        action_desc=a_next.get("action_detail", ""),
                        violated_layers=violated_layers,
                    )
        else:
            from .types import LogicRuleResult
            logic_result = LogicRuleResult(hit=False)

        # ========== 层3: LLM + Skill 深度分析 ==========
        if security_cfg.llm_analysis_enabled:
            try:
                llm_judgment = self._llm_analyzer.analyze(
                    prompt_session=prompt_session,
                    action_history=action_history,
                    a_next=a_next,
                    reason=reason,
                    heuristic_result=heuristic_result,
                    logic_result=logic_result,
                    config=config,
                )
            except Exception as e:
                logger.warning("LLM 分析异常: %s", e)
                if violated_layers:
                    return SecurityJudgment(
                        allowed=False,
                        reason=f"LLM 分析失败且前序检测已拦截: {e}",
                        risk_level="high",
                        source="llm_failure_with_violation",
                        action_desc=a_next.get("action_detail", ""),
                        violated_layers=violated_layers,
                    )
                return SecurityJudgment(
                    allowed=True,
                    reason=f"LLM 分析失败（warn-allow）: {e}",
                    risk_level="low",
                    source="llm_failure_fallback",
                    action_desc=a_next.get("action_detail", ""),
                )
            if not llm_judgment.allowed:
                violated_layers.append("1.3")
                llm_judgment.violated_layers = violated_layers
            return llm_judgment
        else:
            # LLM 分析禁用，且前两层未拦截，默认 Allow
            return SecurityJudgment(
                allowed=True,
                reason="前两层检测均未发现高风险问题，LLM 分析已禁用",
                risk_level="low",
                source="heuristic_and_logic_only",
                action_desc=a_next.get("action_detail", ""),
            )


def judge_security(
    prompt_session: str,
    action_history: list[dict[str, object]],
    a_next: dict[str, str],
    reason: str,
    config: Config,
) -> SecurityJudgment:
    """
    便捷函数：执行安全判断。

    Args:
        prompt_session: 用户输入的原始 prompt
        action_history: 历史动作序列
        a_next: 下一步动作
        reason: 执行理由
        config: 全局配置

    Returns:
        SecurityJudgment: 安全判断结果
    """
    agent = xiaoOSecBot(
        rules_path=config.security.rules_path,
        skills_dir=config.security.skills_dir,
    )
    return agent.judge(prompt_session, action_history, a_next, reason, config)
