use makepad_http::server::*;
use makepad_collab_server::{
    NotificationSender,
    CollabClientAction,
    CollabNotification,
    CollabRequest,
    CollabServer,
    makepad_micro_serde::*
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::mpsc,
    io::prelude::*,
    fs::File,
};

#[derive(Clone)]
struct CollabNotificationSender{
    sender: mpsc::Sender<Vec<u8>>,
} 

impl NotificationSender for CollabNotificationSender{
    fn box_clone(&self) -> Box<dyn NotificationSender> {
        Box::new(self.clone())
    }
    
    fn send_notification(&self, notification: CollabNotification) {
        let mut buf = Vec::new();
        CollabClientAction::Notification(notification).ser_bin(&mut buf);
        let _ = self.sender.send(buf);
    }
}

fn main() {
    let (tx_request, rx_request) = mpsc::channel::<HttpRequest> ();
    
    #[cfg(target_os = "linux")]
    let addr = SocketAddr::from(([0, 0, 0, 0], 80));
    #[cfg(target_os = "macos")]
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    
    start_http_server(HttpServer{
        listen_address:addr,
        post_max_size: 1024*1024,
        request: tx_request
    });
    println!("Server listening on {}", addr);
    let mut clb_server = CollabServer::new("./");
    let mut clb_connections = HashMap::new();
    let prefixes = [
        format!("/makepad/{}/",std::env::current_dir().unwrap().display()),
        "/makepad/".to_string()
    ];
    while let Ok(message) = rx_request.recv() {
        match message{
            HttpRequest::ConnectWebSocket {web_socket_id, response_sender, ..}=>{
                let sender = CollabNotificationSender{
                    sender:response_sender
                };
                clb_connections.insert(
                    web_socket_id,
                    clb_server.connect(Box::new(sender))
                );
            },
            HttpRequest::DisconnectWebSocket {web_socket_id}=>{
                // eddy do something here
                clb_connections.remove(&web_socket_id);
            },
            HttpRequest::BinaryMessage {web_socket_id, response_sender, data}=>{
                if let Some(connection) = clb_connections.get(&web_socket_id){
                    // turn data into a request
                    if let Ok(request) = CollabRequest::de_bin(&mut 0, &data){
                        let response = connection.handle_request(request);
                        let mut buf = Vec::new();
                        CollabClientAction::Response(response).ser_bin(&mut buf);
                        let _ = response_sender.send(buf);
                    }
                }
            }
            HttpRequest::Get{headers, response_sender}=>{
                let path = &headers.path;
                
                if path == "/$watch"{
                    let header = format!(
                        "HTTP/1.1 200 OK\r\n\
                            Cache-Control: max-age:0\r\n\
                            Connection: close\r\n\r\n",
                    );
                    let _ = response_sender.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                if path == "/favicon.ico"{
                    let header = format!("HTTP/1.1 200 OK\r\n\r\n");
                    let _ = response_sender.send(HttpResponse{header, body:vec![]});
                    continue
                }
                
                let mime_type = if path.ends_with(".html") {"text/html"}
                else if path.ends_with(".wasm") {"application/wasm"}
                else if path.ends_with(".css") {"text/css"}
                else if path.ends_with(".js") {"text/javascript"}
                else if path.ends_with(".ttf") {"application/ttf"}
                else if path.ends_with(".png") {"image/png"}
                else {continue};
                
                if path.contains("..") || path.contains("\\"){
                    continue
                }
                
                let strip = if let Some(strip) = path.strip_prefix(&prefixes[0]){Some(strip)}
                else if let Some(strip) = path.strip_prefix(&prefixes[1]){Some(strip)}
                else {None};

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
