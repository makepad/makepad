use crate::makepad_live_id::*;
use crate::network::HttpResponse;

#[derive(Clone, Debug)]
pub struct HttpResponseEvent {
    pub response: HttpResponse,
}

#[derive(Clone, Debug)]
pub struct HttpRequestErrorEvent {
    pub id: LiveId,
    pub error: String
}

#[derive(Clone, Debug)]
pub struct HttpProgressEvent {
    pub id: LiveId,
    pub loaded: u32,
    pub total: u32
}