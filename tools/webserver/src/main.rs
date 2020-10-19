// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use std::net::{TcpListener, TcpStream, SocketAddr, Shutdown};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::io::prelude::*;
use std::io::BufReader;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use deflate::Compression;
use deflate::write::ZlibEncoder;
use std::time::{Duration, Instant};

mod digest;
mod websocket;
use crate::websocket::{WebSocket, WebSocketResult, WebSocketMessage};

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
                *fc = FileCache{cache:Some(Arc::new(new_zlib_filecache))};
            }
            
            if let Ok(mut fc) = brotli_filecache.wrap.lock() {
                *fc = FileCache{cache:None};
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
pub struct FileCacheWrap{
    wrap: Arc<Mutex<FileCache>>,
}

#[derive(Clone, Default)]
pub struct FileCache{
    cache: Option<Arc<HashMap<String, Vec<u8 >> >>
}

pub struct HttpServer {
    pub listen_thread: Option<std::thread::JoinHandle<() >>,
    pub listen_address: Option<SocketAddr>,
}


impl HttpServer {
    
    fn handle_post(tcp_stream: &mut TcpStream, url: &str, mut body: Vec<u8>) {
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
            _ => return error_out(tcp_stream, 500)
        }
    }
    
    
    fn handle_get(
        tcp_stream: &mut TcpStream,
        url: &str,
        accept_encoding: String,
        zlib_filecache: FileCache,
        brotli_filecache: FileCache,
    ) {
        let url = if let Some(url) = parse_url_file(url) {
            url
        }
        else {
            return error_out(tcp_stream, 500);
        };
        
        let mime_type = if url.ends_with(".html") {"text/html"}
        else if url.ends_with(".wasm") {"application/wasm"}
        else if url.ends_with(".js") {"text/javascript"}
        else {"application/octet-stream"};
        
        if accept_encoding.contains("br") { // we want the brotli
            if let Some(brotli_filecache) = brotli_filecache.cache {
                if let Some(data) = brotli_filecache.get(&url) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: br\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                }
                else {
                    return error_out(tcp_stream, 404);
                }
            }
        }
        
        if accept_encoding.contains("gzip") || accept_encoding.contains("deflate") {
            if let Some(zlib_filecache) = zlib_filecache.cache {
                if let Some(data) = zlib_filecache.get(&url) {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n\
                            Content-encoding: deflate\r\n\
                            Cache-Control: max-age:0\r\n\
                            Content-Length: {}\r\n\
                            Connection: close\r\n\r\n",
                        mime_type,
                        data.len()
                    );
                    write_bytes_to_tcp_stream_no_error(tcp_stream, header.as_bytes());
                    write_bytes_to_tcp_stream_no_error(tcp_stream, &data);
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    return
                }
                else {
                    return error_out(tcp_stream, 404);
                }
            }
        }
        return error_out(tcp_stream, 500);
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
                        
                        let mut reader = BufReader::new(tcp_stream.try_clone().expect("Cannot clone tcp stream"));
                        
                        // read the entire header
                        let mut header = Vec::new();
                        let mut content_length = None;
                        let mut accept_encoding = None;
                        let mut upgrade_to_websocket = None;
                        let mut line = String::new();
                        while let Ok(_) = reader.read_line(&mut line) { // TODO replace this with a non-line read
                            if line == "\r\n" { // the newline
                                break;
                            }
                            if let Some(v) = split_header_line(&line, "Content-Length: ") {
                                content_length = Some(if let Ok(v) = v.parse() {v} else {
                                    return error_out(&mut tcp_stream, 500)
                                });
                            }
                            if let Some(v) = split_header_line(&line, "Accept-Encoding: ") {
                                accept_encoding = Some(v.to_string());
                            }
                            if let Some(v) = split_header_line(&line, "sec-websocket-key: ") {
                                upgrade_to_websocket = Some(v.to_string());
                            }
                            if line.len() > 4096 || header.len() > 4096 { // some overflow protection
                                return error_out(&mut tcp_stream, 500);
                            }
                            header.push(line.clone());
                            line.truncate(0);
                        }
                        if let Some(key) = upgrade_to_websocket {
                            // lets hash the thing
                            if let Some(url) = split_header_line(&header[0], "GET ") {
                                return websocket_channels.handle_websocket(&mut tcp_stream, url, &key);
                            }
                            else {
                                return error_out(&mut tcp_stream, 500);
                            }
                        }
                        if header.len() < 2 {
                            return error_out(&mut tcp_stream, 500);
                        }
                        
                        if let Some(url) = split_header_line(&header[0], "POST ") {
                            // we have to have a content-length or bust
                            if content_length.is_none() {
                                return error_out(&mut tcp_stream, 500);
                            }
                            let content_length = content_length.unwrap();
                            //read the rest
                            let mut body = Vec::new();
                            loop {
                                let len = if let Ok(buf) = reader.fill_buf() {
                                    body.extend_from_slice(buf);
                                    buf.len()
                                }else {0};
                                if len == 0 {
                                    if body.len() < content_length {
                                        return error_out(&mut tcp_stream, 500);
                                    }
                                    break;
                                }
                                if body.len()>40960000 || body.len() as usize >= content_length {
                                    return error_out(&mut tcp_stream, 500);
                                }
                                reader.consume(len);
                            }
                            // ok we have the data. Jump to the post handler
                            return Self::handle_post(&mut tcp_stream, url, body);
                        }
                        
                        if let Some(url) = split_header_line(&header[0], "GET ") {
                            if let Some(accept_encoding) = accept_encoding {
                                return Self::handle_get(&mut tcp_stream, url, accept_encoding, zlib_filecache, brotli_filecache)
                            }
                        }
                        
                        return error_out(&mut tcp_stream, 500);
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

