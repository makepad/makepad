use crate::script::net::*;

#[derive(Default)]
pub struct CxScriptData{
    pub http_requests: Vec<CxScriptDataHttp>,
}
