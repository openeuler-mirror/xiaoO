#!/usr/bin/env python3
"""auditagent — AuditAgent 命令行工具

安装后全局可用：auditagent input.json

功能：
  - 读取 input.json 作为工具输入
  - 调用 audit_action 执行审计
  - 命令行打印结果
  - Allow → policy 写入 .toml 文件
  - Deny → 原因写入 deny_reason.txt

用法：
  auditagent input.json                          # 基本用法
  auditagent input.json -o ./output              # 指定输出目录
  auditagent input.json --step-interval 8        # 设置 Step1/Step2 间隔
  auditagent input.json --no-file                # 只打印结果，不写文件
  auditagent --init                              # 在当前目录生成 input.json.example
"""

import argparse
import json
import sys
import time
from pathlib import Path

from audit_policy_checker.main import audit_action
from audit_policy_checker.config import get_default_config
from audit_policy_checker.logging_utils import setup_logger
from audit_policy_checker.policy_cache import get_default_cache
from audit_policy_checker import __version__


# ============================================================
# 输入 JSON 格式示例
# ============================================================

INPUT_EXAMPLE = {
    "session_id": "my-session-001",
    "prompt_session": "帮我读取 /var/log/app.log 文件并分析其中的错误信息",
    "action_history": [],
    "a_next": {
        "action_type": "shell_command",
        "action_detail": "cat /var/log/app.log"
    },
    "reason": "需要读取日志文件内容以进行错误分析"
}


# ============================================================
# 核心逻辑
# ============================================================

def run(input_path: Path, output_dir: Path | None, step_interval: float | None,
        no_file: bool, debug: bool) -> int:
    """读取输入 → 调用审计 → 输出结果 → 返回退出码"""

    # 强制 stdout 行缓冲，确保实时输出
    sys.stdout.reconfigure(line_buffering=True)

    # 1. 读取 input.json
    if not input_path.exists():
        print(f"错误: 输入文件不存在: {input_path}", file=sys.stderr)
        return 2

    try:
        with open(input_path, "r", encoding="utf-8") as f:
            data = json.load(f)
    except json.JSONDecodeError as e:
        print(f"错误: JSON 解析失败: {e}", file=sys.stderr)
        return 2

    # 2. 校验必填字段
    # 支持两种格式：
    #   a) 扁平格式：{"session_id": "...", "prompt_session": "...", ...}
    #   b) 测试用例格式：{"id": "...", "input": {"session_id": "...", ...}, "expected_decision": "..."}
    if "input" in data and isinstance(data["input"], dict):
        data = data["input"]

    required_fields = ["session_id", "prompt_session", "a_next", "reason"]
    missing = [f for f in required_fields if f not in data]
    if missing:
        print(f"错误: 输入缺少必填字段: {', '.join(missing)}", file=sys.stderr)
        print(f"  必填字段: session_id, prompt_session, a_next, reason", file=sys.stderr)
        print(f"  可选字段: action_history (默认为 [])", file=sys.stderr)
        return 2

    # 补全默认值
    data.setdefault("action_history", [])

    # 3. 配置
    setup_logger("DEBUG" if debug else "WARNING")

    if step_interval is not None:
        get_default_config().timeout.step_interval = step_interval

    # 清空缓存
    get_default_cache().clear()

    # 4. 打印输入摘要
    print("=" * 60, flush=True)
    print("AuditAgent — Agent 动作审计工具", flush=True)
    print("=" * 60, flush=True)
    print(f"  会话 ID:     {data['session_id']}", flush=True)
    print(f"  用户 Prompt: {data['prompt_session']}", flush=True)
    a_next = data["a_next"]
    print(f"  动作类型:    {a_next.get('action_type', 'unknown')}", flush=True)
    print(f"  动作详情:    {a_next.get('action_detail', '')}", flush=True)
    print(f"  执行理由:    {data['reason']}", flush=True)
    print(f"  历史动作数:   {len(data['action_history'])}", flush=True)
    print("-" * 60, flush=True)

    # 5. 调用审计
    print("正在审计中，请稍候...", flush=True)
    start = time.monotonic()
    result = audit_action(
        session_id=data["session_id"],
        prompt_session=data["prompt_session"],
        action_history=data["action_history"],
        a_next=data["a_next"],
        reason=data["reason"],
    )
    elapsed = time.monotonic() - start

    # 6. 打印结果
    decision = result["decision"]
    is_allow = decision == "Allow"

    if is_allow:
        status_icon = "\033[92m✔ ALLOW\033[0m"
    else:
        status_icon = "\033[91m✘ DENY\033[0m"

    print(f"\n  审计决策:   {status_icon}", flush=True)
    # 审计理由逐行缩进打印
    reason_text = result["reason"]
    reason_lines = reason_text.split("\n")
    print(f"  审计理由:", flush=True)
    for line in reason_lines:
        print(f"    {line}", flush=True)
    if result.get("violated_policy"):
        print(f"  违反策略:   {result['violated_policy']}", flush=True)
    if result.get("violated_layers"):
        layers_display = ", ".join(result["violated_layers"])
        print(f"  违反层级:   [{layers_display}]", flush=True)
    print(f"  耗时:       {elapsed:.2f}s", flush=True)

    # 7. 写入文件
    if not no_file:
        # 确定输出目录
        if output_dir is None:
            output_dir = input_path.parent
        output_dir = Path(output_dir)
        output_dir.mkdir(parents=True, exist_ok=True)

        sid = data["session_id"].replace("/", "_").replace("\\", "_")

        if is_allow:
            # Allow → policy.toml
            toml_path = output_dir / f"{sid}_policy.toml"
            toml_path.write_text(result["policy"], encoding="utf-8")
            print(f"\n  ✔ Policy 已写入: {toml_path}", flush=True)
        else:
            # Deny → deny_reason.txt
            deny_path = output_dir / f"{sid}_deny_reason.txt"
            # 审计理由逐行格式化
            analysis_lines = result["reason"].split("\n")
            analysis_formatted = "\n".join(f"  {line}" for line in analysis_lines)
            content = (
                f"Decision: Deny\n"
                f"Session ID: {data['session_id']}\n"
                f"Prompt: {data['prompt_session']}\n"
                f"Action: {a_next.get('action_type', 'unknown')} — {a_next.get('action_detail', '')}\n"
                f"Reason for action: {data['reason']}\n"
                f"\n"
                f"Violated Layers: {', '.join(result.get('violated_layers', []))}\n"
                f"Violation: {result['violated_policy']}\n"
                f"\n"
                f"Analysis:\n{analysis_formatted}\n"
            )
            deny_path.write_text(content, encoding="utf-8")
            print(f"\n  ✘ Deny 理由已写入: {deny_path}", flush=True)

    # 8. 返回 JSON 结果到 stdout（便于管道处理）
    print("\n" + "=" * 60, flush=True)
    json_result = json.dumps(result, ensure_ascii=False, indent=2)
    print(json_result, flush=True)

    # 退出码：Allow=0, Deny=1
    return 0 if is_allow else 1


