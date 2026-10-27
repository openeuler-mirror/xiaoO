#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Check if called from project root.
# `$SCRIPT_DIR/Cargo.toml` is the script's OWN directory and always exists, so it
# could never catch the case this guard is for. What matters is the working
# directory: `cargo build` and the plugin installer below both run in `$PWD`.
if [ ! -f Cargo.toml ]; then
    echo "Error: build.sh must be run from the project root directory (no Cargo.toml in $PWD)" >&2
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
# NOTE: the agent_moss decision logic lives in a standalone resident HTTP
# service (the AgentMoss server). xiaoO only installs a thin bridge plugin
# (bridge.py) that forwards to it. So "installing agent_moss" = (1) register
# the bridge plugin in the xiaoO config + (2) make sure the AgentMoss
# service is running. User expectation: choosing Y must bring the service up
# automatically — probe → ask to install if absent → try to start after
# install → abort the build if starting fails.
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
    # The user explicitly declined the install.
    :
else
    # Default: install (empty input or Y/y).
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
# Second question: enable LLM tier-3 analysis?
# Key point: the preference is persisted to the runtime JSON
# (~/.config/agentmoss/agent_moss_runtime.json, layers.L3_llm_analysis), not
# an exported environment variable — env only lives inside this build.sh
# shell, and a service started from another terminal or by systemd would
# never see it, making the choice pointless. Once persisted, whoever starts
# the service and whenever, it reads the same preference from the runtime
# JSON. Priority stays env > runtime JSON > settings.json > default; env
# being highest does not conflict.
#
# The question itself directly follows the first one to keep the two-step
# rhythm; the runtime-JSON write is deferred until the service block
# confirms agent_moss is importable — without agent_moss installed the
# runtime JSON cannot be written, step 4 installs it first and persists
# afterwards. So we only record ENABLE_LLM_ENV here and persist later.
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
# `cargo build` failing would abort the script here under `set -e`, leaving the
# report below unreachable. `|| CODE=$?` keeps errexit on for everything else
# while still letting this branch run.
BUILD_EXIT_CODE=0
cargo build "$@" || BUILD_EXIT_CODE=$?

if [ $BUILD_EXIT_CODE -ne 0 ]; then
    echo "cargo build failed with exit code $BUILD_EXIT_CODE"
    exit $BUILD_EXIT_CODE
fi

# After compilation: install the agent_moss service + register the plugin in
# the xiaoO config.toml (only when the user chose Y).
if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  安装 AgentMoss 服务 + 注册插件..."
    echo "═══════════════════════════════════════════════════════════════"

    # Pass the LLM preference to install.sh via environment variable;
    # plugins/hookers/install.sh calls agent_moss/install.sh automatically.
    export AM_ENABLE_LLM="${ENABLE_LLM_ENV:-}"
    cd "$SCRIPT_DIR/plugins/hookers"
    REGISTER_EXIT_CODE=0
    bash install.sh --non-interactive agent_moss || REGISTER_EXIT_CODE=$?
    cd "$SCRIPT_DIR"

    if [ $REGISTER_EXIT_CODE -eq 0 ]; then
        echo ""
        echo "✅ agent_moss installed & service running."
        echo ""
        # Probe the actual port dynamically (same logic as install.sh step 5):
        # agent-moss server --port 0 makes findFreePort scan upward from 9090,
        # so co-located instances may land on 9091+. Only services that are
        # healthy AND instance==xiaoo count, so we never drift to another
        # agent's agentmoss on the same host.
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
