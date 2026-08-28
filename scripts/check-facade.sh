#!/usr/bin/env bash
# scripts/check-facade.sh
#
# 门面纪律门禁。由 scripts/ci.sh 编排调用；编排行为与步骤表见 ci.sh 头注释。
#
# 工具链要求：cargo ≥ 1.50（cargo tree 的 -e / --depth 已稳定）、bash ≥ 4
# （用 mapfile / declare -A 关联数组）、python3（解析 cargo metadata 的 JSON）。
#
# 四条检查的 FAIL 行前缀（便于 ci.sh 抽取原因与读者对位）：
#   [META]       cargo/python3 缺失或 metadata 解析失败——硬失败，整体中止
#   [APP-DEPS]   endside/serverside 直接依赖边不含底层 crate
#   [APP-USE]    apps 源码零底层 use——不出现 `use <底层 crate>::`
#   [PUBLISH]    底层 crate 必须 publish=false
#   [API-FROZEN] xiaoo-api 对外 pub 面冻结——path 依赖集与 pub mod 列表不得扩充
#
# 退出码：0 通过；1 有违反。违规项逐条打印到 stderr。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

log() { printf '%s\n' "$*" >&2; }

# ---- 工具与数据源预检 --------------------------------------------------------
# 缺任一工具或 metadata 解析失败即硬失败退出。
# 防止后续 mapfile 在子 shell 里静默拿空、门禁 fail-open。
for tool in cargo python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        log "FAIL [META]: 缺少 $tool，无法执行门面纪律检查"
        exit 1
    fi
done

# 缓存 cargo metadata 到临时文件，避免后续每个 crate 重复调用（N+1）。
# stderr 单独捕获到 META_ERR：成功即删；失败时把 stderr 原样回显，便于排障
# （之前 2>/dev/null 全吞，cargo 报错用户看不到根因）。
META_FILE="$(mktemp "${TMPDIR:-/tmp}/xiaoo-meta.XXXXXX.json")"
META_ERR="$(mktemp "${TMPDIR:-/tmp}/xiaoo-meta-err.XXXXXX")"
trap 'rm -f "$META_FILE" "$META_ERR"' EXIT
if ! cargo metadata --no-deps --format-version 1 >"$META_FILE" 2>"$META_ERR"; then
    log "FAIL [META]: cargo metadata 失败，无法解析工作区"
    if [ -s "$META_ERR" ]; then
        log "        stderr:"
        sed 's/^/        | /' "$META_ERR" >&2 || true
    fi
    rm -f "$META_ERR"
    exit 1
fi
rm -f "$META_ERR"

# 一次性把所有 crate 的 manifest_path 装进关联数组，后续 lookup 零 python3
# 调用，消除 N+1 冷启动（旧实现每个 crate 各启一次 python3）。
declare -A MANIFEST_PATHS=()
while IFS=$'\t' read -r pkg_name pkg_manifest; do
    [ -n "$pkg_name" ] && MANIFEST_PATHS["$pkg_name"]="$pkg_manifest"
