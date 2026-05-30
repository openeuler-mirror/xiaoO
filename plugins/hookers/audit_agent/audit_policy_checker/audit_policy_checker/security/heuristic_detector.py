"""启发式静态检测 — 快速拦截已知危险模式

采用组合式检测器架构，包含三个独立子检测器：
1. UserRuleMatcher — 用户敏感动作规则匹配（精确 + 通配符）
2. CommandPatternScanner — 关键命令正则检测
3. InjectionKeywordChecker — Prompt 注入检测（工具结果注入关键词匹配）

三者由 HeuristicDetector 统一协调，按优先级依次执行，支持短路返回。
"""

import json
import re
from pathlib import Path

from .types import HeuristicResult

# 默认用户规则文件路径
DEFAULT_RULES_PATH = Path(__file__).parent.parent / "rules" / "user_rules.json"

# ==================== 关键命令正则模式 ====================
# 包含：递归删除根目录、全权限设置、提权执行、访问敏感文件等
CRITICAL_COMMAND_PATTERNS: list[dict] = [
    # rm 命令分级检测（规则从严格到宽松，先匹配的优先）
    # 1. rm -rf 根目录 — critical（最高危）
    {
        "pattern": r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*f[a-zA-Z]*\s+/\s",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到递归强制删除根目录 (rm -rf /)",
    },
    # 2. rm 同时带 -r 和 -f 且目标是绝对路径 — critical
    {
        "pattern": r"\brm\s+-(?=[^\s]*r)(?=[^\s]*f)[a-zA-Z]*\s+/",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到递归强制删除绝对路径 (rm -rf /...)",
    },
    # 3. rm 带 -r 或 -f 且目标是关键系统目录 — critical
    {
        "pattern": r"\brm\s+-[a-zA-Z]*[rf][a-zA-Z]*\s+/(etc|var|home|usr|boot|root|opt|srv|sys|proc)\b",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到删除关键系统目录",
    },
    # 4. rm -r 后跟绝对路径（无 -f）— high
    {
        "pattern": r"\brm\s+-[a-zA-Z]*r[a-zA-Z]*\s+/",
        "risk_level": "high",
        "risk_type": "script_execution",
        "reason": "检测到递归删除操作 (rm -r /...)",
    },
    # 5. rm -f 后跟绝对路径（无 -r）— medium
    {
        "pattern": r"\brm\s+-[a-zA-Z]*f[a-zA-Z]*\s+/",
        "risk_level": "medium",
        "risk_type": "script_execution",
        "reason": "检测到强制删除文件 (rm -f /...)",
    },
    {
        "pattern": r"chmod\s+[0-7]*777[0-7]*",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到全权限设置 (chmod 777)",
    },
    {
        "pattern": r"chown\s+root",
        "risk_level": "high",
        "risk_type": "script_execution",
        "reason": "检测到修改文件所有者为 root",
    },
    {
        "pattern": r"\bsudo\s+",
        "risk_level": "high",
        "risk_type": "script_execution",
        "reason": "检测到提权执行 (sudo)",
    },
    {
        "pattern": r"/etc/shadow",
        "risk_level": "critical",
        "risk_type": "file_access",
        "reason": "检测到访问系统密码文件 (/etc/shadow)",
    },
    {
        "pattern": r"/etc/passwd",
        "risk_level": "high",
        "risk_type": "file_access",
        "reason": "检测到访问系统用户文件 (/etc/passwd)",
    },
    {
        "pattern": r"\.ssh/id_rsa",
        "risk_level": "critical",
        "risk_type": "file_access",
        "reason": "检测到访问 SSH 私钥文件 (.ssh/id_rsa)",
    },
    # xiaoO 特定检测：尝试修改或读取 xiaoO 配置
    {
        "pattern": r"xiaoo\.env|xiaoo\.toml|llm_secrets\.json",
        "risk_level": "high",
        "risk_type": "config_tampering",
        "reason": "检测到尝试访问或修改 xiaoO 配置文件",
    },
    # xiaoO 安全 Skill 目录保护：检测删除/修改操作（允许读取/执行）
    # 只拦截写入类命令，放行 cat/read/ls/grep 等读取操作
    {
        "pattern": r"\.xiaoo/skills/xiaoo-guardian/.*\b(rm|del|delete|remove|修改|删除|chmod|chown|write|save|保存|modify|更新|覆盖|replace|truncate|追加|append)\b",
        "risk_level": "critical",
        "risk_type": "config_tampering",
        "reason": "检测到尝试修改或删除 xiaoO 安全防护 Skill 目录",
    },
    {
        "pattern": r"\b(rm|del|delete|remove|修改|删除|chmod|chown|write|save|保存|modify|更新|覆盖|replace|truncate|追加|append)\b.*\.xiaoo/skills/xiaoo-guardian/",
        "risk_level": "critical",
        "risk_type": "config_tampering",
        "reason": "检测到尝试修改或删除 xiaoO 安全防护 Skill 目录",
    },
]

