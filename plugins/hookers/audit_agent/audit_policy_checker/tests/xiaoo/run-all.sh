#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 默认在当前目录的 target/release 或上级目录查找 xiaoo
DEFAULT_XIAOO_BIN=""
if [ -f "./target/release/xiaoo" ]; then
    DEFAULT_XIAOO_BIN="./target/release/xiaoo"
elif [ -f "../target/release/xiaoo" ]; then
    DEFAULT_XIAOO_BIN="../target/release/xiaoo"
fi

XIAOO_BIN="${XIAOO_BIN:-${DEFAULT_XIAOO_BIN:-xiaoo}}"
XIAOO_CONFIG="${XIAOO_CONFIG:-/tmp/xiaoo_config/config.toml}"
TIMEOUT="${XIAOO_TIMEOUT:-120}"
MAX_TURNS="${XIAOO_MAX_TURNS:-3}"

export XIAOO_BIN
export XIAOO_CONFIG
export XIAOO_TIMEOUT
export XIAOO_MAX_TURNS

PASS=0
FAIL=0
SKIP=0
RESULTS=()

echo "============================================"
echo "  AuditAgent xiaoo run -p 测试套件"
echo "  XIAOO_BIN: $XIAOO_BIN"
echo "  XIAOO_CONFIG: $XIAOO_CONFIG"
echo "  TIMEOUT: ${TIMEOUT}s"
echo "  MAX_TURNS: $MAX_TURNS"
echo "============================================"
echo ""

# 只运行 run-allow-*.sh 和 run-deny-*.sh，排除 run-all.sh 自身
for script in "$SCRIPT_DIR"/run-allow-*.sh "$SCRIPT_DIR"/run-deny-*.sh; do
    name=$(basename "$script" .sh)
    echo ">>> 运行: $name"
    if bash "$script"; then
        PASS=$((PASS + 1))
        RESULTS+=("✅ $name")
    else
        FAIL=$((FAIL + 1))
        RESULTS+=("❌ $name")
    fi
    echo ""
done

echo "============================================"
echo "  测试结果汇总"
echo "============================================"
for r in "${RESULTS[@]}"; do
    echo "  $r"
done
echo ""
echo "  通过: $PASS  失败: $FAIL  跳过: $SKIP"
echo "============================================"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
