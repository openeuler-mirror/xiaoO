use crate::input::Input;
use ratatui::widgets::ScrollbarState;
use std::collections::{BTreeMap, VecDeque};
use xiaoo_shared::session_diff::FileChangeDelta as SharedFileChangeDelta;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeDelta {
    pub file_path: String,
    pub additions: u32,
    pub deletions: u32,
}

impl From<SharedFileChangeDelta> for FileChangeDelta {
    fn from(delta: SharedFileChangeDelta) -> Self {
        Self {
            file_path: delta.file_path,
            additions: delta.additions,
            deletions: delta.deletions,
        }
    }
}

impl From<FileChangeDelta> for SharedFileChangeDelta {
    fn from(delta: FileChangeDelta) -> Self {
        Self {
            file_path: delta.file_path,
            additions: delta.additions,
            deletions: delta.deletions,
        }
    }
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
    pub file_change: Option<FileChangeDelta>,
}

pub use xiaoo_shared::plan::{TodoDisplayStatus, TodoSnapshotUpdate};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedTurn {
    pub prompt: String,
    /// Slash-command metadata when the queued turn originated from a
    /// `~/.xiaoo/commands/<name>.md` invocation; `None` for free-form input.
    /// Carried through the queue so `*.Chat.command.before` still fires when
    /// the turn is dequeued after the running turn finishes.
    pub command_context: Option<agent_types::chat::CommandContext>,
    /// Cross-turn `send_prompt` chain depth. `0` for user-typed turns
    /// (resets the chain). When a `SendPrompt` hook action is enqueued
    /// because a turn is already running, the daemon-stamped
    /// `action.chain_depth` is carried here so `start_next_queued_turn`
    /// relays it back to the daemon via `RuntimeTurnRequest.chain_depth`.
    pub chain_depth: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    pub completion_check_state: Option<CompletionCheckMessageState>,
    pub render_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Error,
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
            completion_check_state: None,
            render_revision: 0,
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
            completion_check_state: None,
            render_revision: 0,
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
            completion_check_state: None,
            render_revision: 0,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Error,
            content: content.into(),
            thinking_content: String::new(),
            timestamp: chrono::Local::now(),
            is_streaming: false,
            tool_state: None,
            completion_check_state: None,
            render_revision: 0,
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
            completion_check_state: None,
            render_revision: 0,
        }
    }

    pub fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.mark_render_dirty();
    }

    pub fn append_content(&mut self, chunk: &str) {
        self.content.push_str(chunk);
        self.mark_render_dirty();
    }

    pub fn set_thinking_content(&mut self, content: impl Into<String>) {
        self.thinking_content = content.into();
        self.mark_render_dirty();
    }

    pub fn set_streaming(&mut self, streaming: bool) {
        if self.is_streaming != streaming {
            self.is_streaming = streaming;
            self.mark_render_dirty();
        }
    }

    pub fn mark_render_dirty(&mut self) {
        self.render_revision = self.render_revision.wrapping_add(1);
    }
}

