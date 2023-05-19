use crate::network::HttpResponse;

#[derive(Clone, Debug)]
pub struct HttpResponseEvent {
    pub response: HttpResponse,
}