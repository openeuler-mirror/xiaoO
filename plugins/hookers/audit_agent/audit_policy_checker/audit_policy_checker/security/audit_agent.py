"""xiaoO Audit Agent 主协调器 — 三层防御 + 脚本内容静态扫描

整体流程：
1. 启发式静态检测（关键命令 + 注入检测 + 用户敏感规则）→ high/critical 直接 Deny
1.5 脚本内容预扫描（递归追踪 + 预处理 + L1/L2 规则复用扫描）→ high/critical 直接 Deny
2. 逻辑规则检测（read_before_write + 意图一致性 + 敏感路径 + 危险模式）→ high/critical 直接 Deny
   2.5 脚本内容 L2 扫描（对预处理后脚本内容做敏感路径/危险模式/意图偏离检测）
3. LLM + Skill 深度分析（启发式和逻辑规则结果作为提示注入）

优化：白名单只读工具（grep/read/glob等）在启发式检测未命中 high/critical 时，跳过后续检测。
脚本内容预扫描：对 bash 类型的脚本执行动作，递归追踪调用链中的脚本文件，
预处理后用 L1/L2 规则做静态扫描，能直接拦截的不走 L3，减少 token 消耗和不稳定性。

配置控制（优先级：环境变量 > runtime JSON 用户本地副本 > config.json 默认值）：
- 环境变量: AUDIT_DISABLE_L1=1 / AUDIT_DISABLE_L2=1 / AUDIT_DISABLE_LLM_LAYER3=1
- runtime JSON: ~/.config/xiaoo/audit_runtime.json 中的 layers 字段
- config.json: security.heuristic_enabled / logic_rules_enabled / llm_analysis_enabled
"""

import logging
from pathlib import Path

from ..config import Config, is_llm_layer3_enabled
from ..runtime_config import is_layer_enabled, get_disabled_l2_rules_with_skip_l3
from .heuristic_detector import (
    HeuristicDetector,
    is_fully_safe_bash_command, is_readonly_sensitive_bash_command,
    is_inline_script_command,
)
from .llm_analyzer import LLMAnalyzer
from .logic_rules import LogicRulesChecker
from .script_content_analyzer import (
    analyze_script_content,
    scan_script_chain_with_l2,
    ScriptChainAnalysisResult,
)
from .skill_engine import SkillEngine
from .types import HeuristicResult, SecurityJudgment

logger = logging.getLogger(__name__)

# 完全安全工具：跳过 Layer 2 + Layer 3（不具备读取文件内容的能力）
FULLY_SAFE_TOOLS = frozenset({
    "ask_user_question", "glob", "list_dir", "ls", "count_text_length",
    "filemgr-globfiles",
})

# 内置安全 Skill 白名单：这些是 xiaoO 系统自带的安全 Skill，加载时直接放行
TRUSTED_SKILLS = frozenset({
    "xiaoo-guardian",
})

# 安全但可能访问敏感路径：跳过 Layer 3，保留 Layer 2 敏感路径检测
READONLY_SENSITIVE_TOOLS = frozenset({
    "read", "file_read", "read_file", "head", "tail", "grep",
    "filemgr-readfile", "filemgr-grepfiles",
})

