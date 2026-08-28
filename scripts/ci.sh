#!/usr/bin/env bash
# scripts/ci.sh
#
# 工程统一 CI 入口。跑这一个脚本即可，未来扩展只需在 STEPS 数组加一行。
#
# 当前编排的检查（全部静态、不编译，秒级）：
#   1. 门面纪律门禁    scripts/check-facade.sh
#      — 应用只依赖 xiaoo-api/xiaoo-shared；底层 publish=false；xiaoo-api 对外 pub 面冻结。
#
# 新增检查：在下方 STEPS 数组加一行 "名称|命令"。命令既可是脚本文件路径，
# 也可是内联 shell 命令。诊断写到 stderr；失败时 ci.sh 捕获并按"失败详情"块
# 缩进回显，同时抽取一行原因进汇总。
#
# 行为：某步失败不会中断，所有步骤都跑完，末尾汇总全部失败项及其原因。
#
# 用法：
#   scripts/ci.sh            # 跑全部检查
#   scripts/ci.sh -h|--help # 用法说明
#
# 退出码：0 全过；1 有失败。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

usage() {
    cat >&2 <<'EOF'
用法: scripts/ci.sh [-h]
  跑工程统一 CI 全部检查（当前为静态门禁，秒级）。
  -h, --help   显示本帮助
退出码：0 全过；1 有失败；2 参数错误
EOF
}

for arg in "$@"; do
    case "$arg" in
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "未知参数: $arg（用法: scripts/ci.sh [-h]）" >&2
            exit 2
            ;;
    esac
done

# ---- 步骤表 -----------------------------------------------------------------
# 每行："名称|命令"。以首个 `|` 切分，故名称不得含 `|`（命令里可含 `|`，
# 例如内联管道，首个 `|` 之后的部分整体作为命令）。
# 命令既可以是脚本文件路径，也可以是内联 shell 命令。
# 顺序即执行顺序。子脚本约定把诊断写到 stderr；失败时 ci.sh 捕获并按
# "失败详情"块缩进回显，同时抽取一行原因进汇总。
STEPS=(
    "门面纪律门禁|scripts/check-facade.sh"
)

# ---- 执行 -------------------------------------------------------------------
overall=0
# 失败步骤记录：步骤名 / 一句话原因 / 可重跑命令 / 日志文件路径，末尾汇总回显。
failed_steps=()
failed_reasons=()
failed_cmds=()
failed_outputs=()

printf '%s\n' "CI: 共 ${#STEPS[@]} 项检查" >&2

# 子脚本输出落到唯一临时目录（mktemp -d 避免并发 CI 互踩固定路径），
# 失败时保留以便用户重跑查看；成功步骤的日志即时删除。
tmplog_dir="$(mktemp -d "${TMPDIR:-/tmp}/xiaoo-ci-logs.XXXXXX")"
# 异常退出（set -e 中途失败）时也清理临时目录；失败汇总末尾用 `trap - EXIT` 解除，
# 以保留失败日志供用户重跑。
trap 'rm -rf "$tmplog_dir"' EXIT

step_no=0
for entry in "${STEPS[@]}"; do
    name="${entry%%|*}"
    cmd="${entry#*|}"
    ci_title "$name"
    # 命令形态：指向一个已存在的脚本文件则直接跑该文件；否则当作内联 shell 命令用 bash -c 执行。
    # rerun：可由用户复制粘贴回终端重跑该步骤的安全引用形式（printf %q 按需转义，含空格/特殊字符也安全）。
    if [ -f "$cmd" ]; then
        run=("bash" "$cmd")
        rerun="bash $(printf '%q' "$cmd")"
    else
        run=("bash" "-c" "$cmd")
        rerun="bash -c $(printf '%q' "$cmd")"
    fi
    # 捕获全部输出（check-*.sh 约定写 stderr），便于失败时回显。
    # set +e：子命令失败由我们自行记录，不触发本脚本的 set -e，也不中断后续步骤。
    # step_no 先自增再拼文件名：日志文件 1-based（step-1.log...），与"第 N 步"语义一致。
    step_no=$((step_no+1))
    out_file="$tmplog_dir/step-${step_no}.log"
    set +e
    "${run[@]}" >"$out_file" 2>&1
    code=$?
    set -e
    ci_result "$name" "$code"
    if [ "$code" -ne 0 ]; then
        overall=1
        reason="$(extract_reason <"$out_file")"
        failed_steps+=("$name")
        failed_reasons+=("$reason")
        failed_cmds+=("$rerun")
        failed_outputs+=("$out_file")
        # 失败时立即把详情缩进回显，紧跟在该步骤结果之后，用户当场就能看到原因。
        ci_section "失败详情: $name"
        print_indented <"$out_file"
    else
        rm -f "$out_file"
    fi
done

# ---- 汇总 -----------------------------------------------------------------
ci_title "CI 结果"
if [ "$overall" -eq 0 ]; then
    printf '  [PASS] 全部检查通过\n' >&2
    exit 0
fi
n=${#failed_steps[@]}
printf '  [FAIL] %d 项失败:\n' "$n" >&2
for ((i=0; i<n; i++)); do
    printf '        - %s\n' "${failed_steps[$i]}" >&2
    printf '            原因: %s\n' "${failed_reasons[$i]}" >&2
    printf '            重跑: %s\n' "${failed_cmds[$i]}" >&2
    out="${failed_outputs[$i]}"
    if [ -n "$out" ] && [ -s "$out" ]; then
        printf '            日志: %s\n' "$out" >&2
    fi
done
# 保留失败日志目录供用户重跑，解除 EXIT trap 不再清理。
trap - EXIT
exit 1
