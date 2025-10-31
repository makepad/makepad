use crate::script::net::*;

#[derive(Default)]
pub struct CxScriptData{
    pub web_sockets: Vec<CxScriptDataWebSocket>,
    pub http_requests: Vec<CxScriptDataHttp>,
}
