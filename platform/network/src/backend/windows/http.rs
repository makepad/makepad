use crate::blocking_http::{self, Error, Limits, Request};
use crate::types::{HttpError, HttpRequest, HttpResponse, NetworkResponse};
use makepad_live_id::LiveId;
use std::collections::BTreeMap;
use std::sync::mpsc::Sender;

pub struct WindowsHttpSocket;

impl WindowsHttpSocket {
    pub fn open(
        request_id: LiveId,
        request: HttpRequest,
        response_sender: Sender<NetworkResponse>,
    ) {
        std::thread::spawn(move || {
            let metadata_id = request.metadata_id;
            if let Err(error) = run(request_id, request, &response_sender) {
                let message = if error == Error::ResponseTooLarge {
                    crate::HTTP_BODY_LIMIT_ERROR.to_string()
                } else {
                    error.to_string()
                };
                let _ = response_sender.send(NetworkResponse::HttpError {
                    request_id,
                    error: HttpError { message, metadata_id },
                });
            }
        });
    }
}

fn run(
    request_id: LiveId,
    request: HttpRequest,
    response_sender: &Sender<NetworkResponse>,
) -> Result<(), Error> {
    let mut limits = Limits::default();
    limits.max_body_bytes =
        usize::try_from(request.max_response_body_bytes).unwrap_or(usize::MAX);
    let mut outbound = Request::with_method(request.url.clone(), request.method).limits(limits);
    for (name, values) in &request.headers {
        for value in values {
            outbound = outbound.header(name, value)?;
        }
    }
    outbound = outbound.body(request.body.clone().unwrap_or_default());
    let response = blocking_http::request_no_redirect(outbound)?;
    let mut headers = BTreeMap::<String, Vec<String>>::new();
    for (name, value) in response.headers {
        headers.entry(name).or_default().push(value);
    }

    if request.is_streaming {
        if !response.body.is_empty() {
            let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                request_id,
                response: HttpResponse::new(
                    request.metadata_id,
                    response.status,
                    BTreeMap::new(),
                    Some(response.body),
                ),
            });
        }
        let _ = response_sender.send(NetworkResponse::HttpStreamComplete {
            request_id,
            response: HttpResponse::new(
                request.metadata_id,
                response.status,
                headers,
                None,
            ),
        });
    } else {
        let _ = response_sender.send(NetworkResponse::HttpResponse {
            request_id,
            response: HttpResponse::new(
                request.metadata_id,
                response.status,
                headers,
                Some(response.body),
            ),
        });
    }
    Ok(())
}
