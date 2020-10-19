use std::net::{TcpStream, Shutdown};
use std::io::Write;
use std::io::BufReader;
use std::io::prelude::*;

pub fn write_bytes_to_tcp_stream_no_error(tcp_stream: &mut TcpStream, bytes: &[u8]) {
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

pub fn http_error_out(tcp_stream: &mut TcpStream, code: usize) {
    write_bytes_to_tcp_stream_no_error(tcp_stream, format!("HTTP/1.1 {}\r\n\r\n", code).as_bytes());
    let _ = tcp_stream.shutdown(Shutdown::Both);
}


pub fn split_header_line<'a>(inp: &'a str, what: &str) -> Option<&'a str> {
    let mut what_lc = what.to_string();
    what_lc.make_ascii_lowercase();
    let mut inp_lc = inp.to_string();
    inp_lc.make_ascii_lowercase();
    if inp_lc.starts_with(&what_lc) {
        return Some(&inp[what.len()..(inp.len() - 2)])
    }
    None
}

pub fn parse_url_file(url: &str) -> Option<String> {
    
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

pub struct HttpHeader {
    pub lines: Vec<String>,
    pub content_length: Option<u64>,
    pub accept_encoding: Option<String>,
    pub sec_websocket_key: Option<String>
}

impl HttpHeader {
    pub fn from_tcp_stream(tcp_stream: TcpStream) -> Option<HttpHeader> {
      let mut reader = BufReader::new(tcp_stream);
                      
        let mut lines = Vec::new();
        let mut content_length = None;
        let mut accept_encoding = None;
        let mut sec_websocket_key = None;
        let mut line = String::new();
        
        while let Ok(_) = reader.read_line(&mut line) { // TODO replace this with a non-line read
            if line == "\r\n" { // the newline
                break;
            }
            if let Some(v) = split_header_line(&line, "Content-Length: ") {
                content_length = Some(if let Ok(v) = v.parse() {v} else {
                    return None
                });
            }
            if let Some(v) = split_header_line(&line, "Accept-Encoding: ") {
                accept_encoding = Some(v.to_string());
            }
            if let Some(v) = split_header_line(&line, "sec-websocket-key: ") {
                sec_websocket_key = Some(v.to_string());
            }
            if line.len() > 4096 || lines.len() > 4096 { // some overflow protection
                return None
            }
            lines.push(line.clone());
            line.truncate(0);
        }
        
        return Some(HttpHeader {
            lines,
            content_length,
            accept_encoding,
            sec_websocket_key
        });
    }
}
