use makepad_relax_api::relax_api;
use crate::makepad_network::{NetworkRuntime, NetworkRuntimeHandle};
use crate::{net::*, run::*, task::*};
use std::sync::Arc;

#[relax_api(handle, default)]
#[derive(Default)]
pub struct ScriptStd {
    pub net: Option<NetworkRuntimeHandle>,
    pub data: ScriptData,
    host_io_only: bool,
}

#[relax_api]
impl ScriptStd {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn with_network_runtime(net: Arc<NetworkRuntime>) -> Self {
        Self::with_network_handle(net.into())
    }

    #[relax_api]
    pub fn with_network_handle(net: NetworkRuntimeHandle) -> Self {
        Self {
            net: Some(net),
            data: ScriptData::default(),
            host_io_only: false,
        }
    }

    #[inline(always)]
    pub fn set_network_runtime(&mut self, net: Arc<NetworkRuntime>) {
        self.set_network_handle(net.into());
    }

    pub fn set_network_handle(&mut self, net: NetworkRuntimeHandle) {
        if !self.host_io_only {
            self.net = Some(net);
        }
    }

    /// Restrict a fresh script context to host-mediated external I/O.
    ///
    /// This cannot be relaxed by script code. Call before evaluating guest code;
    /// callers must not transfer existing network/process handles into the context.
    #[relax_api]
    pub fn restrict_to_host_io(&mut self) {
        self.net = None;
        self.host_io_only = true;
    }

    #[relax_api]
    pub fn host_io_only(&self) -> bool {
        self.host_io_only
    }

    /// Sets the host-io restriction for the HOST's own registration code that runs in a
    /// restricted context after it was restricted (a Splash isolate's lazily registered widget
    /// library loads its fonts); returns the previous value to put back. The network handle is
    /// not given back. Never call it around guest code.
    #[relax_api]
    pub fn set_host_io_only_for_host_code(&mut self, host_io_only: bool) -> bool {
        std::mem::replace(&mut self.host_io_only, host_io_only)
    }
}

#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptData {
    pub tasks: ScriptTasks,
    pub child_processes: Vec<ScriptChildProcessState>,
    pub web_sockets: Vec<ScriptWebSocket>,
    pub socket_streams: std::rc::Rc<std::cell::RefCell<Vec<ScriptSocketStream>>>,
    pub http_requests: Vec<ScriptHttp>,
    pub http_servers: Vec<ScriptHttpServer>,
}

unsafe impl makepad_script::ScriptHostCapability for ScriptStd {
    const ID: makepad_script::LiveId = makepad_script::LiveId::from_str("makepad.script.std.v1");
}
