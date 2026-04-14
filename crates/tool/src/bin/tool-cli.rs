mod tool_cli_runtime;

use agent_contracts::trace::TraceOutcome;
use agent_contracts::{
    RuntimeView, ToolCallBuilder, ToolRegistry, ToolRegistryBuilder, ToolSpecView,
};
use agent_types::common::BuildError;
use agent_types::common::{AgentId, ToolName};
use agent_types::tool::ToolRegistryConfig;
use agent_types::tool::{RawToolCall, RawToolOutcome, ToolExecutionError, ToolVisibilityConfig};
use tokio::runtime::{Builder, Runtime};
use tool::{load_tool_sources, ToolCallBuilderImpl, ToolRegistryBuilderImpl};

use tool_cli_runtime::{ToolCliRuntime, ToolCliTraceConfig};

struct ToolCli {
    agent_id: AgentId,
    registry: Box<dyn ToolRegistry>,
    trace_config: ToolCliTraceConfig,
    tokio_runtime: Runtime,
}

#[derive(Debug)]
enum ToolCliError {
    InvalidUsage,
    InvalidJsonArg(String),
    UnknownTool(String),
    RegistryBuild(BuildError),
    TokioRuntimeBuild(String),
    ToolExecution(ToolExecutionError),
}

enum Command<'a> {
    Run {
        tool_name: &'a str,
        json_arg: &'a str,
    },
    List,
    Show {
        tool_name: &'a str,
    },
}

#[derive(Debug, Clone, Default)]
struct CliOptions {
    trace_backend: Option<String>,
    trace_db: Option<String>,
}

impl ToolCli {
    async fn new(trace_config: ToolCliTraceConfig) -> Result<Self, ToolCliError> {
        let agent_id = AgentId("tool_cli".to_string());
        let sources = load_tool_sources();
        let config = build_tool_cli_registry_config(&sources, &agent_id);

        let registry = ToolRegistryBuilderImpl::new()
            .with_sources(sources)
            .with_config(config)
            .build()
            .map_err(ToolCliError::RegistryBuild)?;

        Ok(Self {
            agent_id,
            registry,
            trace_config,
            tokio_runtime: Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| ToolCliError::TokioRuntimeBuild(error.to_string()))?,
        })
    }

    fn invoke(&self, tool_name: &str, json_arg: &str) -> Result<RawToolOutcome, ToolCliError> {
        let runtime = self
            .tokio_runtime
            .block_on(ToolCliRuntime::new(self.trace_config.clone()))
            .map_err(ToolCliError::RegistryBuild)?;

        let input = serde_json::from_str(json_arg)
            .map_err(|error| ToolCliError::InvalidJsonArg(error.to_string()))?;

        let filter = self.registry.filter_for(&self.agent_id);
        if !filter.allows_tool_name(tool_name) {
            return Err(ToolCliError::UnknownTool(tool_name.to_string()));
        }

        let raw_tool_call = RawToolCall {
            call_id: "tool-cli-call".to_string(),
            tool_name: tool_name.to_string(),
            input,
        };

        let tool_call = ToolCallBuilderImpl::new()
            .with_raw_llm_tool_call(raw_tool_call)
            .with_tool_filter(filter)
            .build()
            .map_err(ToolCliError::RegistryBuild)?;

        let execution_result = self
            .tokio_runtime
            .block_on(tool_call.execute(&runtime))
            .map_err(ToolCliError::ToolExecution)?;

        let trace_outcome = match &execution_result {
            agent_types::tool::ToolExecutionResult::Completed { .. } => TraceOutcome::Ok,
            agent_types::tool::ToolExecutionResult::Suspended { .. } => TraceOutcome::Ok,
            agent_types::tool::ToolExecutionResult::Denied { .. } => TraceOutcome::Denied,
            agent_types::tool::ToolExecutionResult::Failed { .. } => TraceOutcome::Error,
        };

        self.tokio_runtime.block_on(async {
            runtime
                .trace_recorder()
                .finalize_trace(
                    trace_outcome,
                    serde_json::json!({
                        "message": format!("tool-cli run finished for {tool_name}"),
                        "tool_name": tool_name,
                    }),
                )
                .await;
        });

        Ok(match execution_result {
            agent_types::tool::ToolExecutionResult::Completed { raw_outcome, .. } => raw_outcome,
            agent_types::tool::ToolExecutionResult::Suspended { suspend_token, .. } => {
                RawToolOutcome::Error {
                    message: format!("tool execution suspended: {suspend_token}"),
                }
            }
            agent_types::tool::ToolExecutionResult::Denied { error, .. } => RawToolOutcome::Error {
                message: error
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "tool call denied".to_string()),
            },
            agent_types::tool::ToolExecutionResult::Failed {
                execution_error, ..
            } => {
                return Err(ToolCliError::ToolExecution(execution_error));
            }
        })
    }

    fn list_tools(&self) -> Vec<&dyn ToolSpecView> {
        self.registry.list_specs()
    }

    fn get_tool_spec(&self, tool_name: &str) -> Result<&dyn ToolSpecView, ToolCliError> {
        self.registry
            .list_specs()
            .into_iter()
            .find(|spec| spec.name().0 == tool_name)
            .ok_or_else(|| ToolCliError::UnknownTool(tool_name.to_string()))
    }
}

