//! Coordinate writable executable descriptors with fork-to-exec intervals.
//!
//! Writers and process creators exclude one another, but each group can run
//! concurrently. A queued opposite group seals the current batch. On release,
//! the opposite queue has priority and its existing requests enter together.
//! No permit may be held while acquiring another permit or waiting for work
//! that needs the opposite group. Stalled I/O can stall the other group; callers
//! retain their cancellation and timeout policies.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use std::task::{Context, Poll, Waker};
use tokio::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Write = 0,
    Spawn = 1,
}

impl Group {
    fn opposite(self) -> Self {
        match self {
            Self::Write => Self::Spawn,
            Self::Spawn => Self::Write,
        }
    }
}

struct Entry {
    group: Group,
    admitted: bool,
    waker: Option<Waker>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    active: Option<Group>,
    holders: usize,
    sealed: bool,
    entries: HashMap<u64, Entry>,
    queues: [VecDeque<u64>; 2],
}

#[derive(Default)]
struct Coordinator {
    state: Mutex<State>,
    changed: Condvar,
}

impl Coordinator {
    fn request(&self, group: Group) -> Request<'_> {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id = state.next_id.checked_add(1).expect("permit id overflow");
        let admitted = state.active.is_none() || (state.active == Some(group) && !state.sealed);
        if admitted {
            state.active = Some(group);
            state.holders += 1;
        } else {
            state.sealed = true;
            state.queues[group as usize].push_back(id);
        }
        state.entries.insert(
            id,
            Entry {
                group,
                admitted,
                waker: None,
            },
        );
        Request {
            coordinator: self,
            id,
        }
    }

    fn release(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        let entry = state.entries.remove(&id).expect("live permit");
        if entry.admitted {
            state.holders -= 1;
        } else {
            state.queues[entry.group as usize].retain(|&queued| queued != id);
        }
        let mut wake = Vec::new();
        if state.holders == 0 {
            let previous = state.active.take().unwrap_or(entry.group);
            let next = [previous.opposite(), previous]
                .into_iter()
                .find(|group| !state.queues[*group as usize].is_empty());
            if let Some(group) = next {
                state.active = Some(group);
                // The batch consists only of requests already queued here.
                state.sealed = true;
                let batch = std::mem::take(&mut state.queues[group as usize]);
                state.holders = batch.len();
                for id in batch {
                    let entry = state.entries.get_mut(&id).unwrap();
                    entry.admitted = true;
                    if let Some(waker) = entry.waker.take() {
                        wake.push(waker);
                    }
                }
            } else {
                state.sealed = false;
            }
        }
        drop(state);
        self.changed.notify_all();
        for waker in wake {
            waker.wake();
        }
    }
}

struct Request<'a> {
    coordinator: &'a Coordinator,
    id: u64,
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        self.coordinator.release(self.id);
    }
}

struct Acquire<'a>(Option<Request<'a>>);

impl<'a> Future for Acquire<'a> {
    type Output = Permit<'a>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let request = this.0.as_ref().expect("acquire polled after completion");
        let mut state = request.coordinator.state.lock().unwrap();
        let entry = state.entries.get_mut(&request.id).unwrap();
        if !entry.admitted {
            entry.waker = Some(context.waker().clone());
            return Poll::Pending;
        }
        drop(state);
        Poll::Ready(Permit {
            _request: this.0.take().unwrap(),
        })
    }
}

/// Releases its group slot on return, error, cancellation or unwinding.
#[must_use]
pub struct Permit<'a> {
    _request: Request<'a>,
}

fn global() -> &'static Coordinator {
    static COORDINATOR: OnceLock<Coordinator> = OnceLock::new();
    COORDINATOR.get_or_init(Coordinator::default)
}

/// Acquire only on an execution thread that can perform the write directly.
/// Never enqueue work onto a runtime pool while holding this permit.
/// Async filesystem operations should use [`write_blocking`] instead.
pub async fn write_permit() -> Permit<'static> {
    Acquire(Some(global().request(Group::Write))).await
}

struct CancelWrite(Arc<AtomicBool>);

impl Drop for CancelWrite {
    fn drop(&mut self) {
        // Pair cancellation with the same mutex as condvar wait, avoiding a
        // lost wake between the worker's flag check and going to sleep.
        let state = global().state.lock().unwrap();
        self.0.store(true, Ordering::Release);
        drop(state);
        global().changed.notify_all();
    }
}

