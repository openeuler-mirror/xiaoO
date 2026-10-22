#!/usr/bin/env bash
# scripts/check-dockerfile-copy-coverage.sh
#
# Dockerfile COPY 覆盖门禁。由 scripts/ci.sh 编排调用；编排行为与步骤表见
# ci.sh 头注释。
#
# 背景（issue #148）：Dockerfile 的分层 COPY 清单与 workspace 成员之间曾出现
# 双向漂移——引用已删除的 crate（llm-client-cli）、遗漏新增成员（xiaoo-api /
# protocol）、对已删除的 hooker 子目录做操作（audit_agent / audit_policy_checker）。
# 这类 drift 只有 `docker build` 跑到对应步骤才暴露（一次完整 Rust 编译），
# 不适合当日常回归；三项判定全部可静态完成，故固化为本门禁。
#
# 纯离线、不联网、不编译、秒级。检查项：
#
#   [COPY]    Dockerfile 中每条 COPY（`--from=<stage>` 除外）的每个源路径
#             必须在仓库内真实存在。
#   [MEMBER]  根 Cargo.toml `members` 数组的每个活跃成员（注释行跳过）必须被
#             一条 `COPY <member>/Cargo.toml` 覆盖——`RUN cargo fetch` 发生在
#             `COPY . .` 之前，缺任何一个成员整个 workspace 都解析不了。
#   [PLUG]    Dockerfile 中引用的每个 `plugins/<subpath>` / `hookers/<subpath>`
#             路径（至少一段子路径）必须映射到仓库内真实文件/目录
#             （`hookers/<x>` → `plugins/hookers/<x>`），防止对已删除插件
#             目录的残留引用。
#
# 失败行的前缀（便于 ci.sh 抽取原因与读者对位）：
#   [COPY] / [MEMBER] / [PLUG]   见上。
#
# 退出码：0 通过；1 有违反。违规项逐条打印到 stderr。
#
# 设计取舍：只做"存在性 + 覆盖率"静态判定，不解析 Dockerfile 语义（stage
# 依赖、RUN 顺序等）；JSON 数组形态的 COPY 本仓不使用，出现即报无法解析，
# 促使改回空格分隔形态。反向覆盖（COPY 了非 workspace 成员的 manifest，
# 如为重新启用 cerberus 预留）不算违规，避免误报。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

log() { printf '%s\n' "$*" >&2; }

DOCKERFILE="Dockerfile"
MANIFEST="Cargo.toml"

[ -f "$DOCKERFILE" ] || { log "FAIL [COPY]: 缺少 $DOCKERFILE"; exit 1; }
[ -f "$MANIFEST" ] || { log "FAIL [MEMBER]: 缺少 $MANIFEST"; exit 1; }

failures=0
member_total=0
fail() { log "FAIL $*"; failures=$((failures + 1)); }

# ---- 解析 COPY 行：收集源路径与 manifest 覆盖集合 ---------------------------
# copy_srcs     "行号|源路径" 列表（--from 的 stage 拷贝不算，源在构建产物里）。
# copy_manifests COPY 源里形如 <member>/Cargo.toml 的成员路径列表。
copy_srcs=()
copy_manifests=()
while IFS= read -r raw; do
    [ -n "$raw" ] || continue
    lineno="${raw%%:*}"
    text="${raw#*:}"
    case "$text" in
        *--from*) continue ;;
    esac
    args="$(printf '%s' "$text" | sed -E 's/^[[:space:]]*COPY[[:space:]]+//')"
    read -r -a tokens <<< "$args"
    plain=()
    json_form=0
    for t in ${tokens[@]+"${tokens[@]}"}; do
        case "$t" in
            --*) ;;
            *\[*|*\]*) json_form=1 ;;
            *) plain+=("$t") ;;
        esac
    done
    if [ "$json_form" -eq 1 ]; then
        fail "[COPY]: $DOCKERFILE:$lineno JSON 数组形态的 COPY 不受支持，请改用空格分隔形态: $text"
        continue
    fi
    n=${#plain[@]}
    if [ "$n" -lt 2 ]; then
        fail "[COPY]: $DOCKERFILE:$lineno 无法解析 COPY 参数（至少需 源 + 目标）: $text"
        continue
    fi
    # 最后一个 token 是目标，其余是源。
    for ((i = 0; i < n - 1; i++)); do
        src="${plain[$i]}"
        copy_srcs+=("$lineno|$src")
        case "$src" in
            */Cargo.toml) copy_manifests+=("${src%/Cargo.toml}") ;;
        esac
    done
done < <(grep -n -E '^[[:space:]]*COPY([[:space:]]|$)' "$DOCKERFILE" || true)

# ---- [COPY] 源路径存在性 ----------------------------------------------------
for entry in ${copy_srcs[@]+"${copy_srcs[@]}"}; do
    lineno="${entry%%|*}"
    src="${entry#*|}"
    if [ ! -e "$src" ]; then
        fail "[COPY]: $DOCKERFILE:$lineno 源路径 '$src' 在仓库中不存在"
    fi
done

# ---- [MEMBER] workspace 成员全覆盖 ------------------------------------------
# members 块内先剥注释再取引号串，天然跳过注释掉的成员（如 cerberus 三件套）。
members_block="$(sed -n '/^[[:space:]]*members[[:space:]]*=/,/]/p' "$MANIFEST")"
if [ -z "$members_block" ]; then
    fail "[MEMBER]: $MANIFEST 中未找到 [workspace] members 数组"
else
    is_covered() {
        local m="$1" c
        for c in ${copy_manifests[@]+"${copy_manifests[@]}"}; do
            [ "$c" = "$m" ] && return 0
        done
        return 1
    }
    member_total=0
    while IFS= read -r m; do
        [ -n "$m" ] || continue
        member_total=$((member_total + 1))
        if ! is_covered "$m"; then
            fail "[MEMBER]: workspace 成员 '$m' 未被 COPY，docker build 会在 cargo fetch 前解析失败（补 COPY $m/Cargo.toml 或将其注释出 members）"
        fi
    done < <(printf '%s\n' "$members_block" | sed 's/#.*//' | grep -oE '"[^"]+"' | tr -d '"' || true)
fi

# ---- [PLUG] plugins/ 与 hookers/ 引用存在性 ---------------------------------
# 从 Dockerfile 全文提取（含 RUN/CMD 与注释），refs 至少带一段子路径；
# `plugins/<x>` 原样校验，`hookers/<x>` 映射到 `plugins/hookers/<x>`。
plug_refs="$(grep -oE '(plugins|hookers)/[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)*' "$DOCKERFILE" | sort -u || true)"
while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    case "$ref" in
        plugins/*) repo_path="$ref" ;;
        hookers/*) repo_path="plugins/$ref" ;;
    esac
    if [ ! -e "$repo_path" ]; then
        fail "[PLUG]: $DOCKERFILE 引用 '$ref'，但仓库内 '$repo_path' 不存在（已删除的插件目录残留？）"
    fi
done <<< "$plug_refs"

# ---- 结果输出 ----------------------------------------------------------------
if [ "$failures" -ne 0 ]; then
    log "Dockerfile COPY 覆盖门禁检查完成：${#copy_srcs[@]} 个 COPY 源、$member_total 个 workspace 成员，$failures 项违规"
    exit 1
fi

log "Dockerfile COPY 覆盖门禁检查完成：${#copy_srcs[@]} 个 COPY 源、$member_total 个 workspace 成员，全部通过"
