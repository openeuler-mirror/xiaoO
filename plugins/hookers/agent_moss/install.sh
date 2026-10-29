#!/bin/bash
set -euo pipefail

# agent_moss 安装脚本
# 用法：bash install.sh [--enable-llm|--disable-llm]
# 创建 venv + 从 PyPI 安装 agent-moss 包 + 注册 systemd service + 启动服务。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# AgentMoss 版本（单一定义，多处使用）
AM_VERSION="0.15.1"

# 探测端口窗口与 instance 归属过滤的共享定义（含 AM_INSTANCE）。
# 本脚本全部探测与 build.sh 的 console URL 探测共用 find_instance_port，
# 归属过滤语义见 probe.sh，勿在此内联探测循环。
source "${SCRIPT_DIR}/probe.sh"

# venv 路径（systemd 与 nohup 两种模式统一用 /opt/agent_moss/venv）。
# 如果 venv 已存在，优先使用已有的，避免重复创建
VENV_DIR="/opt/agent_moss/venv"
AM_BIN="$VENV_DIR/bin/agent-moss"

# nohup 启动模式的日志文件：按实例命名，同机多 agentmoss 并存时
# 后装的实例不覆盖先装实例的日志，排障指引才不会指向别人的输出。
AM_LOG_FILE="/tmp/agentmoss-${AM_INSTANCE}.log"

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
            echo "  AM_ENABLE_LLM   设置 LLM 分析偏好（1=启用，0=禁用；未设置则不修改）"
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

# 能力检测归一：USE_SYSTEMD 无论来自 --systemd 参数、交互选择还是非
# 交互默认，只要环境没有 systemctl（容器/WSL 常见），统一在此翻转为
# nohup 模式。缺少这一步时步骤 4 会静默走 nohup 启动，而步骤 5 的失败
# 日志指引与结尾的管理命令仍按 systemd 打印——journalctl 指向不存在
# 的 unit、systemctl stop 停不掉 nohup 进程。翻转后 EXISTING_PORT 段
# 的"你选择的安装方式"也随之反映真实模式。
if [ "$USE_SYSTEMD" = true ] && ! command -v systemctl &>/dev/null; then
    echo "⚠️  未检测到 systemctl，无法注册 systemd 服务，改为直接后台启动（nohup）"
    USE_SYSTEMD=false
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

# 检测是否有本实例的服务已在运行（无论什么模式）。
# 归属过滤语义与 instance 定义见 probe.sh。
EXISTING_PORT="$(find_instance_port || true)"
if [ -n "$EXISTING_PORT" ]; then
    echo ""
    echo "⚠️  检测到已有 AgentMoss 服务运行在端口 ${EXISTING_PORT}"
    EXISTING_MODE="systemd"
    if command -v systemctl &>/dev/null && systemctl is-active agent-moss &>/dev/null 2>&1; then
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
            sudo systemctl stop agent-moss 2>/dev/null || true
            sudo systemctl disable agent-moss 2>/dev/null || true
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
# 注入归属实例标识 AGENT_MOSS_INSTANCE（AM_INSTANCE 定义见 probe.sh）。
# 多个 agentmoss 同机并存时，bridge 探测靠此字段过滤，避免漂移到别人的进程。
# systemd 模式：service 的 EnvironmentFile=-/etc/agent_moss/agent_moss.env 读这个文件；
# nohup 模式：下方 export 让进程继承。两种模式都覆盖。
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
    # 必须用 if 包裹：本脚本 set -e，裸执行失败会直接终止，走不到
    # 下面转 nohup 的兜底分支
    if sudo "$AM_BIN" install --enable 2>&1; then
        echo "✅ systemd service 已安装（instance=${AM_INSTANCE}）"
    else
        # unit 没装成时后续 systemctl start 注定失败，直接切到 nohup 模式，
        # 让步骤 4 的启动路径与结尾打印的管理命令跟实际运行方式一致
        echo "⚠️  systemd service 安装失败，将以后台模式启动"
        USE_SYSTEMD=false
    fi
else
    echo "跳过 systemd（用户选择不使用 systemd 或未检测到 systemctl）"
fi

