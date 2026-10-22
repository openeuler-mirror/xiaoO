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
use std::sync::{Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

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

/// Acquire before opening a writable executable descriptor. Move this guard
/// into a blocking task when it owns the descriptor; cancelling its caller must
/// not release the guard while blocking I/O still has the file open.
pub async fn write_permit() -> Permit<'static> {
    Acquire(Some(global().request(Group::Write))).await
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

/// Only the spawn call holds the permit; child I/O and wait happen afterwards.
pub async fn spawn_tokio(
    command: &mut tokio::process::Command,
) -> std::io::Result<tokio::process::Child> {
    let _permit = Acquire(Some(global().request(Group::Spawn))).await;
    command.spawn()
}

pub async fn spawn_std(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    let _permit = Acquire(Some(global().request(Group::Spawn))).await;
    command.spawn()
}

/// Synchronous creation on a blocking thread or multi-thread runtime.
pub fn spawn_std_blocking(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    let _permit = acquire_blocking(Group::Spawn);
    command.spawn()
}

#[cfg(test)]
#[path = "../../../tests/unit/process_coordination/coordination_test.rs"]
mod tests;
