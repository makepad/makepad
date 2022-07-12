// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use makepad_http::server::*;
use std::{
    net::SocketAddr,
    sync::mpsc,
    io::prelude::*,
    fs::File,
};

fn main() {
    let (tx_socket, rx_socket) = mpsc::channel::<HttpMessage> ();
    
    start_http_server(HttpServer{
        listen_address:SocketAddr::from(([0, 0, 0, 0], 8080)),
        post_max_size: 1024*1024,
        request: tx_socket
    });
    while let Ok(message) = rx_socket.recv() {
        match message{
            HttpMessage::WebSocketMessageBinary{data, response}=>{
            }
            HttpMessage::HttpGet{headers, response}=>{
                let path = &headers.path;
                
                if path == "/$watch"{
                    let header = format!(
                        "HTTP/1.1 200 OK\r\n
                            Cache-Control: max-age:0\r\n\
                            Connection: close\r\n\r\n",
                    );
                    let _ = response.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                if path == "/favicon.ico"{
                    let header = format!("HTTP/1.1 200 OK\r\n\r\n");
                    let _ = response.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                let mime_type = if path.ends_with(".html") {"text/html"}
                else if path.ends_with(".wasm") {"application/wasm"}
                else if path.ends_with(".css") {"text/css"}
                else if path.ends_with(".js") {"text/javascript"}
                else if path.ends_with(".ttf") {"application/ttf"}
                else {continue};
                
                if path.contains("..") || path.contains("//") || path.contains("\\"){
                    continue
                }
                if let Some(base) = path.strip_prefix("/makepad/"){
                    if let Ok(mut file_handle) = File::open(base) {
                        let mut body = Vec::<u8>::new();
                        if file_handle.read_to_end(&mut body).is_ok() {
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                                    Content-encoding: none\r\n\
                                    Cache-Control: max-age:0\r\n\
                                    Content-Length: {}\r\n\
                                    Connection: close\r\n\r\n",
                                mime_type,
                                body.len()
                            );
                            let _ = response.send(HttpResponse{header, body});
                        }
                    }
                }
            }
            HttpMessage::HttpPost{..}=>{//headers, body, response}=>{
            }
        }
    }
}
