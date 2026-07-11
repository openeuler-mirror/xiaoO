"""运行时配置管理 — 种子默认值 → 用户本地副本的初始化、加载、合并

核心模型：
  Python 源码中的规则 = "出厂默认"（种子，不可变）
  ~/.config/xiaoo/audit_runtime.json = "用户副本"（可增删改，持久化在本地）

优先级：
  环境变量 > 用户本地 JSON > 源码种子默认值

热生效原理：
  audit-agent 每次 tool call 以独立子进程启动，启动时读取用户本地 JSON。
  修改 JSON 文件 → 下次调用自动生效，无需重启、无需信号通知。
"""

import json
import logging
import os
import re
from pathlib import Path

logger = logging.getLogger(__name__)

# 用户本地配置文件路径
RUNTIME_CONFIG_DIR = Path.home() / ".config" / "xiaoo"
RUNTIME_CONFIG_PATH = RUNTIME_CONFIG_DIR / "audit_runtime.json"

# 环境变量：可指定自定义 runtime 配置文件路径
ENV_RUNTIME_CONFIG_PATH = "AUDIT_RUNTIME_CONFIG_PATH"


# ==================== 种子默认值生成 ====================

def _l1_command_patterns_to_rules() -> list[dict]:
    """将 CRITICAL_COMMAND_PATTERNS + EXTRA_DANGEROUS_PATTERNS 转换为规则列表"""
    from .security.heuristic_detector import CRITICAL_COMMAND_PATTERNS, EXTRA_DANGEROUS_PATTERNS
    rules = []
    for p in CRITICAL_COMMAND_PATTERNS:
        # 从 pattern 中自动生成 id
        id_str = _generate_pattern_id(p["pattern"])
        rules.append({
            "id": id_str,
            "pattern": p["pattern"],
            "risk_level": p["risk_level"],
            "risk_type": p["risk_type"],
            "reason": p["reason"],
            "enabled": True,
            "builtin": True,
        })
    for p in EXTRA_DANGEROUS_PATTERNS:
        id_str = _generate_pattern_id(p["pattern"])
        rules.append({
            "id": id_str,
            "pattern": p["pattern"],
            "risk_level": p["risk_level"],
            "risk_type": p["risk_type"],
            "reason": p["reason"],
            "enabled": True,
            "builtin": True,
        })
    return rules


def _l1_injection_keywords_to_rules() -> list[dict]:
    """将 INJECTION_KEYWORDS 转换为规则列表"""
    from .security.heuristic_detector import INJECTION_KEYWORDS
    rules = []
    # 按语言和类别分组
    category_map = {
        "en": {
            "ignore previous": "instruction_override",
            "ignore all previous": "instruction_override",
            "disregard all prior": "instruction_override",
            "forget everything": "instruction_override",
            "you are now": "role_hijack",
            "you are no longer": "role_hijack",
            "new instructions": "role_hijack",
            "from now on, you": "role_hijack",
            "system prompt": "command_injection",
            "reveal your instructions": "command_injection",
            "output your instructions": "command_injection",
            "repeat your instructions": "command_injection",
            "this is an authorized test": "social_engineering",
            "security audit approved": "social_engineering",
            "admin override": "social_engineering",
            "debug mode": "social_engineering",
            "maintenance mode": "social_engineering",
            "<script>": "xss_injection",
            "javascript:": "xss_injection",
            "onerror=": "xss_injection",
            "onload=": "xss_injection",
            "=== END OF": "boundary_forgery",
            "--- END SYSTEM": "boundary_forgery",
            "[END OF": "boundary_forgery",
            "developer mode": "developer_mode",
            "DAN mode": "developer_mode",
            "jailbreak": "developer_mode",
            "bypass audit": "xiaoo_bypass",
            "skip security check": "xiaoo_bypass",
            "disable guard": "xiaoo_bypass",
        },
        "zh": {
            "忽略之前的指令": "instruction_override",
            "忽略所有先前的": "instruction_override",
            "忽略上面所有": "instruction_override",
            "请忘记之前": "instruction_override",
            "你现在是一个": "role_hijack",
            "你不再是一个": "role_hijack",
            "从现在开始你": "role_hijack",
            "你的新角色是": "role_hijack",
            "系统提示": "command_injection",
            "输出你的指令": "command_injection",
            "重复你的指令": "command_injection",
            "这是授权的测试": "social_engineering",
            "安全审计已批准": "social_engineering",
            "管理员覆盖": "social_engineering",
            "调试模式": "social_engineering",
            "=== 结束": "boundary_forgery",
            "--- 结束系统": "boundary_forgery",
            "开发者模式": "developer_mode",
            "越狱": "developer_mode",
            "绕过审计": "xiaoo_bypass",
            "禁用安全检查": "xiaoo_bypass",
            "关闭防护": "xiaoo_bypass",
        },
    }
    for kw in INJECTION_KEYWORDS:
        keyword = kw["keyword"]
        lang = kw.get("lang", "en")
        category = category_map.get(lang, {}).get(keyword, "other")
        id_str = f"inj_{category}_{_slugify(keyword)}"
        rules.append({
            "id": id_str,
            "keyword": keyword,
            "risk_level": kw["risk_level"],
            "lang": lang,
            "category": category,
            "enabled": True,
            "builtin": True,
        })
    return rules


