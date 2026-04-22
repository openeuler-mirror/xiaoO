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
read -p "Install audit_agent? [y/N]: " choice

INSTALL_AUDIT=false
if [[ "$choice" =~ ^[Yy]$ ]]; then
    INSTALL_AUDIT=true
else
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

    cd "$SCRIPT_DIR/plugins/hookers"
    bash install.sh --non-interactive audit_agent
    INSTALL_EXIT_CODE=$?

    cd "$SCRIPT_DIR"

    if [ $INSTALL_EXIT_CODE -eq 0 ]; then
        echo ""
        echo "✅ audit_agent installed successfully."
        echo ""
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
