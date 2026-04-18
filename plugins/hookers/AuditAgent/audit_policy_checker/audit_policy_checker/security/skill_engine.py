"""Skill 引擎 — 加载、匹配和注入安全检测 Skill

管理安全检测 Skill 规则的生命周期：
- 加载 skills/ 目录下的所有 .md 规则文件
- 根据动作类型和内容匹配最相关的 Skill
- 格式化后注入 LLM prompt 指导安全判断
"""

import re
from pathlib import Path

from .types import SkillMatch

# Skills 目录路径
DEFAULT_SKILLS_DIR = Path(__file__).parent.parent / "skills"

# ==================== Skill 分类映射 ====================
# action_type / action_detail 中的关键词 → 匹配的 skill 文件名
SKILL_KEYWORD_MAP: dict[str, list[str]] = {
    "file_access_guard": [
        "file", "文件", "path", "路径", "read", "write", "cat", "open",
        "access", "访问", "读写", "copy", "cp", "mv", "move",
    ],
    "script_execution_guard": [
        "command", "shell", "bash", "exec", "execute", "执行", "命令",
        "script", "脚本", "run", "运行", "subprocess", "system",
        "eval", "compile", "安装", "install", "pip", "npm",
    ],
    "data_exfiltration_guard": [
        "upload", "上传", "send", "发送", "transfer", "传输", "export", "导出",
        "download", "下载", "post", "http", "request", "fetch", "curl", "wget",
        "外传", "泄露", "exfil", "network", "网络",
    ],
    "browser_web_access_guard": [
        "browser", "浏览器", "web", "网页", "url", "navigate", "浏览",
        "scrape", "crawl", "爬取", "click", "screenshot",
    ],
    "email_operation_guard": [
        "email", "邮件", "mail", "smtp", "send_email", "发送邮件",
        "inbox", "收件箱", "附件", "attachment",
    ],
    "general_tool_risk_guard": [
        "tool", "工具", "api", "invoke", "调用", "function", "函数",
    ],
    "lateral_movement_guard": [
        "ssh", "scp", "ftp", "sftp", "rdp", "vnc", "telnet",
        "内网", "横向", "tunnel", "代理", "proxy", "port", "端口",
        "scan", "扫描", "ping", "traceroute", "nslookup",
    ],
    "persistence_backdoor_guard": [
        "cron", "crontab", "定时", "scheduled", "startup", "启动项",
        "service", "服务", "daemon", "守护", "registry", "注册表",
        "backdoor", "后门", "persist", "持久化", "authorized_keys",
    ],
    "resource_exhaustion_guard": [
        "fork", "bomb", "炸弹", "memory", "内存", "disk", "磁盘",
        "cpu", "process", "进程", "dd", "while true", "无限循环",
        "耗尽", "exhaust", "资源",
    ],
    "skill_installation_guard": [
        "skill", "plugin", "插件", "extension", "扩展", "install skill",
        "安装插件", "安装技能", "marketplace", "商店",
    ],
    "supply_chain_guard": [
        "dependency", "依赖", "package", "包", "npm install", "pip install",
        "requirements", "import", "library", "库", "registry", "仓库",
        "supply", "供应链", "typosquatting",
    ],
    "intent_deviation_guard": [
        "rm", "del", "delete", "remove", "删除", "format", "格式化",
        "drop", "truncate", "清空", "wipe", "擦除", "重置", "reset",
        "sudo", "root", "admin", "管理员", "privilege", "提权",
    ],
}


class SkillEngine:
    """Skill 引擎 — 加载、匹配和注入安全检测 Skill"""

    def __init__(self, skills_dir: str | Path | None = None):
        self._skills_dir = Path(skills_dir) if skills_dir else DEFAULT_SKILLS_DIR
        self._skills_cache: dict[str, str] = {}
        self._load_all_skills()

    def _load_all_skills(self) -> None:
        """加载所有 Skill 文件到缓存"""
        if not self._skills_dir.exists():
            return
        for md_file in sorted(self._skills_dir.glob("*.md")):
            skill_name = md_file.stem
            self._skills_cache[skill_name] = md_file.read_text(encoding="utf-8")

    def match_skills(
        self,
        a_next: dict[str, str],
        reason: str,
        max_skills: int = 3,
    ) -> list[SkillMatch]:
        """
        根据 a_next 和 reason 匹配最相关的 Skill。

        Args:
            a_next: 下一步动作
            reason: 执行理由
            max_skills: 最多返回的 Skill 数量

        Returns:
            list[SkillMatch]: 匹配到的 Skill 列表，按相关度降序
        """
        action_type = a_next.get("action_type", "").lower()
        action_detail = a_next.get("action_detail", "").lower()
        combined = f"{action_type} {action_detail} {reason}".lower()

        # 计算每个 skill 的相关度
        scores: dict[str, float] = {}
        for skill_name, keywords in SKILL_KEYWORD_MAP.items():
            score = 0.0
            for kw in keywords:
                if kw in combined:
                    # action_type 精确匹配权重更高
                    if kw == action_type:
                        score += 2.0
                    elif kw in action_detail:
                        score += 1.5
                    elif kw in reason.lower():
                        score += 1.0
                    else:
                        score += 0.5
            if score > 0:
                scores[skill_name] = score

        # 排序并取 top-N
        sorted_skills = sorted(scores.items(), key=lambda x: x[1], reverse=True)
        top_skills = sorted_skills[:max_skills]

        # 归一化相关度到 0-1
        max_score = top_skills[0][1] if top_skills else 1.0

        results = []
        for skill_name, score in top_skills:
            content = self._skills_cache.get(skill_name, "")
            if content:
                results.append(
                    SkillMatch(
                        skill_name=skill_name,
                        relevance=min(score / max_score, 1.0),
                        skill_content=content,
                    )
                )

        # 如果没有匹配到任何 skill，加载 general_tool_risk_guard 作为兜底
        if not results:
            fallback = self._skills_cache.get("general_tool_risk_guard", "")
            if fallback:
                results.append(
                    SkillMatch(
                        skill_name="general_tool_risk_guard",
                        relevance=0.3,
                        skill_content=fallback,
                    )
                )

        return results

    def format_skills_for_prompt(self, skills: list[SkillMatch]) -> str:
        """
        将匹配到的 Skill 格式化为 LLM prompt 可用的文本。

        Args:
            skills: 匹配到的 Skill 列表

        Returns:
            str: 格式化后的 Skill 规则文本
        """
        if not skills:
            return "（未匹配到特定安全检测规则）"

        sections = []
        for skill in skills:
            sections.append(
                f"### 安全检测规则: {skill.skill_name} (相关度: {skill.relevance:.0%})\n\n"
                f"{skill.skill_content}"
            )

        return "\n\n---\n\n".join(sections)

    @property
    def available_skills(self) -> list[str]:
        """获取所有已加载的 Skill 名称"""
        return list(self._skills_cache.keys())

    def reload_skills(self) -> None:
        """重新加载所有 Skill 文件"""
        self._skills_cache.clear()
        self._load_all_skills()
