#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check if called from project root
if [ ! -f "$SCRIPT_DIR/Cargo.toml" ]; then
    echo "Error: build.sh must be run from the project root directory"
    exit 1
fi

# Detect if we're in CI mode
if [ -n "${CI:-}" ]; then
    echo "Detected CI environment, skipping agent_moss installation prompt"
    exec cargo build "$@"
fi

# Detect if stdin is a TTY
if [ ! -t 0 ]; then
    echo "Non-interactive mode detected, skipping agent_moss installation prompt"
    exec cargo build "$@"
fi

# Interactive mode: ask user if they want to install agent_moss
# NOTE: agent_moss 是 audit_agent 的后继者——判定逻辑已迁出为独立常驻 HTTP 服务
# （AgentMoss server）。xiaoO 只装一个瘦 bridge 插件（bridge.py）转发到服务。
# 所以"装 agent_moss"= ①注册 bridge 插件到 xiaoO config + ②确保 AgentMoss 服务在跑。
# 用户的诉求：选 Y 后必须自动把服务拉起来——探活→没装则问装不装→装了尝试起→起失败终止 build。
echo ""
echo "╔═════════════════════════════════════════════════════════════╗"
echo "║  Security Plugin: agent_moss                                ║"
echo "║  Audit tool execution via the AgentMoss HTTP service.        ║"
echo "║  - Helps prevent accidental execution of dangerous          ║"
echo "║    commands (rm -rf, credential leaks, etc.)                ║"
echo "║  - Requires the AgentMoss server running (see below)         ║"
echo "║  - To uninstall later: ./plugins/hookers/uninstall.sh      ║"
echo "║                                                             ║"
echo "║  Install now?                                              ║"
echo "╚═════════════════════════════════════════════════════════════╝"
echo ""
read -p "Install agent_moss? [Y/n]: " choice

INSTALL_AUDIT=false
if [[ "$choice" =~ ^[Nn]$ ]]; then
    # 用户明确选择不安装
    :
else
    # 默认安装（空输入或 Y/y）
    INSTALL_AUDIT=true
fi

if [ "$INSTALL_AUDIT" = false ]; then
    echo ""
    echo "⚠️  Security Notice:"
    echo "   Without agent_moss, tool execution lacks security audit."
    echo "   This may expose your system to potential risks."
    echo ""
    echo "   To install later, run:"
    echo "   ./plugins/hookers/install.sh --non-interactive agent_moss"
    echo ""
fi

# ──────────────────────────────────────────────────────────────
# 第二次问：是否启用 LLM 第三层分析（与老版 audit_agent 一致，两次问的节奏不变）。
# 关键：偏好落盘到 runtime JSON（~/.config/agentmoss/agent_moss_runtime.json 的
# layers.L3_llm_analysis），不能用 export 环境变量——env 只在 build.sh 这个 shell
# 进程内有效，用户在别的终端起服务、或 systemd 起服务都读不到，等于白选。
# 落盘后，服务不管谁起、何时起，都从同一个 runtime JSON 读到这个偏好。
# 优先级仍是 env > runtime JSON > settings.json > 默认，env 最高不冲突。
#
# 注意：问（读用户输入）紧跟第一次问，保持两次问的节奏；但"落盘 runtime JSON"
# 推迟到服务块里 agent_moss 确认能 import 之后——没装 agent_moss 时写不了 runtime JSON，
# 步骤4 会先装，装完再落盘。所以这里只记 ENABLE_LLM_ENV，落盘延后。
# ──────────────────────────────────────────────────────────────
ENABLE_LLM_ENV=""  # 1=启用 L3，0=禁用 L3
if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "╔═════════════════════════════════════════════════════════════╗"
    echo "║  LLM Analysis (Layer 3)                                     ║"
    echo "║  - Uses LLM to detect complex security threats              ║"
    echo "║  - More comprehensive but increases latency (~5-30s)        ║"
    echo "║  - Can intercept attacks that heuristic rules miss          ║"
    echo "║                                                             ║"
    echo "║  Enable LLM analysis?                                       ║"
    echo "╚═════════════════════════════════════════════════════════════╝"
    echo ""
    read -p "Enable LLM analysis? [y/N]: " llm_choice
    if [[ "$llm_choice" =~ ^[Yy]$ ]]; then
        ENABLE_LLM_ENV="1"
        echo "LLM analysis will be enabled."
    else
        ENABLE_LLM_ENV="0"
        echo "LLM analysis will be disabled."
    fi
fi

# Run cargo build
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  Running cargo build $@..."
echo "═══════════════════════════════════════════════════════════════"
cargo build "$@"
BUILD_EXIT_CODE=$?

if [ $BUILD_EXIT_CODE -ne 0 ]; then
    echo "cargo build failed with exit code $BUILD_EXIT_CODE"
    exit $BUILD_EXIT_CODE
fi

# 编译完成后，安装 agent_moss 服务 + 注册插件到 xiaoO config.toml（仅用户选 Y 时）
if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  安装 AgentMoss 服务 + 注册插件..."
    echo "═══════════════════════════════════════════════════════════════"

    # 通过环境变量传递 LLM 偏好给 install.sh
    # plugins/hookers/install.sh 会自动调用 agent_moss/install.sh
    export AM_ENABLE_LLM="${ENABLE_LLM_ENV:-}"
    cd "$SCRIPT_DIR/plugins/hookers"
    bash install.sh --non-interactive agent_moss
    REGISTER_EXIT_CODE=$?
    cd "$SCRIPT_DIR"

    if [ $REGISTER_EXIT_CODE -eq 0 ]; then
        echo ""
        echo "✅ agent_moss installed & service running."
        echo ""
        # 动态探测实际端口（与 install.sh 步骤5 同源逻辑）：agent-moss server --port 0
        # findFreePort 从 9090 往上找空闲，同机多实例并存时可能落在 9091+。只认
        # healthy 且 instance==xiaoo 的服务，避免漂移到同机其他 agent 的 agentmoss。
        AM_CONSOLE_URL="http://127.0.0.1:9090/console"  # 探测失败兜底
        for port in 9090 9091 9092 9093 9094 9095; do
            if curl -sf -m 1 "http://127.0.0.1:${port}/api/v1/health" 2>/dev/null \
                | grep -q '"instance":"xiaoo"'; then
                AM_CONSOLE_URL="http://127.0.0.1:${port}/console"
                break
            fi
        done
        echo "   Policy Console: ${AM_CONSOLE_URL}"
        if [ "${ENABLE_LLM_ENV:-}" = "0" ]; then
            echo "   LLM 分析 (L3): 已禁用"
        else
            echo "   LLM 分析 (L3): 已启用"
        fi
        echo ""
        echo "   To uninstall later, run:"
        echo "     ./plugins/hookers/uninstall.sh"
        echo ""
    else
        echo ""
        echo "❌ agent_moss plugin registration failed."
        echo ""
    fi
else
    # Print final security notice after build
    echo ""
    echo "╔═════════════════════════════════════════════════════════════╗"
    echo "║  ⚠️  Build Complete - Security Notice                       ║"
    echo "╠═════════════════════════════════════════════════════════════╣"
    echo "║  agent_moss is NOT installed.                              ║"
    echo "║  Your tool execution lacks security audit.                  ║"
    echo "║                                                             ║"
    echo "║  To install, run:                                           ║"
    echo "║  ./plugins/hookers/install.sh --non-interactive agent_moss ║"
    echo "╚═════════════════════════════════════════════════════════════╝"
    echo ""
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  Build complete."
echo "═══════════════════════════════════════════════════════════════"
