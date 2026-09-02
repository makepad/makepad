use std::io::prelude::*;
use std::net::{Shutdown, SocketAddr, TcpStream};

#[cfg(feature = "script")]
use makepad_script::*;
#[cfg(feature = "script")]
use std::net::{IpAddr, Ipv4Addr};

pub fn write_bytes_to_tcp_stream_no_error(tcp_stream: &mut TcpStream, bytes: &[u8]) -> bool {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_written) = tcp_stream.write(buf) {
            if bytes_written == 0 {
                return true;
            }
            bytes_left -= bytes_written;
        } else {
            return true;
        }
    }
    false
}

pub fn http_error_out(mut tcp_stream: TcpStream, code: usize) {
    write_bytes_to_tcp_stream_no_error(
        &mut tcp_stream,
        format!("HTTP/1.1 {}\r\n\r\n", code).as_bytes(),
    );
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

pub fn split_header_line<'a>(inp: &'a str, what: &str) -> Option<&'a str> {
    let mut what_lc = what.to_string();
    what_lc.make_ascii_lowercase();
    let mut inp_lc = inp.to_string();
    inp_lc.make_ascii_lowercase();
    if inp_lc.starts_with(&what_lc) {
        return Some(&inp[what.len()..(inp.len() - 2)]);
    }
    None
}

pub fn parse_url_path(url: &str, append_index_html: bool) -> Option<(String, Option<String>)> {
    // find the end_of_name skipping everything else
    let end_of_name = url.find(' ');
    end_of_name?;
    let end_of_name = end_of_name.unwrap();
    let mut search = None;
    let end_of_name = if let Some(q) = url.find('?') {
        search = Some(url[q + 1..end_of_name].to_string());
        q
    } else {
        end_of_name
    };

    let mut url = url[0..end_of_name].to_string();

    if append_index_html && url.ends_with('/') {
        url.push_str("index.html");
    }

    Some((url, search))
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone)]
pub struct HttpServerHeaders {
    #[cfg_attr(
        feature = "script",
        rust(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080))
    )]
    pub addr: SocketAddr,
    #[cfg_attr(feature = "script", live)]
    pub addr_text: String,
    #[cfg_attr(feature = "script", live)]
    pub lines: Vec<String>,
    #[cfg_attr(feature = "script", live)]
    pub verb: String,
    #[cfg_attr(feature = "script", live)]
    pub path: String,
    #[cfg_attr(feature = "script", live)]
    pub path_no_slash: String,
    #[cfg_attr(feature = "script", live)]
    pub search: Option<String>,
    #[cfg_attr(feature = "script", live)]
    pub content_length: Option<u64>,
    #[cfg_attr(feature = "script", live)]
    pub accept_encoding: Option<String>,
    #[cfg_attr(feature = "script", live)]
    pub sec_websocket_key: Option<String>,
}

/// Largest request head we will buffer before giving up.
const MAX_HEAD_BYTES: usize = 1024 * 1024;

/// Index just past the `\r\n\r\n` that ends the request head, if present.
fn find_head_end(buf: &[u8], search_from: usize) -> Option<usize> {
    let start = search_from.saturating_sub(3);
    buf[start..]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| start + p + 4)
}

