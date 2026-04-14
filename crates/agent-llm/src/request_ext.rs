use agent_types::{ChatMessage, CompletionConfig, LlmRequest, ResponseFormat, Tool, ToolChoice};

pub trait ResponseFormatExt {
    fn json_schema(name: &str, schema: serde_json::Value) -> Self
    where
        Self: Sized;
}

impl ResponseFormatExt for ResponseFormat {
    fn json_schema(name: &str, schema: serde_json::Value) -> Self {
        Self::JsonSchema {
            name: name.to_string(),
            schema,
        }
    }
}

pub trait CompletionConfigExt {
    fn validate(&self) -> Result<(), String>;
}

impl CompletionConfigExt for CompletionConfig {
    fn validate(&self) -> Result<(), String> {
        if self.max_tokens == 0 {
            return Err("max_tokens must be greater than zero".to_string());
        }
        if self.temperature < 0.0 || self.temperature > 2.0 {
            return Err("temperature must be between 0.0 and 2.0".to_string());
        }
        Ok(())
    }
}

pub trait LlmRequestExt {
    fn new(messages: Vec<ChatMessage>) -> Self
    where
        Self: Sized;

    fn with_tools(self, tools: Vec<Tool>) -> Self
    where
        Self: Sized;

    fn with_tool_choice(self, tool_choice: ToolChoice) -> Self
    where
        Self: Sized;

    fn with_max_tokens(self, tokens: usize) -> Self
    where
        Self: Sized;

    fn with_temperature(self, temp: f64) -> Self
    where
        Self: Sized;

    fn with_response_format(self, format: ResponseFormat) -> Self
    where
        Self: Sized;
}

impl LlmRequestExt for LlmRequest {
    fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            max_tokens: None,
            temperature: None,
            response_format: ResponseFormat::Text,
        }
    }

    fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = tool_choice;
        self
    }

    fn with_max_tokens(mut self, tokens: usize) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp);
        self
    }

    fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = format;
        self
    }
}