# 向后兼容
READONLY_SAFE_TOOLS = FULLY_SAFE_TOOLS | READONLY_SENSITIVE_TOOLS


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

        # ========== 内联脚本命令检查（假阳性防护） ==========
        # 对内联脚本命令（python -c, perl -e 等），Layer 1/2 的 file_access 模式
        # （如 /etc/shadow）不立即 Deny。因为敏感路径可能出现在字符串字面量中，
        # 而纯正则无法可靠区分字符串和执行路径。交给 Layer 3 LLM 做语义判断。
        action_detail = a_next.get("action_detail", "").lower()
        is_inline_script_cmd = is_inline_script_command(action_detail)
        skip_inline_file_access = False

        # ========== 层1: 启发式静态检测 ==========
        # 优先级：环境变量 > runtime JSON > config.json > 默认值
        l1_enabled = is_layer_enabled("L1_heuristic") and security_cfg.heuristic_enabled
        if l1_enabled:
            heuristic_result = self._heuristic_detector.detect(a_next, reason)
            logger.debug(
                "启发式检测结果: hit=%s, risk_level=%s, reason=%s",
                heuristic_result.hit,
                heuristic_result.risk_level,
                heuristic_result.reason,
            )
            if heuristic_result.hit:
                # 对内联脚本命令，file_access 风险不立即 Deny
                skip_inline_file_access = (
                    is_inline_script_cmd
                    and heuristic_result.risk_type == "file_access"
                )
                if heuristic_result.risk_level in ("high", "critical") and not skip_inline_file_access:
                    violated_layers.append("1.1")
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

        # ========== L1.5: 脚本内容预扫描（递归追踪 + L1/L2 规则复用） ==========
        # 对 bash 类型的脚本执行动作，递归追踪脚本调用链，
        # 预处理后用 L1 CommandPatternScanner 扫描，命中 high/critical 直接 Deny。
        # 这避免了"脚本内有 rm -rf / 但 L1 只看命令字段 bash deploy.sh 不命中"的漏报，
        # 也减少了 L3 的 token 消耗（L1 可拦截的场景不再走 LLM）。
        # 注意：此阶段 action_type/action_detail 局部变量尚未赋值
        # （它们在"白名单快速放行"阶段才定义），所以直接从 a_next 取值。
        script_chain_result: ScriptChainAnalysisResult | None = None
        _l15_action_type = a_next.get("action_type", "").lower()
        _l15_action_detail = a_next.get("action_detail", "").lower()
        if _l15_action_type == "bash":
            script_chain_result = analyze_script_content(_l15_action_type, _l15_action_detail)

            if script_chain_result and script_chain_result.nodes:
                for node in script_chain_result.nodes:
                    if node.l1_hit and node.l1_risk_level in ("high", "critical"):
                        violated_layers.append("1.1-script")
                        logger.info(
                            "脚本内容 L1 拦截: path=%s, depth=%d, risk_level=%s, reason=%s",
                            node.script_path, node.depth, node.l1_risk_level, node.l1_reason,
                        )
                        return SecurityJudgment(
                            allowed=False,
                            reason=f"脚本内容包含危险操作: {node.l1_reason} (文件: {node.script_path}, 深度: {node.depth})",
                            risk_level=node.l1_risk_level,
                            risk_type=node.l1_risk_type,
                            confidence=90,
                            source="heuristic_script_scan",
                            action_desc=a_next.get("action_detail", ""),
                            violated_layers=violated_layers,
                        )

        # ========== 白名单快速放行（两级） ==========
        action_type = a_next.get("action_type", "").lower()
        action_detail = a_next.get("action_detail", "").lower()
        heuristic_risk = heuristic_result.risk_level or "low"
        no_high_risk = heuristic_risk not in ("high", "critical")

        # --- 完全放行：跳过 L2 + L3 ---
        is_fully_safe_tool = action_type in FULLY_SAFE_TOOLS
        # 内置安全 Skill 直接放行（如 xiaoo-guardian）
        is_trusted_skill = (
            action_type == "skill"
            and action_detail in TRUSTED_SKILLS
        )
        is_fully_safe_bash = (
            action_type == "bash"
            and is_fully_safe_bash_command(action_detail)
            and no_high_risk
        )

        # 安全兜底：is_fully_safe_bash_command 只检查管道前的第一段命令，
        # 对于 "echo ... | passwd" 等管道命令，第一段 (echo) 是安全的，
        # 但完整命令包含危险模式。此处用 CommandPatternScanner 扫描完整命令，
        # 如果命中 high/critical 模式则不允许白名单放行。
        if is_fully_safe_bash:
            from .heuristic_detector import CommandPatternScanner
            _full_cmd_scanner = CommandPatternScanner()
            _full_scan = _full_cmd_scanner.scan(action_detail)
            if _full_scan.hit and _full_scan.risk_level in ("high", "critical"):
                is_fully_safe_bash = False
                logger.info(
                    "白名单放行覆盖: 完整命令命中高危模式 [%s]: %s",
                    _full_scan.risk_level, _full_scan.reason,
                )

        if (is_fully_safe_tool or is_trusted_skill) and no_high_risk:
            logger.info(
                "完全安全工具快速放行: action_type=%s, heuristic_risk=%s",
                action_type, heuristic_risk,
            )
            return SecurityJudgment(
                allowed=True,
                reason=f"完全安全工具，跳过深度分析: {action_type}",
                risk_level=heuristic_risk,
                source="whitelist_bypass",
                action_desc=action_detail,
            )

        if is_fully_safe_bash:
            logger.info("完全安全 bash 命令快速放行: command=%s", action_detail[:50])
            return SecurityJudgment(
                allowed=True,
                reason=f"安全的只读 bash 命令: {action_detail[:100]}",
                risk_level="low",
                source="whitelist_bypass",
                action_desc=action_detail,
            )

        # --- 轻量放行：跳过 L3，保留 L2 ---
        skip_llm = False
        if action_type in READONLY_SENSITIVE_TOOLS and no_high_risk:
            skip_llm = True
        elif (
            action_type == "bash"
            and is_readonly_sensitive_bash_command(action_detail)
            and no_high_risk
        ):
            skip_llm = True

        if skip_llm:
            logger.info(
                "只读敏感工具，跳过 LLM 分析: action_type=%s", action_type,
            )

        # ========== 层2: 逻辑规则检测 ==========
        # 优先级：环境变量 > runtime JSON > config.json > 默认值
        l2_enabled = is_layer_enabled("L2_logic_rules") and security_cfg.logic_rules_enabled
        if l2_enabled:
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
                # 内联脚本的 file_access 不立即 Deny，转 Layer 3 语义判断
                if logic_result.risk_level in ("high", "critical") and not skip_inline_file_access:
                    violated_layers.append("1.2")
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

        # ========== L2.5: 脚本内容 L2 扫描（对预处理后脚本内容做 L2 规则检测） ==========
        # 对脚本调用链中每个节点的预处理内容，复用 L2 的敏感路径/危险模式/意图偏离检测。
        # 只复用不依赖 action_history 的规则：
        #   - sensitive_path_access
        #   - dangerous_patterns
        #   - intent_consistency（用 prompt_session 判断脚本内容是否偏离用户意图）
        if script_chain_result and script_chain_result.nodes and l2_enabled:
            scan_script_chain_with_l2(
                script_chain_result, self._logic_rules_checker, prompt_session
            )
            for node in script_chain_result.nodes:
                if node.l2_hit and node.l2_risk_level in ("high", "critical"):
                    violated_layers.append("1.2-script")
                    logger.info(
                        "脚本内容 L2 拦截: path=%s, depth=%d, violated_rule=%s, reason=%s",
                        node.script_path, node.depth, node.l2_violated_rule, node.l2_reason,
                    )
                    return SecurityJudgment(
                        allowed=False,
                        reason=node.l2_reason,
                        risk_level=node.l2_risk_level,
                        risk_type=node.l2_risk_type,
                        confidence=85,
                        source="logic_rule_script_scan",
                        action_desc=a_next.get("action_detail", ""),
                        violated_layers=violated_layers,
                    )

        # ========== 层3: LLM + Skill 深度分析 ==========
        # 配置控制：环境变量 > runtime JSON > config.json > 默认值
        # skip_llm 由白名单快速放行逻辑设置，只读敏感工具跳过 LLM 分析
        l3_runtime_enabled = is_layer_enabled("L3_llm_analysis")
        llm_analysis_enabled = security_cfg.llm_analysis_enabled and is_llm_layer3_enabled() and l3_runtime_enabled and not skip_llm

        # 收集 L2 禁用且 skip_l3_on_disabled 的规则关键内容，
        # 命中时作为硬约束传给 L3：对这些路径/关键词必须放行，
        # 但 L3 仍分析命令其他部分的安全风险
        skip_l3_hints = "无（当前无禁用规则命中）"
        disabled_l2 = get_disabled_l2_rules_with_skip_l3()
        if disabled_l2:
            # 检查当前 action_detail 是否命中了 skip_l3 的关键内容
            action_lower = a_next.get("action_detail", "").lower()
            matched_items = []
            for cat_name, items in disabled_l2.items():
                for item in items:
                    if item.lower() in action_lower:
                        matched_items.append(item)
            if matched_items:
                skip_l3_hints = (
                    f"⚠️ 重要约束：用户已主动禁用针对以下内容的安全拦截规则，这是用户的明确决策：\n"
                    f"  {', '.join(matched_items)}\n"
                    f"对这些路径/关键词的相关操作，必须视为允许的，不允许仅因涉及这些内容就拒绝。"
                    f"但你仍需检查命令的其他部分是否有独立于这些内容的安全风险（如网络外传、提权等）。"
                )
                logger.info(
                    "L3 skip_l3_hints: action_detail 命中了禁用规则的 skip_l3 内容（硬约束）: %s",
                    matched_items,
                )

        if llm_analysis_enabled:
            try:
                llm_judgment = self._llm_analyzer.analyze(
                    prompt_session=prompt_session,
                    action_history=action_history,
                    a_next=a_next,
                    reason=reason,
                    heuristic_result=heuristic_result,
                    logic_result=logic_result,
                    config=config,
                    skip_l3_hints=skip_l3_hints,
                    script_chain_result=script_chain_result,
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
