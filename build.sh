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

# _am_persist_llm_pref：把 L3 偏好落盘到 runtime JSON。要求 agent_moss 已 importable。
# 在服务块里"确认能 import 之后、起服务之前"调用。返回 0=成功，1=失败（仅警告不阻断）。
_am_persist_llm_pref() {
    python3 - "${ENABLE_LLM_ENV:-}" <<'PYEOF' || return 1
import sys
try:
    from agent_moss.infra.runtime_config import update_layer_enabled
    enabled = sys.argv[1] == "1"
    update_layer_enabled("L3_llm_analysis", enabled)
    print(f"  ✓ L3 偏好已写入 runtime JSON (L3_llm_analysis={enabled})")
    sys.exit(0)
except Exception as e:
    print(f"  (warn: runtime JSON 写入失败: {e})，服务将用默认 L3 设置", file=sys.stderr)
    sys.exit(1)
PYEOF
}

# ──────────────────────────────────────────────────────────────
# 用户选 Y 装 agent_moss 后：必须确保 AgentMoss 服务在跑。
# 流程：探活 → 没装则问装不装 → 装了尝试起 → 起失败终止 build.sh。
# 这是硬约束：服务没起时 xiaoO 所有操作 fail-closed 被拒，等于装了个废插件。
# ──────────────────────────────────────────────────────────────
AGENT_MOSS_HOST="${AGENT_MOSS_HOST:-127.0.0.1}"
AGENT_MOSS_PORT="${AGENT_MOSS_PORT:-9090}"
# AgentMoss 源码仓位置：优先 env，默认 ../AgentMoss（与 xiaoO 同级）。
AGENT_MOSS_HOME="${AGENT_MOSS_HOME:-$SCRIPT_DIR/../AgentMoss}"
AGENT_MOSS_LOG="/tmp/agentmoss.log"
# 探测端口范围：服务 findFreePort 从 9090 往上找空闲，所以探测扫 9090-9095
# （与 bridge.py _PROBE_PORTS、OpenDesk hook.ts GATE_PROBE_RANGE 一致）。
_AM_PROBE_PORTS=(9090 9091 9092 9093 9094 9095)

# _am_health_port：扫 _AM_PROBE_PORTS 找返回 healthy 的端口，打印端口号。
# 找不到打印空。服务可能因 9090 被占而 findFreePort 到 9091+，必须扫范围。
_am_health_port() {
    local port
    for port in "${_AM_PROBE_PORTS[@]}"; do
        if curl -sf -m 2 "http://${AGENT_MOSS_HOST}:${port}/api/v1/health" 2>/dev/null \
           | grep -q '"healthy"'; then
            echo "$port"
            return 0
        fi
    done
    return 1
}

# _am_health：探活。返回 0=任一探测端口健康，1=全部不可达。
_am_health() {
    _am_health_port >/dev/null 2>&1
}

# _am_installed：检查 agent_moss 包是否可 import。返回 0=已装，1=未装。
_am_installed() {
    python3 -c "import agent_moss" >/dev/null 2>&1
}

# _am_install：pip install -e <AgentMoss 仓>。返回 pip 退出码。
_am_install() {
    local home="$1"
    pip install -e "$home" >/dev/null 2>&1
}

# _am_start：后台起服务（走 agent_moss CLI server 命令，非裸 uvicorn）。
# 关键：走 CLI 才进 run_server，里面的 _find_free_port 才生效（9090 被占自动找空闲）。
# 裸 uvicorn --factory 不经 run_server，端口占用会直接崩。
# L3 开关偏好已落盘 runtime JSON，服务 is_layer_enabled 现读生效，不用 env。
_am_start() {
    # 先确认能 import（前置条件）
    if ! _am_installed; then
        return 1
    fi
    # nohup 后台起，走 CLI server（run_server 内 findFreePort 自动处理端口）。
    # 用 python3 -m agent_moss.cli 而非裸 uvicorn，确保 _find_free_port 生效。
    nohup python3 -m agent_moss.cli server \
        --host "${AGENT_MOSS_HOST}" --port "${AGENT_MOSS_PORT}" \
        > "${AGENT_MOSS_LOG}" 2>&1 &
    local pid=$!
    # 等服务起来（最多 8 秒轮询 health，扫 9090-9095 找 healthy）
    local i=0
    while [ $i -lt 8 ]; do
        sleep 1
        if _am_health; then
            local actual_port
            actual_port=$(_am_health_port)
            echo "AgentMoss 服务已启动 (PID ${pid}, 端口 ${actual_port:-${AGENT_MOSS_PORT}})"
            return 0
        fi
        # 进程都死了，别等了
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        i=$((i+1))
    done
    return 1
}

