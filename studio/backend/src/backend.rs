use crate::dispatch::{StudioCore, StudioEvent};
use crate::gateway::{start_http_gateway, GatewayHandle};
use makepad_studio_protocol::backend_protocol::{ClientId, QueryId, StudioToUI, UIToStudio, UIToStudioEnvelope};
use crate::virtual_fs::VirtualFs;
use makepad_network::ToUIReceiver;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

// In-process UI uses a dedicated transport connection id that never overlaps
// with websocket ids from the gateway (which start from 1).
const IN_PROCESS_UI_CONNECTION_ID: u64 = 0;

#[derive(Clone, Debug)]
pub struct MountConfig {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub listen_address: SocketAddr,
    pub post_max_size: u64,
    pub mounts: Vec<MountConfig>,
    pub enable_in_process_gateway: bool,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001),
            post_max_size: 1024 * 1024,
            mounts: Vec::new(),
            enable_in_process_gateway: false,
        }
    }
}

pub struct BackendHandle {
    pub listen_address: SocketAddr,
    pub event_tx: Sender<StudioEvent>,
    pub _gateway: GatewayHandle,
    pub _core_thread: JoinHandle<()>,
}

pub struct StudioConnection {
    client_id: ClientId,
    web_socket_id: u64,
    event_tx: Sender<StudioEvent>,
    recv_typed: ToUIReceiver<StudioToUI>,
    next_counter: u64,
    _gateway: Option<GatewayHandle>,
    _core_thread: JoinHandle<()>,
}

impl StudioConnection {
    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn send(&mut self, msg: UIToStudio) -> QueryId {
        let query_id = QueryId::new(self.client_id, self.next_counter);
        self.next_counter = self.next_counter.wrapping_add(1);
        let envelope = UIToStudioEnvelope { query_id, msg };
        let _ = self.event_tx.send(StudioEvent::UiEnvelope {
            web_socket_id: self.web_socket_id,
            envelope,
        });
        query_id
    }

    pub fn cancel_query(&mut self, query_id: QueryId) {
        let _ = self.send(UIToStudio::CancelQuery { query_id });
    }

    pub fn try_recv(&self) -> Option<StudioToUI> {
        self.recv_typed.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<StudioToUI> {
        self.recv_typed.receiver.recv_timeout(timeout).ok()
    }
}

pub struct StudioBackend;

impl StudioBackend {
    pub fn start_in_process(config: BackendConfig) -> Result<StudioConnection, String> {
        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();

        let mut vfs = VirtualFs::new();
        for mount in &config.mounts {
            vfs.mount(&mount.name, mount.path.clone())
                .map_err(|e| format!("mount {} failed: {}", mount.name, e))?;
        }

        let mut gateway = None;
        let mut studio_addr = None;
        if config.enable_in_process_gateway {
            let handle = start_http_gateway_with_fallback(
                config.listen_address,
                config.post_max_size,
                &event_tx,
            )?;
            studio_addr = Some(studio_addr_for_child(handle.listen_address));
            gateway = Some(handle);
        }

        let mut core = StudioCore::new(event_rx, event_tx.clone(), vfs, studio_addr);
        let core_thread = std::thread::spawn(move || {
            core.run();
        });

        let web_socket_id = IN_PROCESS_UI_CONNECTION_ID;
        let raw_rx = ToUIReceiver::<Vec<u8>>::default();
        let typed_rx = ToUIReceiver::<StudioToUI>::default();
        event_tx
            .send(StudioEvent::UiConnected {
                web_socket_id,
                sender: raw_rx.sender(),
                typed_sender: Some(typed_rx.sender()),
            })
            .map_err(|err| format!("failed to connect in-process ui client: {}", err))?;

        let hello = typed_rx
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|err| format!("backend did not send hello: {}", err))?;
        let client_id = match hello {
            StudioToUI::Hello { client_id } => client_id,
            other => {
                return Err(format!(
                    "expected hello from backend, got unexpected message: {:?}",
                    other
                ))
            }
        };

        Ok(StudioConnection {
            client_id,
            web_socket_id,
            event_tx,
            recv_typed: typed_rx,
            next_counter: 0,
            _gateway: gateway,
            _core_thread: core_thread,
        })
    }

    pub fn start_headless(config: BackendConfig) -> Result<BackendHandle, String> {
        let (event_tx, event_rx) = mpsc::channel::<StudioEvent>();

        let mut vfs = VirtualFs::new();
        for mount in &config.mounts {
            vfs.mount(&mount.name, mount.path.clone())
                .map_err(|e| format!("mount {} failed: {}", mount.name, e))?;
        }

        let gateway = start_http_gateway_with_fallback(
            config.listen_address,
            config.post_max_size,
            &event_tx,
        )?;
        let listen_address = gateway.listen_address;

        let mut core = StudioCore::new(
            event_rx,
            event_tx.clone(),
            vfs,
            Some(studio_addr_for_child(listen_address)),
        );
        let core_thread = std::thread::spawn(move || {
            core.run();
        });

        Ok(BackendHandle {
            listen_address,
            event_tx,
            _gateway: gateway,
            _core_thread: core_thread,
        })
    }
}

fn start_http_gateway_with_fallback(
    base: SocketAddr,
    post_max_size: u64,
    event_tx: &Sender<StudioEvent>,
) -> Result<GatewayHandle, String> {
    let mut last_err: Option<String> = None;
    for candidate in gateway_bind_candidates(base) {
        match start_http_gateway(candidate, post_max_size, event_tx.clone()) {
            Ok(handle) => return Ok(handle),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        format!(
            "failed to bind http server at {} and all higher ports up to {}:{}",
            base,
            base.ip(),
            u16::MAX
        )
    }))
}

fn gateway_bind_candidates(base: SocketAddr) -> impl Iterator<Item = SocketAddr> {
    let ip = base.ip();
    (base.port()..=u16::MAX).map(move |port| SocketAddr::new(ip, port))
}

fn studio_addr_for_child(listen_address: SocketAddr) -> String {
    let ip = match listen_address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("{}:{}", ip, listen_address.port())
}
