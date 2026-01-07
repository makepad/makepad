use crate::script::net::*;
use crate::script::task::*;
use crate::script::timer::*;
use crate::script::run::*;

#[derive(Default)]
pub struct CxScriptData{
    pub random_seed: u64,
    pub tasks: CxScriptTasks,
    pub timers: CxScriptTimers,
    pub child_processes: Vec<CxScriptChildProcess>,
    pub web_sockets: Vec<CxScriptWebSocket>,
    pub http_requests: Vec<CxScriptHttp>,
    pub http_servers: Vec<CxScriptHttpServer>,
}
