use std::net::{TcpListener, SocketAddr, Shutdown};
use std::sync::{mpsc};
use std::io::prelude::*;
use std::str;

pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
}

impl HttpServer {
    fn start_http_server(port: u16, _root_dir: &str) -> HttpServer {
        let listen_address = SocketAddr::from(([127, 0, 0, 1], port));
        
        let listener = TcpListener::bind(listen_address).expect("Cannot bind server address");
        
        let listen_thread = {
            //let hub_log = hub_log.clone();
            std::thread::spawn(move || {
                for tcp_stream in listener.incoming() {
                    let tcp_stream = tcp_stream.expect("Incoming stream failure");

                    let (_tx_write, rx_write) = mpsc::channel::<Vec<u8>>();

                    let _read_thread = {
                        let mut tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        //let hub_log = hub_log.clone();
                        std::thread::spawn(move || {
                            let mut storage = Vec::new();
                            loop {
                                let offset = storage.len();
                                storage.resize(offset + 32, 0u8);
                                let new_len = storage.len(); 
                                let n_bytes_read = tcp_stream.read(&mut storage[offset..new_len]).expect("cannot read");
                                storage.resize(offset + n_bytes_read, 0u8);
                                    println!("read {}", n_bytes_read);
                                if n_bytes_read == 0{ 
                                    println!("END {:?}", storage);
                                    // done
                                    for (index, ch) in storage.iter().enumerate() {
                                        if *ch == '\n' as u8{
                                            if let Ok(line) = str::from_utf8(&storage[0..index]) {
                                                println!("GOT {}", line);
                                            }
                                        }
                                    }
                                    return;
                                }
                            }
                        })
                    };
                    let _write_thread = {
                        let mut _tcp_stream = tcp_stream.try_clone().expect("Cannot clone tcp stream");
                        //let hub_log = hub_log.clone();
                        std::thread::spawn(move || {
                            if let Ok(_write) = rx_write.recv() {
                                // write the data
                            }
                            // close the socket
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        })
                    };
                }
            })
        };
        
        HttpServer {
            listen_thread: Some(listen_thread)
        }
    }
    
    
    pub fn join_threads(&mut self) {
        self.listen_thread.take().expect("cant take listen thread").join().expect("cant join listen thread");
    }
}

fn main() {
    let mut http_server = HttpServer::start_http_server(2000, "");
    println!("HI");
    http_server.join_threads();
}
