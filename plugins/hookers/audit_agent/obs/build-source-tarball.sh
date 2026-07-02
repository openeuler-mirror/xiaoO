#!/bin/bash
# 从 xiaoo 仓库中提取 audit_agent 源码，生成 OBS 构建用的 tar.gz
# 用法: bash plugins/hookers/audit_agent/obs/build-source-tarball.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../../.." && pwd)"
PLUGIN_DIR="$REPO_ROOT/plugins/hookers/audit_agent"
VERSION="0.1.0"
PKG_NAME="audit-agent"
STAGING=$(mktemp -d)

trap 'rm -rf "$STAGING"' EXIT

DEST="$STAGING/$PKG_NAME-$VERSION"
mkdir -p "$DEST"

echo "=== 从 $PLUGIN_DIR 提取源码 ==="

# 1. 根目录文件
echo "  audit.py"
cp "$PLUGIN_DIR/audit.py" "$DEST/"

echo "  audit_settings.json.example"
cp "$PLUGIN_DIR/audit_settings.json.example" "$DEST/"

echo "  README.md"
cp "$PLUGIN_DIR/README.md" "$DEST/"

echo "  SECURITY_RULES.md"
cp "$PLUGIN_DIR/SECURITY_RULES.md" "$DEST/"

# 2. audit_policy_checker Python 包（排除构建产物）
echo "  audit_policy_checker/ (排除 venv, build, __pycache__, *.egg-info)"
rsync -a --delete \
    --exclude='venv/' \
    --exclude='build/' \
    --exclude='dist/' \
    --exclude='__pycache__/' \
    --exclude='*.egg-info/' \
    --exclude='.pytest_cache/' \
    --exclude='*.pyc' \
    --exclude='*.log' \
    "$PLUGIN_DIR/audit_policy_checker/" "$DEST/audit_policy_checker/"

# 2.1 audit_dashboard 控制面板包（排除构建产物）
echo "  audit_dashboard/ (排除 __pycache__, *.pyc)"
rsync -a --delete \
    --exclude='__pycache__/' \
    --exclude='*.pyc' \
    "$PLUGIN_DIR/audit_dashboard/" "$DEST/audit_dashboard/"

# 3. 生成 tar.gz
OUTPUT_DIR="$PLUGIN_DIR/obs"
mkdir -p "$OUTPUT_DIR"
OUTPUT="$OUTPUT_DIR/$PKG_NAME-$VERSION.tar.gz"

cd "$STAGING"
tar czf "$OUTPUT" "$PKG_NAME-$VERSION/"

echo ""
echo "=== 完成 ==="
echo "输出: $OUTPUT"
echo "大小: $(du -h "$OUTPUT" | cut -f1)"
echo ""
echo "=== 内容清单 ==="
tar tzf "$OUTPUT" | grep -v '/$' | sort