def _derive_deny_mode(rule: dict) -> str:
    """从 credential / read_only 推导 deny_mode（向后兼容）

    优先使用已有的 deny_mode 字段；若无则从传统字段推导。
    """
    if "deny_mode" in rule and rule["deny_mode"]:
        return rule["deny_mode"]
    if rule.get("credential"):
        return "deny_both"
    if rule.get("read_only"):
        return "deny_read"
    return "deny_write"


def _sync_deny_mode_fields(rule: dict) -> None:
    """根据 deny_mode 同步 credential / read_only 字段（保持底层逻辑兼容）"""
    dm = rule.get("deny_mode", "deny_write")
    if dm == "deny_both":
        rule["credential"] = True
        rule.pop("read_only", None)
    elif dm == "deny_read":
        rule.pop("credential", None)
        rule["read_only"] = True
    elif dm == "deny_write":
        rule.pop("credential", None)
        rule.pop("read_only", None)


DENY_MODE_LABELS = {
    "deny_write": "仅拦截写入",
    "deny_read": "仅拦截读取",
    "deny_both": "读写均拦截",
}


def _l2_sensitive_paths_to_rules() -> list[dict]:
    """将 SENSITIVE_PATHS 转换为规则列表"""
    from .security.logic_rules import SENSITIVE_PATHS
    rules = []
    for sp in SENSITIVE_PATHS:
        id_str = f"path_{_slugify(sp['path'])}"
        deny_mode = _derive_deny_mode(sp)
        rule = {
            "id": id_str,
            "path": sp["path"],
            "risk_level": sp["risk_level"],
            "desc": sp["desc"],
            "deny_mode": deny_mode,
            "source_deny_mode": deny_mode,  # 记录代码仓原始拦截模式
            "enabled": True,
            "builtin": True,
        }
        # 保留传统字段以兼容底层检测逻辑
        _sync_deny_mode_fields(rule)
        rules.append(rule)
    return rules


def _l2_intent_patterns_to_rules() -> list[dict]:
    """将 INTENT_DEVIATION_PATTERNS 转换为规则列表"""
    from .security.logic_rules import INTENT_DEVIATION_PATTERNS
    rules = []
    for i, pat in enumerate(INTENT_DEVIATION_PATTERNS):
        id_str = f"intent_{i+1}_{_slugify(pat['reason'][:30])}"
        rules.append({
            "id": id_str,
            "intent_keywords": pat["intent_keywords"],
            "dangerous_actions": pat["dangerous_actions"],
            "reason": pat["reason"],
            "enabled": True,
            "builtin": True,
        })
    return rules


def _l2_password_patterns_to_rules() -> list[dict]:
    """将 PASSWORD_MODIFY_PATTERNS 转换为规则列表"""
    from .security.logic_rules import PASSWORD_MODIFY_PATTERNS
    rules = []
    for pat in PASSWORD_MODIFY_PATTERNS:
        id_str = f"pw_{_slugify(pat)}"
        rules.append({
            "id": id_str,
            "pattern": pat,
            "enabled": True,
            "builtin": True,
        })
    return rules


def _l2_user_deletion_patterns_to_rules() -> list[dict]:
    """将 USER_DELETION_PATTERNS 转换为规则列表"""
    from .security.logic_rules import USER_DELETION_PATTERNS
    rules = []
    for pat in USER_DELETION_PATTERNS:
        id_str = f"ud_{_slugify(pat)}"
        rules.append({
            "id": id_str,
            "pattern": pat,
            "enabled": True,
            "builtin": True,
        })
    return rules