# ==================== 额外危险命令模式 ====================
# 额外增加的危险操作检测
EXTRA_DANGEROUS_PATTERNS: list[dict] = [
    {
        "pattern": r"\bdd\s+if=/dev/zero",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到磁盘清零操作 (dd if=/dev/zero)",
    },
    {
        "pattern": r"\bmkfs\b",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到格式化文件系统操作 (mkfs)",
    },
    {
        "pattern": r">\s*/etc/",
        "risk_level": "critical",
        "risk_type": "file_access",
        "reason": "检测到覆盖写入系统配置目录",
    },
    {
        "pattern": r"\bformat\b.*[A-Z]:",
        "risk_level": "critical",
        "risk_type": "script_execution",
        "reason": "检测到磁盘格式化操作",
    },
    {
        "pattern": r"/dev/(zero|random|urandom|full|core|loop|sd|hd|nvme|vd|mem|kmsg|port)",
        "risk_level": "high",
        "risk_type": "file_access",
        "reason": "检测到访问设备文件 (/dev/*)",
    },
    # Docker socket 权限提升
    {
        "pattern": r"/var/run/docker\.sock",
        "risk_level": "critical",
        "risk_type": "privilege_escalation",
        "reason": "检测到访问 Docker socket，可能导致容器逃逸",
    },
    # git 强制推送
    {
        "pattern": r"git\s+push\s+.*--force",
        "risk_level": "high",
        "risk_type": "script_execution",
        "reason": "检测到强制推送操作 (git push --force)，可能覆盖远程提交历史",
    },
    # curl POST 外传敏感数据
    {
        "pattern": r"curl\s+.*-X\s+POST.*\b(env|environment|secret|key|token|password)\b",
        "risk_level": "high",
        "risk_type": "data_exfiltration",
        "reason": "检测到通过 curl POST 外传可能包含敏感信息的请求",
    },
    # 注意：/dev/null 已从检测列表移除，因为它通常只用于 shell 重定向（如 2>/dev/null），
    # 不是真正的安全风险。真正的攻击不会通过读取 /dev/null 来实现。
]

