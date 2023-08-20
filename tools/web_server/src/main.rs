use makepad_http::server::*;

use std::{
    net::SocketAddr,
    sync::mpsc,
    io::prelude::*,
    fs::File,
};

fn main() {
    let (tx_request, rx_request) = mpsc::channel::<HttpRequest> ();
    
    #[cfg(target_os = "linux")]
    let addr = SocketAddr::from(([0, 0, 0, 0], 80));
    #[cfg(target_os = "macos")]
    let addr = SocketAddr::from(([127, 0, 0, 1], 61234));
    
    start_http_server(HttpServer{
        listen_address:addr,
        post_max_size: 1024*1024,
        request: tx_request
    });
    println!("Server listening on {}", addr);
    
    //let route_secret = fs::read_to_string("route_secret.txt").unwrap_or("\nNO\nACCESS\n".to_string()).trim().to_string();
    //let route_start = format!("/route/{}", route_secret);
    //let mut route_connections = HashMap::new();
    
    let prefixes = [
        format!("/makepad/{}/",std::env::current_dir().unwrap().display()),
        "/makepad//".to_string(),
        "/makepad/".to_string()
    ];
    while let Ok(message) = rx_request.recv() {
        match message{
            HttpRequest::ConnectWebSocket {web_socket_id:_, response_sender:_, headers:_}=>{
                
            },
            HttpRequest::DisconnectWebSocket {web_socket_id:_}=>{
                
            },
            HttpRequest::BinaryMessage {web_socket_id:_, response_sender:_, data:_}=>{
                
            }
            HttpRequest::Get{headers, response_sender}=>{
                let path = &headers.path;
                
                if path == "/$watch"{
                    let header = "HTTP/1.1 200 OK\r\n\
                            Cache-Control: max-age:0\r\n\
                            Connection: close\r\n\r\n".to_string();
                    let _ = response_sender.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                if path == "/favicon.ico"{
                    let header = "HTTP/1.1 200 OK\r\n\r\n".to_string();
                    let _ = response_sender.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                let mime_type = if path.ends_with(".html") {"text/html"}
                else if path.ends_with(".wasm") {"application/wasm"}
                else if path.ends_with(".css") {"text/css"}
                else if path.ends_with(".js") {"text/javascript"}
                else if path.ends_with(".ttf") {"application/ttf"}
                else if path.ends_with(".png") {"image/png"}
                else if path.ends_with(".jpg") {"image/jpg"}
                else if path.ends_with(".svg") {"image/svg+xml"}
                else {continue};

                if path.contains("..") || path.contains('\\'){
                    continue
                }
                
                let strip = path.strip_prefix(&prefixes[0]).or_else(|| path.strip_prefix(&prefixes[1])).or_else(|| path.strip_prefix(&prefixes[2]));

                if let Some(base) = strip{
                    if let Ok(mut file_handle) = File::open(base) {
                        let mut body = Vec::<u8>::new();
                        if file_handle.read_to_end(&mut body).is_ok() {
                            let header = format!(
                                "HTTP/1.1 200 OK\r\n\
                                Content-Type: {}\r\n\
                                Cross-Origin-Embedder-Policy: require-corp\r\n\
                                Cross-Origin-Opener-Policy: same-origin\r\n\
                                Content-encoding: none\r\n\
                                Cache-Control: max-age:0\r\n\
                                Content-Length: {}\r\n\
                                Connection: close\r\n\r\n",
                                mime_type,
                                body.len()
                            );
                            let _ = response_sender.send(HttpResponse{header, body});
                        }
                    }
                }
            }
            HttpRequest::Post{..}=>{//headers, body, response}=>{
            }
        }
    }
}
