"""脚本内容分析器 — 递归追踪 + 预处理 + L1/L2 静态扫描

核心改造（相比旧版）：
1. 预处理流水线：去注释 → 去 echo 文本 → 简单变量展开，消除常见误报
2. 扩展脚本路径提取：去掉 .sh 后缀限制，增加 Python/Node/Ruby 等解释器
3. 去掉行数限制改用文件大小限制：完整读取文件内容再扫描（实测开销毫秒级）
4. 递归追踪：A.sh → B.sh → C.sh 的嵌套调用也能拦截
5. L1/L2 复用扫描：脚本内容经过预处理后，复用现有 CommandPatternScanner
   和 LogicRulesChecker 做静态扫描，high/critical 直接拦截，不走 L3

性能预算（实测数据）：
  - 10000 行 (~600KB) 读取+扫描: 43ms
  - 50000 行 (~2MB)  读取+扫描: 123ms
  - 3 层递归 (3 个普通脚本): 预计 ~5-10ms
  - vs L3 LLM 调用: 2-10s + 数千 token
"""

import logging
import re
from pathlib import Path

from .types import ScriptNode, ScriptChainAnalysisResult

logger = logging.getLogger(__name__)

# ==================== 预处理流水线 ====================


def _strip_comments(content: str) -> str:
    """去除 # 开头的注释行。

    Shell 脚本只有 # 单行注释，没有 /* */ 多行注释。
    注释中的危险关键词（如 # rm -rf /、# /etc/shadow）不应触发拦截。
    """
    return '\n'.join(
        line for line in content.split('\n')
        if not line.lstrip().startswith('#')
    )


def _strip_output_text(content: str) -> str:
    """将 echo/printf 的文本内容清空，避免字符串中的危险关键词误报。

    echo "Warning: do not run rm -rf /" 中的 rm -rf / 是文本描述，不是执行意图。
    替换为 echo "" 后，正则扫描不会误报。

    支持多行字符串：echo "line1
    line2 with rm -rf /" 也会被替换为 echo ""。
    """
    # echo "xxx" → echo ""（支持多行字符串：[^"]* 换为 .*? 配合 re.DOTALL）
    result = re.sub(r'echo\s+".*?"', 'echo ""', content, flags=re.DOTALL)
    result = re.sub(r"echo\s+'.*?'", "echo ''", result, flags=re.DOTALL)
    # printf "xxx" ... → printf ""（printf 通常不跨行，但仍用 DOTALL 保护）
    result = re.sub(r'printf\s+".*?"[^\n]*', 'printf ""', result, flags=re.DOTALL)
    result = re.sub(r"printf\s+'.*?'[^\n]*", "printf ''", result, flags=re.DOTALL)
    return result


def _expand_simple_variables(content: str) -> str:
    """提取 VAR=value 形式的简单赋值，对 $VAR / ${VAR} 做文本替换。

    只展开不含 $ 的简单值（避免多步赋值和命令替换的复杂场景）。
    多轮展开（最多 3 轮）处理 VAR_A=$VAR_B; VAR_B=/etc 这种两步赋值。

    能处理的示例：
      TARGET=/etc/nginx → rm -rf $TARGET → rm -rf /etc/nginx (命中 L1 规则!)
    不能处理的示例（交给 L3）：
      TARGET=$HOME/config → $HOME 无法静态展开
      TARGET=$(find /tmp -name "*.sh") → 命令替换无法静态计算
    """
    var_map: dict[str, str] = {}
    _assignment_re = re.compile(
        r'^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.+)$'
    )

    for line in content.split('\n'):
        m = _assignment_re.match(line)
        if m:
            var_name = m.group(1)
            var_val = m.group(2).strip()
            # 只展开不含 $ 和 ` 的简单值
            if var_val and '$' not in var_val and '`' not in var_val:
                # 去掉引号包裹
                if (var_val.startswith('"') and var_val.endswith('"')) or \
                   (var_val.startswith("'") and var_val.endswith("'")):
                    var_val = var_val[1:-1]
                var_map[var_name] = var_val

    expanded = content
    # 多轮展开（最多 3 轮）
    # 替换时按变量名长度从长到短排序，避免 $TARGET 把 $TARGET_DIR 的前缀也替换掉
    # 例如：先替换 $TARGET_DIR → 再替换 $TARGET，这样不会破坏长变量名
    sorted_vars = sorted(var_map.keys(), key=len, reverse=True)
    for _ in range(3):
        new_expanded = expanded
        for var in sorted_vars:
            val = var_map[var]
            new_expanded = new_expanded.replace('${' + var + '}', val)
            new_expanded = new_expanded.replace('$' + var, val)
        if new_expanded == expanded:
            break
        expanded = new_expanded
        # 更新 var_map（可能有新值被展开了），同时更新排序
        for line in expanded.split('\n'):
            m = _assignment_re.match(line)
            if m:
                var_name = m.group(1)
                var_val = m.group(2).strip()
                if var_val and '$' not in var_val and '`' not in var_val:
                    if (var_val.startswith('"') and var_val.endswith('"')) or \
                       (var_val.startswith("'") and var_val.endswith("'")):
                        var_val = var_val[1:-1]
                    var_map[var_name] = var_val
        sorted_vars = sorted(var_map.keys(), key=len, reverse=True)

    return expanded


