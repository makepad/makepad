// this is the simplest local development http server you can write in Rust

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::sync::{mpsc, Arc, Mutex};
use std::io::prelude::*;
use std::io::BufReader;
use std::str;
use std::time::Duration;

#[derive(Default)]
pub struct HttpServer {
    pub watcher_id: u64,
    pub watchers: Vec<(u64, mpsc::Sender<String>)>,
    pub files_read: Vec<String>,
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
}

impl HttpServer {
    pub fn start_http_server(port: u16, bind_ip:[u8;4], root_dir: &str) -> Arc<Mutex<HttpServer>> {
        let listen_address = SocketAddr::from((bind_ip, port));

        let listener = TcpListener::bind(listen_address).expect("Cannot bind server address");
        let root_dir = root_dir.to_string();

        let http_server_root = Arc::new(Mutex::new(HttpServer::default()));
        let http_server = Arc::clone(&http_server_root);
        
        // the file signal message forwarder queue
        
        let listen_thread = std::thread::spawn(move || {
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = tcp_stream.expect("Incoming stream failure");
                let (tx_write, rx_write) = mpsc::channel::<String>();
                let mut reader = BufReader::new(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                let root_dir = root_dir.clone();
                let http_server = Arc::clone(&http_server);
                let _read_thread = std::thread::spawn(move || {
                    
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("http read line fail");
                    if !line.starts_with("GET /") || line.len() < 10{
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        return
                    }
                    
                    let line = &line[5..];
                    let space = line.find(' ').expect("http space fail");
                    let url = line[0..space].to_lowercase();
                    
                    if url.ends_with("key.bin") || url.find("..").is_some() || url.starts_with("/") {
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        return
                    }
                    if url.starts_with("$watch") { // its a watcher wait for the finish
                        let mut watcher_id = 0;
                        if let Ok(mut http_server) = http_server.lock() {
                            http_server.watcher_id += 1;
                            watcher_id = http_server.watcher_id;
                            http_server.watchers.push((watcher_id, tx_write));
                        };
                        match rx_write.recv_timeout(Duration::from_secs(30)) {
                            Ok(path) => { // let the watcher know
                                println!("Got filechange! {}", path);
                            },
                            Err(_) => { // close gracefully
                                
                            }
                        }
                        // remove from our watchers array
                        if let Ok(mut http_server) = http_server.lock() {
                            for i in 0..http_server.watchers.len(){
                                let (id, _) = &http_server.watchers[i];
                                if *id == watcher_id{
                                    http_server.watchers.remove(i);
                                    break
                                }
                            }
                        };
                        return
                    }

                    let file_path = format!("{}/{}", root_dir, url);

                    // keep track of the files we read
                    if let Ok(mut http_server) = http_server.lock() {
                        if http_server.files_read.iter().find(|v| **v == url).is_none(){
                            http_server.files_read.push(url.to_string());
                        }
                    };
                    
                    // lets read the file from disk and dump it back.
                    if let Ok(data) = std::fs::read(&file_path) {
                        let mime_type = if url.ends_with(".html") {"text/html"}
                        else if url.ends_with(".wasm") {"application/wasm"}
                        else if url.ends_with(".js") {"text/javascript"}
                        else {"application/octet-stream"};
                        
                        // write the header
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
        });
        if let Ok(mut http_server) = http_server_root.lock() {
            http_server.listen_thread = Some(listen_thread);
        };
        http_server_root
    }
    
    pub fn send_file_change(&mut self, path:&str){
        if !self.files_read.iter().find(|v| **v == path).is_none(){
            for (_, tx) in &self.watchers{
                let _ = tx.send(path.to_string());
            }
        }
    }
    
    pub fn join_threads(&mut self) {
        self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
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