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