# nohup 直启并粗等就绪；成败的最终裁决统一在步骤 5 的健康探测，本函数
# 只做两件事：spawn 进程，以及进程崩溃时尽早 tail 日志（步骤 5 只报
# "连不上"，不给原因）。端口探测复用 probe.sh 的 find_instance_port，
# 端口窗口与 --port 0 从 9090 向上找空闲端口的耦合关系见 probe.sh。
# 调用处必须带 `|| true`：在 set -e 下裸调用会在超时/崩溃时直接退出
# 脚本，跳过步骤 5 更完整的 6 轮探测与统一报错。
launch_nohup_and_wait() {
    nohup "$AM_BIN" server --host 127.0.0.1 --port 0 \
        > "$AM_LOG_FILE" 2>&1 &
    local pid=$!
    echo "等待服务启动（PID ${pid}）..."
    local i port
    for i in $(seq 1 5); do
        sleep 1
        port="$(find_instance_port || true)"
        if [ -n "$port" ]; then
            echo "✅ AgentMoss 服务已启动（PID ${pid}，端口 ${port}）"
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "❌ 进程已退出，查看日志："
            tail -10 "$AM_LOG_FILE" 2>/dev/null | sed 's/^/  /'
            return 1
        fi
        echo "   等待中...（${i}/5）"
    done
    return 1
}

# 步骤 4：启动服务
echo ""
echo "步骤 4/5：启动 AgentMoss 服务..."
if [ "$USE_SYSTEMD" = true ] && command -v systemctl &>/dev/null; then
    if sudo systemctl is-active agent-moss &>/dev/null; then
        echo "✅ AgentMoss 服务已在运行，重启以加载新版本..."
        sudo systemctl restart agent-moss 2>&1 || {
            echo "⚠️  重启失败，尝试重新启动..."
            sudo systemctl start agent-moss 2>&1 || true
        }
    else
        echo "正在通过 systemctl 启动..."
        if sudo systemctl start agent-moss 2>&1; then
            echo "✅ AgentMoss 服务已启动（systemctl）"
        else
            echo "❌ systemctl 启动失败，尝试直接后台启动..."
            # 与步骤 3 unit 安装失败的兜底同理翻转 USE_SYSTEMD：实际
            # 运行方式已变为 nohup，结尾的管理命令与健康检查失败时的
            # 日志指引必须跟实际运行方式一致，否则 systemctl stop 停
            # 不掉服务、journalctl 也看不到 nohup 的日志
            USE_SYSTEMD=false
            launch_nohup_and_wait || true
        fi
    fi
else
    echo "直接后台启动..."
    launch_nohup_and_wait || true
fi

# 步骤 5：验证
echo ""
echo "步骤 5/5：验证服务..."
FOUND_PORT=""
echo "等待服务就绪..."
for wait in $(seq 1 6); do
    FOUND_PORT="$(find_instance_port || true)"
    if [ -n "$FOUND_PORT" ]; then
        echo "✅ AgentMoss 服务健康（端口 ${FOUND_PORT}，instance=${AM_INSTANCE}）"
        echo "   Policy Console: http://127.0.0.1:${FOUND_PORT}/console"
        break
    fi
    [ $wait -lt 6 ] && echo "   等待中...（${wait}/6）" && sleep 1
done
if [ -z "$FOUND_PORT" ]; then
    echo "❌ 服务健康检查未通过，无法连接 AgentMoss（instance=${AM_INSTANCE}）"
    # 失败必须以非零退出：调用链上层的 build.sh 靠退出码决定是否报安装
    # 失败，这里吞掉会让用户看到"✅ installed & service running"的假成功
    if [ "$USE_SYSTEMD" = true ]; then
        echo "   查看日志：sudo journalctl -u agent-moss -n 20 --no-pager"
    else
        echo "   查看日志：cat ${AM_LOG_FILE}"
    fi
    exit 1
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  AgentMoss 安装完成"
echo "═══════════════════════════════════════════════════════════════"
echo "  venv:      ${VENV_DIR}"
echo "  CLI:       ${AM_BIN}"
if [ "$USE_SYSTEMD" = true ]; then
    echo "  启动：     sudo systemctl start agent-moss"
    echo "  停止：     sudo systemctl stop agent-moss"
    echo "  状态：     sudo systemctl status agent-moss"
    echo "  日志：     journalctl -u agent-moss -f"
    echo "  卸载：     sudo rm -rf /opt/agent_moss"
    echo "             sudo rm /etc/systemd/system/agent-moss.service"
    echo "             sudo systemctl daemon-reload"
else
    PROC_ID=$(pgrep -f "agent_moss.cli" 2>/dev/null | head -1 || true)
    echo "  进程 PID:  ${PROC_ID:-?}（后台进程，退出 Shell 后服务停止）"
    echo "  启动：     ${AM_BIN} server --port 0"
    echo "  停止：     kill ${PROC_ID:-PID}"
    echo "  日志：     cat ${AM_LOG_FILE}"
    echo "  卸载：     rm -rf ${VENV_DIR}"
    echo "             kill \$(pgrep -f agent_moss.cli)"
fi
echo "═══════════════════════════════════════════════════════════════"
