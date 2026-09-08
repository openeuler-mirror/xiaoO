#!/usr/bin/env bash
# 测试：agent_moss bridge 在 /api/v1/analyze 永久阻塞时不卡死、自愈 fail-closed deny。
#
# 这是旧 audit_agent hang-llm-repro 用例的等价重写。
#
# 旧场景（已废弃）：audit.py 用 ThreadPoolExecutor 跑 L3 call_llm，客户"http 超时失效"
# 导致 call_llm 永不返回、worker 永久卡住、进程卡死。旧用例 patch call_llm 为永久阻塞，
# 验证 audit.py 末尾 os._exit 自愈退出。
#
# agent_moss 新场景：判定逻辑迁出为常驻 HTTP 服务，xiaoO 只装瘦 bridge.py 转发。
# bridge 没有"卡住的 worker 线程"风险，但必须保证"服务 analyze 永久阻塞时 bridge 自己
# 不卡死"——urlopen 对 /api/v1/analyze 带 timeout=AGENT_MOSS_HOOK_TIMEOUT，超时后
# 抛 URLError → fail-closed deny + exit 0。
#
# 复现方法：起一个假 AgentMoss 服务（fake_agentmoss_server.py），/api/v1/health 立即
# 返回 healthy、/api/v1/analyze 永久阻塞。设 AGENT_MOSS_URL 指向它（bridge 跳过探测，
# 直接用），AGENT_MOSS_HOOK_TIMEOUT 设短（5s），让 bridge 在 ~5s 后超时自愈。
#
# 通过标准：exit 0（正常退出，非被外层 timeout 杀）、stdout 含 {"result": "deny"}、
#           耗时 ≥ AGENT_MOSS_HOOK_TIMEOUT 且 < 外层 timeout（自愈而非卡死）。
# 失败：exit 124（被外层 timeout 杀 = bridge 卡死，urlopen timeout 修复失效）。
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../../../../" && pwd)"
BRIDGE_PY="$REPO_ROOT/plugins/hookers/agent_moss/bridge.py"
FAKE_SERVER="$SCRIPT_DIR/fake_agentmoss_server.py"

# bridge 对 analyze 的超时（秒）。设短以便快速自愈。
HOOK_TIMEOUT="${AGENT_MOSS_HOOK_TIMEOUT:-5}"
# 外层杀进程的超时。必须 > HOOK_TIMEOUT + 健康检查开销 + 余量，否则会把正常的自愈误判成卡死。
OUTER_TIMEOUT="${OUTER_TIMEOUT:-30}"
FAKE_PORT="${FAKE_AGENTMOSS_PORT:-19090}"

if [ ! -f "$BRIDGE_PY" ]; then
  echo "❌ SKIP: bridge.py 不存在 ($BRIDGE_PY)，请确认 agent_moss 插件在位"; exit 2
fi
if [ ! -f "$FAKE_SERVER" ]; then
  echo "❌ SKIP: 假服务脚本不存在 ($FAKE_SERVER)"; exit 2
fi

# 起假服务（后台），测完 kill 收尾。
PYTHON_BIN="${PYTHON_BIN:-python3}"
"$PYTHON_BIN" "$FAKE_SERVER" --port "$FAKE_PORT" --analyze-hang-seconds 3600 &
FAKE_PID=$!
cleanup() {
  if kill -0 "$FAKE_PID" 2>/dev/null; then kill "$FAKE_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

# 等假服务 /api/v1/health 就绪（轮询，最多 ~5s）
READY=0
for _ in $(seq 1 25); do
  if curl -fsS "http://127.0.0.1:${FAKE_PORT}/api/v1/health" >/dev/null 2>&1; then
    READY=1; break
  fi
  sleep 0.2
done
if [ "$READY" -ne 1 ]; then
  echo "❌ SKIP: 假服务未在 5s 内就绪 (port ${FAKE_PORT})"; exit 2
fi

# 标准 tool-pre hook payload（与 xiaoo 实际发给 bridge 的结构一致）。
PAYLOAD='{"stage":"pre","session_id":"s","prompt_session":"写测试文件","action_history":[],"hooker":{"id":"agent_moss","hook_point":"*.Tool.*.pre","command":"python3 bridge.py","agent_id":"a"},"metadata":{"trace_id":"t","span_id":"s","parent_span_id":""},"call":{"call_id":"c","tool_name":"file_write","input":{"file_path":"/tmp/test_hang.txt","content":"hello"}},"policy":null,"definition":null}'

OUT_FILE="$(mktemp)"
echo "=== 测试: analyze 永久阻塞, 期望 bridge ${HOOK_TIMEOUT}s 后 fail-closed 自愈 (外层 ${OUTER_TIMEOUT}s) ==="
START=$(date +%s)
set +e
timeout "${OUTER_TIMEOUT}s" env \
  AGENT_MOSS_URL="http://127.0.0.1:${FAKE_PORT}" \
  AGENT_MOSS_HOOK_TIMEOUT="$HOOK_TIMEOUT" \
  AGENT_MOSS_HEALTH_TIMEOUT="2" \
  PYTHONPATH="" \
  "$PYTHON_BIN" "$BRIDGE_PY" <<<"$PAYLOAD" >"$OUT_FILE" 2>&1
RC=$?
END=$(date +%s)
ELAPSED=$((END - START))

PASS=0
if [ "$RC" -eq 124 ]; then
  echo "❌ FAIL: 被外层 ${OUTER_TIMEOUT}s timeout 杀掉（bridge 卡死，urlopen timeout 修复无效）"
  PASS=1
elif [ "$RC" -ne 0 ]; then
  echo "❌ FAIL: 异常退出码 $RC（非 0 非 124）"
  PASS=1
elif [ "$ELAPSED" -ge "$OUTER_TIMEOUT" ]; then
  echo "❌ FAIL: 耗时 ${ELAPSED}s 达外层阈值（疑似卡死）"
  PASS=1
elif [ "$ELAPSED" -lt "$HOOK_TIMEOUT" ]; then
  echo "❌ FAIL: 耗时 ${ELAPSED}s < HOOK_TIMEOUT ${HOOK_TIMEOUT}s（疑似未进 analyze/超时未生效）"
  PASS=1
elif ! grep -q '"result": *"deny"' "$OUT_FILE"; then
  echo "❌ FAIL: stdout 不含 fail-closed deny"
  PASS=1
else
  echo "✅ PASS: bridge 在 ${ELAPSED}s 自愈退出（exit 0，≈ HOOK_TIMEOUT ${HOOK_TIMEOUT}s，fail-closed deny）"
fi
echo "--- bridge 输出(前500字符) ---"
head -c 500 "$OUT_FILE"
echo
rm -f "$OUT_FILE" 2>/dev/null || true
exit $PASS