def preprocess_script_content(content: str) -> str:
    """三步预处理流水线：去注释 → 去输出文本 → 简单变量展开。

    Args:
        content: 脚本原始内容

    Returns:
        str: 预处理后的内容，可直接用于 L1/L2 正则扫描
    """
    result = _strip_comments(content)
    result = _strip_output_text(result)
    result = _expand_simple_variables(result)
    return result


# ==================== 高风险检测规则（关键词扫描，兼容 L3） ====================
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


# ==================== 脚本执行模式匹配 ====================
# 入口命令的脚本路径提取（从 action_detail 提取）

SCRIPT_EXEC_PATTERNS: list[str] = [
    # Shell 系列（不限后缀）
    r"(?:^|\s|&&|\|\|)\s*bash\s+(\S+)",          # bash anything
    r"(?:^|\s|&&|\|\|)\s*/bin/bash\s+(\S+)",     # /bin/bash anything
    r"(?:^|\s|&&|\|\|)\s*sh\s+(\S+)",            # sh anything
    r"(?:^|\s|&&|\|\|)\s*zsh\s+(\S+)",           # zsh anything
    r"\./(\S+)",                                  # ./anything (可执行脚本)
    r"(?:^|\s|&&|\|\|)\s*source\s+(\S+)",        # source anything
    r"(?:^|\s|&&|\|\|)\s*\.\s+(\S+)",            # . anything（source 的简写）
    # Python 系列
    r"(?:^|\s|&&|\|\|)\s*python3?\s+(\S+\.py)",  # python script.py
    # Node/Ruby/Perl/PHP/Lua
    r"(?:^|\s|&&|\|\|)\s*node\s+(\S+\.js)",      # node script.js
    r"(?:^|\s|&&|\|\|)\s*ruby\s+(\S+\.rb)",      # ruby script.rb
    r"(?:^|\s|&&|\|\|)\s*perl\s+(\S+\.pl)",      # perl script.pl
    r"(?:^|\s|&&|\|\|)\s*php\s+(\S+\.php)",      # php script.php
    r"(?:^|\s|&&|\|\|)\s*lua\s+(\S+\.lua)",      # lua script.lua
]

# 从脚本内容中提取子脚本路径（递归追踪用）
CHILD_SCRIPT_PATTERNS: list[str] = [
    # Shell 系列
    r"(?:^|\s|&&|\|\|)\s*bash\s+(\S+)",
    r"(?:^|\s|&&|\|\|)\s*sh\s+(\S+)",
    r"(?:^|\s|&&|\|\|)\s*source\s+(\S+)",
    r"(?:^|\s|&&|\|\|)\s*\.\s+(\S+)",
    # Python/Node/Ruby/Perl/PHP/Lua
    r"(?:^|\s|&&|\|\|)\s*python3?\s+(\S+\.py)",
    r"(?:^|\s|&&|\|\|)\s*node\s+(\S+\.js)",
    r"(?:^|\s|&&|\|\|)\s*ruby\s+(\S+\.rb)",
    r"(?:^|\s|&&|\|\|)\s*perl\s+(\S+\.pl)",
    r"(?:^|\s|&&|\|\|)\s*php\s+(\S+\.php)",
    r"(?:^|\s|&&|\|\|)\s*lua\s+(\S+\.lua)",
]

# 编译正则
_SCRIPT_EXEC_COMPILED = [re.compile(p, re.IGNORECASE) for p in SCRIPT_EXEC_PATTERNS]
_CHILD_SCRIPT_COMPILED = [re.compile(p, re.IGNORECASE) for p in CHILD_SCRIPT_PATTERNS]


# ==================== 关键词组合风险评估规则 ====================
# (关键词集合1, 关键词集合2, 风险等级, 风险原因)
# 集合2 为空时，集合1 单独出现即为对应风险等级