done < <(python3 -c "import json,sys
d=json.load(open(sys.argv[1]))
for p in d['packages']:
    print(p['name']+'\t'+p['manifest_path'])" "$META_FILE" 2>/dev/null)

# manifest_path_of：从预解析的 MANIFEST_PATHS 取 manifest 路径；未命中回退空串。
manifest_path_of() {
    printf '%s' "${MANIFEST_PATHS[$1]:-}"
}

# ---- 受管 crate 名单 ----------------------------------------------------------
# 两个应用门面（允许被应用直接依赖）：
FACADE_CRATES="xiaoo-api xiaoo-shared"
# 应用（被约束的对象）：
APPS="xiaoo-endside xiaoo-serverside"
# 底层 crates：禁止被应用直接依赖、必须 publish=false、禁止在 apps 源码里被 use。
# 名单 = 工作区所有 path crate − 门面 − 应用 − vault（vault 仅被 shared 引用，
# 属于 shared 的实现细节，不向应用暴露，但当前不是应用直依约束对象）。
# 名单由缓存 metadata 动态生成，避免增删底层 crate 时名单滞后。
# 复用上方 MANIFEST_PATHS 的键集合，不再二次调 python3。
WORKSPACE_CRATES=("${!MANIFEST_PATHS[@]}")
if [ "${#WORKSPACE_CRATES[@]}" -eq 0 ]; then
    log "FAIL [META]: 工作区 crate 名单为空（metadata 解析异常）"
    exit 1
fi
# 排除集（门面 + 应用 + vault）由上面的名单变量拼出，单一真相源：
# 新增门面/app 只改 FACADE_CRATES/APPS，过滤正则自动同步。
EXCLUDE_PAT="$(printf '%s\n' $FACADE_CRATES $APPS vault | paste -sd'|' -)"
mapfile -t BOTTOM_CRATES < <(printf '%s\n' "${WORKSPACE_CRATES[@]}" \
    | grep -vxE "$EXCLUDE_PAT" \
    | sort -u)
if [ "${#BOTTOM_CRATES[@]}" -eq 0 ]; then
    log "FAIL [META]: 底层 crate 名单为空（名单计算异常）"
    exit 1
fi

# `use <crate>::` 的 crate 名在 Rust 源码里用下划线（连字符归一化为下划线）。
BOTTOM_USE_PAT="$(printf '%s\n' "${BOTTOM_CRATES[@]}" | tr '-' '_' | paste -sd'|' -)"
if [ -z "$BOTTOM_USE_PAT" ]; then
    log "FAIL [META]: 底层 use 模式为空（名单计算异常）"
    exit 1
fi

fail=0

# ---- [APP-DEPS] 应用依赖边只认门面 -------------------------------------------
# endside / serverside 的直接依赖边（normal/build/dev 三类，深度 1）只允许
# 出现两个门面 crate，底层 crate 出现即 FAIL。
# cargo tree 每 app 只调一次，缓存输出再判退出码，避免重复调用。
for app in $APPS; do
    tree_out="$(mktemp "${TMPDIR:-/tmp}/xiaoo-tree.XXXXXX")"
    tree_err="$(mktemp "${TMPDIR:-/tmp}/xiaoo-tree-err.XXXXXX")"
    if ! cargo tree -p "$app" --depth 1 -e normal,build,dev --prefix none \
            >"$tree_out" 2>"$tree_err"; then
        log "FAIL [APP-DEPS]: 无法解析 $app 的依赖树（cargo tree 失败）"
        if [ -s "$tree_err" ]; then
            log "        stderr:"
            sed 's/^/        | /' "$tree_err" >&2 || true
        fi
        fail=1
        rm -f "$tree_out" "$tree_err"
        continue
    fi
    rm -f "$tree_err"
    # awk 拆出首列 crate 名；tree_out 由 cargo tree 刚写入，失败即视作 META 硬失败。
    deps="$(awk '{print $1}' "$tree_out" | sort -u)"
    rm -f "$tree_out"
    # grep 退码 1=无匹配属正常，需 || true；deps 已保证为 awk 实际产物。
    bad="$(printf '%s\n' "$deps" \
           | grep -xFf <(printf '%s\n' "${BOTTOM_CRATES[@]}") || true)"
    if [ -n "$bad" ]; then
        log "FAIL [APP-DEPS]: $app 直接依赖了底层 crates（只允许 $FACADE_CRATES）："
        printf '%s\n' "$bad" | sed "s/^/$LIST_PREFIX/"
        fail=1
    fi
done

# ---- [APP-USE] apps 源码零底层 use -------------------------------------------
# 任何 `use <底层 crate>::` 都是违反：应用必须经 xiaoo-shared 的粗粒度入口
# 或再导出访问，不直接命名底层 crate。
# 正则 `use[[:space:]]+` 后紧跟 crate 名、再以 `::` 锚定，已天然防止子串误报
# （如 `use my_agent_types::` 不会命中 `agent_types`）；故不依赖 GNU 的 `\b`，保跨平台。
# 可见性部分 `(pub[[:space:]]*(\([^)]*\))?[[:space:]]+)?` 同时覆盖
# `use`、`pub use`、`pub(crate) use`、`pub (crate) use` 四种合法写法。
# 已知限制：不覆盖跨行 `use\n    <crate>::Foo;`——按 rustfmt 单行规范，命中即可；
# 若后续源码出现跨行 use，应先 rustfmt 再跑本检查。
# APP_SCAN 由 $APPS 派生（crate 名去 "xiaoo-" 前缀即 apps/ 下的目录名），
# 单一真相源：新增 app 只改 APPS，扫描路径自动同步。
APP_SCAN=()
for app in $APPS; do
    dir="${app#xiaoo-}"
    APP_SCAN+=("apps/$dir/src")
    [ -f "apps/$dir/build.rs" ] && APP_SCAN+=("apps/$dir/build.rs")
done
# 扫前校验目标存在，避免重命名/删除后 grep 静默无结果导致 fail-open。
for d in "${APP_SCAN[@]}"; do
    [ -e "$d" ] || { log "FAIL [META]: 缺失应用源码 $d"; exit 1; }
done
USE_RE='^[[:space:]]*(pub[[:space:]]*(\([^)]*\))?[[:space:]]+)?use[[:space:]]+('"$BOTTOM_USE_PAT"')::'
# grep 退码 1=无匹配属正常，需 || true。
violations=$(grep -rnE "$USE_RE" "${APP_SCAN[@]}" || true)
if [ -n "$violations" ]; then
    log "FAIL [APP-USE]: apps 源码出现底层 crate 的 use（应经 xiaoo-shared 门面访问）："
    printf '%s\n' "$violations" | sed "s/^/$LIST_PREFIX/"
    fail=1
fi

# ---- [PUBLISH] 底层 crates 必须 publish=false ---------------------------------
# xiaoo-api 作为门面允许发布；apps（shared/endside/serverside/vault）是 workspace
# 内 path crate，同样不强求 publish=false（它们本就不对外发布路径）。
# 唯独底层实现 crates 必须锁死 publish=false，杜绝仓库外直接依赖。
#
# PUBLISH_EXEMPT：白名单——这些 crate 当前未配 publish=false，跳过检查。
# 列在白名单即长期豁免，不再强制补 publish=false；移出白名单删条目即可。
# 豁免项每次跑打 WARN，保持 CI 输出可见、可审计。
PUBLISH_EXEMPT=(
    "moirai"
)
# is_exempt：crate 是否在白名单内。
is_exempt() {
    local target="$1" en
    for en in "${PUBLISH_EXEMPT[@]}"; do
        [ "$en" = "$target" ] && return 0
    done
    return 1
}
for c in "${BOTTOM_CRATES[@]}"; do
    if is_exempt "$c"; then
        log "WARN [PUBLISH]: $c 在 PUBLISH_EXEMPT 白名单内，跳过 publish=false 检查"
        continue
    fi
    manifest="$(manifest_path_of "$c")"
    # 非豁免底层 crate 的 manifest 必须能定位；解析失败不静默跳过，否则 publish 检查 fail-open。
    if [ -z "$manifest" ]; then
        log "FAIL [META]: 无法定位底层 crate $c 的 manifest（metadata 解析异常）"
        fail=1
        continue
    fi
    if ! grep -qE '^[[:space:]]*publish[[:space:]]*=[[:space:]]*false' "$manifest"; then
        log "FAIL [PUBLISH]: 底层 crate $c 缺少 publish = false（$manifest）"
        fail=1
    fi
done

# ---- [API-FROZEN] xiaoo-api 对外 pub 面冻结 ----------------------------------
# 冻结基线存于 scripts/baselines/*.txt（一处真相源，变更基线时改文件即可）。
API_DEPS_BASELINE="$SCRIPT_DIR/baselines/api-deps.txt"
API_MODS_BASELINE="$SCRIPT_DIR/baselines/api-mods.txt"
for f in "$API_DEPS_BASELINE" "$API_MODS_BASELINE"; do
    if [ ! -s "$f" ]; then
        log "FAIL [META]: 缺失或空的基线文件 $f"
        exit 1
    fi
done

# 1) 路径依赖集不得新增：当前 path 依赖必须是基线集的子集。
#    只认 [dependencies] 表内的 path 依赖（dev/build 依赖不进对外 pub 面）。
#    两步 grep：先选 inline-table 行（`= {`），再在其中找 path 键。
#    这样 path 无论是否为 inline table 首键都能命中——例如 `{ version="1.0", path=".." }`
#    （version 在前）旧正则 `={[[:space:]]*path` 漏掉，会被新增依赖绕过冻结；
#    新正则 `[^[:alnum:]_-]path[[:space:]]*=` 用非标识符边界防止 `mypath` 之类子串误报。
api_manifest="$(manifest_path_of xiaoo-api)"
if [ -z "$api_manifest" ]; then
    log "FAIL [META]: 无法定位 xiaoo-api 的 manifest（metadata 解析异常）"
    exit 1
fi
api_lib="${api_manifest%Cargo.toml}src/lib.rs"
# 扫前校验目标文件存在且非空，避免路径迁移/误删后 grep 静默无结果导致 fail-open
# ——这恰是冻结检查最该保护的对象。
for f in "$api_manifest" "$api_lib"; do
    [ -s "$f" ] || { log "FAIL [META]: 缺失或空 $f"; exit 1; }
done

# awk 抽取 [dependencies] 表体（到下一个 [xxx] 表为止），再走两步 grep 提取依赖名。
api_path_deps=$(awk '
    /^\[dependencies\][[:space:]]*$/ { in_dep=1; next }
    /^\[/ { in_dep=0; next }
    in_dep
' "$api_manifest" \
    | grep -E '=[[:space:]]*\{' \
    | grep -E '[^[:alnum:]_-]path[[:space:]]*=' \
    | sed -E 's/^([a-z0-9_-]+)[[:space:]]*=.*/\1/' | sort -u)
mapfile -t api_frozen_deps < <(grep -vE '^[[:space:]]*(#|$)' "$API_DEPS_BASELINE" | sort -u)
# comm 退码 1（输入无差异）属正常，|| true 防御性兜底。
new_deps=$(comm -13 \
    <(printf '%s\n' "${api_frozen_deps[@]}") \
    <(printf '%s\n' "$api_path_deps") || true)
if [ -n "$new_deps" ]; then
    log "FAIL [API-FROZEN]: xiaoo-api 新增了非基线的 path 依赖（对外 pub 面已冻结，不得扩充）："
    printf '%s\n' "$new_deps" | sed "s/^/$LIST_PREFIX/"
    fail=1
fi

# 2) 对外模块固定：lib.rs 的 pub mod 列表不得扩充（含 `mod foo;` 文件模块与 `mod foo {` 内联块）。
cur_mods=$(sed -nE 's/^(pub[[:space:]]+)?mod[[:space:]]+([a-z0-9_]+)[;{].*/\2/p' "$api_lib" | sort -u)
mapfile -t api_frozen_mods < <(grep -vE '^[[:space:]]*(#|$)' "$API_MODS_BASELINE" | sort -u)
# comm 退码 1（输入无差异）属正常，|| true 防御性兜底。
extra_mods=$(comm -13 \
    <(printf '%s\n' "${api_frozen_mods[@]}") \
    <(printf '%s\n' "$cur_mods") || true)
if [ -n "$extra_mods" ]; then
    log "FAIL [API-FROZEN]: xiaoo-api 新增了对外 pub mod（对外 pub 面已冻结，不得扩充）："
    printf '%s\n' "$extra_mods" | sed "s/^/$LIST_PREFIX/"
    fail=1
fi

# ---- 汇总 -------------------------------------------------------------------
if [ "$fail" -ne 0 ]; then
    log ""
    log "门面纪律检查未通过。"
    log "修复方向：应用只依赖 xiaoo-api + xiaoo-shared；底层访问经 shared 粗粒度入口；"
    log "新增 pub 项前确认有应用消费者，仅内部复用写 pub(crate)。"
    exit 1
fi

log "门面纪律检查通过：应用仅依赖 xiaoo-api + xiaoo-shared，底层 pub 面已锁定。"
exit 0
