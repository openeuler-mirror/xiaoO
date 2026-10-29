# AgentMoss 实例健康探测的共享库，供本目录 install.sh 与仓库根 build.sh
# source。单一事实来源：AM_INSTANCE、探测端口窗口与归属过滤规则只在此
# 声明，消费方一律通过 find_instance_port 取端口，禁止内联复制探测循环
# ——install.sh 三处 + build.sh 一处各自拷贝的历史已证明拷贝会漂移
# （超时轮数各不相同）。本文件是库而非可执行脚本，直接运行无意义。

# 本实例归属标识。安装期探测与运行期 bridge 都按此字段过滤：只认
# healthy 且 instance==xiaoo 的服务，避免把同机其他 agent 的 agentmoss
# （如 OpenDesk 的 instance=opendesk）当成自己的误停/误连。依赖
# agent-moss >= 0.10.3 的 /api/v1/health 带 instance 字段；旧版缺该字段
# 时 grep 不匹配，宁可探测不到也不漂移。install.sh 的 env 注入
# （AGENT_MOSS_INSTANCE）与日志命名也复用此值。
AM_INSTANCE="xiaoo"

# 健康探测端口窗口：agent-moss server --port 0 从 9090 向上找空闲端口，
# 单机正常量级（1-3 个共存实例）都会落在该窗口内。这是与 agent-moss
# 端口搜索行为的隐式协议耦合——若 9090-9095 全被占用导致服务绑到
# 9096+，探测会失败，属已知安全边界。
AM_PROBE_PORTS=(9090 9091 9092 9093 9094 9095)

# 探测本实例的健康端口，命中时 stdout 输出端口号并返回 0，否则返回 1。
# 调用方需以 `$(find_instance_port || true)` 形式捕获：set -e 下命令
# 替换的非零状态会沿赋值语句传播，缺 `|| true` 时未命中即终止脚本。
find_instance_port() {
    local port
    for port in "${AM_PROBE_PORTS[@]}"; do
        if curl -sf -m 1 "http://127.0.0.1:${port}/api/v1/health" 2>/dev/null \
            | grep -q "\"instance\":\"${AM_INSTANCE}\""; then
            echo "$port"
            return 0
        fi
    done
    return 1
}
