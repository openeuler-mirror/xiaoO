use std::fs;
use std::io::{self, Write};

use agent_llm::{ChatMessageExt, LlmRequestExt, ResponseFormatExt};
use clap::{Parser, Subcommand};

use llm_client::{
    create_llm_provider_from_resolved, create_model_catalog, provider_registry, resolve_config,
    ChatMessage, LlmRequest, ReasoningEffort, ResolveInput, ResponseFormat, Tool, ToolChoice,
};

#[derive(Debug, Parser)]
#[command(
    name = "llm-client-cli",
    version,
    about = "LLM Client CLI - Query LLM providers from the command line"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Query {
        #[arg(long)]
        provider: Option<String>,

        #[arg(long)]
        protocol: Option<String>,

        #[arg(long)]
        base_url: Option<String>,

        #[arg(long)]
        api_key_env: Option<String>,

        #[arg(long)]
        api_key: Option<String>,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        stream: bool,

        #[arg(long)]
        schema: Option<String>,

        #[arg(long)]
        schema_file: Option<String>,

        #[arg(long)]
        tools: Option<String>,

        #[arg(long)]
        tools_file: Option<String>,

        #[arg(long)]
        tool_choice: Option<String>,

        #[arg(long)]
        temperature: Option<f64>,

        #[arg(long)]
        max_tokens: Option<usize>,

        #[arg(long, value_parser = clap::value_parser!(ReasoningEffort))]
        reasoning_effort: Option<ReasoningEffort>,

        prompt: String,
    },

    Models {
        #[arg(long)]
        provider: Option<String>,

        #[arg(long)]
        protocol: Option<String>,

        #[arg(long)]
        base_url: Option<String>,

        #[arg(long)]
        api_key_env: Option<String>,

        #[arg(long)]
        api_key: Option<String>,

        #[arg(long)]
        json: bool,
    },

    Test {
        #[arg(long)]
        provider: Option<String>,

        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug)]
pub enum CliError {
    Config(String),
    Api(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "config error: {}", msg),
            Self::Api(msg) => write!(f, "api error: {}", msg),
        }
    }
}

impl std::error::Error for CliError {}

impl From<llm_client::ResolveError> for CliError {
    fn from(err: llm_client::ResolveError) -> Self {
        Self::Config(err.to_string())
    }
}

impl From<llm_client::LlmError> for CliError {
    fn from(err: llm_client::LlmError) -> Self {
        Self::Api(err.to_string())
    }
}

pub async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Query {
            provider,
            protocol,
            base_url,
            api_key_env,
            api_key,
            model,
            stream,
            schema,
            schema_file,
            tools,
            tools_file,
            tool_choice,
            temperature,
            max_tokens,
            reasoning_effort,
            prompt,
        } => {
            run_query(QueryArgs {
                provider,
                protocol,
                base_url,
                api_key_env,
                api_key,
                model,
                stream,
                schema,
                schema_file,
                tools,
                tools_file,
                tool_choice,
                temperature,
                max_tokens,
                reasoning_effort,
                prompt,
            })
            .await
        }
        Commands::Models {
            provider,
            protocol,
            base_url,
            api_key_env,
            api_key,
            json,
        } => {
            run_models(ModelsArgs {
                provider,
                protocol,
                base_url,
                api_key_env,
                api_key,
                json,
            })
            .await
        }
        Commands::Test { provider, json } => run_test(provider, json).await,
    }
}

struct QueryArgs {
    provider: Option<String>,
    protocol: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    stream: bool,
    schema: Option<String>,
    schema_file: Option<String>,
    tools: Option<String>,
    tools_file: Option<String>,
    tool_choice: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<usize>,
    reasoning_effort: Option<ReasoningEffort>,
    prompt: String,
}

struct ModelsArgs {
    provider: Option<String>,
    protocol: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    api_key: Option<String>,
    json: bool,
}

