"""主入口函数 — 串联 Step 1-2 的完整审计流程

Step 1: xiaoO Audit Agent 安全判断（三层防御）
  ├── 1.1 启发式静态检测（关键命令 + 注入检测 + 用户敏感规则）
  ├── 1.2 逻辑规则检测（read_before_write + 意图一致性 + 敏感路径 + 危险模式）
  └── 1.3 LLM + Skill 深度分析
  → 如果 Deny → 直接返回 Deny + 原因（结束）

Step 2: Allow → 查缓存 / 生成 policy
  ├── 缓存命中 → 使用缓存 policy
  └── 缓存未命中 → LLM 生成 policy + 写入缓存
  → 返回 Allow + policy

优化：白名单只读工具（whitelist_bypass）跳过 Step 2 LLM 调用，使用预定义的最小权限 Policy。
"""

import os
import tempfile
import time
from pathlib import Path

from .config import Config, get_default_config
from .llm_client import call_llm
from .parsers import parse_policy_from_llm
from .policy_cache import PolicyCache, get_default_cache
from .prompt_templates import get_prompt1
from .logging_utils import AuditLogger
from .security import judge_security

# Policy 临时文件输出目录
POLICY_OUTPUT_DIR = Path(tempfile.gettempdir()) / "audit_policy_checker"

# 白名单只读工具的默认最小权限 Policy（与 xiaoO Policy.minimal() 保持一致）
# 参考: crates/cerberus/cerberus-core/src/policy/policy.rs
DEFAULT_READONLY_POLICY = """landlock_optional = false
mount_isolation_fallback = false

[path_groups]
system_binaries = true
system_libraries = true
temp_directories = true
device_files = false
proc_filesystem = false
network_config = false
wsl_paths = false

[namespaces]
mount = true
pid = false
network = true
user = false

[resources]
timeout_secs = 30
max_memory_bytes = 268435456

[environment]
whitelist = ["PATH", "LANG", "HOME", "USER", "HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"]
"""


