use std::io::prelude::*;
use makepad_http::websocket::{WebSocket, WebSocketMessage, BinaryMessageHeader};
use std::env;
use std::fs;
use std::sync::mpsc;
use std::net::{TcpStream};
use makepad_micro_serde::*;
use std::io::SeekFrom;

// this is the protocol enum that clients bounce between them.
// right now there is no clear 'server' defined, and you need a tiny webserver to act as a reflector,
// that can plug 2 websockets together.

#[derive(SerBin, DeBin)]
enum RouterMessage {
    FetchFile {name: String},
    FileSize {size: u64},
    FetchChunk {chunk: u64, hash: u64},
    ChunkSkipped {chunk: u64},
    ChunkDownloaded {chunk: u64, data: Vec<u8>}
}

const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len()<3 {
        println!("cargo run makepad-file-router <ip:port> <secret> <optional file to fetch>")
    }
    
    // lets open a socket to our reflector webserver
    let mut tcp_stream = TcpStream::connect(&args[1]).unwrap();
    
    // send it the http websocket upgrade
    let http_req = format!("GET /route/{} HTTP/1.1\r\nHost: makepad.nl\r\nConnection: upgrade\r\nUpgrade: websocket\r\nsec-websocket-key: x\r\n\r\n", args[2]);
    tcp_stream.write_all(http_req.as_bytes()).unwrap();
    
    // skip over response, we don't care. either it works or it doesn't
    let mut data = [0u8; 65535];
    tcp_stream.read(&mut data).unwrap();
    
    // create a new websocket parser instance 
    let mut web_socket = WebSocket::new();
    
    // lets start the websocket write loop
    let (tx_sender, rx_sender) = mpsc::channel::<RouterMessage>();
    std::thread::spawn({
        let mut tcp_stream = tcp_stream.try_clone().unwrap();
        move || loop {
            while let Ok(msg) = rx_sender.recv() {
                let mut bytes = Vec::new();
                msg.ser_bin(&mut bytes);
                let header = BinaryMessageHeader::from_len(bytes.len());
                if tcp_stream.write_all(&header.as_slice()).is_err() {
                    println!("tcp stream write error");
                    return
                };
                if tcp_stream.write_all(&bytes).is_err() {
                    println!("tcp stream write error");
                    return
                };
            }
        }
    });
    
    // the websocket read loop
    let (tx_receiver, rx_receiver) = mpsc::channel::<RouterMessage>();
    std::thread::spawn({
        let mut tcp_stream = tcp_stream.try_clone().unwrap();
        move || loop {
            let mut data = [0u8; 65535];
            match tcp_stream.read(&mut data) {
                Ok(n) => {
                    if n == 0 {
                        println!("tcp stream returns 0 bytes");
                        return
                    }
                    web_socket.parse(&data[0..n], | result | {
                        match result {
                            Ok(WebSocketMessage::Ping(_)) => {},
                            Ok(WebSocketMessage::Pong(_)) => {},
                            Ok(WebSocketMessage::Text(_text)) => {}
                            Ok(WebSocketMessage::Binary(data)) => {
                                // if we
                                let msg = DeBin::deserialize_bin(data).unwrap();
                                tx_receiver.send(msg).unwrap();
                            },
                            Ok(WebSocketMessage::Close) => {
                                println!("Websocket Close message received");
                                return
                            }
                            Err(e) => {
                                println!("Websocket parse error {:?}", e);
                                return
                            }
                        }
                    });
                }
                Err(_) => {
                    println!("tcp stream closed");
                    return
                }
            }
        }
    });
    
    let mut fetch_file = if args.len() == 4 { // if you pass it a 'fetch file' on the commandline it sends the fetch file message to the other end
        let name = args[3].clone();
        tx_sender.send(RouterMessage::FetchFile {name: name.clone()}).unwrap();
        Some(name)
    }
    else {
        None
    };
    
    let mut fetch_chunks = None;
    
    fn get_file_path(name: &str) -> String {
        format!("./{}", name)
    }
    
    // file interaction helper code
    
    fn get_file_len(name:&str)->u64{
        let path = get_file_path(name);
        fs::metadata(path).unwrap().len() as u64        
    }
    
    fn set_file_len(name: &str, len: u64) {
        let path = get_file_path(name);
        let file = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        file.set_len(len).unwrap();
    }

    fn read_chunk(name: &str, chunk: u64) -> Vec<u8> {
        let path = get_file_path(name);
        let mut file = fs::OpenOptions::new().read(true).open(path).unwrap();
        file.seek(SeekFrom::Start(chunk * CHUNK_SIZE)).unwrap();
        // lets read the chunk
        let mut data = Vec::new();
        data.resize(CHUNK_SIZE as usize, 0u8);
        if let Ok(len) = file.read(&mut data) {
            data.resize(len, 0u8);
            return data
        }
        else {
            panic!("File read failed")
        }
    }
    
    fn write_chunk(name: &str, chunk: u64, data: Vec<u8>) {
        let path = get_file_path(name);
        
        let mut file = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(chunk * CHUNK_SIZE)).unwrap();
        // lets read the chunk
        if let Ok(len) = file.write(&data) {
            if len != data.len() {
                panic!("File write failed")
            }

            let mut file = fs::File::create(format!("{}.last",path)).unwrap();
            file.write_all(format!("{}", chunk).as_bytes()).unwrap();
        }
        else {
            panic!("File read failed")
        }
    }
    
    // this is the main message loop for our clients .
    // it needs a bit of session/state handling as thats now not very clean.
    // the assumption here is uploader/ one downloader. but not hard to improve.
    
    while let Ok(msg) = rx_receiver.recv() {
        match msg {
            RouterMessage::FetchFile {name} => {
                // answer a fetch file with the file size in bytes
                if name.contains("..") || name.contains("\\") || name.contains("/") {
                    println!("Fetch file contains incorrect values {}", name);
                    continue
                }
                fetch_file = Some(name.clone());
                let size = get_file_len(&name);
                fetch_chunks = Some(size / CHUNK_SIZE);
                tx_sender.send(RouterMessage::FileSize {size}).unwrap();
            }
            RouterMessage::FileSize {size} => {
                // we received the file size, lets allocate it entirely on disk
                let fetch_file = fetch_file.as_ref().unwrap();
                fetch_chunks = Some(size / CHUNK_SIZE);
                println!("Setting file length of {} to {}, might take a while", fetch_file, size);
                set_file_len(fetch_file, size);
                println!("File length set done");
                // see if we have a 'start chunk' logged in the filename.last file so we can skip all chunk hashes on restart
                let start_chunk = if let Ok(mut file) = fs::File::open(format!("{}.last",get_file_path(fetch_file))){
                    let mut contents = String::new();
                    file.read_to_string(&mut contents).unwrap();
                    contents.parse().unwrap()
                }
                else{
                    0
                };

                // trigger the first chunk to start
                let hash = hash_bytes(&read_chunk(fetch_file, 0));
                tx_sender.send(RouterMessage::FetchChunk {chunk: start_chunk, hash}).unwrap();
            }
            RouterMessage::FetchChunk {chunk, hash} => {
                let data = read_chunk(fetch_file.as_ref().unwrap(), chunk);
                let old_hash = hash_bytes(&data);
                // someone wants a chunk, so we compare the hash request with the actual one
                // and then either send the data or say we skipped the chunk
                if old_hash == hash {
                    println!("FetchChunk {} {}/{} skipped {:.2}%", fetch_file.as_ref().unwrap(), chunk, fetch_chunks.unwrap(), (chunk as f64 / fetch_chunks.unwrap() as f64) * 100.0);
                    tx_sender.send(RouterMessage::ChunkSkipped {chunk}).unwrap();
                }
                else {
                    println!("FetchChunk {} {}/{} uploading {:.2}%", fetch_file.as_ref().unwrap(), chunk, fetch_chunks.unwrap(), (chunk as f64 / fetch_chunks.unwrap() as f64) * 100.0);
                    tx_sender.send(RouterMessage::ChunkDownloaded {chunk, data}).unwrap();
                }
            }
            RouterMessage::ChunkSkipped {chunk} => {
                // the other side said the chunk is identical, so skip it
                println!("ChunkSkipped {} {}/{} {:.2}%", fetch_file.as_ref().unwrap(), chunk, fetch_chunks.unwrap(), (chunk as f64 / fetch_chunks.unwrap() as f64) * 100.0);
                let hash = hash_bytes(&read_chunk(fetch_file.as_ref().unwrap(), chunk + 1));
                tx_sender.send(RouterMessage::FetchChunk {chunk: chunk + 1, hash}).unwrap();
            }
            RouterMessage::ChunkDownloaded {chunk, data} => {
                // the other side returned a valid chunk
                println!("ChunkDownloaded {} {}/{} {:.2}%", fetch_file.as_ref().unwrap(), chunk, fetch_chunks.unwrap(), (chunk as f64 / fetch_chunks.unwrap() as f64) * 100.0);
                let data_len = data.len();
                write_chunk(fetch_file.as_ref().unwrap(), chunk, data);
                let hash = hash_bytes(&read_chunk(fetch_file.as_ref().unwrap(), chunk + 1));
                if data_len == CHUNK_SIZE as usize {
                    tx_sender.send(RouterMessage::FetchChunk {chunk: chunk + 1, hash}).unwrap();
                }
                else {
                    println!("File done");
                    return;
                }
            }
        }
    }
    
}

// simple not so fast u64 hash function to use as chunk comparison function. i'm sure it has collision states but don't care atm.
fn hash_bytes(id_bytes: &[u8]) -> u64 {
    let mut x: u64 = 0xd6e8_feb8_6659_fd93;
    let mut i = 0;
    while i < id_bytes.len() {
        x = x.overflowing_add(id_bytes[i] as u64).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
        x ^= x >> 32;
        i += 1;
    }
    return x
}
