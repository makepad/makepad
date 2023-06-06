use crate::network::{
    HttpResponse,
    HttpRequestError,
    HttpResponseProgress,
    HttpUploadProgress
};

#[derive(Clone, Debug)]
pub struct HttpResponseEvent {
    pub response: HttpResponse,
}

#[derive(Clone, Debug)]
pub struct HttpRequestErrorEvent {
    pub request_error: HttpRequestError,
}

#[derive(Clone, Debug)]
pub struct HttpResponseProgressEvent {
    pub response_progress: HttpResponseProgress,
}

#[derive(Clone, Debug)]
pub struct HttpUploadProgressEvent {
    pub upload_progress: HttpUploadProgress,
}