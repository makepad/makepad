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
    io::BufReader,
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
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    
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
                let (tx_body, rx_body) = mpsc::channel::<HttpChunk> ();
                let (tx_ready, rx_ready) = mpsc::channel::<()> ();
                if path == "/$watch"{
                    let header = format!(
                        "HTTP/1.1 200 OK\r\n\
                            Cache-Control: max-age:0\r\n\
                            Connection: close\r\n\r\n",
                    );
                    let _ = response_sender.send(HttpResponse{header, rx_body, tx_ready});
                    continue
                }
                
                if path == "/favicon.ico"{
                    let header = format!("HTTP/1.1 200 OK\r\n\r\n");
                    let _ = response_sender.send(HttpResponse{header, rx_body, tx_ready});
                    continue
                }
                
                let mime_type = if path.ends_with(".html") {"text/html"}
                else if path.ends_with(".wasm") {"application/wasm"}
                else if path.ends_with(".css") {"text/css"}
                else if path.ends_with(".js") {"text/javascript"}
                else if path.ends_with(".ttf") {"application/ttf"}
                else if path.ends_with(".png") {"image/png"}
                else if path.ends_with(".mp4") {"application/octet-stream"}
                else {continue};
                
                if path.contains("..") || path.contains("\\"){
                    continue
                }
                
                let strip = if let Some(strip) = path.strip_prefix(&prefixes[0]){Some(strip)}
                else if let Some(strip) = path.strip_prefix(&prefixes[1]){Some(strip)}
                else {None};

                if let Some(base) = strip{
                    if let Ok(file_handle) = File::open(base) {
                        // lets fetch the filesize
                        let file_len = fs::metadata(base).unwrap().len();
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
                            file_len
                        );
                        let _ = response_sender.send(HttpResponse{header, rx_body, tx_ready});
                        // if more than 16 megs start a thread
                        if file_len > 16*1024*1024{
                            let base = base.to_string();
                            std::thread::spawn(move || {
                                let mut buf = [0u8;1400];
                                let mut reader = BufReader::new(file_handle);
                                let mut counter = 0;
                                let mut skipper = 0;
                                while let Ok(len) = reader.read(&mut buf){
                                    counter += len;
                                    if tx_body.send(HttpChunk{buf, len}).is_err(){
                                        break
                                    };
                                    if skipper%10 == 0{
                                        println!("Sending {} byte {}k of {}k - {:.2}%", base, counter/1024, file_len/1024, (counter as f64 / file_len as f64) * 100.0);
                                    } 
                                    skipper += 1;
                                    if rx_ready.recv().is_err(){
                                        break;
                                    }
                                }
                            });
                        }
                        else{ // otherwise do it here in one go
                            let mut buf = [0u8;1400];
                            let mut reader = BufReader::new(file_handle);
                            while let Ok(len) = reader.read(&mut buf){
                                if tx_body.send(HttpChunk{buf, len}).is_err(){
                                    break
                                };
                            }
                        }
                    }
                }
            }
            HttpRequest::Post{..}=>{//headers, body, response}=>{
            }
        }
    }
}
