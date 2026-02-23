use crate::event::{
    HttpError, HttpMethod, HttpRequest, HttpResponse, NetworkResponse, NetworkResponseItem,
};
use crate::makepad_live_id::LiveId;
use crate::web_socket::WebSocketMessage;

pub fn to_network_http_request(request: HttpRequest) -> makepad_network::HttpRequest {
    let method = match request.method {
        HttpMethod::GET => makepad_network::HttpMethod::Get,
        HttpMethod::HEAD => makepad_network::HttpMethod::Head,
        HttpMethod::POST => makepad_network::HttpMethod::Post,
        HttpMethod::PUT => makepad_network::HttpMethod::Put,
        HttpMethod::DELETE => makepad_network::HttpMethod::Delete,
        HttpMethod::CONNECT => makepad_network::HttpMethod::Connect,
        HttpMethod::OPTIONS => makepad_network::HttpMethod::Options,
        HttpMethod::TRACE => makepad_network::HttpMethod::Trace,
        HttpMethod::PATCH => makepad_network::HttpMethod::Patch,
    };

    makepad_network::HttpRequest {
        metadata_id: request.metadata_id.0,
        url: request.url,
        method,
        headers: request.headers,
        ignore_ssl_cert: request.ignore_ssl_cert,
        is_streaming: request.is_streaming,
        body: request.body,
    }
}

pub fn from_network_response_item(
    item: makepad_network::NetworkResponseItem,
) -> NetworkResponseItem {
    let response = match item.response {
        makepad_network::NetworkResponse::HttpRequestError(err) => {
            NetworkResponse::HttpRequestError(HttpError {
                message: err.message,
                metadata_id: LiveId(err.metadata_id),
            })
        }
        makepad_network::NetworkResponse::HttpResponse(resp) => {
            NetworkResponse::HttpResponse(from_network_http_response(resp))
        }
        makepad_network::NetworkResponse::HttpStreamResponse(resp) => {
            NetworkResponse::HttpStreamResponse(from_network_http_response(resp))
        }
        makepad_network::NetworkResponse::HttpStreamComplete(resp) => {
            NetworkResponse::HttpStreamComplete(from_network_http_response(resp))
        }
        makepad_network::NetworkResponse::HttpProgress(progress) => {
            NetworkResponse::HttpProgress(crate::event::HttpProgress {
                loaded: progress.loaded,
                total: progress.total,
            })
        }
    };

    NetworkResponseItem {
        request_id: LiveId(item.request_id),
        response,
    }
}

fn from_network_http_response(resp: makepad_network::HttpResponse) -> HttpResponse {
    HttpResponse {
        metadata_id: LiveId(resp.metadata_id),
        status_code: resp.status_code,
        headers: resp.headers,
        body: resp.body,
    }
}

pub fn to_network_response_item(
    item: NetworkResponseItem,
) -> makepad_network::NetworkResponseItem {
    makepad_network::NetworkResponseItem {
        request_id: item.request_id.0,
        response: match item.response {
            NetworkResponse::HttpRequestError(err) => {
                makepad_network::NetworkResponse::HttpRequestError(makepad_network::HttpError {
                    message: err.message,
                    metadata_id: err.metadata_id.0,
                })
            }
            NetworkResponse::HttpResponse(resp) => {
                makepad_network::NetworkResponse::HttpResponse(to_network_http_response(
                    resp,
                ))
            }
            NetworkResponse::HttpStreamResponse(resp) => {
                makepad_network::NetworkResponse::HttpStreamResponse(
                    to_network_http_response(resp),
                )
            }
            NetworkResponse::HttpStreamComplete(resp) => {
                makepad_network::NetworkResponse::HttpStreamComplete(
                    to_network_http_response(resp),
                )
            }
            NetworkResponse::HttpProgress(progress) => {
                makepad_network::NetworkResponse::HttpProgress(
                    makepad_network::HttpProgress {
                        loaded: progress.loaded,
                        total: progress.total,
                    },
                )
            }
        },
    }
}

fn to_network_http_response(resp: HttpResponse) -> makepad_network::HttpResponse {
    makepad_network::HttpResponse {
        metadata_id: resp.metadata_id.0,
        status_code: resp.status_code,
        headers: resp.headers,
        body: resp.body,
    }
}

pub fn to_network_ws_message(
    message: WebSocketMessage,
) -> makepad_network::WebSocketMessage {
    match message {
        WebSocketMessage::Error(err) => makepad_network::WebSocketMessage::Error(err),
        WebSocketMessage::Binary(data) => makepad_network::WebSocketMessage::Binary(data),
        WebSocketMessage::String(data) => makepad_network::WebSocketMessage::String(data),
        WebSocketMessage::Opened => makepad_network::WebSocketMessage::Opened,
        WebSocketMessage::Closed => makepad_network::WebSocketMessage::Closed,
    }
}

pub fn from_network_ws_message(
    message: makepad_network::WebSocketMessage,
) -> WebSocketMessage {
    match message {
        makepad_network::WebSocketMessage::Error(err) => WebSocketMessage::Error(err),
        makepad_network::WebSocketMessage::Binary(data) => WebSocketMessage::Binary(data),
        makepad_network::WebSocketMessage::String(data) => WebSocketMessage::String(data),
        makepad_network::WebSocketMessage::Opened => WebSocketMessage::Opened,
        makepad_network::WebSocketMessage::Closed => WebSocketMessage::Closed,
    }
}