if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  检查 AgentMoss 服务状态..."
    echo "═══════════════════════════════════════════════════════════════"

    # 步骤 1：探活
    if _am_health; then
        echo "✅ AgentMoss 服务已在运行（${AGENT_MOSS_HOST}:${AGENT_MOSS_PORT}）。"
        # 服务在跑也落盘 L3 偏好：is_layer_enabled 每次 analyze 现读 runtime JSON，
        # 落盘后当前服务下次判定即生效，无需重起。
        _am_persist_llm_pref
    else
        # 步骤 2：服务没起，检查是否已安装
        if _am_installed; then
            echo "ℹ️  AgentMoss 已安装但服务未运行，尝试启动..."
            # 步骤 3：已装 → 落盘 L3 偏好后直接起
            _am_persist_llm_pref
            if _am_start; then
                echo "✅ AgentMoss 服务已启动。"
            else
                echo ""
                echo "❌ AgentMoss 服务启动失败，终止 build.sh。"
                echo "   诊断信息："
                echo "   - 启动命令: nohup python3 -m agent_moss.cli server \\"
                echo "               --host ${AGENT_MOSS_HOST} --port ${AGENT_MOSS_PORT} &"
                echo "   - 日志文件: ${AGENT_MOSS_LOG}"
                echo "   - 端口占用检查: ss -tlnp | grep ${AGENT_MOSS_PORT}"
                echo "   - 端口 ${AGENT_MOSS_PORT} 可能被占用，或 agent_moss 包损坏。"
                echo "   - 日志尾部："
                tail -n 15 "${AGENT_MOSS_LOG}" 2>/dev/null | sed 's/^/     /'
                exit 1
            fi
        else
            # 步骤 4：没装 → 弹窗问装不装
            echo "⚠️  AgentMoss 未安装（python3 -c 'import agent_moss' 失败）。"
            echo "   源码仓位置: ${AGENT_MOSS_HOME}"
            if [ ! -d "${AGENT_MOSS_HOME}" ]; then
                echo "   ⚠️  该路径不存在。请用环境变量指定："
                echo "       AGENT_MOSS_HOME=/path/to/AgentMoss ./build.sh --release"
            fi
            read -p "是否安装并开启 AgentMoss（pip install -e）? [Y/n]: " install_choice
            if [[ ! "$install_choice" =~ ^[Nn]$ ]]; then
                # 默认 Y：装
                echo "正在安装 AgentMoss (pip install -e ${AGENT_MOSS_HOME})..."
                _am_install "${AGENT_MOSS_HOME}"
                local_install_rc=$?
                if [ $local_install_rc -ne 0 ]; then
                    echo ""
                    echo "❌ AgentMoss 安装失败（pip exit ${local_install_rc}），终止 build.sh。"
                    echo "   手动排查：cd ${AGENT_MOSS_HOME} && pip install -e ."
                    exit 1
                fi
                echo "✅ AgentMoss 安装成功。"
                # 装完 → 落盘 L3 偏好 → 尝试起
                _am_persist_llm_pref
                echo "正在启动 AgentMoss 服务..."
                if _am_start; then
                    echo "✅ AgentMoss 服务已启动。"
                else
                    echo ""
                    echo "❌ AgentMoss 服务启动失败，终止 build.sh。"
                    echo "   安装成功但服务起不来，常见原因："
                    echo "   - 端口 ${AGENT_MOSS_PORT} 被占用: ss -tlnp | grep ${AGENT_MOSS_PORT}"
                    echo "   - 依赖缺失（uvicorn/fastapi）"
                    echo "   - 日志: ${AGENT_MOSS_LOG}"
                    echo "   - 日志尾部："
                    tail -n 15 "${AGENT_MOSS_LOG}" 2>/dev/null | sed 's/^/     /'
                    exit 1
                fi
            else
                # 用户选不装：警告放行，不终止 build（编译照常进行，插件照常注册）
                echo ""
                echo "⚠️  WARNING: 你选择不安装 AgentMoss 服务。"
                echo "   build.sh 将继续完成编译和插件注册，但："
                echo "   - AgentMoss 服务未运行，xiaoO 调工具时会被 fail-closed 拒绝（所有操作都被拦）。"
                echo "   - 之后想用，需手动："
                echo "       cd ${AGENT_MOSS_HOME} && pip install -e ."
                echo "       nohup python3 -m agent_moss.cli server \\"
                echo "         --host ${AGENT_MOSS_HOST} --port ${AGENT_MOSS_PORT} &"
                echo ""
            fi
        fi
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

# 编译完成后，注册 agent_moss 插件到 xiaoO config.toml（仅用户选 Y 时）
if [ "$INSTALL_AUDIT" = true ]; then
    echo ""
    echo "═══════════════════════════════════════════════════════════════"
    echo "  Registering agent_moss plugin into xiaoO config..."
    echo "═══════════════════════════════════════════════════════════════"

    # 通过 hookers/install.sh 注册 agent_moss 插件到 ~/.config/xiaoo/config.toml。
    # agent_moss 是纯 bridge 转发，无需 venv 安装（不像 audit_agent 有 install.sh）。
    cd "$SCRIPT_DIR/plugins/hookers"
    bash install.sh --non-interactive agent_moss
    INSTALL_EXIT_CODE=$?

    cd "$SCRIPT_DIR"

    if [ $INSTALL_EXIT_CODE -eq 0 ]; then
        echo ""
        echo "✅ agent_moss installed & service running."
        echo ""
        echo "   服务地址: http://${AGENT_MOSS_HOST}:${AGENT_MOSS_PORT} (health OK)"
        echo "   Policy Console: http://${AGENT_MOSS_HOST}:${AGENT_MOSS_PORT}/console"
        if [ "${ENABLE_LLM_ENV:-}" = "0" ]; then
            echo "   LLM 分析 (L3): 已禁用 (AGENT_MOSS_DISABLE_LLM=1)"
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
