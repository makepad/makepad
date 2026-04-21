use crate::makepad_network::NetworkRuntime;
use crate::{net::*, run::*, task::*};
use std::sync::Arc;

#[derive(Default)]
pub struct ScriptStd {
    pub net: Option<Arc<NetworkRuntime>>,
    pub data: ScriptData,
}

impl ScriptStd {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_network_runtime(net: Arc<NetworkRuntime>) -> Self {
        Self {
            net: Some(net),
            data: ScriptData::default(),
        }
    }

    pub fn set_network_runtime(&mut self, net: Arc<NetworkRuntime>) {
        self.net = Some(net);
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
