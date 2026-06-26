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

#[derive(Debug, Clone)]
pub struct SpawnDecision {
    pub result: SpawnSubagentResult,
    pub actions: Vec<HostAction>,
}

#[derive(Debug, Clone)]
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

    pub fn build_with_predefined_prompt(predefined_prompt: &str, task_context: &str) -> String {
        let context_section = if task_context.is_empty() {
            String::new()
        } else {
            format!("\n\nAdditional Task Context:\n{}", task_context)
        };
        format!("{}{}", predefined_prompt, context_section)
    }
}

/// Configuration options for controlling subagent boundaries and quotas.
#[derive(Debug, Clone, Copy)]
pub struct SubagentCoordinatorConfig {
    /// The maximum number of subagents allowed to run concurrently within a given session.
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

        if state
            .agents
            .values()
            .filter(|r| r.status == SubagentStatus::Running)
            .count()
            >= self.config.max_subagents_per_session
        {
            return Err(SubagentControlError::InvalidState {
                message: format!(
                    "maximum number of concurrent subagents ({}) reached for this session",
                    self.config.max_subagents_per_session
                ),
            });
        }

        if state.agents.contains_key(&child_agent_id.0) {
            return Err(SubagentControlError::InvalidState {
                message: format!("duplicate subagent id: {}", child_agent_id),
            });
        }

        let built_prompt = if let Some(predefined) = &request.predefined_prompt {
            SubagentPromptBuilder::build_with_predefined_prompt(predefined, &request.task_context)
        } else {
            SubagentPromptBuilder::build(
                &request.task_goal,
                &request.task_context,
                request.output_schema.as_ref(),
            )
        };

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
                max_turns: request.max_turns,
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
    use super::*;
    use crate::state::SubagentSessionState;
    use crate::types::SpawnSubagentRequest;
    use agent_types::common::ids::AgentId;

    fn make_parent_agent_id() -> AgentId {
        AgentId("parent-1".to_string())
    }

    fn make_request(parent_id: &AgentId, desc: &str) -> SpawnSubagentRequest {
        SpawnSubagentRequest {
            session_id: "test-session".to_string(),
            parent_agent_id: parent_id.clone(),
            description: desc.to_string(),
            task_goal: desc.to_string(),
            task_context: String::new(),
            output_schema: None,
            subagent_role_id: None,
            predefined_prompt: None,
            max_turns: None,
        }
    }

    fn make_coordinator(max: usize) -> SubagentCoordinator {
        SubagentCoordinator::with_config(SubagentCoordinatorConfig {
            max_subagents_per_session: max,
        })
    }

    #[test]
    fn concurrent_limit_allows_spawn_when_under_limit() {
        let coordinator = make_coordinator(3);
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        for i in 0..3 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            let result = coordinator.spawn(&mut state, &request, child_id, 1000);
            assert!(result.is_ok(), "spawn {} should succeed", i);
        }

