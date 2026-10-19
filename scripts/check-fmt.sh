#!/usr/bin/env bash
# scripts/check-fmt.sh
#
# 代码格式门禁：cargo fmt --check。由 scripts/ci.sh 编排调用；编排行为与
# 步骤表见 ci.sh 头注释。
#
# 纯静态、不编译、秒级。按仓库 rustfmt 配置（当前无 rustfmt.toml，即默认
# 规则，edition 取自各 crate 的 Cargo.toml）检查全部 workspace member 的
# 源码，含经 `#[cfg(test)] #[path]` 接入的仓库根 tests/ 测试文件——确保
# 每次提交的排版都与 `cargo fmt` 一致，格式噪音不混进代码评审。
#
# FAIL 行前缀（便于 ci.sh 抽取原因与读者对位）：
#   [META]  cargo / rustfmt 缺失，或 cargo fmt 自身运行失败（非格式差异）——硬失败
#   [FMT]   文件排版与 cargo fmt 输出不一致（file:line 对位 rustfmt 报告的差异处）
#
# 修复方式：在仓库根运行 `cargo fmt`（只改排版不改语义），不要手工套 diff；
# 修复后本门禁应零改动通过。
#
# 退出码：0 通过；1 有违反或工具异常。违规项逐条打印到 stderr。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

log() { printf '%s\n' "$*" >&2; }

# ---- 工具预检 -----------------------------------------------------------------
# cargo fmt 同时需要 cargo 与 rustfmt（rustup 缺组件、RPM 未装 rustfmt 包时
# 均不可用）。`cargo fmt --version` 两者一并探测，失败即硬失败，防 fail-open。
if ! command -v cargo >/dev/null 2>&1; then
    log "FAIL [META]: 缺少 cargo，无法执行代码格式检查"
    exit 1
fi
if ! cargo fmt --version >/dev/null 2>&1; then
    log "FAIL [META]: cargo fmt 不可用（缺少 rustfmt 组件），无法执行代码格式检查"
    exit 1
fi

# ---- 执行检查 -----------------------------------------------------------------
# --all 覆盖全部 workspace member；--check 只报告不改写；`-- --color never`
# 把颜色参数透传给 rustfmt，输出落日志文件后仍可读。
OUT_FILE="$(mktemp "${TMPDIR:-/tmp}/xiaoo-fmt.XXXXXX")"
trap 'rm -f "$OUT_FILE"' EXIT
set +e
cargo fmt --all --check -- --color never >"$OUT_FILE" 2>&1
fmt_code=$?
set -e

mapfile -t diff_lines < <(grep '^Diff in ' "$OUT_FILE" || true)

# ---- 判定 ---------------------------------------------------------------------
# exit 0 且无 Diff 行 = 全合规。非 0 且有 Diff 行 = 格式违规，逐条报 [FMT]。
# 非 0 且无 Diff 行 = 工具自身失败（源码解析错误、内部错误等），按 [META]
# 硬失败并回显原始输出——宁可误报不可漏报。
if [ "$fmt_code" -eq 0 ] && [ "${#diff_lines[@]}" -eq 0 ]; then
    log "代码格式门禁检查完成：全部 workspace 源码符合 cargo fmt"
    exit 0
fi
if [ "${#diff_lines[@]}" -eq 0 ]; then
    log "FAIL [META]: cargo fmt --check 运行失败（exit $fmt_code），无差异报告"
    sed 's/^/        | /' "$OUT_FILE" >&2 || true
    exit 1
fi

# 每处差异一条 FAIL [FMT]。rustfmt 打印绝对路径，且 #[path] 接入的测试文件
# 带 ../../.. 前缀；realpath 归一为仓库根相对路径更可读，realpath 不可用时
# 回退仅剥 PWD 前缀。
file_count=0
declare -A seen_files=()
for line in "${diff_lines[@]}"; do
    loc="${line#Diff in }"
    loc="${loc%:}"
    lineno="${loc##*:}"
    path="${loc%:*}"
    rel="${path#"$PWD"/}"
    norm="$(realpath -m --relative-to="$PWD" "$path" 2>/dev/null || printf '%s' "$rel")"
    log "FAIL [FMT]: ${norm}:${lineno} 排版与 cargo fmt 不一致"
    if [ -z "${seen_files[$norm]:-}" ]; then
        seen_files[$norm]=1
        file_count=$((file_count+1))
    fi
done

log "代码格式门禁检查完成：${file_count} 个文件共 ${#diff_lines[@]} 处不符合 cargo fmt；请在仓库根运行 cargo fmt 修复（只改排版不改语义）后重新提交"
exit 1