/// Enqueue first; acquire the write group only after occupying a blocking
/// worker. Cancellation before admission prevents the operation from starting.
/// Once I/O starts the worker keeps the guard until every descriptor closes.
pub async fn write_blocking<T: Send + 'static>(
    operation: impl FnOnce() -> T + Send + 'static,
) -> Result<Option<T>, tokio::task::JoinError> {
    let cancel = CancelWrite(Arc::new(AtomicBool::new(false)));
    let cancelled = Arc::clone(&cancel.0);
    let deadline = current_deadline();
    tokio::task::spawn_blocking(move || {
        let request = global().request(Group::Write);
        let mut state = global().state.lock().unwrap();
        loop {
            if cancelled.load(Ordering::Acquire) || deadline.is_some_and(|d| Instant::now() >= d) {
                drop(state);
                drop(request);
                return None;
            }
            if state.entries[&request.id].admitted {
                break;
            }
            state = if let Some(deadline) = deadline {
                global()
                    .changed
                    .wait_timeout(state, deadline.saturating_duration_since(Instant::now()))
                    .unwrap()
                    .0
            } else {
                global().changed.wait(state).unwrap()
            };
        }
        drop(state);
        let _permit = Permit { _request: request };
        Some(operation())
    })
    .await
}

tokio::task_local! {
    static DEADLINE: Instant;
}

/// Propagate an existing caller deadline through internal calls without
/// changing filesystem capability traits or request envelopes. Nested scopes
/// preserve the earlier deadline. This context is local to the polled task.
pub async fn with_deadline<F: Future>(deadline: Instant, future: F) -> F::Output {
    let deadline = current_deadline().map_or(deadline, |parent| parent.min(deadline));
    DEADLINE.scope(deadline, future).await
}

pub fn current_deadline() -> Option<Instant> {
    DEADLINE.try_with(|deadline| *deadline).ok()
}

fn expired(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

fn admission_timeout() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "process creation deadline elapsed before spawn",
    )
}

async fn spawn_permit(deadline: Option<Instant>) -> std::io::Result<Permit<'static>> {
    let request = Acquire(Some(global().request(Group::Spawn)));
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, request)
            .await
            .map_err(|_| admission_timeout()),
        None => Ok(request.await),
    }
}

// Synchronous application entry points run on the multi-thread application
// runtime or outside a runtime. Hand its worker back while waiting so queued
// async admissions on that worker can progress. Current-thread async callers
// must use the async entry points, or offload the entire synchronous operation.
fn acquire_blocking(group: Group) -> Permit<'static> {
    let acquire = || {
        let request = global().request(group);
        let mut state = global().state.lock().unwrap();
        while !state.entries[&request.id].admitted {
            state = global().changed.wait(state).unwrap();
        }
        drop(state);
        Permit { _request: request }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(acquire)
        }
        _ => acquire(),
    }
}

/// For synchronous file I/O on a blocking thread or multi-thread runtime.
pub fn write_permit_blocking() -> Permit<'static> {
    acquire_blocking(Group::Write)
}

/// Only spawn holds the permit; the deadline is checked after admission and
/// immediately before the system call, even if the outer timer was not polled.
pub async fn spawn_tokio(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    spawn_tokio_with_deadline(command, None).await
}

pub async fn spawn_tokio_with_deadline(
    command: &mut tokio::process::Command,
    deadline: Option<Instant>,
) -> std::io::Result<tokio::process::Child> {
    let deadline = match (deadline, current_deadline()) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let _permit = spawn_permit(deadline).await?;
    if expired(deadline) {
        return Err(admission_timeout());
    }
    command.spawn()
}

pub async fn spawn_std(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    let deadline = current_deadline();
    let _permit = spawn_permit(deadline).await?;
    if expired(deadline) {
        return Err(admission_timeout());
    }
    command.spawn()
}

/// Synchronous creation on a blocking thread or multi-thread runtime.
pub fn spawn_std_blocking(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    let deadline = current_deadline();
    let _permit = acquire_blocking(Group::Spawn);
    if expired(deadline) {
        return Err(admission_timeout());
    }
    command.spawn()
}

#[cfg(test)]
#[path = "../../../tests/unit/process_coordination/coordination_test.rs"]
mod tests;
