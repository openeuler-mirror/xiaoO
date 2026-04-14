# llm-client

**Internal LLM Client Library for AgentOS**

This crate provides a unified client interface for multiple LLM providers. It is an internal workspace crate and is intended for use inside this repository.

## Features

- **Unified Interface**: Single `LlmProvider` trait for all providers
- **Streaming Support**: Callback-based streaming with `complete_stream(on_chunk)` for real-time responses and tool-call deltas
- **Structured Output**: Native support for JSON mode and JSON Schema across providers
- **Tool Calling**: Unified function calling interface with provider-specific translation
- **Model Discovery**: `ModelCatalog` trait for listing available models
- **Provider Abstraction**: OpenAI, DeepSeek, Anthropic, Gemini, Ollama, Zhipu, OpenRouter, and more
- **Provider Capabilities**: Each provider declares its capabilities (streaming, tools, JSON mode, context window)

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Public API Surface                       │
│  create_llm_provider()  create_model_catalog()  resolve_config()│
├─────────────────────────────────────────────────────────────┤
│                      Capability Traits                       │
│  ┌─────────────────────────┐  ┌─────────────────────┐       │
│  │ LlmProvider             │  │ ModelCatalog        │       │
│  │ - complete()            │  │ - list_models()     │       │
│  │ - complete_stream()     │  └─────────────────────┘       │
│  │ - capabilities()        │                                │
│  └─────────────────────────┘                                │
├─────────────────────────────────────────────────────────────┤
│                    Provider Registry                         │
│  ProtocolFamily | ProviderProfile | resolve_provider_profile │
├─────────────────────────────────────────────────────────────┤
│                    Protocol Implementations                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐       │
│  │ OpenAI   │ │ Anthropic│ │ Gemini   │ │ Ollama   │       │
│  │ Family   │ │          │ │          │ │          │       │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘       │
├─────────────────────────────────────────────────────────────┤
│               Internal Wire Types & Convert                  │
│  ChatMessage ↔ WireMessage  LlmRequest ↔ WireRequest        │
│  AssistantMessage ← WireResponse   StreamChunk ← ParsedChunk│
└─────────────────────────────────────────────────────────────┘
```

## Usage

### Simple (LlmProviderConfig)

```rust
use llm_client::{
    create_llm_provider, LlmProviderConfig, LlmRequest, ChatMessage,
};

let provider = create_llm_provider(
    &LlmProviderConfig::new("openai", "gpt-4o")
        .with_api_key(api_key),
)?;

let request = LlmRequest::new(vec![ChatMessage::user("Hello!")]);
let response = provider.complete(&request).await?;
println!("{:?}", response.message.text);
```

### Advanced (Resolver)

For custom endpoints and protocol overrides:

```rust
use llm_client::{
    resolve_config, ResolveInput, create_llm_provider_from_resolved,
    create_model_catalog, LlmRequest, ChatMessage,
};

let config = resolve_config(ResolveInput {
    protocol: Some("anthropic".to_string()),
    base_url: Some("https://custom.api.com/v1".to_string()),
    api_key_env: Some("CUSTOM_API_KEY".to_string()),
    ..Default::default()
})?;

let provider = create_llm_provider_from_resolved(&config, "claude-sonnet-4-6".to_string())?;

let catalog = create_model_catalog(&config)?;
let models = catalog.list_models().await?;
```

### Streaming

```rust
let response = provider.complete_stream(&request, &|chunk| {
    if let Some(text) = &chunk.delta_text {
        print!("{}", text);
    }
}).await?;
```

### Tool Calling

```rust
use llm_client::{Tool, ToolChoice};

let tools = vec![Tool {
    name: "get_weather".to_string(),
    description: "Get the current weather".to_string(),
    parameters: serde_json::json!({
        "type": "object",
        "properties": {
            "location": {"type": "string"}
        }
    }),
}];

let request = LlmRequest::new(messages)
    .with_tools(tools)
    .with_tool_choice(ToolChoice::Auto);
```

### Structured Output

```rust
use llm_client::ResponseFormat;

let request = LlmRequest::new(messages)
    .with_response_format(ResponseFormat::JsonSchema {
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            }
        }),
    });
```

## Provider Support

| Provider | Protocol | API Key Env | Models Endpoint |
|----------|----------|-------------|-----------------|
| OpenAI | OpenAI Compatible | `OPENAI_API_KEY` | `GET /v1/models` |
| Anthropic | Anthropic | `ANTHROPIC_API_KEY` | `GET /v1/models` |
| Gemini | Gemini | `GEMINI_API_KEY` | `GET /v1beta/models` |
| Ollama | Ollama | (none) | `GET /api/tags` |
| Zhipu | Zhipu | `ZHIPU_API_KEY` | `GET /models` |
| OpenRouter | OpenAI Compatible | `OPENROUTER_API_KEY` | `GET /v1/models` |
| DeepSeek | OpenAI Compatible | `DEEPSEEK_API_KEY` | `GET /models` |
| Groq | OpenAI Compatible | `GROQ_API_KEY` | `GET /v1/models` |
| Mistral | OpenAI Compatible | `MISTRAL_API_KEY` | `GET /v1/models` |
| Together | OpenAI Compatible | `TOGETHER_API_KEY` | `GET /v1/models` |
| xAI | OpenAI Compatible | `XAI_API_KEY` | `GET /v1/models` |
| MiniMax | OpenAI Compatible | `MINIMAX_API_KEY` | — |
| MiniMax (Anthropic) | Anthropic | `MINIMAX_API_KEY` | — |

## Core Types

### LlmRequest

```rust
pub struct LlmRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
    pub max_tokens: Option<usize>,
    pub temperature: Option<f64>,
    pub response_format: ResponseFormat,
}
```

### ChatMessage

```rust
ChatMessage::user("Hello");
ChatMessage::system("You are helpful");
ChatMessage::tool_result("call_123", "get_weather", "72°F", false, 0);
```

### LlmResponse

```rust
pub struct LlmResponse {
    pub message: AssistantMessage,
}

pub struct AssistantMessage {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolUseBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}
```

### ProviderCapabilities

```rust
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tool_calls: bool,
    pub supports_json_mode: bool,
    pub max_context_window: usize,
    pub model_name: String,
}
```

## Testing

```bash
# Unit tests
cargo test -p llm-client

# Tests cover:
# - Wire type serialization (tool, message, format, response, stream, route_info)
# - Provider registry (protocol family, profile resolution, API base normalization)
# - Resolver (provider resolution, missing params, protocol mismatch)
# - Auth module (state machine, pool CRUD, cooldown expiry, store operations)
# - Convert module (ChatMessage↔wire, wire→LlmResponse, tool choice)
# - Anthropic stream parsing (content delta, message delta, stop, tool fragments)
# - Gemini (schema sanitization, model normalization, mockito complete, stream parsing)
# - Ollama (stream parsing, response format conversion)
# - Zhipu (schema description generation)
# - Factory (config builder, unknown provider, missing key, missing base)
```
