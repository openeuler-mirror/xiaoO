use std::{
    collections::HashSet,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use agent_client_protocol::{schema::v1 as acp, Client, ConnectionTo, Error};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::time::{timeout, timeout_at, Instant};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use xiaoo_shared::gateway::controlled_delegation::{
    parse_tool_input, ClientCommand, ClientOperations, ControlledToolClient, ExecOutput,
    ExecResult, OperationError,
};

use super::server::TurnUpdates;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(1);
const TERMINAL_CLEANUP_TIMEOUT: Duration = Duration::from_millis(1500);
pub(crate) const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Permission {
    #[default]
    Deny,
    Ask,
    Preauthorized,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct DelegationConfig {
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub exclusive_writer: bool,
}

impl DelegationConfig {
    pub fn enabled(&self) -> bool {
        !matches!(self.permission, Permission::Deny)
    }
}

pub(crate) struct Delegation {
    connection: ConnectionTo<Client>,
    session: acp::SessionId,
    permission: Permission,
    active: Mutex<Option<Arc<ActiveTurn>>>,
    uncertain: Arc<AtomicBool>,
}

struct ActiveTurn {
    updates: TurnUpdates,
    cancel: CancellationToken,
    tasks: TaskTracker,
    calls: Mutex<HashSet<acp::ToolCallId>>,
    question: AtomicBool,
}

pub(crate) enum DelegationOutcome {
    Settled,
    InteractionRequired,
    Uncertain,
}

impl Delegation {
    pub fn new(
        connection: ConnectionTo<Client>,
        session: acp::SessionId,
        permission: Permission,
    ) -> Self {
        Self {
            connection,
            session,
            permission,
            active: Mutex::new(None),
            uncertain: Arc::default(),
        }
    }

    pub fn is_uncertain(&self) -> bool {
        self.uncertain.load(Ordering::Acquire)
    }

    pub fn start(&self, cancel: CancellationToken, updates: TurnUpdates) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        debug_assert!(active.is_none());
        *active = Some(Arc::new(ActiveTurn {
            updates,
            cancel,
            tasks: TaskTracker::new(),
            calls: Mutex::default(),
            question: AtomicBool::default(),
        }));
    }

    pub async fn settle(&self) -> DelegationOutcome {
        let turn = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let Some(turn) = turn else {
            return DelegationOutcome::Settled;
        };
        turn.cancel.cancel();
        turn.tasks.close();
        if timeout(SETTLE_TIMEOUT, turn.tasks.wait()).await.is_err() {
            self.uncertain.store(true, Ordering::Release);
        }
        let calls =
            std::mem::take(&mut *turn.calls.lock().unwrap_or_else(|error| error.into_inner()));
        for id in calls {
            let _ = turn
                .updates
                .emit(tool_status(id, acp::ToolCallStatus::Failed));
        }
        if self.is_uncertain() {
            DelegationOutcome::Uncertain
        } else if turn.question.load(Ordering::Acquire) {
            DelegationOutcome::InteractionRequired
        } else {
            DelegationOutcome::Settled
        }
    }

    fn turn(&self) -> Result<Arc<ActiveTurn>, OperationError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let turn = active.as_ref().ok_or_else(cancelled)?;
        if turn.cancel.is_cancelled() || self.is_uncertain() {
            return Err(cancelled());
        }
        Ok(turn.clone())
    }

    async fn operation<T: Send + 'static>(
        &self,
        f: impl Future<Output = Result<T, OperationError>> + Send + 'static,
        turn: &ActiveTurn,
    ) -> Result<T, OperationError> {
        // A surrounding tool deadline can drop this waiter. The worker still
        // owns any published mutation or terminal and settles it before the
        // prompt owner permits another operation.
        let guard = turn.cancel.clone().drop_guard();
        let result = turn.tasks.spawn(f).await;
        let _ = guard.disarm();
        match result {
            Ok(result) => result,
            Err(_) => {
                self.uncertain.store(true, Ordering::Release);
                turn.cancel.cancel();
                Err(transport("client callback could not be settled"))
            }
        }
    }
}

fn tool_status(id: acp::ToolCallId, status: acp::ToolCallStatus) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id,
        acp::ToolCallUpdateFields::new().status(status),
    ))
}

fn kind(name: &str) -> acp::ToolKind {
    match name {
        "bash" => acp::ToolKind::Execute,
        "file_read" => acp::ToolKind::Read,
        "file_edit" | "file_write" => acp::ToolKind::Edit,
        "glob" | "grep" => acp::ToolKind::Search,
        "todo_write" => acp::ToolKind::Think,
        _ => acp::ToolKind::Other,
    }
}

fn transport(message: &str) -> OperationError {
    OperationError::Transport {
        message: message.into(),
    }
}

fn cancelled() -> OperationError {
    transport("prompt cancelled")
}

fn denied() -> OperationError {
    OperationError::PermissionDenied {
        path: "client tool call".into(),
    }
}

