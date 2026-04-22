"""逻辑规则检测 — 基于动作历史和意图一致性的安全检查

增强的安全检测机制：
1. read_before_write 原则 — 写入文件前必须有读取操作
2. 意图一致性检测 — 动作是否与原始 prompt 意图一致
3. 敏感路径访问检测 — 访问系统关键文件
4. 危险操作模式检测 — 批量删除/修改、通配符滥用
"""

import os
import re
from pathlib import Path

from .types import LogicRuleResult

# ==================== 敏感路径列表 ====================
SENSITIVE_PATHS: list[dict] = [
    # 认证与密钥
    {"path": "/etc/shadow", "risk_level": "critical", "desc": "系统密码文件"},
    {"path": "/etc/passwd", "risk_level": "high", "desc": "系统用户文件"},
    {"path": "/etc/sudoers", "risk_level": "critical", "desc": "sudo 配置"},
    {"path": ".ssh/id_rsa", "risk_level": "critical", "desc": "SSH 私钥"},
    {"path": ".ssh/id_ed25519", "risk_level": "critical", "desc": "SSH 私钥 (ed25519)"},
    {"path": ".ssh/authorized_keys", "risk_level": "high", "desc": "SSH 授权密钥"},
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
    # xiaoO 系统级安全 Skill（保护目录及所有子目录/文件）
    {"path": "~/.xiaoo/skills/xiaoo-guardian/", "risk_level": "critical", "desc": "xiaoO 安全防护 Skill 目录"},
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

# ==================== 意图偏离关键词对 ====================
# prompt 意图关键词 → a_next 中不应出现的危险关键词
INTENT_DEVIATION_PATTERNS: list[dict] = [
    {
        "intent_keywords": ["读取", "查看", "分析", "read", "analyze", "view", "list", "列出"],
        "dangerous_actions": ["删除", "remove", "delete", "rm ", "格式化", "format"],
        "reason": "原始意图为读取/分析，但执行了删除操作",
    },
    {
        "intent_keywords": ["查询", "搜索", "search", "query", "find"],
        "dangerous_actions": ["修改", "修改", "update", "write", "写入", "alter"],
        "reason": "原始意图为查询/搜索，但执行了修改操作",
    },
]


class LogicRulesChecker:
    """逻辑规则检测器"""

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
        # 1. read_before_write 原则
        rbw_result = self._check_read_before_write(action_history, a_next)
        if rbw_result.hit:
            return rbw_result

        # 2. 意图一致性检测
        intent_result = self._check_intent_consistency(prompt_session, a_next)
        if intent_result.hit:
            return intent_result

        # 3. 敏感路径访问检测
        path_result = self._check_sensitive_path_access(a_next)
        if path_result.hit:
            return path_result

        # 4. 危险操作模式检测
        dangerous_result = self._check_dangerous_patterns(a_next)
        if dangerous_result.hit:
            return dangerous_result

        return LogicRuleResult(hit=False)

    def _check_read_before_write(
        self, action_history: list[dict[str, object]], a_next: dict[str, str]
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

        # 判断是否为写入操作
        is_write = any(kw in action_type or kw in action_detail for kw in WRITE_KEYWORDS)
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
            hist_detail = action.get("action_detail", "").lower() if isinstance(action, dict) else str(action).lower()
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
                # 排除新文件创建：检查 reason 或 action_detail 中是否有创建/新建关键词
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

        for pattern in INTENT_DEVIATION_PATTERNS:
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

        # 判断是否为写入/删除操作
        is_write_op = any(kw in action_type or kw in action_detail for kw in WRITE_KEYWORDS)

        for sp in SENSITIVE_PATHS:
            raw_path = sp["path"]
            path = raw_path.lower()

            # 检查路径是否在操作中出现
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
                # 对于 xiaoo-guardian 保护目录：只拦截写入/删除操作，读取操作放行
                if raw_path.startswith("~") and not is_write_op:
                    continue  # 允许读取操作
                # 对于其他敏感路径，所有访问都拦截（保持原有逻辑）
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

        # 通配符 + 删除/修改
        if ("*" in action_detail or "?" in action_detail) and any(
            kw in action_detail for kw in ["rm", "del", "delete", "remove", "删除", "移除"]
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