# ==================== Prompt 注入检测关键词 ====================
INJECTION_KEYWORDS: list[dict] = [
    # ---- 指令覆盖 / Instruction Override ----
    {"keyword": "ignore previous instructions", "risk_level": "critical", "lang": "en"},
    {"keyword": "ignore all previous", "risk_level": "critical", "lang": "en"},
    {"keyword": "disregard all prior", "risk_level": "critical", "lang": "en"},
    {"keyword": "forget everything", "risk_level": "high", "lang": "en"},
    {"keyword": "忽略之前的指令", "risk_level": "critical", "lang": "zh"},
    {"keyword": "忽略所有先前的", "risk_level": "critical", "lang": "zh"},
    {"keyword": "忽略上面所有", "risk_level": "high", "lang": "zh"},
    {"keyword": "请忘记之前", "risk_level": "high", "lang": "zh"},
    # ---- 角色劫持 / Role Hijacking ----
    {"keyword": "you are now", "risk_level": "high", "lang": "en"},
    {"keyword": "you are no longer", "risk_level": "high", "lang": "en"},
    {"keyword": "new instructions", "risk_level": "high", "lang": "en"},
    {"keyword": "from now on, you", "risk_level": "high", "lang": "en"},
    {"keyword": "你现在是一个", "risk_level": "high", "lang": "zh"},
    {"keyword": "你不再是一个", "risk_level": "high", "lang": "zh"},
    {"keyword": "从现在开始你", "risk_level": "high", "lang": "zh"},
    {"keyword": "你的新角色是", "risk_level": "high", "lang": "zh"},
    # ---- 命令注入 / Command Injection ----
    {"keyword": "system prompt", "risk_level": "high", "lang": "en"},
    {"keyword": "reveal your instructions", "risk_level": "high", "lang": "en"},
    {"keyword": "output your instructions", "risk_level": "high", "lang": "en"},
    {"keyword": "repeat your instructions", "risk_level": "medium", "lang": "en"},
    {"keyword": "系统提示", "risk_level": "high", "lang": "zh"},
    {"keyword": "输出你的指令", "risk_level": "high", "lang": "zh"},
    {"keyword": "重复你的指令", "risk_level": "medium", "lang": "zh"},
    # ---- 社会工程 / Social Engineering ----
    {"keyword": "this is an authorized test", "risk_level": "high", "lang": "en"},
    {"keyword": "security audit approved", "risk_level": "high", "lang": "en"},
    {"keyword": "admin override", "risk_level": "critical", "lang": "en"},
    {"keyword": "debug mode", "risk_level": "high", "lang": "en"},
    {"keyword": "maintenance mode", "risk_level": "high", "lang": "en"},
    {"keyword": "这是授权的测试", "risk_level": "high", "lang": "zh"},
    {"keyword": "安全审计已批准", "risk_level": "high", "lang": "zh"},
    {"keyword": "管理员覆盖", "risk_level": "critical", "lang": "zh"},
    {"keyword": "调试模式", "risk_level": "high", "lang": "zh"},
    # ---- XSS / 注入 ----
    {"keyword": "<script>", "risk_level": "high", "lang": "en"},
    {"keyword": "javascript:", "risk_level": "high", "lang": "en"},
    {"keyword": "onerror=", "risk_level": "high", "lang": "en"},
    {"keyword": "onload=", "risk_level": "high", "lang": "en"},
    # ---- 伪造边界 / Fake Boundaries ----
    {"keyword": "=== END OF", "risk_level": "high", "lang": "en"},
    {"keyword": "--- END SYSTEM", "risk_level": "high", "lang": "en"},
    {"keyword": "[END OF", "risk_level": "high", "lang": "en"},
    {"keyword": "=== 结束", "risk_level": "high", "lang": "zh"},
    {"keyword": "--- 结束系统", "risk_level": "high", "lang": "zh"},
    # ---- 开发者模式 ----
    {"keyword": "developer mode", "risk_level": "critical", "lang": "en"},
    {"keyword": "DAN mode", "risk_level": "critical", "lang": "en"},
    {"keyword": "jailbreak", "risk_level": "critical", "lang": "en"},
    {"keyword": "开发者模式", "risk_level": "critical", "lang": "zh"},
    {"keyword": "越狱", "risk_level": "critical", "lang": "zh"},
    # ---- xiaoO 特定绕过尝试 ----
    {"keyword": "bypass audit", "risk_level": "critical", "lang": "en"},
    {"keyword": "skip security check", "risk_level": "critical", "lang": "en"},
    {"keyword": "disable guard", "risk_level": "critical", "lang": "en"},
    {"keyword": "绕过审计", "risk_level": "critical", "lang": "zh"},
    {"keyword": "禁用安全检查", "risk_level": "critical", "lang": "zh"},
    {"keyword": "关闭防护", "risk_level": "critical", "lang": "zh"},
]

# ==================== 安全工具白名单 ====================
SAFE_ACTION_TYPES = frozenset({
    "ask_user_question", "glob", "grep", "list_dir", "count_text_length",
})

# ==================== 安全 Bash 子命令模式 ====================

# 完全安全的 bash 子命令：跳过 Layer 2 + Layer 3
# 这些命令不涉及文件内容读取，无敏感路径泄露风险
FULLY_SAFE_BASH_PATTERNS = [
    # 目录查看
    r"^ls(?:\s|$)",
    r"^dir(?:\s|$)",
    # 路径查看
    r"^pwd(?:\s|$)",
    r"^which\s+",
    r"^whereis\s+",
    r"^realpath\s+",
    # 文件信息（不读内容）
    r"^file\s+",
    r"^stat\s+",
    r"^du(?:\s|$)",
    # 打印（不涉及文件）
    r"^echo\s+",
    r"^printf\s+",
    # 类型检查
    r"^type\s+",
    r"^command\s+",
    r"^basename\s+",
    r"^dirname\s+",
]

