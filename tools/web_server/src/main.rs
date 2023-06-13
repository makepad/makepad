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
    fs,
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
    let addr = SocketAddr::from(([0, 0, 0, 0], 61234));
    
    start_http_server(HttpServer{
        listen_address:addr,
        post_max_size: 1024*1024,
        request: tx_request
    });
    println!("Server listening on {}", addr);
    let mut clb_server = CollabServer::new("./");
    let mut clb_connections = HashMap::new();
    
    let route_secret = fs::read_to_string("route_secret.txt").unwrap_or("\nNO\nACCESS\n".to_string()).trim().to_string();
    let route_start = format!("/route/{}", route_secret);
    let mut route_connections = HashMap::new();
    
    let prefixes = [
        format!("/makepad/{}/",std::env::current_dir().unwrap().display()),
        "/makepad/".to_string()
    ];
    while let Ok(message) = rx_request.recv() {
        match message{
            HttpRequest::ConnectWebSocket {web_socket_id, response_sender, headers}=>{
                if headers.path == route_start{
                    // plug this websocket into a route
                    // the client then routes messages to the child process
                    route_connections.insert(web_socket_id, response_sender);
                    continue;
                }
                
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
                route_connections.remove(&web_socket_id);
                clb_connections.remove(&web_socket_id);
            },
            HttpRequest::BinaryMessage {web_socket_id, response_sender, data}=>{
                if route_connections.get(&web_socket_id).is_some(){
                    // lets send it to everyone connected except us
                    for (other_id, sender) in &mut route_connections{
                        if *other_id != web_socket_id{
                            let _ = sender.send(data.clone());
                        }
                    }
                }
                else if let Some(connection) = clb_connections.get(&web_socket_id){
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
                else {continue};
                
                if path.contains("..") || path.contains('\\'){
                    continue
                }
                
                let strip = path.strip_prefix(&prefixes[0]).or_else(|| path.strip_prefix(&prefixes[1]));

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
