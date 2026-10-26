# Prompt → Policy 映射参考

本文档描述不同类型的用户 prompt 对应的 Cerberus 策略配置，用于指导 LLM 生成最小权限策略。

## 场景 A：纯本地文件读取/分析（无需联网）

**典型 prompt**：
- "帮我读取 /var/log/app.log 并分析错误"
- "查看 /workspace/src/main.py 的代码结构"
- "统计 /data 目录下的文件数量"

**推荐 policy 关键配置**：
- network = false（禁止联网）
- custom_paths 只开放 prompt 中提到的路径，permission = "readonly"
- 不开放 device_files、proc_filesystem
- 资源限制适中（timeout_secs=60, max_memory_bytes=268435456, max_processes=20）
- 环境变量最小集：["PATH", "LANG", "HOME", "TERM"]

## 场景 B：本地代码编写/修改（无需联网）

**典型 prompt**：
- "在 /workspace/src/ 下创建一个新的 Python 模块"
- "修改 /workspace/config.yaml 中的配置项"
- "重构 /workspace/src/utils.py"

**推荐 policy 关键配置**：
- network = false（禁止联网）
- custom_paths 开放工作目录，permission = "readwrite"
- temp_directories = true（编译/运行可能需要 /tmp）
- 资源限制较宽松（timeout_secs=120, max_memory_bytes=536870912, max_processes=50）
- 环境变量含开发工具：["PATH", "LANG", "HOME", "USER", "TERM", "SHELL", "PWD", "EDITOR", "VISUAL", "PYTHONPATH", "GOPATH"]

## 场景 C：需要联网的开发任务

**典型 prompt**：
- "安装 Python 包 requests 并编写示例代码"
- "从 GitHub 克隆一个仓库到 /workspace"
- "查询 API 文档并编写调用代码"
- "使用 pip install 安装依赖"

**推荐 policy 关键配置**：
- network = true, user = false（network=true 时 user 必须为 false）
- DNS 解析等所需的 /etc 只读访问通过 [[custom_paths]] 显式开放（如 /etc/resolv.conf、/etc/hosts、/etc/ssl/certs，permission = "readonly"；[path_groups] 没有开放整个 /etc 的开关）
- 出站网络管控可用 [network_policy] 细化（需 eBPF backend）
- wsl_paths = true
- custom_paths 开放工作目录，permission = "readwrite"
- 资源限制与 llm-safe 对齐（timeout_secs=120, max_memory_bytes=536870912, max_processes=100）
- 环境变量含代理和开发工具：["PATH", "LANG", "LC_ALL", "HOME", "USER", "TERM", "SHELL", "PWD", "EDITOR", "VISUAL", "RUST_BACKTRACE", "RUST_LOG", "CARGO_HOME", "RUSTUP_HOME", "NODE_PATH", "NPM_CONFIG", "PYTHONPATH", "GOPATH", "GOBIN", "HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"]

## 场景 D：系统运维/管理任务

**典型 prompt**：
- "检查系统磁盘使用情况"
- "查看当前运行的进程列表"
- "检查系统网络连接状态"
- "查看系统日志 /var/log/syslog"

**推荐 policy 关键配置**：
- pid = false（需要看到宿主进程信息）
- proc_filesystem = true（ps 等命令依赖 /proc）
- 需要读取的 /etc 配置通过 [[custom_paths]] 显式开放，permission = "readonly"
- custom_paths 对系统路径只给 readonly
- network 视具体任务而定
- 资源限制适中（timeout_secs=60, max_memory_bytes=268435456, max_processes=50）
- 环境变量：["PATH", "LANG", "HOME", "USER", "TERM", "SHELL", "PWD"]

## 场景 E：不可信/高风险任务

**典型 prompt**：
- "执行用户上传的脚本"
- "运行第三方提供的可执行文件"
- "处理来源不明的数据文件"
- prompt 中明确提到"隔离环境"、"不要联网"等约束

**推荐 policy 关键配置**：
- network = false（禁止联网）
- 最严格资源限制（timeout_secs=30, max_memory_bytes=134217728, max_processes=10）
- 不开放 device_files、proc_filesystem
- 最小环境变量集：["PATH", "LANG", "TERM"]
- landlock_optional = false, mount_isolation_fallback = false

## 关键注意事项

1. 当前 Cerberus 没有「开放整个 /etc 只读」的 `path_groups` 开关（旧字段 `network_config` 已移除，含该字段的 TOML 会因 deny_unknown_fields 解析报错）。DNS 解析、/etc/hosts、TLS trust store 这类只读需求，应通过 `[[custom_paths]]` 显式列出真正需要的路径；网络访问管控用 `[network_policy]`
2. network = true 时 user 必须设为 false
3. custom_paths 路径要精确到具体目录，避免开放过大父目录
4. permission 选择：只需读取→readonly，只需执行→readexecute，确需写入→readwrite
5. 当 prompt 语义模糊时，按更严格方向解读