//! The in-process client hub: the slice of studio's gateway
//! (studio/hub/src/gateway.rs) a host needs. A localhost websocket
//! accepts `--stdin-loop` Makepad children at `/app?build=<id>` and
//! forwards their binary `AppToStudio` traffic to the UI thread; the host
//! answers over the per-socket sender with `StudioToAppVec` frames.
//!
//! A WM client states the version it negotiated on the WM socket (`proto=<v>`).
//! A version in `makepad_studio_protocol::HOSTED_VERSIONS` admits it at that version,
//! whatever its `wire=<hex>` fingerprint. A child that states no version must state
//! this build's `makepad_studio_protocol::WIRE` and is admitted at its newest version.
//! Otherwise its socket is closed and the host hears [`HubEvent::Mismatch`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, Sender};

use makepad_network::http_server::{start_http_server, HttpServer, HttpServerRequest, HttpServerThread};
use makepad_studio_protocol::{AppToStudio, AppToStudioVec, StudioToApp, StudioToAppVec, HOSTED_VERSIONS, WIRE};
use makepad_platform::makepad_micro_serde::{DeBin, SerBin};
use makepad_platform::thread::{SignalToUI, ThreadOptions, ThreadSpawner};

pub type ClientId = u64;

pub enum HubEvent {
    /// A child connected its websocket; `sender` transmits raw ws frames.
    Connected {
        client: ClientId,
        version: u32,
        socket: u64,
        sender: Sender<Vec<u8>>,
    },
    Disconnected {
        socket: u64,
    },
    /// A child stated an unsupported version, or no version and another wire
    /// (`wire`: what it stated, None: nothing). It was not admitted.
    Mismatch {
        client: ClientId,
        wire: Option<u64>,
    },
    FromApp {
        client: ClientId,
        msgs: Vec<AppToStudio>,
    },
}

pub struct AppHub {
    pub port: u16,
    pub rx: Receiver<HubEvent>,
    /// Its accept loop. Dropping the hub stops it; the `app-hub` worker ends when the last
    /// connection did (its request channel closes), so a quitting host leaves no thread behind.
    server: HttpServerThread,
}

impl Drop for AppHub {
    fn drop(&mut self) {
        self.server.stop();
    }
}

impl AppHub {
    /// Bind the first free port in a small range and start serving.
    pub fn start(spawner: ThreadSpawner) -> Option<AppHub> {
        for port in 8765..8785u16 {
            if let Some(hub) = Self::start_on(port, &spawner) {
                return Some(hub);
            }
        }
        None
    }

