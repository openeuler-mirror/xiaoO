use crate::input::Input;
use ratatui::widgets::ScrollbarState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolExecutionUpdate {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    pub args_preview: String,
    pub command_preview: Option<String>,
    pub command: Option<String>,
    pub detail: String,
    pub status: ToolExecutionStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDisplayStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone)]
pub struct TodoSnapshotItem {
    pub status: TodoDisplayStatus,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct TodoSnapshotUpdate {
    pub title: String,
    pub items: Vec<TodoSnapshotItem>,
}

#[derive(Debug, Clone)]
pub struct CompletionCheckUpdate {
    pub reason: String,
    pub missing_information: String,
    pub next_step_hint: String,
}

#[derive(Debug, Clone)]
pub struct ToolMessageState {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    pub args_preview: String,
    pub command_preview: Option<String>,
    pub command: Option<String>,
    pub detail: String,
    pub expanded: bool,
    pub status: ToolExecutionStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TodoMessageState {
    pub title: String,
    pub items: Vec<(TodoDisplayStatus, String)>,
}

#[derive(Debug, Clone)]
pub struct CompletionCheckMessageState {
    pub reason: String,
    pub missing_information: String,
    pub next_step_hint: String,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub thinking_content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub is_streaming: bool,
    pub tool_state: Option<ToolMessageState>,
    pub todo_state: Option<TodoMessageState>,
    pub completion_check_state: Option<CompletionCheckMessageState>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            todo_state: None,
            completion_check_state: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            todo_state: None,
            completion_check_state: None,
        }
    }