impl HttpServerHeaders {
    /// Returns the first request-header value with an ASCII case-insensitive
    /// name. `lines` remains public for callers that need the untouched head.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.lines.iter().skip(1).find_map(|line| {
            let line = line.strip_suffix("\r\n").unwrap_or(line);
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    /// Reads the request head, returning the parsed headers together with every
    /// byte that was read past the head terminator.
    ///
    /// Those trailing bytes are the start of the body whenever a client sends
    /// headers and body in one TCP segment. Whatever finds the end of the head
    /// must read ahead to do it, and the socket will not hand those bytes over
    /// a second time — so they travel with the headers and the caller consumes
    /// them before reading further.
    pub fn from_tcp_stream(tcp_stream: &mut TcpStream) -> Option<(HttpServerHeaders, Vec<u8>)> {
        let addr = tcp_stream.peer_addr().ok()?;

        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // Bytes already scanned for the terminator; each pass re-checks only
        // the new bytes plus the three that could hold a straddling \r\n\r\n.
        let mut searched = 0;
        let head_end = loop {
            if let Some(end) = find_head_end(&buf, searched) {
                break end;
            }
            searched = buf.len();
            if buf.len() > MAX_HEAD_BYTES {
                return None;
            }
            match tcp_stream.read(&mut chunk) {
                // EOF before the head ended: not a request we can serve.
                Ok(0) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return None,
            }
        };

        // The head minus its blank terminator line, so each entry still keeps
        // the trailing \r\n that split_header_line trims.
        let head = String::from_utf8_lossy(&buf[..head_end - 2]).into_owned();
        let body_prefix = buf[head_end..].to_vec();

        let mut lines = Vec::new();
        let mut content_length = None;
        let mut accept_encoding = None;
        let mut sec_websocket_key = None;

        for raw in head.split_inclusive("\r\n") {
            let line = raw.to_string();
            if let Some(v) = split_header_line(&line, "Content-Length: ") {
                content_length = Some(if let Ok(v) = v.parse() {
                    v
                } else {
                    return None;
                });
            }
            if let Some(v) = split_header_line(&line, "Accept-Encoding: ") {
                accept_encoding = Some(v.to_string());
            }
            if let Some(v) = split_header_line(&line, "sec-websocket-key: ") {
                sec_websocket_key = Some(v.to_string());
            }
            if line.len() > 4096 || lines.len() > 4096 {
                // some overflow protection
                return None;
            }
            lines.push(line);
        }
        if lines.len() < 2 {
            return None;
        }
        let verb;
        let path;
        if let Some(v) = split_header_line(&lines[0], "GET ") {
            verb = "GET";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else if let Some(v) = split_header_line(&lines[0], "HEAD ") {
            verb = "HEAD";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else if let Some(v) = split_header_line(&lines[0], "OPTIONS ") {
            verb = "OPTIONS";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else if let Some(v) = split_header_line(&lines[0], "POST ") {
            verb = "POST";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else if let Some(v) = split_header_line(&lines[0], "PUT ") {
            verb = "PUT";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else if let Some(v) = split_header_line(&lines[0], "DELETE ") {
            verb = "DELETE";
            path = parse_url_path(v, sec_websocket_key.is_none())
        } else {
            return None;
        }
        path.as_ref()?;
        let path = path.unwrap();

        Some((
            HttpServerHeaders {
                addr,
                addr_text: format!("{}", addr),
                verb: verb.to_string(),
                path_no_slash: path.0[1..].to_string(),
                path: path.0,
                search: path.1,
                lines,
                content_length,
                accept_encoding,
                sec_websocket_key,
            },
            body_prefix,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_url_path, HttpServerHeaders};
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    /// Serves one connection, returning what `from_tcp_stream` produced.
    fn read_head(write_request: impl FnOnce(&mut TcpStream) + Send + 'static) -> (Vec<String>, Vec<u8>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            write_request(&mut stream);
            // Hold the connection open: a server that has to go back to the
            // socket for the body would otherwise see EOF and pass by luck.
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
        let (mut server, _) = listener.accept().unwrap();
        let (headers, body_prefix) = HttpServerHeaders::from_tcp_stream(&mut server).unwrap();
        client.join().unwrap();
        (headers.lines, body_prefix)
    }

    #[test]
    fn body_sent_with_the_headers_is_returned_not_swallowed() {
        // The case that used to wedge the server thread forever: one write, so
        // the body lands in the same segment as the headers.
        let (lines, body) = read_head(|stream| {
            stream
                .write_all(b"POST /pair HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello")
                .unwrap();
        });
        assert_eq!(lines.len(), 3);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn body_sent_after_the_headers_is_left_on_the_socket() {
        let (lines, body) = read_head(|stream| {
            stream
                .write_all(b"POST /pair HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\n")
                .unwrap();
            stream.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            stream.write_all(b"hello").unwrap();
        });
        assert_eq!(lines.len(), 3);
        assert!(body.is_empty());
    }

    #[test]
    fn get_without_a_body_parses_the_request_line() {
        let (lines, body) = read_head(|stream| {
            stream
                .write_all(b"GET /ui/ HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
        });
        assert_eq!(lines[0], "GET /ui/ HTTP/1.1\r\n");
        assert!(body.is_empty());
    }

    #[test]
    fn head_preserves_verb_and_raw_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream
                .write_all(b"HEAD /asset.wasm HTTP/1.1\r\nHost: x\r\nRange: bytes=4-7\r\n\r\n")
                .unwrap();
        });
        let (mut server, _) = listener.accept().unwrap();
        let (headers, body) = HttpServerHeaders::from_tcp_stream(&mut server).unwrap();
        client.join().unwrap();
        assert_eq!(headers.verb, "HEAD");
        assert_eq!(headers.path, "/asset.wasm");
        assert_eq!(headers.header("range"), Some("bytes=4-7"));
        assert!(headers.lines.iter().any(|line| line == "Range: bytes=4-7\r\n"));
        assert!(body.is_empty());
    }

    #[test]
    fn appends_index_html_for_plain_http_directory_paths() {
        assert_eq!(
            parse_url_path("/ui/ HTTP/1.1\r\n", true),
            Some(("/ui/index.html".to_string(), None))
        );
    }

    #[test]
    fn preserves_trailing_slash_for_websocket_paths() {
        assert_eq!(
            parse_url_path("/ui/ HTTP/1.1\r\n", false),
            Some(("/ui/".to_string(), None))
        );
    }
}
