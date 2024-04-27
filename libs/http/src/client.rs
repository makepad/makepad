use std::io::{BufRead, BufReader, Read};
use std::collections::BTreeMap;
use std::sync::{Arc};
use std::thread::{spawn, JoinHandle};

use makepad_url::Url;
use makepad_net::tcp::Socket;
use crate::server::HttpServerResponse;


#[derive(Debug)]
enum HttpResponseError {
    InvalidStatusLine,
    InvalidStatusCode
}

fn parse_header_line(header_line: String) -> (String, Vec<String>) {
    let (key, value) = header_line.split_once(":").unwrap();
    (key.trim().to_string(), vec![value.trim().to_string()])
}

fn prepare_headers(method: String, url: Url, headers: BTreeMap<String, Vec<String>>) -> Vec<String> {
    let mut payload: Vec<String> = Vec::new();
    let request_line = format!("{} {} HTTP/1.1", method, url.pathname);
    payload.push(request_line);
    payload.push(format!("Host: {}", url.hostname));
    for (key, values) in headers {
        payload.push(format!("{}: {}", key, values.join("")))
    }
    payload.push(String::new());
    payload.push(String::new());
    payload
}

fn parse_response_status(status_line: String) -> Result<(u16, String), HttpResponseError> {
    let mut split: Vec<&str> = status_line.splitn(3," ").collect();
    if split.len() == 2 {
        split.push("");
    }
    if split.len() != 3 {
        return Err(HttpResponseError::InvalidStatusLine);
    }
    let status_str: &str = split[1];
    match status_str.parse() {
        Ok(code) => Ok((code, split[2].to_string())),
        _ => Err(HttpResponseError::InvalidStatusCode)
    }
}

pub struct HttpClient {
    url: Url,
    headers: Option<BTreeMap<String, Vec<String>>>,
    socket: Option<Arc<Socket>>
}

impl HttpClient {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            headers: None,
            socket: None
        }
    }

    pub fn set_socket(mut self, socket: Arc<Socket>) -> Self {
        self.socket = Some(socket);
        self
    }

    pub fn set_headers(mut self, headers: BTreeMap<String, Vec<String>>) -> Self {
        self.headers = Some(headers);
        self
    }

    pub fn get<T: Send + 'static>(self, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T)  -> JoinHandle<T> {
        self.send("GET".into(), None, on_complete)
    }

    pub fn post<T: Send + 'static>(self, body: Option<&[u8]>, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T)  -> JoinHandle<T> {
        self.send("POST".into(), body, on_complete)
    }

    pub fn put<T: Send + 'static>(self, body: Option<&[u8]>, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T)  -> JoinHandle<T> {
        self.send("PUT".into(), body, on_complete)
    }

    pub fn patch<T: Send + 'static>(self, body: Option<&[u8]>, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T)  -> JoinHandle<T> {
        self.send("PATCH".into(), body, on_complete)
    }

    pub fn delete<T: Send + 'static>(self, body: Option<&[u8]>, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T)  -> JoinHandle<T> {
        self.send("DELETE".into(), body, on_complete)
    }

    fn create_socket(&mut self) -> Socket {
        let port = self.url.port.unwrap();
        let host = self.url.host.as_str();
        Socket::bind(host, port, self.url.secure).unwrap()
    }

    fn send<T: Send + 'static>(mut self, method: &str, body: Option<&[u8]>, on_complete: impl 'static + Send + FnOnce(u16, HttpServerResponse) -> T) -> JoinHandle<T> {
        let url = self.url.to_owned();
        let raw_headers= self.headers.to_owned();
        let headers = prepare_headers(method.into(), url, raw_headers.clone().unwrap_or_default());
        let socket = match self.socket.clone() {
            Some(s) => s.clone(),
            None => Arc::new(self.create_socket())
        };
        let binding = socket.clone();
        let mut output_stream = binding.output_stream.lock().unwrap();
        output_stream.write_all(headers.join("\r\n").as_bytes()).unwrap();
        if let Some(body) = body {
            output_stream.write_all(body).unwrap();
        }
        spawn(move || {
            let (status_code, response) = self.read_response(socket);
            return on_complete(status_code, response);
        })
    }

    fn read_response(&mut self, socket: Arc<Socket>) -> (u16, HttpServerResponse) {
        let mut input_stream = socket.input_stream.lock().unwrap();
        let mut reader = BufReader::new(&mut *input_stream);
        let mut done = false;
        let mut status_line = String::new();
        let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut len = reader.read_line(&mut status_line).unwrap();
        let (status_code, _) = parse_response_status(status_line).unwrap();
        while len > 0 && !done {
            let mut line = String::new();
            len = reader.read_line(&mut line).unwrap();
            if line.trim() == "" {
                done = true;
            } else {
                let (key, values) = parse_header_line(line);
                headers.insert(key.to_lowercase(), values);
            }
        }
        let body = match headers.get("content-length") {
            Some(length_str) => {
                let size: usize = length_str[0].parse().unwrap();
                let mut body: Vec<u8> = vec![0; size];
                reader.read_exact(&mut body).unwrap();
                body
            },
            _ => Vec::new()
        };
        let response = HttpServerResponse {
            header: headers.iter().map(
                |(key, values)| format!("{}: {}", key, values.join(""))
            ).collect::<Vec<_>>().join("\r\n"),
            body
        };
        (status_code, response)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use makepad_url::Url;
    use crate::client::{HttpClient, prepare_headers};

    #[test]
    fn generate_request_with_headers() {
        let url = Url::parse("http://localhost:8080/").unwrap();
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_string(), vec!["Bearer token".to_string()]);
        headers.insert("Content-Type".to_string(), vec!["application/json".to_string()]);
        let payload = prepare_headers("GET".to_string(), url, headers);
        assert_eq!(payload.len(), 5);
        assert_eq!(payload[0], "GET / HTTP/1.1");
        assert_eq!(payload[1], "Host: localhost:8080");
        assert_eq!(payload[2], "Authorization: Bearer token");
        assert_eq!(payload[3], "Content-Type: application/json");
        assert_eq!(payload[4], "");
    }

    #[async_std::test]
    async fn it_works() {
        let http = HttpClient::new("http://localhost:8080/".into());
        http.get(move |status_code, _| {
            assert_eq!(status_code, 200);
        }).join().unwrap();
    }
}