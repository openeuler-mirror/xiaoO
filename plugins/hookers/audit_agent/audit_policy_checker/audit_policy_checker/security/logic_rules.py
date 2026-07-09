"""逻辑规则检测 — 基于动作历史和意图一致性的安全检查

增强的安全检测机制：
1. read_before_write 原则 — 写入文件前必须有读取操作
2. 意图一致性检测 — 动作是否与原始 prompt 意图一致
3. 敏感路径访问检测 — 访问系统关键文件
4. 危险操作模式检测 — 批量删除/修改、通配符滥用
5. 密码修改授权检测 — 非交互式密码修改需用户授权
6. 用户删除授权检测 — 删除系统用户需用户授权

规则来源优先级：
  runtime JSON 用户本地副本 > 源码种子默认值
  禁用的分类/规则在 runtime JSON 中 enabled/category_enabled=False，加载时自动过滤。
"""

import json
import os
import re
from pathlib import Path

from .types import LogicRuleResult
from ..runtime_config import (
    load_runtime_config,
    get_enabled_l2_sensitive_paths_or_default,
    get_enabled_l2_intent_patterns_or_default,
    get_enabled_l2_password_patterns_or_default,
    get_enabled_l2_user_deletion_patterns_or_default,
    is_l2_category_enabled,
)

# ==================== 敏感路径列表 ====================
SENSITIVE_PATHS: list[dict] = [
    # 认证与密钥（credential=True — 只读也算泄密，无论读写都拦截）
    {"path": "/etc/shadow", "risk_level": "critical", "desc": "系统密码文件", "credential": True},
    {"path": "/etc/gshadow", "risk_level": "critical", "desc": "系统组密码文件", "credential": True},
    {"path": "/etc/passwd", "risk_level": "high", "desc": "系统用户文件"},
    {"path": "/etc/sudoers", "risk_level": "critical", "desc": "sudo 配置", "credential": True},
    {"path": ".ssh/id_rsa", "risk_level": "critical", "desc": "SSH 私钥", "credential": True},
    {"path": ".ssh/id_ed25519", "risk_level": "critical", "desc": "SSH 私钥 (ed25519)", "credential": True},
    {"path": ".ssh/authorized_keys", "risk_level": "high", "desc": "SSH 授权密钥", "credential": True},
    # 系统配置
    {"path": "/etc/hosts", "risk_level": "medium", "desc": "DNS 解析配置"},
    {"path": "/etc/crontab", "risk_level": "high", "desc": "系统定时任务"},
    {"path": "/etc/systemd/", "risk_level": "high", "desc": "systemd 服务配置"},
    {"path": "/etc/ssh/sshd_config", "risk_level": "high", "desc": "SSH 服务配置"},
    # 危险目录
    {"path": "/boot/", "risk_level": "critical", "desc": "启动引导目录"},
    {"path": "/proc/sys/", "risk_level": "high", "desc": "内核参数"},
    {"path": "/sys/", "risk_level": "high", "desc": "sysfs 内核接口"},
    # 设备文件
    {"path": "/dev/zero", "risk_level": "high", "desc": "零设备（无限空字节输出）"},
    {"path": "/dev/null", "risk_level": "medium", "desc": "空设备（丢弃所有写入）"},
    {"path": "/dev/random", "risk_level": "medium", "desc": "随机数设备"},
    {"path": "/dev/urandom", "risk_level": "medium", "desc": "伪随机数设备"},
    {"path": "/dev/mem", "risk_level": "critical", "desc": "物理内存访问设备"},
    {"path": "/dev/kmsg", "risk_level": "high", "desc": "内核消息缓冲区"},
    # xiaoO 系统级安全 Skill（系统级路径 - 所有用户共享的内置 skill）
    {"path": "/usr/lib/.xiaoo/skills/xiaoo-guardian/", "risk_level": "critical", "desc": "xiaoO 系统级安全防护 Skill 目录"},
    # xiaoO 用户级安全 Skill（用户级路径 - 兼容旧版本）
    {"path": "~/.xiaoo/skills/xiaoo-guardian/", "risk_level": "critical", "desc": "xiaoO 用户级安全防护 Skill 目录"},
    # 凭据文件（credential=True — 无论读写都拦截，使用 \b 边界匹配避免部分匹配误报）
    {"path": "credentials.yml", "risk_level": "high", "desc": "凭据配置文件", "credential": True},
    {"path": "credentials.yaml", "risk_level": "high", "desc": "凭据配置文件", "credential": True},
    {"path": "secrets.yml", "risk_level": "high", "desc": "密钥配置文件", "credential": True},
    {"path": "secrets.yaml", "risk_level": "high", "desc": "密钥配置文件", "credential": True},
    {"path": ".env", "risk_level": "high", "desc": "环境变量/凭据配置文件", "credential": True},
]

