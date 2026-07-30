#!/bin/bash
set -euo pipefail

# agent_moss 安装脚本
# 用法：bash install.sh [--enable-llm|--disable-llm]
# 创建 venv + 从 PyPI 安装 agent-moss 包 + 注册 systemd service + 启动服务。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# AgentMoss 版本（单一定义，多处使用）
# 0.10.3 起 health 带 instance 字段，配合本脚本注入 AGENT_MOSS_INSTANCE=xiaoo
# 实现多实例探测过滤（避免漂移到其他agent的agentmoss）。
AM_VERSION="0.10.4"

# venv 路径（sudo 模式用 /opt/agent_moss/venv/，非 sudo 模式 fallback 到用户目录）
# 如果 venv 已存在，优先使用已有的，避免重复创建
VENV_DIR="/opt/agent_moss/venv"
AM_BIN="$VENV_DIR/bin/agent-moss"

# 参数
ENABLE_LLM=""  # 空=不修改偏好，true=启用，false=禁用
USE_SYSTEMD="" # 空=询问用户，true=用 systemd，false=不用

# 如果通过环境变量传入 LLM 偏好（build.sh 调用 plugins/hookers/install.sh 时使用）
if [ -n "${AM_ENABLE_LLM:-}" ]; then
    if [ "$AM_ENABLE_LLM" = "1" ]; then
        ENABLE_LLM=true
    elif [ "$AM_ENABLE_LLM" = "0" ]; then
        ENABLE_LLM=false
    fi
fi

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
        --systemd)
            USE_SYSTEMD=true
            shift
            ;;
        --no-systemd)
            USE_SYSTEMD=false
            shift
            ;;
        --help|-h)
            echo "用法: $0 [options]"
            echo ""
            echo "选项:"
            echo "  --enable-llm    启用 LLM 分析（L3）"
            echo "  --disable-llm   禁用 LLM 分析"
            echo "  --systemd       安装为 systemd 服务（需要 sudo）"
            echo "  --no-systemd    不使用 systemd，直接后台启动"
            echo "  --help, -h      显示此帮助"
            echo ""
            echo "环境变量:"
            echo "  VENV_DIR        自定义 venv 路径（默认 /opt/agent_moss/venv）"
            exit 0
            ;;
        *)
            echo "未知选项: $1"
            exit 1
            ;;
    esac
done

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  AgentMoss 安装"
echo "═══════════════════════════════════════════════════════════════"

# 询问是否使用 systemd（如果用户没有指定）
# 非交互模式（无 TTY）时默认 systemd，不弹提示
if [ -z "$USE_SYSTEMD" ]; then
    if [ ! -t 0 ]; then
        USE_SYSTEMD=true
        echo "非交互模式，默认使用 systemd 方式安装"
    else
        echo ""
        echo "安装方式："
        echo "  1) 注册为 systemd 服务（需要 sudo，支持开机自启、自动重启）"
        echo "  2) 直接后台启动（无需 sudo，退出 Shell 后服务停止）"
        read -p "请选择 [1/2]（默认 1）: " sysd_choice
        if [[ "$sysd_choice" = "2" ]]; then
            USE_SYSTEMD=false
        else
            USE_SYSTEMD=true
        fi
    fi
fi

# 步骤 1：创建 venv + 安装 agent-moss 包
echo ""
echo "步骤 1/5：创建 venv 并安装 agent-moss..."

VENV_VALID=false
if [ -d "$VENV_DIR" ] && [ -x "$VENV_DIR/bin/python3" ]; then
    echo "检测到已存在的 venv，正在验证其有效性..."
    INSTALLED_VER=$("$VENV_DIR/bin/python3" -c "from agent_moss import __version__; print(__version__)" 2>/dev/null || echo "")
    if [ -n "$INSTALLED_VER" ]; then
        if [ "$INSTALLED_VER" = "$AM_VERSION" ]; then
            echo "venv 有效，agent-moss ${INSTALLED_VER} 已安装，跳过重新创建"
            VENV_VALID=true
        else
            echo "venv 中 agent-moss 版本为 ${INSTALLED_VER}，升级到 ${AM_VERSION}..."
            if sudo "$VENV_DIR/bin/pip" install --upgrade agent-moss=="${AM_VERSION}" 2>&1; then
                VENV_VALID=true
            else
                echo "⚠️ 升级失败，将重建 venv"
            fi
        fi
    else
        echo "venv 中 agent-moss 模块导入失败，尝试重新安装..."
        if sudo "$VENV_DIR/bin/pip" install --force-reinstall agent-moss=="${AM_VERSION}" 2>&1; then
            echo "✅ 重新安装成功"
            VENV_VALID=true
        else
            echo "⚠️ 重新安装失败，将重建 venv"
        fi
    fi
