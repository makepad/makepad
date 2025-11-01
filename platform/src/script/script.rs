use crate::script::net::*;
use crate::script::std::*;

#[derive(Default)]
pub struct CxScriptData{
    pub timers: CxScriptTimers,
    pub web_sockets: Vec<CxScriptWebSocket>,
    pub http_requests: Vec<CxScriptHttp>,
}
