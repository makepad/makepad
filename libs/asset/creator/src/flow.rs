//! Creator execution through flow-server instances. Embedded and remote
//! servers use the same authenticated HTTP protocol. All calls are blocking
//! and belong on an app worker, never on the UI thread.

use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::{CreateInstanceRequest, RunRowDto, RunState, ValueBytes};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Owns only the server it starts. Dropping a remote attachment cannot stop
/// another app's server. Keep this session alive for the creator's lifetime.
pub struct CreatorFlow {
    client: FlowClient,
    #[cfg(feature = "flow-host")]
    host: Option<makepad_flow::host::FlowServer>,
}

impl CreatorFlow {
    /// Share the embedded host across simultaneous creator jobs. This is
    /// called on workers; no app/UI thread waits for the initialization lock.
    pub fn shared() -> Result<std::sync::Arc<Self>, String> {
        use std::sync::{Arc, Mutex, OnceLock, Weak};
        static SESSION: OnceLock<Mutex<Weak<CreatorFlow>>> = OnceLock::new();
        let mut weak = SESSION
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .map_err(|_| "creator Flow initialization failed")?;
        if let Some(session) = weak.upgrade() {
            return Ok(session);
        }
        let session = Arc::new(Self::configured()?);
        *weak = Arc::downgrade(&session);
        Ok(session)
    }

    /// Explicit remote endpoints always win over local hosting. A client-only
    /// build attaches to the root unless remote endpoints were supplied.
    pub fn configured() -> Result<Self, String> {
        if let Ok(control) = std::env::var("CREATOR_FLOW_CONTROL") {
            let data = std::env::var("CREATOR_FLOW_DATA")
                .map_err(|_| "CREATOR_FLOW_DATA is required with CREATOR_FLOW_CONTROL")?;
            let token = std::env::var("CREATOR_FLOW_TOKEN")
                .map_err(|_| "CREATOR_FLOW_TOKEN is required with remote endpoints")?;
            return Self::connect(
                Endpoints {
                    control: control
                        .parse()
                        .map_err(|_| "invalid CREATOR_FLOW_CONTROL socket address")?,
                    data: data
                        .parse()
                        .map_err(|_| "invalid CREATOR_FLOW_DATA socket address")?,
                },
                token,
                None,
            );
        }
        let root = std::env::var_os("CREATOR_FLOW_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(makepad_flow::embed::default_root);
        if std::env::var("CREATOR_FLOW_EMBED")
            .is_ok_and(|v| ["never", "off", "0"].contains(&v.as_str()))
        {
            return Self::attach(root);
        }
        #[cfg(feature = "flow-host")]
        {
            Self::open(makepad_flow::host::FlowServerConfig::new(root))
        }
        #[cfg(not(feature = "flow-host"))]
        {
            Self::attach(root)
        }
    }
    pub fn connect(
        endpoints: Endpoints,
        token: String,
        server_id: Option<[u8; 16]>,
    ) -> Result<Self, String> {
        let client = FlowClient::connect(endpoints, token, server_id).map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            #[cfg(feature = "flow-host")]
            host: None,
        })
    }

    pub fn attach(root: impl AsRef<Path>) -> Result<Self, String> {
        let client = FlowClient::connect_root(root).map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            #[cfg(feature = "flow-host")]
            host: None,
        })
    }

    #[cfg(feature = "flow-host")]
    pub fn start(config: makepad_flow::host::FlowServerConfig) -> Result<Self, String> {
        let host = makepad_flow::host::FlowServer::start(config).map_err(|e| e.to_string())?;
        let e = host.endpoints();
        let client = FlowClient::connect(
            Endpoints {
                control: e.control,
                data: e.data,
            },
            e.token,
            Some(e.server_id),
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            host: Some(host),
        })
    }

    /// Attach when another process owns the root. A startup race resolves
    /// by attaching to the winning host; errors never trigger a second root.
    #[cfg(feature = "flow-host")]
    pub fn open(config: makepad_flow::host::FlowServerConfig) -> Result<Self, String> {
        let root = config.root.clone();
        if let Ok(client) = Self::attach(&root) {
            return Ok(client);
        }
        match makepad_flow::host::FlowServer::start(config) {
            Ok(host) => {
                let e = host.endpoints();
                let client = FlowClient::connect(
                    Endpoints {
                        control: e.control,
                        data: e.data,
                    },
                    e.token,
                    Some(e.server_id),
                )
                .map_err(|e| e.to_string())?;
                Ok(Self {
                    client,
                    host: Some(host),
                })
            }
            Err(makepad_flow::host::ServerError::Locked) => {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    match Self::attach(&root) {
                        Ok(client) => return Ok(client),
                        Err(error) if std::time::Instant::now() >= deadline => return Err(error),
                        Err(_) => std::thread::sleep(Duration::from_millis(25)),
                    }
                }
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn client(&self) -> &FlowClient {
        &self.client
    }

    /// Every invocation gets a distinct instance; graph definitions stay
    /// reusable and inputs never leak into another creator's invocation.
    pub fn submit(&self, flow: &str, inputs: CreateInstanceRequest) -> Result<CreatorRun, String> {
        let instance = self
            .client
            .create_instance(flow, &inputs)
            .map_err(|e| e.to_string())?
            .instance;
        let run = match self.client.start_run(&instance, None) {
            Ok(run) => run,
            Err(error) => {
                let _ = self.client.delete_instance(&instance);
                return Err(error.to_string());
            }
        };
        Ok(CreatorRun {
            instance,
            run_id: run.run_id,
        })
    }

    pub fn snapshot(&self, run: &CreatorRun) -> Result<RunRowDto, String> {
        self.client.run(&run.run_id).map_err(|e| e.to_string())
    }

    pub fn cancel(&self, run: &CreatorRun) -> Result<(), String> {
        self.client
            .cancel_run(&run.run_id)
            .map_err(|e| e.to_string())
    }

    /// Observe server-owned scheduling. On cancellation or a local failure,
    /// release the invocation we own, preserving already completed results.
    pub fn wait(
        &self,
        run: &CreatorRun,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(&RunRowDto),
        poll_interval: Duration,
    ) -> Result<HashMap<String, ValueBytes>, String> {
        let result = (|| loop {
            if cancelled() {
                return Err("creator flow cancelled".to_string());
            }
            let row = self.snapshot(run)?;
            progress(&row);
            match row.state {
                RunState::Done => {
                    let mut outputs = HashMap::new();
                    for (name, value) in row.outputs {
                        let bytes = self
                            .client
                            .value(&value.digest)
                            .map_err(|e| e.to_string())?;
                        outputs.insert(name, bytes);
                    }
                    return Ok(outputs);
                }
                RunState::Failed => {
                    return Err(row
                        .nodes
                        .values()
                        .find_map(|n| n.error.clone())
                        .unwrap_or_else(|| "creator flow failed".into()))
                }
                RunState::Cancelled => return Err("creator flow cancelled".into()),
                RunState::Waiting => return Err("creator flow requires an answer".into()),
                _ => std::thread::sleep(poll_interval.max(Duration::from_millis(10))),
            }
        })();
        if result.is_err() {
            let _ = self.cancel(run);
        }
        result
    }

    pub fn is_embedded(&self) -> bool {
        #[cfg(feature = "flow-host")]
        {
            self.host.is_some()
        }
        #[cfg(not(feature = "flow-host"))]
        {
            false
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreatorRun {
    pub instance: String,
    pub run_id: String,
}