fi

if [ "$VENV_VALID" = false ]; then
    if [ -d "$VENV_DIR" ]; then
        echo "删除旧的 venv 并重新创建..."
        sudo rm -rf "$VENV_DIR"
    fi
    echo "创建 venv 到 ${VENV_DIR}..."
    sudo mkdir -p /opt/agent_moss
    sudo python3 -m venv "$VENV_DIR"
    echo "安装 agent-moss==${AM_VERSION}（首次安装会下载依赖，可能需要几分钟）..."
    sudo "$VENV_DIR/bin/pip" install agent-moss=="${AM_VERSION}" 2>&1 || {
        echo "❌ pip install 失败"
        exit 1
    }
    echo "✅ agent-moss 安装成功"
fi

# 步骤 2：落盘 L3 偏好（如果指定了）
if [ -n "$ENABLE_LLM" ]; then
    echo ""
    echo "步骤 2/5：写入 L3 偏好..."
    "$VENV_DIR/bin/python3" -c "
import sys
try:
    from agent_moss.infra.runtime_config import update_layer_enabled
    enabled = '${ENABLE_LLM}' == 'true'
    update_layer_enabled('L3_llm_analysis', enabled)
    print(f'  ✓ L3 偏好已写入 runtime JSON (L3_llm_analysis={enabled})')
except Exception as e:
    print(f'  (warn: runtime JSON 写入失败: {e})，服务将用默认 L3 设置', file=sys.stderr)
" || true
fi

# 检测是否有服务已在运行（无论什么模式）
# 注意：仅判断 healthy，不校验 instance 归属（安装期探测，bridge 运行期才过滤）
EXISTING_PORT=""
for port in 9090 9091 9092 9093 9094 9095; do
    if curl -sf -m 1 "http://127.0.0.1:${port}/api/v1/health" 2>/dev/null | grep -q '"healthy"'; then
        EXISTING_PORT="$port"
        break
    fi
done
if [ -n "$EXISTING_PORT" ]; then
    echo ""
    echo "⚠️  检测到已有 AgentMoss 服务运行在端口 ${EXISTING_PORT}"
    EXISTING_MODE="systemd"
    if command -v systemctl &>/dev/null && systemctl is-active agent_moss &>/dev/null 2>&1; then
        EXISTING_MODE="systemd"
    else
        EXISTING_MODE="后台进程（nohup）"
    fi
    echo "   当前运行方式: ${EXISTING_MODE}"
    echo "   你选择的安装方式: $([ "$USE_SYSTEMD" = true ] && echo 'systemd 服务' || echo '后台进程（nohup）')"
    read -p "是否停止现有服务并继续安装？[y/N]: " clean_choice
    if [[ "$clean_choice" =~ ^[Yy]$ ]]; then
        echo "正在停止现有服务..."
        if [ "$EXISTING_MODE" = "systemd" ]; then
            sudo systemctl stop agent_moss 2>/dev/null || true
            sudo systemctl disable agent_moss 2>/dev/null || true
        fi
        # 杀掉所有 agent_moss 进程
        pkill -f "agent_moss.cli" 2>/dev/null || true
        sleep 1
        echo "✅ 已停止现有服务"
    else
        echo "❌ 安装终止"
        exit 1
    fi
fi

# 步骤 3：安装 systemd service
echo ""
echo "步骤 3/5：安装 systemd service..."
# 注入归属实例标识 AGENT_MOSS_INSTANCE=xiaoo。
# 多个 agentmoss 同机并存时，bridge 探测靠此字段过滤，避免漂移到别人的进程。
# systemd 模式：service 的 EnvironmentFile=-/etc/agent_moss/agent_moss.env 读这个文件；
# nohup 模式：下方 export 让进程继承。两种模式都覆盖。
AM_INSTANCE="xiaoo"
sudo mkdir -p /etc/agent_moss
if [ -f /etc/agent_moss/agent_moss.env ]; then
    # 已有 env 文件：删旧的同名行（避免重复），再追加
    sudo sed -i '/^AGENT_MOSS_INSTANCE=/d' /etc/agent_moss/agent_moss.env
    echo "AGENT_MOSS_INSTANCE=${AM_INSTANCE}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
else
    echo "AGENT_MOSS_INSTANCE=${AM_INSTANCE}" | sudo tee /etc/agent_moss/agent_moss.env > /dev/null