KEYWORD_COMBINATION_RISKS: list[tuple[set[str], set[str], str, str]] = [
    # Critical: 数据外传 - 凭证/密钥
    (
        {"curl POST", "curl --data", "curl -d", "wget --post-data", "wget --post-file"},
        {"$(base64", "`base64", "base64 decode pipe"},
        "critical",
        "检测到网络数据发送 + Base64编码组合，构成 Critical 级别数据外传风险。攻击者可能将敏感数据编码后外传。",
    ),
    (
        {"curl POST", "curl --data", "curl -d", "wget --post-data", "wget --post-file"},
        {"/etc/shadow", "id_rsa", "id_ed25519"},
        "critical",
        "检测到网络数据发送 + 敏感文件访问组合，构成 Critical 级别凭证外传风险。",
    ),
    # High: 反弹 Shell
    (
        {"nc -e", "ncat -e", "/dev/tcp", "/dev/udp"},
        set(),  # 单独出现即为 High
        "high",
        "检测到反弹 Shell 特征，攻击者可能建立远程控制通道。",
    ),
    # High: 持久化后门
    (
        {"crontab install", "authorized_keys", "ssh no host key check"},
        set(),
        "high",
        "检测到持久化后门特征，攻击者可能在系统中植入后门。",
    ),
]

# 单独关键词的默认风险等级
SINGLE_KEYWORD_RISKS: dict[str, tuple[str, str]] = {
    "curl POST": ("high", "检测到 curl POST 请求，可能向外部服务器发送数据。"),
    "curl --data": ("high", "检测到 curl --data 参数，可能向外部服务器发送数据。"),
    "curl -d": ("high", "检测到 curl -d 参数，可能向外部服务器发送数据。"),
    "wget --post-data": ("high", "检测到 wget POST 请求，可能向外部服务器发送数据。"),
    "wget --post-file": ("high", "检测到 wget 文件上传，可能向外部服务器发送文件。"),
    "nc -e": ("critical", "检测到 nc -e 反弹 Shell 特征。"),
    "ncat -e": ("critical", "检测到 ncat -e 反弹 Shell 特征。"),
    "/dev/tcp": ("critical", "检测到 /dev/tcp 反弹 Shell 特征。"),
    "/dev/udp": ("critical", "检测到 /dev/udp 反弹 Shell 特征。"),
    "$(base64": ("high", "检测到 Base64 编码执行，可能是混淆逃逸。"),
    "`base64": ("high", "检测到 Base64 编码执行，可能是混淆逃逸。"),
    "base64 decode pipe": ("high", "检测到 Base64 解码后管道执行，可能是混淆逃逸。"),
    "/etc/shadow": ("critical", "检测到访问系统密码文件。"),
    "id_rsa": ("critical", "检测到访问 SSH 私钥。"),
    "id_ed25519": ("critical", "检测到访问 SSH 私钥。"),
    "crontab install": ("high", "检测到修改定时任务，可能是持久化后门。"),
    "authorized_keys": ("high", "检测到修改 SSH 授权密钥，可能是持久化后门。"),
    "ssh no host key check": ("high", "检测到 SSH 跳过主机密钥验证，可能是横向移动。"),
}

_RISK_PRIORITY = {"low": 0, "medium": 1, "high": 2, "critical": 3}


def scan_script_for_keywords(script_content: str) -> list[str]:
    """扫描脚本内容中的高风险模式。

    Args:
        script_content: 脚本内容（建议传入预处理后的内容）

    Returns:
        list[str]: 命中的高风险模式名称列表
    """
    matched = []
    for name, pattern in SUSPICIOUS_PATTERNS:
        if pattern.search(script_content):
            matched.append(name)
    return matched


def assess_keyword_combination_risk(
    matched_keywords: list[str], script_content: str
) -> tuple[str, str]:
    """根据命中的关键词组合评估风险等级。

    Args:
        matched_keywords: 命中的关键词列表
        script_content: 脚本内容（用于额外检测）

    Returns:
        tuple[str, str]: (风险等级, 风险原因)
    """
    if not matched_keywords:
        return "", ""

    keyword_set = set(matched_keywords)

    # 检查关键词组合规则
    for group1, group2, level, reason in KEYWORD_COMBINATION_RISKS:
        hit_group1 = bool(keyword_set & group1)
        hit_group2 = bool(keyword_set & group2) if group2 else True

        if hit_group1 and hit_group2:
            return level, reason

    # 额外检测：curl POST + env 变量外传
    has_network_post = bool(keyword_set & {
        "curl POST", "curl --data", "curl -d", "wget --post-data", "wget --post-file"
    })
    if has_network_post:
        env_patterns = ["$env", "$(env)", "$USER", "$(hostname)", "$(pwd)", "$HOME"]
        content_lower = script_content.lower()
        for pattern in env_patterns:
            if pattern.lower() in content_lower:
                return "critical", (
                    "检测到网络数据发送 + 系统环境变量收集组合，构成 Critical 级别数据外传风险。"
                    "脚本可能将用户环境变量（可能包含 API Key、密钥等敏感信息）发送到远程服务器。"
                )

    # 单独关键词的默认风险等级：取最高
    best_level = ""
    best_reason = ""

    for kw in matched_keywords:
        if kw in SINGLE_KEYWORD_RISKS:
            level, reason = SINGLE_KEYWORD_RISKS[kw]
            if _RISK_PRIORITY.get(level, 0) > _RISK_PRIORITY.get(best_level, 0):
                best_level = level
                best_reason = reason

    return best_level, best_reason


