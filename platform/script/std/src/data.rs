use crate::makepad_network::NetworkRuntime;
use crate::{net::*, run::*, task::*};
use std::sync::Arc;

#[derive(Default)]
pub struct ScriptStd {
    pub net: Option<Arc<NetworkRuntime>>,
    pub data: ScriptData,
    host_io_only: bool,
}

impl ScriptStd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_network_runtime(net: Arc<NetworkRuntime>) -> Self {
        Self {
            net: Some(net),
            data: ScriptData::default(),
            host_io_only: false,
        }
    }

    pub fn set_network_runtime(&mut self, net: Arc<NetworkRuntime>) {
        if !self.host_io_only {
            self.net = Some(net);
        }
    }

    /// Restrict a fresh script context to host-mediated external I/O.
    ///
    /// This cannot be relaxed by script code. Call before evaluating guest code;
    /// callers must not transfer existing network/process handles into the context.
    pub fn restrict_to_host_io(&mut self) {
        self.net = None;
        self.host_io_only = true;
    }

    pub fn host_io_only(&self) -> bool {
        self.host_io_only
    }
}

#[derive(Default)]
pub struct ScriptData {
    pub tasks: ScriptTasks,
    pub child_processes: Vec<ScriptChildProcessState>,
    pub web_sockets: Vec<ScriptWebSocket>,
    pub socket_streams: std::rc::Rc<std::cell::RefCell<Vec<ScriptSocketStream>>>,
    pub http_requests: Vec<ScriptHttp>,
    pub http_servers: Vec<ScriptHttpServer>,
}