fn build_tool_cli_registry_config(
    sources: &[Box<dyn tool::ToolSource>],
    agent_id: &AgentId,
) -> ToolRegistryConfig {
    let allowed_tool_names = sources
        .iter()
        .flat_map(|source| source.discover())
        .map(|tool| ToolName(tool.spec.name().0.clone()))
        .collect();
    // 当前，tool-cli 允许访问所有从 sources 发现的工具。未来可以改成通过命令行参数指定一个子集。
    let mut per_agent_allowed_tools = std::collections::HashMap::new();
    per_agent_allowed_tools.insert(agent_id.clone(), allowed_tool_names);

    ToolRegistryConfig {
        visibility: ToolVisibilityConfig {
            per_agent_allowed_tools,
        },
    }
}

fn parse_args(argv: &[String]) -> Result<(CliOptions, Command<'_>), ToolCliError> {
    let mut options = CliOptions::default();
    let mut idx = 1;

    while idx < argv.len() {
        match argv[idx].as_str() {
            "--trace-backend" => {
                let Some(value) = argv.get(idx + 1) else {
                    return Err(ToolCliError::InvalidUsage);
                };
                options.trace_backend = Some(value.clone());
                idx += 2;
            }
            "--trace-db" => {
                let Some(value) = argv.get(idx + 1) else {
                    return Err(ToolCliError::InvalidUsage);
                };
                options.trace_db = Some(value.clone());
                idx += 2;
            }
            _ => break,
        }
    }

    let remaining = &argv[idx..];
    let command = match remaining {
        [command] if command == "list" => Command::List,
        [command, tool_name] if command == "show" => Command::Show { tool_name },
        [command, tool_name, flag, json_arg] if command == "run" && flag == "-a" => Command::Run {
            tool_name,
            json_arg,
        },
        _ => return Err(ToolCliError::InvalidUsage),
    };

    Ok((options, command))
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  tool-cli [--trace-backend <stdout|noop|moirai-sqlite>] [--trace-db <path>] run <tool_name> -a <json_arg>");
    eprintln!("  tool-cli [--trace-backend <stdout|noop|moirai-sqlite>] [--trace-db <path>] list");
    eprintln!("  tool-cli [--trace-backend <stdout|noop|moirai-sqlite>] [--trace-db <path>] show <tool_name>");
}

fn print_outcome(outcome: RawToolOutcome) {
    match outcome {
        RawToolOutcome::Success { output } => {
            println!("{}", output);
        }
        RawToolOutcome::Error { message } => {
            eprintln!("tool returned error output: {}", message);
        }
    }
}

fn print_error(error: ToolCliError) {
    match error {
        ToolCliError::InvalidUsage => {
            print_usage();
        }
        ToolCliError::InvalidJsonArg(message) => {
            eprintln!("invalid json arg: {}", message);
        }
        ToolCliError::UnknownTool(tool_name) => {
            eprintln!("unknown tool: {}", tool_name);
        }
        ToolCliError::RegistryBuild(error) => {
            eprintln!("tool registry build failed: {}", error);
        }
        ToolCliError::TokioRuntimeBuild(message) => {
            eprintln!("tokio runtime build failed: {}", message);
        }
        ToolCliError::ToolExecution(error) => {
            eprintln!("tool execution failed: {}", error);
        }
    }
}

fn print_tool_list(tool_specs: &[&dyn ToolSpecView]) {
    for spec in tool_specs {
        println!("{}", spec.name().0);
    }
}

fn print_tool_spec(spec: &dyn ToolSpecView) {
    let schema = serde_json::to_string_pretty(&spec.input_schema().schema)
        .unwrap_or_else(|_| "<failed to render input schema>".to_string());

    println!("[name] {}", spec.name().0);
    println!("\t[id] {}", spec.id());
    println!("\t[description] {}", spec.description());
    println!("\t[output] {}", spec.output_contract().description);
    println!("\t[input_schema] {}", schema);
}

fn main() {
    let bootstrap_runtime = match Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            print_error(ToolCliError::TokioRuntimeBuild(error.to_string()));
            std::process::exit(1);
        }
    };
    let argv: Vec<String> = std::env::args().collect();
    let (options, command) = match parse_args(&argv) {
        Ok(parsed) => parsed,
        Err(error) => {
            print_error(error);
            std::process::exit(2);
        }
    };

    let trace_config = ToolCliTraceConfig {
        backend: options
            .trace_backend
            .unwrap_or_else(|| "stdout".to_string()),
        db_path: options.trace_db,
    };

    let cli = match bootstrap_runtime.block_on(ToolCli::new(trace_config)) {
        Ok(cli) => cli,
        Err(error) => {
            print_error(error);
            std::process::exit(1);
        }
    };

    match command {
        Command::Run {
            tool_name,
            json_arg,
        } => match cli.invoke(tool_name, json_arg) {
            Ok(outcome) => print_outcome(outcome),
            Err(error) => {
                print_error(error);
                std::process::exit(1);
            }
        },
        Command::List => {
            let tool_specs = cli.list_tools();
            print_tool_list(&tool_specs);
        }
        Command::Show { tool_name } => match cli.get_tool_spec(tool_name) {
            Ok(spec) => print_tool_spec(spec),
            Err(error) => {
                print_error(error);
                std::process::exit(1);
            }
        },
    }
}