# ==================== 脚本路径提取 ====================


def extract_script_path(command: str) -> str | None:
    """从 bash 命令中提取脚本路径。

    Args:
        command: bash 命令字符串（action_detail）

    Returns:
        str | None: 脚本路径，如果未匹配到则返回 None
    """
    for compiled in _SCRIPT_EXEC_COMPILED:
        match = compiled.search(command)
        if match:
            return match.group(1)
    return None


def _extract_inline_script_path(action_detail: str) -> str | None:
    """从内联脚本命令中尝试提取引用的文件路径。

    例如: python3 -c "exec(open('evil.py').read())" → 尝试提取 evil.py
    """
    for m in re.finditer(r"""open\s*\(\s*['"]([^'"]+)['"]\s*\)""", action_detail):
        path = m.group(1)
        resolved = Path(path).expanduser().resolve()
        if resolved.exists() and resolved.is_file():
            return str(resolved)
    return None


# ==================== 脚本内容读取 ====================


def read_script_content(
    script_path: str,
    max_file_size: int = 2 * 1024 * 1024,
) -> tuple[str, int, int, str]:
    """读取脚本内容，限制文件大小而非行数。

    实测数据：10000 行 (~600KB) 读取+扫描 43ms，50000 行 (~2MB) 123ms。
    完整读取不会有显著的性能开销，而且避免了"跳过后半部分漏报"的问题。

    Args:
        script_path: 脚本路径
        max_file_size: 单文件大小上限（默认 2MB）

    Returns:
        tuple[str, int, int, str]: (内容, 行数, 文件大小bytes, 错误信息)
    """
    try:
        path = Path(script_path).expanduser().resolve()
        if not path.exists():
            return "", 0, 0, f"脚本不存在: {script_path}"
        if not path.is_file():
            return "", 0, 0, f"不是文件: {script_path}"

        file_size = path.stat().st_size
        if file_size > max_file_size:
            logger.warning(
                "脚本文件过大 (%d bytes > %d bytes)，跳过读取: %s",
                file_size, max_file_size, script_path,
            )
            return "", 0, file_size, f"脚本文件过大 ({file_size} bytes)，跳过读取"

        content = path.read_text(encoding="utf-8", errors="ignore")
        line_count = content.count('\n') + 1 if content else 0
        return content, line_count, file_size, ""

    except Exception as e:
        return "", 0, 0, f"读取脚本失败: {e}"


# ==================== 递归追踪 ====================


def _extract_child_script_paths(content: str, base_dir: str) -> list[str]:
    """从预处理后的脚本内容中提取子脚本路径，只返回存在的文件。

    Args:
        content: 预处理后的脚本内容
        base_dir: 入口脚本所在目录（用于解析相对路径）

    Returns:
        list[str]: 存在于磁盘上的子脚本绝对路径列表（去重）
    """
    paths: list[str] = []
    for compiled in _CHILD_SCRIPT_COMPILED:
        for m in compiled.finditer(content):
            path_str = m.group(1)
            # 过滤明显不是文件路径的情况
            if path_str.startswith('-') or path_str.startswith('$'):
                continue
            # 过滤 /dev/null、管道等
            if '/dev/' in path_str or path_str.startswith('|') or path_str.startswith(';'):
                continue
            # 解析路径
            full_path = Path(base_dir) / path_str if not Path(path_str).is_absolute() else Path(path_str)
            full_path = full_path.expanduser().resolve()
            if full_path.exists() and full_path.is_file():
                paths.append(str(full_path))
    # 去重保持顺序
    return list(dict.fromkeys(paths))


