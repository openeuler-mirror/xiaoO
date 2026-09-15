#!/usr/bin/env bash
# scripts/check-tests-hygiene.sh
#
# 测试卫生门禁：防止测试代码回流功能源文件，强制测试统一收归仓库根 tests/。
# 目录约定与命名规则见 tests/README.md。暂未接入 scripts/ci.sh（待存量迁移
# 完成后启用），当前可手工运行，输出即迁移进度报告。
#
# FAIL 行前缀（便于 ci.sh 抽取原因与读者对位）：
#   [META]       cargo/python3 缺失、metadata 解析失败或扫描范围异常——硬失败，整体中止
#   [SRC-TEST]   member src/ 中出现 #[test] / #[tokio::test]
#   [SRC-CFG]    member src/ 中 test 门控项不符合唯一允许的形式：
#                单个 `#[cfg(test)] #[path = "…"] mod …;` 声明（每文件至多 1 个）。
#                覆盖：内联测试模块（mod 带 body）、门控的 fn/const/impl/use
#                测试构造器与探针、缺 #[path] 的 mod 声明、同文件多个声明等。
#   [SRC-PATH]   test 门控 mod 声明的 #[path] 目标不在仓库根 tests/ 目录下
#   [TEST-NAME]  tests/ 下含 #[test] 的 .rs 文件未以 _test.rs 命名（白名单除外）
#
# 扫描范围（以 cargo metadata 的 workspace members 动态推导，不硬编码 crate 名单）：
#   - SRC-TEST / SRC-CFG / SRC-PATH：各 member 的 src/；
#   - TEST-NAME：各 member 的 tests/（存在时）+ 仓库根 tests/。
# plugins/ 不是 workspace member，天然不在扫描范围（不参与本约定，零改动）。
# cerberus/ 未入 workspace，同样天然不被扫描；未来加入 workspace 即自动纳入覆盖，
# 无需改本脚本（也不硬编码排除它）。
#
# test 门控判定：cfg 属性谓词中出现裸词 test——覆盖 cfg(test) / cfg(all(test, …)) /
# cfg(any(…, test)) / cfg(not(test)) 等一切变体，不写死 `#[cfg(test)]` 字面量；
# 引号内的 "test…"（如 cfg(feature = "test-support")）不算 test 门控。
#
# 白名单（新增需评审）：
#   SEAM_WHITELIST      src 中保留原位的双版本 seam：cfg(test)/cfg(not(test)) 同名
#                       函数成对共存，测试版本被生产代码路径在测试构建下调用，
#                       无法外移。格式："相对仓库根路径:符号名"，一条登记覆盖两个版本。
#   MULTI_DECL_WHITELIST 一文件多 test 门控 mod 声明的例外（功能文件按职责
#                       拆分后各主题测试各得一个测试文件的核准场景，如
#                       command_spec.rs 的 bubblewrap/dynsandbox 两个主题测试）。
#                       格式："相对仓库根路径"。
#                       例外文件中每个声明仍须为合规形式（单门控 + 单 #[path] +
#                       mod 声明，目标在仓库根 tests/ 下）。
#   TEST_NAME_WHITELIST tests/ 下含 #[test] 但因辅助职责不按 _test.rs 命名的文件。
#                       格式："相对仓库根路径"。不含 #[test] 的辅助文件天然豁免，无需登记。
#
# 已知限制（fail-closed，宁可误报不可漏报）：
#   - 行级正则扫描，不解析 Rust 语法；字符串/文档注释中出现 `#[test]`、
#     `cfg(test)` 字样会误报（先清理注释再复查）；
#   - 跨行书写的 cfg 属性不识别（rustfmt 会折回单行）；
#   - 罕见的 feature 名恰含裸词 test（如 "a-test-b"）会被当作 test 门控。
#
# 退出码：0 通过；1 有违反。违规项逐条打印到 stderr。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
ROOT_DIR="$(root_dir)"
cd "$ROOT_DIR"

log() { printf '%s\n' "$*" >&2; }

# ---- 工具与数据源预检 --------------------------------------------------------
# 缺任一工具或 metadata 解析失败即硬失败退出，防止后续扫描静默拿空、门禁 fail-open。
for tool in cargo python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        log "FAIL [META]: 缺少 $tool，无法执行测试卫生检查"
        exit 1
    fi
done

