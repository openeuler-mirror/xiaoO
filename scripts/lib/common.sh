#!/usr/bin/env bash
# shellcheck shell=bash
# scripts/lib/common.sh
#
# CI 脚本共用工具。由 scripts/ci.sh 与 scripts/check-*.sh source 引用。
# 不直接执行；本文件被 source 时不重复 set 配置，交由调用方负责 set -euo pipefail。
# 环境变量（如 LC_ALL）由此处统一 export，source 后即生效并向子进程继承。

# 统一 locale：保证 sort/comm 等文本比较跨环境一致；export 让子进程继承。
# 用 C（非 C.UTF-8）以最大化跨环境可用性；下方边框/缩进字符均用 ASCII，
# 故 C locale 下也无乱码风险。
export LC_ALL=C

# 缩进与列表项前缀常量：CI 输出里所有"详情块正文"用 INDENT（2 空格），
# 所有"失败明细列表项"用 LIST_PREFIX（8 空格 + "- "）。集中定义避免各处
# 硬编码空格数不一致。
INDENT='  '
LIST_PREFIX='        - '

# 切到仓库根（脚本可在任意目录调用）。
# 复用：调用方若已 cd 到仓库根则幂等。
root_dir() {
    local d="${BASH_SOURCE[0]:-$0}"
    local dir
    dir="$(cd "$(dirname "$d")" && pwd)"
    # scripts/lib/common.sh -> ../../  = 仓库根
    echo "$(cd "$dir/../.." && pwd)"
}

# 打印带框的 stage 标题行。边框用 ASCII '='，配合 LC_ALL=C 在任何终端都不会乱码。
ci_title() {
    printf '\n================================================================\n  %s\n================================================================\n' "$*" >&2
}

# 打印带框的小节标题（比 ci_title 更轻，用于详情块标题）。
ci_section() {
    printf '\n--- %s ---\n' "$*" >&2
}

# 打印步骤结果行。$1=step 名，$2=exit code。
ci_result() {
    local step="$1" code="$2"
    if [ "$code" -eq 0 ]; then
        printf '  [PASS] %s\n' "$step" >&2
    else
        printf '  [FAIL] %s (exit %s)\n' "$step" "$code" >&2
    fi
}

# 从一段输出里抽取"首条 FAIL 行"作为该步骤的一句话原因摘要。
# 输入：stdin。输出：stdout（去除 "FAIL [TAG]: " 前缀的人话部分）。
# 找不到 FAIL 行则回退首条非空行。
#
# 单次遍历：边读边记录首个非空行作为回退，遇到 FAIL 行立即返回。
# （旧实现先扫 FAIL 再单独扫非空行，第二段循环因 stdin 已被消费而永远进不去。）
extract_reason() {
    local line stripped first_nonempty=""
    while IFS= read -r line || [ -n "$line" ]; do
        # 去前导空白（POSIX 字符类）。
        stripped="${line#"${line%%[![:space:]]*}"}"
        # 记录首条非空行作为回退（仅一次）。
        if [ -z "$first_nonempty" ] && [ -n "$stripped" ]; then
            first_nonempty="$stripped"
        fi
        case "$stripped" in
            FAIL[[:space:]]*|FAIL:*)
                # 去掉 "FAIL [TAG]: " 前缀，保留人话。
                # 期望子脚本格式：`FAIL [TAG]: message`；\[[^]]*\] 限定只剥首段 [TAG]，
                # 兼容无 [TAG] 的 `FAIL: message` 与多空格变体。
                # 冒号 `[:-]?` 设为可选：兼容 `FAIL message`（无冒号）这种写法，
                # 否则前缀剥不掉、原因行会残留 "FAIL " 字样。
                stripped="$(printf '%s' "$stripped" \
                    | sed -E 's/^FAIL[[:space:]]*(\[[^]]*\][[:space:]]*)?[:-]?[[:space:]]*//')"
                printf '%s\n' "$stripped"
                return 0
                ;;
        esac
    done
    # 回退：首条非空行；都没有则占位。
    if [ -n "$first_nonempty" ]; then
        printf '%s\n' "$first_nonempty"
    else
        printf '%s\n' "(无详细输出)"
    fi
}

# 打印一段文本，每行加 INDENT 前缀缩进，作为详情块正文。
print_indented() {
    local line
    while IFS= read -r line || [ -n "$line" ]; do
        printf '%s%s\n' "$INDENT" "$line" >&2
    done
}
