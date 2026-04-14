# llm-client-cli

**Command-line interface for LLM providers**

A CLI tool for querying LLM providers and listing available models, built on the `llm-client` crate's `LlmProvider` interface.

## Build

```bash
cargo build --release -p llm-client-cli

# target/release/llm-client-cli
```

## Usage

### Query

```bash
# Basic query
llm-client-cli query --provider openai --model gpt-4o-mini "hello"

# Streaming output
llm-client-cli query --provider openrouter --model openai/gpt-4o --stream "tell me a story"

# Custom endpoint with protocol override
llm-client-cli query \
    --protocol anthropic \
    --base-url https://api.lkeap.cloud.tencent.com/coding/anthropic \
    --api-key-env TECENT_CODING_API_KEY \
    --model glm-5 \
    "hello"

# Direct API key (takes precedence over env var)
llm-client-cli query \
    --provider openrouter \
    --api-key "sk-or-xxx" \
    --model anthropic/claude-sonnet-4 \
    "hello"

# With JSON schema for structured output
llm-client-cli query \
    --provider openai \
    --model gpt-4o \
    --schema '{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"integer"}}}' \
    "extract: John is 25 years old"

# With JSON schema file
llm-client-cli query \
    --provider openai \
    --model gpt-4o \
    --schema-file schema.json \
    "extract data from this text"

# With tools for function calling
llm-client-cli query \
    --provider openai \
    --model gpt-4o \
    --tools '[{"name":"get_weather","description":"Get weather for a location","parameters":{"type":"object","properties":{"location":{"type":"string"}}}}]' \
    --tool-choice auto \
    "What's the weather in Tokyo?"

# With tools file
llm-client-cli query \
    --provider openai \
    --model gpt-4o \
    --tools-file tools.json \
    --tool-choice required \
    "Check the weather"
```

### Models

```bash
# List OpenRouter models
llm-client-cli models --provider openrouter

# List Ollama local models
llm-client-cli models --provider ollama

# List OpenAI models
llm-client-cli models --provider openai

# JSON output
llm-client-cli models --provider openai --json

# Custom endpoint
llm-client-cli models \
    --protocol openai \
    --base-url https://api.custom.com/v1 \
    --api-key-env CUSTOM_API_KEY
```

### Test

Test API key availability for providers:

```bash
# Test all providers
llm-client-cli test

# Test a specific provider
llm-client-cli test --provider openai

# JSON output
llm-client-cli test --json
```

## Arguments

### Query Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PROMPT>` | The prompt text | Yes |
| `--provider` | Provider name (openai, anthropic, gemini, ollama, openrouter, etc.) | No* |
| `--protocol` | Protocol override (openai, anthropic, gemini, ollama) | No* |
| `--base-url` | API base URL override | No* |
| `--api-key-env` | Environment variable name for API key | No* |
| `--api-key` | API key value directly (takes precedence over api-key-env) | No* |
| `--model` | Model name | Yes |
| `--stream` | Enable streaming output | No |
| `--schema` | JSON schema string for structured output | No |
| `--schema-file` | Path to JSON schema file | No |
| `--tools` | JSON array of tool definitions | No |
| `--tools-file` | Path to JSON file with tool definitions | No |
| `--tool-choice` | Tool choice: auto, none, required, or function name | No |
| `--temperature` | Temperature for response randomness | No |
| `--max-tokens` | Maximum tokens in the response | No |

\* Either `--provider` OR `--protocol` + `--base-url` must be provided.

### Models Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `--provider` | Provider name | No* |
| `--protocol` | Protocol override | No* |
| `--base-url` | API base URL override | No* |
| `--api-key-env` | Environment variable name for API key | No* |
| `--api-key` | API key value | No* |
| `--json` | Output as JSON | No |

\* Either `--provider` OR `--protocol` + `--base-url` must be provided.

## Supported Providers

| Provider | Protocol | API Key Env |
|----------|----------|-------------|
| OpenAI | OpenAI Compatible | `OPENAI_API_KEY` |
| Anthropic | Anthropic | `ANTHROPIC_API_KEY` |
| Gemini | Gemini | `GEMINI_API_KEY` |
| Ollama | Ollama | (none) |
| Zhipu | Zhipu | `ZHIPU_API_KEY` |
| OpenRouter | OpenAI Compatible | `OPENROUTER_API_KEY` |
| DeepSeek | OpenAI Compatible | `DEEPSEEK_API_KEY` |
| Groq | OpenAI Compatible | `GROQ_API_KEY` |
| Mistral | OpenAI Compatible | `MISTRAL_API_KEY` |
| Together | OpenAI Compatible | `TOGETHER_API_KEY` |
| xAI | OpenAI Compatible | `XAI_API_KEY` |
| MiniMax | OpenAI Compatible | `MINIMAX_API_KEY` |

## Parameter Precedence

- **protocol**: `--protocol` > provider default > error
- **base-url**: `--base-url` > provider default > error
- **api-key**: `--api-key` > `--api-key-env` > provider default env > error (if required)

## Examples

### Quick Test

```bash
export OPENAI_API_KEY=sk-xxx

llm-client-cli query --provider openai --model gpt-4o-mini "say hello"
llm-client-cli models --provider openai
```

### Streaming Response

```bash
llm-client-cli query \
    --provider openai \
    --model gpt-4o-mini \
    --stream \
    "write a short poem about code"
```

### Structured Output

```bash
llm-client-cli query \
    --provider openai \
    --model gpt-4o-mini \
    --schema '{"type":"object","properties":{"sentiment":{"type":"string"},"score":{"type":"number"}}}' \
    "analyze: I love this product!"
```

### Function Calling (Tools)

```bash
llm-client-cli query \
    --provider openai \
    --model gpt-4o \
    --tools '[{"name":"get_weather","description":"Get current weather","parameters":{"type":"object","properties":{"location":{"type":"string"}},"required":["location"]}}]' \
    --tool-choice auto \
    "What's the weather like in Tokyo?"
```

### Local Ollama

```bash
llm-client-cli models --provider ollama

llm-client-cli query \
    --provider ollama \
    --model llama3 \
    "hello"
```

## Error Handling

The CLI exits with code 1 on error and prints to stderr:

```bash
$ llm-client-cli query --provider openai --model gpt-4o-mini "hello"
error: config error: missing API key: specify --api_key, --api_key_env, or --provider with default
```
