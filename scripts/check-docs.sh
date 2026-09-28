#!/usr/bin/env bash
# scripts/check-docs.sh
#
# 文档门禁。由 scripts/ci.sh 编排调用；编排行为与步骤表见 ci.sh 头注释。
#
# 纯离线、不联网、不编译、秒级。检查项：
#
#   [LINK]   所有 .md 中的相对链接必须可达（含图片、目录；http(s)/mailto/锚点豁免）。
#            目标按"仓库根优先、文档所在目录兜底"解析——仓库文档惯用根相对写法
#            （如 `docs/plugins.md`），也允许同目录相对写法（如 `./cli_config.md`）。
#
# 失败行的前缀（便于 ci.sh 抽取原因与读者对位）：
#   [META]   python3 缺失或扫描失败——硬失败。
#   [LINK]   相对链接指向不存在的路径。
#
# 退出码：0 通过；1 有违反。违规项逐条打印到 stderr。
#
# 设计取舍：本门禁**只查链接可达性**，不建立、也不要求 docs/ 下的文档索引。
# 文档索引曾由 docs/README.md 承担，现已移除，故不再校验索引覆盖与双语 README
# 索引对称性——避免门禁把"某个特定索引文件"变成硬约束。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"
cd "$(root_dir)"

log() { printf '%s\n' "$*" >&2; }

if ! command -v python3 >/dev/null 2>&1; then
    log "FAIL [META]: 缺少 python3，无法执行文档门禁检查"
    exit 1
fi

# 扫描与判定全部交给一段 python3：跨文件解析 Markdown 链接用 shell 做不稳
# （正则、路径归一、锚点剥离）。python 只读文件、不写盘。
python3 - <<'PY'
import os
import re
import sys

ROOT = os.getcwd()
failures = []

# ---------------------------------------------------------------- 收集 markdown
md_files = []
for dirpath, dirnames, filenames in os.walk("."):
    # 跳过构建产物与版本库内部目录；.git 下有大量 .md（模板等），不属于项目文档。
    dirnames[:] = [
        d for d in dirnames
        if d not in {"target", ".git", "node_modules", ".venv", "__pycache__"}
    ]
    for name in filenames:
        if name.endswith(".md"):
            path = os.path.normpath(os.path.join(dirpath, name))
            md_files.append(path)
md_files.sort()

# 行内链接 [text](target)；同时覆盖图片 ![alt](target)。
LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")


def read(path):
    with open(path, encoding="utf-8", errors="replace") as handle:
        return handle.read()


def resolve(source, target):
    """按仓库根优先、文档所在目录兜底解析相对链接；返回存在的路径或 None。"""
    if target.startswith(("http://", "https://", "mailto:", "#", "//")):
        return "external"
    clean = target.split("#", 1)[0].split("?", 1)[0]
    if not clean:
        return "external"
    source_dir = os.path.dirname(source) or "."
    candidates = [
        os.path.normpath(os.path.join(ROOT, clean)),   # 根相对（仓库惯例）
        os.path.normpath(os.path.join(source_dir, clean)),  # 同目录相对
    ]
    for candidate in candidates:
        if os.path.exists(candidate):
            return candidate
    return None


# ------------------------------------------------------------- [LINK] 链接可达
checked_links = 0
for md in md_files:
    for lineno, line in enumerate(read(md).splitlines(), 1):
        for target in LINK_RE.findall(line):
            result = resolve(md, target)
            if result == "external":
                continue
            checked_links += 1
            if result is None:
                failures.append(
                    f"FAIL [LINK]: {md}:{lineno} 链接目标不存在: {target}"
                )

# ------------------------------------------------------------------- 结果输出
for failure in failures:
    print(failure, file=sys.stderr)

if failures:
    print(
        f"文档门禁检查完成：扫描 {len(md_files)} 个 .md、{checked_links} 条相对链接，"
        f"{len(failures)} 项违规",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"文档门禁检查完成：扫描 {len(md_files)} 个 .md、{checked_links} 条相对链接，全部通过",
    file=sys.stderr,
)
PY
