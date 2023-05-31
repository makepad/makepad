use crate::network::{HttpResponse, HttpRequestError};

#[derive(Clone, Debug)]
pub struct HttpResponseEvent {
    pub response: HttpResponse,
}

#[derive(Clone, Debug)]
pub struct HttpRequestErrorEvent {
    pub request_error: HttpRequestError,
}