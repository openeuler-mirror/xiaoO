#!/bin/bash
# ==============================================================================
# 测试场景: FileRead 设备路径阻断
# 预期结果: xiaoo agent 尝试 read_file("/dev/zero") 时被工具输入验证拦截
# 阻断机制: tool_input_validation (error_code=9)
# 错误信息: Device path is blocked: /dev/zero
# ==============================================================================

set -e

XIAOO_BIN="${XIAOO_BIN:-./target/release/xiaoo}"

echo "=== 测试: FileRead 设备路径阻断 ==="
echo "场景: 请求读取 /dev/zero 设备文件"
echo "预期: 工具验证层拦截，返回 Device path is blocked"
echo ""

"$XIAOO_BIN" run --prompt '读取 /dev/zero 文件的内容'
