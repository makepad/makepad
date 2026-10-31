//! Host discovery and startup run on the task pool, away from UI input.
use crate::testpattern;
use makepad_flow::client::{SessionConfig, SessionConnector};
use makepad_flow::embed::{default_root, resolve, EmbedPolicy, Resolved};
use makepad_ai_hub::job::FixedRouter;
use makepad_flow::engine::results::FolderStore;
use makepad_flow::engine::{HubChat, HubHttp, Seams};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::runtime::LiveOutputs;
use std::sync::Arc;

pub struct Bootstrap {
    pub host: Option<FlowServer>,
    pub testpattern: Option<testpattern::TestpatternService>,
    pub session: SessionConnector,
}

/// `media_max_bytes` is the media admission budget the embedded server
/// runs with, scaled by the caller to the machine.
pub fn start(media_max_bytes: usize) -> Result<Bootstrap, String> {
    let root = default_root();
    let mut host = None;
    let mut testpattern = None;
    let (hint, token) = match resolve(EmbedPolicy::from_env(), &root, None) {
        Resolved::Attach(hint, token, _) => (hint, token),
        Resolved::Host => {
            let mut config = FlowServerConfig::new(root.clone());
            (config.registry, config.executors) = crate::registries()?;
            // The workbench shows what the instance is now: a run that
            // outlived an edit keeps its result in its run row and never
            // repaints the live instance.
            config.live_outputs = LiveOutputs::CurrentRevision;
            config.media_max_bytes = media_max_bytes;
            // Results, and every generated value and terminal output, as
            // files in the results folder.
            config.results = Some(Arc::new(FolderStore::open(root.join("results"), "flows")?.with_archive_outputs(true)));
            if let Some(value) = std::env::var("FLOW_GEN_BASE_URL").ok().filter(|s| !s.is_empty()) {
                let seams = if value == "testpattern" {
                    let service = testpattern::start_service()?;
                    let url = service.url.clone();
                    testpattern = Some(service);
                    Seams { chat: Arc::new(testpattern::TestpatternChat), gen: Arc::new(FixedRouter(url)), http: Arc::new(HubHttp) }
                } else {
                    Seams { chat: Arc::new(HubChat::from_env()), gen: Arc::new(FixedRouter(value)), http: Arc::new(HubHttp) }
                };
                config = config.with_seams(seams);
            }
            let server = FlowServer::start(config).map_err(|e| format!("Could not host flow server: {e}"))?;
            let served = server.endpoints();
            let hint = Some(makepad_flow::client::Endpoints { control: served.control, data: served.data });
            let token = Some(served.token.clone());
            host = Some(server);
            (hint, token)
        }
    };
    let session = SessionConnector::start(SessionConfig { hint, root: Some(root), token, ..SessionConfig::default() });
    Ok(Bootstrap { host, testpattern, session })
}
