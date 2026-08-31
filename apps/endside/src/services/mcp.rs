use std::fmt::Write as _;

use crate::config::Config;

pub fn render_mcp_overview(config: &Config) -> String {
    let servers = config.mcp_servers();

    if servers.is_empty() {
        return String::from("当前未配置 MCP 服务端。\n\n你可以在 config.toml 中添加 [[mcp.servers]] 来配置 MCP 服务端，\n也可以使用 .mcp.json 文件（详见 docs/mcp.md）。");
    }

    let mut output = format!("当前已配置的 MCP 服务端（{}）:\n", servers.len());
    for (i, server) in servers.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        let status = if server.is_enabled() {
            "✅ 启用"
        } else {
            "❌ 禁用"
        };
        let _ = write!(output, "## {}", server.name);
        let _ = write!(output, "\n- 状态: {status}");

        let transport_label = match server.transport {
            xiaoo_shared::mcp_support::Transport::Stdio => "stdio（标准输入输出）",
            xiaoo_shared::mcp_support::Transport::Sse => "SSE（Server-Sent Events）",
            xiaoo_shared::mcp_support::Transport::StreamableHttp => "Streamable HTTP",
        };
        let _ = write!(output, "\n- 传输方式: {transport_label}");

        match server.transport {
            xiaoo_shared::mcp_support::Transport::Stdio => {
                if let Some(cmd) = &server.command {
                    let _ = write!(output, "\n- 命令: {cmd}");
                    if !server.args.is_empty() {
                        let _ = write!(output, "\n- 参数: {}", server.args.join(" "));
                    }
                }
            }
            xiaoo_shared::mcp_support::Transport::Sse
            | xiaoo_shared::mcp_support::Transport::StreamableHttp => {
                if let Some(url) = &server.url {
                    let _ = write!(output, "\n- URL: {url}");
                }
            }
        }

        if let Some(ref agent_id) = server.agent_id {
            let _ = write!(output, "\n- Agent ID: {agent_id}");
        }

        // Effect summary
        let mut effects = Vec::new();
        if server.effect.reads_filesystem {
            effects.push("读文件");
        }
        if server.effect.writes_filesystem {
            effects.push("写文件");
        }
        if server.effect.network_access {
            effects.push("网络访问");
        }
        if server.effect.side_effects {
            effects.push("副作用");
        }
        if !effects.is_empty() {
            let _ = write!(output, "\n- 效应: {}", effects.join(", "));
        }
    }

    output
}
