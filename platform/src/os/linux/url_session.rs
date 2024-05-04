use std::{
    sync::{
        mpsc::Sender
    }
};

use makepad_http::client::HttpClient;
use makepad_url::Url;

use crate::{
    event::{
        HttpRequest,
        HttpResponse,
        NetworkResponse,
        NetworkResponseItem,
    },
    makepad_live_id::LiveId,
};


pub fn make_http_request(request_id: LiveId, request: HttpRequest, networking_sender: Sender<NetworkResponseItem>) {
    let url = Url::parse_string(request.url).unwrap();
    let headers = request.headers;
    let request_id = request_id.clone();
    HttpClient::new(url)
        .set_headers(headers)
        .get(move |response_code, response| {
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
