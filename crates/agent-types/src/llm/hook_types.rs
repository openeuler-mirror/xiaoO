use crate::llm::error::LlmError;
use crate::llm::request::LlmRequest;
use crate::llm::response::LlmResponse;

#[derive(Clone, Debug)]
pub struct PreLlmHookInput {
    pub request: LlmRequest,
}

#[derive(Clone, Debug)]
pub enum PreLlmHookResult {
    Allow,
    Transform { modified_request: LlmRequest },
}

#[derive(Clone, Debug)]
pub struct PostLlmHookInput {
    pub request: LlmRequest,
    pub response: LlmResponse,
}

#[derive(Clone, Debug)]
pub enum PostLlmHookResult {
    Accept,
    Transform { modified_response: LlmResponse },
}

#[derive(Clone, Debug)]
pub struct ErrorLlmHookInput {
    pub request: LlmRequest,
    pub error: LlmError,
}

#[derive(Clone, Debug)]
pub enum ErrorLlmHookResult {
    Propagate,
    Recover { response: LlmResponse },
}
