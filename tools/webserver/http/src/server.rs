// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::io::prelude::*;
use std::sync::{mpsc};

use crate::websocket::{WebSocket, WebSocketMessage};
use crate::utils::*;

#[derive(Clone)]
pub struct HttpServer{
    pub listen_address: SocketAddr,
    pub request: mpsc::Sender<HttpMessage>,
    pub post_max_size: u64
}

pub struct HttpResponse{
    pub header: String,
    pub body: Vec<u8>
}

pub enum HttpMessage{
    WebSocketMessageBinary{
        data: Vec<u8>,
        response: mpsc::Sender<Vec<u8>>,
    },
    HttpGet{
        headers: HttpHeaders,
        response: mpsc::Sender<HttpResponse>,
    },
    HttpPost{
        headers: HttpHeaders,
        body: Vec<u8>,
        response: mpsc::Sender<HttpResponse>,
    }
}

/*
impl HttpGetConnection {
    fn get_mime_type(&self) -> Option<&'static str> {
        let path = &self.headers.path;
        if path.ends_with(".html") {Some("text/html")}
        else if path.ends_with(".wasm") {Some("application/wasm")}
        else if path.ends_with(".js") {Some("text/javascript")}
        else if path.ends_with(".ttf") {Some("application/ttf")}
        else {None}
    }
    
    fn handle_get(mut self) {
        let mime_type = self.get_mime_type();
        if mime_type.is_none() {
            return http_error_out(self.tcp_stream, 500);
        }
        let mime_type = mime_type.unwrap();
        
        // lets read the file from disk and return it
        
        /*
        if accept_encoding.contains("br") { // we want the brotli
            if let Some(brotli_filecache) = brotli_filecache.cache {
                if let Some(data) = brotli_filecache.get(path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: br\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                }
                else {
                    return http_error_out(tcp_stream, 404);
                }
            }
        }
        
        if accept_encoding.contains("gzip") || accept_encoding.contains("deflate") {
            if let Some(zlib_filecache) = zlib_filecache.cache {
                if let Some(data) = zlib_filecache.get(path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: deflate\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    return
                }
                else {
                    return http_error_out(tcp_stream, 404);
                }
            }
        }*/
        return http_error_out(self.tcp_stream, 500);
    }
}*/

pub fn start_http_server(
    http_server: HttpServer,
) ->  Option<std::thread::JoinHandle<() >> {
    
    let listener = if let Ok(listener) = TcpListener::bind(http_server.listen_address.clone()) {listener} else {println!("Cannot bind http server port"); return None};
    
    let listen_thread = {
        std::thread::spawn(move || {
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                    tcp_stream
                }
                else {
                    println!("Incoming stream failure");
                    continue
                };
                let http_server = http_server.clone();
                let _read_thread = std::thread::spawn(move || {
                    
                    let headers = HttpHeaders::from_tcp_stream(&mut tcp_stream);
                    if headers.is_none() {
                        return http_error_out(tcp_stream, 500);
                    }
                    let headers = headers.unwrap();
                    
                    if headers.sec_websocket_key.is_some() {
                        return handle_web_socket(http_server, tcp_stream, headers);
                    }
                    if headers.verb == "POST" {
                        return handle_post(http_server,tcp_stream, headers);
                    }
                    if headers.verb == "GET" {
                        return handle_get(http_server,tcp_stream, headers);
                    }
                    return http_error_out(tcp_stream, 500);
                });
            }
        })
    };
    Some(listen_thread)
}

fn handle_post(http_server:HttpServer, mut tcp_stream:TcpStream, headers:HttpHeaders) {
    // we have to have a content-length or bust
    if headers.content_length.is_none() {
        return http_error_out(tcp_stream, 500);
    }
    let content_length = headers.content_length.unwrap();
    if content_length > http_server.post_max_size{
        return http_error_out(tcp_stream, 500);
    }
    let bytes_total = content_length as usize;
    let mut body = Vec::new();
    body.resize(bytes_total, 0u8);
    // lets read content_length
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &mut body[(bytes_total - bytes_left)..bytes_total];
        let bytes_read = tcp_stream.read(buf);
        if bytes_read.is_err() {
            return http_error_out(tcp_stream, 500);
        }
        let bytes_read = bytes_read.unwrap();
        if bytes_read == 0 {
            return http_error_out(tcp_stream, 500);
        }
        bytes_left -= bytes_read;
    }
    // send our channel the post
    let (tx_socket, rx_socket) = mpsc::channel::<HttpResponse> ();
    if http_server.request.send(HttpMessage::HttpPost{
        headers,
        body,
        response: tx_socket
    }).is_err(){
        return http_error_out(tcp_stream, 500);
    };
    
    if let Ok(response) = rx_socket.recv() {
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

fn handle_web_socket(http_server:HttpServer, mut tcp_stream:TcpStream, headers:HttpHeaders) {
    let upgrade_response = WebSocket::create_upgrade_response(&headers.sec_websocket_key.unwrap());
    
    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, upgrade_response.as_bytes());
    
    let mut write_tcp_stream = tcp_stream.try_clone().unwrap();
    let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8 >> ();
    
    let _write_thread = std::thread::spawn(move || {
        // we have a bus we read from, which we hand to our websocket server.
        while let Ok(data) = rx_socket.recv() {
            write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
        }
        let _ = write_tcp_stream.shutdown(Shutdown::Both);
    });
    
    let mut web_socket = WebSocket::new();
    
    loop {
        let mut data = [0u8; 1024];
        match tcp_stream.read(&mut data) {
            Ok(n) => {
                if n == 0 {
                    let _  = tcp_stream.shutdown(Shutdown::Both);
                    return
                }
                web_socket.parse(&data[0..n], | result | {
                    match result {
                        Ok(WebSocketMessage::Ping) => {},
                        Ok(WebSocketMessage::Pong) => {},
                        Ok(WebSocketMessage::Text(_text)) => {
                        }
                        Ok(WebSocketMessage::Binary(data)) => {
                            if http_server.request.send(HttpMessage::WebSocketMessageBinary{
                                data: data.to_vec(),
                                response: tx_socket.clone()
                            }).is_err(){
                                let _ = tcp_stream.shutdown(Shutdown::Both);
                            };
                            // we have to send this to the websocket server
                            //x_bus.send((socket_id.clone(),data)).unwrap();
                            //let s = std::str::from_utf8(&data);
                        },
                        Ok(WebSocketMessage::Close) => {
                        }
                        Err(_) => {
                            eprintln!("Websocket error");
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                    }
                });
            }
            Err(_) => {
                let _ = tcp_stream.shutdown(Shutdown::Both);
                return
            }
        }
    }
}
/*
fn get_mime_type(&self) -> Option<&'static str> {
    let path = &self.headers.path;
    if path.ends_with(".html") {Some("text/html")}
    else if path.ends_with(".wasm") {Some("application/wasm")}
    else if path.ends_with(".js") {Some("text/javascript")}
    else if path.ends_with(".ttf") {Some("application/ttf")}
    else {None}
}*/

fn handle_get(http_server:HttpServer, mut tcp_stream: TcpStream, headers:HttpHeaders) {
        // send our channel the post
    let (tx_socket, rx_socket) = mpsc::channel::<HttpResponse> ();
    if http_server.request.send(HttpMessage::HttpGet{
        headers,
        response: tx_socket
    }).is_err(){
        return http_error_out(tcp_stream, 500);
    };
    
    if let Ok(response) = rx_socket.recv() {
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}