    pub fn assistant_streaming() -> Self {
        Self {
            role: MessageRole::Assistant,
            content: String::new(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: true,
            tool_state: None,
            todo_state: None,
            completion_check_state: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            todo_state: None,
            completion_check_state: None,
        }
    }

    pub fn tool_event(update: ToolExecutionUpdate) -> Self {
        Self {
            role: MessageRole::Tool,
            content: String::new(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: Some(ToolMessageState {
                call_id: update.call_id,
                tool: update.tool,
                summary: update.summary,
                args_preview: update.args_preview,
                command_preview: update.command_preview,
                command: update.command,
                detail: update.detail,
                expanded: false,
                status: update.status,
                exit_code: update.exit_code,
                duration_ms: update.duration_ms,
            }),
            todo_state: None,
            completion_check_state: None,
        }
    }

    pub fn todo_snapshot(update: TodoSnapshotUpdate) -> Self {
        Self {
            role: MessageRole::System,
            content: String::new(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            todo_state: Some(TodoMessageState {
                title: update.title,
                items: update
                    .items
                    .into_iter()
                    .map(|item| (item.status, item.content))
                    .collect(),
            }),
            completion_check_state: None,
        }
    }

    pub fn completion_check(update: CompletionCheckUpdate) -> Self {
        Self {
            role: MessageRole::System,
            content: String::new(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            todo_state: None,
            completion_check_state: Some(CompletionCheckMessageState {
                reason: update.reason,
                missing_information: update.missing_information,
                next_step_hint: update.next_step_hint,
            }),
        }
    }
}

pub struct ChatState {
    pub messages: Vec<Message>,
    pub input: Input,
    /// Line-based scroll: number of lines skipped from the top of the message list.
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    pub is_loading: bool,
    pub available_providers: Vec<ProviderInfo>,
    /// When true, view stays at bottom when new content arrives (e.g. streaming).
    pub stick_to_bottom: bool,
    /// Total line count of the message list (updated each render).
    pub total_lines: usize,
    /// Inner height of the Messages area (updated each render) for scroll clamping.
    pub last_visible_height: usize,
    /// True while user is dragging the scrollbar thumb.
    pub scrollbar_dragging: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub name: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

/// Default provider list shown in TUI (openai, anthropic, openrouter, ollama).
pub fn default_provider_list() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            name: "openai".to_string(),
            models: vec![
                ModelInfo {
                    id: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                },
                ModelInfo {
                    id: "gpt-4-turbo".to_string(),
                    name: "GPT-4 Turbo".to_string(),
                },
                ModelInfo {
                    id: "gpt-3.5-turbo".to_string(),
                    name: "GPT-3.5 Turbo".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "anthropic".to_string(),
            models: vec![
                ModelInfo {
                    id: "claude-sonnet-4-20250514".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                },
                ModelInfo {
                    id: "claude-3-5-sonnet-20241022".to_string(),
                    name: "Claude 3.5 Sonnet".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "deepseek".to_string(),
            models: vec![
                ModelInfo {
                    id: "deepseek-chat".to_string(),
                    name: "DeepSeek Chat".to_string(),
                },
                ModelInfo {
                    id: "deepseek-reasoner".to_string(),
                    name: "DeepSeek Reasoner".to_string(),
                },
            ],
        },
        // 智谱 AI (Zhipu / BigModel) — open.bigmodel.cn
        // Aliases resolved by core: zai, zai-cn, zai-china, zai-global, z-ai, z.ai, bigmodel, glm-cn
        ProviderInfo {
            name: "zhipu".to_string(),
            models: vec![
                ModelInfo {
                    id: "glm-5".to_string(),
                    name: "GLM-5 (Flagship)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.7".to_string(),
                    name: "GLM-4.7".to_string(),
                },
                ModelInfo {
                    id: "glm-4.7-flash".to_string(),
                    name: "GLM-4.7 Flash (Fast)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.6".to_string(),
                    name: "GLM-4.6".to_string(),
                },
                ModelInfo {
                    id: "glm-4.6v".to_string(),
                    name: "GLM-4.6V (Vision)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5".to_string(),
                    name: "GLM-4.5".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5-air".to_string(),
                    name: "GLM-4.5 Air".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5v".to_string(),
                    name: "GLM-4.5V (Vision)".to_string(),
                },
                ModelInfo {
                    id: "glm-4-plus".to_string(),
                    name: "GLM-4-Plus".to_string(),
                },
                ModelInfo {
                    id: "glm-4-flash".to_string(),
                    name: "GLM-4-Flash".to_string(),
                },
                ModelInfo {
                    id: "glm-4-long".to_string(),
                    name: "GLM-4-Long (1M ctx)".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "openrouter".to_string(),
            models: vec![
                ModelInfo {
                    id: "z-ai/glm-5".to_string(),
                    name: "GLM-5 (z-ai)".to_string(),
                },
                ModelInfo {
                    id: "minimax/minimax-m2.7".to_string(),
                    name: "MiniMax M2.7".to_string(),
                },
                ModelInfo {
                    id: "minimax/minimax-m2.5".to_string(),
                    name: "MiniMax M2.5".to_string(),
                },
                ModelInfo {
                    id: "minimax/minimax-m2.5:free".to_string(),
                    name: "MiniMax M2.5 (free)".to_string(),
                },
                ModelInfo {
                    id: "anthropic/claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                },
                ModelInfo {
                    id: "openai/gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "ollama".to_string(),
            models: vec![
                ModelInfo {
                    id: "llama3.2".to_string(),
                    name: "Llama 3.2".to_string(),
                },
                ModelInfo {
                    id: "qwen2.5".to_string(),
                    name: "Qwen 2.5".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "gitcode".to_string(),
            models: vec![ModelInfo {
                id: "Qwen/Qwen3.5-397B-A17B".to_string(),
                name: "Qwen 3.5 (GitCode)".to_string(),
            }],
        },
    ]
}

/// Merge config's provider and model into the list: add provider with one model if not present, or add model to existing provider.
pub fn merge_config_provider(
    mut list: Vec<ProviderInfo>,
    provider: &str,
    model_id: &str,
) -> Vec<ProviderInfo> {
    let name = provider.to_string();
    let model = ModelInfo {
        id: model_id.to_string(),
        name: model_id.to_string(),
    };
    if let Some(p) = list.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&name)) {
        if !p.models.iter().any(|m| m.id.eq_ignore_ascii_case(model_id)) {
            p.models.push(model);
        }
    } else {
        list.push(ProviderInfo {
            name,
            models: vec![model],
        });
    }
    list
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            messages: vec![Message::system(
                "Welcome to XiaoO TUI. Type /connect to select provider/model. Type your message and press Enter to send.",
            )],
            input: Input::default(),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_loading: false,
            available_providers: default_provider_list(),
            stick_to_bottom: true,
            total_lines: 0,
            last_visible_height: 0,
            scrollbar_dragging: false,
        }
    }
}

impl ChatState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn messages_to_display(&self, height: usize) -> usize {
        self.messages
            .len()
            .saturating_sub(self.scroll_offset)
            .min(height.saturating_sub(4) / 3)
    }

    pub fn context_size(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .map(|m| m.content.chars().count())
            .sum()
    }

    pub fn estimated_tokens(&self) -> usize {
        const SAFETY_MARGIN: f64 = 1.2;
        let total_chars = self.context_size();
        let base_estimate = (total_chars + 3) / 4;
        let message_overhead = self.messages.len() * 4;
        let total = base_estimate + message_overhead;
        ((total as f64) * SAFETY_MARGIN).ceil() as usize
    }

    /// Max scroll offset (lines) so the last line is visible. Uses last_visible_height and total_lines.
    pub fn max_scroll_offset(&self) -> usize {
        self.total_lines
            .saturating_sub(self.last_visible_height)
            .min(self.total_lines)
    }

    pub fn scroll_up(&mut self) {
        self.stick_to_bottom = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    pub fn scroll_down(&mut self) {
        let max = self.max_scroll_offset();
        if self.scroll_offset < max {
            self.scroll_offset = (self.scroll_offset + 1).min(max);
        }
        if self.scroll_offset >= max {
            self.stick_to_bottom = true;
        }
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    /// Scroll up by a page (half the visible height).
    pub fn page_up(&mut self) {
        let page_size = (self.last_visible_height / 2).max(1);
        self.stick_to_bottom = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    /// Scroll down by a page (half the visible height).
    pub fn page_down(&mut self) {
        let page_size = (self.last_visible_height / 2).max(1);
        let max = self.max_scroll_offset();
        if self.scroll_offset < max {
            self.scroll_offset = (self.scroll_offset + page_size).min(max);
        }
        if self.scroll_offset >= max {
            self.stick_to_bottom = true;
        }
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    /// Scroll to the bottom of the message list.
    pub fn scroll_to_bottom(&mut self) {
        let max = self.max_scroll_offset();
        self.scroll_offset = max;
        self.stick_to_bottom = true;
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }

    /// Scroll to the top of the message list.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.stick_to_bottom = false;
        self.scrollbar_state = self.scrollbar_state.position(0);
    }

    /// Set scroll position by line index (e.g. from scrollbar drag). Clamps to valid range.
    pub fn set_scroll_offset(&mut self, line_offset: usize) {
        let max = self.max_scroll_offset();
        self.scroll_offset = line_offset.min(max);
        self.stick_to_bottom = self.scroll_offset >= max;
        self.scrollbar_state = self.scrollbar_state.position(self.scroll_offset);
    }
}
