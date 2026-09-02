//! The in-process client hub: the slice of studio's gateway
//! (studio/hub/src/gateway.rs) a compositor needs. A localhost websocket
//! accepts `--stdin-loop` Makepad children at `/app?build=<id>` and
//! forwards their binary `AppToStudio` traffic to the UI thread; the WM
//! answers over the per-socket sender with `StudioToAppVec` frames.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, Sender};

use makepad_network::http_server::{start_http_server, HttpServer, HttpServerRequest};
use makepad_studio_protocol::{AppToStudio, AppToStudioVec, StudioToApp, StudioToAppVec};
use makepad_widgets::makepad_micro_serde::{DeBin, SerBin};
use makepad_widgets::makepad_platform::thread::SignalToUI;

pub type ClientId = u64;

pub enum HubEvent {
    /// A child connected its websocket; `sender` transmits raw ws frames.
    Connected {
        client: ClientId,
        socket: u64,
        sender: Sender<Vec<u8>>,
    },
    Disconnected {
        socket: u64,
    },
    FromApp {
        client: ClientId,
        msgs: Vec<AppToStudio>,
    },
}

pub struct WmHub {
    pub port: u16,
    pub rx: Receiver<HubEvent>,
}

impl WmHub {
    /// Bind the first free port in a small range and start serving.
    pub fn start() -> Option<WmHub> {
        for port in 8765..8785u16 {
            if let Some(hub) = Self::start_on(port) {
                return Some(hub);
            }
        }
        None
    }

    fn start_on(port: u16) -> Option<WmHub> {
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().ok()?;
        // The http server's bind failure only prints; probe the port first
        // so a WM instance still holding it (or any other service) makes
        // us move on to the next one instead of hosting nothing.
        match std::net::TcpListener::bind(addr) {
            Ok(probe) => drop(probe),
            Err(_) => return None,
        }
        let (request_tx, request_rx) = mpsc::channel::<HttpServerRequest>();
        start_http_server(HttpServer {
            listen_address: addr,
            request: request_tx,
            post_max_size: 1024 * 1024,
            post_max_size_overrides: Vec::new(),
        })?;

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        std::thread::Builder::new()
            .name("wm-hub".into())
            .spawn(move || {
                // socket id -> client id, so binary frames route by socket.
                let mut socket_client = HashMap::<u64, ClientId>::new();
                while let Ok(request) = request_rx.recv() {
                    match request {
                        HttpServerRequest::ConnectWebSocket {
                            web_socket_id,
                            headers,
                            response_sender,
                        } => {
                            let path = if let Some(search) =
                                headers.search.as_ref().filter(|s| !s.is_empty())
                            {
                                format!("{}?{}", headers.path, search)
                            } else {
                                headers.path.clone()
                            };
                            let Some(client) = parse_app_path(&path) else {
                                // Not a client socket: close it.
                                let _ = response_sender.send(Vec::new());
                                continue;
                            };
                            socket_client.insert(web_socket_id, client);
                            let _ = event_tx.send(HubEvent::Connected {
                                client,
                                socket: web_socket_id,
                                sender: response_sender,
                            });
                            SignalToUI::set_ui_signal();
                        }
                        HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                            if socket_client.remove(&web_socket_id).is_some() {
                                let _ = event_tx.send(HubEvent::Disconnected {
                                    socket: web_socket_id,
                                });
                                SignalToUI::set_ui_signal();
                            }
                        }
                        HttpServerRequest::BinaryMessage {
                            web_socket_id,
                            data,
                            ..
                        } => {
                            let Some(&client) = socket_client.get(&web_socket_id) else {
                                continue;
                            };
                            // Children send both single messages and vecs.
                            let msgs = match AppToStudioVec::deserialize_bin(&data) {
                                Ok(vec) => vec.0,
                                Err(_) => match AppToStudio::deserialize_bin(&data) {
                                    Ok(msg) => vec![msg],
                                    Err(_) => continue,
                                },
                            };
                            let _ = event_tx.send(HubEvent::FromApp { client, msgs });
                            SignalToUI::set_ui_signal();
                        }
                        _ => {}
                    }
                }
            })
            .ok()?;

        Some(WmHub {
            port,
            rx: event_rx,
        })
    }
}

/// Send a batch of StudioToApp messages over a client socket sender.
pub fn send_to_app(sender: &Sender<Vec<u8>>, msgs: Vec<StudioToApp>) {
    if msgs.is_empty() {
        return;
    }
    let _ = sender.send(StudioToAppVec(msgs).serialize_bin());
}

/// `/app/<id>` and `/app?build=<id>[&crate=..]`, like studio's gateway.
fn parse_app_path(path: &str) -> Option<ClientId> {
    if let Some(rest) = path.strip_prefix("/app/") {
        if rest.is_empty() || rest.contains('/') {
            return None;
        }
        return rest.parse::<u64>().ok();
    }
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    if route != "/app" {
        return None;
    }
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "build" {
            return value.trim().parse::<u64>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_paths() {
        assert_eq!(parse_app_path("/app/42"), Some(42));
        assert_eq!(parse_app_path("/app?build=7&crate=terminal"), Some(7));
        assert_eq!(parse_app_path("/app?crate=terminal"), None);
        assert_eq!(parse_app_path("/ui"), None);
        assert_eq!(parse_app_path("/app/x"), None);
    }
}
