//! A stand-in for an app's `--remote` surface, used by the crate's own tests:
//! a loopback HTTP/1.1 server that records every request target and answers
//! from a route table.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Routes = HashMap<String, (u16, String)>;

pub struct FixtureServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    routes: Arc<Mutex<Routes>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FixtureServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fixture");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let port = listener.local_addr().expect("fixture addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let requests = requests.clone();
            let routes = routes.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => serve(stream, &requests, &routes),
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Self {
            port,
            requests,
            routes,
            stop,
            thread: Some(thread),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Answer `path` (query ignored) with `status` and a JSON/text body.
    pub fn route(&self, path: &str, status: u16, body: &str) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_string(), (status, body.to_string()));
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Close the request connection without a response, as an exiting app
    /// may do after accepting its final shutdown command.
    pub fn disconnect(&self, path: &str) {
        self.route(path, 0, "");
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(mut stream: TcpStream, requests: &Mutex<Vec<String>>, routes: &Mutex<Routes>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .to_string();
    requests.lock().unwrap().push(target.clone());
    let path = target.split('?').next().unwrap_or("/").to_string();
    let (status, body) = routes
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or((404, "{\"err\":\"no route\"}".to_string()));
    let reason = match status {
        0 => return,
        200 => "OK",
        404 => "Not Found",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
