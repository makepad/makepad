// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::collections::HashMap;
use std::sync::{Arc};
use std::io::prelude::*;
use std::io::BufReader;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use deflate::Compression;
use deflate::write::ZlibEncoder;

fn main() {
    // read the entire file tree, and brotli it.
    let mut filecache = HashMap::new();
    
    fn compress_tree_recursive(base_path: &str, calc_path: &str, ext_inc: &[&str], file_ex: &[&str], dir_ex: &[&str], filecache: &mut HashMap<String, Vec<u8>>) {
        if let Ok(read_dir) = fs::read_dir(calc_path) {
            for entry in read_dir {
                if entry.is_err() {continue};
                let entry = entry.unwrap();
                
                let ty = entry.file_type();
                if ty.is_err() {continue};
                let ty = ty.unwrap();
                
                let name = entry.file_name().into_string();
                if name.is_err() {continue};
                let name = name.unwrap();
                
                if ty.is_dir() {
                    if dir_ex.iter().find( | dir | **dir == name).is_some() {
                        continue
                    }
                    compress_tree_recursive(&format!("{}/{}", base_path, name), &format!("{}/{}", calc_path, name), ext_inc, file_ex, dir_ex, filecache);
                }
                else {
                    if file_ex.iter().find( | file | **file == name).is_some() {
                        continue
                    }
                    if ext_inc.iter().find( | ext | name.ends_with(*ext)).is_some() {
                        let key_path = format!("{}/{}", base_path, name);
                        let read_path = format!("{}/{}", calc_path, name);
                        let data = fs::read(read_path).expect("Can't read file");
                        // lets brotli it
                        
                        /*
                        let mut result = Vec::new();
                        {
                            let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, 11, 22);
                            writer.write_all(&data).expect("Can't write data");
                        }
                        */
                        // brotli no work on http, i guess we'll do gzip for now
                        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::Default);
                        encoder.write_all(&data).expect("Write error!");
                        let result = encoder.finish().expect("Failed to finish compression!");
                        
                        println!("Compressed {} {}->{}", key_path, data.len(), result.len());
                        filecache.insert(key_path, result);
                    }
                }
            }
        }
    }
    
    compress_tree_recursive(
        "",
        "./",
        &[".rs", ".js", ".toml", ".html", ".js", ".wasm", ".ttf", ".ron"],
        &["bindings.rs"],
        &["deps", "build", "edit_repo"],
        &mut filecache
    ); 
    
    let http_server = HttpServer::start_http_server(
        SocketAddr::from(([0, 0, 0, 0], 80)),
        Arc::new(filecache),
    ).expect("Can't start server");
    http_server.listen_thread.unwrap().join().expect("can't join thread");
}

pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<()>>,
    pub listen_address: Option<SocketAddr>,
}
 
impl HttpServer {
    pub fn start_http_server(listen_address: SocketAddr, filecache: Arc<HashMap<String, Vec<u8>>>) -> Option<HttpServer> {
        
        let listener = if let Ok(listener) = TcpListener::bind(listen_address.clone()) {listener} else {println!("Cannot bind http server port"); return None};
        
        let listen_thread = {
            std::thread::spawn(move || {
                for tcp_stream in listener.incoming() {
                    
                    let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                        tcp_stream
                    }
                    else {
                        println!("Incoming stream failure");
                        continue
                    };
                    let filecache = filecache.clone();
                    let _read_thread = std::thread::spawn(move || {
                        let mut reader = BufReader::new(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                        
                        let mut line = String::new();
                        reader.read_line(&mut line).expect("http read line fail");
                        
                        if line.starts_with("POST /subscribe"){ // writing email address
                            let mut content_size:u32 = 0;
                            while let Ok(_) = reader.read_line(&mut line){
                                if line == "\r\n"{ // the newline
                                    break;
                                }
                                let lc = line.to_ascii_lowercase();
                                if lc.starts_with("content-length: "){
                                    content_size = lc[16..(lc.len()-2)].parse().expect("can't parse content size");
                                }
                                line.truncate(0);
                            }
                            //read the rest
                            let mut data = Vec::new();
                            loop {
                                let len = if let Ok(buf) = reader.fill_buf(){
                                    data.extend_from_slice(buf);
                                    buf.len()
                                }else{0};
                                if len == 0{
                                    break
                                }
                                if data.len()>255 || data.len() as u32 >= content_size{
                                    break
                                }
                                reader.consume(len);
                            }
                            if data.len()>0 && data.len()<255{
                                let mut file = OpenOptions::new()
                                    .create(true)
                                    .write(true)
                                    .append(true)
                                    .open("subscribe.db")
                                    .unwrap();
                                    
                                data.push('\n' as u8);
                                if let Err(_) = file.write(&data){
                                    println!("Couldn't append email to file");
                                }
                            }
                            // lets append this to our file 
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, "HTTP/1.1 200 OK\r\n\r\n".as_bytes());
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            return 
                        }

                        if !line.starts_with("GET /") || line.len() < 10 {
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            return
                        }
                        
                        let line = &line[4..];
                        
                        let space = line.find(' ').expect("http space fail");
                        let space = if let Some(q) = line.find('?'){
                            space.min(q)
                        }else{space};
                        
                        let mut url = line[0..space].to_string();
                        if url.ends_with("/") {
                            url.push_str("index.html");
                        }

                        if let Some(data) = filecache.get(&url){
                            let mime_type = if url.ends_with(".html") {"text/html"}
                                else if url.ends_with(".wasm") {"application/wasm"}
                                else if url.ends_with(".js") {"text/javascript"}
                                else {"application/octet-stream"};
                            
                            // write the header
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-encoding: deflate\r\nCache-Control: max-age:0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                mime_type,
                                data.len()
                            );
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                        else{
                            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, "HTTP/1.1 404 NotFound\r\n\r\n".as_bytes());
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                        }
                    });
                }
            })
        };
        Some(HttpServer {
            listen_thread: Some(listen_thread),
            listen_address: Some(listen_address.clone()),
        })
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