# 安全但可能访问敏感路径的 bash 子命令：跳过 Layer 3，保留 Layer 2
# 这些命令可以读取文件内容，可能泄露 /etc/shadow 等敏感信息
READONLY_SENSITIVE_BASH_PATTERNS = [
    # 文件内容查看
    r"^cat\s+\S",
    r"^head(?:\s|$)",
    r"^tail(?:\s|$)",
    r"^less(?:\s|$)",
    r"^more(?:\s|$)",
    r"^wc(?:\s|$)",
    # 搜索
    r"^grep(?:\s|$)",
    r"^find\s+",
    r"^ag(?:\s|$)",
    r"^rg(?:\s|$)",
    # 内容提取
    r"^awk\s+",
    r"^sed\s+",
    r"^cut\s+",
    r"^sort(?:\s|$)",
    r"^uniq(?:\s|$)",
]

# 向后兼容：合并为完整安全列表（原 SAFE_BASH_PATTERNS）
SAFE_BASH_PATTERNS = FULLY_SAFE_BASH_PATTERNS + READONLY_SENSITIVE_BASH_PATTERNS

# 编译正则
FULLY_SAFE_BASH_COMPILED = [re.compile(p, re.IGNORECASE) for p in FULLY_SAFE_BASH_PATTERNS]
READONLY_SENSITIVE_BASH_COMPILED = [re.compile(p, re.IGNORECASE) for p in READONLY_SENSITIVE_BASH_PATTERNS]
SAFE_BASH_COMPILED = FULLY_SAFE_BASH_COMPILED + READONLY_SENSITIVE_BASH_COMPILED

# ==================== 风险等级优先级映射 ====================
_RISK_PRIORITY = {"low": 0, "medium": 1, "high": 2, "critical": 3}


def _max_risk(*levels: str) -> str:
    """返回多个风险等级中最高的那个"""
    return max(levels, key=lambda l: _RISK_PRIORITY.get(l, 0))


class UserRuleMatcher:
    """子检测器：用户自定义敏感动作/工具规则匹配"""

    def __init__(self, rules_path: str | Path | None = None):
        self._rules = self._load(rules_path)

    @staticmethod
    def _load(rules_path: str | Path | None) -> dict:
        path = Path(rules_path) if rules_path else DEFAULT_RULES_PATH
        if not path.exists():
            return {"sensitive_actions": [], "sensitive_tools": []}
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)

    def check(self, action_type: str, action_detail: str) -> HeuristicResult:
        # 通配符匹配敏感动作
        for rule in self._rules.get("sensitive_actions", []):
            pattern = rule.get("pattern", "").lower()
            description = rule.get("description", "")
            if not pattern:
                continue
            regex = re.escape(pattern).replace(r"\*", ".*")
            if re.search(regex, action_detail) or re.search(regex, action_type):
                return HeuristicResult(
                    hit=True,
                    matched_patterns=[pattern],
                    risk_level="high",
                    reason=f"命中用户敏感动作规则: {description or pattern}",
                    risk_type="sensitive_action",
                )

        # 精确匹配敏感工具名
        for tool_name in self._rules.get("sensitive_tools", []):
            if tool_name.lower() == action_type:
                return HeuristicResult(
                    hit=True,
                    matched_patterns=[tool_name],
                    risk_level="high",
                    reason=f"命中敏感工具规则: {tool_name}",
                    risk_type="sensitive_tool",
                )

        return HeuristicResult(hit=False)


