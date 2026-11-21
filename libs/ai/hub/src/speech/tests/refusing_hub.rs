use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

/// A loopback hub that refuses every generation without creating a job.
pub(super) struct RefusingHub {
    pub(super) base: String,
    refusals: Receiver<()>,
    _lifetime: Sender<()>,
}

impl RefusingHub {
    pub(super) fn start(get: fn(&str) -> &'static str) -> Self {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (sent, refusals) = channel();
        let (lifetime, stop) = channel();
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if answer(stream, get) && sent.send(()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if stop.recv_timeout(Duration::from_millis(1)) != Err(RecvTimeoutError::Timeout) {
                        break;
                    }
                }
                Err(e) => panic!("accept: {e}"),
            }
        });
        Self { base, refusals, _lifetime: lifetime }
    }

    /// Seeing the second POST proves the client entered admission backoff.
    pub(super) fn wait_for_retry(&self) {
        for _ in 0..2 {
            self.refusals.recv_timeout(Duration::from_secs(2)).expect("a refused submission");
        }
    }
}

fn answer(mut stream: TcpStream, get: fn(&str) -> &'static str) -> bool {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut reader = BufReader::new(&mut stream);
    let mut request = String::new();
    reader.read_line(&mut request).unwrap();
    let mut length = 0;
    loop {
        let mut header = String::new();
        assert_ne!(reader.read_line(&mut header).unwrap(), 0, "incomplete request");
        if header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            }
        }
    }
    reader.read_exact(&mut vec![0; length]).unwrap();
    let mut words = request.split_whitespace();
    let method = words.next().unwrap();
    let path = words.next().unwrap();
    let (status, body) = match (method, path) {
        ("POST", "/generate") => ("409 Conflict", r#"{"job_id":null,"ws_path":null,"error":"busy: a job is already queued or running"}"#),
        ("GET", _) => ("200 OK", get(path)),
        _ => panic!("unexpected request: {request}"),
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    method == "POST"
}