async fn run_query(args: QueryArgs) -> Result<(), CliError> {
    let config = resolve_config(ResolveInput {
        provider: args.provider,
        protocol: args.protocol,
        api_key: args.api_key,
        api_key_env: args.api_key_env,
        base_url: args.base_url,
    })?;

    let model = args
        .model
        .ok_or_else(|| CliError::Config("model is required for query".to_string()))?;

    let mut request = LlmRequest::new(vec![ChatMessage::user(&args.prompt)]);

    let schema_str = args.schema.or_else(|| {
        args.schema_file.as_ref().and_then(|path| {
            fs::read_to_string(path)
                .map_err(|e| eprintln!("Warning: Failed to read schema file: {}", e))
                .ok()
        })
    });

    if let Some(schema_str) = schema_str {
        let schema: serde_json::Value = serde_json::from_str(&schema_str)
            .map_err(|e| CliError::Config(format!("Invalid JSON schema: {}", e)))?;
        request = request.with_response_format(ResponseFormat::json_schema("response", schema));
    }

    let tools_str = args.tools.or_else(|| {
        args.tools_file.as_ref().and_then(|path| {
            fs::read_to_string(path)
                .map_err(|e| eprintln!("Warning: Failed to read tools file: {}", e))
                .ok()
        })
    });

    if let Some(tools_str) = tools_str {
        let tools: Vec<Tool> = serde_json::from_str(&tools_str)
            .map_err(|e| CliError::Config(format!("Invalid tools JSON: {}", e)))?;
        request = request.with_tools(tools);
    }

    if let Some(choice) = args.tool_choice {
        let tool_choice = match choice.as_str() {
            "auto" => ToolChoice::Auto,
            "none" => ToolChoice::None,
            "required" => ToolChoice::Required,
            name => ToolChoice::Specific(name.to_string()),
        };
        request = request.with_tool_choice(tool_choice);
    }

    if let Some(temp) = args.temperature {
        request = request.with_temperature(temp);
    }

    if let Some(tokens) = args.max_tokens {
        request = request.with_max_tokens(tokens);
    }

    if let Some(effort) = args.reasoning_effort {
        request = request.with_reasoning_effort(effort);
    }

    let provider = create_llm_provider_from_resolved(&config, model, None, None)
        .map_err(|e| CliError::Api(e.to_string()))?;

    if args.stream {
        let response = provider
            .complete_stream(&request, &|chunk| {
                if let Some(ref text) = chunk.delta_text {
                    print!("{}", text);
                    io::stdout().flush().ok();
                }
                if let Some(ref tc) = chunk.delta_tool_call {
                    if !tc.tool_name.is_empty() {
                        eprintln!("[Tool Call] {}", tc.tool_name);
                    }
                }
            })
            .await?;

        println!();

        if !response.message.tool_calls.is_empty() {
            for tc in &response.message.tool_calls {
                eprintln!("[Tool Call] {}({})", tc.tool_name, tc.input);
            }
        }
    } else {
        let response = provider.complete(&request).await?;

        if !response.message.tool_calls.is_empty() {
            for tc in &response.message.tool_calls {
                eprintln!("[Tool Call] {}({})", tc.tool_name, tc.input);
            }
        }

        if let Some(content) = &response.message.text {
            println!("{}", content);
        }
    }

    Ok(())
}

async fn run_models(args: ModelsArgs) -> Result<(), CliError> {
    let config = resolve_config(ResolveInput {
        provider: args.provider,
        protocol: args.protocol,
        api_key: args.api_key,
        api_key_env: args.api_key_env,
        base_url: args.base_url,
    })?;

    if !config.supports_model_catalog {
        let provider = config.provider.as_deref().unwrap_or("this provider");
        return Err(CliError::Config(format!(
            "{} does not support model catalog API",
            provider
        )));
    }

    let catalog = create_model_catalog(&config)?;
    let models = catalog.list_models().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&models).unwrap());
    } else {
        println!("Available models:\n");
        for model in models {
            let display_name = model.display_name.as_deref().unwrap_or(&model.id);
            println!("  {} - {}", model.id, display_name);
        }
    }

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
struct TestResult {
    provider: String,
    env_var: String,
    status: String,
    error: Option<String>,
}

