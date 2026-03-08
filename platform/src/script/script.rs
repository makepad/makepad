use crate::script::net::*;
use crate::script::res::*;
use crate::script::run::*;
use crate::script::task::*;
use crate::script::timer::*;
use crate::live_reload::CxLiveReloadState;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Default)]
pub struct CxScriptData {
    pub random_seed: u64,
    pub tasks: CxScriptTasks,
    pub timers: CxScriptTimers,
    pub resources: CxScriptResources,
    pub child_processes: Vec<CxScriptChildProcess>,
    pub web_sockets: Vec<CxScriptWebSocket>,
    pub socket_streams: Rc<RefCell<Vec<CxScriptSocketStream>>>,
    pub http_requests: Vec<CxScriptHttp>,
    pub http_servers: Vec<CxScriptHttpServer>,
    /// Shared reference to the VM's crate_manifests so we can access them
    /// even when script_vm is temporarily taken by with_vm during script eval.
    pub crate_manifests: Rc<RefCell<HashMap<String, String>>>,
    pub live_reload: CxLiveReloadState,
}