# ==================== 写入操作关键词 ====================
WRITE_KEYWORDS = [
    "write", "写入", "save", "保存", "modify", "修改",
    "update", "更新", "delete", "删除", "remove", "移除",
    "create", "创建", "append", "追加", "truncate", "截断",
    "overwrite", "覆盖", "replace", "替换",
]

# ==================== 读取操作关键词 ====================
READ_KEYWORDS = [
    "read", "读取", "cat", "head", "tail", "less", "more",
    "view", "查看", "open", "打开", "load", "加载",
    "grep", "search", "搜索", "find", "查找",
]

# ==================== 只读命令集合（用于重定向写判定与敏感路径读写区分）====================
# 这些命令本身只读，即使带 `>` 重定向也不应判为写文件意图。
# 1) 传统文本查看/过滤命令
# 2) 只读系统信息查询命令（lsblk/blockdev/smartctl/udevadm/dmidecode 等，常配 2>/dev/null 查设备信息）
# 注：sed/awk 虽能原地编辑(-i)，此处仍按只读看待——原地改写由其他规则另行覆盖。
READ_ONLY_COMMANDS: set[str] = {
    # 传统文本查看/过滤
    "cat", "head", "tail", "less", "more", "grep", "find", "awk", "sed",
    "sort", "uniq", "wc", "cut", "strings", "od", "xxd", "hexdump", "tr",
    "file", "stat", "du", "df", "ls", "dir", "tree", "nl", "tac", "rev",
    # 只读系统/设备信息查询
    "lsblk", "blockdev", "smartctl", "udevadm", "dmidecode", "lscpu", "lspci",
    "lsusb", "lsmem", "lsns", "lsof", "hwinfo", "inxi", "fdisk", "parted",
    "dmesg", "journalctl", "systemctl", "hostnamectl", "localectl", "timedatectl",
    "ps", "top", "free", "vmstat", "iostat", "mpstat", "pidof", "pgrep",
    "ip", "ifconfig", "route", "ss", "netstat", "arp", "ethtool", "nmcli",
    "uname", "arch", "nproc", "getconf", "getent", "id", "whoami", "who",
    "w", "last", "uptime", "lsmod", "modinfo", "sysctl",
}

# 重定向到 /dev/null 的丢弃写法（2>/dev/null、&>/dev/null、>/dev/null）——这是丢弃输出的标准
# shell 实践，不是写文件意图，不应计入"写操作"判定。
_DEVNULL_REDIRECT_RE = re.compile(r"(?:2|&|1)?\s*>\s*/dev/null\b")

