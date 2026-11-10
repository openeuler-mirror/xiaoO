#!/bin/bash
set -euo pipefail

# agent_moss 安装脚本
# 用法：bash install.sh [--enable-llm|--disable-llm]
# 创建 venv + 从 PyPI 安装 agent-moss 包 + 注册 systemd service + 启动服务。
# 慢网/内网可用 PIP_INDEX_URL / PIP_TRUSTED_HOST 指定镜像（sudo 下会显式透传）。

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# AgentMoss 版本（单一定义，多处使用）
# 0.10.3 起 health 带 instance 字段，配合本脚本注入 AGENT_MOSS_INSTANCE=xiaoo
# 实现多实例探测过滤（避免漂移到其他agent的agentmoss）。
AM_VERSION="0.17.21"

# 探测端口窗口与 instance 归属过滤的共享定义（含 AM_INSTANCE）。
# 本脚本全部探测与 build.sh 的 console URL 探测共用 find_instance_port，
# 归属过滤语义见 probe.sh，勿在此内联探测循环。
source "${SCRIPT_DIR}/probe.sh"

# 鉴权 token 文件。AgentMoss 0.16.14 起默认开启鉴权（auto 生成 token）：
# bridge.py 不带 Bearer 会 401，而 bridge 的失败分支是 fail-closed，
# 等于所有工具调用都被拒。这里生成一份固定 token：
#   - systemd 模式以 root 运行 AgentMoss，root 可读用户目录下的 0600 文件；
#   - nohup 模式以当前用户运行，同属主自然可读。
# bridge.py 的 _resolve_auth_token() 默认读同一路径。
_AM_RUN_HOME="$HOME"
if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    _AM_RUN_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6)"
    [ -n "$_AM_RUN_HOME" ] || _AM_RUN_HOME="$HOME"
fi
AM_AUTH_TOKEN_FILE="${_AM_RUN_HOME}/.config/agentmoss/xiaoo_token"
# AgentMoss 0.17.22 起 root 启动不再猜测「真实用户」，数据目录固定为当前用户的
# ~/.config/agentmoss（root 下即 /root）。这里显式指定数据目录，保证 sudo/systemd
# 安装时 config/runtime/token 仍落在安装用户目录，与升级前行为一致。
AM_DATA_DIR="${_AM_RUN_HOME}/.config/agentmoss"
mkdir -p "$(dirname "${AM_AUTH_TOKEN_FILE}")" 2>/dev/null || true
if [ ! -s "${AM_AUTH_TOKEN_FILE}" ]; then
    ( umask 077
      if command -v python3 >/dev/null 2>&1; then
          python3 -c 'import secrets; print(secrets.token_hex(32))' > "${AM_AUTH_TOKEN_FILE}"
      else
          od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "${AM_AUTH_TOKEN_FILE}"
      fi )