def _l3_skills_to_rules() -> list[dict]:
    """将 SKILL_KEYWORD_MAP 转换为 skill 规则列表，带分类"""
    from .security.skill_engine import SKILL_KEYWORD_MAP
    # skill 分类定义
    skill_categories = {
        "file_operations": ["file_access_guard", "script_execution_guard"],
        "network_security": [
            "data_exfiltration_guard", "browser_web_access_guard",
            "lateral_movement_guard", "email_operation_guard",
        ],
        "persistence_supply_chain": [
            "persistence_backdoor_guard", "supply_chain_guard",
            "skill_installation_guard",
        ],
        "intent_and_general": [
            "intent_deviation_guard", "general_tool_risk_guard",
        ],
        "resource": ["resource_exhaustion_guard"],
    }

    skills_by_category = {}
    for cat_name, skill_ids in skill_categories.items():
        skills_by_category[cat_name] = {
            "category_enabled": True,
            "skills": [
                {
                    "id": sid,
                    "keywords": SKILL_KEYWORD_MAP.get(sid, []),
                    "enabled": True,
                    "builtin": True,
                }
                for sid in skill_ids
            ],
        }

    # 用户自定义分类
    skills_by_category["user_custom"] = {
        "category_enabled": True,
        "skills": [],
    }

    return skills_by_category


def _generate_pattern_id(pattern: str) -> str:
    """从正则 pattern 生成简短的规则 id"""
    # 取 pattern 的前几个关键字符，生成可辨识的 id
    s = pattern.strip()
    # 去掉常见正则前缀
    s = s.replace("\\b", "").replace("\\s+", "_").replace("\\s", "_")
    s = s.replace(r"\brm", "rm").replace(r"\bchmod", "chmod")
    s = s.replace(r"\bsudo", "sudo")
    # 只保留 ASCII + 数字 + 下划线
    slug = _slugify(s[:40])
    return f"cmd_{slug}"


def _slugify(text: str) -> str:
    """将文本转为 slug 形式（小写、下划线连接）"""
    import re
    # 替换非字母数字为下划线
    s = re.sub(r"[^a-zA-Z0-9一-鿿]", "_", text.lower())
    # 去除连续下划线
    s = re.sub(r"_+", "_", s)
    # 去除首尾下划线
    s = s.strip("_")
    # 限制长度
    return s[:50]


def generate_source_defaults() -> dict:
    """从 Python 源码中的硬编码规则，提取为 JSON 结构（出厂默认种子）"""
    return {
        "version": 1,
        "layers": {
            "L1_heuristic": True,
            "L2_logic_rules": True,
            "L3_llm_analysis": True,
        },
        "L1_rules": {
            "dangerous_commands": {
                "category_enabled": True,
                "category_desc": "危险命令检测（递归删除、全权限设置、提权执行等）",
                "rules": _l1_command_patterns_to_rules(),
            },
            "injection_detection": {
                "category_enabled": True,
                "category_desc": "Prompt 注入攻击检测（指令覆盖、角色劫持、命令注入、社会工程等）",
                "rules": _l1_injection_keywords_to_rules(),
            },
            "user_sensitive_actions": {
                "category_enabled": True,
                "category_desc": "用户自定义敏感动作/工具规则（从 user_rules.json 加载）",
                "rules": [],  # 从 user_rules.json 动态加载，不在此处硬编码
            },
            "user_custom": {
                "category_enabled": True,
                "category_desc": "用户自定义命令规则",
                "rules": [],
            },
        },
        "L2_rules": {
            "sensitive_path_access": {
                "category_enabled": True,
                "category_desc": "敏感路径访问检测（系统关键文件、密钥、凭据等）",
                "rules": _l2_sensitive_paths_to_rules(),
            },
            "intent_consistency": {
                "category_enabled": True,
                "category_desc": "意图一致性检测（动作与原始 prompt 意图偏离）",
                "rules": _l2_intent_patterns_to_rules(),
            },
            "password_modify_consent": {
                "category_enabled": True,
                "category_desc": "非交互式密码修改授权检测",
                "rules": _l2_password_patterns_to_rules(),
            },
            "user_deletion_consent": {
                "category_enabled": True,
                "category_desc": "用户/组删除操作授权检测",
                "rules": _l2_user_deletion_patterns_to_rules(),
            },
            "read_before_write": {
                "category_enabled": True,
                "category_desc": "read_before_write 安全原则检测",
                "rules": [],  # 此规则为逻辑型规则，不需要 pattern 列表
            },
            "dangerous_patterns": {
                "category_enabled": True,
                "category_desc": "危险操作模式检测（通配符滥用、重定向覆盖等）",
                "rules": [],  # 此规则为逻辑型规则，不需要 pattern 列表
            },
        },
        "L3_skills": _l3_skills_to_rules(),
    }


# ==================== 配置加载与合并 ====================

