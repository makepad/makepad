use {
    crate::{
        event::{
            HttpRequest,
            HttpResponse,
            NetworkResponse,
            NetworkResponseItem,
        },
        makepad_live_id::LiveId,
    },
    std::{
        sync::{
            mpsc::Sender
        }
    },
    makepad_http::client::send
};
use makepad_http::client::HttpRequestOptions;
use makepad_url::Url;


pub fn make_http_request(request_id: LiveId, request: HttpRequest, networking_sender: Sender<NetworkResponseItem>) {
    let method = request.method.to_string().to_string();
    let url = Url::parse_string(request.url).unwrap();
    let options = HttpRequestOptions {
        headers: request.headers,
        body: request.body
    };
    let request_id = request_id.clone();

    send(method, url, options, move |response_code, response| {
        let response = HttpResponse::new(
            request.metadata_id,
            response_code,
            "".to_string(),
            Some(response.body),
        );
        handle_http_response(request_id, response, networking_sender);
    });
}

pub fn handle_http_response(request_id: LiveId, response: HttpResponse, networking_sender: Sender<NetworkResponseItem>) {
    let message = NetworkResponseItem {
        request_id,
        response: NetworkResponse::HttpResponse(response),
    };
    networking_sender.send(message).unwrap();
}