def resolve_script_chain(
    entry_path: str,
    max_depth: int = 3,
    max_file_size: int = 2 * 1024 * 1024,
    max_total_size: int = 6 * 1024 * 1024,
    visited: set[str] | None = None,
    base_dir: str | None = None,
    command_scanner=None,  # CommandPatternScanner instance
) -> ScriptChainAnalysisResult:
    """递归追踪脚本调用链，读取+预处理每个节点。

    Args:
        entry_path: 入口脚本路径
        max_depth: 最大递归深度（0=入口，1=入口调用的子脚本，...）
        max_file_size: 单文件大小上限（默认 2MB）
        max_total_size: 所有文件累计大小上限（默认 6MB）
        visited: 已访问路径集合（防循环引用，如 A→B→A）
        base_dir: 入口脚本所在目录（用于解析相对路径）

    Returns:
        ScriptChainAnalysisResult: 调用链完整分析结果
    """
    if visited is None:
        visited = set()

    result = ScriptChainAnalysisResult(entry_path=entry_path)

    # 解析入口路径
    entry = Path(entry_path).expanduser()
    if not entry.is_absolute():
        if base_dir:
            entry = Path(base_dir) / entry
        else:
            entry = Path.cwd() / entry
    entry = entry.resolve()

    # 递归处理
    _resolve_recursive(
        path=entry,
        depth=0,
        max_depth=max_depth,
        max_file_size=max_file_size,
        max_total_size=max_total_size,
        visited=visited,
        result=result,
        base_dir=str(entry.parent) if entry.exists() else (base_dir or str(Path.cwd())),
        command_scanner=command_scanner,
    )

    # 设置汇总标志
    result.has_critical = any(
        n.l1_risk_level == "critical" or n.l2_risk_level == "critical" or n.keyword_risk_level == "critical"
        for n in result.nodes
    )
    result.has_high = any(
        n.l1_risk_level in ("high", "critical") or
        n.l2_risk_level in ("high", "critical") or
        n.keyword_risk_level in ("high", "critical")
        for n in result.nodes
    )
    result.first_hit_node = next(
        (n for n in result.nodes if n.l1_hit or n.l2_hit or n.is_suspicious),
        None,
    )

    return result


def _resolve_recursive(
    path: Path,
    depth: int,
    max_depth: int,
    max_file_size: int,
    max_total_size: int,
    visited: set[str],
    result: ScriptChainAnalysisResult,
    base_dir: str,
    command_scanner=None,  # CommandPatternScanner, inline L1 during recursion
) -> None:
    """递归处理单个脚本节点及其子脚本。

    边界控制：
    - visited 防循环引用
    - max_depth 防无限递归
    - max_total_size 防内存爆炸
    - 命中 critical 短路返回
    """
    path_str = str(path)

    # 边界检查
    if path_str in visited:
        logger.debug("跳过已访问脚本: %s", path_str)
        return
    if depth > max_depth:
        result.max_depth_reached = True
        logger.info("递归追踪触达深度上限 (%d), 跳过: %s", max_depth, path_str)
        return
    if result.total_size >= max_total_size:
        result.max_size_reached = True
        logger.info("累计大小触达上限 (%d bytes), 跳过: %s", max_total_size, path_str)
        return

    visited.add(path_str)

    # 读取文件
    content, line_count, file_size, error = read_script_content(
        str(path), max_file_size
    )
    if error:
        node = ScriptNode(
            script_path=str(path),
            depth=depth,
            error=error,
            file_size=file_size,
            line_count=line_count,
        )
        result.nodes.append(node)
        return

    result.total_size += file_size
    result.total_lines += line_count

    # 预处理
    preprocessed = preprocess_script_content(content)

    # 创建节点
    node = ScriptNode(
        script_path=str(path),
        raw_content=content,
        preprocessed_content=preprocessed,
        line_count=line_count,
        file_size=file_size,
        depth=depth,
    )
    result.nodes.append(node)

    logger.debug(
        "脚本节点: path=%s, depth=%d, lines=%d, size=%d bytes",
        str(path), depth, line_count, file_size,
    )

    # Inline L1 scan during recursion
    if command_scanner is not None:
        l1_result = command_scanner.scan(preprocessed)
        if l1_result.hit:
            node.l1_hit = True
            node.l1_risk_level = l1_result.risk_level
            node.l1_risk_type = l1_result.risk_type
            node.l1_reason = l1_result.reason
            node.l1_matched_patterns = l1_result.matched_patterns
            logger.info(
                "Inline L1 hit: path=%s, depth=%d, risk=%s, reason=%s",
                node.script_path, node.depth, node.l1_risk_level, node.l1_reason,
            )
            if l1_result.risk_level == "critical":
                result.has_critical = True
                result.first_hit_node = node
                logger.info("L1 critical short-circuit: %s", str(path))
                return

    # 从预处理后的内容中提取子脚本路径
    child_paths = _extract_child_script_paths(preprocessed, base_dir)
    node.child_paths = child_paths

    if child_paths:
        logger.debug("脚本 %s 提取到子脚本: %s", str(path), child_paths)

    # 递归处理子脚本
    for child_path in child_paths:
        child = Path(child_path).resolve()
        _resolve_recursive(
            path=child,
            depth=depth + 1,
            max_depth=max_depth,
            max_file_size=max_file_size,
            max_total_size=max_total_size,
            visited=visited,
            result=result,
            base_dir=str(child.parent),
            command_scanner=command_scanner,
        )
        # 如果子节点命中 critical，也短路
        if result.has_critical:
            return