/// Reconstruct TUI display `Message`s from the daemon-side LLM conversation
/// context (`LoopStateSnapshot.messages`). Used when switching to a remote
/// session that already has history so the user sees the prior turns
/// instead of a blank transcript.
///
/// Lossy by design: the daemon only stores the LLM-facing
/// `ChatMessage`s (role + content blocks + reasoning_content), not the
/// TUI's rich per-message metadata (render revisions, subagent lane
/// layout, completion-check state, etc.). What we recover:
/// - User text → `Message::user`
/// - Assistant text + `reasoning_content` → `Message::assistant` (with
///   thinking content)
/// - Tool results → `Message::tool_event` with args pulled from the
///   matching earlier `ToolUse` block (same `call_id`)
///
/// System messages are dropped: they are internal LLM context (system
/// prompt, tool descriptions) and not part of the user-visible chat
/// transcript the TUI maintains.
pub fn messages_from_chat_messages(messages: Vec<llm_client::ChatMessage>) -> Vec<Message> {
    use agent_types::llm::message::{ContentBlock, MessageRole};
    use std::collections::HashMap;

    // First pass: collect ToolUse (tool_name, args) keyed by call_id so the
    // ToolResult pass can attach the original invocation args to the same
    // tool card. The LLM context stores the call (Assistant role with
    // ToolUse block) and the result (Tool role with ToolResult block) in
    // separate ChatMessages; the TUI display wants a single tool card per
    // call_id showing both args and output.
    let mut tool_args: HashMap<String, (String, String)> = HashMap::new();
    for msg in &messages {
        if !matches!(msg.role, MessageRole::Assistant) {
            continue;
        }
        for block in &msg.blocks {
            if let ContentBlock::ToolUse {
                call_id,
                tool_name,
                input,
            } = block
            {
                let args_preview = input.to_string();
                tool_args.insert(call_id.clone(), (tool_name.clone(), args_preview));
            }
        }
    }

    let mut result: Vec<Message> = Vec::new();
    for msg in messages {
        match msg.role {
            MessageRole::System => {
                // Skip — see doc comment above.
                continue;
            }
            MessageRole::User => {
                let text: String = msg
                    .blocks
                    .into_iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    result.push(Message::user(text));
                }
            }
            MessageRole::Assistant => {
                let text: String = msg
                    .blocks
                    .into_iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_reasoning = msg
                    .reasoning_content
                    .as_deref()
                    .map(|rc| !rc.trim().is_empty())
                    .unwrap_or(false);
                if !text.trim().is_empty() || has_reasoning {
                    let mut message = Message::assistant_streaming();
                    message.set_content(text);
                    message.set_streaming(false);
                    if let Some(reasoning) = msg.reasoning_content {
                        message.set_thinking_content(reasoning);
                    }
                    result.push(message);
                }
            }
            MessageRole::Tool => {
                for block in msg.blocks {
                    if let ContentBlock::ToolResult {
                        call_id,
                        tool_name,
                        output,
                        is_error,
                    } = block
                    {
                        let (tool, args_preview) = tool_args
                            .remove(&call_id)
                            .unwrap_or((tool_name, String::new()));
                        let update = ToolExecutionUpdate {
                            call_id,
                            tool,
                            summary: if is_error {
                                "tool failed".to_string()
                            } else {
                                "tool completed".to_string()
                            },
                            args_preview,
                            command_preview: None,
                            command: None,
                            detail: output,
                            status: if is_error {
                                ToolExecutionStatus::Failed
                            } else {
                                ToolExecutionStatus::Completed
                            },
                            exit_code: None,
                            duration_ms: None,
                            file_change: None,
                        };
                        result.push(Message::tool_event(update));
                    }
                }
            }
        }
    }
    result
}

pub struct ChatState {
    pub messages: Vec<Message>,
    pub input: Input,
    /// Persisted user input history, ordered from oldest to newest.
    pub input_history: Vec<String>,
    /// Current input-history cursor, indexing into `input_history`.
    pub input_history_cursor: Option<usize>,
    /// Draft text to restore after navigating back to the newest history edge.
    pub input_history_draft: String,
    pub pending_turns: VecDeque<QueuedTurn>,
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
    pub subagent_lanes: BTreeMap<String, SubagentLaneState>,
    pub active_subagent_stack: Vec<String>,
}

pub struct SubagentLaneState {
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub task_goal: String,
    pub messages: Vec<Message>,
    pub stream_message_index: Option<usize>,
    pub scroll_offset: usize,
    pub scrollbar_state: ScrollbarState,
    pub stick_to_bottom: bool,
    pub total_lines: usize,
    pub last_visible_height: usize,
    pub scrollbar_dragging: bool,
    pub is_running: bool,
    pub last_turn: Option<u32>,
}

impl SubagentLaneState {
    pub fn new(
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    ) -> Self {
        Self {
            agent_id,
            parent_agent_id,
            title,
            description,
            task_goal,
            messages: Vec::new(),
            stream_message_index: None,
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            stick_to_bottom: true,
            total_lines: 0,
            last_visible_height: 0,
            scrollbar_dragging: false,
            is_running: true,
            last_turn: None,
        }
    }