async fn run_test(provider: Option<String>, json: bool) -> Result<(), CliError> {
    let providers = provider.map(|p| vec![p]).unwrap_or_else(|| {
        provider_registry::supported_providers()
            .iter()
            .filter(|p| !p.contains("compatible") && !p.contains("anthropic"))
            .map(|s| s.to_string())
            .collect()
    });

    let mut results: Vec<TestResult> = Vec::new();

    for provider_name in providers {
        let profile = match provider_registry::resolve_provider_profile(&provider_name) {
            Some(p) => p,
            None => {
                results.push(TestResult {
                    provider: provider_name.clone(),
                    env_var: "-".to_string(),
                    status: "unknown".to_string(),
                    error: Some("Provider not found".to_string()),
                });
                continue;
            }
        };

        let env_var = profile.default_api_key_env.unwrap_or("-");

        if !profile.api_key_required {
            results.push(TestResult {
                provider: provider_name.clone(),
                env_var: env_var.to_string(),
                status: "available".to_string(),
                error: None,
            });
            continue;
        }

        let config_result = resolve_config(ResolveInput {
            provider: Some(provider_name.clone()),
            ..Default::default()
        });

        match config_result {
            Ok(config) => {
                if config.supports_model_catalog {
                    match create_model_catalog(&config) {
                        Ok(catalog) => match catalog.list_models().await {
                            Ok(_) => {
                                results.push(TestResult {
                                    provider: provider_name.clone(),
                                    env_var: env_var.to_string(),
                                    status: "ok".to_string(),
                                    error: None,
                                });
                            }
                            Err(e) => {
                                let error_msg = e.to_string();
                                let status = if error_msg.contains("401")
                                    || error_msg.contains("Unauthorized")
                                {
                                    "invalid_key"
                                } else {
                                    "error"
                                };
                                results.push(TestResult {
                                    provider: provider_name.clone(),
                                    env_var: env_var.to_string(),
                                    status: status.to_string(),
                                    error: if status == "error" {
                                        Some(error_msg)
                                    } else {
                                        None
                                    },
                                });
                            }
                        },
                        Err(e) => {
                            results.push(TestResult {
                                provider: provider_name.clone(),
                                env_var: env_var.to_string(),
                                status: "config_error".to_string(),
                                error: Some(e.to_string()),
                            });
                        }
                    }
                } else {
                    results.push(TestResult {
                        provider: provider_name.clone(),
                        env_var: env_var.to_string(),
                        status: "ok".to_string(),
                        error: None,
                    });
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                let status = if error_msg.contains("missing API key")
                    || error_msg.contains("environment variable not found")
                {
                    "no_key"
                } else {
                    "config_error"
                };
                results.push(TestResult {
                    provider: provider_name.clone(),
                    env_var: env_var.to_string(),
                    status: status.to_string(),
                    error: if status == "config_error" {
                        Some(error_msg)
                    } else {
                        None
                    },
                });
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&results).unwrap());
    } else {
        println!("API Key Test Results:\n");
        for result in &results {
            let status_icon = match result.status.as_str() {
                "ok" | "available" => "✓",
                "no_key" => "○",
                "invalid_key" => "✗",
                _ => "?",
            };
            let status_text = match result.status.as_str() {
                "ok" => "OK",
                "available" => "Available (no key required)",
                "no_key" => "No API key configured",
                "invalid_key" => "Invalid API key",
                other => other,
            };
            println!(
                "  {} {} ({}) - {}",
                status_icon, result.provider, result.env_var, status_text
            );
        }
    }

    Ok(())
}