# ==================== 写/删命令模式（用于写操作判定）====================
# WRITE_KEYWORDS 走子串匹配，无法可靠识别 `rm`/`cp`/`dd` 等命令（"rm" 会误命中 arm/form），
# 故单独用词边界正则识别这些命令出现即视为写/删意图。覆盖：删除、复制写入、重定向写入、
# 块设备写入、文件系统格式化等。
_WRITE_COMMAND_RES = [
    re.compile(r"\brm\b"),          # 删除文件
    re.compile(r"\bunlink\b"),      # 删除文件
    re.compile(r"\brmdir\b"),       # 删除目录
    re.compile(r"\bshred\b"),       # 安全擦除
    re.compile(r"\bcp\b"),          # 复制（会写目标）
    re.compile(r"\bmv\b"),          # 移动（覆盖目标）
    re.compile(r"\binstall\b"),     # 复制并设置属性
    re.compile(r"\btee\b"),         # 从 stdin 写文件
    re.compile(r"\bdd\b"),          # 块复制（常写块设备）
    re.compile(r"\bmkfs\b"),        # 格式化文件系统
    re.compile(r"\b(fdisk|parted|cfdisk|sfdisk)\b"),  # 分区表写入
    re.compile(r"\bchmod\b"),       # 改权限
    re.compile(r"\bchown\b"),       # 改属主
    re.compile(r"\btruncate\b"),    # 截断文件
    # 任意重定向写入（> file、>> file）由 _is_write_operation 末尾的重定向分支处理，
    # 那里会先排除 /dev/null 丢弃并跳过只读命令，避免在此误判 2>/dev/null。
]


def _is_write_operation(action_type: str, action_detail: str) -> bool:
    """综合判定是否为写/删操作：关键词命中 或 写/删命令命中 或（真实重定向且非只读命令）。
    统一供 _check_read_before_write 与 _check_sensitive_path_access 使用，避免两处逻辑漂移。
    """
    if any(kw in action_type or kw in action_detail for kw in WRITE_KEYWORDS):
        return True
    if any(rx.search(action_detail) for rx in _WRITE_COMMAND_RES):
        return True
    # 重定向写入：先排除 /dev/null 丢弃，再看是否还有真实重定向且首命令非只读
    if ">" in action_detail:
        detail_without_devnull = _DEVNULL_REDIRECT_RE.sub("", action_detail)
        if ">" in detail_without_devnull:
            first_word = action_detail.split()[0].strip().lower() if action_detail.split() else ""
            if first_word not in READ_ONLY_COMMANDS:
                return True
    return False

# ==================== 非交互式密码修改命令模式 ====================
PASSWORD_MODIFY_PATTERNS: list[str] = [
    r"\|\s*passwd\b",              # echo pass | passwd
    r"\bpasswd\s+--stdin\b",       # passwd --stdin
    r"\bchpasswd\b",               # 批量改密码
    r"\bchgpasswd\b",              # 批量修改组密码
    r"\b(newusers|lnewusers)\b",   # 批量添加用户（标准/libuser）
    r"\bpasswd\s+-[a-zA-Z]*d\b",   # 删除密码 (passwd -d)
    r"\bpasswd\s+-[a-zA-Z]*l\b",   # 锁定账户 (passwd -l)
    r"\blpasswd\s",                # lpasswd（libuser 版 passwd）
    r"\bgpasswd\s+(?!-[adA])",     # 组密码管理（排除 -a/-d/-A 成员管理）
    r"\b(useradd|luseradd|groupadd|usermod|lusermod|groupmod|lgroupmod)\b[^&|;]*?\s-p\b",  # 通用命令 + -p 参数
]

# ==================== 用户删除命令模式 ====================
USER_DELETION_PATTERNS: list[str] = [
    r"\buserdel\s",
    r"\bdeluser\s",
    r"\bluserdel\s",       # libuser 版删除用户
    r"\bgroupdel\s",       # 删除用户组
    r"\blgroupdel\s",      # libuser 版删除组
]

# ==================== 意图偏离关键词对 ====================
# prompt 意图关键词 → a_next 中不应出现的危险关键词
INTENT_DEVIATION_PATTERNS: list[dict] = [
    {
        "intent_keywords": ["读取", "查看", "分析", "read", "analyze", "view", "list", "列出"],
        "dangerous_actions": ["删除", "remove", "delete", "rm ", "格式化", "format"],
        "reason": "原始意图为读取/分析，下一步动作疑似偏离意图执行了删除操作",
    },
    {
        "intent_keywords": ["查询", "搜索", "search", "query", "find"],
        "dangerous_actions": ["修改", "修改", "update", "write", "写入", "alter"],
        "reason": "原始意图为查询/搜索，下一步动作疑似偏离意图执行了修改操作",
    },
]


