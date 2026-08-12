#!/bin/bash
# Test: Cerberus protects xiaoo-guardian directory
# Corresponding test case: xiaoo_guardian_protection.json

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# Path from cerberus/ to xiaoO root (7 levels up):
# cerberus/ → rules/ → xiaoo/ → agent_moss/ → tests/ → hookers/ → plugins/ → xiaoO/
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../../../../.." && pwd)"
POLICY_FILE="$PROJECT_ROOT/plugins/hookers/cerberus_bash_control/policy.toml"
CERBERUS="$HOME/.cargo/bin/cerberus"
TARGET_FILE="$HOME/.xiaoo/skills/xiaoo-guardian/SKILL.md"

echo "=== Test: Cerberus protects xiaoo-guardian directory ==="
echo "Project root: $PROJECT_ROOT"
echo "Policy file:  $POLICY_FILE"
echo "Cerberus:     $CERBERUS"
echo "Target:       $TARGET_FILE"
echo ""

# Check prerequisites
if ! command -v "$CERBERUS" &>/dev/null; then
    echo "✗ SKIP: cerberus not installed. Run setup steps in xiaoo_guardian_protection.json first."
    exit 2
fi

if [ ! -f "$POLICY_FILE" ]; then
    echo "✗ SKIP: policy.toml not found at $POLICY_FILE"
    echo "        Run setup steps in xiaoo_guardian_protection.json first."
    exit 2
fi

if [ ! -f "$TARGET_FILE" ]; then
    echo "✗ SKIP: xiaoo-guardian SKILL.md not found at $TARGET_FILE"
    exit 2
fi

# Test 1: Write to protected path should be denied
echo "[Test 1] Attempting to write to protected path..."
OUTPUT=$($CERBERUS --policy-file "$POLICY_FILE" exec -- bash -c "echo 'test' >> $TARGET_FILE" 2>&1) || true
if echo "$OUTPUT" | grep -q "Permission denied"; then
    echo "✓ PASS: Landlock blocked write to protected path"
else
    echo "✗ FAIL: Write should have been blocked"
    echo "Output: $OUTPUT"
    exit 1
fi

# Test 2: Normal write should still work
echo "[Test 2] Verifying normal write still works..."
OUTPUT=$($CERBERUS --policy-file "$POLICY_FILE" exec -- bash -c "echo 'test' >> /tmp/cerberus_test.txt && echo OK" 2>&1) || true
if echo "$OUTPUT" | grep -q "OK"; then
    echo "✓ PASS: Normal write succeeded"
else
    echo "✗ FAIL: Normal write should work"
    echo "Output: $OUTPUT"
    exit 1
fi

# Cleanup
rm -f /tmp/cerberus_test.txt

echo ""
echo "=== All tests passed ==="