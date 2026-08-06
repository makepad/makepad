//! The `/pair` HTTP endpoint: a keyboard-less device serves this on the LAN so
//! a nearby computer can paste an API key into it.
//!
//! Uses platform/network's http_server. The device shows the URL and a 4-digit
//! code; the page echoes the code back with the key. See pairing.rs for the
//! (deliberately modest) security posture.

use crate::pairing::{PairError, Pairing, Provider};
use makepad_widgets::makepad_platform::makepad_network::http_server::*;
use makepad_widgets::makepad_platform::makepad_network::{NetworkConfig, NetworkRuntime};
use std::net::SocketAddr;
use std::sync::mpsc;

pub struct PairServer {
    pub pairing: Pairing,
    pub addr: SocketAddr,
    requests: mpsc::Receiver<HttpServerRequest>,
    _runtime: NetworkRuntime,
}

/// What the last poll did, so the UI can say something true.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairEvent {
    Stored(Provider),
    Rejected(PairError),
}

/// The port the device advertises: fixed, because the user has to type it.
pub const DEFAULT_PAIR_PORT: u16 = 8790;

impl PairServer {
    pub fn start(provider: Provider, seed: u64, port: u16) -> Option<Self> {
        let runtime = NetworkRuntime::new(NetworkConfig::default());
        let (tx, rx) = mpsc::channel::<HttpServerRequest>();
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        runtime.start_http_server(HttpServer {
            listen_address: addr,
            // A key is a few hundred bytes; anything larger is not a key.
            post_max_size: 8 * 1024,
            request: tx,
        });
        Some(Self {
            pairing: Pairing::new(provider, seed),
            addr,
            requests: rx,
            _runtime: runtime,
        })
    }

    /// Non-blocking: drain whatever arrived since the last call.
    pub fn poll(&mut self) -> Vec<PairEvent> {
        let mut events = Vec::new();
        while let Ok(request) = self.requests.try_recv() {
            match request {
                HttpServerRequest::Get { headers, response_sender } => {
                    let (status, body) = if headers.path == "/pair" {
                        (200, self.pairing.page_html())
                    } else {
                        (404, "not found".to_string())
                    };
                    send(&response_sender, status, "text/html", body.into_bytes());
                }
                HttpServerRequest::Post { headers, body, response } => {
                    if headers.path != "/pair" {
                        send(&response, 404, "text/plain", b"not found".to_vec());
                        continue;
                    }
                    let text = String::from_utf8_lossy(&body).to_string();
                    match self.pairing.accept(&text) {
                        Ok(key) => {
                            // Store, then report WITHOUT the key in the message.
                            match crate::pairing::store_key(self.pairing.provider, &key) {
                                Ok(()) => {
                                    events.push(PairEvent::Stored(self.pairing.provider));
                                    send(
                                        &response,
                                        200,
                                        "text/html",
                                        b"<h2>Connected.</h2><p>You can close this page.</p>"
                                            .to_vec(),
                                    );
                                }
                                Err(_) => send(
                                    &response,
                                    500,
                                    "text/html",
                                    b"<h2>Could not store the key on the device.</h2>".to_vec(),
                                ),
                            }
                        }
                        Err(err) => {
                            events.push(PairEvent::Rejected(err));
                            let msg: &[u8] = match err {
                                PairError::WrongCode => {
                                    b"<h2>Wrong code.</h2><p>Check the code on the device.</p>"
                                }
                                PairError::Malformed => b"<h2>Missing code or key.</h2>",
                            };
                            send(&response, 400, "text/html", msg.to_vec());
                        }
                    }
                }
                // Pairing speaks only GET/POST; sockets and messages are
                // not part of this surface.
                _ => {}
            }
        }
        events
    }
}

fn send(
    sender: &mpsc::Sender<HttpServerResponse>,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
) {
    let _ = sender.send(HttpServerResponse {
        header: format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
        body,
    });
}