fi
chmod 600 "${AM_AUTH_TOKEN_FILE}" 2>/dev/null || true
if [ "$(id -u)" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
    chown "$SUDO_USER" "${AM_AUTH_TOKEN_FILE}" 2>/dev/null || true
fi

# venv 路径与提权前缀在「安装方式」确定后按模式设置（见下方 USE_SYSTEMD 决策块之后）：
#   方式 1（systemd）：/opt/agent_moss/venv + sudo
#   方式 2（nohup）  ：~/.agent_moss/venv + 不提权
# 可用环境变量 VENV_DIR 显式覆盖（自建路径的写权限由使用者自担）。

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
            echo "  VENV_DIR        自定义 venv 路径（默认：systemd 模式 /opt/agent_moss/venv，nohup 模式 ~/.agent_moss/venv）"
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

# ---- 按安装模式确定 venv 路径与提权前缀 ----
# 方式 2（nohup）不写系统目录、无需 root：venv 放用户主目录，命令一律不加 sudo。
if [ -z "${VENV_DIR:-}" ]; then
    if [ "$USE_SYSTEMD" = true ]; then
        VENV_DIR="/opt/agent_moss/venv"
    else
        VENV_DIR="$HOME/.agent_moss/venv"
    fi
fi
AM_BIN="$VENV_DIR/bin/agent-moss"
# 提权前缀：空值时 "$SUDO cmd" 等价直接执行 cmd（非 sudo 模式零提权）。
if [ "$USE_SYSTEMD" = true ]; then SUDO="sudo"; else SUDO=""; fi

# ---- pip 源透传（慢网/内网必备）----
# sudo 默认 env_reset 且 HOME=/root，读不到当前用户 ~/.config/pip/pip.conf 里的
# index-url，pip 会静默回落到 pypi.org。慢网下大依赖（pydantic 等）读超时后，
# pip 报的是 "Could not find a version ... (from versions: none)" —— 看起来像
# 版本不存在，实际是索引没拉到，极易误判。
#
# 优先级：
#   1) 显式设了 PIP_INDEX_URL → 只透传它（及 PIP_TRUSTED_HOST）；
#   2) 否则把当前用户的 pip 配置文件整体透传（PIP_CONFIG_FILE），
#      让 index-url / timeout / retries / trusted-host 全部生效；
#   3) 两者都没有 → 不传任何参数，保持默认行为。
PIP_FWD=()
if [ -n "${PIP_INDEX_URL:-}" ]; then
    PIP_FWD+=("PIP_INDEX_URL=$PIP_INDEX_URL")
    if [ -n "${PIP_TRUSTED_HOST:-}" ]; then PIP_FWD+=("PIP_TRUSTED_HOST=$PIP_TRUSTED_HOST"); fi
else
    _USER_PIP_CONF="${PIP_CONFIG_FILE:-}"
    if [ -z "$_USER_PIP_CONF" ]; then
        for _c in "$HOME/.config/pip/pip.conf" "$HOME/.pip/pip.conf"; do
            if [ -f "$_c" ]; then _USER_PIP_CONF="$_c"; break; fi
        done
    fi
    if [ -n "$_USER_PIP_CONF" ] && [ -f "$_USER_PIP_CONF" ]; then
        PIP_FWD+=("PIP_CONFIG_FILE=$_USER_PIP_CONF")
    fi
fi
PIP_ENV=()
if [ ${#PIP_FWD[@]} -gt 0 ]; then PIP_ENV=(env "${PIP_FWD[@]}"); fi

# pip 安装（先按配置的源；失败自动回退官方 PyPI 重试）。
# 镜像站对新版本有同步延迟（2026-11 实测：0.17.21 发布后华为源仍缺该版本），
# 回退能避免「新版本装不到 → 升级失败」。
pip_install_safe() {
    if $SUDO "${PIP_ENV[@]}" "$VENV_DIR/bin/pip" install --disable-pip-version-check "$@" 2>&1; then
        return 0
    fi
    echo "⚠️  当前 pip 源安装失败，自动回退官方 PyPI 重试..."
    if $SUDO env PIP_INDEX_URL=https://pypi.org/simple/ \
        "$VENV_DIR/bin/pip" install --disable-pip-version-check "$@" 2>&1; then
        return 0
    fi
    return 1
}

# 步骤 1：创建 venv + 安装 agent-moss 包
echo ""
echo "步骤 1/5：创建 venv 并安装 agent-moss..."

VENV_VALID=false
# 记录「本次安装是否发生了实质变更」——没有变更就不重启服务（避免每次构建都闪断）。
MOSS_NEEDS_RESTART=false
if [ -d "$VENV_DIR" ] && [ -x "$VENV_DIR/bin/python3" ]; then
    echo "检测到已存在的 venv，正在验证其有效性..."
    # 读**发行元数据**而不是 `from agent_moss import __version__`：
    # 2026-11 事故：0.17.20 的 wheel 元数据是 0.17.20 但包内 __version__.py 仍是
    # 0.17.19，两个源不一致导致「判定要升级 → pip 说已满足」无限循环。
    # importlib.metadata 是 pip 安装结果的事实源，-I 同时隔离 PYTHONPATH/用户站点。
    INSTALLED_VER=$("$VENV_DIR/bin/python3" -I -c "from importlib.metadata import version; print(version('agent-moss'))" 2>/dev/null || echo "")
    if [ -n "$INSTALLED_VER" ]; then
        if [ "$INSTALLED_VER" = "$AM_VERSION" ]; then
            echo "venv 有效，agent-moss ${INSTALLED_VER} 已安装，跳过重新创建"
            VENV_VALID=true
        else
            if pip_install_safe --upgrade --quiet agent-moss=="${AM_VERSION}"; then
                # pip 成功后按元数据复核：确认真的换上了
                INSTALLED_VER=$("$VENV_DIR/bin/python3" -I -c "from importlib.metadata import version; print(version('agent-moss'))" 2>/dev/null || echo "")
                if [ "$INSTALLED_VER" = "$AM_VERSION" ]; then
                    echo "✅ agent-moss 已升级到 ${AM_VERSION}"
                    MOSS_NEEDS_RESTART=true
                else
                    echo "⚠️ 升级后版本仍为 ${INSTALLED_VER}（继续使用现版本）"
                fi
            else
                # 不删 venv：源暂时不可达时保留可用环境，比"删掉再装失败"安全
                echo "⚠️ 升级失败，继续使用已安装的 ${INSTALLED_VER}；请检查网络/镜像源后重试"
            fi
            VENV_VALID=true
        fi
    else
        echo "venv 中 agent-moss 模块导入失败，尝试重新安装..."
        if pip_install_safe --force-reinstall agent-moss=="${AM_VERSION}"; then
            echo "✅ 重新安装成功"
            VENV_VALID=true
            MOSS_NEEDS_RESTART=true
        else
            echo "⚠️ 重新安装失败，将重建 venv"
        fi
    fi
fi

if [ "$VENV_VALID" = false ]; then
    if [ -d "$VENV_DIR" ]; then
        echo "删除旧的 venv 并重新创建..."
        $SUDO rm -rf "$VENV_DIR"
    fi
    echo "创建 venv 到 ${VENV_DIR}..."
    $SUDO mkdir -p "$(dirname "$VENV_DIR")"
    $SUDO python3 -m venv "$VENV_DIR"
    echo "安装 agent-moss==${AM_VERSION}（首次安装会下载依赖，可能需要几分钟）..."
    pip_install_safe agent-moss=="${AM_VERSION}" || {
        echo "❌ pip install 失败"
        echo "   慢网/内网环境请先设镜像再重跑："
        echo "   export PIP_INDEX_URL=https://mirrors.aliyun.com/pypi/simple/"
        echo "   export PIP_TRUSTED_HOST=mirrors.aliyun.com"
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
        # 杀掉 Python 版 agentmoss 进程。进程命令行实际形态是
        # `<venv>/bin/agent-moss server ...`（entry point），旧版才是
        # `agent_moss.cli` 模块路径——两种都杀，否则重装时旧服务不会被停
        # （端口占用、新旧并存）。TS 版（OpenDesk）是 node 进程，不受影响。
        pkill -f "bin/agent-moss server" 2>/dev/null || true
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
if [ "$USE_SYSTEMD" = true ]; then
    sudo mkdir -p /etc/agent_moss
    # 记录 env 指纹：内容变了才需要重启服务（避免每次安装都闪断）
    _ENV_SUM_BEFORE=$(sudo sha256sum /etc/agent_moss/agent_moss.env 2>/dev/null | cut -d' ' -f1 || true)
    # 日志白名单：AgentMoss 0.17.0 起校验 llm_log_path，root 服务默认只允许
    # /root + /tmp。systemd 模式下服务是 root、bridge 是当前用户，日志必然落在
    # 当前用户家目录 → 必须显式授权。
    # 多用户共享同一 systemd 服务：每个用户装一次，把自己的日志根目录**累积**
    # 进白名单（去重、不覆盖），否则后装的人会把前一个人的挤掉。
    _AM_LOG_ROOT="${AGENT_MOSS_LOG_PATH:+$(dirname "${AGENT_MOSS_LOG_PATH}")}"
    _AM_LOG_ROOT="${_AM_LOG_ROOT:-$HOME}"
    if [ -f /etc/agent_moss/agent_moss.env ]; then
        # 已有 env 文件：删旧的同名行（避免重复），再追加
        sudo sed -i '/^AGENT_MOSS_INSTANCE=/d' /etc/agent_moss/agent_moss.env
        echo "AGENT_MOSS_INSTANCE=${AM_INSTANCE}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
        sudo sed -i '/^AGENT_MOSS_AUTH_TOKEN_FILE=/d' /etc/agent_moss/agent_moss.env
        echo "AGENT_MOSS_AUTH_TOKEN_FILE=${AM_AUTH_TOKEN_FILE}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
        sudo sed -i '/^AGENT_MOSS_DATA_DIR=/d' /etc/agent_moss/agent_moss.env
        echo "AGENT_MOSS_DATA_DIR=${AM_DATA_DIR}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
        _AM_OLD_ROOTS=$(sudo sed -n 's/^AGENT_MOSS_LOG_ROOTS=//p' /etc/agent_moss/agent_moss.env 2>/dev/null || true)
        case ",${_AM_OLD_ROOTS}," in
            *",${_AM_LOG_ROOT},"*) ;;  # 已授权，跳过
            *)
                _AM_NEW_ROOTS="${_AM_OLD_ROOTS:+${_AM_OLD_ROOTS},}${_AM_LOG_ROOT}"
                sudo sed -i '/^AGENT_MOSS_LOG_ROOTS=/d' /etc/agent_moss/agent_moss.env
                echo "AGENT_MOSS_LOG_ROOTS=${_AM_NEW_ROOTS}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
                ;;
        esac
    else
        echo "AGENT_MOSS_INSTANCE=${AM_INSTANCE}" | sudo tee /etc/agent_moss/agent_moss.env > /dev/null
        echo "AGENT_MOSS_AUTH_TOKEN_FILE=${AM_AUTH_TOKEN_FILE}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
        echo "AGENT_MOSS_DATA_DIR=${AM_DATA_DIR}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
        echo "AGENT_MOSS_LOG_ROOTS=${_AM_LOG_ROOT}" | sudo tee -a /etc/agent_moss/agent_moss.env > /dev/null
    fi
    _ENV_SUM_AFTER=$(sudo sha256sum /etc/agent_moss/agent_moss.env 2>/dev/null | cut -d' ' -f1 || true)
    if [ "$_ENV_SUM_BEFORE" != "$_ENV_SUM_AFTER" ]; then
        MOSS_NEEDS_RESTART=true
    fi
fi
export AGENT_MOSS_INSTANCE="${AM_INSTANCE}"
export AGENT_MOSS_AUTH_TOKEN_FILE="${AM_AUTH_TOKEN_FILE}"
export AGENT_MOSS_DATA_DIR="${AM_DATA_DIR}"
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
        if [ "$MOSS_NEEDS_RESTART" = true ]; then
            echo "检测到版本/配置变更，重启 AgentMoss 加载新版本..."
            sudo systemctl restart agent-moss 2>&1 || {
                echo "⚠️  重启失败，尝试重新启动..."
                sudo systemctl start agent-moss 2>&1 || true
            }
        else
            echo "✅ AgentMoss 服务运行中，版本与配置未变，跳过重启"
        fi
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
    PROC_ID=$(pgrep -f "bin/agent-moss server" 2>/dev/null | head -1 || true)
    echo "  进程 PID:  ${PROC_ID:-?}（后台进程，退出 Shell 后服务停止）"
    echo "  启动：     ${AM_BIN} server --port 0"
    echo "  停止：     kill ${PROC_ID:-PID}"
    echo "  日志：     cat ${AM_LOG_FILE}"
    echo "  卸载：     rm -rf ${VENV_DIR}"
    echo "             pkill -f 'bin/agent-moss server'"
fi
echo "═══════════════════════════════════════════════════════════════"
