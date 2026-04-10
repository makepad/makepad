use crate::dispatch::HubEvent;
use makepad_micro_serde::SerBin;
use makepad_script_std::makepad_network::{
    start_http_server, HttpServer, HttpServerRequest, HttpServerResponse, ToUISender,
};
use makepad_studio_protocol::hub_protocol::{HubToClient, QueryId};
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

#[derive(Clone, Copy)]
enum SocketRole {
    Client,
    App,
    BuildBox,
}

pub struct GatewayHandle {
    pub listen_address: SocketAddr,
    pub request_thread: JoinHandle<()>,
    pub http_thread: JoinHandle<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppConnectInfo {
    build_id: Option<QueryId>,
    crate_name: Option<String>,
}

fn studio_hub_debug_enabled() -> bool {
    env::var_os("MAKEPAD_STUDIO_HUB_DEBUG").is_some()
}

pub fn start_http_gateway(
    listen_address: SocketAddr,
    post_max_size: u64,
    event_tx: Sender<HubEvent>,
) -> Result<GatewayHandle, String> {
    let (request_tx, request_rx) = mpsc::channel::<HttpServerRequest>();
    let http_thread = start_http_server(HttpServer {
        listen_address,
        request: request_tx,
        post_max_size,
    })
    .ok_or_else(|| format!("failed to bind http server at {}", listen_address))?;

    let request_thread = std::thread::spawn(move || {
        let mut socket_roles = HashMap::<u64, SocketRole>::new();
        while let Ok(request) = request_rx.recv() {
            match request {
                HttpServerRequest::ConnectWebSocket {
                    web_socket_id,
                    headers,
                    response_sender,
                } => {
                    if studio_hub_debug_enabled() {
                        eprintln!(
                            "studio hub debug: websocket connect id={} path={}",
                            web_socket_id, headers.path
                        );
                    }
                    if headers.path == "/ui" {
                        if studio_hub_debug_enabled() {
                            eprintln!(
                                "studio hub debug: websocket id={} accepted as ui client",
                                web_socket_id
                            );
                        }
                        socket_roles.insert(web_socket_id, SocketRole::Client);
                        let _ = event_tx.send(HubEvent::ClientConnected {
                            web_socket_id: web_socket_id,
                            sender: ToUISender::from_sender(response_sender),
                            typed_sender: None,
                        });
                        continue;
                    }
                    if let Some(app_connect) = parse_app_path(&headers.path) {
                        if studio_hub_debug_enabled() {
                            eprintln!(
                                "studio hub debug: websocket id={} accepted as app build={:?} crate={:?}",
                                web_socket_id,
                                app_connect.build_id.map(|id| id.0),
                                app_connect.crate_name
                            );
                        }
                        socket_roles.insert(web_socket_id, SocketRole::App);
                        let _ = event_tx.send(HubEvent::AppConnected {
                            build_id: app_connect.build_id,
                            crate_name: app_connect.crate_name,
                            web_socket_id: web_socket_id,
                            sender: response_sender,
                        });
                        continue;
                    }
                    if headers.path == "/$studio_buildbox" {
                        if studio_hub_debug_enabled() {
                            eprintln!(
                                "studio hub debug: websocket id={} accepted as buildbox",
                                web_socket_id
                            );
                        }
                        socket_roles.insert(web_socket_id, SocketRole::BuildBox);
                        let _ = event_tx.send(HubEvent::BuildBoxConnected {
                            web_socket_id: web_socket_id,
                            sender: response_sender,
                        });
                        continue;
                    }
                    if studio_hub_debug_enabled() {
                        eprintln!(
                            "studio hub debug: websocket id={} rejected path={}",
                            web_socket_id, headers.path
                        );
                    }
                    let _ = response_sender.send(
                        HubToClient::Error {
                            message: format!("invalid websocket path: {}", headers.path),
                        }
                        .serialize_bin(),
                    );
                    let _ = response_sender.send(Vec::new());
                }
                HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                    if let Some(role) = socket_roles.remove(&web_socket_id) {
                        if studio_hub_debug_enabled() {
                            let role_name = match role {
                                SocketRole::Client => "ui client",
                                SocketRole::App => "app",
                                SocketRole::BuildBox => "buildbox",
                            };
                            eprintln!(
                                "studio hub debug: websocket disconnect id={} role={}",
                                web_socket_id, role_name
                            );
                        }
                        match role {
                            SocketRole::Client => {
                                let _ = event_tx.send(HubEvent::ClientDisconnected {
                                    web_socket_id: web_socket_id,
                                });
                            }
                            SocketRole::App => {
                                let _ = event_tx.send(HubEvent::AppDisconnected {
                                    web_socket_id: web_socket_id,
                                });
                            }
                            SocketRole::BuildBox => {
                                let _ = event_tx.send(HubEvent::BuildBoxDisconnected {
                                    web_socket_id: web_socket_id,
                                });
                            }
                        }
                    }
                }
                HttpServerRequest::BinaryMessage {
                    web_socket_id,
                    response_sender: _,
                    data,
                } => match socket_roles.get(&web_socket_id) {
                    Some(SocketRole::Client) => {
                        let _ = event_tx.send(HubEvent::ClientBinary {
                            web_socket_id: web_socket_id,
                            data,
                        });
                    }
                    Some(SocketRole::App) => {
                        let _ = event_tx.send(HubEvent::AppBinary {
                            web_socket_id: web_socket_id,
                            data,
                        });
                    }
                    Some(SocketRole::BuildBox) => {
                        let _ = event_tx.send(HubEvent::BuildBoxBinary {
                            web_socket_id: web_socket_id,
                            data,
                        });
                    }
                    None => {}
                },
                HttpServerRequest::TextMessage {
                    web_socket_id,
                    response_sender: _,
                    string,
                } => match socket_roles.get(&web_socket_id) {
                    Some(SocketRole::Client) => {
                        let _ = event_tx.send(HubEvent::ClientText {
                            web_socket_id: web_socket_id,
                            text: string,
                        });
                    }
                    Some(SocketRole::App) | Some(SocketRole::BuildBox) | None => {}
                },
                HttpServerRequest::Get {
                    headers,
                    response_sender,
                } => {
                    if headers.path == "/$studio_health" {
                        let _ = response_sender.send(ok_response(b"ok".to_vec(), "text/plain"));
                    } else {
                        let _ = response_sender.send(not_found_response());
                    }
                }
                HttpServerRequest::Post { response, .. } => {
                    let _ = response.send(not_found_response());
                }
            }
        }
    });

