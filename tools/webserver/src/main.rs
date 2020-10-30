// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use deflate::Compression;
use deflate::write::ZlibEncoder;

use std::io::prelude::*;

use makepad_http::channel::WebSocketChannels;
use makepad_http::httputil::*;

fn main() {
    // config params
    let base_path = "";
    let calc_path = "./";
    let exts = [".rs", ".js", ".toml", ".html", ".js", ".wasm", ".ttf", ".ron"];
    let ex_file = ["bindings.rs"];
    let ex_dirs = ["deps", "build", "edit_repo"];
    
    let brotli_filecache = FileCacheWrap::default();
    let zlib_filecache = FileCacheWrap::default();
    let websocket_channels = WebSocketChannels::default();
    
    let http_server = HttpServer::start_http_server(
        SocketAddr::from(([0, 0, 0, 0], 80)),
        brotli_filecache.clone(),
        zlib_filecache.clone(),
        websocket_channels,
    ).expect("Can't start server");
    
    // the file reload loop
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut iter = stdin.lock().lines();
        loop {
            let mut new_zlib_filecache = HashMap::new();
            //println!("Starting zlib compression");
            let mut total_size = 0;
            HttpServer::compress_tree_recursive(base_path, calc_path, &exts, &ex_file, &ex_dirs, &mut new_zlib_filecache, &mut total_size, false);
            //println!("Done with zlib compression {} files {} bytes", new_zlib_filecache.len(), total_size);
            if let Ok(mut fc) = zlib_filecache.wrap.lock() {
                *fc = FileCache {cache: Some(Arc::new(new_zlib_filecache))};
            }
            
            if let Ok(mut fc) = brotli_filecache.wrap.lock() {
                *fc = FileCache {cache: None};
            }
            /*
            let mut new_brotli_filecache = HashMap::new();
            println!("Starting brotli compression");
            let mut total_size = 0;
            HttpServer::compress_tree_recursive(base_path, calc_path, &exts, &ex_file, &ex_dirs, &mut new_brotli_filecache, &mut total_size, true);
            println!("Done with brotli compression {} files {} bytes", new_brotli_filecache.len(), total_size);
            if let Ok(mut fc) = brotli_filecache.wrap.lock() {
                *fc = FileCache{cache:Some(Arc::new(new_brotli_filecache))};
            }*/
            println!("Press return to reload");
            //return
            iter.next();
            //std::thread::sleep(std::time::Duration::from_millis(1500));
        }
    });
    
    http_server.listen_thread.unwrap().join().expect("can't join thread");
}

#[derive(Clone, Default)]
pub struct FileCacheWrap {
    wrap: Arc<Mutex<FileCache >>,
}

#[derive(Clone, Default)]
pub struct FileCache {
    cache: Option<Arc<HashMap<String, Vec<u8 >> >>
}

pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<() >>,
    pub listen_address: Option<SocketAddr>,
}


impl HttpServer {
    
    fn handle_post(tcp_stream: TcpStream, url: &str, mut body: Vec<u8>) {
        match url {
            "/subscribe" => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(true)
                    .open("subscribe.db")
                    .unwrap();
                
                body.push('\n' as u8);
                if let Err(_) = file.write(&body) {
                    println!("Couldn't append email to file");
                }
            },
            _ => return http_error_out(tcp_stream, 500)
        }
    }
    
    
    fn handle_get(
        mut tcp_stream: TcpStream,
        path: &str,
        accept_encoding: String,
        zlib_filecache: FileCache,
        brotli_filecache: FileCache,
    ) {
        
        let mime_type = if path.ends_with(".html") {"text/html"}
        else if path.ends_with(".wasm") {"application/wasm"}
        else if path.ends_with(".js") {"text/javascript"}
        else if path.ends_with(".ttf") {"application/ttf"}
        else {"application/octet-stream"};
        
        if accept_encoding.contains("br") { // we want the brotli
            if let Some(brotli_filecache) = brotli_filecache.cache {
                if let Some(data) = brotli_filecache.get(path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: br\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                }
                else {
                    return http_error_out(tcp_stream, 404);
                }
            }
        }
        
        if accept_encoding.contains("gzip") || accept_encoding.contains("deflate") {
            if let Some(zlib_filecache) = zlib_filecache.cache {
                if let Some(data) = zlib_filecache.get(path) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: deflate\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    return
                }
                else {
                    return http_error_out(tcp_stream, 404);
                }
            }
        }
        return http_error_out(tcp_stream, 500);
    }
    
    fn compress_tree_recursive(
        base_path: &str,
        calc_path: &str,
        ext_inc: &[&str],
        file_ex: &[&str],
        dir_ex: &[&str],
        filecache: &mut HashMap<String, Vec<u8 >>,
        total_size: &mut usize,
        use_brotli: bool
    ) {
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
                    Self::compress_tree_recursive(&format!("{}/{}", base_path, name), &format!("{}/{}", calc_path, name), ext_inc, file_ex, dir_ex, filecache, total_size, use_brotli);
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
                        let mut result = Vec::new();
                        if use_brotli {
                            let mut writer = brotli::CompressorWriter::new(&mut result, 4096 /* buffer size */, 11, 22);
                            writer.write_all(&data).expect("Can't write data");
                        }
                        else {
                            // brotli no work on http, i guess we'll do gzip for now
                            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::Default);
                            encoder.write_all(&data).expect("Write error!");
                            result = encoder.finish().expect("Failed to finish compression!");
                        }
                        
                        *total_size += result.len();
                        //println!("Compressed {} {}->{}", key_path, data.len(), result.len());
                        filecache.insert(key_path, result);
                    }
                }
            }
        }
    }
    
    
    pub fn start_http_server(
        listen_address: SocketAddr,
        brotli_filecache: FileCacheWrap,
        zlib_filecache: FileCacheWrap,
        websocket_channels: WebSocketChannels,
    ) -> Option<HttpServer> {
        
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
                    
                    let zlib_filecache = if let Ok(v) = zlib_filecache.wrap.lock() {
                        v.clone()
                    }
                    else {
                        FileCache::default()
                    };
                    
                    let brotli_filecache = if let Ok(v) = brotli_filecache.wrap.lock() {
                        v.clone()
                    }
                    else {
                        FileCache::default()
                    };
                    
                    let websocket_channels = websocket_channels.clone();
                    let _read_thread = std::thread::spawn(move || {
                        
                        let header = HttpHeader::from_tcp_stream(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                        
                        if header.is_none() {
                            return http_error_out(tcp_stream, 500);
                        }
                        let header = header.unwrap();
                        
                        if let Some(key) = header.sec_websocket_key {
                            // lets hash the thing
                            return websocket_channels.handle_websocket(&mut tcp_stream, &header.path, &key);
                        }
                        
                        if header.verb == "POST" {
                            // we have to have a content-length or bust
                            if header.content_length.is_none() {
                                return http_error_out(tcp_stream, 500);
                            }
                            let content_length = header.content_length.unwrap();
                            
                            let bytes_total = content_length as usize;
                            let mut body = Vec::new();
                            body.resize(bytes_total, 0u8);
                            // lets read content_length
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
                            
                            return Self::handle_post(tcp_stream, &header.path, body);
                        }
                        
                        if header.verb == "GET" {
                            if let Some(accept_encoding) = header.accept_encoding {
                                return Self::handle_get(
                                    tcp_stream,
                                    &header.path,
                                    accept_encoding,
                                    zlib_filecache,
                                    brotli_filecache
                                )
                            }
                        }
                        
                        return http_error_out(tcp_stream, 500);
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