    pub fn update_metadata(
        &mut self,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    ) {
        if self.parent_agent_id.is_none() {
            self.parent_agent_id = parent_agent_id;
        }
        if !title.trim().is_empty() {
            self.title = title;
        }
        if !description.trim().is_empty() {
            self.description = description;
        }
        if !task_goal.trim().is_empty() {
            self.task_goal = task_goal;
        }
    }

    pub fn max_scroll_offset(&self) -> usize {
        self.total_lines
            .saturating_sub(self.last_visible_height)
            .min(self.total_lines)
    }

    pub fn sync_scrollbar_state(&mut self) {
        let max_scroll = self.max_scroll_offset();
        let scrollbar_content_length = max_scroll.saturating_add(1);
        self.scrollbar_state = self
            .scrollbar_state
            .content_length(scrollbar_content_length)
            .viewport_content_length(self.last_visible_height)
            .position(self.scroll_offset.min(max_scroll));
    }

    pub fn scroll_up(&mut self) {
        self.stick_to_bottom = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.sync_scrollbar_state();
    }

    pub fn scroll_down(&mut self) {
        let max = self.max_scroll_offset();
        if self.scroll_offset < max {
            self.scroll_offset = (self.scroll_offset + 1).min(max);
        }
        if self.scroll_offset >= max {
            self.stick_to_bottom = true;
        }
        self.sync_scrollbar_state();
    }

