//! The headless Flow host a shell without a studio of its own runs recipes
//! on: an embedded flow-server under the hosting app's own folder, its
//! generators routed by the app's [`GenRouter`], its `Publish` nodes writing
//! into the app's [`ResultStore`], and the recipe checks ([`crate::nodes`])
//! registered. All calls block; they belong on a worker, never the UI thread.

use makepad_ai_hub::job::GenRouter;
use makepad_flow::client::{Endpoints, FlowClient};
use makepad_flow::engine::results::ResultStore;
use makepad_flow::engine::{HubChat, HubHttp, Seams};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{CreateInstanceRequest, RunRowDto};
use std::path::PathBuf;
use std::sync::Arc;

pub struct GenHostConfig {
    /// The hosting app's folder for the host (values, runs). Never a
    /// machine-wide root.
    pub root: PathBuf,
    /// Where results publish and fixed-seed runs are found again: a
    /// [`makepad_flow::engine::results::FolderStore`] or the app's library.
    pub results: Arc<dyn ResultStore>,
    /// The namespace results publish under (the store's own, for reuse).
    pub namespace: String,
    pub router: Arc<dyn GenRouter>,
}

/// One embedded flow-server, owned: dropping the host stops it.
pub struct GenHost {
    client: FlowClient,
    _server: FlowServer,
}

impl GenHost {
    pub fn start(config: GenHostConfig) -> Result<GenHost, String> {
        let mut registry = makepad_flow::document::Registry::with_builtins();
        let mut executors = makepad_flow::engine::providers::ExecutorRegistry::default();
        crate::nodes::register(&mut registry, &mut executors)?;
        let mut server = FlowServerConfig::new(config.root.clone()).with_seams(Seams {
            chat: Arc::new(HubChat::from_env()),
            gen: config.router,
            http: Arc::new(HubHttp),
        });
        server.registry = Arc::new(registry);
        server.executors = Arc::new(executors);
        server.results = Some(config.results);
        let server = FlowServer::start(server).map_err(|error| error.to_string())?;
        let endpoints = server.endpoints();
        let client = FlowClient::connect(Endpoints { control: endpoints.control, data: endpoints.data }, endpoints.token, Some(endpoints.server_id))
            .map_err(|error| error.to_string())?;
        Ok(GenHost { client, _server: server })
    }

    pub fn client(&self) -> &FlowClient {
        &self.client
    }

    /// Every invocation gets its own instance; the recipe definition stays
    /// reusable and inputs never leak into another invocation.
    pub fn submit(&self, flow: &str, inputs: CreateInstanceRequest) -> Result<GenRunHandle, String> {
        let instance = self.client.create_instance(flow, &inputs).map_err(|e| e.to_string())?.instance;
        match self.client.start_run(&instance, None) {
            Ok(run) => Ok(GenRunHandle { instance, run_id: run.run_id }),
            Err(error) => {
                let _ = self.client.delete_instance(&instance);
                Err(error.to_string())
            }
        }
    }

    pub fn snapshot(&self, run: &GenRunHandle) -> Result<RunRowDto, String> {
        self.client.run(&run.run_id).map_err(|e| e.to_string())
    }

    pub fn cancel(&self, run: &GenRunHandle) -> Result<(), String> {
        self.client.cancel_run(&run.run_id).map_err(|e| e.to_string())
    }
}

/// A submitted recipe run.
#[derive(Clone, Debug)]
pub struct GenRunHandle {
    pub instance: String,
    pub run_id: String,
}
