//! Per-provider default `api_key_env` / `api_base` tables, shared by
//! `/connect` prefills, model-list fetching and `[[llm.models]]` matching.

pub(crate) fn api_key_env_for_provider(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "openai" => "OPENAI_API_KEY",
        "anthropic" | "claude" => "ANTHROPIC_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "groq" => "GROQ_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "xai" | "xai-grok" => "XAI_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "zai" | "zai-global" | "z.ai" | "zai-cn" | "zai-china" | "bigmodel" | "zhipu"
        | "glm-cn" => "ZHIPU_API_KEY",
        "zai-coding-plan" | "zhipu-coding-plan" | "zhipuai-coding-plan" => "ZHIPU_API_KEY",
        "glm" | "glm-global" => "GLM_API_KEY",
        "minimax"
        | "minimax-openai"
        | "minimax-anthropic"
        | "minimax-coding-plan"
        | "minimax-code-plan"
        | "minimax-token-plan" => "MINIMAX_API_KEY",
        "kimi" | "moonshot" | "moonshot-ai" => "MOONSHOT_API_KEY",
        "kimi-coding-plan" | "kimi-code-plan" | "kimi-code" | "kimi-for-coding" => "KIMI_API_KEY",
        "gitcode" => "GITCODE_API_KEY",
        "ollama" => "OLLAMA_HOST",
        "local" => "API_KEY",
        _ => "API_KEY",
    }
}

pub(crate) fn default_api_key_env_for_provider(provider: &str) -> Option<String> {
    let env_var = api_key_env_for_provider(provider);
    match env_var {
        "API_KEY" | "OLLAMA_HOST" => None,
        _ => Some(env_var.to_string()),
    }
}

pub(crate) fn default_api_base_for_provider(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "openai" => "https://api.openai.com/v1".to_string(),
        "openrouter" => "https://openrouter.ai/api/v1".to_string(),
        "groq" => "https://api.groq.com/openai/v1".to_string(),
        "mistral" => "https://api.mistral.ai/v1".to_string(),
        "together" => "https://api.together.xyz/v1".to_string(),
        "xai" | "xai-grok" => "https://api.x.ai/v1".to_string(),
        "deepseek" => "https://api.deepseek.com/v1".to_string(),
        "gitcode" => "https://api-ai.gitcode.com/v1".to_string(),
        "minimax" | "minimax-openai" => "https://api.minimaxi.com/v1".to_string(),
        "minimax-anthropic" => "https://api.minimaxi.com/anthropic/v1".to_string(),
        "minimax-coding-plan" | "minimax-code-plan" | "minimax-token-plan" => {
            "https://api.minimax.io/v1".to_string()
        }
        "kimi" | "moonshot" | "moonshot-ai" => "https://api.moonshot.cn/v1".to_string(),
        "kimi-coding-plan" | "kimi-code-plan" | "kimi-code" | "kimi-for-coding" => {
            "https://api.kimi.com/coding/v1".to_string()
        }
        "ollama" => "http://localhost:11434".to_string(),
        "local" => "http://localhost:8080/v1".to_string(),
        "zai-coding-plan" | "zhipu-coding-plan" | "zhipuai-coding-plan" => {
            "https://api.z.ai/api/coding/paas/v4".to_string()
        }
        _ => String::new(),
    }
}