def get_runtime_config_path() -> Path:
    """
    获取 runtime 配置文件路径。

    优先级：环境变量 AUDIT_RUNTIME_CONFIG_PATH > audit_settings.json 中的同名键 > 默认路径
    """
    env_path = os.getenv(ENV_RUNTIME_CONFIG_PATH)
    if env_path:
        return Path(env_path)

    # 回退：从 audit_settings.json 读取（静态配置，运维统一设定）
    from .config import get_audit_setting
    settings_path = get_audit_setting(ENV_RUNTIME_CONFIG_PATH, "")
    if settings_path:
        return Path(settings_path)

    return RUNTIME_CONFIG_PATH


def load_runtime_config() -> dict:
    """
    加载运行时配置。

    流程：
    1. 如果用户本地 JSON 不存在 → 从源码种子生成完整默认副本 → 写入本地
    2. 如果用户已有本地副本 → 加载并合并新版本新增的出厂规则（不覆盖用户开关设置）

    Returns:
        dict: 完整的运行时配置
    """
    config_path = get_runtime_config_path()

    if not config_path.exists():
        # 首次运行 → 从种子生成完整副本
        defaults = generate_source_defaults()
        config_path.parent.mkdir(parents=True, exist_ok=True)
        try:
            config_path.write_text(
                json.dumps(defaults, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
            logger.info("首次运行，已生成默认配置: %s", config_path)
        except OSError as e:
            logger.warning("无法写入配置文件 %s: %s，使用内存默认值", config_path, e)
        return defaults

    # 加载用户本地副本
    try:
        user_config = json.loads(config_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as e:
        logger.warning("配置文件 %s 读取失败: %s，使用默认值", config_path, e)
        return generate_source_defaults()

    # 合并：源码新增的规则补入用户副本，用户的开关设置不受影响
    source_defaults = generate_source_defaults()
    merged = merge_config(user_config, source_defaults)

    # 如果合并产生了变化，写回文件
    if merged != user_config:
        try:
            config_path.write_text(
                json.dumps(merged, indent=2, ensure_ascii=False),
                encoding="utf-8",
            )
            logger.info("配置已合并更新: %s", config_path)
        except OSError as e:
            logger.warning("无法写入合并后的配置: %s", e)

    return merged


def merge_config(user_cfg: dict, source_defaults: dict) -> dict:
    """
    合并用户配置与源码默认值。

    合并策略：
    - 用户已有的 builtin 规则：保留用户的 enabled 状态，不覆盖
    - 源码新增的 builtin 规则（用户副本中没有的）：添加进去，默认 enabled=True
    - 用户自定义规则（builtin=false）：永远保留
    - 层级开关：保留用户设置
    - 分类开关：保留用户设置
    """
    # 版本号
    if "version" not in user_cfg:
        user_cfg["version"] = source_defaults.get("version", 1)

    # 层级开关：保留用户设置，缺失的补默认值
    for layer_key, default_val in source_defaults["layers"].items():
        if layer_key not in user_cfg["layers"]:
            user_cfg["layers"][layer_key] = default_val

    # L1 规则合并
    _merge_rule_categories(user_cfg, source_defaults, "L1_rules")

    # L2 规则合并
    _merge_rule_categories(user_cfg, source_defaults, "L2_rules")

    # L3 Skills 合并
    _merge_skill_categories(user_cfg, source_defaults, "L3_skills")

    return user_cfg


def _merge_rule_categories(user_cfg: dict, source_defaults: dict, layer_key: str) -> None:
    """合并规则分类（L1_rules 或 L2_rules）"""
    user_layer = user_cfg.get(layer_key, {})
    source_layer = source_defaults.get(layer_key, {})

    for cat_name, cat_defaults in source_layer.items():
        if cat_name not in user_layer:
            # 新增的分类 → 整个加入
            user_layer[cat_name] = cat_defaults
        else:
            # 已有分类 → 逐条检查规则
            user_cat = user_layer[cat_name]
            source_cat = cat_defaults

            # 补充缺失的字段
            if "category_desc" not in user_cat and "category_desc" in source_cat:
                user_cat["category_desc"] = source_cat["category_desc"]
            if "category_enabled" not in user_cat:
                user_cat["category_enabled"] = source_cat.get("category_enabled", True)

            # 合并规则列表
            user_rules = user_cat.get("rules", [])
            source_rules = source_cat.get("rules", [])
            user_ids = {r.get("id") for r in user_rules if r.get("id")}

            for sr in source_rules:
                if sr.get("id") and sr["id"] not in user_ids:
                    # 新增的出厂规则 → 加入，默认 enabled=True
                    user_rules.append(sr)
                else:
                    # 已有规则 → 同步源码新增的字段（不影响用户的 enabled 等开关设置）
                    # 例如 sensitive_path_access 规则新增 credential 标记后，老副本需补上
                    ur = next((r for r in user_rules if r.get("id") == sr.get("id")), None)
                    if ur is not None:
                        for field, val in sr.items():
                            if field in ("id", "path", "risk_level", "desc", "builtin"):
                                # 这些字段源码为准（出厂定义），但仅在缺失时补，避免覆盖用户未感知的改动
                                if field not in ur:
                                    ur[field] = val
                            elif field == "deny_mode":
                                # source_deny_mode 以源码为准（代码仓原始值）；deny_mode 保留用户值
                                ur["source_deny_mode"] = val
                                if "deny_mode" not in ur:
                                    ur["deny_mode"] = val
                            elif field == "source_deny_mode":
                                pass  # 由 deny_mode 同步处理
                            elif field == "credential":
                                # credential 标记以源码为准：源码标了就同步 True（读密钥必拦）
                                ur["credential"] = val
                            elif field == "read_only":
                                # read_only 标记以源码为准：源码标了就同步 True（仅拦截读取）
                                ur["read_only"] = val

            # 源码已移除的内置规则 → 禁用（保留在列表中但 enabled=False）
            # 例如 /dev/null、/dev/zero、/dev/urandom 从 SENSITIVE_PATHS 移除后，
            # 老用户副本里还残留，继续拦截会产生误报。禁用而非删除，让用户在
            # dashboard 上能看到这些规则（标注为已禁用），还能手动启用。
            source_ids = {sr.get("id") for sr in source_rules if sr.get("id")}
            for ur in user_rules:
                if ur.get("id") and ur.get("builtin") and ur["id"] not in source_ids and ur.get("enabled", True):
                    ur["enabled"] = False
                    logger.info(
                        "内置规则 %s 已从源码移除，在用户副本中禁用（category=%s）",
                        ur["id"], cat_name,
                    )

            # 敏感路径规则：确保 deny_mode 字段与 credential/read_only 一致
            # 向后兼容：老配置没有 deny_mode，从传统字段推导
            if cat_name == "sensitive_path_access":
                for ur in user_rules:
                    if "deny_mode" not in ur:
                        ur["deny_mode"] = _derive_deny_mode(ur)
                    _sync_deny_mode_fields(ur)

            user_cat["rules"] = user_rules

    user_cfg[layer_key] = user_layer


def _merge_skill_categories(user_cfg: dict, source_defaults: dict, layer_key: str) -> None:
    """合并 skill 分类"""
    user_layer = user_cfg.get(layer_key, {})
    source_layer = source_defaults.get(layer_key, {})

    for cat_name, cat_defaults in source_layer.items():
        if cat_name not in user_layer:
            user_layer[cat_name] = cat_defaults
        else:
            user_cat = user_layer[cat_name]
            source_cat = cat_defaults

            if "category_enabled" not in user_cat:
                user_cat["category_enabled"] = source_cat.get("category_enabled", True)
            if "category_desc" not in user_cat and "category_desc" in source_cat:
                user_cat["category_desc"] = source_cat.get("category_desc", "")

            # 合并 skill 列表
            user_skills = user_cat.get("skills", [])
            source_skills = source_cat.get("skills", [])
            user_ids = {s.get("id") for s in user_skills if s.get("id")}

            for ss in source_skills:
                if ss.get("id") and ss["id"] not in user_ids:
                    user_skills.append(ss)

            user_cat["skills"] = user_skills

    user_cfg[layer_key] = user_layer


# ==================== 层级开关读取（考虑环境变量覆盖） ====================

def is_layer_enabled(layer_key: str) -> bool:
    """
    判断指定层级是否启用。

    优先级：环境变量 > 用户本地 runtime JSON（热更新） > audit_settings.json（静态） > 默认值

    Args:
        layer_key: "L1_heuristic" | "L2_logic_rules" | "L3_llm_analysis"

    Returns:
        bool: 是否启用
    """
    # 层级键 → 对应的"禁用"配置键（环境变量与 audit_settings.json 共用同一键名）
    disable_key_map = {
        "L1_heuristic": "AUDIT_DISABLE_L1",
        "L2_logic_rules": "AUDIT_DISABLE_L2",
        "L3_llm_analysis": "AUDIT_DISABLE_LLM_LAYER3",
    }
    disable_key = disable_key_map.get(layer_key)

    # 1. 环境变量覆盖（最高优先级，紧急运维硬性禁用）
    if disable_key and os.getenv(disable_key) == "1":
        logger.info("层级 %s 被环境变量 %s=1 强制禁用", layer_key, disable_key)
        return False

    # 2. 用户本地 runtime JSON（热更新，可视化平台改的就是这里）
    runtime = load_runtime_config()
    layers = runtime.get("layers", {})
    if layer_key in layers:
        return bool(layers[layer_key])

    # 3. audit_settings.json 静态配置（值="1" 表示禁用，与 L3 的 is_llm_layer3_enabled 语义一致）
    if disable_key:
        # 延迟 import，避免模块加载顺序问题
        from .config import get_audit_setting
        if get_audit_setting(disable_key, "") == "1":
            logger.info("层级 %s 被 audit_settings.json %s=1 禁用", layer_key, disable_key)
            return False

    # 4. 默认启用
    return True


def get_env_overrides() -> dict:
    """获取当前环境变量的覆盖状态"""
    overrides = {}
    env_map = {
        "L1_heuristic": "AUDIT_DISABLE_L1",
        "L2_logic_rules": "AUDIT_DISABLE_L2",
        "L3_llm_analysis": "AUDIT_DISABLE_LLM_LAYER3",
    }
    for layer_key, env_key in env_map.items():
        val = os.getenv(env_key)
        if val is not None:
            overrides[layer_key] = {
                "env_var": env_key,
                "env_value": val,
                "overridden": val == "1",
                "effect": "disabled" if val == "1" else "no_override",
            }
    return overrides


# ==================== 规则过滤 ====================

def filter_enabled_rules(rules: list[dict], category_enabled: bool = True) -> list[dict]:
    """过滤出启用的规则"""
    if not category_enabled:
        return []
    return [r for r in rules if r.get("enabled", True)]


def get_enabled_l1_command_patterns(runtime: dict) -> list[dict]:
    """获取启用的 L1 命令正则模式列表"""
    cat = runtime.get("L1_rules", {}).get("dangerous_commands", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    # 只返回 pattern/risk_level/risk_type/reason，保持与原 CRITICAL_COMMAND_PATTERNS 格式一致
    return [
        {
            "pattern": r["pattern"],
            "risk_level": r["risk_level"],
            "risk_type": r["risk_type"],
            "reason": r["reason"],
        }
        for r in rules
    ]


def get_enabled_l1_injection_keywords(runtime: dict) -> list[dict]:
    """获取启用的 L1 注入关键词列表"""
    cat = runtime.get("L1_rules", {}).get("injection_detection", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    return [
        {
            "keyword": r["keyword"],
            "risk_level": r["risk_level"],
            "lang": r.get("lang", "en"),
        }
        for r in rules
    ]


def get_enabled_l2_sensitive_paths(runtime: dict) -> list[dict]:
    """获取启用的 L2 敏感路径列表"""
    cat = runtime.get("L2_rules", {}).get("sensitive_path_access", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    return [
        {
            "path": r["path"],
            "risk_level": r["risk_level"],
            "desc": r["desc"],
            "deny_mode": r.get("deny_mode", _derive_deny_mode(r)),
            **({"credential": True} if r.get("credential") else {}),
            **({"read_only": True} if r.get("read_only") else {}),
        }
        for r in rules
    ]


def get_enabled_l2_intent_patterns(runtime: dict) -> list[dict]:
    """获取启用的 L2 意图偏离模式列表"""
    cat = runtime.get("L2_rules", {}).get("intent_consistency", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    return [
        {
            "intent_keywords": r["intent_keywords"],
            "dangerous_actions": r["dangerous_actions"],
            "reason": r["reason"],
        }
        for r in rules
    ]


def get_enabled_l2_password_patterns(runtime: dict) -> list[str]:
    """获取启用的 L2 密码修改正则模式列表"""
    cat = runtime.get("L2_rules", {}).get("password_modify_consent", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    return [r["pattern"] for r in rules]


def get_enabled_l2_user_deletion_patterns(runtime: dict) -> list[str]:
    """获取启用的 L2 用户删除正则模式列表"""
    cat = runtime.get("L2_rules", {}).get("user_deletion_consent", {})
    rules = filter_enabled_rules(cat.get("rules", []), cat.get("category_enabled", True))
    return [r["pattern"] for r in rules]


def is_l2_category_enabled(runtime: dict, category_key: str) -> bool:
    """判断 L2 某分类是否启用"""
    cat = runtime.get("L2_rules", {}).get(category_key, {})
    return cat.get("category_enabled", True)


def has_runtime_config() -> bool:
    """判断用户本地 runtime 配置文件是否存在且可正常读取。

    用于区分两种场景：
      - 文件不存在 → 从未通过 Dashboard 配置过，应使用源码默认值
      - 文件存在但规则列表为空 → 用户主动逐条禁用了全部规则，不应回退
    """
    config_path = get_runtime_config_path()
    if not config_path.exists():
        return False
    try:
        json.loads(config_path.read_text(encoding="utf-8"))
        return True
    except (json.JSONDecodeError, OSError):
        return False


def get_enabled_l1_command_patterns_or_default(runtime: dict) -> list[dict]:
    """获取启用的 L1 命令正则模式；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l1_command_patterns(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.heuristic_detector import CRITICAL_COMMAND_PATTERNS, EXTRA_DANGEROUS_PATTERNS
        return CRITICAL_COMMAND_PATTERNS + EXTRA_DANGEROUS_PATTERNS
    return []


def get_enabled_l1_injection_keywords_or_default(runtime: dict) -> list[dict]:
    """获取启用的 L1 注入关键词；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l1_injection_keywords(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.heuristic_detector import INJECTION_KEYWORDS
        return INJECTION_KEYWORDS
    return []


def get_enabled_l2_sensitive_paths_or_default(runtime: dict) -> list[dict]:
    """获取启用的 L2 敏感路径；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l2_sensitive_paths(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.logic_rules import SENSITIVE_PATHS
        return SENSITIVE_PATHS
    return []


def get_enabled_l2_intent_patterns_or_default(runtime: dict) -> list[dict]:
    """获取启用的 L2 意图偏离模式；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l2_intent_patterns(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.logic_rules import INTENT_DEVIATION_PATTERNS
        return INTENT_DEVIATION_PATTERNS
    return []


def get_enabled_l2_password_patterns_or_default(runtime: dict) -> list[str]:
    """获取启用的 L2 密码修改正则模式；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l2_password_patterns(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.logic_rules import PASSWORD_MODIFY_PATTERNS
        return PASSWORD_MODIFY_PATTERNS
    return []


def get_enabled_l2_user_deletion_patterns_or_default(runtime: dict) -> list[str]:
    """获取启用的 L2 用户删除正则模式；仅在无 runtime config 时才回退到硬编码默认值"""
    enabled = get_enabled_l2_user_deletion_patterns(runtime)
    if enabled:
        return enabled
    if not has_runtime_config():
        from .security.logic_rules import USER_DELETION_PATTERNS
        return USER_DELETION_PATTERNS
    return []


def get_disabled_skill_ids(runtime: dict) -> set[str]:
    """获取所有被禁用的 skill id 集合"""
    disabled = set()
    for cat_name, cat_data in runtime.get("L3_skills", {}).items():
        if not cat_data.get("category_enabled", True):
            # 整个分类禁用 → 所有 skill 都不加载
            for s in cat_data.get("skills", []):
                disabled.add(s["id"])
        else:
            # 只禁用单个 skill
            for s in cat_data.get("skills", []):
                if not s.get("enabled", True):
                    disabled.add(s["id"])
    return disabled


# ==================== 配置写入（供 dashboard API 使用） ====================

def save_runtime_config(config: dict) -> None:
    """保存运行时配置到用户本地文件"""
    config_path = get_runtime_config_path()
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(
        json.dumps(config, indent=2, ensure_ascii=False),
        encoding="utf-8",
    )
    logger.info("配置已保存: %s", config_path)


def update_layer_enabled(layer_key: str, enabled: bool) -> dict:
    """更新层级开关并保存"""
    runtime = load_runtime_config()
    runtime["layers"][layer_key] = enabled
    save_runtime_config(runtime)
    return runtime


def update_rule_enabled(layer: str, category: str, rule_id: str, enabled: bool) -> dict:
    """更新单条规则的启用状态并保存"""
    runtime = load_runtime_config()
    rules = runtime.get(layer, {}).get(category, {}).get("rules", [])
    for r in rules:
        if r.get("id") == rule_id:
            r["enabled"] = enabled
            break
    save_runtime_config(runtime)
    return runtime


def update_rule_deny_mode(layer: str, category: str, rule_id: str, deny_mode: str) -> dict:
    """更新单条敏感路径规则的拦截模式并保存"""
    runtime = load_runtime_config()
    rules = runtime.get(layer, {}).get(category, {}).get("rules", [])
    for r in rules:
        if r.get("id") == rule_id:
            r["deny_mode"] = deny_mode
            _sync_deny_mode_fields(r)
            break
    save_runtime_config(runtime)
    return runtime


def update_category_enabled(layer: str, category: str, enabled: bool) -> dict:
    """更新整个分类的启用状态并保存"""
    runtime = load_runtime_config()
    cat = runtime.get(layer, {}).get(category, {})
    cat["category_enabled"] = enabled
    save_runtime_config(runtime)
    return runtime


def add_custom_rule(layer: str, category: str, rule: dict) -> dict:
    """新增自定义规则并保存"""
    runtime = load_runtime_config()
    rules = runtime.get(layer, {}).get(category, {}).get("rules", [])
    rule["builtin"] = False
    rule["enabled"] = True
    # 如果没有 id，自动生成
    if not rule.get("id"):
        import time
        rule["id"] = f"custom_{int(time.time())}_{_slugify(str(rule)[:30])}"
    # 如果指定了 deny_mode，同步 credential/read_only 以兼容底层检测
    if "deny_mode" in rule:
        _sync_deny_mode_fields(rule)
    rules.append(rule)
    save_runtime_config(runtime)
    return runtime


def delete_custom_rule(layer: str, category: str, rule_id: str) -> dict | None:
    """删除自定义规则（builtin=true 的不允许删除）并保存"""
    runtime = load_runtime_config()
    rules = runtime.get(layer, {}).get(category, {}).get("rules", [])
    target = None
    for r in rules:
        if r.get("id") == rule_id:
            if r.get("builtin", True):
                return None  # 不允许删除内置规则
            target = r
            break
    if target:
        rules.remove(target)
        save_runtime_config(runtime)
    return runtime


_SKILL_ID_RE = re.compile(r'^[a-zA-Z0-9_-]+$')


def _validate_skill_id(skill_id: str) -> None:
    """校验 skill_id：只允许字母、数字、下划线、连字符，禁止路径穿越"""
    if not skill_id or not _SKILL_ID_RE.match(skill_id):
        raise ValueError(
            f"skill_id '{skill_id}' 不合法：只允许字母、数字、下划线和连字符（^[a-zA-Z0-9_-]+$），"
            "禁止路径穿越字符（如 ../）"
        )


def add_custom_skill(skill_id: str, category: str, keywords: list[str], content: str) -> dict:
    """新增自定义 skill：写入 markdown 文件 + 更新 runtime JSON

    自定义 skill 文件写入用户目录 ~/.config/xiaoo/audit_skills/，
    而非内置 skills 目录（RPM 安装后内置目录只读，普通用户无法写入）。
    """
    _validate_skill_id(skill_id)
    runtime = load_runtime_config()

    # 写入用户级 skill markdown 文件（可写目录）
    skills_dir = RUNTIME_CONFIG_DIR / "audit_skills"
    skills_dir.mkdir(parents=True, exist_ok=True)
    skill_path = skills_dir / f"{skill_id}.md"
    skill_path.write_text(content, encoding="utf-8")

    # 更新 runtime JSON
    cat_skills = runtime.get("L3_skills", {}).get(category, {}).get("skills", [])
    cat_skills.append({
        "id": skill_id,
        "keywords": keywords,
        "enabled": True,
        "builtin": False,
    })
    save_runtime_config(runtime)
    return runtime


def delete_custom_skill(skill_id: str, category: str) -> dict | None:
    """删除自定义 skill（builtin=true 不允许删除）"""
    _validate_skill_id(skill_id)
    runtime = load_runtime_config()
    cat_skills = runtime.get("L3_skills", {}).get(category, {}).get("skills", [])
    target = None
    for s in cat_skills:
        if s.get("id") == skill_id:
            if s.get("builtin", True):
                return None
            target = s
            break
    if target:
        cat_skills.remove(target)
        # 删除用户级 skill markdown 文件
        skills_dir = RUNTIME_CONFIG_DIR / "audit_skills"
        skill_path = skills_dir / f"{skill_id}.md"
        if skill_path.exists():
            skill_path.unlink()
        save_runtime_config(runtime)
    return runtime


def update_skill_enabled(skill_id: str, enabled: bool) -> dict:
    """更新单个 skill 的启用状态"""
    runtime = load_runtime_config()
    for cat_name, cat_data in runtime.get("L3_skills", {}).items():
        for s in cat_data.get("skills", []):
            if s.get("id") == skill_id:
                s["enabled"] = enabled
                break
    save_runtime_config(runtime)
    return runtime


def update_skill_category_enabled(category: str, enabled: bool) -> dict:
    """更新 skill 分类启用状态"""
    runtime = load_runtime_config()
    cat = runtime.get("L3_skills", {}).get(category, {})
    cat["category_enabled"] = enabled
    save_runtime_config(runtime)
    return runtime
