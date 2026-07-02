#!/bin/bash
# xiaoO Audit Dashboard 启动脚本
#
# 用法:
#   ./run_dashboard.sh          # 默认端口 9765
#   ./run_dashboard.sh 8080     # 指定端口
#   AUDIT_DASHBOARD_TOKEN=mytoken ./run_dashboard.sh  # 带 Bearer token 验证

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# audit_dashboard 包所在的父目录(plugins/hookers/audit_agent),需加入 PYTHONPATH 才能 import audit_dashboard
DASHBOARD_PARENT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
POLICY_CHECKER_DIR="$SCRIPT_DIR/../audit_policy_checker"
VENV_DIR="$POLICY_CHECKER_DIR/venv"
PORT="${1:-9765}"
HOST="${AUDIT_DASHBOARD_HOST:-127.0.0.1}"

# 设置环境变量
export AUDIT_DASHBOARD_PORT="$PORT"
export AUDIT_DASHBOARD_HOST="$HOST"

# 添加 audit_dashboard 父目录 + audit_policy_checker 到 Python 路径
export PYTHONPATH="$DASHBOARD_PARENT_DIR:$POLICY_CHECKER_DIR:$PYTHONPATH"

# 检查 Python 和依赖
PYTHON="$VENV_DIR/bin/python3"
if [ ! -f "$PYTHON" ]; then
    PYTHON="python3"
fi

# 检查 fastapi 和 uvicorn 是否安装
$PYTHON -c "import fastapi; import uvicorn" 2>/dev/null
if [ $? -ne 0 ]; then
    echo "⚠️ 缺少依赖: fastapi, uvicorn"
    echo "安装方式: pip install fastapi uvicorn"
    echo ""
    echo "正在尝试安装到 venv..."
    $VENV_DIR/bin/pip install fastapi uvicorn 2>/dev/null || pip3 install fastapi uvicorn
fi

echo "🛡️ xiaoO Audit Dashboard"
echo "   地址: http://$HOST:$PORT"
echo "   API文档: http://$HOST:$PORT/docs"
echo ""
if [ -n "$AUDIT_DASHBOARD_TOKEN" ]; then
    echo "   认证: Bearer token 已启用"
fi

# 启动服务
$PYTHON -c "
import sys
sys.path.insert(0, '$DASHBOARD_PARENT_DIR')
sys.path.insert(0, '$POLICY_CHECKER_DIR')
sys.path.insert(0, '$POLICY_CHECKER_DIR/audit_policy_checker')
from audit_dashboard.app import main
main()
"
