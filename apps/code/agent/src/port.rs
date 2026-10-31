//! The seam between a front end (the window, the terminal) and the agent:
//! one `AgentPort`, commands in, events out. `start` gives the real agent,
//! or the scripted `mock` with `CODE_MOCK` set.

use crate::event::{AgentConfig, AgentEvent, Command};
use std::sync::Arc;

/// Called from the agent's threads when there is something to `poll`; the
/// window turns it into a UI signal, so an idle front end costs nothing.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub trait AgentPort {
    fn send(&mut self, command: Command);
    /// Everything that arrived since the last call, in order.
    fn poll(&mut self) -> Vec<AgentEvent>;
}

impl AgentPort for crate::Agent {
    fn send(&mut self, command: Command) {
        crate::Agent::send(self, command)
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        crate::Agent::poll(self)
    }
}

/// The real agent; `CODE_MOCK=1` runs the scripted one instead (for building
/// a front end without a model).
pub fn start(config: AgentConfig, wake: Wake) -> Box<dyn AgentPort> {
    if std::env::var_os("CODE_MOCK").is_some() {
        return Box::new(crate::mock::MockAgent::start(config, wake));
    }
    Box::new(crate::Agent::start(config, wake))
}
