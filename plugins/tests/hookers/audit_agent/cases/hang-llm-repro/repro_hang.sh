#!/usr/bin/env bash
# 测试：开启 L3 时 call_llm 永久阻塞（http 超时失效等效条件），验证 audit.py 不卡死。
#
# 复现方法：sitecustomize 注入，patch llm_analyzer.call_llm 为永久阻塞（worker 永不退出）。
# 这是客户"http 超时失效"的等效最终效果：L3 worker 永久卡住。
#
# 验证的修复：audit.py 末尾用 os._exit 而非 sys.exit，跳过解释器清理，
# 不等非守护 worker 线程，进程在 L3 超时附近立即退出（worker 随进程消亡）。
#
# 通过标准：exit 0（正常退出，非被外层 timeout 杀）且耗时 ≈ AUDIT_LLM_TIMEOUT。
# 失败：exit 124（被外层 timeout 杀 = 卡死）或耗时异常。
#
# 依赖：plugins/hookers/audit_agent/audit_policy_checker/venv 存在（install.sh 建立）。
set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../../../../" && pwd)"
AUDIT_PLUGIN="$REPO_ROOT/plugins/hookers/audit_agent"
AUDIT_PY="$AUDIT_PLUGIN/audit.py"
PYTHON_BIN="$AUDIT_PLUGIN/audit_policy_checker/venv/bin/python3"
LLM_TIMEOUT="${AUDIT_LLM_TIMEOUT:-10}"
OUTER_TIMEOUT="${OUTER_TIMEOUT:-30}"

if [ ! -x "$PYTHON_BIN" ]; then
  echo "❌ SKIP: venv python 不存在 ($PYTHON_BIN)，请先跑 install.sh"; exit 2
fi

TEST_CFG_DIR="$(mktemp -d)"
TEST_CFG="$TEST_CFG_DIR/config.json"
cat > "$TEST_CFG" <<"JSON"
{"llm":{"api_key":"sk-fake","model":"m","temperature":0.1,"base_url":"http://127.0.0.1:1/api/v1"},"timeout":{"total_timeout":60.0,"prompt1_timeout":30.0,"prompt2_timeout":20.0,"step_interval":0.0},"cache":{"enabled":false,"max_size":1000},"retry":{"max_retries":1,"retry_interval":1.0},"security":{"enabled":true,"heuristic_enabled":true,"logic_rules_enabled":true,"llm_analysis_enabled":true,"rules_path":"","skills_dir":""},"log_level":"INFO"}
JSON
PAYLOAD=$(cat <<"JSON"
{"stage":"pre","session_id":"s","prompt_session":"写测试文件","action_history":[],"hooker":{"id":"audit_agent","hook_point":"test.Tool.file_write.pre","command":"python3 audit.py","agent_id":"a"},"metadata":{"trace_id":"t","span_id":"s","parent_span_id":""},"call":{"call_id":"c","tool_name":"file_write","input":{"file_path":"/tmp/test_hang.txt","content":"hello"}},"policy":null,"definition":null}
JSON
)
FAKE_HOME="$(mktemp -d)"
OUT_FILE="$(mktemp)"

echo "=== 测试: L3 call_llm 永久阻塞, 期望 os._exit 自愈 (LLM_TIMEOUT=${LLM_TIMEOUT}s, 外层=${OUTER_TIMEOUT}s) ==="
START=$(date +%s)
set +e
timeout "${OUTER_TIMEOUT}s" env PYTHONPATH="$SCRIPT_DIR" HOME="$FAKE_HOME" AUDIT_CONFIG_PATH="$TEST_CFG" AUDIT_LLM_TIMEOUT="$LLM_TIMEOUT" "$PYTHON_BIN" "$AUDIT_PY" <<<"$PAYLOAD" >"$OUT_FILE" 2>&1
RC=$?
END=$(date +%s)
ELAPSED=$((END - START))

PASS=0
if [ "$RC" -eq 124 ]; then
  echo "❌ FAIL: 被外层 ${OUTER_TIMEOUT}s timeout 杀掉（audit.py 卡死，os._exit 修复无效）"
  PASS=1
elif [ "$RC" -ne 0 ]; then
  echo "❌ FAIL: 异常退出码 $RC（非 0 非 124）"
  PASS=1
elif [ "$ELAPSED" -ge "$OUTER_TIMEOUT" ]; then
  echo "❌ FAIL: 耗时 ${ELAPSED}s 达外层阈值（疑似卡死）"
  PASS=1
elif [ "$ELAPSED" -lt "$LLM_TIMEOUT" ]; then
  echo "❌ FAIL: 耗时 ${ELAPSED}s < L3 超时 ${LLM_TIMEOUT}s（疑似 call_llm 未被 patch/未进 L3）"
  PASS=1
else
  echo "✅ PASS: audit.py 在 ${ELAPSED}s 自愈退出（exit 0，≈ L3 超时 ${LLM_TIMEOUT}s）"
fi
echo "--- 输出(前500字符) ---"
head -c 500 "$OUT_FILE"
echo
rm -rf "$TEST_CFG_DIR" "$OUT_FILE" "$FAKE_HOME" 2>/dev/null || true
exit $PASS