        assert_eq!(state.agents.len(), 3);
    }

    #[test]
    fn concurrent_limit_rejects_when_all_running() {
        let coordinator = make_coordinator(2);
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        for i in 0..2 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            let result = coordinator.spawn(&mut state, &request, child_id, 1000);
            assert!(result.is_ok(), "spawn {} should succeed", i);
        }

        let child_id = AgentId("child-2".to_string());
        let request = make_request(&parent_id, "task-2");
        let result = coordinator.spawn(&mut state, &request, child_id, 1000);
        assert!(
            result.is_err(),
            "3rd spawn should be rejected when limit is 2"
        );
    }

    #[test]
    fn concurrent_limit_allows_spawn_after_one_completes() {
        let coordinator = make_coordinator(2);
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        for i in 0..2 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            let result = coordinator.spawn(&mut state, &request, child_id, 1000);
            assert!(result.is_ok(), "spawn {} should succeed", i);
        }

        // child-0 completes
        let terminal = SubagentTerminalSnapshot {
            status: SubagentTerminalKind::Completed,
            reply: Some("done".to_string()),
            error: None,
            completed_at_ms: 2000,
        };
        coordinator
            .on_terminal(&mut state, &AgentId("child-0".to_string()), terminal)
            .expect("on_terminal should succeed");

        // Now a new spawn should succeed because only 1 is still Running
        let child_id = AgentId("child-2".to_string());
        let request = make_request(&parent_id, "task-2");
        let result = coordinator.spawn(&mut state, &request, child_id, 3000);
        assert!(
            result.is_ok(),
            "spawn after completion should succeed (concurrent limit, not historical)"
        );
    }

    #[test]
    fn concurrent_limit_rejects_again_after_all_spots_refilled() {
        let coordinator = make_coordinator(2);
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        for i in 0..2 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            coordinator
                .spawn(&mut state, &request, child_id, 1000)
                .unwrap();
        }

        // child-0 completes
        let terminal = SubagentTerminalSnapshot {
            status: SubagentTerminalKind::Completed,
            reply: Some("done".to_string()),
            error: None,
            completed_at_ms: 2000,
        };
        coordinator
            .on_terminal(&mut state, &AgentId("child-0".to_string()), terminal)
            .unwrap();

        // child-2 fills the freed spot
        let child_id = AgentId("child-2".to_string());
        let request = make_request(&parent_id, "task-2");
        coordinator
            .spawn(&mut state, &request, child_id, 3000)
            .unwrap();

        // Now both child-1 and child-2 are Running, limit reached again
        let child_id = AgentId("child-3".to_string());
        let request = make_request(&parent_id, "task-3");
        let result = coordinator.spawn(&mut state, &request, child_id, 4000);
        assert!(
            result.is_err(),
            "should be rejected when limit reached again"
        );
    }

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

        assert!(prompt.contains("Count files"));
        assert!(prompt.contains("Use find"));
        assert!(prompt.contains("disposable exploration worker"));
        assert!(prompt.contains("strictly adheres to the following JSON schema"));
        assert!(prompt.contains("\"count\""));
        assert!(prompt.contains("\"integer\""));
    }

    #[test]
    fn subagent_prompt_builder_without_schema() {
        let prompt = SubagentPromptBuilder::build("Summarize logs", "Check /var/log", None);

        assert!(prompt.contains("Summarize logs"));
        assert!(prompt.contains("Check /var/log"));
        assert!(prompt.contains("disposable exploration worker"));
        assert!(prompt.contains("Conclude your task by providing a clear, concise summary"));
        assert!(!prompt.contains("JSON schema"));
    }

    #[test]
    fn allows_concurrent_pending_joins_from_same_waiter() {
        let coordinator = SubagentCoordinator::new();
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        // Spawn 3 subagents
        for i in 0..3 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            coordinator
                .spawn(&mut state, &request, child_id, 1000)
                .unwrap();
        }

        // Parent can register multiple pending joins simultaneously
        for i in 0..3 {
            let child_id = AgentId(format!("child-{}", i));
            let join_request = JoinSubagentRequest {
                session_id: "test-session".to_string(),
                waiter_agent_id: parent_id.clone(),
                target_agent_id: child_id,
            };
            let result = coordinator.join(&mut state, &join_request, 2000);
            assert!(
                result.is_ok(),
                "join {} should succeed (allowing concurrent pending joins)",
                i
            );

            // All should be Pending since subagents are still running
            match result.unwrap() {
                JoinDecision::Pending { .. } => {}
                JoinDecision::Immediate { .. } => {
                    panic!("join should be pending while subagent is running")
                }
            }
        }

        // Verify all 3 joins are registered
        assert_eq!(state.joins.len(), 3);
        assert!(state
            .joins
            .values()
            .all(|j| j.status == JoinStatus::Pending));
    }

    #[test]
    fn multiple_joins_resolved_when_subagent_completes() {
        let coordinator = SubagentCoordinator::new();
        let mut state = SubagentSessionState::default();
        let parent_id = make_parent_agent_id();

        // Spawn 2 subagents
        for i in 0..2 {
            let child_id = AgentId(format!("child-{}", i));
            let request = make_request(&parent_id, &format!("task-{}", i));
            coordinator
                .spawn(&mut state, &request, child_id, 1000)
                .unwrap();
        }

        // Register joins for both
        for i in 0..2 {
            let child_id = AgentId(format!("child-{}", i));
            let join_request = JoinSubagentRequest {
                session_id: "test-session".to_string(),
                waiter_agent_id: parent_id.clone(),
                target_agent_id: child_id,
            };
            coordinator.join(&mut state, &join_request, 2000).unwrap();
        }

        // Complete first subagent
        let terminal = SubagentTerminalSnapshot {
            status: SubagentTerminalKind::Completed,
            reply: Some("child-0 done".to_string()),
            error: None,
            completed_at_ms: 3000,
        };
        let actions = coordinator
            .on_terminal(&mut state, &AgentId("child-0".to_string()), terminal)
            .unwrap();

        // Should have wake action for first join
        assert_eq!(actions.len(), 2); // mailbox item + wake waiter
        let wake_actions = actions
            .iter()
            .filter(|a| matches!(a, HostAction::WakeWaiter { .. }))
            .count();
        assert_eq!(wake_actions, 1);

        // First join should be satisfied, second still pending
        let satisfied_joins = state
            .joins
            .values()
            .filter(|j| j.status == JoinStatus::Satisfied)
            .count();
        assert_eq!(satisfied_joins, 1);

        let pending_joins = state
            .joins
            .values()
            .filter(|j| j.status == JoinStatus::Pending)
            .count();
        assert_eq!(pending_joins, 1);
    }
}
