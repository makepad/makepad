//! A stand-in for an app's `--remote` endpoint, used by the crate's own tests:
//! a real makepad-remote endpoint on its own Unix socket (one per fixture, so
//! tests run in parallel) that records every request and answers from a
//! command table. It serves `cmd`, and `frames` with one fake PNG per frame.

use makepad_remote::conn::{Conn, Event, CLOSE_NORMAL};
use makepad_remote::endpoint::Factory;
use makepad_remote::frame::FrameReader;
use makepad_remote::{json_str, Endpoint, Req};
use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// What a command answers: its lines, or no answer at all (the connection to
/// the command is closed, as an exiting app may do).
#[derive(Clone)]
enum Answer {
    Lines(String),
    Disconnect,
}

type Routes = HashMap<String, Answer>;

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct FixtureServer {
    socket: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    routes: Arc<Mutex<Routes>>,
    conns: Arc<Mutex<Vec<Conn>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FixtureServer {
    pub fn start() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let app = format!("fixture-app-{}-{id}", std::process::id());
        let socket = std::env::temp_dir().join(format!("mt-fixture-{}-{id}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("bind fixture socket");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let routes: Arc<Mutex<Routes>> = Arc::new(Mutex::new(HashMap::new()));
        let conns = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let endpoint = Endpoint::new(&app);
        endpoint.register("cmd", 1, cmd_service(requests.clone(), routes.clone()));
        endpoint.register("frames", 1, frames_service(requests.clone()));
        let mut fixture = Self {
            socket,
            requests,
            routes,
            conns: conns.clone(),
            stop: stop.clone(),
            thread: None,
        };
        // `grab` names its folder from the app's own `ping`.
        fixture.route(
            "ping",
            &format!("{{\"pong\":1,\"pid\":{},\"app\":{}}}", std::process::id(), json_str(&app)),
        );
        let thread = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        serve(&endpoint, stream, &conns);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        fixture.thread = Some(thread);
        fixture
    }

    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// Answer `command` with these lines (newline-separated; the `end` line is
    /// added by the fixture).
    pub fn route(&self, command: &str, lines: &str) {
        self.routes
            .lock()
            .unwrap()
            .insert(command.to_string(), Answer::Lines(lines.to_string()));
    }

    /// Every request seen, in order: `cmd k=v ...` for commands (positional
    /// words join the command) and `frames <open args>` for image requests.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Close the command's channel without an answer, as an exiting app may do
    /// after accepting its final shutdown command.
    pub fn disconnect(&self, command: &str) {
        self.routes
            .lock()
            .unwrap()
            .insert(command.to_string(), Answer::Disconnect);
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        for conn in self.conns.lock().unwrap().drain(..) {
            conn.close("fixture stopped");
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn serve(endpoint: &Endpoint, stream: UnixStream, conns: &Arc<Mutex<Vec<Conn>>>) {
    let (Ok(reader), Ok(writer)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    let shutdown = move || {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    };
    if let Ok(conn) = endpoint.serve(FrameReader::new(Box::new(reader)), Box::new(writer), Box::new(shutdown)) {
        conns.lock().unwrap().push(conn);
    }
}

/// The `cmd` service: one answer per request line, `end` line last. The
/// dispatch of the real service is a plain `fn`, so the fixture speaks the
/// protocol itself to keep its tables per fixture.
fn cmd_service(requests: Arc<Mutex<Vec<String>>>, routes: Arc<Mutex<Routes>>) -> Factory {
    Arc::new(move |_, open| {
        let (requests, routes) = (requests.clone(), routes.clone());
        thread::spawn(move || {
            let channel = open.channel;
            while let Some(event) = channel.recv() {
                let data = match event {
                    Event::Data(data) => data,
                    Event::Closed(..) => return,
                    Event::Opened(_) => continue,
                };
                for line in String::from_utf8_lossy(&data).lines().filter(|l| !l.trim().is_empty()) {
                    let req = Req::parse(line);
                    let mut seen = req.cmd.clone();
                    for word in &req.pos {
                        seen.push(' ');
                        seen.push_str(word);
                    }
                    for (key, value) in &req.kv {
                        seen.push_str(&format!(" {key}={value}"));
                    }
                    requests.lock().unwrap().push(seen);
                    let answer = routes.lock().unwrap().get(&req.cmd).cloned();
                    let mut reply = match answer {
                        Some(Answer::Lines(lines)) => lines,
                        Some(Answer::Disconnect) => {
                            channel.close(CLOSE_NORMAL, "");
                            return;
                        }
                        None => format!("{{\"err\":{}}}", json_str(&format!("unknown command `{}`", req.cmd))),
                    };
                    reply.push_str(&format!("\n{{\"end\":{},\"ms\":0}}\n", json_str(&req.cmd)));
                    if channel.send(reply.into_bytes()).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(())
    })
}

/// The `frames` service: `n` (or one, for `once=1`) fake 4x3 frames of the
/// asked window, each record's bytes `PNG<window>-<index>`.
fn frames_service(requests: Arc<Mutex<Vec<String>>>) -> Factory {
    Arc::new(move |_, open| {
        requests.lock().unwrap().push(format!("frames {}", open.args));
        let arg = |key: &str| {
            open.args
                .split_whitespace()
                .find_map(|a| a.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_string)
        };
        let window: usize = arg("window").and_then(|v| v.parse().ok()).unwrap_or(0);
        let count: usize = if arg("once").is_some() {
            1
        } else {
            arg("n").and_then(|v| v.parse().ok()).unwrap_or(1)
        };
        thread::spawn(move || {
            let channel = open.channel;
            for index in 0..count {
                let head = format!(
                    "{{\"seq\":{index},\"w\":4,\"h\":3,\"window\":{window},\"t_ns\":5,\"codec\":\"png\",\"capture_ms\":1.5,\"encode_ms\":0.5,\"capture_kind\":\"pixels\"}}"
                );
                let record = makepad_remote::frames::encode_record(&head, format!("PNG{window}-{index}").as_bytes());
                if channel.send(record).is_err() {
                    return;
                }
            }
            channel.close(CLOSE_NORMAL, "");
        });
        Ok(())
    })
}
