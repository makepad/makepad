// this webserver is serving our site. Why? WHYYY. Because it was fun to write. And MUCH faster and MUCH simpler than anything else imaginable.

use crate::utils::*;
pub use crate::web_socket_parser::{
    WebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser,
    SERVER_WEB_SOCKET_PING_MESSAGE, SERVER_WEB_SOCKET_PONG_MESSAGE,
};
use std::io::prelude::*;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, mpsc::RecvTimeoutError};
use std::time::Duration;

/// Sockets whose peer vanished (a client retry storm timing out and
/// abandoning connects, a NAT dropping the flow) used to pin their
/// per-connection thread FOREVER in a blocking read — thousands of leaked
/// threads eventually slow `thread::spawn` on the ACCEPT loop itself, the
/// listen backlog fills, and new SYNs get dropped: the service looks dead
/// from outside while established flows stay fine. Bound every socket wait.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a request thread waits for the app side to answer before 504.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
/// Concurrent connection threads (process-wide). Over the cap, new
/// connections are shed with an inline 503 — cheap and honest — instead of
/// growing the thread pile.
const MAX_CONNS: usize = 256;
static ACTIVE_CONNS: AtomicUsize = AtomicUsize::new(0);

struct ConnGuard;
impl ConnGuard {
    fn try_acquire() -> Option<ConnGuard> {
        let prev = ACTIVE_CONNS.fetch_add(1, Ordering::SeqCst);
        if prev >= MAX_CONNS {
            ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(ConnGuard)
    }
}
impl Drop for ConnGuard {
    fn drop(&mut self) {
        ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(feature = "script")]
use makepad_script::*;

#[derive(Clone)]
pub struct HttpServer {
    pub listen_address: SocketAddr,
    pub request: mpsc::Sender<HttpServerRequest>,
    pub post_max_size: u64,
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone)]
pub struct HttpServerResponse {
    #[cfg_attr(feature = "script", live)]
    pub header: String,
    #[cfg_attr(feature = "script", live)]
    pub body: Vec<u8>,
}

pub enum HttpServerRequest {
    ConnectWebSocket {
        web_socket_id: u64,
        headers: HttpServerHeaders,
        response_sender: mpsc::Sender<Vec<u8>>,
    },
    DisconnectWebSocket {
        web_socket_id: u64,
    },
    BinaryMessage {
        web_socket_id: u64,
        response_sender: mpsc::Sender<Vec<u8>>,
        data: Vec<u8>,
    },
    TextMessage {
        web_socket_id: u64,
        response_sender: mpsc::Sender<Vec<u8>>,
        string: String,
    },
    Get {
        headers: HttpServerHeaders,
        response_sender: mpsc::Sender<HttpServerResponse>,
    },
    Post {
        headers: HttpServerHeaders,
        body: Vec<u8>,
        response: mpsc::Sender<HttpServerResponse>,
    },
}

pub fn start_http_server(http_server: HttpServer) -> Option<std::thread::JoinHandle<()>> {
    let listener = if let Ok(listener) = TcpListener::bind(http_server.listen_address) {
        listener
    } else {
        println!("Cannot bind http server port");
        return None;
    };

    let listen_thread = {
        std::thread::spawn(move || {
            let mut connection_counter = 0u64;
            for tcp_stream in listener.incoming() {
                let mut tcp_stream = if let Ok(tcp_stream) = tcp_stream {
                    tcp_stream
                } else {
                    println!("Incoming stream failure");
                    continue;
                };
                let http_server = http_server.clone();
                connection_counter += 1;
                // Shed over the cap INLINE (no thread): the pile of leaked
                // connection threads is what used to stall this accept loop.
                let Some(guard) = ConnGuard::try_acquire() else {
                    let _ = tcp_stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n",
                    );
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    continue;
                };
                // Bounded socket waits, so an abandoned peer can never pin
                // this thread forever (websocket upgrades clear it again —
                // their idle reads are legitimate and ping-covered).
                let _ = tcp_stream.set_read_timeout(Some(SOCKET_TIMEOUT));
                let _ = tcp_stream.set_write_timeout(Some(SOCKET_TIMEOUT));
                let _read_thread = std::thread::spawn(move || {
                    let _guard = guard;
                    let head = HttpServerHeaders::from_tcp_stream(&mut tcp_stream);
                    if head.is_none() {
                        return http_error_out(tcp_stream, 500);
                    }
                    // Whatever arrived in the same segment as the headers is
                    // already off the socket and has to be handed onward.
                    let (headers, body_prefix) = head.unwrap();

                    if headers.sec_websocket_key.is_some() {
                        return handle_web_socket(
                            http_server,
                            tcp_stream,
                            headers,
                            body_prefix,
                            connection_counter,
                        );
                    }
                    if headers.verb == "POST" {
                        return handle_post(http_server, tcp_stream, headers, body_prefix);
                    }
                    if headers.verb == "GET" {
                        return handle_get(http_server, tcp_stream, headers);
                    }
                    http_error_out(tcp_stream, 500)
                });
            }
        })
    };
    Some(listen_thread)
}