# ==================== L1/L2 复用扫描 ====================


def scan_script_chain_with_l1(
    chain_result: ScriptChainAnalysisResult,
    command_scanner,  # CommandPatternScanner instance
) -> ScriptChainAnalysisResult:
    """对脚本调用链中每个节点的预处理内容做 L1 CommandPatternScanner 扫描。

    复用现有 L1 正则规则（CRITICAL_COMMAND_PATTERNS + EXTRA_DANGEROUS_PATTERNS），
    对预处理后的脚本内容逐一扫描。

    Args:
        chain_result: 脚本调用链分析结果
        command_scanner: CommandPatternScanner 实例

    Returns:
        ScriptChainAnalysisResult: 更新后的分析结果
    """
    for node in chain_result.nodes:
        # 跳过：错误节点、无内容节点、递归过程中已内联扫描的命中节点（避免重复扫描）
        if node.error or not node.preprocessed_content or node.l1_hit:
            continue

        # 复用 CommandPatternScanner 扫描预处理后的脚本内容
        l1_result = command_scanner.scan(node.preprocessed_content)
        if l1_result.hit:
            node.l1_hit = True
            node.l1_risk_level = l1_result.risk_level
            node.l1_risk_type = l1_result.risk_type
            node.l1_reason = l1_result.reason
            node.l1_matched_patterns = l1_result.matched_patterns

            logger.info(
                "脚本内容 L1 扫描命中: path=%s, depth=%d, risk_level=%s, reason=%s",
                node.script_path, node.depth, node.l1_risk_level, node.l1_reason,
            )

            # 命中 critical 短路标记
            if l1_result.risk_level == "critical":
                chain_result.has_critical = True
                chain_result.first_hit_node = node
                break

    return chain_result


def scan_script_chain_with_l2(
    chain_result: ScriptChainAnalysisResult,
    logic_checker,  # LogicRulesChecker instance
    prompt_session: str,
) -> ScriptChainAnalysisResult:
    """对脚本调用链中每个节点的预处理内容做 L2 规则扫描。

    只复用以下 L2 规则（不依赖 action_history）：
    - sensitive_path_access — 敏感路径访问检测
    - dangerous_patterns — 危险操作模式检测
    - intent_consistency — 意图偏离检测（用 prompt_session）

    不复用以下 L2 规则（依赖 action_history）：
    - read_before_write
    - password_modify_consent
    - user_deletion_consent

    Args:
        chain_result: 脚本调用链分析结果
        logic_checker: LogicRulesChecker 实例
        prompt_session: 用户输入的原始 prompt

    Returns:
        ScriptChainAnalysisResult: 更新后的分析结果
    """
    for node in chain_result.nodes:
        if node.error or not node.preprocessed_content:
            continue
        if node.l1_risk_level == "critical":
            break  # L1 已拦截，L2 不需要继续

        # 构造虚拟 a_next，action_detail 为预处理后的脚本内容
        virtual_a_next = {
            "action_type": "bash",
            "action_detail": node.preprocessed_content,
        }

        # 1. 敏感路径访问检测
        path_result = logic_checker._check_sensitive_path_access(virtual_a_next)
        if path_result.hit:
            node.l2_hit = True
            node.l2_violated_rule = f"script_sensitive_path:{node.script_path}"
            node.l2_risk_level = path_result.risk_level
            node.l2_risk_type = path_result.risk_type
            node.l2_reason = f"脚本 {node.script_path} (深度 {node.depth}) 中: {path_result.reason}"

            logger.info(
                "脚本内容 L2 敏感路径命中: path=%s, depth=%d, risk_level=%s",
                node.script_path, node.depth, node.l2_risk_level,
            )

            if path_result.risk_level == "critical":
                chain_result.has_critical = True
                chain_result.first_hit_node = node
                break
            continue  # 已命中，跳过此节点后续检测

        # 2. 危险操作模式检测
        dangerous_result = logic_checker._check_dangerous_patterns(virtual_a_next)
        if dangerous_result.hit:
            node.l2_hit = True
            node.l2_violated_rule = f"script_dangerous_pattern:{node.script_path}"
            node.l2_risk_level = dangerous_result.risk_level
            node.l2_risk_type = dangerous_result.risk_type
            node.l2_reason = f"脚本 {node.script_path} (深度 {node.depth}) 中: {dangerous_result.reason}"

            logger.info(
                "脚本内容 L2 危险模式命中: path=%s, depth=%d, risk_level=%s",
                node.script_path, node.depth, node.l2_risk_level,
            )

            if dangerous_result.risk_level == "critical":
                chain_result.has_critical = True
                chain_result.first_hit_node = node
                break
            continue

        # 3. 意图偏离检测（用 prompt_session 判断脚本内容是否偏离用户意图）
        if prompt_session and prompt_session.strip():
            intent_result = logic_checker._check_intent_consistency(
                prompt_session, virtual_a_next
            )
            if intent_result.hit:
                node.l2_hit = True
                node.l2_violated_rule = f"script_intent_deviation:{node.script_path}"
                node.l2_risk_level = intent_result.risk_level
                node.l2_risk_type = intent_result.risk_type
                node.l2_reason = f"脚本 {node.script_path} (深度 {node.depth}) 中意图偏离: {intent_result.reason}"

                logger.info(
                    "脚本内容 L2 意图偏离命中: path=%s, depth=%d, reason=%s",
                    node.script_path, node.depth, node.l2_reason,
                )

    # 更新汇总标志
    if not chain_result.has_high:
        chain_result.has_high = any(
            n.l2_risk_level in ("high", "critical") for n in chain_result.nodes
        )

    return chain_result


