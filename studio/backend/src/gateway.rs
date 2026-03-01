use crate::dispatch::StudioEvent;
use makepad_studio_protocol::backend_protocol::QueryId;
use makepad_network::{
    start_http_server, HttpServer, HttpServerRequest, HttpServerResponse, ToUISender,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

enum SocketRole {
    Ui,
    App,
    BuildBox,
}

pub struct GatewayHandle {
    pub listen_address: SocketAddr,
    pub request_thread: JoinHandle<()>,
    pub http_thread: JoinHandle<()>,
}

pub fn start_http_gateway(
    listen_address: SocketAddr,
    post_max_size: u64,
    event_tx: Sender<StudioEvent>,
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
                    if headers.path == "/$studio_ui" {
                        socket_roles.insert(web_socket_id, SocketRole::Ui);
                        let _ = event_tx.send(StudioEvent::UiConnected {
                            connection_id: web_socket_id,
                            sender: ToUISender::from_sender(response_sender),
                            typed_sender: None,
                        });
                        continue;
                    }
                    if let Some(build_id) = parse_app_path(&headers.path) {
                        socket_roles.insert(web_socket_id, SocketRole::App);
                        let _ = event_tx.send(StudioEvent::AppConnected {
                            build_id,
                            connection_id: web_socket_id,
                            sender: response_sender,
                        });
                        continue;
                    }
                    if headers.path == "/$studio_buildbox" {
                        socket_roles.insert(web_socket_id, SocketRole::BuildBox);
                        let _ = event_tx.send(StudioEvent::BuildBoxConnected {
                            connection_id: web_socket_id,
                            sender: response_sender,
                        });
                        continue;
                    }
                    let _ = response_sender.send(Vec::new());
                }
                HttpServerRequest::DisconnectWebSocket { web_socket_id } => {
                    if let Some(role) = socket_roles.remove(&web_socket_id) {
                        match role {
                            SocketRole::Ui => {
                                let _ =
                                    event_tx.send(StudioEvent::UiDisconnected { connection_id: web_socket_id });
                            }
                            SocketRole::App => {
                                let _ =
                                    event_tx.send(StudioEvent::AppDisconnected { connection_id: web_socket_id });
                            }
                            SocketRole::BuildBox => {
                                let _ = event_tx
                                    .send(StudioEvent::BuildBoxDisconnected { connection_id: web_socket_id });
                            }
                        }
                    }
                }
                HttpServerRequest::BinaryMessage {
                    web_socket_id,
                    response_sender: _,
                    data,
                } => match socket_roles.get(&web_socket_id) {
                    Some(SocketRole::Ui) => {
                        let _ = event_tx.send(StudioEvent::UiBinary {
                            connection_id: web_socket_id,
                            data,
                        });
                    }
                    Some(SocketRole::App) => {
                        let _ = event_tx.send(StudioEvent::AppBinary {
                            connection_id: web_socket_id,
                            data,
                        });
                    }
                    Some(SocketRole::BuildBox) => {
                        let _ = event_tx.send(StudioEvent::BuildBoxBinary {
                            connection_id: web_socket_id,
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
                    Some(SocketRole::Ui) => {
                        let _ = event_tx.send(StudioEvent::UiText {
                            connection_id: web_socket_id,
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

fn parse_app_path(path: &str) -> Option<QueryId> {
    for prefix in ["/$studio_app/", "/$studio_web_socket/"] {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            return None;
        }
        if let Ok(id) = rest.parse::<u64>() {
            return Some(QueryId(id));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_studio_app_path() {
        assert_eq!(parse_app_path("/$studio_app/42"), Some(QueryId(42)));
    }

    #[test]
    fn parse_current_studio_web_socket_path() {
        assert_eq!(parse_app_path("/$studio_web_socket/99"), Some(QueryId(99)));
    }

    #[test]
    fn reject_missing_or_invalid_build_id() {
        assert_eq!(parse_app_path("/$studio_app/"), None);
        assert_eq!(parse_app_path("/$studio_web_socket/not-a-number"), None);
        assert_eq!(parse_app_path("/$studio_ui"), None);
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
