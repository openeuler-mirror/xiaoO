#!/bin/bash
set -e

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
CHECKER_DIR="$SCRIPT_DIR/audit_policy_checker"

cd "$CHECKER_DIR"

# 检查 venv 是否存在且合法
VENV_VALID=false

if [ -d "venv" ] && [ -f "venv/bin/activate" ]; then
    echo "检测到已存在的 venv，正在验证其有效性..."

    # 检查 venv 中的 Python 是否可用
    if [ -x "venv/bin/python3" ]; then
        # 检查已安装的包中是否包含 audit_policy_checker
        # 这能检测出重命名目录后 venv 路径不匹配的问题
        INSTALLED_PACKAGES=$(./venv/bin/pip list --format=freeze 2>/dev/null || echo "")
        if echo "$INSTALLED_PACKAGES" | grep -qi "audit-policy-checker"; then
            # 进一步检查：尝试验证模块导入是否正常
            if ./venv/bin/python3 -c "import audit_policy_checker" 2>/dev/null; then
                echo "venv 有效，跳过重新创建"
                VENV_VALID=true
            else
                echo "venv 模块导入失败，需要重新创建"
            fi
        else
            echo "venv 中未找到 audit_policy_checker 包，需要重新创建"
        fi
    else
        echo "venv 中的 Python 不可用，需要重新创建"
    fi
fi

if [ "$VENV_VALID" = false ]; then
    echo "删除旧的 venv 并重新创建..."
    rm -rf venv
    python3 -m venv venv
fi

# 激活虚拟环境并安装/更新包
source venv/bin/activate
pip install .

echo "audit_agent 安装完成"