fn split_header_line<'a>(inp: &'a str, what: &str) -> Option<&'a str> {
    let mut what_lc = what.to_string();
    what_lc.make_ascii_lowercase();
    let mut inp_lc = inp.to_string();
    inp_lc.make_ascii_lowercase();
    if inp_lc.starts_with(&what_lc) {
        return Some(&inp[what.len()..(inp.len() - 2)])
    }
    None
}

fn parse_url_file(url: &str) -> Option<String> {
    
    // find the end_of_name skipping everything else
    let end_of_name = url.find(' ');
    if end_of_name.is_none() {
        return None;
    }
    let end_of_name = end_of_name.unwrap();
    let end_of_name = if let Some(q) = url.find('?') {
        end_of_name.min(q)
    }else {end_of_name};
    
    let mut url = url[0..end_of_name].to_string();
    
    if url.ends_with("/") {
        url.push_str("index.html");
    }
    
    Some(url)
}

fn error_out(tcp_stream: &mut TcpStream, code: usize) {
    write_bytes_to_tcp_stream_no_error(tcp_stream, format!("HTTP/1.1 {}\r\n\r\n", code).as_bytes());
    let _ = tcp_stream.shutdown(Shutdown::Both);
}



#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WebSocketId(pub u32);
pub struct WebSocketChannel {
    pub id_alloc: u32,
    pub tx_bus:  mpsc::Sender<(WebSocketId,Vec<u8>)>,
    pub bus_thread: std::thread::JoinHandle<()>,
    pub out_sockets: HashMap<WebSocketId, mpsc::Sender<Vec<u8>>>,
}

#[derive(Clone, Default)]
pub struct WebSocketChannels{
    channels: Arc<Mutex<HashMap<String,WebSocketChannel>>>
}

impl WebSocketChannels{
    