def init_example():
    """在当前目录生成 input.json.example"""
    path = Path("input.json.example")
    if path.exists():
        print(f"文件已存在: {path}")
        return
    with open(path, "w", encoding="utf-8") as f:
        json.dump(INPUT_EXAMPLE, f, ensure_ascii=False, indent=4)
        f.write("\n")
    print(f"已生成: {path}")
    print("编辑后可通过 auditagent input.json.example 运行")


def main():
    parser = argparse.ArgumentParser(
        prog="auditagent",
        description="AuditAgent — Agent 动作审计命令行工具",
    )
    parser.add_argument(
        "input",
        nargs="?",
        type=Path,
        help="输入 JSON 文件路径（包含 session_id, prompt_session, a_next, reason 等字段）",
    )
    parser.add_argument(
        "-o", "--output-dir",
        type=Path,
        default=None,
        help="输出目录（默认与输入文件同目录）",
    )
    parser.add_argument(
        "--step-interval",
        type=float,
        default=None,
        help="Step1 与 Step2 之间的间隔秒数（默认读取 config.json）",
    )
    parser.add_argument(
        "--no-file",
        action="store_true",
        help="不写入文件，只在命令行输出结果",
    )
    parser.add_argument(
        "--init",
        action="store_true",
        help="在当前目录生成 input.json.example 模板",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="显示详细调试日志",
    )
    parser.add_argument(
        "-v", "--version",
        action="version",
        version=f"auditagent {__version__}",
    )

    args = parser.parse_args()

    if args.init:
        init_example()
        return 0

    if args.input is None:
        parser.print_help()
        print("\n示例:")
        print("  auditagent input.json                  # 审计动作")
        print("  auditagent input.json -o ./results     # 指定输出目录")
        print("  auditagent --init                      # 生成输入模板")
        print("  auditagent input.json --step-interval 10  # 设置Step间隔")
        return 0

    return run(
        input_path=args.input,
        output_dir=args.output_dir,
        step_interval=args.step_interval,
        no_file=args.no_file,
        debug=args.debug,
    )


if __name__ == "__main__":
    sys.exit(main())
