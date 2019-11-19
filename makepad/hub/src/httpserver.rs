// this is the simplest local development http server you can write in Rust

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::sync::{mpsc, Arc, Mutex};
use std::io::prelude::*;
use std::io::BufReader;
use std::str;
use std::time::Duration;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HttpServerConfig {
    Offline,
    Localhost(u16),
    Network(u16),
    InterfaceV4((u16, [u8; 4]))
}

#[derive(Default)]
pub struct HttpServerShared {
    pub terminate: bool,
    pub watcher_id: u64,
    pub watch_pending: Vec<(u64, mpsc::Sender<String>)>,
    pub files_read: Vec<String>,
}

#[derive(Default)]
pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
    pub listen_address: Option<SocketAddr>,
    pub shared: Arc<Mutex<HttpServerShared>>,
}

impl HttpServer {
    pub fn start_http_server(config: &HttpServerConfig, projects_arc: Arc<Mutex<HashMap<String, String>>>) -> Option<HttpServer> {
        
        let listen_address = match config {
            HttpServerConfig::Offline => return None,
            HttpServerConfig::Localhost(port) => SocketAddr::from(([127, 0, 0, 1], *port)),
            HttpServerConfig::Network(port) => SocketAddr::from(([0, 0, 0, 0], *port)),
            HttpServerConfig::InterfaceV4((port, ip)) => SocketAddr::from((*ip, *port)),
        };
        
        let listener = if let Ok(listener) = TcpListener::bind(listen_address.clone()) {listener} else {println!("Cannot bind http server port"); return None};
        let projects = Arc::clone(&projects_arc);
        let shared = Arc::new(Mutex::new(HttpServerShared::default()));
        
        let listen_thread = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for tcp_stream in listener.incoming() {
                    if let Ok(shared) = shared.lock() {
                        if shared.terminate {
                            return
                        }
                    }
                    let mut tcp_stream = tcp_stream.expect("Incoming stream failure");
                    let (tx_write, rx_write) = mpsc::channel::<String>();
                    let mut reader = BufReader::new(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                    let projects = Arc::clone(&projects);
                    let shared = Arc::clone(&shared);
                    let _read_thread = std::thread::spawn(move || {
                        
                        let mut line = String::new();
                        reader.read_line(&mut line).expect("http read line fail");
                        if !line.starts_with("GET /") || line.len() < 10 {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            return
                        }
                        
                        let line = &line[5..];
                        let space = line.find(' ').expect("http space fail");
                        let url = &line[0..space];
                        let url_lc = url.to_string();
                        url_lc.to_lowercase();
                        if url_lc.ends_with("/key.ron") || url.find("..").is_some() || url.starts_with("/") {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            return
                        }
                        if url_lc.starts_with("$watch") { // its a watcher wait for the finish
                            let mut watcher_id = 0;
                            if let Ok(mut shared) = shared.lock() {
                                shared.watcher_id += 1;
                                watcher_id = shared.watcher_id;
                                shared.watch_pending.push((watcher_id, tx_write));
                            };
                            match rx_write.recv_timeout(Duration::from_secs(30)) {
                                Ok(msg) => { // let the watcher know
                                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, msg.as_bytes());
                                    let _ = tcp_stream.shutdown(Shutdown::Both);
                                },
                                Err(_) => { // close gracefully
                                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, "HTTP/1.1 201 Retry\r\n\r\n".as_bytes());
                                    let _ = tcp_stream.shutdown(Shutdown::Both);
                                }
                            }
                            // remove from our watchers array
                            if let Ok(mut shared) = shared.lock() {
                                for i in 0..shared.watch_pending.len() {
                                    let (id, _) = &shared.watch_pending[i];
                                    if *id == watcher_id {
                                        shared.watch_pending.remove(i);
                                        break
                                    }
                                }
                            };
                            return
                        }
                        
                        // lets look up the first part of url, the project.
                        let file_path = if let Some(file_pos) = url.find('/') {
                            let (project, rest) = url.split_at(file_pos);
                            let (_, rest) = rest.split_at(1);
                            // find the project
                            if let Ok(projects) = projects.lock() {
                                if let Some(abs_path) = projects.get(project) {
                                    Some(format!("{}/{}", abs_path, rest))
                                }
                                else {None}
                            }
                            else {None}
                        }
                        else {None};
                        
                        if file_path.is_none() {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            return
                        }
                        let file_path = file_path.unwrap();
                        
                        // keep track of the files we read
                        if let Ok(mut shared) = shared.lock() {
                            if shared.files_read.iter().find( | v | **v == url).is_none() {
                                shared.files_read.push(url.to_string());
                            }
                        };
                        
                        // lets read the file from disk and dump it back.
                        println!("HTTP Server serving file: {}", file_path);
                        if let Ok(data) = std::fs::read(&file_path) {
                            let mime_type = if url.ends_with(".html") {"text/html"}
                            else if url.ends_with(".wasm") {"application/wasm"}
                            else if url.ends_with(".js") {"text/javascript"}
                            else {"application/octet-stream"};
                            
                            // write the header
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-encoding: identity\r\nTransfer-encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                mime_type,
                                data.len()
                            );
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                        else { // 404
                            let _ = tcp_stream.write("HTTP/1.1 404 NotFound\r\n".as_bytes());
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                    });
                }
            })
        };
        Some(HttpServer {
            listen_thread: Some(listen_thread),
            listen_address: Some(listen_address.clone()),
            shared: shared,
        })
    }
    
    pub fn send_json_message(&mut self, json_msg: &str) {
        if let Ok(shared) = self.shared.lock() {
            for (_, tx) in &shared.watch_pending {
                let msg = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-encoding: identity\r\nTransfer-encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json_msg.len(),
                    json_msg
                );
                let _ = tx.send(msg);
            }
        }
    }
    
    pub fn send_file_change(&mut self, path: &str) {
        if let Ok(shared) = self.shared.lock() {
            if shared.files_read.iter().find( | v | **v == path).is_none() {
                return
            }
        }
        self.send_json_message(&format!("{{\"type\":\"file_change\",\"path\":\"{}\"}}", path));
    }
    
    pub fn send_build_start(&mut self) {
        self.send_json_message(&format!("{{\"type\":\"build_start\"}}"));
    }
    
    pub fn terminate(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.terminate = true;
            for (_, tx) in &shared.watch_pending {
                let _ = tx.send("HTTP/1.1 201 Retry\r\n\r\n".to_string());
            }
        }
        if let Some(listen_address) = self.listen_address {
            self.listen_address = None;
            // just do a single connection to the listen address to break the wait.
            if let Ok(_) = TcpStream::connect(listen_address) {
                self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
            }
        }
    }
}

fn write_bytes_to_tcp_stream_no_error(tcp_stream: &mut TcpStream, bytes: &[u8]) {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_written) = tcp_stream.write(buf) {
            if bytes_written == 0 {
                return
            }
            bytes_left -= bytes_written;
        }
        else {
            return
        }
    }
}