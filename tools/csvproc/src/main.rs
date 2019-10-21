use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::sync::{mpsc};
use std::io::prelude::*;
use std::io::BufReader;
use std::str;

pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
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

impl HttpServer {
    fn start_http_server(port: u16, _root_dir: &str) -> HttpServer {
        let listen_address = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(listen_address).expect("Cannot bind server address");
        
        let listen_thread = std::thread::spawn(move || {
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = tcp_stream.expect("Incoming stream failure");
                let (tx_write, rx_write) = mpsc::channel::<String>();
                let mut reader = BufReader::new(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                
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
                    
                    if url.find("key.bin").is_some() || url.find("..").is_some() || url.starts_with("/") {
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        return
                    }
                    if url.starts_with("$watch") { // its a watcher wait for the finish
                        match rx_write.recv_timeout(std::time::Duration::from_secs(30)) {
                            Ok(_) => { // let the watcher know
                                
                            },
                            Err(_) => { // close gracefully
                                
                            }
                        }
                    }
                    // lets read the file from disk and dump it back.
                    if let Ok(data) = std::fs::read(&url) {
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
        
        HttpServer {
            listen_thread: Some(listen_thread)
        }
    }
    
    pub fn signal_file_change(&mut self) {
        
    }
    
    pub fn join_threads(&mut self) {
        self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
    }
}

fn main() {
    let mut http_server = HttpServer::start_http_server(2000, "");
    
    http_server.join_threads();
}
