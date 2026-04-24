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
    echo "Detected CI environment, skipping audit_agent installation prompt"
    exec cargo build "$@"
fi

# Detect if stdin is a TTY
if [ ! -t 0 ]; then
    echo "Non-interactive mode detected, skipping audit_agent installation prompt"
    exec cargo build "$@"
fi

# Interactive mode: ask user if they want to install audit_agent
echo ""
echo "╔═════════════════════════════════════════════════════════════╗"
echo "║  Security Plugin: audit_agent                               ║"
echo "║  Provides security audit for tool execution.                ║"
echo "║  - Helps prevent accidental execution of dangerous          ║"
echo "║    commands (rm -rf, credential leaks, etc.)                ║"
echo "║  - May increase response latency in your sessions           ║"
echo "║  - To uninstall later: ./plugins/hookers/uninstall.sh       ║"
echo "║                                                             ║"
echo "║  Install now?                                               ║"
echo "╚═════════════════════════════════════════════════════════════╝"
echo ""
read -p "Install audit_agent? [Y/n]: " choice

INSTALL_AUDIT=false
ENABLE_LLM_ENV=""  # 环境变量值：1=启用，0=禁用
if [[ "$choice" =~ ^[Nn]$ ]]; then
    # 用户明确选择不安装
    :
else
    # 默认安装（空输入或 Y/y）
    INSTALL_AUDIT=true
    # 追问是否启用 LLM 分析
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

if [ "$INSTALL_AUDIT" = false ]; then
    echo ""
    echo "⚠️  Security Notice:"
    echo "   Without audit_agent, tool execution lacks security audit."
    echo "   This may expose your system to potential risks."
    echo ""
    echo "   To install later, run:"
    echo "   ./plugins/hookers/install.sh --non-interactive audit_agent"
    echo ""
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

# After build completes, install audit_agent if user chose to
if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Installing audit_agent..."
    echo "═══════════════════════════════════════════════════════════════"

    # 先通过 hookers/install.sh 注册插件（会自动调用 audit_agent/install.sh）
    cd "$SCRIPT_DIR/plugins/hookers"
    AUDIT_ENABLE_LLM="$ENABLE_LLM_ENV" bash install.sh --non-interactive audit_agent
    INSTALL_EXIT_CODE=$?

    cd "$SCRIPT_DIR"

    if [ $INSTALL_EXIT_CODE -eq 0 ]; then
        echo ""
        echo "✅ audit_agent installed successfully."
        echo ""
        if [ "$ENABLE_LLM_ENV" = "1" ]; then
            echo "LLM analysis is enabled."
            echo ""
            echo "To disable LLM analysis later:"
            echo "  - Set environment variable: export AUDIT_DISABLE_LLM_LAYER3=1"
            echo "  - Or edit: plugins/hookers/audit_agent/audit_settings.json"
            echo ""
        elif [ "$ENABLE_LLM_ENV" = "0" ]; then
            echo "LLM analysis is disabled."
            echo ""
            echo "To enable LLM analysis later:"
            echo "  - Remove AUDIT_DISABLE_LLM_LAYER3 from environment"
            echo "  - Or edit: plugins/hookers/audit_agent/audit_settings.json"
            echo ""
        fi
        echo "To uninstall later, run:"
        echo "  ./plugins/hookers/uninstall.sh"
        echo ""
    else
        echo ""
        echo "❌ audit_agent installation failed."
        echo ""
    fi
else
    # Print final security notice after build
    echo ""
    echo "╔═════════════════════════════════════════════════════════════╗"
    echo "║  ⚠️  Build Complete - Security Notice                       ║"
    echo "╠═════════════════════════════════════════════════════════════╣"
    echo "║  audit_agent is NOT installed.                              ║"
    echo "║  Your tool execution lacks security audit.                  ║"
    echo "║                                                             ║"
    echo "║  To install, run:                                           ║"
    echo "║  ./plugins/hookers/install.sh --non-interactive audit_agent ║"
    echo "╚═════════════════════════════════════════════════════════════╝"
    echo ""
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  Build complete."
echo "═══════════════════════════════════════════════════════════════"
