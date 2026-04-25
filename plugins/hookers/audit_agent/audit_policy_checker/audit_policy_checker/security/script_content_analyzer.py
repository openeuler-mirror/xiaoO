"""脚本内容分析器 — 预扫描脚本内容检测可疑关键词

当检测到脚本执行动作时，预扫描脚本内容中的可疑关键词。
如果命中可疑关键词，将脚本内容注入 LLM prompt 进行深度分析。

方案3: 关键词预筛选
- 限制读取 500 行，避免 prompt 爆炸
- 只在检测到可疑关键词时才注入脚本内容
- 不影响前两层（启发式、逻辑规则）的处理逻辑
"""

import logging
import re
from dataclasses import dataclass, field
from pathlib import Path

logger = logging.getLogger(__name__)

# 高风险检测规则 — (名称, 正则模式)
# 仅保留误报率极低、几乎必然指示恶意行为的模式
SUSPICIOUS_PATTERNS: list[tuple[str, re.Pattern]] = [
    # 网络外传（curl/wget POST 在脚本中几乎无合法用途）
    ("curl POST", re.compile(r"curl\s+(.*\s)?-X\s+POST", re.IGNORECASE)),
    ("curl --data", re.compile(r"curl\s+(.*\s)?--data", re.IGNORECASE)),
    ("curl -d", re.compile(r"curl\s+(.*\s)?-d\s", re.IGNORECASE)),
    ("wget --post-data", re.compile(r"wget\s+(.*\s)?--post-data", re.IGNORECASE)),
    ("wget --post-file", re.compile(r"wget\s+(.*\s)?--post-file", re.IGNORECASE)),
    # 反弹 shell
    ("nc -e", re.compile(r"\bnc\s+.*-e\b")),
    ("ncat -e", re.compile(r"\bncat\s+.*-e\b")),
    ("/dev/tcp", re.compile(r"/dev/tcp/")),
    ("/dev/udp", re.compile(r"/dev/udp/")),
    # base64 编码执行（混淆逃逸的强信号）
    ("$(base64", re.compile(r"\$\(\s*base64")),
    ("`base64", re.compile(r"`\s*base64")),
    ("base64 decode pipe", re.compile(r"base64\s+(-d|--decode)\s*\|")),
    # 敏感文件读取（正常脚本不会读 shadow/ssh 密钥）
    ("/etc/shadow", re.compile(r"/etc/shadow")),
    ("id_rsa", re.compile(r"\bid_rsa\b")),
    ("id_ed25519", re.compile(r"\bid_ed25519\b")),
    # 持久化后门
    ("crontab install", re.compile(r"crontab\s+-")),
    ("authorized_keys", re.compile(r"authorized_keys")),
    ("ssh no host key check", re.compile(r"ssh\s+.*StrictHostKeyChecking\s*=\s*no")),
]

# 脚本执行模式匹配
SCRIPT_EXEC_PATTERNS = [
    r"bash\s+(\S+\.sh)",  # bash script.sh
    r"sh\s+(\S+\.sh)",  # sh script.sh
    r"\./(\S+\.sh)",  # ./script.sh
    r"source\s+(\S+\.sh)",  # source script.sh
    r"\.\s+(\S+\.sh)",  # . script.sh
    r"zsh\s+(\S+\.sh)",  # zsh script.sh
]


@dataclass
class ScriptAnalysisResult:
    """脚本内容分析结果"""

    script_path: str = ""  # 脚本路径
    script_content: str = ""  # 脚本内容（最多 500 行）
    matched_keywords: list[str] = field(default_factory=list)  # 命中的可疑关键词
    line_count: int = 0  # 脚本行数
    is_suspicious: bool = False  # 是否可疑
    error: str = ""  # 读取错误信息


def extract_script_path(command: str) -> str | None:
    """
    从 bash 命令中提取脚本路径。

    Args:
        command: bash 命令字符串

    Returns:
        str | None: 脚本路径，如果未匹配到则返回 None
    """
    for pattern in SCRIPT_EXEC_PATTERNS:
        match = re.search(pattern, command)
        if match:
            return match.group(1)
    return None


def scan_script_for_keywords(script_content: str) -> list[str]:
    """
    扫描脚本内容中的高风险模式。

    Args:
        script_content: 脚本内容

    Returns:
        list[str]: 命中的高风险模式名称列表
    """
    matched = []
    for name, pattern in SUSPICIOUS_PATTERNS:
        if pattern.search(script_content):
            matched.append(name)
    return matched


def read_script_content(script_path: str, max_lines: int = 500) -> tuple[str, int, str]:
    """
    读取脚本内容，限制行数。

    Args:
        script_path: 脚本路径
        max_lines: 最大读取行数

    Returns:
        tuple[str, int, str]: (脚本内容, 实际行数, 错误信息)
    """
    try:
        path = Path(script_path).expanduser()
        if not path.exists():
            return "", 0, f"脚本不存在: {script_path}"
        if not path.is_file():
            return "", 0, f"不是文件: {script_path}"

        lines = []
        with open(path, "r", encoding="utf-8", errors="ignore") as f:
            for i, line in enumerate(f):
                if i >= max_lines:
                    break
                lines.append(line.rstrip("\n"))

        content = "\n".join(lines)
        return content, len(lines), ""

    except Exception as e:
        return "", 0, f"读取脚本失败: {e}"


def analyze_script_content(
    action_type: str,
    action_detail: str,
    max_lines: int = 500,
) -> ScriptAnalysisResult:
    """
    分析脚本内容，检测可疑关键词。

    只在 action_type 为 bash 且命令中包含脚本执行时才进行分析。

    Args:
        action_type: 动作类型
        action_detail: 动作详情（命令）
        max_lines: 最大读取行数

    Returns:
        ScriptAnalysisResult: 分析结果
    """
    # 只处理 bash 类型的动作
    if action_type.lower() != "bash":
        return ScriptAnalysisResult()

    # 提取脚本路径
    script_path = extract_script_path(action_detail)
    if not script_path:
        return ScriptAnalysisResult()

    logger.debug("检测到脚本执行: %s", script_path)

    # 读取脚本内容
    content, line_count, error = read_script_content(script_path, max_lines)
    if error:
        logger.warning("读取脚本内容失败: %s", error)
        return ScriptAnalysisResult(script_path=script_path, error=error)

    # 扫描可疑关键词
    matched_keywords = scan_script_for_keywords(content)
    is_suspicious = len(matched_keywords) > 0

    if is_suspicious:
        logger.info(
            "脚本内容检测到可疑关键词: path=%s, keywords=%s",
            script_path,
            matched_keywords,
        )

    return ScriptAnalysisResult(
        script_path=script_path,
        script_content=content,
        matched_keywords=matched_keywords,
        line_count=line_count,
        is_suspicious=is_suspicious,
    )


def format_script_analysis_for_prompt(result: ScriptAnalysisResult) -> str:
    """
    将脚本分析结果格式化为 LLM prompt 可用的文本。

    Args:
        result: 脚本分析结果

    Returns:
        str: 格式化后的文本
    """
    if not result.script_path:
        return ""

    if result.error:
        return f"⚠️ 脚本内容分析失败: {result.error}"

    if not result.is_suspicious:
        return ""

    keywords_str = ", ".join(result.matched_keywords)
    return (
        f"⚠️ 检测到脚本执行，且脚本内容包含可疑关键词:\n"
        f"   脚本路径: {result.script_path}\n"
        f"   可疑关键词: {keywords_str}\n"
        f"   脚本行数: {result.line_count}\n\n"
        f"### 脚本内容 ###\n```\n{result.script_content}\n```\n"
    )
