#!/usr/bin/env bash
# scripts/verify-p1-rate-limit.sh
#
# P1「全局并发/速率闸门」（llm_rate_limit_resilience_plan.md）的手动验证脚本：
# 编译 + 格式 + lint + 相关单测。只读验证、不改代码，供实现后人工执行。
#
# 覆盖项（全部针对本次触及的 crate，非全量 workspace，控制耗时）：
#   1. cargo fmt --all --check     —— 全 workspace 格式（含 #[path] 接入的测试文件）
#   2. cargo clippy                —— llm-client / xiaoo-api / xiaoo-serverside
#   3. cargo test -p llm-client    —— 含新增 rate_limit 模块与既有回归（mockito 需本地端口）
#   4. cargo test -p xiaoo-serverside daemon_config —— 配置解析与装载侧效应
#   5. 门禁脚本                    —— check-facade.sh / check-tests-hygiene.sh
#                                      （本次改动涉及 xiaoo-api pub 面与 src 内 cfg(test) 纪律）
#
# FAIL 行前缀与仓库其余 check-*.sh 一致，便于人眼对位：
#   [META] 工具缺失或命令自身失败（非测试失败）——硬失败
#   [TEST] 编译/格式/lint/测试未通过
#
# 退出码：0 全部通过；1 存在未通过项。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

log() { printf '%s\n' "$*" >&2; }

FAILED=0

run_step() {
    local name="$1"
    shift
    ci_title "$name"
    if "$@"; then
        ci_result "$name" 0
    else
        ci_result "$name" "$?"
        FAILED=1
    fi
}

run_step "cargo fmt --all --check" \
    cargo fmt --all --check

run_step "cargo clippy (llm-client / xiaoo-api / xiaoo-serverside)" \
    cargo clippy -p llm-client -p xiaoo-api -p xiaoo-serverside --all-targets -- -D warnings

run_step "cargo test -p llm-client (含 rate_limit 模块)" \
    cargo test -p llm-client

run_step "cargo test -p xiaoo-serverside (daemon_config)" \
    cargo test -p xiaoo-serverside --bin xiaoo-daemon daemon_config

run_step "check-facade.sh (xiaoo-api pub 面与 apps 依赖纪律)" \
    "$SCRIPT_DIR/check-facade.sh"

run_step "check-tests-hygiene.sh (测试收归 tests/ 纪律)" \
    "$SCRIPT_DIR/check-tests-hygiene.sh"

if [ "$FAILED" -ne 0 ]; then
    log "FAIL [TEST]: 存在未通过项，见上方 [FAIL] 标记"
    exit 1
fi
log "P1 闸门验证全部通过"
