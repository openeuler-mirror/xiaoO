use agent_types::common::ids::AgentId;
use ulid::Ulid;

use crate::control::HostAction;
use crate::state::{
    JoinRecord, JoinStatus, SubagentMailboxItem, SubagentRecord, SubagentSessionState,
    SubagentStatus, SubagentTerminalKind, SubagentTerminalSnapshot,
};
use crate::types::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControlError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnDecision {
    pub result: SpawnSubagentResult,
    pub actions: Vec<HostAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinDecision {
    Immediate {
        result: JoinSubagentResult,
        actions: Vec<HostAction>,
    },
    Pending {
        result: JoinSubagentResult,
        actions: Vec<HostAction>,
    },
}

pub struct SubagentPromptBuilder;

const SUBAGENT_PROMPT_TEMPLATE: &str = include_str!("prompts/subagent_prompt_template.txt");

impl SubagentPromptBuilder {
    pub fn build(
        task_goal: &str,
        task_context: &str,
        output_schema: Option<&serde_json::Value>,
    ) -> String {
        let schema_section = match output_schema {
            Some(schema) => format!(
                "You MUST conclude your task by producing a final result that strictly adheres to the following JSON schema. Do not include any other explanatory text in your final finish/terminal reply, ONLY the JSON matching this schema:\n{}",
                serde_json::to_string_pretty(schema).unwrap_or_else(|_| "{}".to_string())
            ),
            None => "Conclude your task by providing a clear, concise summary of your findings.".to_string(),
        };

        SUBAGENT_PROMPT_TEMPLATE
            .trim_end_matches(['\r', '\n'])
            .replace("{{task_goal}}", task_goal)
            .replace("{{task_context}}", task_context)
            .replace("{{output_schema_section}}", &schema_section)
    }
}

/// Configuration options for controlling subagent boundaries and quotas.
#[derive(Debug, Clone, Copy)]
pub struct SubagentCoordinatorConfig {
    /// The maximum total number of subagents allowed to exist within a given session simultaneously.
    pub max_subagents_per_session: usize,
}

impl Default for SubagentCoordinatorConfig {
    fn default() -> Self {
        Self {
            max_subagents_per_session: 10,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SubagentCoordinator {
    config: SubagentCoordinatorConfig,
}

impl SubagentCoordinator {
    pub fn new() -> Self {
        Self {
            config: SubagentCoordinatorConfig::default(),
        }
    }

    pub fn with_config(config: SubagentCoordinatorConfig) -> Self {
        Self { config }
    }

    pub fn spawn(
        &self,
        state: &mut SubagentSessionState,
        request: &SpawnSubagentRequest,
        child_agent_id: AgentId,
        created_at_ms: u64,
    ) -> Result<SpawnDecision, SubagentControlError> {
        if state.agents.contains_key(&request.parent_agent_id.0) {
            return Err(SubagentControlError::InvalidState {
                message: format!(
                    "nested subagents are not allowed: caller {} is already a subagent",
                    request.parent_agent_id.0
                ),
            });
        }

        if state.agents.len() >= self.config.max_subagents_per_session {
            return Err(SubagentControlError::InvalidState {
                message: format!(
                    "maximum number of subagents ({}) reached for this session",
                    self.config.max_subagents_per_session
                ),
            });
        }

        if state.agents.contains_key(&child_agent_id.0) {
            return Err(SubagentControlError::InvalidState {
                message: format!("duplicate subagent id: {}", child_agent_id),
            });
        }

        let built_prompt = SubagentPromptBuilder::build(
            &request.task_goal,
            &request.task_context,
            request.output_schema.as_ref(),
        );

        state.agents.insert(
            child_agent_id.0.clone(),
            SubagentRecord {
                agent_id: child_agent_id.clone(),
                parent_agent_id: Some(request.parent_agent_id.clone()),
                description: request.description.clone(),
                prompt: built_prompt.clone(),
                output_schema: request.output_schema.clone(),
                status: SubagentStatus::Running,
                created_at_ms,
                updated_at_ms: created_at_ms,
                last_terminal: None,
            },
        );

        Ok(SpawnDecision {
            result: SpawnSubagentResult {
                agent_id: child_agent_id.clone(),
            },
            actions: vec![HostAction::SpawnWorker {
                agent_id: child_agent_id,
                parent_agent_id: request.parent_agent_id.clone(),
                description: request.description.clone(),
                prompt: built_prompt,
                output_schema: request.output_schema.clone(),
            }],
        })
    }

    pub fn join(
        &self,
        state: &mut SubagentSessionState,
        request: &JoinSubagentRequest,
        created_at_ms: u64,
    ) -> Result<JoinDecision, SubagentControlError> {
        if request.waiter_agent_id == request.target_agent_id {
            return Err(SubagentControlError::SelfJoin {
                agent_id: request.waiter_agent_id.to_string(),
            });
        }

        let target = state
            .agents
            .get(&request.target_agent_id.0)
            .ok_or_else(|| SubagentControlError::AgentNotFound {
                agent_id: request.target_agent_id.to_string(),
            })?;

        if state.joins.values().any(|join| {
            join.waiter_agent_id == request.waiter_agent_id && join.status == JoinStatus::Pending
        }) {
            return Err(SubagentControlError::WaiterAlreadyWaiting {
                agent_id: request.waiter_agent_id.to_string(),
            });
        }

        if target.status.is_terminal() {
            let terminal = target.last_terminal.clone().ok_or_else(|| {
                SubagentControlError::MissingTerminalSnapshot {
                    agent_id: request.target_agent_id.to_string(),
                }
            })?;
            return Ok(JoinDecision::Immediate {
                result: JoinSubagentResult::Ready { terminal },
                actions: Vec::new(),
            });
        }

        let join_id = Ulid::new().to_string();
        state.joins.insert(
            join_id.clone(),
            JoinRecord {
                join_id: join_id.clone(),
                waiter_agent_id: request.waiter_agent_id.clone(),
                target_agent_id: request.target_agent_id.clone(),
                status: JoinStatus::Pending,
                created_at_ms,
                resolved_at_ms: None,
            },
        );

        Ok(JoinDecision::Pending {
            result: JoinSubagentResult::Pending {
                join_id: join_id.clone(),
            },
            actions: vec![HostAction::SuspendWaiter {
                join_id,
                waiter_agent_id: request.waiter_agent_id.clone(),
                target_agent_id: request.target_agent_id.clone(),
            }],
        })
    }

    pub fn on_terminal(
        &self,
        state: &mut SubagentSessionState,
        agent_id: &AgentId,
        terminal: SubagentTerminalSnapshot,
    ) -> Result<Vec<HostAction>, SubagentControlError> {
        let record = state.agents.get_mut(&agent_id.0).ok_or_else(|| {
            SubagentControlError::AgentNotFound {
                agent_id: agent_id.to_string(),
            }
        })?;

        record.status = terminal_kind_to_status(&terminal.status);
        record.updated_at_ms = terminal.completed_at_ms;
        record.last_terminal = Some(terminal.clone());

        let summary = terminal
            .reply
            .clone()
            .or_else(|| terminal.error.clone())
            .unwrap_or_else(|| format!("subagent {} finished", agent_id));

        let mut actions = vec![HostAction::EnqueueMailboxItem {
            item: SubagentMailboxItem {
                agent_id: agent_id.clone(),
                parent_agent_id: record.parent_agent_id.clone(),
                status: terminal.status.clone(),
                summary,
                completed_at_ms: terminal.completed_at_ms,
            },
        }];

        for join in state
            .joins
            .values_mut()
            .filter(|join| join.target_agent_id == *agent_id && join.status == JoinStatus::Pending)
        {
            join.status = JoinStatus::Satisfied;
            join.resolved_at_ms = Some(terminal.completed_at_ms);
            actions.push(HostAction::WakeWaiter {
                join_id: join.join_id.clone(),
                waiter_agent_id: join.waiter_agent_id.clone(),
                terminal: terminal.clone(),
            });
        }

        Ok(actions)
    }
}

fn terminal_kind_to_status(kind: &SubagentTerminalKind) -> SubagentStatus {
    match kind {
        SubagentTerminalKind::Completed => SubagentStatus::Completed,
        SubagentTerminalKind::Failed => SubagentStatus::Failed,
        SubagentTerminalKind::Cancelled => SubagentStatus::Cancelled,
        SubagentTerminalKind::MaxTurnsReached => SubagentStatus::MaxTurnsReached,
        SubagentTerminalKind::BudgetExhausted => SubagentStatus::BudgetExhausted,
    }
}

#[cfg(test)]
mod tests {
    use super::SubagentPromptBuilder;

    #[test]
    fn subagent_prompt_builder_with_schema() {
        let prompt = SubagentPromptBuilder::build(
            "Count files",
            "Use find",
            Some(&serde_json::json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer" }
                },
                "required": ["count"]
            })),
        );

        assert_eq!(
            prompt,
            "You are a subagent summoned by a parent agent. Your primary goal is:\n\
Count files\n\n\
Task Context:\n\
Use find\n\n\
You MUST conclude your task by producing a final result that strictly adheres to the following JSON schema. Do not include any other explanatory text in your final finish/terminal reply, ONLY the JSON matching this schema:\n\
{\n  \"properties\": {\n    \"count\": {\n      \"type\": \"integer\"\n    }\n  },\n  \"required\": [\n    \"count\"\n  ],\n  \"type\": \"object\"\n}"
        );
    }

    #[test]
    fn subagent_prompt_builder_without_schema() {
        let prompt = SubagentPromptBuilder::build("Summarize logs", "Check /var/log", None);

        assert_eq!(
            prompt,
            "You are a subagent summoned by a parent agent. Your primary goal is:\n\
Summarize logs\n\n\
Task Context:\n\
Check /var/log\n\n\
Conclude your task by providing a clear, concise summary of your findings."
        );
    }
}