def scan_script_chain_keywords(
    chain_result: ScriptChainAnalysisResult,
) -> ScriptChainAnalysisResult:
    """对调用链中每个节点做 SUSPICIOUS_PATTERNS 关键词扫描（兼容 L3 流程）。

    只对预处理后的内容做关键词扫描，且只扫描 L1/L2 未命中的节点
    （已命中的节点不再需要关键词扫描辅助 L3）。

    Args:
        chain_result: 脚本调用链分析结果

    Returns:
        ScriptChainAnalysisResult: 更新后的分析结果
    """
    for node in chain_result.nodes:
        if node.error or not node.preprocessed_content:
            continue
        # L1/L2 已命中的节点不需要关键词扫描辅助 L3
        if node.l1_hit or node.l2_hit:
            continue

        matched = scan_script_for_keywords(node.preprocessed_content)
        node.matched_keywords = matched
        node.is_suspicious = bool(matched)

        if matched:
            risk_level, risk_reason = assess_keyword_combination_risk(
                matched, node.preprocessed_content
            )
            node.keyword_risk_level = risk_level
            node.keyword_risk_reason = risk_reason

            logger.info(
                "脚本关键词命中: path=%s, depth=%d, keywords=%s, risk_level=%s",
                node.script_path, node.depth, matched, risk_level,
            )

    return chain_result


# ==================== 入口函数 ====================


def analyze_script_content(
    action_type: str,
    action_detail: str,
) -> ScriptChainAnalysisResult:
    """分析脚本内容 — 新版（递归追踪 + 预处理 + L1/L2 扫描）。

    只在 action_type 为 bash 且命令中包含脚本执行时才进行分析。

    流程：
    1. 提取脚本路径
    2. 递归追踪读取调用链
    3. L1 CommandPatternScanner 扫描预处理内容
    4. 关键词扫描（兼容 L3）
    5. 返回 ScriptChainAnalysisResult

    Args:
        action_type: 动作类型
        action_detail: 动作详情（命令）

    Returns:
        ScriptChainAnalysisResult: 调用链完整分析结果
    """
    # 只处理 bash 类型的动作
    if action_type.lower() != "bash":
        return ScriptChainAnalysisResult()

    # 提取脚本路径
    script_path = extract_script_path(action_detail)
    if not script_path:
        # 内联脚本（python -c 等）—— 尝试提取引用的文件路径
        inline_path = _extract_inline_script_path(action_detail)
        if not inline_path:
            return ScriptChainAnalysisResult()
        script_path = inline_path

    logger.info("检测到脚本执行，启动递归追踪: %s", script_path)

    # 创建 L1 扫描器：传入递归过程，命中 critical 时即时短路，避免读取无用子脚本
    from .heuristic_detector import CommandPatternScanner
    command_scanner = CommandPatternScanner()

    # 递归追踪读取调用链（传入 command_scanner，递归过程中即时做 L1 扫描）
    chain_result = resolve_script_chain(script_path, command_scanner=command_scanner)

    if not chain_result.nodes:
        return chain_result

    # L1 兜底扫描：递归过程中已内联扫描的命中节点（l1_hit）跳过，其余补扫
    scan_script_chain_with_l1(chain_result, command_scanner)

    # 关键词扫描（兼容 L3）
    scan_script_chain_keywords(chain_result)

    return chain_result


# ==================== L3 prompt 格式化 ====================


