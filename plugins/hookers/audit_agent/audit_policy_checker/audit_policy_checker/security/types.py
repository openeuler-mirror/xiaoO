"""安全检测相关类型定义"""

from dataclasses import dataclass, field


@dataclass
class SecurityJudgment:
    """xiaoO Audit Agent 安全判断结果"""

    allowed: bool
    reason: str
    risk_level: str = "low"  # "low" | "medium" | "high" | "critical"
    risk_type: str = ""  # 风险类别（如 "file_access", "script_execution" 等）
    confidence: int = 100  # 0-100
    source: str = ""  # 判断来源："heuristic" | "logic_rule" | "llm"
    action_desc: str = ""  # 动作描述
    violated_layers: list[str] = field(default_factory=list)  # 违反的具体层号，如 ["1.1"], ["1.2", "1.3"]


@dataclass
class HeuristicResult:
    """启发式检测 результат"""

    hit: bool = False
    matched_patterns: list[str] = field(default_factory=list)
    risk_level: str = "low"
    reason: str = ""
    risk_type: str = ""


@dataclass
class LogicRuleResult:
    """逻辑规则检测结果"""

    hit: bool = False
    violated_rule: str = ""
    risk_level: str = "low"
    reason: str = ""
    risk_type: str = ""


@dataclass
class SkillMatch:
    """匹配到的 Skill"""

    skill_name: str = ""
    relevance: float = 0.0  # 0-1 相关度
    skill_content: str = ""  # Skill 文件内容