fi
export AGENT_MOSS_INSTANCE="${AM_INSTANCE}"
if [ "$USE_SYSTEMD" = true ] && command -v systemctl &>/dev/null; then
    echo "正在安装 systemd service（${AM_BIN}）..."
    sudo "$AM_BIN" install --enable 2>&1
    if [ $? -eq 0 ]; then
        echo "✅ systemd service 已安装（instance=${AM_INSTANCE}）"
    else
        echo "⚠️  systemd service 安装失败，将以后台模式启动"
    fi
else
    echo "跳过 systemd（用户选择不使用 systemd 或未检测到 systemctl）"
fi

# 步骤 4：启动服务
echo ""
echo "步骤 4/5：启动 AgentMoss 服务..."
if [ "$USE_SYSTEMD" = true ] && command -v systemctl &>/dev/null; then
    if sudo systemctl is-active agent_moss &>/dev/null; then
        echo "✅ AgentMoss 服务已在运行，重启以加载新版本..."
        sudo systemctl restart agent_moss 2>&1 || {
            echo "⚠️  重启失败，尝试重新启动..."
            sudo systemctl start agent_moss 2>&1 || true
        }
    else
        echo "正在通过 systemctl 启动..."
        if sudo systemctl start agent_moss 2>&1; then
            echo "✅ AgentMoss 服务已启动（systemctl）"
        else
            echo "❌ systemctl 启动失败，尝试直接后台启动..."
            nohup "$AM_BIN" server --host 127.0.0.1 --port 0 \
                > /tmp/agentmoss.log 2>&1 &
            pid=$!
            echo "等待服务启动（PID ${pid}）..."
            for i in $(seq 1 5); do
                sleep 1
                if curl -sf -m 1 http://127.0.0.1:9090/api/v1/health 2>/dev/null | grep -q '"healthy"'; then
                    echo "✅ AgentMoss 服务已启动（PID ${pid}，端口 9090）"
                    break
                fi
                if ! kill -0 "$pid" 2>/dev/null; then
                    echo "❌ 进程已退出，查看日志："
                    tail -10 /tmp/agentmoss.log 2>/dev/null | sed 's/^/  /'
                    break
                fi
                echo "   等待中...（${i}/5）"
            done
        fi
    fi
else
    echo "直接后台启动..."
    nohup "$AM_BIN" server --host 127.0.0.1 --port 0 \
        > /tmp/agentmoss.log 2>&1 &
    echo "✅ AgentMoss 服务已启动（日志: /tmp/agentmoss.log，PID $!）"
fi

# 步骤 5：验证
echo ""
echo "步骤 5/5：验证服务..."
FOUND_PORT=""
echo "等待服务就绪..."
for wait in $(seq 1 6); do
    for port in 9090 9091 9092 9093 9094 9095; do
        if curl -sf -m 1 "http://127.0.0.1:${port}/api/v1/health" 2>/dev/null | grep -q '"healthy"'; then
            FOUND_PORT="$port"
            echo "✅ AgentMoss 服务健康（端口 ${port}）"
            echo "   Policy Console: http://127.0.0.1:${port}/console"
            break
        fi
    done
    if [ -n "$FOUND_PORT" ]; then break; fi
    [ $wait -lt 6 ] && echo "   等待中...（${wait}/5）" && sleep 1
done
if [ -z "$FOUND_PORT" ]; then
    echo "❌ 服务健康检查未通过，无法连接 AgentMoss"
    echo "   查看日志：sudo journalctl -u agent_moss -n 20 --no-pager"
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  AgentMoss 安装完成"
echo "═══════════════════════════════════════════════════════════════"
echo "  venv:      ${VENV_DIR}"
echo "  CLI:       ${AM_BIN}"
if [ "$USE_SYSTEMD" = true ]; then
    echo "  启动：     sudo systemctl start agent_moss"
    echo "  停止：     sudo systemctl stop agent_moss"
    echo "  状态：     sudo systemctl status agent_moss"
    echo "  日志：     journalctl -u agent_moss -f"
    echo "  卸载：     sudo rm -rf /opt/agent_moss"
    echo "             sudo rm /etc/systemd/system/agent_moss.service"
    echo "             sudo systemctl daemon-reload"
else
    PROC_ID=$(pgrep -f "agent_moss.cli" 2>/dev/null | head -1 || true)
    echo "  进程 PID:  ${PROC_ID:-?}（后台进程，退出 Shell 后服务停止）"
    echo "  启动：     ${AM_BIN} server --port 0"
    echo "  停止：     kill ${PROC_ID:-PID}"
    echo "  日志：     cat /tmp/agentmoss.log"
    echo "  卸载：     rm -rf ${VENV_DIR}"
    echo "             kill \$(pgrep -f agent_moss.cli)"
fi
echo "═══════════════════════════════════════════════════════════════"