META_FILE="$(mktemp "${TMPDIR:-/tmp}/xiaoo-meta.XXXXXX.json")"
META_ERR="$(mktemp "${TMPDIR:-/tmp}/xiaoo-meta-err.XXXXXX")"
trap 'rm -f "$META_FILE" "$META_ERR"' EXIT
if ! cargo metadata --no-deps --format-version 1 >"$META_FILE" 2>"$META_ERR"; then
    log "FAIL [META]: cargo metadata 失败，无法解析工作区"
    if [ -s "$META_ERR" ]; then
        log "        stderr:"
        sed 's/^/        | /' "$META_ERR" >&2 || true
    fi
    exit 1
fi
rm -f "$META_ERR"

# ---- 扫描范围推导 -------------------------------------------------------------
# member 根目录 = manifest_path 去掉尾部 Cargo.toml；src/ 为 Rust crate 必有，
# 缺失即目录结构异常（硬失败）；tests/ 仅存在时纳入 TEST-NAME 扫描。
# 仓库根 tests/ 是测试收归的目标结构骨架，必须存在。
mapfile -t MEMBER_ROOTS < <(python3 -c "import json,sys
d=json.load(open(sys.argv[1]))
for p in d['packages']:
    print(p['manifest_path'].rsplit('/',1)[0])" "$META_FILE" 2>/dev/null | sort -u)
if [ "${#MEMBER_ROOTS[@]}" -eq 0 ]; then
    log "FAIL [META]: workspace member 名单为空（metadata 解析异常）"
    exit 1
fi

SRC_DIRS=()
TEST_DIRS=("tests")
missing_src=()
for member in "${MEMBER_ROOTS[@]}"; do
    if [ -d "$member/src" ]; then
        SRC_DIRS+=("$member/src")
    else
        missing_src+=("$member/src")
    fi
    [ -d "$member/tests" ] && TEST_DIRS+=("$member/tests")
done
if [ "${#missing_src[@]}" -gt 0 ]; then
    log "FAIL [META]: 以下 member 缺失 src/ 目录（目录结构异常）："
    printf '%s\n' "${missing_src[@]}" | sed "s/^/$LIST_PREFIX/"
    exit 1
fi
if [ ! -d "tests" ]; then
    log "FAIL [META]: 缺失仓库根 tests/ 目录（测试收归的目标结构骨架）"
    exit 1
fi

# ---- 白名单 -------------------------------------------------------------------
SEAM_WHITELIST=(
    "apps/shared/src/gateway/memory_automation.rs:lock_wait_timeout"
)
MULTI_DECL_WHITELIST=(
    # command_spec.rs 拆分后 bubblewrap/dynsandbox 两个主题测试各得一个
    # _test.rs（不合并），源文件因此有 2 个 #[path] 声明。
    "crates/operation_backend/src/backends/local/exec/command_spec.rs"
)
TEST_NAME_WHITELIST=(
    # 暂无
)

# ---- 扫描器 -------------------------------------------------------------------
# 核心扫描为行级状态机，内嵌 python3 实现；规则见本脚本头注释。
# 环境变量传入扫描根、目录清单与白名单（换行分隔），避免 argv 转义问题。
# 退出码非 0 仅记 fail，不中断（本脚本自身汇总）。
fail=0
XIAOO_SCAN_ROOT="$ROOT_DIR" \
XIAOO_SRC_DIRS="$(printf '%s\n' "${SRC_DIRS[@]}")" \
XIAOO_TEST_DIRS="$(printf '%s\n' "${TEST_DIRS[@]}")" \
XIAOO_SEAM_WHITELIST="$(printf '%s\n' "${SEAM_WHITELIST[@]}")" \
XIAOO_MULTI_DECL_WHITELIST="$(printf '%s\n' "${MULTI_DECL_WHITELIST[@]}")" \
XIAOO_TEST_NAME_WHITELIST="$(printf '%s\n' "${TEST_NAME_WHITELIST[@]}")" \
python3 - <<'PYEOF' || fail=1
import os
import re
import sys

ROOT = os.environ["XIAOO_SCAN_ROOT"]
SRC_DIRS = [d for d in os.environ["XIAOO_SRC_DIRS"].splitlines() if d]
TEST_DIRS = [d for d in os.environ["XIAOO_TEST_DIRS"].splitlines() if d]
SEAM_WL = {e for e in os.environ["XIAOO_SEAM_WHITELIST"].splitlines() if e}
MULTI_DECL_WL = {e for e in os.environ["XIAOO_MULTI_DECL_WHITELIST"].splitlines() if e}
NAME_WL = {e for e in os.environ["XIAOO_TEST_NAME_WHITELIST"].splitlines() if e}

# 单个外层属性 #[...]（属性值内不含 ]；#[path] 的值、cfg 谓词均满足）
ATTR_RE = re.compile(r'#\[[^]]*\]')
# cfg 属性：谓词 = cfg( 与配对 )]
CFG_ATTR_RE = re.compile(r'#\[\s*cfg\s*\((.*)\)\s*\]')
# 谓词中的裸词 test：前后不得是标识符字符或引号。
# 排除 "test-support" 等字符串字面量（test 前是引号）；cfg(test)/cfg(all(test, unix))/
# cfg(any(…, test))/cfg(not(test)) 等裸词形式均命中（all/not 等组合中 test 前是
# "(" 或逗号或空格，后是 ")" 或逗号或空格）。
# lookbehind 不得排除 "("，否则 cfg(all(test, …)) 变体漏检（fail-open）。
BARE_TEST_RE = re.compile(r'(?<![\w"])test(?![\w"])')
# 测试属性：#[test] / #[test ] / #[tokio::test] / #[tokio::test(...)]
TEST_ATTR_RE = re.compile(r'#\[\s*(?:tokio::)?test\s*[\](]')
# mod 声明（无 body）：mod NAME; / pub mod NAME; / pub(crate) mod NAME;
MOD_DECL_RE = re.compile(r'^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(\w+)\s*;\s*$')
# #[path = "…"] 的目标值
PATH_ATTR_RE = re.compile(r'#\[\s*path\s*=\s*"([^"]*)"\s*\]')
# 项符号名提取（seam 白名单按 符号名 匹配；fn/const/static）
SYMBOL_RE = re.compile(
    r'^\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:const\s+)?(?:unsafe\s+)?(?:async\s+)?'
    r'(?:fn|const|static)\s+(\w+)')


def is_test_gate(attr):
    """属性是否为 test 门控（cfg 谓词含裸词 test，含 not(test) 变体）。"""
    m = CFG_ATTR_RE.search(attr)
    return bool(m) and bool(BARE_TEST_RE.search(m.group(1)))


def split_attrs(line):
    """把一行拆成（行首连续的属性列表, 余下内容）。"""
    attrs = []
    pos = 0
    while True:
        m = ATTR_RE.match(line, pos)
        if not m:
            break
        attrs.append(m.group(0))
        pos = m.end()
        while pos < len(line) and line[pos] in ' \t':
            pos += 1
    return attrs, line[pos:]


def is_comment(stripped):
    return (stripped.startswith('//') or stripped.startswith('/*')
            or stripped.startswith('*') or stripped.endswith('*/'))


def scan_src(path):
    """扫描一个 src 文件。返回 (rel, test_attr_lines, cfg_violations, path_violations)。

    cfg_violations / path_violations 元素为 (行号, 说明)。
    """
    rel = os.path.relpath(path, ROOT)
    test_attrs = []
    cfg_violations = []
    path_violations = []
    decls = []  # 允许形式的 test 门控 mod 声明：(行号, mod 名)

    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            lines = f.read().splitlines()
    except OSError as e:
        return rel, [], [(0, "无法读取文件: %s" % e)], []

    pending = []  # 挂起的属性行：[(行号, [属性串])]

    def handle(attr_parts, item_text, item_lineno):
        """对一条项及其前置属性做门控合规判定。attr_parts = [(行号, [属性串])]。"""
        all_attrs = [a for _, lst in attr_parts for a in lst]
        attr_line = attr_parts[0][0] if attr_parts else item_lineno
        gates = [a for a in all_attrs if is_test_gate(a)]
        if not gates:
            return
        path_vals = []
        for a in all_attrs:
            pm = PATH_ATTR_RE.search(a)
            if pm:
                path_vals.append(pm.group(1))
        md = MOD_DECL_RE.match(item_text)
        if md and len(gates) == 1 and len(path_vals) == 1:
            # 唯一允许的形式：单个 test 门控 + 单个 #[path] + mod 声明
            decls.append((attr_line, md.group(1)))
            target = os.path.normpath(
                os.path.join(os.path.dirname(path), path_vals[0]))
            rel_target = os.path.relpath(target, ROOT)
            if not (rel_target == "tests"
                    or rel_target.startswith("tests" + os.sep)):
                path_violations.append(
                    (attr_line, "#[path] 目标 %s 解析为 %s，不在仓库根 tests/ 下"
                     % (path_vals[0], rel_target)))
            return
        # 其余形式一律不允许；seam 白名单（路径:符号名）例外
        sym = SYMBOL_RE.match(item_text)
        if sym and "%s:%s" % (rel, sym.group(1)) in SEAM_WL:
            return
        if md and not path_vals:
            desc = "test 门控 mod 声明缺少 #[path]（须指向仓库根 tests/）"
        elif md:
            desc = ("test 门控 mod 声明形式不合规"
                    "（要求恰一个门控 + 恰一个 #[path]）")
        elif item_text.lstrip().startswith("mod "):
            desc = "内联测试模块（mod 带 body），须迁出到 tests/unit/"
        else:
            desc = ("test 门控的源码项 `%s`（测试构造器/探针须迁入 _test.rs，"
                    "确属双版本 seam 则登记白名单）" % item_text.strip())
        cfg_violations.append((attr_line, desc))

    for i, raw in enumerate(lines):
        stripped = raw.strip()
        lineno = i + 1
        # 空行与注释：不中断挂起属性（属性与项之间允许文档注释），也不视为项
        if not stripped or is_comment(stripped):
            continue
        # 内层属性（#![...]）属于文件/模块本身，跳过不挂到下一项
        if stripped.startswith("#!"):
            continue
        if stripped.startswith("#"):
            attrs, rest = split_attrs(stripped)
            if rest:
                # 单行形式：#[cfg(test)] #[path = "…"] mod foo;
                handle([(lineno, attrs)], rest, lineno)
                pending = []
            elif attrs:
                pending.append((lineno, attrs))
            # 以 # 开头但拆不出属性（如宏续行）：忽略
            continue
        # 普通项行：仅在有挂起属性时需要判定
        if pending:
            handle(pending, stripped, lineno)
            pending = []

    # 每文件至多 1 个 test 门控 mod 声明（杜绝多测试模块回潮）；
    # MULTI_DECL_WHITELIST 例外（核准的主题拆分场景，
    # 各声明仍须为合规形式——上面 handle() 已逐条校验）。
    if len(decls) > 1 and rel not in MULTI_DECL_WL:
        locs = ", ".join("%d:%s" % (l, m) for l, m in decls)
        cfg_violations.append(
            (decls[0][0], "同文件 %d 个 test 门控 mod 声明（至多 1 个）：%s"
             % (len(decls), locs)))

    # #[test] / #[tokio::test] 全文件统计（含测试模块体内）
    for i, raw in enumerate(lines):
        if TEST_ATTR_RE.search(raw):
            test_attrs.append(i + 1)

    return rel, test_attrs, cfg_violations, path_violations


def collect_rs(dirs):
    out = []
    for d in dirs:
        for dirpath, dirnames, filenames in os.walk(d):
            dirnames.sort()
            for fn in sorted(filenames):
                if fn.endswith(".rs"):
                    out.append(os.path.join(dirpath, fn))
    return out


def main():
    n_fail = 0
    for path in collect_rs(SRC_DIRS):
        rel, test_attrs, cfg_v, path_v = scan_src(path)
        if test_attrs:
            preview = ", ".join(str(l) for l in test_attrs[:5])
            more = "" if len(test_attrs) <= 5 else " …"
            print("FAIL [SRC-TEST]: %s — %d 处 #[test]/#[tokio::test]（行 %s%s）"
                  % (rel, len(test_attrs), preview, more), file=sys.stderr)
            n_fail += 1
        for line, desc in cfg_v:
            print("FAIL [SRC-CFG]: %s:%d — %s" % (rel, line, desc),
                  file=sys.stderr)
            n_fail += 1
        for line, desc in path_v:
            print("FAIL [SRC-PATH]: %s:%d — %s" % (rel, line, desc),
                  file=sys.stderr)
            n_fail += 1

    for path in collect_rs(TEST_DIRS):
        rel = os.path.relpath(path, ROOT)
        base = os.path.basename(path)
        if base.endswith("_test.rs") or rel in NAME_WL:
            continue
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                content = f.read()
        except OSError:
            continue
        if TEST_ATTR_RE.search(content):
            print("FAIL [TEST-NAME]: %s — 含 #[test] 但未以 _test.rs 命名"
                  "（辅助文件请登记 TEST_NAME_WHITELIST）" % rel, file=sys.stderr)
            n_fail += 1

    sys.exit(1 if n_fail else 0)


main()
PYEOF

# ---- 汇总 -------------------------------------------------------------------
if [ "$fail" -ne 0 ]; then
    log ""
    log "测试卫生检查未通过。"
    log "修复方向：测试代码迁入 tests/unit/<pkg>/…（<被测文件>_test.rs），"
    log "源文件尾部仅保留一行 #[cfg(test)] #[path] 声明；"
    log "系统测试迁入 tests/system/ 并以 [[test]] 接线；例外项登记白名单并评审。"
    log "目录约定与命名规则见 tests/README.md。"
    exit 1
fi

log "测试卫生检查通过：src 无内联测试，test 门控仅剩 #[path] 声明，tests/ 命名合规。"
exit 0