fn definitive_rejection(error: &Error) -> bool {
    matches!(i32::from(error.code), -32602..=-32600 | -32002 | -32000)
}

#[async_trait]
impl ControlledToolClient for Delegation {
    async fn start_tool(&self, name: &str, input: &Value) -> Result<String, OperationError> {
        let turn = self.turn()?;
        let identifier = uuid::Uuid::new_v4().to_string();
        let id = acp::ToolCallId::new(identifier.clone());
        turn.updates
            .emit(acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(id.clone(), name).kind(kind(name)),
            ))
            .map_err(|_| cancelled())?;
        turn.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.clone());

        if name == "ask_user_question" {
            turn.question.store(true, Ordering::Release);
            turn.cancel.cancel();
            return Err(cancelled());
        }

        if name != "todo_write" {
            let allowed = match self.permission {
                Permission::Deny => false,
                Permission::Preauthorized => true,
                Permission::Ask => {
                    // The client must receive the tool announcement before its approval request.
                    turn.updates.flush().await.map_err(|_| cancelled())?;
                    let request = acp::RequestPermissionRequest::new(
                        self.session.clone(),
                        acp::ToolCallUpdate::new(
                            id.clone(),
                            acp::ToolCallUpdateFields::new().raw_input(input.clone()),
                        ),
                        vec![
                            acp::PermissionOption::new(
                                "allow_once",
                                "Allow once",
                                acp::PermissionOptionKind::AllowOnce,
                            ),
                            acp::PermissionOption::new(
                                "reject_once",
                                "Reject once",
                                acp::PermissionOptionKind::RejectOnce,
                            ),
                        ],
                    );
                    tokio::select! {
                        biased;
                        _ = turn.cancel.cancelled() => false,
                        result = async {
                            timeout(REQUEST_TIMEOUT, self.connection.send_request(request).block_task()).await
                        } => {
                            matches!(result, Ok(Ok(response)) if matches!(
                                &response.outcome,
                                acp::RequestPermissionOutcome::Selected(selected)
                                    if selected.option_id.to_string() == "allow_once"
                            ))
                        },
                    }
                }
            };
            if !allowed {
                self.finish_tool(&identifier, name, None);
                return Err(denied());
            }
        }
        if turn.cancel.is_cancelled() {
            return Err(cancelled());
        }
        turn.updates
            .emit(tool_status(id, acp::ToolCallStatus::InProgress))
            .map_err(|_| cancelled())?;
        turn.updates.flush().await.map_err(|_| cancelled())?;
        Ok(identifier)
    }

    fn finish_tool(&self, id: &str, name: &str, completed_input: Option<&Value>) {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(turn) = active else { return };
        let id = acp::ToolCallId::new(id);
        turn.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
        let status = if completed_input.is_some() {
            acp::ToolCallStatus::Completed
        } else {
            acp::ToolCallStatus::Failed
        };
        let _ = turn.updates.emit(tool_status(id, status));
        if let Some(input) = completed_input.filter(|_| name == "todo_write") {
            if let Ok(todos) = parse_tool_input::<TodoPlan>(input) {
                let plan = acp::Plan::new(
                    todos
                        .todos
                        .into_iter()
                        .map(|item| {
                            acp::PlanEntry::new(
                                item.content,
                                acp::PlanEntryPriority::Medium,
                                item.status,
                            )
                        })
                        .collect(),
                );
                let _ = turn.updates.emit(acp::SessionUpdate::Plan(plan));
            }
        }
    }
}

#[derive(Deserialize)]
struct TodoPlan {
    todos: Vec<TodoItem>,
}

#[derive(Deserialize)]
struct TodoItem {
    content: String,
    status: acp::PlanEntryStatus,
}

#[async_trait]
impl ClientOperations for Delegation {
    async fn read_text(&self, path: &str) -> Result<String, OperationError> {
        let turn = self.turn()?;
        let request = acp::ReadTextFileRequest::new(self.session.clone(), path);
        tokio::select! {
            biased;
            _ = turn.cancel.cancelled() => Err(cancelled()),
            result = async {
                timeout(REQUEST_TIMEOUT, self.connection.send_request(request).block_task()).await
            } => result.ok().and_then(Result::ok).map(|response| response.content)
                .ok_or_else(|| transport("client file read rejected or timed out")),
        }
    }

