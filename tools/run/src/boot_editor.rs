//! The existing Studio JumpToFile wire path, owned by the run action worker.
//! No Cx, app websocket thread or widget dependencies are needed to open a location.
use makepad_live_id::LiveId;
use makepad_network::{
    HttpMethod, HttpRequest, NetworkResponse, NetworkRuntime, WebSocketTransport, WsSend,
};
use makepad_script::makepad_math::makepad_micro_serde::SerBin;
use makepad_studio_protocol::{AppToStudio, AppToStudioVec, JumpToFile};
use std::collections::VecDeque;

const SOCKET: LiveId = LiveId(1);

pub struct Editor {
    network: Option<NetworkRuntime>,
    connected: bool,
    pending: VecDeque<AppToStudio>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            network: None,
            connected: false,
            pending: VecDeque::new(),
        }
    }

    pub fn open_file(&mut self, file: String, line: u32, col: u32) -> Result<(), String> {
        if self.network.is_none() {
            let url =
                studio_url().ok_or("Connect an editor through Studio to open source locations")?;
            let network = NetworkRuntime::new(Default::default());
            network.set_quiet_socket(Some(SOCKET));
            let mut request = HttpRequest::new(url, HttpMethod::GET);
            request.set_websocket_transport(WebSocketTransport::PlainTcp);
            network
                .ws_open(SOCKET, request)
                .map_err(|e| format!("Studio connection: {e:?}"))?;
            self.network = Some(network);
        }
        self.pending.push_back(AppToStudio::JumpToFile(JumpToFile {
            file_name: file,
            line,
            column: col,
        }));
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        let Some(network) = &self.network else {
            return errors;
        };
        let mut disconnected = false;
        while let Some(event) = network.try_recv() {
            match event {
                NetworkResponse::WsOpened { .. } => self.connected = true,
                NetworkResponse::WsClosed { .. } => {
                    disconnected = true;
                    errors.push("Studio disconnected; pending source locations retained. Use Open on dev machine (E) to reconnect.".into());
                }
                NetworkResponse::WsError { message, .. } => {
                    disconnected = true;
                    errors.push(message);
                }
                _ => {}
            }
        }
        if disconnected {
            self.connected = false;
            let _ = network.ws_close(SOCKET);
            self.network = None;
            return errors;
        }
        if self.connected {
            while let Some(message) = self.pending.front() {
                match network.ws_send(
                    SOCKET,
                    WsSend::Binary(AppToStudioVec(vec![message.clone()]).serialize_bin()),
                ) {
                    Ok(()) => {
                        self.pending.pop_front();
                    }
                    Err(error) => {
                        errors.push(format!(
                            "Studio send failed: {error:?}; source location retained"
                        ));
                        self.connected = false;
                        let _ = network.ws_close(SOCKET);
                        self.network = None;
                        break;
                    }
                }
            }
        }
        errors
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if let Some(network) = &self.network {
            let _ = network.ws_close(SOCKET);
        }
    }
}

fn studio_url() -> Option<String> {
    // Same identity as the app's existing /app?build=...&crate=... connection.
    let studio = std::env::var("STUDIO").unwrap_or_default();
    let host = std::env::var("STUDIO_HOST").unwrap_or_else(|_| studio.clone());
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        return None;
    }
    let (scheme, rest) = host.split_once("://").unwrap_or(("http", host));
    let authority = rest.split(['/', '?', '#']).next()?;
    let query = |name: &str| {
        studio.split_once('?').and_then(|(_, query)| {
            query
                .split('&')
                .filter_map(|p| p.split_once('='))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        })
    };
    let build = std::env::var("STUDIO_BUILD")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| query("build"))
        .or_else(|| {
            studio
                .split_once("/app/")
                .and_then(|(_, v)| v.split(['/', '?', '#']).next())
                .map(str::to_string)
        });
    let package = std::env::var("STUDIO_CRATE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| query("crate"));
    let mut identity = Vec::new();
    if let Some(build) = build {
        identity.push(format!("build={build}"));
    }
    if let Some(package) = package {
        identity.push(format!("crate={package}"));
    }
    if identity.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}/app?{}", identity.join("&")))
}