    pub fn set_scroll_offset(&mut self, line_offset: usize) {
        let max = self.max_scroll_offset();
        self.scroll_offset = line_offset.min(max);
        self.stick_to_bottom = self.scroll_offset >= max;
        self.sync_scrollbar_state();
    }
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
                    id: "deepseek-v4-flash".to_string(),
                    name: "DeepSeek V4 Flash".to_string(),
                },
                ModelInfo {
                    id: "deepseek-v4-pro".to_string(),
                    name: "DeepSeek V4 Pro".to_string(),
                },
                ModelInfo {
                    id: "deepseek-chat".to_string(),
                    name: "DeepSeek Chat V3".to_string(),
                },
                ModelInfo {
                    id: "deepseek-reasoner".to_string(),
                    name: "DeepSeek Reasoner V3".to_string(),
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
            name: "minimax".to_string(),
            models: vec![
                ModelInfo {
                    id: "MiniMax-M2.7".to_string(),
                    name: "MiniMax M2.7".to_string(),
                },
                ModelInfo {
                    id: "MiniMax-M2.7-highspeed".to_string(),
                    name: "MiniMax M2.7 Highspeed".to_string(),
                },
                ModelInfo {
                    id: "MiniMax-M2.5".to_string(),
                    name: "MiniMax M2.5".to_string(),
                },
                ModelInfo {
                    id: "MiniMax-M2.5-highspeed".to_string(),
                    name: "MiniMax M2.5 Highspeed".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "kimi".to_string(),
            models: vec![
                ModelInfo {
                    id: "kimi-k2-0905-preview".to_string(),
                    name: "Kimi K2 0905 Preview".to_string(),
                },
                ModelInfo {
                    id: "kimi-latest".to_string(),
                    name: "Kimi Latest".to_string(),
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
            name: "local".to_string(),
            models: vec![ModelInfo {
                id: "glm4.7".to_string(),
                name: "GLM 4.7 (Local)".to_string(),
            }],
        },
        ProviderInfo {
            name: "gitcode".to_string(),
            models: vec![ModelInfo {
                id: "Qwen/Qwen3.5-397B-A17B".to_string(),
                name: "Qwen 3.5 (GitCode)".to_string(),
            }],
        },
        // MiniMax Coding Plan — api.minimax.io OpenAI-compatible endpoint
        ProviderInfo {
            name: "minimax-coding-plan".to_string(),
            models: vec![
                ModelInfo {
                    id: "MiniMax-M2.7".to_string(),
                    name: "MiniMax M2.7 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "MiniMax-M2.7-highspeed".to_string(),
                    name: "MiniMax M2.7 Highspeed (Coding Plan)".to_string(),
                },
            ],
        },
        // Kimi Coding Plan — api.kimi.com/coding/v1 OpenAI-compatible endpoint
        ProviderInfo {
            name: "kimi-coding-plan".to_string(),
            models: vec![ModelInfo {
                id: "kimi-for-coding".to_string(),
                name: "Kimi for Coding".to_string(),
            }],
        },
        // Z.AI Coding Plan (Zhipu Coding Plan) — api.z.ai OpenAI-compatible
        // Models: glm-4.5, glm-4.5-air, glm-4.5-flash, glm-4.5v, glm-4.6, glm-4.6v, glm-4.7
        ProviderInfo {
            name: "zai-coding-plan".to_string(),
            models: vec![
                ModelInfo {
                    id: "glm-5.1".to_string(),
                    name: "GLM-5.1 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-5".to_string(),
                    name: "GLM-5 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.7".to_string(),
                    name: "GLM-4.7 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.6".to_string(),
                    name: "GLM-4.6 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.6v".to_string(),
                    name: "GLM-4.6V (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5".to_string(),
                    name: "GLM-4.5 (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5-air".to_string(),
                    name: "GLM-4.5 Air (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5-flash".to_string(),
                    name: "GLM-4.5 Flash (Coding Plan)".to_string(),
                },
                ModelInfo {
                    id: "glm-4.5v".to_string(),
                    name: "GLM-4.5V (Coding Plan)".to_string(),
                },
            ],
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
            input_history: Vec::new(),
            input_history_cursor: None,
            input_history_draft: String::new(),
            pending_turns: VecDeque::new(),
            scroll_offset: 0,
            scrollbar_state: ScrollbarState::default(),
            is_loading: false,
            available_providers: default_provider_list(),
            stick_to_bottom: true,
            total_lines: 0,
            last_visible_height: 0,
            scrollbar_dragging: false,
            subagent_lanes: BTreeMap::new(),
            active_subagent_stack: Vec::new(),
        }
    }
}

impl ChatState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_input_history(&mut self, history: Vec<String>) {
        self.input_history = history
            .into_iter()
            .filter(|entry| !entry.trim().is_empty())
            .collect();
        let excess = self
            .input_history
            .len()
            .saturating_sub(crate::services::input_history::MAX_INPUT_HISTORY);
        if excess > 0 {
            self.input_history.drain(0..excess);
        }
        self.reset_input_history_navigation();
    }

    pub fn record_input_history(&mut self, input: &str) {
        crate::services::input_history::append_entry(&mut self.input_history, input);
        self.reset_input_history_navigation();
    }

    pub fn reset_input_history_navigation(&mut self) {
        self.input_history_cursor = None;
        self.input_history_draft.clear();
    }

    pub fn enqueue_pending_turn(
        &mut self,
        prompt: String,
        command_context: Option<agent_types::chat::CommandContext>,
        chain_depth: usize,
    ) {
        self.pending_turns.push_back(QueuedTurn {
            prompt,
            command_context,
            chain_depth,
        });
        self.input.reset();
        self.stick_to_bottom = true;
    }

    pub fn pop_pending_turn(&mut self) -> Option<QueuedTurn> {
        self.pending_turns.pop_front()
    }

    pub fn remove_pending_turn_prompt(&mut self, prompt: &str) -> bool {
        let Some(index) = self
            .pending_turns
            .iter()
            .position(|queued| queued.prompt == prompt)
        else {
            return false;
        };
        self.pending_turns.remove(index);
        true
    }

    pub fn has_pending_turns(&self) -> bool {
        !self.pending_turns.is_empty()
    }

    pub fn previous_input_history(&mut self) -> bool {
        if self.input_history.is_empty() {
            return false;
        }

        let index = match self.input_history_cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.input_history_draft = self.input.value().to_string();
                self.input_history.len().saturating_sub(1)
            }
        };
        self.input_history_cursor = Some(index);
        self.input = Input::from(self.input_history[index].clone());
        true
    }

    pub fn next_input_history(&mut self) -> bool {
        let Some(current) = self.input_history_cursor else {
            return false;
        };

        if current + 1 < self.input_history.len() {
            let next = current + 1;
            self.input_history_cursor = Some(next);
            self.input = Input::from(self.input_history[next].clone());
        } else {
            let draft = std::mem::take(&mut self.input_history_draft);
            self.input_history_cursor = None;
            self.input = Input::from(draft);
        }
        true
    }

    /// Max scroll offset (lines) so the last line is visible. Uses last_visible_height and total_lines.
    pub fn max_scroll_offset(&self) -> usize {
        self.total_lines
            .saturating_sub(self.last_visible_height)
            .min(self.total_lines)
    }

    pub fn sync_scrollbar_state(&mut self) {
        let max_scroll = self.max_scroll_offset();
        let scrollbar_content_length = max_scroll.saturating_add(1);
        self.scrollbar_state = self
            .scrollbar_state
            .content_length(scrollbar_content_length)
            .viewport_content_length(self.last_visible_height)
            .position(self.scroll_offset.min(max_scroll));
    }

    pub fn scroll_up(&mut self) {
        self.stick_to_bottom = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.sync_scrollbar_state();
    }

    pub fn scroll_down(&mut self) {
        let max = self.max_scroll_offset();
        if self.scroll_offset < max {
            self.scroll_offset = (self.scroll_offset + 1).min(max);
        }
        if self.scroll_offset >= max {
            self.stick_to_bottom = true;
        }
        self.sync_scrollbar_state();
    }

    /// Set scroll position by line index (e.g. from scrollbar drag). Clamps to valid range.
    pub fn set_scroll_offset(&mut self, line_offset: usize) {
        let max = self.max_scroll_offset();
        self.scroll_offset = line_offset.min(max);
        self.stick_to_bottom = self.scroll_offset >= max;
        self.sync_scrollbar_state();
    }

    /// Page step for PageUp/PageDown: scroll by the visible height minus one line so a
    /// single line of context overlaps between pages (standard pager behavior). Always
    /// at least one line so paging still works on very short viewports.
    fn page_step(&self) -> usize {
        self.last_visible_height.saturating_sub(1).max(1)
    }

    /// Scroll the transcript up by one page (visible height). Disables stick-to-bottom
    /// so the view stays in place instead of jumping back to the streaming tail.
    pub fn scroll_page_up(&mut self) {
        self.stick_to_bottom = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(self.page_step());
        self.sync_scrollbar_state();
    }

    /// Scroll the transcript down by one page (visible height). Re-enables
    /// stick-to-bottom once the bottom of the transcript is reached.
    pub fn scroll_page_down(&mut self) {
        let max = self.max_scroll_offset();
        self.scroll_offset = self.scroll_offset.saturating_add(self.page_step()).min(max);
        self.stick_to_bottom = self.scroll_offset >= max;
        self.sync_scrollbar_state();
    }

    pub fn active_subagent_id(&self) -> Option<&str> {
        self.active_subagent_stack.last().map(String::as_str)
    }

    pub fn is_subagent_view_active(&self) -> bool {
        self.active_subagent_id().is_some()
    }

    pub fn enter_subagent_view(&mut self, agent_id: &str) -> bool {
        if !self.subagent_lanes.contains_key(agent_id) {
            return false;
        }
        if self.active_subagent_id() == Some(agent_id) {
            return true;
        }
        self.active_subagent_stack.push(agent_id.to_string());
        true
    }

    pub fn leave_subagent_view(&mut self) -> bool {
        self.active_subagent_stack.pop().is_some()
    }

    pub fn ensure_subagent_lane(
        &mut self,
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    ) -> &mut SubagentLaneState {
        self.subagent_lanes
            .entry(agent_id.clone())
            .and_modify(|lane| {
                lane.update_metadata(
                    parent_agent_id.clone(),
                    title.clone(),
                    description.clone(),
                    task_goal.clone(),
                );
            })
            .or_insert_with(|| {
                SubagentLaneState::new(agent_id, parent_agent_id, title, description, task_goal)
            })
    }

    /// Like [`Self::ensure_subagent_lane`] but never invokes
    /// [`SubagentLaneState::update_metadata`] on an existing lane: returns
    /// a `&mut` without touching title/description/task_goal; otherwise
    /// inserts a new lane with the supplied fallback metadata. Use this in
    /// handlers that only need the lane present (e.g. `TurnStart` /
    /// `SetAssistantContent` / `Tool` for a subagent whose metadata was
    /// already populated by an earlier `SubagentSpawn` SSE event).
    pub fn ensure_subagent_lane_preserve_metadata(
        &mut self,
        agent_id: String,
        fallback_parent_agent_id: Option<String>,
        fallback_title: String,
        fallback_description: String,
        fallback_task_goal: String,
    ) -> &mut SubagentLaneState {
        self.subagent_lanes
            .entry(agent_id.clone())
            .or_insert_with(|| {
                SubagentLaneState::new(
                    agent_id,
                    fallback_parent_agent_id,
                    fallback_title,
                    fallback_description,
                    fallback_task_goal,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatState, Input, Message};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget},
    };

    #[test]
    fn message_render_revision_updates_with_content_and_streaming_changes() {
        let mut message = Message::assistant_streaming();
        assert_eq!(message.render_revision, 0);

        message.set_content("hello");
        assert_eq!(message.render_revision, 1);

        message.append_content(" world");
        assert_eq!(message.render_revision, 2);

        message.set_streaming(false);
        assert_eq!(message.render_revision, 3);

        message.set_streaming(false);
        assert_eq!(message.render_revision, 3);
    }

    #[test]
    fn input_history_walks_entries_from_newest_to_oldest() {
        let mut chat = ChatState::default();
        chat.set_input_history(vec!["first".to_string(), "second".to_string()]);

        assert!(chat.previous_input_history());
        assert_eq!(chat.input.value(), "second");

        assert!(chat.previous_input_history());
        assert_eq!(chat.input.value(), "first");

        assert!(chat.previous_input_history());
        assert_eq!(chat.input.value(), "first");
    }

    #[test]
    fn input_history_down_restores_draft_after_latest_entry() {
        let mut chat = ChatState::default();
        chat.set_input_history(vec!["first".to_string(), "second".to_string()]);
        chat.input = Input::from("draft");

        assert!(chat.previous_input_history());
        assert_eq!(chat.input.value(), "second");

        assert!(chat.next_input_history());
        assert_eq!(chat.input.value(), "draft");
        assert_eq!(chat.input_history_cursor, None);
    }

    #[test]
    fn input_history_ignores_transcript_messages() {
        let mut chat = ChatState::default();
        chat.messages.push(Message::system("system"));
        chat.messages.push(Message::assistant_streaming());
        chat.messages.push(Message::user("not history"));
        chat.input = Input::from("draft");

        assert!(!chat.previous_input_history());
        assert_eq!(chat.input.value(), "draft");
    }

    #[test]
    fn record_input_history_keeps_latest_entries() {
        let mut chat = ChatState::default();

        chat.record_input_history("first");
        chat.record_input_history("first");
        chat.record_input_history("second");

        assert_eq!(
            chat.input_history,
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn enqueue_pending_turn_adds_fifo_item_without_transcript_message() {
        let mut chat = ChatState::default();
        chat.messages.clear();
        chat.input = Input::from("queued");

        chat.enqueue_pending_turn("queued".to_string(), None, 0);

        assert_eq!(chat.input.value(), "");
        assert!(chat.stick_to_bottom);
        assert!(chat.messages.is_empty());
        assert_eq!(
            chat.pop_pending_turn().map(|queued| queued.prompt),
            Some("queued".to_string())
        );
        assert!(!chat.has_pending_turns());
    }

    #[test]
    fn synced_scrollbar_reaches_track_bottom_when_chat_is_at_bottom() {
        let mut chat = ChatState::default();
        chat.total_lines = 100;
        chat.last_visible_height = 20;
        chat.scroll_offset = chat.max_scroll_offset();
        chat.sync_scrollbar_state();

        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 5));
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .render(buffer.area, &mut buffer, &mut chat.scrollbar_state);

        assert_eq!(buffer[(0, 4)].symbol(), "█");
    }

    #[test]
    fn total_line_scrollbar_state_leaves_gap_at_bottom_for_chat_offsets() {
        let mut legacy_state = ScrollbarState::default()
            .content_length(100)
            .viewport_content_length(20)
            .position(80);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 5));
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("░"))
            .render(buffer.area, &mut buffer, &mut legacy_state);

        assert_eq!(buffer[(0, 4)].symbol(), "░");
    }

    #[test]
    fn page_step_overlaps_one_line_for_pager_style_paging() {
        let mut chat = ChatState::default();
        chat.last_visible_height = 20;
        assert_eq!(chat.page_step(), 19);

        // A viewport of a single line still pages by one.
        chat.last_visible_height = 1;
        assert_eq!(chat.page_step(), 1);

        // A zero-height viewport degrades to a one-line step instead of stalling.
        chat.last_visible_height = 0;
        assert_eq!(chat.page_step(), 1);
    }

    #[test]
    fn scroll_page_up_moves_back_by_viewport_and_unsticks_from_bottom() {
        let mut chat = ChatState::default();
        chat.total_lines = 100;
        chat.last_visible_height = 20;
        // Start glued to the streaming tail.
        chat.scroll_offset = chat.max_scroll_offset(); // 80
        chat.stick_to_bottom = true;

        chat.scroll_page_up();

        // 80 - 19 = 61, with one line of overlap retained.
        assert_eq!(chat.scroll_offset, 61);
        assert!(!chat.stick_to_bottom);
    }

    #[test]
    fn scroll_page_up_clamps_at_top_without_overshooting() {
        let mut chat = ChatState::default();
        chat.total_lines = 100;
        chat.last_visible_height = 20;
        chat.scroll_offset = 5;
        chat.stick_to_bottom = false;

        chat.scroll_page_up();

        // 5.saturating_sub(19) == 0; never goes negative.
        assert_eq!(chat.scroll_offset, 0);
        assert!(!chat.stick_to_bottom);
    }

    #[test]
    fn scroll_page_down_advances_by_viewport_and_resticks_at_bottom() {
        let mut chat = ChatState::default();
        chat.total_lines = 100;
        chat.last_visible_height = 20;
        chat.scroll_offset = 0;
        chat.stick_to_bottom = false;

        chat.scroll_page_down();

        // 0 + 19 = 19, one line of overlap with the previous page.
        assert_eq!(chat.scroll_offset, 19);
        assert!(!chat.stick_to_bottom);

        // Paging further eventually lands exactly on the bottom and re-sticks.
        chat.scroll_page_down(); // 19 + 19 = 38
        chat.scroll_page_down(); // 38 + 19 = 57
        chat.scroll_page_down(); // 57 + 19 = 76
        chat.scroll_page_down(); // 76 + 19 = 95, clamped to max 80
        assert_eq!(chat.scroll_offset, chat.max_scroll_offset());
        assert!(chat.stick_to_bottom);
    }

    #[test]
    fn scroll_page_down_at_bottom_keeps_stick_to_bottom() {
        let mut chat = ChatState::default();
        chat.total_lines = 100;
        chat.last_visible_height = 20;
        chat.scroll_offset = chat.max_scroll_offset();
        chat.stick_to_bottom = true;

        chat.scroll_page_down();

        // Already at the bottom: stays at max and remains stuck.
        assert_eq!(chat.scroll_offset, chat.max_scroll_offset());
        assert!(chat.stick_to_bottom);
    }

    #[test]
    fn page_scroll_respects_short_transcript_without_overflow() {
        let mut chat = ChatState::default();
        chat.total_lines = 5;
        chat.last_visible_height = 20;
        chat.scroll_offset = 0;

        // max_scroll_offset is 0 (everything fits), so paging down stays put and sticks.
        chat.scroll_page_down();
        assert_eq!(chat.scroll_offset, 0);
        assert!(chat.stick_to_bottom);
    }

    #[test]
    fn messages_from_chat_messages_drops_system_role_and_maps_user_assistant_text() {
        use super::messages_from_chat_messages;
        use super::MessageRole;
        use agent_types::llm::message::{ContentBlock, MessageRole as LlmRole};
        use llm_client::ChatMessage;

        let messages = vec![
            ChatMessage {
                role: LlmRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "you are helpful".to_string(),
                }],
                message_id: None,
                timestamp_ms: 0,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
            ChatMessage {
                role: LlmRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "hello there".to_string(),
                }],
                message_id: None,
                timestamp_ms: 1,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
            ChatMessage {
                role: LlmRole::Assistant,
                blocks: vec![
                    ContentBlock::Text {
                        text: "thinking...".to_string(),
                    },
                    ContentBlock::Text {
                        text: "answer".to_string(),
                    },
                ],
                message_id: None,
                timestamp_ms: 2,
                api_usage_tokens: None,
                reasoning_content: Some("reasoning here".to_string()),
                estimated_tokens: None,
            },
        ];

        let tui = messages_from_chat_messages(messages);

        // System message dropped; user text joined; assistant text blocks
        // joined with thinking content preserved.
        assert_eq!(tui.len(), 2);
        assert_eq!(tui[0].role, MessageRole::User);
        assert_eq!(tui[0].content, "hello there");
        assert_eq!(tui[1].role, MessageRole::Assistant);
        assert_eq!(tui[1].content, "thinking...\nanswer");
        assert_eq!(tui[1].thinking_content, "reasoning here");
        assert!(!tui[1].is_streaming);
    }

    #[test]
    fn messages_from_chat_messages_merges_tool_use_and_tool_result_into_one_card() {
        use super::messages_from_chat_messages;
        use super::{MessageRole, ToolExecutionStatus};
        use agent_types::llm::message::{ContentBlock, MessageRole as LlmRole};
        use llm_client::ChatMessage;

        let messages = vec![
            ChatMessage {
                role: LlmRole::Assistant,
                blocks: vec![ContentBlock::ToolUse {
                    call_id: "call-1".to_string(),
                    tool_name: "shell".to_string(),
                    input: serde_json::json!({ "command": "ls" }),
                }],
                message_id: None,
                timestamp_ms: 0,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
            ChatMessage {
                role: LlmRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    call_id: "call-1".to_string(),
                    tool_name: "shell".to_string(),
                    output: "file.txt".to_string(),
                    is_error: false,
                }],
                message_id: None,
                timestamp_ms: 1,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
        ];

        let tui = messages_from_chat_messages(messages);

        // ToolUse + ToolResult collapse to a single tool card carrying both
        // args (from the ToolUse) and output (from the ToolResult).
        assert_eq!(tui.len(), 1);
        assert_eq!(tui[0].role, MessageRole::Tool);
        let tool = tui[0]
            .tool_state
            .as_ref()
            .expect("tool_state should be populated");
        assert_eq!(tool.call_id, "call-1");
        assert_eq!(tool.tool, "shell");
        assert!(tool.args_preview.contains("ls"));
        assert_eq!(tool.detail, "file.txt");
        assert_eq!(tool.status, ToolExecutionStatus::Completed);
    }

    #[test]
    fn messages_from_chat_messages_marks_failed_tool_results() {
        use super::messages_from_chat_messages;
        use super::ToolExecutionStatus;
        use agent_types::llm::message::{ContentBlock, MessageRole as LlmRole};
        use llm_client::ChatMessage;

        let messages = vec![ChatMessage {
            role: LlmRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call-9".to_string(),
                tool_name: "file_edit".to_string(),
                output: "permission denied".to_string(),
                is_error: true,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        }];

        let tui = messages_from_chat_messages(messages);
        assert_eq!(tui.len(), 1);
        let tool = tui[0]
            .tool_state
            .as_ref()
            .expect("tool_state should be populated");
        assert_eq!(tool.status, ToolExecutionStatus::Failed);
        assert_eq!(tool.detail, "permission denied");
        // No matching ToolUse → tool_name falls back to the ToolResult's name.
        assert_eq!(tool.tool, "file_edit");
    }
}
