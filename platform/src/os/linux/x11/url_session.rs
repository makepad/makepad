use {
    crate::{
        makepad_live_id::LiveId,
        event::{
            NetworkResponseItem,
            NetworkResponse,
            HttpRequest,
            HttpResponse,
            HttpError,
        },
    },
    std::collections::BTreeMap,
    std::io::{Read, Write},
    std::net::TcpStream,
    std::sync::mpsc::Sender,
};

#[derive(Default)]
pub struct X11HttpRequests{
}

impl X11HttpRequests{

    pub fn new()->Self{
        Self{}
    }

    pub fn cancel_http_request(&mut self, _request_id: LiveId) {
        // nop?
    }

    pub fn handle_response_item(&mut self, _item:&NetworkResponseItem) {
        // nop?
    }

    pub fn make_http_request(
        &mut self,
        request_id: LiveId,
        request: HttpRequest,
        response_sender: Sender<NetworkResponseItem>,
    ) {

        let method = request.method.to_string(); // Access fields directly
        let url = request.url;
        let body = request.body;
        let headers = request.headers;

        // Basic URL parsing (scheme, host, port) - sufficient for our needs.
        let (proto, rest) = url.split_once("://").ok_or("Invalid URL: Missing scheme").unwrap();
        if proto != "http" && proto != "https"{
            let _ = response_sender.send(NetworkResponseItem {
                request_id,
                response: NetworkResponse::HttpRequestError(HttpError {
                    metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                    message: format!("Invalid URL: only http/https supported"),
                }),
            });
            return;
        }

        let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
        let (host, port) = match host.split_once(':') {
            Some((host, port)) => (host, port.parse().unwrap_or(0)), // Attempt to parse port
            None => (host, if proto == "http" { 80 } else { 443 }),      // Default ports
        };

        if port == 0 {
            let _ = response_sender.send(NetworkResponseItem {
                request_id,
                response: NetworkResponse::HttpRequestError(HttpError {
                    metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                    message: format!("Invalid URL, no port"),
                }),
            });
            return;
        }

        let mut stream = match TcpStream::connect((host, port)) {
            Ok(stream) => stream,
            Err(e) => {
                let _ = response_sender.send(NetworkResponseItem {
                    request_id,
                    response: NetworkResponse::HttpRequestError(HttpError {
                        metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                        message: format!("HTTP connect error: {}", e),
                    }),
                });
                return;
            }
        };

        let mut http_request = format!("{} /{} HTTP/1.1\r\n", method, rest);
        http_request.push_str(&format!("Host: {}\r\n", host));
        //http_request.push_str(&format!("Connection: close\r\n"));
        for (key, values) in &headers {
            for value in values {
                http_request.push_str(&format!("{}: {}\r\n", key, value));
            }
        }
        http_request.push_str("\r\n");

        if let Err(e) =  stream.write_all(http_request.as_bytes()){
            let _ = response_sender.send(NetworkResponseItem {
                request_id,
                response: NetworkResponse::HttpRequestError(HttpError {
                    metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                    message: format!("HTTP write error: {}", e),
                }),
            });
            return;
        }
        if let Some(body) = body {
            if let Err(e) = stream.write_all(&body){
                let _ = response_sender.send(NetworkResponseItem {
                    request_id,
                    response: NetworkResponse::HttpRequestError(HttpError {
                        metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                        message: format!("HTTP write error: {}", e),
                    }),
                });
                return;
            }
        }

        let mut response = Vec::new();
        if let Err(e) =  stream.read_to_end(&mut response){
             let _ = response_sender.send(NetworkResponseItem {
                request_id,
                response: NetworkResponse::HttpRequestError(HttpError {
                    metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                    message: format!("HTTP read error: {}", e),
                }),
            });
            return;
        }
        let response = String::from_utf8_lossy(&response).to_string();

        let mut lines = response.lines();
        let status_line = match lines.next(){
            Some(status_line) => status_line,
            None=> {
                let _ = response_sender.send(NetworkResponseItem {
                    request_id,
                    response: NetworkResponse::HttpRequestError(HttpError {
                        metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                        message: format!("Empty HTTP Response"),
                    }),
                });
                return;
            }
        };
        let status_code:u16 = match status_line.split_whitespace().nth(1) {
            Some(status_code) => status_code.parse().unwrap_or(0),
            None => {
                let _ = response_sender.send(NetworkResponseItem {
                    request_id,
                    response: NetworkResponse::HttpRequestError(HttpError {
                        metadata_id: makepad_shader_compiler::makepad_live_compiler::LiveId(0),
                        message: format!("Invalid status line: {}", status_line),
                    }),
                });
                return;
            }
        };

        let mut headers = BTreeMap::new();
        while let Some(line) = lines.next() {
            if line.trim().is_empty() { // headers finished
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.entry(name.trim().to_string())
                       .or_insert_with(Vec::new)
                       .push(value.trim().to_string());
            }
        }

        let mut body_str = String::new();
        for line in lines{
            body_str.push_str(line);
        }

        let message = NetworkResponseItem {
            request_id,
            response: NetworkResponse::HttpResponse(HttpResponse {
                headers,
                metadata_id: request.metadata_id,
                status_code,
                body: Some(body_str.into_bytes()),
            }),
        };
        let _ = response_sender.send(message);
    }
}
