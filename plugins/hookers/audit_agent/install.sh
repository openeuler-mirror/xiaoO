#!/bin/bash
set -e

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" &>/dev/null && pwd)
CHECKER_DIR="$SCRIPT_DIR/audit_policy_checker"

# 解析参数
ENABLE_LLM=""  # 空=未指定，true=启用，false=禁用

while [[ $# -gt 0 ]]; do
    case $1 in
        --enable-llm)
            ENABLE_LLM=true
            shift
            ;;
        --disable-llm)
            ENABLE_LLM=false
            shift
            ;;
        --help|-h)
            echo "Usage: $0 [--enable-llm|--disable-llm]"
            echo ""
            echo "Options:"
            echo "  --enable-llm   Enable LLM analysis (layer 3) for more comprehensive security checks"
            echo "  --disable-llm  Disable LLM analysis, only use heuristic and logic rules"
            echo "  --help, -h     Show this help message"
            echo ""
            echo "Environment variables:"
            echo "  AUDIT_ENABLE_LLM=1  Enable LLM analysis (if no command line option)"
            echo "  AUDIT_ENABLE_LLM=0  Disable LLM analysis (if no command line option)"
            echo ""
            echo "If neither option nor env var is specified, LLM analysis is enabled by default."
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# 如果命令行参数未指定，检查环境变量
if [ -z "$ENABLE_LLM" ] && [ -n "${AUDIT_ENABLE_LLM:-}" ]; then
    if [ "$AUDIT_ENABLE_LLM" = "1" ]; then
        ENABLE_LLM=true
    elif [ "$AUDIT_ENABLE_LLM" = "0" ]; then
        ENABLE_LLM=false
    fi
fi

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

# 生成 audit_settings.json（根据参数设置 AUDIT_DISABLE_LLM_LAYER3）
SETTINGS_FILE="$SCRIPT_DIR/audit_settings.json"
EXAMPLE_FILE="$SCRIPT_DIR/audit_settings.json.example"

generate_settings() {
    local disable_llm_value="$1"
    if [ -f "$EXAMPLE_FILE" ]; then
        # 从 example 复制并修改
        python3 -c "
import json
with open('$EXAMPLE_FILE', 'r') as f:
    cfg = json.load(f)
# 过滤掉 _comment 键
cfg = {k: v for k, v in cfg.items() if not k.startswith('_')}
cfg['AUDIT_DISABLE_LLM_LAYER3'] = '$disable_llm_value'
with open('$SETTINGS_FILE', 'w') as f:
    json.dump(cfg, f, indent=4, ensure_ascii=False)
"
    else
        # 创建最小配置
        python3 -c "
import json
cfg = {
    'AUDIT_DISABLE_LLM_LAYER3': '$disable_llm_value',
    'AUDIT_LLM_TIMEOUT': 300,
    'AUDIT_LOG_PATH': '',
    'AUDIT_ENABLE_POLICY_GEN': '',
    'AUDIT_CONFIG_PATH': ''
}
with open('$SETTINGS_FILE', 'w') as f:
    json.dump(cfg, f, indent=4, ensure_ascii=False)
"
    fi
}

if [ -n "$ENABLE_LLM" ]; then
    # 用户明确指定了启用或禁用
    if [ "$ENABLE_LLM" = true ]; then
        echo "LLM analysis: enabled"
        generate_settings ""
    else
        echo "LLM analysis: disabled"
        generate_settings "1"
    fi
    echo "Created audit_settings.json"
else
    # 未指定参数，默认启用 LLM
    if [ ! -f "$SETTINGS_FILE" ]; then
        if [ -f "$EXAMPLE_FILE" ]; then
            cp "$EXAMPLE_FILE" "$SETTINGS_FILE"
            echo "Created audit_settings.json from example (LLM analysis: enabled by default)"
        else
            generate_settings ""
            echo "Created audit_settings.json (LLM analysis: enabled by default)"
        fi
    else
        echo "Using existing audit_settings.json"
    fi
fi

echo "audit_agent 安装完成"