    fn handle_websocket(
        self,
        tcp_stream: &mut TcpStream,
        url: &str,
        key: &str,
    ) {
        let upgrade_response = WebSocket::create_upgrade_response(key);

        write_bytes_to_tcp_stream_no_error(tcp_stream, upgrade_response.as_bytes());
        
        // start a thread for the write side
        let mut write_tcp_stream = tcp_stream.try_clone().unwrap();

        let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8>>();
        
        let (socket_id, tx_bus) = self.add_socket_tx(url, tx_socket);
        
        // ok we get a socket id, we'll add that to our websocket datavec as a header.
        let mut web_socket = WebSocket::new();
        
        // start a write thread for the return messages
        let _write_thread = std::thread::spawn(move || {
            // we have a bus we read from, which we hand to our websocket server.
            while let Ok(data) = rx_socket.recv(){
                println!("SENDING DATA {:?}", data);
                write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
            }
            let _ = write_tcp_stream.shutdown(Shutdown::Both);
        });
        
        // do the websocket read here
        loop {
            let mut data = [0u8; 1024];
            match tcp_stream.read(&mut data) {
                Ok(n) => {
                    for result in web_socket.parse(&data[0..n]){
                        match result{
                            WebSocketResult::Ping(_)=>{},
                            WebSocketResult::Pong(_)=>{},
                            WebSocketResult::Data(data)=>{
                                // we have to send this to the websocket server
                                println!("GOT DATA #{:?}#", data);
                                tx_bus.send((socket_id.clone(),data)).unwrap();
                                //let s = std::str::from_utf8(&data);
                            },
                            WebSocketResult::Error(_)=>{},
                            WebSocketResult::Close=>{
                            }
                        }
                    }
                }
                Err(_) => {
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    self.remove_socket(url, socket_id);
                    return
                }
            }
        }
    }
    
    
    pub fn add_channel_if_none(&self, in_url:&str){
        // lets start a thread
        if let Ok(mut channels) = self.channels.lock(){
            if channels.get(in_url).is_some(){
                return
            }
            let (tx_bus, rx_bus) = mpsc::channel::<(WebSocketId,Vec<u8>)>();
            let arc_channels = self.channels.clone();
            let url = in_url.to_string();
            let thread = std::thread::spawn(move || {
                // we should collect all the messages we can in 50ms and then send it out together in 1 message
                let max_wait = Duration::from_millis(50);
                let mut last_start = Instant::now();
                let mut msg_stack = Vec::new();
                loop{
                    // do the message pump, but combine messages to not overflow the sockets
                    if let Ok((websocket_id,msg)) = rx_bus.recv_timeout(max_wait) {
                        msg_stack.push((websocket_id,msg));
                    }
                    if last_start.elapsed() > max_wait{ 
                        if msg_stack.len() > 0{ // send it out all together
                            // lets push out a little header: numclients, nummsgs
                            
                            let mut all_msg_len = 0;
                            for (_socket_id, msg) in &mut msg_stack{
                                all_msg_len += msg.len() + std::mem::size_of::<u32>();
                            }

                            let mut out_sockets = HashMap::new();
                            
                            // quickly lock sockets
                            if let Ok(channels) = arc_channels.lock(){
                                if let Some(channel) = channels.get(&url){
                                    out_sockets = channel.out_sockets.clone()
                                }
                            }
                            for (my_socket_id, tx_socket) in &out_sockets{
                                // lets build the message header
                                let mut head = Vec::new();
                                // number of sockets
                                head.extend_from_slice(&(out_sockets.len() as u64).to_le_bytes());
                                // my own socket
                                head.extend_from_slice(&(my_socket_id.0 as u32).to_le_bytes());
                                // all other sockets
                                for (socket_id, _) in &out_sockets{
                                    if socket_id != my_socket_id{
                                        head.extend_from_slice(&(socket_id.0 as u32).to_le_bytes());
                                    }
                                }
                                // the number of messages
                                head.extend_from_slice(&(msg_stack.len() as u64).to_le_bytes());
                                
                                let mut ws_msg  = WebSocketMessage::new_binary(head.len() + all_msg_len);
                                ws_msg.append(&head);
                                
                                for (websocket_id,msg) in &mut msg_stack{
                                    ws_msg.append(&(websocket_id.0 as u32).to_le_bytes());
                                    ws_msg.append(msg);
                                }
                                
                                // send it off blindly
                                let _ = tx_socket.send(ws_msg.take());
                            }
                            
                            msg_stack.truncate(0);
                        }
                        last_start = Instant::now()
                    }
                }
            });
            channels.insert(in_url.to_string(), WebSocketChannel{
                id_alloc: 0,
                out_sockets:HashMap::new(),
                tx_bus: tx_bus,
                bus_thread: thread
            });
        }
    }
    
    pub fn add_socket_tx(&self, url:&str, tx:mpsc::Sender::<Vec<u8>>)->(WebSocketId, mpsc::Sender<(WebSocketId,Vec<u8>)>){
        self.add_channel_if_none(url);
        
        if let Ok(mut channels) = self.channels.lock(){
            if let Some(channel) = channels.get_mut(url){
                let socket_id = WebSocketId(channel.id_alloc);
                channel.id_alloc += 1;
                channel.out_sockets.insert(socket_id.clone(), tx);
                return (socket_id, channel.tx_bus.clone())
            }
            else{
                panic!()
            }
        }
        else{
            panic!()
        }
    }
    
    pub fn remove_socket(&self, url:&str, socket_id:WebSocketId){
        if let Ok(mut servers) = self.channels.lock(){
            if let Some(server) = servers.get_mut(url){
                server.out_sockets.remove(&socket_id);
            }
        }
    }
}

