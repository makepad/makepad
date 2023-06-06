use crate::makepad_live_id::*;
use crate::network::{
    HttpResponse,
    HttpResponseProgress,
    HttpUploadProgress
};

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
pub struct HttpResponseProgressEvent {
    pub response_progress: HttpResponseProgress,
}

#[derive(Clone, Debug)]
pub struct HttpUploadProgressEvent {
    pub upload_progress: HttpUploadProgress,
}