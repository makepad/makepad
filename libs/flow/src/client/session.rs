use super::{parse_hex16, ClientError, Endpoints, FlowClient};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub hint: Option<Endpoints>,
    pub root: Option<PathBuf>,
    pub token: Option<String>,
    pub retry_min_ms: u64,
    pub retry_max_ms: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            hint: None,
            root: None,
            token: None,
            retry_min_ms: 1_000,
            retry_max_ms: 10_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Discovering,
    Connecting {
        endpoints: Endpoints,
    },
    Connected {
        server_id: [u8; 16],
        endpoints: Endpoints,
    },
    Retrying {
        error: ClientError,
        in_secs: u64,
    },
}

struct Shared {
    status: Mutex<SessionStatus>,
    client: Mutex<Option<Arc<Mutex<FlowClient>>>>,
    stopping: AtomicBool,
    wake: (Mutex<()>, Condvar),
}

/// Native connection/discovery worker with an active liveness watch.
pub struct SessionConnector {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl SessionConnector {
    pub fn start(mut config: SessionConfig) -> SessionConnector {
        if config.retry_min_ms == 0 {
            config.retry_min_ms = 1_000;
        }
        if config.retry_max_ms < config.retry_min_ms {
            config.retry_max_ms = config.retry_min_ms;
        }
        let shared = Arc::new(Shared {
            status: Mutex::new(SessionStatus::Discovering),
            client: Mutex::new(None),
            stopping: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
        });
        let worker_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name("flow-session-connect".into())
            .spawn(move || worker(config, worker_shared))
            .ok();
        if join.is_none() {
            set_status(
                &shared,
                SessionStatus::Retrying {
                    error: ClientError::Io {
                        op: "spawn session connector",
                        kind: std::io::ErrorKind::Other,
                    },
                    in_secs: 1,
                },
            );
        }
        SessionConnector { shared, join }
    }

    pub fn status(&self) -> SessionStatus {
        self.shared
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or(SessionStatus::Retrying {
                error: ClientError::Protocol("session status lock poisoned".into()),
                in_secs: 1,
            })
    }

    pub fn client(&self) -> Option<Arc<Mutex<FlowClient>>> {
        self.shared
            .client
            .lock()
            .ok()
            .and_then(|client| client.clone())
    }

