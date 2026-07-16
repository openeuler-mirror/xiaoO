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
    source: str = ""  # 判断来源："heuristic" | "logic_rule" | "llm" | "heuristic_script_scan" | "logic_rule_script_scan"
    action_desc: str = ""  # 动作描述
    violated_layers: list[str] = field(default_factory=list)  # 违反的具体层号，如 ["1.1"], ["1.2", "1.3"]


@dataclass
class HeuristicResult:
    """启发式检测结果"""

    hit: bool = False
    matched_patterns: list[str] = field(default_factory=list)
    risk_level: str = "low"
    reason: str = ""
    risk_type: str = ""
    confidence: int = 100  # 0-100，置信度；<80 时不立即 Deny，转交 Layer 2/3 分析


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


# ==================== 脚本内容静态扫描相关类型 ====================


@dataclass
class ScriptNode:
    """递归追踪链中的单个脚本节点"""

    script_path: str = ""                          # 脚本绝对路径（resolve 后）
    raw_content: str = ""                          # 原始脚本内容（完整读取）
    preprocessed_content: str = ""                 # 预处理后内容（去注释+去echo+变量展开）
    line_count: int = 0                            # 原始行数
    file_size: int = 0                             # 文件大小（bytes）
    depth: int = 0                                 # 递归深度（0=入口脚本）
    child_paths: list[str] = field(default_factory=list)  # 提取到的子脚本路径
    error: str = ""                                # 读取错误信息

    # L1 扫描结果（CommandPatternScanner 对 preprocessed_content 的扫描结果）
    l1_hit: bool = False
    l1_risk_level: str = ""
    l1_risk_type: str = ""
    l1_reason: str = ""
    l1_matched_patterns: list[str] = field(default_factory=list)

    # L2 扫描结果（LogicRulesChecker 对 preprocessed_content 的扫描结果）
    l2_hit: bool = False
    l2_violated_rule: str = ""
    l2_risk_level: str = ""
    l2_risk_type: str = ""
    l2_reason: str = ""

    # 关键词扫描结果（兼容现有 L3 SUSPICIOUS_PATTERNS 流程）
    matched_keywords: list[str] = field(default_factory=list)
    is_suspicious: bool = False
    keyword_risk_level: str = ""
    keyword_risk_reason: str = ""


@dataclass
class ScriptChainAnalysisResult:
    """脚本调用链完整分析结果"""

    nodes: list[ScriptNode] = field(default_factory=list)
    entry_path: str = ""                           # 入口脚本路径
    max_depth_reached: bool = False                # 是否触达递归深度上限
    max_size_reached: bool = False                 # 是否触达总大小上限
    total_size: int = 0                            # 所有文件累计大小（bytes）
    total_lines: int = 0                           # 所有文件累计行数
    has_critical: bool = False                     # 调用链中是否有 critical 级别风险
    has_high: bool = False                         # 调用链中是否有 high 级别风险
    first_hit_node: ScriptNode | None = None       # 第一个命中的节点（用于快速 Deny）
    error: str = ""                                # 全局错误信息
