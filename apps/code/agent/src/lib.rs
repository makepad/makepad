#![allow(dead_code)] // the front ends consume this API; not every item is used yet

//! The agent core of Makepad Code: no UI types in here. The window
//! (makepad-app-code) and the terminal (makepad-code) both run an `Agent`
//! and consume its `AgentEvent`s (see `event`).
//!
//! One worker thread owns the model session(s), the tools and the transcript.
//! A front end sends `Command`s and drains `AgentEvent`s on its own schedule;
//! the wake hook fires after every event so an idle front end costs nothing.
//! `port::start` picks the real agent or, with `CODE_MOCK`, the scripted one
//! (`mock`); `index` is the repo's git-ignore-aware file list.

pub mod event;

pub mod context;
pub mod diff;
pub mod pictures;
mod engine;
mod hub_context;
pub mod index;
pub mod mock;
pub mod output;
pub mod port;
mod prompt;
mod remote;
mod risk;
pub mod runs;
mod sandbox;
mod scope_log;
pub mod session_store;
pub mod tools;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub use event::*;

pub struct Agent {
    commands: Sender<Command>,
    events: Receiver<AgentEvent>,
    worker: Option<JoinHandle<()>>,
}

impl Agent {
    /// Start the worker. It elects a model right away (see `Status` events),
    /// so the first `Submit` does not wait for the load.
    pub fn start(config: AgentConfig, wake: Arc<dyn Fn() + Send + Sync>) -> Agent {
        let (commands, command_rx) = channel();
        let (event_tx, events) = channel();
        let worker = std::thread::Builder::new()
            .name("code-agent".into())
            .spawn(move || engine::run(config, command_rx, event_tx, wake, Box::new(hub_context::NodeHub)))
            .expect("spawn agent worker");
        Agent { commands, events, worker: Some(worker) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Everything that happened since the last call, oldest first.
    pub fn poll(&self) -> Vec<AgentEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