    Ok(GatewayHandle {
        listen_address,
        request_thread,
        http_thread,
    })
}

fn parse_app_path(path: &str) -> Option<AppConnectInfo> {
    if let Some(rest) = path.strip_prefix("/app/") {
        if rest.is_empty() || rest.contains('/') {
            return None;
        }
        let Ok(id) = rest.parse::<u64>() else {
            return None;
        };
        return Some(AppConnectInfo {
            build_id: Some(QueryId(id)),
            crate_name: None,
        });
    }

    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    if route != "/app" {
        return None;
    }

    let mut build_id = None;
    let mut crate_name = None;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = value.trim();
        match key {
            "build" => {
                if value.is_empty() {
                    continue;
                }
                let Ok(id) = value.parse::<u64>() else {
                    return None;
                };
                build_id = Some(QueryId(id));
            }
            "crate" => {
                if !value.is_empty() {
                    crate_name = Some(value.to_string());
                }
            }
            _ => {}
        }
    }

    if build_id.is_none() && crate_name.is_none() {
        return None;
    }

    Some(AppConnectInfo {
        build_id,
        crate_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_app_path() {
        assert_eq!(
            parse_app_path("/app/42"),
            Some(AppConnectInfo {
                build_id: Some(QueryId(42)),
                crate_name: None,
            })
        );
        assert_eq!(
            parse_app_path("/app?build=42&crate=makepad-example-xr"),
            Some(AppConnectInfo {
                build_id: Some(QueryId(42)),
                crate_name: Some("makepad-example-xr".to_string()),
            })
        );
        assert_eq!(
            parse_app_path("/app?crate=makepad-example-xr"),
            Some(AppConnectInfo {
                build_id: None,
                crate_name: Some("makepad-example-xr".to_string()),
            })
        );
    }

    #[test]
    fn reject_missing_or_invalid_build_id() {
        assert_eq!(parse_app_path("/app/"), None);
        assert_eq!(parse_app_path("/app/not-a-number"), None);
        assert_eq!(parse_app_path("/app/77/extra"), None);
        assert_eq!(parse_app_path("/app"), None);
        assert_eq!(parse_app_path("/app?build=nope&crate=makepad-example-xr"), None);
        assert_eq!(parse_app_path("/ui"), None);
    }
}

fn ok_response(body: Vec<u8>, content_type: &str) -> HttpServerResponse {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type,
        body.len()
    );
    HttpServerResponse { header, body }
}

fn not_found_response() -> HttpServerResponse {
    let body = b"not found".to_vec();
    let header = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    HttpServerResponse { header, body }
}