def audit_action(
    session_id: str,
    prompt_session: str,
    action_history: list[dict[str, object]],
    a_next: dict[str, str],
    reason: str,
    config: Config | None = None,
    cache: PolicyCache | None = None,
) -> dict[str, str]:
    """
    审计 agent 的下一步动作是否在安全范围内，并在允许时生成最小 policy。

    Args:
        session_id: 会话唯一标识
        prompt_session: 用户输入的原始 prompt
        action_history: 历史动作序列，格式为 [{"name": "a1"}, {"name": "a2"}, ...]
        a_next: 下一步动作，含 action_type 和 action_detail
        reason: 执行该动作的理由
        config: 配置项，为 None 时使用默认配置
        cache: Policy 缓存实例，为 None 时使用默认缓存

    Returns:
        dict: {"decision": "Allow"|"Deny", "policy": "...", "reason": "...", "violated_policy": "..."}
    """
    cfg = config or get_default_config()
    audit_log = AuditLogger(session_id)
    max_retries = cfg.retry.max_retries
    retry_interval = cfg.retry.retry_interval

    # 记录输入
    audit_log.log_input(prompt_session, action_history, a_next, reason)

    try:
        # ==================== Step 1: xiaoO Audit Agent 安全判断 ====================
        audit_log.start_step("step1_security_judge")
        judgment = judge_security(
            prompt_session=prompt_session,
            action_history=action_history,
            a_next=a_next,
            reason=reason,
            config=cfg,
        )

        if not judgment.allowed:
            # Deny → 直接返回
            layers_str = ", ".join(judgment.violated_layers) if judgment.violated_layers else judgment.source
            audit_log.end_step(
                "step1_security_judge",
                detail=f"allowed=False, risk_level={judgment.risk_level}, violated_layers=[{layers_str}]",
            )
            result = {
                "decision": "Deny",
                "policy": "",
                "reason": judgment.reason,
                "violated_policy": f"[{judgment.risk_type}] {judgment.reason}",
                "violated_layers": judgment.violated_layers,
            }
            audit_log.log_result("Deny", result["reason"])
            return result

        audit_log.end_step(
            "step1_security_judge",
            detail=f"allowed=True, risk_level={judgment.risk_level}, source={judgment.source}",
        )

        # ==================== Step 2: Allow → 查缓存 / 生成 policy ====================
        policy_cache = cache or get_default_cache()

        # 检查是否启用 LLM Policy 生成（默认关闭，通过环境变量开启）
        enable_policy_gen = os.environ.get("AUDIT_ENABLE_POLICY_GEN", "") == "1"

        audit_log.start_step("step2_cache_lookup")
        cached_policy = policy_cache.get(session_id, prompt_session)

        # 使用缓存或生成 policy
        if cached_policy is not None:
            audit_log.end_step("step2_cache_lookup", detail="缓存命中")
            policy_a_next = cached_policy
            _ = _save_policy_to_file(session_id, policy_a_next)
        elif not enable_policy_gen:
            # Policy 生成已禁用，使用默认 Policy
            audit_log.end_step("step2_cache_lookup", detail="缓存未命中，Policy 生成已禁用")
            audit_log.start_step("step2_default_policy")
            policy_a_next = DEFAULT_READONLY_POLICY
            policy_cache.put(session_id, prompt_session, policy_a_next)
            _ = _save_policy_to_file(session_id, policy_a_next)
            audit_log.end_step("step2_default_policy", detail="使用默认最小权限 Policy")
            result = {
                "decision": "Allow",
                "policy": policy_a_next,
                "reason": judgment.reason,
                "violated_policy": "",
            }
            audit_log.log_result("Allow", result["reason"], policy_a_next)
            return result
        else:
            audit_log.end_step("step2_cache_lookup", detail="缓存未命中，启用 LLM 生成")

            # 白名单只读工具：跳过 LLM 调用，使用预定义的最小权限 Policy
            if judgment.source == "whitelist_bypass":
                audit_log.start_step("step2_whitelist_policy")
                policy_a_next = DEFAULT_READONLY_POLICY
                # 写入缓存供后续使用
                policy_cache.put(session_id, prompt_session, policy_a_next)
                _ = _save_policy_to_file(session_id, policy_a_next)
                audit_log.end_step("step2_whitelist_policy", detail="白名单工具使用预定义最小权限 Policy")
                result = {
                    "decision": "Allow",
                    "policy": policy_a_next,
                    "reason": judgment.reason,
                    "violated_policy": "",
                }
                audit_log.log_result("Allow", result["reason"], policy_a_next)
                return result

            # Step1 与 Step2 之间的间隔（避免 API 限流）
            if cfg.timeout.step_interval > 0:
                time.sleep(cfg.timeout.step_interval)

            # LLM 生成 policy（复用原 Step1 逻辑）
            audit_log.start_step("step2_generate_policy")
            prompt1_text = get_prompt1(prompt_session)
            step2_error: Exception | None = None
            policy_a_next = None

            for attempt in range(1, max_retries + 1):
                try:
                    llm_response = call_llm(
                        prompt=prompt1_text,
                        timeout=cfg.timeout.prompt1_timeout,
                        config=cfg.llm,
                    )
                    policy_a_next = parse_policy_from_llm(llm_response)
                    audit_log.log_llm_interaction(
                        "step2_generate_policy",
                        prompt1_text[:200],
                        llm_response[:200],
                    )
                    # 缓存生成的 policy
                    policy_cache.put(session_id, prompt_session, policy_a_next)
                    _ = _save_policy_to_file(session_id, policy_a_next)
                    audit_log.end_step("step2_generate_policy", detail="Policy 生成并缓存成功")
                    step2_error = None
                    break
                except (TimeoutError, ValueError, Exception) as e:
                    step2_error = e
                    if attempt < max_retries:
                        audit_log.log_error("step2_generate_policy", e)
                        audit_log.start_step(f"step2_retry_{attempt}")
                        time.sleep(retry_interval)
                        audit_log.end_step(
                            f"step2_retry_{attempt}",
                            detail=f"第 {attempt} 次重试，等待 {retry_interval}s",
                        )
                    else:
                        audit_log.log_error("step2_generate_policy", e)

            if step2_error is not None or policy_a_next is None:
                # Policy 生成失败，虽然安全检测通过，但出于安全考虑仍 Deny
                if isinstance(step2_error, TimeoutError):
                    audit_log.end_step(
                        "step2_generate_policy",
                        detail=f"超时（已重试 {max_retries} 次）",
                    )
                    result = {
                        "decision": "Deny",
                        "policy": "",
                        "reason": f"安全检测通过，但策略生成超时（已重试 {max_retries} 次），出于安全考虑拒绝执行",
                        "violated_policy": "无法生成策略：LLM 调用超时",
                    }
                elif isinstance(step2_error, ValueError):
                    audit_log.end_step(
                        "step2_generate_policy",
                        detail=f"解析失败（已重试 {max_retries} 次）: {step2_error}",
                    )
                    result = {
                        "decision": "Deny",
                        "policy": "",
                        "reason": f"安全检测通过，但策略生成失败（已重试 {max_retries} 次）：{step2_error}",
                        "violated_policy": "生成的策略格式无效",
                    }
                else:
                    audit_log.end_step(
                        "step2_generate_policy",
                        detail=f"异常（已重试 {max_retries} 次）: {step2_error}",
                    )
                    result = {
                        "decision": "Deny",
                        "policy": "",
                        "reason": f"安全检测通过，但策略生成服务异常（已重试 {max_retries} 次）：{step2_error}",
                        "violated_policy": "无法生成策略：LLM 服务异常",
                    }
                audit_log.log_result("Deny", result["reason"])
                return result

        # Allow + policy
        result = {
            "decision": "Allow",
            "policy": policy_a_next,
            "reason": judgment.reason,
            "violated_policy": "",
        }
        audit_log.log_result("Allow", result["reason"], policy_a_next)
        return result

    except Exception as e:
        # 兜底异常处理：fail-closed
        audit_log.log_error("unknown", e)
        result = {
            "decision": "Deny",
            "policy": "",
            "reason": f"审计过程发生未预期异常：{e}",
            "violated_policy": "未知异常",
        }
        audit_log.log_result("Deny", result["reason"])
        return result


def _save_policy_to_file(session_id: str, policy_toml: str) -> Path:
    """
    将生成的 policy 写入本地临时文件。

    文件路径: /tmp/audit_policy_checker/{session_id}.toml

    Args:
        session_id: 会话唯一标识
        policy_toml: TOML 格式的 policy 字符串

    Returns:
        Path: 写入的文件路径
    """
    POLICY_OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    # 替换 session_id 中的路径分隔符，避免创建子目录
    safe_name = session_id.replace("/", "_").replace("\\", "_")
    filepath = POLICY_OUTPUT_DIR / f"{safe_name}.toml"
    _ = filepath.write_text(policy_toml, encoding="utf-8")
    return filepath
