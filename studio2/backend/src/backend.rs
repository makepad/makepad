use crate::dispatch::{StudioCore, StudioEvent};
use crate::gateway::{start_http_gateway, GatewayHandle};
use crate::protocol::{ClientId, QueryId, StudioToUI, UIToStudio, UIToStudioEnvelope};
use crate::virtual_fs::VirtualFs;
use makepad_micro_serde::{DeBin, SerBin};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use std::thread::JoinHandle;

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
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            listen_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8001),
            post_max_size: 1024 * 1024,
            mounts: Vec::new(),
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
    recv_raw: Receiver<Vec<u8>>,
    next_counter: u64,
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
        let _ = self.event_tx.send(StudioEvent::UiBinary {
            web_socket_id: self.web_socket_id,
            data: envelope.serialize_bin(),
        });
        query_id
    }

    pub fn cancel_query(&mut self, query_id: QueryId) {
        let _ = self.send(UIToStudio::CancelQuery { query_id });
    }

    pub fn try_recv(&self) -> Option<StudioToUI> {
        let data = self.recv_raw.try_recv().ok()?;
        StudioToUI::deserialize_bin(&data).ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<StudioToUI> {
        let data = self.recv_raw.recv_timeout(timeout).ok()?;
        StudioToUI::deserialize_bin(&data).ok()
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

        let mut core = StudioCore::new(event_rx, event_tx.clone(), vfs);
        let core_thread = std::thread::spawn(move || {
            core.run();
        });

        let web_socket_id = 1u64;
        let (raw_tx, raw_rx) = mpsc::channel::<Vec<u8>>();
        event_tx
            .send(StudioEvent::UiConnected {
                web_socket_id,
                sender: raw_tx,
            })
            .map_err(|err| format!("failed to connect in-process ui client: {}", err))?;

        let hello = raw_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|err| format!("backend did not send hello: {}", err))?;
        let hello = StudioToUI::deserialize_bin(&hello).map_err(|err| err.msg)?;
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
            recv_raw: raw_rx,
            next_counter: 0,
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

        let mut core = StudioCore::new(event_rx, event_tx.clone(), vfs);
        let core_thread = std::thread::spawn(move || {
            core.run();
        });

        let gateway = start_http_gateway(
            config.listen_address,
            config.post_max_size,
            event_tx.clone(),
        )?;

        Ok(BackendHandle {
            listen_address: config.listen_address,
            event_tx,
            _gateway: gateway,
            _core_thread: core_thread,
        })
    }
}