    fn start_on(port: u16, spawner: &ThreadSpawner) -> Option<AppHub> {
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().ok()?;
        // The http server's bind failure only prints; probe the port first
        // so a host instance still holding it (or any other service) makes
        // us move on to the next one instead of hosting nothing.
        match std::net::TcpListener::bind(addr) {
            Ok(probe) => drop(probe),
            Err(_) => return None,
        }
        let (request_tx, request_rx) = mpsc::channel::<HttpServerRequest>();
        let server = start_http_server(HttpServer {
            listen_address: addr,
            request: request_tx,
            post_max_size: 1024 * 1024,
            post_max_size_overrides: Vec::new(),
            pre_admit_posts: false,
            client_ip_resolver: None,
            trusted_proxy: None,
            allowed_methods: None,
        })?;

        let (event_tx, event_rx) = mpsc::channel::<HubEvent>();
        let worker = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("app-hub".into()),
                    ..Default::default()
                },
                move || {
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
                            let (client, version) = match admit(&path) {
                                Some(Ok(admission)) => admission,
                                Some(Err(mismatch)) => {
                                    let _ = response_sender.send(Vec::new());
                                    let _ = event_tx.send(mismatch);
                                    SignalToUI::set_ui_signal();
                                    continue;
                                }
                                None => {
                                    // Not a client socket: close it.
                                    let _ = response_sender.send(Vec::new());
                                    continue;
                                }
                            };
                            socket_client.insert(web_socket_id, client);
                            let _ = event_tx.send(HubEvent::Connected {
                                client,
                                version,
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
                },
            )
            .ok();
        let Some(worker) = worker else {
            server.stop();
            return None;
        };
        worker.detach();

        Some(AppHub {
            port,
            rx: event_rx,
            server,
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

/// A child's client id and admitted version: a `proto` in [`HOSTED_VERSIONS`], whatever its
/// wire, or no `proto` and this build's [`WIRE`], at the newest version. Otherwise
/// [`HubEvent::Mismatch`]; None: not a child's socket.
fn admit(path: &str) -> Option<Result<(ClientId, u32), HubEvent>> {
    let (client, wire, proto) = parse_app_path(path)?;
    Some(match proto {
        Some(version) if (HOSTED_VERSIONS.0..=HOSTED_VERSIONS.1).contains(&version) => Ok((client, version)),
        None if wire == Some(WIRE) => Ok((client, HOSTED_VERSIONS.1)),
        _ => Err(HubEvent::Mismatch { client, wire }),
    })
}

/// `/app/<id>` and `/app?build=<id>[&crate=..][&wire=<hex>][&proto=<v>]`:
/// the client, wire and version it states.
fn parse_app_path(path: &str) -> Option<(ClientId, Option<u64>, Option<u32>)> {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let value = |name: &str| query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| value.trim())
    });
    let wire = value("wire").and_then(|hex| u64::from_str_radix(hex, 16).ok());
    let proto = value("proto").and_then(|v| v.parse().ok());
    if let Some(rest) = route.strip_prefix("/app/") {
        if rest.is_empty() || rest.contains('/') {
            return None;
        }
        return Some((rest.parse::<u64>().ok()?, wire, proto));
    }
    if route != "/app" {
        return None;
    }
    Some((value("build")?.parse::<u64>().ok()?, wire, proto))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_paths() {
        assert_eq!(parse_app_path("/app/42"), Some((42, None, None)));
        assert_eq!(parse_app_path("/app?build=7&crate=terminal"), Some((7, None, None)));
        assert_eq!(parse_app_path("/app?build=7&crate=terminal&wire=00000000000000ff"), Some((7, Some(255), None)));
        assert_eq!(parse_app_path("/app/42?wire=1f"), Some((42, Some(31), None)));
        assert_eq!(parse_app_path("/app?build=7&wire=nothex"), Some((7, None, None)));
        assert_eq!(parse_app_path("/app?build=7&wire=ff&proto=1"), Some((7, Some(255), Some(1))));
        assert_eq!(parse_app_path("/app?build=7&proto=x"), Some((7, None, None)));
        assert_eq!(parse_app_path("/app?crate=terminal"), None);
        assert_eq!(parse_app_path("/ui"), None);
        assert_eq!(parse_app_path("/app/x"), None);
    }

    /// Fake children connecting: one built from these sources, one that
    /// states another wire, one from before children stated it.
    #[test]
    fn an_unversioned_child_needs_this_wire() {
        let ours = format!("/app?build=3&crate=makepad-example-calculator&wire={WIRE:016x}");
        assert!(matches!(admit(&ours), Some(Ok((3, version))) if version == HOSTED_VERSIONS.1));
        let other = format!("/app?build=4&crate=makepad-example-calculator&wire={:016x}", WIRE ^ 1);
        assert!(matches!(admit(&other), Some(Err(HubEvent::Mismatch { client: 4, wire: Some(w) })) if w == WIRE ^ 1));
        assert!(matches!(admit("/app?build=5&crate=makepad-example-calculator"), Some(Err(HubEvent::Mismatch { client: 5, wire: None }))));
        assert!(admit("/ui").is_none());
    }

    #[test]
    fn a_versioned_child_is_admitted_whatever_its_wire() {
        let other = format!("/app?build=3&wire={:016x}&proto=1", WIRE ^ 1);
        assert!(matches!(admit(&other), Some(Ok((3, 1)))));
        assert!(matches!(admit("/app?build=4&proto=2"), Some(Ok((4, 2)))));
        let unsupported = format!("/app?build=5&wire={WIRE:016x}&proto=9");
        assert!(matches!(admit(&unsupported), Some(Err(HubEvent::Mismatch { client: 5, wire: Some(WIRE) }))));
    }
}