    pub fn stop(&mut self) {
        self.request_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    fn request_stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.wake.1.notify_all();
    }
}

impl Drop for SessionConnector {
    fn drop(&mut self) {
        // A connector normally gets an explicit `stop` during app shutdown.
        // Drop remains non-blocking in case it occurs on a UI event thread.
        self.request_stop();
        self.join.take();
    }
}

fn worker(config: SessionConfig, shared: Arc<Shared>) {
    let mut backoff_ms = config.retry_min_ms;
    while !shared.stopping.load(Ordering::Acquire) {
        set_status(&shared, SessionStatus::Discovering);
        let root_files = config.root.as_deref().map(read_root_files).unwrap_or_default();
        let endpoints = match config.hint.or(root_files.endpoints) {
            Some(endpoints) => endpoints,
            None => {
                let error = ClientError::Io {
                    op: "discover flow server",
                    kind: std::io::ErrorKind::NotFound,
                };
                if !retry(&shared, &mut backoff_ms, &config, error) {
                    return;
                }
                continue;
            }
        };
        set_status(&shared, SessionStatus::Connecting { endpoints });
        let token = config
            .token
            .clone()
            .or(root_files.token)
            .unwrap_or_default();
        match FlowClient::connect(endpoints, token, root_files.server_id) {
            Ok(client) => {
                backoff_ms = config.retry_min_ms;
                let server_id = client.server_id();
                let handle = Arc::new(Mutex::new(client));
                if let Ok(mut slot) = shared.client.lock() {
                    *slot = Some(handle.clone());
                }
                set_status(
                    &shared,
                    SessionStatus::Connected {
                        server_id,
                        endpoints,
                    },
                );

                let mut lost_samples = 0u8;
                loop {
                    if !wait_interruptible(&shared, 500) {
                        return;
                    }
                    let health = handle
                        .lock()
                        .map_err(|_| ClientError::Protocol("flow client lock poisoned".into()))
                        .and_then(|client| client.health());
                    match health {
                        Ok(health) if health.server_id == server_id => lost_samples = 0,
                        Ok(_) => {
                            // An answering endpoint changing identity is not
                            // a transport loss, but continuing to hand out
                            // the old verified handle would violate the pin.
                            if let Ok(mut slot) = shared.client.lock() {
                                *slot = None;
                            }
                            set_status(
                                &shared,
                                SessionStatus::Retrying {
                                    error: ClientError::ServerIdentityMismatch,
                                    in_secs: config.retry_min_ms.div_ceil(1_000),
                                },
                            );
                            if !wait_interruptible(&shared, config.retry_min_ms) {
                                return;
                            }
                            break;
                        }
                        Err(error) if error.is_connection_loss() => {
                            lost_samples = lost_samples.saturating_add(1);
                            if lost_samples >= 3 {
                                if let Ok(mut slot) = shared.client.lock() {
                                    *slot = None;
                                }
                                set_status(&shared, SessionStatus::Discovering);
                                break;
                            }
                        }
                        Err(_) => lost_samples = 0,
                    }
                }
            }
            Err(error) => {
                if !retry(&shared, &mut backoff_ms, &config, error) {
                    return;
                }
            }
        }
    }
}

fn retry(
    shared: &Shared,
    backoff_ms: &mut u64,
    config: &SessionConfig,
    error: ClientError,
) -> bool {
    let wait_ms = (*backoff_ms).min(config.retry_max_ms);
    set_status(
        shared,
        SessionStatus::Retrying {
            error,
            in_secs: wait_ms.div_ceil(1_000),
        },
    );
    if !wait_interruptible(shared, wait_ms) {
        return false;
    }
    *backoff_ms = backoff_ms
        .saturating_mul(2)
        .min(config.retry_max_ms);
    true
}

fn set_status(shared: &Shared, status: SessionStatus) {
    if let Ok(mut current) = shared.status.lock() {
        *current = status;
    }
}

fn wait_interruptible(shared: &Shared, milliseconds: u64) -> bool {
    if shared.stopping.load(Ordering::Acquire) {
        return false;
    }
    let Ok(guard) = shared.wake.0.lock() else {
        return false;
    };
    let _ = shared
        .wake
        .1
        .wait_timeout(guard, Duration::from_millis(milliseconds));
    !shared.stopping.load(Ordering::Acquire)
}

#[derive(Default)]
pub(crate) struct RootFiles {
    pub endpoints: Option<Endpoints>,
    pub token: Option<String>,
    pub server_id: Option<[u8; 16]>,
}

pub(crate) fn read_root_files(root: &Path) -> RootFiles {
    RootFiles {
        endpoints: std::fs::read_to_string(root.join("listen"))
            .ok()
            .and_then(|text| parse_listen(text.lines().next().unwrap_or_default())),
        token: std::fs::read_to_string(root.join("token"))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        server_id: std::fs::read_to_string(root.join("server-id"))
            .ok()
            .and_then(|text| parse_hex16(text.trim())),
    }
}

pub fn parse_listen(spec: &str) -> Option<Endpoints> {
    let mut parts = spec.trim().rsplitn(3, ':');
    let data = parts.next()?.parse::<u16>().ok()?;
    let control = parts.next()?.parse::<u16>().ok()?;
    if data == 0 || control == 0 {
        return None;
    }
    let ip_text = parts.next()?;
    let ip_text = ip_text
        .strip_prefix('[')
        .and_then(|text| text.strip_suffix(']'))
        .unwrap_or(ip_text);
    let ip = ip_text.parse::<IpAddr>().ok()?;
    Some(Endpoints {
        control: SocketAddr::new(ip, control),
        data: SocketAddr::new(ip, data),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_file_parses_ipv4_and_ipv6() {
        assert_eq!(
            parse_listen("127.0.0.1:4000:4001").unwrap(),
            Endpoints {
                control: "127.0.0.1:4000".parse().unwrap(),
                data: "127.0.0.1:4001".parse().unwrap(),
            }
        );
        assert_eq!(
            parse_listen("[::1]:4000:4001").unwrap().control,
            "[::1]:4000".parse().unwrap()
        );
    }
}
