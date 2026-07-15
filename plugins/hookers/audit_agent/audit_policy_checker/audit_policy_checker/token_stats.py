"""Token 用量统计 — 记录和查询 audit-agent 中每次 LLM 色调用的 token 消耗

统计数据存储在 ~/.config/xiaoo/audit_token_stats.json，每次 LLM 调用后追加一条记录。

数据结构：
  每条记录包含 timestamp, session_id, step, model, prompt_tokens, completion_tokens, total_tokens

查询支持：
  - 按时间范围查询（今天/近7天/近30天/全部）
  - 按 step 分组汇总（L3 安全判断 / Step2 策略生成）
  - 按 model 分组汇总
  - 总计汇总

文件大小控制：
  - 最多保留 10000 条记录（超过时自动裁剪最早的记录）
  - JSON 文件 append-friendly（整体重写而非追加，避免损坏）
"""

import json
import logging
import os
from datetime import datetime, timedelta
from pathlib import Path

logger = logging.getLogger(__name__)

# 统计文件路径（与 runtime config 同目录）
STATS_DIR = Path.home() / ".config" / "xiaoo"
STATS_FILE = STATS_DIR / "audit_token_stats.json"

# 环境变量：可指定自定义统计文件路径
ENV_STATS_PATH = "AUDIT_TOKEN_STATS_PATH"

# 最大记录条数（超过时自动裁剪）
MAX_RECORDS = 10000


def get_stats_path() -> Path:
    """获取统计文件路径（优先环境变量）"""
    env_path = os.getenv(ENV_STATS_PATH)
    if env_path:
        return Path(env_path)
    return STATS_FILE


def record_token_usage(
    session_id: str,
    step: str,
    model: str,
    prompt_tokens: int,
    completion_tokens: int,
    total_tokens: int,
) -> None:
    """
    记录一次 LLM 调用的 token 用量。

    Args:
        session_id: 会话 ID
        step: 调用步骤（如 "L3_security_judge", "step2_policy_gen"）
        model: 实际使用的模型名称
        prompt_tokens: 输入 token 数
        completion_tokens: 输出 token 数
        total_tokens: 总 token 数
    """
    stats_path = get_stats_path()
    record = {
        "timestamp": datetime.now().isoformat(timespec="milliseconds"),
        "session_id": session_id,
        "step": step,
        "model": model,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": total_tokens,
    }

    # 加载现有记录
    records = _load_all_records(stats_path)
    records.append(record)

    # 裁剪超出上限的记录
    if len(records) > MAX_RECORDS:
        records = records[-MAX_RECORDS:]

    # 写回文件
    _save_records(stats_path, records)
    logger.debug(
        "Token usage recorded: step=%s, model=%s, total_tokens=%d",
        step, model, total_tokens,
    )


def _load_all_records(stats_path: Path) -> list[dict]:
    """加载所有统计记录"""
    if not stats_path.exists():
        return []
    try:
        data = json.loads(stats_path.read_text(encoding="utf-8"))
        # 兼容两种格式：纯列表 或 {"records": [...]} 包装
        if isinstance(data, list):
            return data
        if isinstance(data, dict) and "records" in data:
            return data["records"]
        return []
    except (json.JSONDecodeError, OSError) as e:
        logger.warning("Token stats file %s read error: %s", stats_path, e)
        return []


def _save_records(stats_path: Path, records: list[dict]) -> None:
    """保存统计记录到文件"""
    stats_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        stats_path.write_text(
            json.dumps(records, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )
    except OSError as e:
        logger.warning("Token stats file %s write error: %s", stats_path, e)


def get_token_stats(days: int = 0) -> dict:
    """
    获取 token 用量统计汇总。

    Args:
        days: 查询最近多少天的数据。0 表示全部，1 表示今天，7 表示近7天。

    Returns:
        dict: 包含以下字段：
          - total_prompt_tokens: 总输入 token 数
          - total_completion_tokens: 总输出 token 数
          - total_tokens: 总 token 数
          - total_calls: 总 LLM 调用次数
          - by_step: 按 step 分组的汇总
          - by_model: 按 model 分组的汇总
          - by_date: 按日期分组的汇总（最近7天）
          - records: 原始记录列表（可选）
    """
    stats_path = get_stats_path()
    records = _load_all_records(stats_path)

    # 按时间范围过滤
    if days > 0:
        cutoff = datetime.now() - timedelta(days=days)
        filtered = []
        for r in records:
            try:
                ts = datetime.fromisoformat(r["timestamp"])
                if ts >= cutoff:
                    filtered.append(r)
            except (ValueError, KeyError):
                filtered.append(r)  # 时间解析失败，保留
        records = filtered

    # 汇总计算
    total_prompt = sum(r.get("prompt_tokens", 0) for r in records)
    total_completion = sum(r.get("completion_tokens", 0) for r in records)
    total_total = sum(r.get("total_tokens", 0) for r in records)
    total_calls = len(records)

    # 按 step 分组
    by_step: dict[str, dict] = {}
    for r in records:
        step = r.get("step", "unknown")
        entry = by_step.setdefault(step, {
            "prompt_tokens": 0, "completion_tokens": 0,
            "total_tokens": 0, "calls": 0,
        })
        entry["prompt_tokens"] += r.get("prompt_tokens", 0)
        entry["completion_tokens"] += r.get("completion_tokens", 0)
        entry["total_tokens"] += r.get("total_tokens", 0)
        entry["calls"] += 1

    # 按 model 分组
    by_model: dict[str, dict] = {}
    for r in records:
        model = r.get("model", "unknown")
        entry = by_model.setdefault(model, {
            "prompt_tokens": 0, "completion_tokens": 0,
            "total_tokens": 0, "calls": 0,
        })
        entry["prompt_tokens"] += r.get("prompt_tokens", 0)
        entry["completion_tokens"] += r.get("completion_tokens", 0)
        entry["total_tokens"] += r.get("total_tokens", 0)
        entry["calls"] += 1

    # 按日期分组（最近7天）
    by_date: dict[str, dict] = {}
    for r in records:
        try:
            ts = datetime.fromisoformat(r["timestamp"])
            date_key = ts.strftime("%Y-%m-%d")
        except (ValueError, KeyError):
            date_key = "unknown"
        entry = by_date.setdefault(date_key, {
            "prompt_tokens": 0, "completion_tokens": 0,
            "total_tokens": 0, "calls": 0,
        })
        entry["prompt_tokens"] += r.get("prompt_tokens", 0)
        entry["completion_tokens"] += r.get("completion_tokens", 0)
        entry["total_tokens"] += r.get("total_tokens", 0)
        entry["calls"] += 1

    # 排序 by_date 按日期倒序
    sorted_dates = sorted(by_date.items(), key=lambda x: x[0], reverse=True)
    by_date = dict(sorted_dates)

    return {
        "total_prompt_tokens": total_prompt,
        "total_completion_tokens": total_completion,
        "total_tokens": total_total,
        "total_calls": total_calls,
        "by_step": by_step,
        "by_model": by_model,
        "by_date": by_date,
    }


def get_recent_records(limit: int = 20) -> list[dict]:
    """获取最近的 N 条记录"""
    stats_path = get_stats_path()
    records = _load_all_records(stats_path)
    return records[-limit:]


def reset_token_stats() -> None:
    """重置（删除）所有 token 统计记录"""
    stats_path = get_stats_path()
    if stats_path.exists():
        stats_path.unlink()
        logger.info("Token stats reset: %s deleted", stats_path)