fn handle_post(
    http_server: HttpServer,
    mut tcp_stream: TcpStream,
    headers: HttpServerHeaders,
    body_prefix: Vec<u8>,
) {
    // we have to have a content-length or bust
    if headers.content_length.is_none() {
        return http_error_out(tcp_stream, 500);
    }
    let content_length = headers.content_length.unwrap();
    if content_length > http_server.post_max_size {
        return http_error_out(tcp_stream, 500);
    }
    let bytes_total = content_length as usize;
    let mut body = Vec::new();
    body.resize(bytes_total, 0u8);

    // Body bytes that came in with the headers are already read; reading them
    // from the socket again would block until the peer or the timeout gives up.
    let prefix_len = body_prefix.len().min(bytes_total);
    body[..prefix_len].copy_from_slice(&body_prefix[..prefix_len]);

    let mut bytes_left = bytes_total - prefix_len;
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

    let (tx_socket, rx_socket) = mpsc::channel::<HttpServerResponse>();
    if http_server
        .request
        .send(HttpServerRequest::Post {
            headers,
            body,
            response: tx_socket,
        })
        .is_err()
    {
        return http_error_out(tcp_stream, 500);
    };

    match rx_socket.recv_timeout(RESPONSE_TIMEOUT) {
        Ok(response) => {
            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
        }
        Err(_) => return http_error_out(tcp_stream, 504),
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

fn handle_web_socket(
    http_server: HttpServer,
    mut tcp_stream: TcpStream,
    headers: HttpServerHeaders,
    body_prefix: Vec<u8>,
    web_socket_id: u64,
) {
    // Low-latency control traffic (e.g. studio Tick messages) benefits from
    // disabling Nagle on loopback websocket links.
    let _ = tcp_stream.set_nodelay(true);
    // A websocket idles legitimately (the write thread pings it); the
    // accept-time socket timeout would kill it, so clear it here.
    let _ = tcp_stream.set_read_timeout(None);
    let _ = tcp_stream.set_write_timeout(None);

    let upgrade_response =
        WebSocketParser::create_upgrade_response(headers.sec_websocket_key.as_ref().unwrap());

    write_bytes_to_tcp_stream_no_error(&mut tcp_stream, upgrade_response.as_bytes());

    let mut write_tcp_stream = tcp_stream.try_clone().unwrap();
    let _ = write_tcp_stream.set_nodelay(true);
    let (tx_socket, rx_socket) = mpsc::channel::<Vec<u8>>();

    let _write_thread = std::thread::spawn(move || {
        // xx
        loop {
            match rx_socket.recv_timeout(Duration::from_millis(2000)) {
                Ok(data) => {
                    if data.is_empty() {
                        break;
                    }
                    let header = WebSocketMessageHeader::from_len(
                        data.len(),
                        WebSocketMessageFormat::Binary,
                        false,
                    );
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, header.as_slice());
                    write_bytes_to_tcp_stream_no_error(&mut write_tcp_stream, &data);
                }
                Err(RecvTimeoutError::Timeout) => {
                    write_bytes_to_tcp_stream_no_error(
                        &mut write_tcp_stream,
                        &SERVER_WEB_SOCKET_PING_MESSAGE,
                    );
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = write_tcp_stream.shutdown(Shutdown::Both);
    });

    if http_server
        .request
        .send(HttpServerRequest::ConnectWebSocket {
            headers,
            web_socket_id,
            response_sender: tx_socket.clone(),
        })
        .is_err()
    {
        let _ = tcp_stream.shutdown(Shutdown::Both);
        return;
    };

    let mut web_socket = WebSocketParser::new();
    // A client may pipeline its first frames into the same segment as the
    // upgrade request; those bytes came off the socket with the headers and
    // must be parsed before we block waiting for more.
    let mut pending = body_prefix;
    loop {
        let mut data = [0u8; 65535];
        let pipelined;
        let read = if pending.is_empty() {
            tcp_stream.read(&mut data)
        } else {
            pipelined = std::mem::take(&mut pending);
            data[..pipelined.len()].copy_from_slice(&pipelined);
            Ok(pipelined.len())
        };
        match read {
            Ok(n) => {
                if n == 0 {
                    let _ = tcp_stream.shutdown(Shutdown::Both);
                    let _ = tx_socket.send(Vec::new());
                    break;
                }
                web_socket.parse(&data[0..n], |result| match result {
                    Ok(WebSocketMessage::Ping(_)) => {
                        let _ = tx_socket.send(SERVER_WEB_SOCKET_PONG_MESSAGE.to_vec());
                    }
                    Ok(WebSocketMessage::Pong(_)) => {}
                    Ok(WebSocketMessage::Text(text)) => {
                        if http_server
                            .request
                            .send(HttpServerRequest::TextMessage {
                                web_socket_id,
                                response_sender: tx_socket.clone(),
                                string: text.into(),
                            })
                            .is_err()
                        {
                            eprintln!("Websocket message deserialize error");
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            let _ = tx_socket.send(Vec::new());
                        };
                    }
                    Ok(WebSocketMessage::Binary(data)) => {
                        if http_server
                            .request
                            .send(HttpServerRequest::BinaryMessage {
                                web_socket_id,
                                response_sender: tx_socket.clone(),
                                data: data.to_vec(),
                            })
                            .is_err()
                        {
                            eprintln!("Websocket message deserialize error");
                            let _ = tcp_stream.shutdown(Shutdown::Both);
                            let _ = tx_socket.send(Vec::new());
                        };
                    }
                    Ok(WebSocketMessage::Close) => {
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                    }
                    Err(e) => {
                        eprintln!("Websocket error {:?}", e);
                        let _ = tcp_stream.shutdown(Shutdown::Both);
                        let _ = tx_socket.send(Vec::new());
                    }
                });
            }
            Err(_) => {
                println!("Websocket closed");
                let _ = tcp_stream.shutdown(Shutdown::Both);
                let _ = tx_socket.send(Vec::new());
                break;
            }
        }
    }

    let _ = http_server
        .request
        .send(HttpServerRequest::DisconnectWebSocket { web_socket_id });
}

fn handle_get(http_server: HttpServer, mut tcp_stream: TcpStream, headers: HttpServerHeaders) {
    // send our channel the post
    let (tx_socket, rx_socket) = mpsc::channel::<HttpServerResponse>();
    if http_server
        .request
        .send(HttpServerRequest::Get {
            headers,
            response_sender: tx_socket,
        })
        .is_err()
    {
        return http_error_out(tcp_stream, 500);
    };

    match rx_socket.recv_timeout(RESPONSE_TIMEOUT) {
        Ok(response) => {
            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, response.header.as_bytes());
            write_bytes_to_tcp_stream_no_error(&mut tcp_stream, &response.body);
        }
        Err(_) => return http_error_out(tcp_stream, 504),
    }
    let _ = tcp_stream.shutdown(Shutdown::Both);
}
