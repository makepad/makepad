
use crate::event::HttpRequest;
use crate::web_socket::{WebSocketMessage};
use std::sync::mpsc::{channel, Sender};
use std::net::TcpStream;
use std::io::{Read};
use makepad_http::utils::write_bytes_to_tcp_stream_no_error;
use makepad_http::websocket::{ServerWebSocket, ServerWebSocketMessageFormat, ServerWebSocketMessageHeader, ServerWebSocketMessage, SERVER_WEB_SOCKET_PONG_MESSAGE};

pub struct OsWebSocket{
    sender: Option<Sender<WebSocketMessage>>
}

impl OsWebSocket{
    pub fn send_message(&mut self, message:WebSocketMessage)->Result<(),()>{
        // lets encode the message into a membuffer and send it to the write thread
        if let Some(sender) = &mut self.sender{
            if sender.send(message).is_err(){
                return Err(());
            }
            return Ok(())
        }
        Err(())
    }
                    
    pub fn open(_socket_id:u64, request: HttpRequest, rx_sender:Sender<WebSocketMessage>)->OsWebSocket{
        // parse the url
        let split = request.split_url();
        // strip off any hashes
        // alright we have proto, host, port and hash now
        // lets open a tcpstream
        let stream = TcpStream::connect(format!("{}:{}", split.host, split.port));
        // alright lets construct a http request
        // lets join the headers
        
        let mut http_request = format!("GET /{} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n", split.file, split.host);
        http_request.push_str(&request.get_headers_string());
        http_request.push_str("\r\n"); 
        
        // lets write the http request
        if stream.is_err(){
            rx_sender.send(WebSocketMessage::Error("Error connecting websocket tcpstream".into())).unwrap();
            return OsWebSocket{sender:None}
        }
        let mut stream = stream.unwrap();
        if write_bytes_to_tcp_stream_no_error(&mut stream, http_request.as_bytes()){
            rx_sender.send(WebSocketMessage::Error("Error writing request to websocket".into())).unwrap();
            return OsWebSocket{sender:None}
        }
        
        // lets start the thread
        let mut input_stream = stream.try_clone().unwrap();
        let mut output_stream = stream.try_clone().unwrap();
        let (sender, receiver) = channel();
        
        let _writer_thread = std::thread::spawn(move || {
            while let Ok(msg) = receiver.recv(){
                match msg{
                    WebSocketMessage::Binary(data)=>{
                        let header = ServerWebSocketMessageHeader::from_len(data.len(), ServerWebSocketMessageFormat::Binary, false);
                        if write_bytes_to_tcp_stream_no_error(&mut output_stream, header.as_slice()) ||
                        write_bytes_to_tcp_stream_no_error(&mut output_stream, &data){
                            break;
                        }
                    }
                    WebSocketMessage::String(data)=>{
                        let header = ServerWebSocketMessageHeader::from_len(data.len(), ServerWebSocketMessageFormat::Binary, false);
                        if write_bytes_to_tcp_stream_no_error(&mut output_stream, header.as_slice()) ||
                        write_bytes_to_tcp_stream_no_error(&mut output_stream, &data.as_bytes()){
                            break;
                        }
                    }
                    _=>{
                        crate::error!("WebSocketMessage of this type sending not implemented");
                    }
                }
            }
        });
        
        let _reader_thread = std::thread::spawn(move || {
            let mut web_socket = ServerWebSocket::new();
            let mut done = false;
            while !done {
                let mut buffer = [0u8; 65535];
                match input_stream.read(&mut buffer) {
                    Ok(bytes_read) => {
                        web_socket.parse(&buffer[0..bytes_read], | result | {
                            match result {
                                Ok(ServerWebSocketMessage::Ping(_)) => {
                                    println!("ping!");
                                    if write_bytes_to_tcp_stream_no_error(&mut input_stream, &SERVER_WEB_SOCKET_PONG_MESSAGE){
                                        done = true;
                                        let _ = rx_sender.send(WebSocketMessage::Error("Pong message send failed".into()));
                                    }
                                },
                                Ok(ServerWebSocketMessage::Pong(_)) => {
                                },
                                Ok(ServerWebSocketMessage::Text(text)) => {
                                    if rx_sender.send(WebSocketMessage::String(text.into())).is_err(){
                                        done = true;
                                    };
                                    println!("text => {}", text);
                                },
                                Ok(ServerWebSocketMessage::Binary(data)) => {
                                    if rx_sender.send(WebSocketMessage::Binary(data.into())).is_err(){
                                        done = true;
                                    };
                                    println!("binary!");
                                },
                                Ok(ServerWebSocketMessage::Close) => {
                                    let _ = rx_sender.send(WebSocketMessage::Closed);
                                    done = true;
                                },
                                Err(e) => {
                                    eprintln!("Websocket error {:?}", e);
                                }
                            }
                        });                            
                    }
                    Err(e) => {
                        eprintln!("Failed to receive data: {}", e);
                    }
                }
            }
        });
                
        OsWebSocket{sender:Some(sender)}
    }
}