    async fn write_text(&self, path: &str, content: String) -> Result<(), OperationError> {
        let turn = self.turn()?;
        let worker = turn.clone();
        let connection = self.connection.clone();
        let request = acp::WriteTextFileRequest::new(self.session.clone(), path, content);
        let uncertain = self.uncertain.clone();
        self.operation(
            async move {
                if worker.cancel.is_cancelled() {
                    return Err(cancelled());
                }
                let response = connection.send_request(request).block_task();
                tokio::pin!(response);
                let result = tokio::select! {
                    biased;
                    _ = worker.cancel.cancelled() => timeout(CONFIRM_TIMEOUT, &mut response).await,
                    result = timeout(REQUEST_TIMEOUT, &mut response) => result,
                };
                match result {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(error)) if definitive_rejection(&error) => Err(denied()),
                    _ => {
                        uncertain.store(true, Ordering::Release);
                        worker.cancel.cancel();
                        Err(transport("client write result is unknown"))
                    }
                }
            },
            &turn,
        )
        .await
    }

    async fn run_terminal(&self, command: ClientCommand) -> Result<ExecResult, OperationError> {
        let turn = self.turn()?;
        let controller = TerminalController {
            connection: self.connection.clone(),
            session: self.session.clone(),
            turn: turn.clone(),
            uncertain: self.uncertain.clone(),
        };
        self.operation(controller.run(command), &turn).await
    }
}

struct TerminalController {
    connection: ConnectionTo<Client>,
    session: acp::SessionId,
    turn: Arc<ActiveTurn>,
    uncertain: Arc<AtomicBool>,
}

impl TerminalController {
    fn mark_uncertain(&self) -> OperationError {
        self.uncertain.store(true, Ordering::Release);
        self.turn.cancel.cancel();
        transport("client terminal result is unknown")
    }

    async fn run(self, command: ClientCommand) -> Result<ExecResult, OperationError> {
        if self.turn.cancel.is_cancelled() {
            return Err(cancelled());
        }
        let deadline = Instant::now() + command.timeout;
        let request = acp::CreateTerminalRequest::new(self.session.clone(), command.command)
            .args(command.args)
            .cwd(Some(std::path::PathBuf::from(command.cwd)))
            .output_byte_limit(command.output_byte_limit);
        let create = self.connection.send_request(request).block_task();
        tokio::pin!(create);
        let create_deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
        let created = tokio::select! {
            biased;
            _ = self.turn.cancel.cancelled() => timeout(CONFIRM_TIMEOUT, &mut create).await,
            result = timeout_at(create_deadline, &mut create) => result,
        };
        let id = match created {
            Ok(Ok(response)) => response.terminal_id,
            Ok(Err(error)) if definitive_rejection(&error) => return Err(denied()),
            _ => return Err(self.mark_uncertain()),
        };

        let waited = tokio::select! {
            biased;
            _ = self.turn.cancel.cancelled() => Err(cancelled()),
            result = async {
                let request = acp::WaitForTerminalExitRequest::new(self.session.clone(), id.clone());
                timeout_at(deadline, self.connection.send_request(request).block_task()).await
            } => match result {
                Ok(Ok(response)) => Ok(Some(response)),
                Ok(Err(_)) => Err(transport("client terminal wait failed")),
                Err(_) => Ok(None),
            },
        };
        let result = match waited {
            Ok(Some(exit)) => {
                self.output(
                    &id,
                    exit.exit_status.exit_code,
                    command.output_byte_limit,
                    deadline,
                )
                .await
            }
            Ok(None) => Ok(ExecResult {
                timed_out: true,
                output: ExecOutput::Combined { truncated: false },
                ..Default::default()
            }),
            Err(error) => Err(error),
        };
        let kill_failed = if result.as_ref().map_or(true, |result| result.timed_out)
            || self.turn.cancel.is_cancelled()
        {
            let kill = acp::KillTerminalRequest::new(self.session.clone(), id.clone());
            !matches!(
                timeout(
                    TERMINAL_CLEANUP_TIMEOUT,
                    self.connection.send_request(kill).block_task()
                )
                .await,
                Ok(Ok(_))
            )
        } else {
            false
        };
        let release = acp::ReleaseTerminalRequest::new(self.session.clone(), id);
        if !matches!(
            timeout(
                TERMINAL_CLEANUP_TIMEOUT,
                self.connection.send_request(release).block_task()
            )
            .await,
            Ok(Ok(_))
        ) || kill_failed
        {
            return Err(self.mark_uncertain());
        }
        result
    }

    async fn output(
        &self,
        id: &acp::TerminalId,
        exit_code: Option<u32>,
        limit: u64,
        deadline: Instant,
    ) -> Result<ExecResult, OperationError> {
        let request = acp::TerminalOutputRequest::new(self.session.clone(), id.clone());
        let response = tokio::select! {
            biased;
            _ = self.turn.cancel.cancelled() => return Err(cancelled()),
            result = async {
                timeout_at(deadline.min(Instant::now() + REQUEST_TIMEOUT),
                    self.connection.send_request(request).block_task()).await
            } => result,
        };
        let response = response
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| transport("client terminal output failed"))?;
        if response.output.len() as u64 > limit {
            return Err(transport("client terminal exceeded the output limit"));
        }
        Ok(ExecResult {
            stdout: response.output.into_bytes(),
            exit_code: exit_code.and_then(|code| i32::try_from(code).ok()),
            output: ExecOutput::Combined {
                truncated: response.truncated,
            },
            ..Default::default()
        })
    }
}