class LogicRulesChecker:
    """逻辑规则检测器

    规则来源：优先从 runtime JSON 加载启用的规则，
    如果 runtime JSON 无效则回退到源码硬编码默认值。
    分类级别开关：category_enabled=False 时跳过整个分类的检测。
    """

    def __init__(self):
        # 从 runtime config 加载规则；
        # 仅在配置文件不存在时回退到源码硬编码默认值，
        # 配置文件存在但规则列表为空（用户逐条禁用）时不回退。
        runtime = load_runtime_config()
        self._sensitive_paths = get_enabled_l2_sensitive_paths_or_default(runtime)
        self._intent_patterns = get_enabled_l2_intent_patterns_or_default(runtime)
        self._password_patterns = get_enabled_l2_password_patterns_or_default(runtime)
        self._user_deletion_patterns = get_enabled_l2_user_deletion_patterns_or_default(runtime)
        self._runtime = runtime

    def check(
        self,
        prompt_session: str,
        action_history: list[dict[str, object]],
        a_next: dict[str, str],
        reason: str,
    ) -> LogicRuleResult:
        """
        执行逻辑规则检测。

        Args:
            prompt_session: 用户输入的原始 prompt
            action_history: 历史动作序列
            a_next: 下一步动作
            reason: 执行该动作的理由

        Returns:
            LogicRuleResult: 检测结果
        """
        # 1. read_before_write 原则（分类开关控制）
        if is_l2_category_enabled(self._runtime, "read_before_write"):
            rbw_result = self._check_read_before_write(action_history, a_next, reason)
            if rbw_result.hit:
                return rbw_result

        # 2. 意图一致性检测（分类开关控制）
        if is_l2_category_enabled(self._runtime, "intent_consistency"):
            intent_result = self._check_intent_consistency(prompt_session, a_next)
            if intent_result.hit:
                return intent_result

        # 3. 敏感路径访问检测（分类开关控制）
        if is_l2_category_enabled(self._runtime, "sensitive_path_access"):
            path_result = self._check_sensitive_path_access(a_next)
            if path_result.hit:
                return path_result

        # 4. 危险操作模式检测（分类开关控制）
        if is_l2_category_enabled(self._runtime, "dangerous_patterns"):
            dangerous_result = self._check_dangerous_patterns(a_next)
            if dangerous_result.hit:
                return dangerous_result

        # 5. 密码修改授权检测（分类开关控制）
        if is_l2_category_enabled(self._runtime, "password_modify_consent"):
            consent_result = self._check_password_consent(action_history, a_next)
            if consent_result.hit:
                return consent_result

        # 6. 用户删除授权检测（分类开关控制）
        if is_l2_category_enabled(self._runtime, "user_deletion_consent"):
            user_del_result = self._check_user_deletion_consent(action_history, a_next)
            if user_del_result.hit:
                return user_del_result

        return LogicRuleResult(hit=False)

    def _check_read_before_write(
        self, action_history: list[dict[str, object]], a_next: dict[str, str], reason: str = ""
    ) -> LogicRuleResult:
        """
        read_before_write 原则：
        如果 a_next 是对某个文件的写入操作，但历史动作序列中没有该文件的读取操作，则 Deny。
        排除非写入类工具（如 ask_user_question、glob、grep 等）。
        """
        action_detail = a_next.get("action_detail", "").lower()
        action_type = a_next.get("action_type", "").lower()

        # 排除非写入类工具
        non_write_tools = {"ask_user_question", "glob", "grep", "list_dir", "count_text_length", "search"}
        if any(t in action_type or t in action_detail for t in non_write_tools):
            return LogicRuleResult(hit=False)

        # 判断是否为写入操作（统一用 _is_write_operation，含关键词/写删命令/重定向）
        is_write = _is_write_operation(action_type, action_detail)

        if not is_write:
            return LogicRuleResult(hit=False)

        # 提取 a_next 中涉及的文件路径
        target_paths = self._extract_file_paths(action_detail)
        if not target_paths:
            # 无法提取路径时，不触发此规则
            return LogicRuleResult(hit=False)

        # 检查历史动作中是否有对这些路径的读取操作
        history_read_paths = set()
        for action in action_history:
            # action_detail 可能是 dict 或 string，需要统一处理
            hist_detail_raw = action.get("action_detail", "") if isinstance(action, dict) else ""
            if isinstance(hist_detail_raw, dict):
                hist_detail = json.dumps(hist_detail_raw, ensure_ascii=False).lower()
            else:
                hist_detail = str(hist_detail_raw).lower()
            hist_name = action.get("name", action.get("action_type", "")).lower() if isinstance(action, dict) else ""
            is_read = any(kw in hist_name or kw in hist_detail for kw in READ_KEYWORDS)
            if is_read:
                hist_paths = self._extract_file_paths(hist_detail)
                history_read_paths.update(hist_paths)

        # 检查写入目标是否在已读取的路径中
        unread_paths = []
        for path in target_paths:
            # 精确匹配或前缀匹配
            if not any(
                path == rp or path.startswith(rp) or rp.startswith(path)
                for rp in history_read_paths
            ):
                # 排除新文件创建：文件不存在于磁盘时视为新建，直接放行
                # 同时检查 reason/action_detail 中是否有创建/新建关键词
                is_create = not os.path.exists(path)
                if not is_create:
                    create_keywords = ["创建", "新建", "create", "new file", "touch"]
                    is_create = any(kw in reason.lower() or kw in action_detail for kw in create_keywords)
                if not is_create:
                    unread_paths.append(path)

        if unread_paths:
            return LogicRuleResult(
                hit=True,
                violated_rule="read_before_write",
                risk_level="high",
                reason=(
                    f"违反 read_before_write 安全原则：对文件 {unread_paths[0]} 执行写入操作，"
                    f"但历史执行序列中未发现对该文件的读取操作"
                ),
                risk_type="read_before_write",
            )

        return LogicRuleResult(hit=False)

    def _check_intent_consistency(
        self, prompt_session: str, a_next: dict[str, str]
    ) -> LogicRuleResult:
        """
        意图一致性检测：
        检查 a_next 是否与 prompt_session 的原始意图偏离。
        当 prompt_session 为空时跳过此检测（无法判断意图偏离）。
        """
        if not prompt_session or not prompt_session.strip():
            return LogicRuleResult(hit=False)

        prompt_lower = prompt_session.lower()
        action_detail = a_next.get("action_detail", "").lower()

        for pattern in self._intent_patterns:
            intent_matched = any(kw in prompt_lower for kw in pattern["intent_keywords"])
            dangerous_matched = any(kw in action_detail for kw in pattern["dangerous_actions"])
            if intent_matched and dangerous_matched:
                return LogicRuleResult(
                    hit=True,
                    violated_rule="intent_consistency",
                    risk_level="high",
                    reason=pattern["reason"],
                    risk_type="intent_deviation",
                )

        return LogicRuleResult(hit=False)

    def _check_sensitive_path_access(self, a_next: dict[str, str]) -> LogicRuleResult:
        """
        敏感路径访问检测：
        检查 a_next 中是否包含对敏感路径的修改/删除操作（读取操作放行）。
        支持 ~ 路径展开，同时检查原始模式和展开后的绝对路径。
        """
        action_type = a_next.get("action_type", "").lower()
        action_detail = a_next.get("action_detail", "").lower()

        # 判断是否为写入/删除操作（统一用 _is_write_operation，与 read_before_write 对齐）
        is_write_op = _is_write_operation(action_type, action_detail)

        for sp in self._sensitive_paths:
            raw_path = sp["path"]
            path = raw_path.lower()
            is_credential = sp.get("credential", False)

            # 检查路径是否在操作中出现
            if is_credential:
                # 凭据文件使用边界匹配，避免非文件名拼接（如 something_credentials_yml）误报
                # 以 . 或 / 开头的路径，其首字符不是单词字符，\b 在它前面不构成边界
                # （空格→/ 之间没有 \b），故用 (?:^|[\s/\\]) 替代前导 \b。
                escaped = re.escape(path)
                if path.startswith(".") or path.startswith("/"):
                    path_match = bool(re.search(rf"(?:^|[\s/\\]){escaped}\b", action_detail))
                else:
                    path_match = bool(re.search(rf"\b{escaped}\b", action_detail))
            else:
                path_match = path in action_detail

            # 如果路径以 ~ 开头， also 检查展开后的绝对路径
            if not path_match and raw_path.startswith("~"):
                expanded = os.path.expanduser(raw_path)
                if expanded.lower() in action_detail:
                    path_match = True
                else:
                    # 检查家目录的绝对路径形式（如 /home/hkl/.xiaoo/...）
                    home_dir = str(Path.home())
                    if expanded.startswith(home_dir):
                        rel_path = expanded[len(home_dir):]
                        if rel_path.lower() in action_detail or expanded.lower() in action_detail:
                            path_match = True

            if path_match:
                # 凭据文件：无论读写都拦截（读凭据文件同样危险）
                if is_credential:
                    return LogicRuleResult(
                        hit=True,
                        violated_rule="sensitive_path_access",
                        risk_level=sp["risk_level"],
                        reason=f"访问敏感路径: {sp['desc']} ({sp['path']})",
                        risk_type="file_access",
                    )
                # 非凭据路径：只拦截写入/删除操作，只读访问（如 cat /sys/class/block/sda/size、
                # lsblk、smartctl 查设备信息）放行。修复历史误报：只读访问 /sys/、/proc/sys/、
                # /dev/* 被一刀切拦截。
                if not is_write_op:
                    continue  # 允许读取操作
                # 写入/删除敏感路径 → 拦截
                return LogicRuleResult(
                    hit=True,
                    violated_rule="sensitive_path_access",
                    risk_level=sp["risk_level"],
                    reason=f"访问敏感路径: {sp['desc']} ({sp['path']})",
                    risk_type="file_access",
                )

        return LogicRuleResult(hit=False)

    def _check_dangerous_patterns(self, a_next: dict[str, str]) -> LogicRuleResult:
        """
        危险操作模式检测：
        检测通配符滥用、批量操作等危险模式。
        """
        action_detail = a_next.get("action_detail", "").lower()
        action_type = a_next.get("action_type", "").lower()

        # 排除 file_write/file_edit，因为 action_detail 包含文件内容
        # 对文件内容进行"通配符+删除"检测会产生大量误报
        if action_type in ("file_write", "file_edit"):
            return LogicRuleResult(hit=False)

        # 通配符 + 删除/修改 — 排除 /tmp 等临时目录的清理操作
        _TEMP_DIRS = ("/tmp/", "/var/tmp/", "/run/", "/dev/shm/")
        is_temp_path = any(td in action_detail for td in _TEMP_DIRS)

        if not is_temp_path and ("*" in action_detail or "?" in action_detail) and any(
            kw in action_detail for kw in ["rm ", "rm\t", "del ", "del\t", "delete", "remove ", "remove\t", "删除", "移除"]
        ):
            return LogicRuleResult(
                hit=True,
                violated_rule="dangerous_wildcard_operation",
                risk_level="high",
                reason="检测到通配符结合删除操作，可能造成批量误删",
                risk_type="script_execution",
            )

        # 重定向覆盖关键文件
        override_patterns = [r">\s*/etc/", r">\s*/boot/", r">\s*/proc/"]
        for pat in override_patterns:
            if re.search(pat, action_detail):
                return LogicRuleResult(
                    hit=True,
                    violated_rule="dangerous_redirect",
                    risk_level="critical",
                    reason="检测到重定向覆盖写入关键系统目录",
                    risk_type="file_access",
                )

        return LogicRuleResult(hit=False)

    def _check_password_consent(
        self, action_history: list[dict[str, object]], a_next: dict[str, str]
    ) -> LogicRuleResult:
        """
        密码修改授权检测：
        非交互式密码修改命令必须在 action_history 中有 ask_user_question 且用户返回了密码。
        交互式 passwd（无参数）不在此列，因为 LLM 无法完成交互式输入。
        """
        action_detail = a_next.get("action_detail", "").lower()
        action_type = a_next.get("action_type", "").lower()

        # 只检测 bash 工具中的密码修改命令
        if action_type != "bash":
            return LogicRuleResult(hit=False)

        is_password_modify = any(
            re.search(p, action_detail, re.IGNORECASE) for p in self._password_patterns
        )
        if not is_password_modify:
            return LogicRuleResult(hit=False)

        # 检查 action_history 中是否有 ask_user_question 且用户返回了答案
        for action in action_history:
            if not isinstance(action, dict):
                continue
            hist_type = str(action.get("action_type", action.get("name", ""))).lower()
            output = str(action.get("output", ""))
            if hist_type == "ask_user_question" and output.strip():
                return LogicRuleResult(hit=False)  # 用户已授权，放行

        return LogicRuleResult(
            hit=True,
            violated_rule="password_modify_requires_consent",
            risk_level="high",
            reason=(
                "执行非交互式密码修改命令，但历史执行序列中未发现向用户确认密码的 "
                "ask_user_question 操作，可能存在未授权的密码修改风险"
            ),
            risk_type="consent_missing",
        )

    def _check_user_deletion_consent(
        self, action_history: list[dict[str, object]], a_next: dict[str, str]
    ) -> LogicRuleResult:
        """
        用户删除授权检测：
        删除用户的命令必须在 action_history 中有 ask_user_question 且用户确认了操作。
        """
        action_detail = a_next.get("action_detail", "").lower()
        action_type = a_next.get("action_type", "").lower()

        # 只检测 bash 工具中的用户删除命令
        if action_type != "bash":
            return LogicRuleResult(hit=False)

        is_user_deletion = any(
            re.search(p, action_detail, re.IGNORECASE) for p in self._user_deletion_patterns
        )
        if not is_user_deletion:
            return LogicRuleResult(hit=False)

        # 检查 action_history 中是否有 ask_user_question 且用户确认了
        for action in action_history:
            if not isinstance(action, dict):
                continue
            hist_type = str(action.get("action_type", action.get("name", ""))).lower()
            output = str(action.get("output", ""))
            if hist_type == "ask_user_question" and output.strip():
                return LogicRuleResult(hit=False)  # 用户已授权，放行

        return LogicRuleResult(
            hit=True,
            violated_rule="user_deletion_requires_consent",
            risk_level="high",
            reason=(
                "执行删除系统用户命令，但历史执行序列中未发现向用户确认的 "
                "ask_user_question 操作，可能存在未授权的用户删除风险"
            ),
            risk_type="consent_missing",
        )

    @staticmethod
    def _extract_file_paths(text: str) -> list[str]:
        """
        从文本中提取文件路径。
        简单提取以 / 开头的路径和引号内的路径。
        """
        paths = []

        # 提取 / 开头的路径
        unix_paths = re.findall(r'(?:^|\s)(/[^\s;|&><\'"]+)', text)
        paths.extend(unix_paths)

        # 提取引号内的路径
        quoted_paths = re.findall(r'["\'](/[^\"\']+)["\']', text)
        paths.extend(quoted_paths)

        # 提取 Windows 路径
        win_paths = re.findall(r'(?:^|\s)([A-Za-z]:[\\/][^\s;|&><\'"]+)', text)
        paths.extend(win_paths)

        # 去重
        return list(dict.fromkeys(paths))