class CommandPatternScanner:
    """子检测器：关键命令正则扫描"""

    def __init__(self):
        all_patterns = CRITICAL_COMMAND_PATTERNS + EXTRA_DANGEROUS_PATTERNS
        self._compiled = [
            {**p, "regex": re.compile(p["pattern"], re.IGNORECASE)}
            for p in all_patterns
        ]

    def scan(self, action_detail: str) -> HeuristicResult:
        hits = [
            p for p in self._compiled
            if p["regex"].search(action_detail)
        ]
        if not hits:
            return HeuristicResult(hit=False)

        top = max(hits, key=lambda p: _RISK_PRIORITY.get(p["risk_level"], 0))
        return HeuristicResult(
            hit=True,
            matched_patterns=[p["pattern"] for p in hits],
            risk_level=top["risk_level"],
            reason="; ".join(p["reason"] for p in hits),
            risk_type=top["risk_type"],
        )


class InjectionKeywordChecker:
    """子检测器：Prompt 注入关键词检测"""

    def check(self, text: str) -> HeuristicResult:
        hits = [p for p in INJECTION_KEYWORDS if p["keyword"].lower() in text]
        if not hits:
            return HeuristicResult(hit=False)

        top = max(hits, key=lambda p: _RISK_PRIORITY.get(p["risk_level"], 0))
        return HeuristicResult(
            hit=True,
            matched_patterns=[p["keyword"] for p in hits],
            risk_level=top["risk_level"],
            reason="; ".join(
                f"检测到注入关键词 [{p.get('lang', 'unknown')}]: {p['keyword']}"
                for p in hits
            ),
            risk_type="prompt_injection",
        )


def _extract_first_command(command: str) -> str:
    """提取管道和链式命令的第一段"""
    main_cmd = command.split("|")[0].strip()
    for sep in ["&&", "||"]:
        if sep in main_cmd:
            main_cmd = main_cmd.split(sep)[0].strip()
            break
    return main_cmd


def is_fully_safe_bash_command(command: str) -> bool:
    """检查是否为完全安全的 bash 命令（跳过 L2 + L3）"""
    main_cmd = _extract_first_command(command)
    return any(p.match(main_cmd) for p in FULLY_SAFE_BASH_COMPILED)


def is_readonly_sensitive_bash_command(command: str) -> bool:
    """检查是否为安全但可能访问敏感路径的 bash 命令（跳过 L3，保留 L2）"""
    main_cmd = _extract_first_command(command)
    return any(p.match(main_cmd) for p in READONLY_SENSITIVE_BASH_COMPILED)


def is_safe_bash_command(command: str) -> bool:
    """
    检查 bash 命令是否是安全的只读操作（向后兼容）。

    Args:
        command: bash 命令字符串

    Returns:
        bool: 如果是安全的只读命令返回 True
    """
    main_cmd = _extract_first_command(command)

    # 检查是否匹配安全模式
    for pattern in SAFE_BASH_COMPILED:
        if pattern.match(main_cmd):
            return True

    return False


class HeuristicDetector:
    """启发式静态检测器 — 组合式协调器

    按优先级依次调用子检测器，支持短路返回：
    1. UserRuleMatcher — 用户敏感规则
    2. CommandPatternScanner — 关键命令正则
    3. InjectionKeywordChecker — Prompt 注入关键词
    """

    def __init__(self, rules_path: str | Path | None = None):
        self._rule_matcher = UserRuleMatcher(rules_path)
        self._command_scanner = CommandPatternScanner()
        self._injection_checker = InjectionKeywordChecker()

    def detect(
        self,
        a_next: dict[str, str],
        reason: str,
    ) -> HeuristicResult:
        """
        执行启发式检测。

        Args:
            a_next: 下一步动作，含 action_type 和 action_detail
            reason: 执行该动作的理由

        Returns:
            HeuristicResult: 检测结果
        """
        action_type = a_next.get("action_type", "").lower()
        action_detail = a_next.get("action_detail", "").lower()
        combined_text = f"{action_type} {action_detail} {reason}".lower()

        # 安全工具白名单跳过
        if any(t in action_type or t in action_detail for t in SAFE_ACTION_TYPES):
            return HeuristicResult(hit=False)

        # 1. 用户敏感规则
        result = self._rule_matcher.check(action_type, action_detail)
        if result.hit:
            return result

        # 2. 关键命令正则
        result = self._command_scanner.scan(action_detail)
        if result.hit:
            return result

        # 3. Prompt 注入
        result = self._injection_checker.check(combined_text)
        if result.hit:
            return result

        return HeuristicResult(hit=False)
