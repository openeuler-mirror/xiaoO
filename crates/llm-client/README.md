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
- **Reasoning Support**: `ReasoningEffort` on requests, `StreamChunk.delta_reasoning` for streamed reasoning content, and per-provider `ReasoningDialect` translation

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
use agent_llm::{ChatMessageExt, LlmRequestExt};
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

> **Note:** the constructors used across these examples — `LlmRequest::new`,
> `ChatMessage::user` / `system` / `tool_result`, and the `with_tools` /
> `with_tool_choice` / `with_response_format` builders — are extension-trait
> methods (`LlmRequestExt`, `ChatMessageExt`) provided by the `agent-llm`
> crate; `llm-client` does not re-export them.

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
use agent_llm::LlmRequestExt;
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
use agent_llm::LlmRequestExt;
use llm_client::ResponseFormat;

let request = LlmRequest::new(messages)
    .with_response_format(ResponseFormat::JsonSchema {
        name: "schema_name".into(),
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
| OpenAI | OpenAI Compatible | `OPENAI_API_KEY` | `/models` |
| Anthropic | Anthropic | `ANTHROPIC_API_KEY` | `/models` |
| Gemini | Gemini | `GEMINI_API_KEY` | `/v1beta/models` |
| Ollama | Ollama | (none) | `/api/tags` |
| Zhipu | Zhipu | `ZHIPU_API_KEY` | `/models` |
| OpenRouter | OpenAI Compatible | `OPENROUTER_API_KEY` | `/models` |
| DeepSeek | OpenAI Compatible | `DEEPSEEK_API_KEY` | `/models` |
| Groq | OpenAI Compatible | `GROQ_API_KEY` | `/models` |
| Mistral | OpenAI Compatible | `MISTRAL_API_KEY` | `/models` |
| Together | OpenAI Compatible | `TOGETHER_API_KEY` | `/models` |
| xAI | OpenAI Compatible | `XAI_API_KEY` | `/models` |
| Kimi / Moonshot | OpenAI Compatible | `MOONSHOT_API_KEY` | `/models` |
| Kimi Coding Plan | OpenAI Compatible | `KIMI_API_KEY` | `/models` |
| MiniMax | OpenAI Compatible | `MINIMAX_API_KEY` | — |
| MiniMax Coding Plan | OpenAI Compatible | `MINIMAX_API_KEY` | `/models` |
| MiniMax (Anthropic) | Anthropic | `MINIMAX_API_KEY` | — |
| GitCode | OpenAI Compatible | `GITCODE_API_KEY` | — |
| Local | OpenAI Compatible | (none) | `/models` |
| Z.AI Coding Plan | OpenAI Compatible | `ZHIPU_API_KEY` | `/models` |

Models endpoints are relative to each provider's base URL; `—` means the
provider declares no model catalog support.

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
    pub reasoning_effort: ReasoningEffort,
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
    pub kv_cache_chunk_hashes: Vec<String>,
}

pub struct AssistantMessage {
    pub text: Option<String>,
    pub reasoning_content: Option<String>,
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

## Error diagnostics and credentials

Error messages, llm-client tracing, and `~/.xiaoo/log/error.log` redact known
credentials from the current request. Request URLs retain scheme, host, path,
and parameter names while removing userinfo and masking query values and
fragments. Other links retain their noncredential content. Logs retain the
complete response body with credential replacement and are created with mode
0600 on Unix.

Credential matching is literal: it covers the original value, its standard
`serde_json` string escaping (without surrounding quotes), and RFC 3986 percent
encoding of the whole value, using either uppercase or lowercase hex digits.
Unreserved URL bytes remain literal. Nonstandard or partial encodings, mixed
hex case, hand-written Unicode escapes for otherwise literal characters, and
other encodings of echoed credentials are not recognized. Diagnostic text is
not decoded. Only credentials known to this request can be redacted.

JSON spellings that begin with a backslash are matched only at an escape boundary:
an odd number of immediately preceding backslashes makes the candidate a literal
escaped backslash rather than the beginning of the credential spelling. Original
credential values still match literally.

Request URL wrappers supported in diagnostics are angle brackets (`<...>`),
parentheses (`(...)`), square brackets (`[...]`), braces (`{...}`), double quotes,
single quotes, backticks, and JSON-escaped double quotes (`\"...\"`). Whitespace,
text start/end, left-side `=` or `:` labels, and right-side commas or semicolons
also delimit URLs. A request URL embedded in another link's parameter remains
ordinary text and receives only credential replacement. Boundary context is
maintained in one forward scan.

Literal matching can also mask a credential value inside an unrelated word; it
does not infer the meaning of each occurrence. If a matcher cannot be built, the
message is replaced with a static placeholder and detailed diagnostics are lost.
