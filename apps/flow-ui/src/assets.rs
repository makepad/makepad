//! Host discovery and startup run on the task pool, away from UI input.
use crate::testpattern;
use makepad_app_asset_server::embed as asset_embed;
use makepad_asset_client::ApiEndpoints;
use makepad_flow::client::{SessionConfig, SessionConnector};
use makepad_flow::embed::{default_root, resolve, EmbedPolicy, Resolved};
use makepad_flow::engine::{FixedGen, HubChat, HubHttp, Seams};
use makepad_flow::host::{FlowServer, FlowServerConfig};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

pub struct Bootstrap {
    pub host: Option<FlowServer>,
    pub testpattern: Option<testpattern::TestpatternService>,
    pub session: SessionConnector,
    // The store is dropped after everything that can talk to it.
    pub store: Option<asset_embed::LocalStore>,
}

fn endpoints(text: &str) -> Option<ApiEndpoints> {
    let mut parts = text.trim().rsplitn(3, ':');
    let data = parts.next()?.parse().ok()?;
    let control = parts.next()?.parse().ok()?;
    let ip: IpAddr = parts.next()?.trim_matches(['[', ']']).parse().ok()?;
    Some(ApiEndpoints { control: SocketAddr::new(ip, control), data: SocketAddr::new(ip, data) })
}

/// Keep an on-disk listen hint only when it currently speaks Asset Server.
/// The file survives restarts while both ports are ephemeral, so treating a
/// syntactically valid but stale hint as authoritative can strand the flow
/// worker even though discovery can find the new server.
fn health_answers(addr: SocketAddr) -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    let request = format!(
        "GET /v1/health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        addr
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = [0u8; 32];
    let mut received = 0usize;
    while received < 16 {
        match stream.read(&mut response[received..]) {
            Ok(0) | Err(_) => break,
            Ok(count) => received += count,
        }
    }
    response[..received].starts_with(b"HTTP/1.1 200")
}

fn read_token(path: std::path::PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn start() -> Result<Bootstrap, String> {
    let root = default_root();
    let mut host = None;
    let mut store = None;
    let mut testpattern = None;
    let (hint, token) = match resolve(EmbedPolicy::from_env(), &root, None) {
        Resolved::Attach(hint, token, _) => (hint, token),
        Resolved::Host => {
            let mut config = FlowServerConfig::new(root.clone());
            config.asset.archive_outputs = true;
            let asset_root = asset_embed::default_store_root("FLOW", "flow-assets");
            let pinned = std::env::var("FLOW_ASSET_SERVER").ok().filter(|s| !s.trim().is_empty());
            let hinted = if let Some(text) = &pinned {
                Some(endpoints(text).ok_or("FLOW_ASSET_SERVER must be ip:control_port:data_port")?)
            } else {
                std::fs::read_to_string(asset_root.join("listen")).ok().and_then(|s| endpoints(&s))
            };
            let resolved = asset_embed::resolve("FLOW", "flow-assets", pinned.is_some(), hinted);
            eprintln!("[flow-ui] assets: {}", resolved.note);
            if let Some(local) = resolved.local {
                config.asset.endpoints = Some(local.endpoints());
                config.asset.server_id = Some(local.server_id());
                config.asset.token = Some(local.token().to_string());
                store = Some(local);
            } else {
                // An explicitly pinned server is authoritative and must use
                // the explicitly supplied credential. For automatic attach,
                // retain the listen hint only after a live health check;
                // otherwise let the asset worker discover the fresh server
                // instead of retrying a stale ephemeral port forever.
                config.asset.endpoints = if pinned.is_some() {
                    hinted
                } else {
                    hinted.filter(|value| health_answers(value.control))
                };
                config.asset.token = if pinned.is_some() {
                    std::env::var("FLOW_ASSET_TOKEN").ok()
                } else {
                    std::env::var("FLOW_ASSET_TOKEN")
                        .ok()
                        .or_else(|| read_token(asset_root.join("admin-token")))
                }
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
                config.asset.discovery_wait_ms = 3_000;
            }
            if let Some(value) = std::env::var("FLOW_GEN_BASE_URL").ok().filter(|s| !s.is_empty()) {
                let seams = if value == "testpattern" {
                    let service = testpattern::start_service()?;
                    let url = service.url.clone();
                    testpattern = Some(service);
                    Seams { chat: Arc::new(testpattern::TestpatternChat), gen: Arc::new(FixedGen(url)), http: Arc::new(HubHttp) }
                } else {
                    Seams { chat: Arc::new(HubChat::from_env()), gen: Arc::new(FixedGen(value)), http: Arc::new(HubHttp) }
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
    Ok(Bootstrap { host, testpattern, session, store })
}