def format_script_chain_for_prompt(chain_result: ScriptChainAnalysisResult) -> str:
    """将脚本调用链分析结果格式化为 LLM prompt 可用的文本。

    - L1/L2 已命中的节点不会走到 L3（在 audit_agent 中直接 Deny），不在此展示。
    - 有可疑信号（关键词命中但 L1/L2 未拦截）的节点：输出可疑内容供 L3 深度判断。
    - 全部节点均无危险/可疑信号（即"安全脚本"）：输出"已分析、未发现危险"明确结论
      + 内容预览，给 LLM 一个判 Allow 的依据，避免 LLM 因缺乏脚本检查信号而对
      /tmp 等路径过度警惕、误判 Deny。

    Args:
        chain_result: 脚本调用链分析结果

    Returns:
        str: 格式化后的文本
    """
    if not chain_result.nodes:
        return ""

    # 检查是否有读取错误（且只有一个节点 — 入口脚本读取失败）
    error_nodes = [n for n in chain_result.nodes if n.error]
    if error_nodes and len(chain_result.nodes) == 1:
        return f"⚠️ 脚本内容分析失败: {error_nodes[0].error}"

    # 只输出有可疑信号的节点（关键词扫描命中但 L1/L2 未拦截的）
    suspicious_nodes = [
        n for n in chain_result.nodes
        if n.is_suspicious and not n.l1_hit and not n.l2_hit
    ]
    if not suspicious_nodes:
        # 安全脚本：三步扫描（L1/L2/关键词）均未命中。输出"已分析、未发现危险"
        # 结论 + 各节点内容预览，让 LLM 据此判 Allow，而非凭空警惕 /tmp 等路径。
        # 仅展示无错误节点，内容预览截断到 600 字符（避免 prompt 过长）。
        safe_nodes = [n for n in chain_result.nodes if not n.error]
        if not safe_nodes:
            return ""
        parts = ["✓ 脚本内容已分析，未发现危险/可疑操作（L1/L2/关键词扫描均未命中）："]
        for node in safe_nodes:
            preview = (node.preprocessed_content or "")[:600]
            if len(node.preprocessed_content or "") > 600:
                preview += "\n... (截断，完整内容 {} 字符)".format(
                    len(node.preprocessed_content or ""))
            parts.append(
                f"   脚本路径: {node.script_path}（递归深度 {node.depth}，{node.line_count} 行）"
                f"\n   内容（预处理后）:\n```\n{preview}\n```"
            )
        return "\n".join(parts)

    parts = []
    for node in suspicious_nodes:
        keywords_str = ", ".join(node.matched_keywords)

        risk_hint = ""
        if node.keyword_risk_level and node.keyword_risk_reason:
            risk_level_upper = node.keyword_risk_level.upper()
            risk_hint = (
                f"\n\n🔴 **风险评估**: {risk_level_upper} 级别\n"
                f"**风险原因**: {node.keyword_risk_reason}\n"
                f"**建议**: 此脚本应被拒绝（Deny），除非有明确的业务需求且已确认目标服务器可信。"
            )

        # 内容截断：预处理后内容最多展示 2000 字符给 LLM（避免 prompt 过长）
        content_preview = node.preprocessed_content[:2000]
        if len(node.preprocessed_content) > 2000:
            content_preview += "\n... (截断，完整内容 {} 字符)".format(len(node.preprocessed_content))

        parts.append(
            f"⚠️ 脚本内容包含可疑关键词:\n"
            f"   脚本路径: {node.script_path}\n"
            f"   递归深度: {node.depth}\n"
            f"   可疑关键词: {keywords_str}\n"
            f"   脚本行数: {node.line_count}"
            f"{risk_hint}\n\n"
            f"### 脚本内容（预处理后）###\n```\n{content_preview}\n```\n"
        )

    header = (
        f"⚠️ 检测到脚本执行链（共 {len(chain_result.nodes)} 个脚本），"
        f"其中 {len(suspicious_nodes)} 个包含可疑关键词:\n"
    )

    if chain_result.max_depth_reached:
        header += "⚠️ 注意：递归追踪触达深度上限，可能存在更深层的脚本调用未被扫描。\n"
    if chain_result.max_size_reached:
        header += "⚠️ 注意：累计文件大小触达上限，部分脚本可能未被完整读取。\n"

    return header + "\n---\n\n".join(parts)


# ==================== 向后兼容（deprecated） ====================
# 旧接口保留，内部使用新实现，标记为 deprecated


def format_script_analysis_for_prompt(result) -> str:
    """[deprecated] 使用 format_script_chain_for_prompt 替代"""
    # 如果传入的是 ScriptChainAnalysisResult，直接用新函数
    if isinstance(result, ScriptChainAnalysisResult):
        return format_script_chain_for_prompt(result)
    # 如果传入的是旧式 ScriptAnalysisResult（不存在于新代码中），
    # 返回空字符串（旧接口不应再被使用）
    return ""


# 旧数据类型向后兼容引用 — 实际不再使用
# ScriptAnalysisResult 已被 ScriptNode + ScriptChainAnalysisResult 替代
