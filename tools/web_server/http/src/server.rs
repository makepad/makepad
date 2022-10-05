// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::io::prelude::*;
use std::sync::{mpsc, mpsc::{RecvTimeoutError}};
use std::time::Duration;

use crate::websocket::{WebSocket, WebSocketMessage, BinaryMessageHeader, PING_MESSAGE};
use crate::utils::*;

#[derive(Clone)]
pub struct HttpServer {
    pub listen_address: SocketAddr,
    pub request: mpsc::Sender<HttpRequest>,
    pub post_max_size: u64
}

pub struct HttpResponse {
    pub header: String,
    pub body: Vec<u8>
}

pub enum HttpRequest {
    ConnectWebSocket {
        web_socket_id: u64,
        headers:HttpHeaders,
        response_sender: mpsc::Sender<Vec<u8 >>,
    },
    DisconnectWebSocket {
        web_socket_id: u64,
    },
    BinaryMessage {
        web_socket_id: u64,
        response_sender: mpsc::Sender<Vec<u8 >>,
        data: Vec<u8>
    },
    Get {
        headers: HttpHeaders,
        response_sender: mpsc::Sender<HttpResponse>,
    },
    Post {
        headers: HttpHeaders,
        body: Vec<u8>,
        response: mpsc::Sender<HttpResponse>,
    }
}

pub fn start_http_server(
    http_server: HttpServer,
) -> Option<std::thread::JoinHandle<() >> {
    
    let listener = if let Ok(listener) = TcpListener::bind(http_server.listen_address.clone()) {listener} else {println!("Cannot bind http server port"); return None};
    
    let listen_thread = {
        std::thread::spawn(move || {
            let mut connection_counter = 0u64;
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                    tcp_stream
                }
                else {
                    println!("Incoming stream failure");
                    continue
                };
                let http_server = http_server.clone();
                connection_counter += 1;
                let _read_thread = std::thread::spawn(move || {
                    
                    let headers = HttpHeaders::from_tcp_stream(&mut tcp_stream);
                    if headers.is_none() {
                        return http_error_out(tcp_stream, 500);
                    }
                    let headers = headers.unwrap();
                    
                    if headers.sec_websocket_key.is_some() {
                        return handle_web_socket(http_server, tcp_stream, headers, connection_counter);
                    }
                    if headers.verb == "POST" {
                        return handle_post(http_server, tcp_stream, headers);
                    }
                    if headers.verb == "GET" {
                        return handle_get(http_server, tcp_stream, headers);
                    }
                    return http_error_out(tcp_stream, 500);
                });
            }
        })
    };
    Some(listen_thread)
}

fn handle_post(http_server: HttpServer, mut tcp_stream: TcpStream, headers: HttpHeaders) {
    // we have to have a content-length or bust
    if headers.content_length.is_none() {
        return http_error_out(tcp_stream, 500);
    }
    let content_length = headers.content_length.unwrap();
    if content_length > http_server.post_max_size {
        return http_error_out(tcp_stream, 500);
    }
    let bytes_total = content_length as usize;
    let mut body = Vec::new();
    body.resize(bytes_total, 0u8);
    
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
    
    let (tx_socket, rx_socket) = mpsc::channel::<HttpResponse> ();
    if http_server.request.send(HttpRequest::Post {
        headers,
        body,
        response: tx_socket
    }).is_err() {
        return http_error_out(tcp_stream, 500);
    };
    
    if let Ok(response) = rx_socket.recv() {
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

fn handle_web_socket(http_server: HttpServer, mut tcp_stream: TcpStream, headers: HttpHeaders, web_socket_id: u64) {
    let upgrade_response = WebSocket::create_upgrade_response(headers.sec_websocket_key.as_ref().unwrap());

    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, upgrade_response.as_bytes());
    
    let mut write_tcp_stream = tcp_stream.try_clone().unwrap();
    let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8 >> ();
    
    let _write_thread = std::thread::spawn(move || {
        // xx
        loop{
            match rx_socket.recv_timeout(Duration::from_millis(2000)){
                Ok(data)=>{
                    if data.len() == 0{
                        println!("Write socket closed");
                        break
                    }
                    let header = BinaryMessageHeader::from_len(data.len());
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &header.as_slice());
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
                },
                Err(RecvTimeoutError::Timeout)=>{ 
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &PING_MESSAGE);
                }
                Err(RecvTimeoutError::Disconnected)=>{
                    println!("Write socket closed");
                    break
                }
            }
        }
        let _ = write_tcp_stream.shutdown(Shutdown::Both);
    });
    
    if http_server.request.send(HttpRequest::ConnectWebSocket {
        headers,
        web_socket_id,
        response_sender: tx_socket.clone()
    }).is_err() {
        let _ = tcp_stream.shutdown(Shutdown::Both);
        return
    };
    
    let mut web_socket = WebSocket::new();
    loop {
        let mut data = [0u8; 65535];
        match tcp_stream.read(&mut data) {
            Ok(n) => {
                if n == 0 {
                    println!("Websocket closed");
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    let _ = tx_socket.send(Vec::new());
                    break 
                }
                web_socket.parse(&data[0..n], | result | {
                    match result {
                        Ok(WebSocketMessage::Ping(_)) => {
                        },
                        Ok(WebSocketMessage::Pong(_)) => {
                        },
                        Ok(WebSocketMessage::Text(_text)) => {
                            println!("Websocket text");
                        }
                        Ok(WebSocketMessage::Binary(data)) => {
                            if http_server.request.send(HttpRequest::BinaryMessage {
                                web_socket_id,
                                response_sender: tx_socket.clone(),
                                data: data.to_vec(),
                            }).is_err() {
                                eprintln!("Websocket message deserialize error");
                                let _ = tcp_stream.shutdown(Shutdown::Both);
                                let _ = tx_socket.send(Vec::new());
                            };
                        },
                        Ok(WebSocketMessage::Close) => {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                        Err(e) => {
                            eprintln!("Websocket error {:?}", e);
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            let _ = tx_socket.send(Vec::new());
                        }
                    }
                });
            }
            Err(_) => {
                println!("Websocket closed");
                let _ = tcp_stream.shutdown(Shutdown::Both);
                let _ = tx_socket.send(Vec::new());
                break;
            }
        }
    }
    
    let _ =  http_server.request.send(HttpRequest::DisconnectWebSocket {
        web_socket_id,
    });
}

fn handle_get(http_server: HttpServer, mut tcp_stream: TcpStream, headers: HttpHeaders) {
    // send our channel the post
    let (tx_socket, rx_socket) = mpsc::channel::<HttpResponse> ();
    if http_server.request.send(HttpRequest::Get {
        headers,
        response_sender: tx_socket
    }).is_err() {
        return http_error_out(tcp_stream, 500);
    };
    
    if let Ok(response) = rx_socket.recv() {
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
        write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}
