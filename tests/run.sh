#!/usr/bin/env bash
# tests/run.sh
#
# 全仓库统一测试入口：cargo test --workspace（单元 + 系统 + 文档测试全量）。
#
# 用法：
#   bash tests/run.sh                    # 跑全部测试
#   bash tests/run.sh -p xiaoo-core      # 额外参数原样透传给 cargo test，聚焦单 crate
#   bash tests/run.sh -- --list          # 仅列出测试不执行（基线核对）
#
# 透传约定：用户已自带目标选择参数（-p / --package / --workspace / --all /
# --manifest-path）时不再强加 --workspace——cargo 会以并集处理，导致 -p 被
# --workspace 吞掉而失去聚焦作用。
#
# 目录约定与命名规则见 tests/README.md。
#
# 退出码：同 cargo test（0 = 全部通过）。

set -euo pipefail

TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# tests/ -> 仓库根
ROOT_DIR="$(cd "$TESTS_DIR/.." && pwd)"

cd "$ROOT_DIR"

# 探测用户是否已指定 cargo 目标选择；未指定时补 --workspace 走全量。
target_selected=0
for arg in "$@"; do
    case "$arg" in
        -p|--package|--workspace|--all|--manifest-path|--exclude)
            target_selected=1 ;;
        --package=*|--manifest-path=*|--exclude=*)
            target_selected=1 ;;
    esac
done
if [ "$target_selected" -eq 0 ]; then
    set -- --workspace "$@"
fi

exec cargo test "$@"
