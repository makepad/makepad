//! The standalone Asset Server host.
//!
//! # Why this process exists
//!
//! The asset store is inherently multiplayer. asset-ui, the VJ, the sandbox
//! and headless workers are all clients of ONE catalog, and they come and go
//! independently. When the catalog lives inside one of those apps, that
//! app's lifetime becomes everybody's lifetime: closing the Asset UI window
//! takes the store, the chat broker and the events hub down under every
//! other connected client, which they see as `503 state unavailable`
//! mid-session. This binary breaks that coupling — the server outlives every
//! window.
//!
//! # What it carries
//!
//! [`Host::start`] composes three things that a fleet of clients needs, all
//! of them existing, tested code:
//!
//! 1. [`makepad_asset_store::AssetServer`] — catalog + CAS over the control
//!    and data planes, the chat broker (including client-executed tool
//!    parking for game sessions), the games publish path, the committed
//!    events hub, the job queue and worker/lease protocol, the lease + blob
//!    GC janitor, and the LAN discovery beacon.
//! 2. The **library publisher** (`makepad_asset_importer::watch`) — whatever
//!    the generation pipelines write into the ai-content library becomes
//!    catalog rows. Headless, so it belongs beside the server rather than
//!    inside a UI.
//! 3. The **fleet job coordinator** (`makepad_asset_importer::gen_service`)
//!    — claims queued generation jobs, dispatches them to the asset-ai GPU
//!    boxes the LAN announces, publishes the verified results, and
//!    advertises what the fleet can execute right now on
//!    `GET /v1/job-profiles`. Without it, jobs any client enqueues sit at
//!    "waiting for agent" forever.
//!
//! # What deliberately stays client-side
//!
//! Loops that DERIVE content using resources only a UI process has — the
//! offscreen thumbnail renders (`Cx`, a GPU surface, the splat/mesh
//! viewers), the classic-game import wizards, the stems/lyrics analysis
//! bake — stay in the app and reach the catalog as ordinary clients. Moving
//! them here would mean giving a headless daemon a window.
//!
//! # Single-owner laws
//!
//! - One process per server root (`<root>/server.lock`). Starting this
//!   binary while the Asset UI still hosts the same root fails cleanly on
//!   the lock instead of corrupting anything; stop the other host first, or
//!   let it attach (see the README).
//! - An `AssetClient` cache root is single-owner too, so each loop gets its
//!   own child of the work root.
//! - The job coordinator is at most ONE per process (its stop flag is a
//!   `'static`, borrowed by the service for the thread's whole life). A
//!   second [`Host`] with `jobs` enabled in the same process refuses the
//!   coordinator and says so in [`Host::jobs_error`] rather than starting a
//!   second claimer that would fight the first for leases.

use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig, PublishRights};
use makepad_asset_store::{AssetServer, DiscoveryConfig, ServerConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Default namespace the coordinator advertises and publishes into. Same
/// value the Asset UI's in-process coordinator uses, so a job enqueued
/// against either host is routed identically.
pub const DEFAULT_NAMESPACE: &str = "gen";

/// Everything the host needs. Every knob is a named field: nothing here is
/// read from the environment behind the operator's back (the binary maps
/// flags and two documented env fallbacks onto it).
#[derive(Clone, Debug)]
pub struct HostConfig {
    /// Server root: catalog, CAS, `server.lock`, `server-id`, `listen`,
    /// `admin-token`.
    pub root: PathBuf,
    /// Control plane bind address. The default is `0.0.0.0:0` — an
    /// ephemeral port on every interface, published in `<root>/listen`,
    /// which is exactly what the Asset UI's attach path reads.
    pub control_addr: SocketAddr,
    pub data_addr: SocketAddr,
    /// Announce on the LAN discovery beacon so clients find this server
    /// without configuration.
    pub beacon: bool,
    /// ai-content library to publish continuously. `None` = no publisher.
    pub library: Option<PathBuf>,
    /// Run the fleet job coordinator.
    pub jobs: bool,
    /// Let the coordinator advertise the fleet's executable job profiles.
    /// Ignored when `jobs` is false.
    pub announce: bool,
    /// Explicit GPU-box URL list; `None` = LAN discovery.
    pub fleet_file: Option<PathBuf>,
    /// Namespace for published rows and advertised profiles.
    pub namespace: String,
    /// Parent for the loops' single-owner client caches. Defaults to the
    /// server root's parent, which puts them exactly where the Asset UI's
    /// own hosting mode puts them.
    pub work_root: PathBuf,
    /// Chat: local Qwen fleet node base URLs. Empty = LAN fleet discovery,
    /// same as the Asset UI's embedded broker.
    pub chat_fleet_bases: Vec<String>,
    /// Named fleet the chat broker talks to. Empty = `default`.
    pub chat_fleet: String,
    /// Live chat sessions this server will hold at once, and how many any
    /// one principal may hold. A box that serves several parallel chat
    /// slots wants these raised; the defaults (32 / 8) match the library.
    /// 0 = keep the library default.
    pub chat_max_sessions: usize,
    pub chat_max_sessions_per_owner: usize,
    /// Log to stderr.
    pub log: bool,
}

impl HostConfig {
    /// Deployment defaults: ephemeral planes on every interface, beacon on,
    /// both background loops on, LAN fleet.
    pub fn new(root: PathBuf) -> Self {
        let work_root = root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        Self {
            root,
            control_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            data_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            beacon: true,
            library: None,
            jobs: true,
            announce: true,
            fleet_file: None,
            namespace: DEFAULT_NAMESPACE.to_string(),
            work_root,
            chat_fleet_bases: Vec::new(),
            chat_fleet: String::new(),
            chat_max_sessions: 0,
            chat_max_sessions_per_owner: 0,
            log: true,
        }
    }
}

/// A running host. Dropping it (or calling [`Host::shutdown`]) stops the
/// loops first and the server last, so nothing is still publishing into a
/// catalog that is closing.
pub struct Host {
    // Declaration order IS drop order: both loops are joined while the
    // server they talk to is still answering.
    publish: Option<BackgroundLoop>,
    jobs: Option<BackgroundLoop>,
    server: Option<AssetServer>,
    endpoints: ApiEndpoints,
    server_id: [u8; 16],
    token: String,
    library_error: Option<String>,
    jobs_error: Option<String>,
}

/// One owned background thread plus the flag that stops it.
struct BackgroundLoop {
    stop: Stop,
    join: Option<JoinHandle<()>>,
}

/// A loop's stop flag. The library watcher takes `&AtomicBool` and can own a
/// per-host `Arc`; the generation service borrows a `&'static` for the
/// thread's whole life, so there is exactly one of those per process.
enum Stop {
    Owned(Arc<AtomicBool>),
    Static(&'static AtomicBool),
}

impl Stop {
    fn raise(&self) {
        match self {
            Stop::Owned(flag) => flag.store(true, Ordering::Release),
            Stop::Static(flag) => flag.store(true, Ordering::Release),
        }
    }
}

impl Drop for BackgroundLoop {
    fn drop(&mut self) {
        self.stop.raise();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Stop flag for the one in-process job coordinator; see the module header.
static JOBS_STOP: AtomicBool = AtomicBool::new(false);
/// Set while a coordinator is live, so a second one is refused instead of
/// silently competing for the same leases.
static JOBS_RUNNING: AtomicBool = AtomicBool::new(false);

impl Host {
    /// Bring the server up, then the background loops.
    ///
    /// The SERVER is the only thing whose failure is fatal — a loop that
    /// cannot start is reported ([`Host::library_error`],
    /// [`Host::jobs_error`]) and logged, never a reason to deny every client
    /// its catalog.
    pub fn start(config: &HostConfig) -> Result<Host, String> {
        let mut cfg = ServerConfig::new(config.root.clone());
        cfg.control_addr = config.control_addr;
        cfg.data_addr = config.data_addr;
        // The host mints (or re-mints) the root admin token: its own loops
        // authenticate with it, and an attaching client reads it from the
        // root the same way it does against an Asset-UI-hosted server.
        cfg.bootstrap_admin = true;
        cfg.discovery = config.beacon.then(DiscoveryConfig::lan_default);
        cfg.chat.fleet_bases = config.chat_fleet_bases.clone();
        cfg.chat.fleet = config.chat_fleet.clone();
        if config.chat_max_sessions > 0 {
            cfg.chat.max_sessions = config.chat_max_sessions;
        }
        if config.chat_max_sessions_per_owner > 0 {
            cfg.chat.max_sessions_per_owner = config.chat_max_sessions_per_owner;
        }
        cfg.log = config.log;
        let server = AssetServer::start(cfg).map_err(|error| format!("asset server: {error}"))?;

        let token = std::fs::read_to_string(config.root.join("admin-token"))
            .map_err(|error| format!("admin token: {error}"))?
            .trim()
            .to_string();
        if token.is_empty() {
            return Err("admin token file is empty".to_string());
        }
        let endpoints = localized_endpoints(&server);
        let server_id = server.server_id();

        let (publish, library_error) = match &config.library {
            None => (None, None),
            Some(dir) => match start_publisher(config, dir, endpoints, server_id, &token) {
                Ok(handle) => (Some(handle), None),
                Err(error) => {
                    log(config.log, &format!("library publisher: {error}"));
                    (None, Some(error))
                }
            },
        };
        let (jobs, jobs_error) = if config.jobs {
            match start_coordinator(config, endpoints, server_id, &token) {
                Ok(handle) => (Some(handle), None),
                Err(error) => {
                    log(config.log, &format!("job coordinator: {error}"));
                    (None, Some(error))
                }
            }
        } else {
            (None, None)
        };

        log(
            config.log,
            &format!(
                "asset host up: control {} data {} · publisher {} · coordinator {}",
                endpoints.control,
                endpoints.data,
                publish.as_ref().map_or("off", |_| "on"),
                jobs.as_ref().map_or("off", |_| "on"),
            ),
        );
        Ok(Host {
            publish,
            jobs,
            server: Some(server),
            endpoints,
            server_id,
            token,
            library_error,
            jobs_error,
        })
    }

    /// Loopback-reachable planes (a `0.0.0.0` bind resolved to `127.0.0.1`),
    /// which is what `<root>/listen` advertises too.
    pub fn endpoints(&self) -> ApiEndpoints {
        self.endpoints
    }

    pub fn server_id(&self) -> [u8; 16] {
        self.server_id
    }

    /// The root admin bearer token this host bootstrapped.
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn publisher_running(&self) -> bool {
        self.publish.is_some()
    }

    pub fn coordinator_running(&self) -> bool {
        self.jobs.is_some()
    }

    /// Why the library publisher is not running, when it was asked for.
    pub fn library_error(&self) -> Option<&str> {
        self.library_error.as_deref()
    }

    /// Why the job coordinator is not running, when it was asked for.
    pub fn jobs_error(&self) -> Option<&str> {
        self.jobs_error.as_deref()
    }

    /// Stop the loops, then the server. Idempotent; also runs on drop.
    pub fn shutdown(&mut self) {
        drop(self.publish.take());
        drop(self.jobs.take());
        if let Some(mut server) = self.server.take() {
            server.shutdown();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A server bound to `0.0.0.0` is reached over loopback from this process.
fn localized_endpoints(server: &AssetServer) -> ApiEndpoints {
    let localize = |addr: SocketAddr| {
        if addr.ip().is_unspecified() {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        } else {
            addr
        }
    };
    ApiEndpoints {
        control: localize(server.control_addr()),
        data: localize(server.data_addr()),
    }
}

/// Publish everything the pipelines write into the ai-content library.
///
/// The client connects HERE, on the caller's thread, so "the publisher could
/// not reach the server it is hosted beside" is an answer the operator (and
/// the tests) get immediately instead of a log line scrolling past.
fn start_publisher(
    config: &HostConfig,
    dir: &Path,
    endpoints: ApiEndpoints,
    server_id: [u8; 16],
    token: &str,
) -> Result<BackgroundLoop, String> {
    let mut client_config = ClientConfig::new(config.work_root.join("publish-cache"));
    client_config.token = Some(token.to_string());
    let mut client = AssetClient::connect(client_config, endpoints, Some(server_id))
        .map_err(|error| format!("cannot connect to the local server: {error}"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let dir = dir.to_path_buf();
    let namespace = config.namespace.clone();
    let log_enabled = config.log;
    let join = std::thread::Builder::new()
        .name("asset-host-publish".to_string())
        .spawn(move || {
            log(
                log_enabled,
                &format!("library publisher: watching {}", dir.display()),
            );
            makepad_asset_importer::watch::run(
                &mut client,
                &dir,
                &namespace,
                &PublishRights::generated_cc0(),
                log_enabled,
                &thread_stop,
            );
            log(log_enabled, "library publisher: stopped");
        })
        .map_err(|error| format!("cannot spawn the publisher thread: {error}"))?;
    Ok(BackgroundLoop {
        stop: Stop::Owned(stop),
        join: Some(join),
    })
}

/// Claim queued generation jobs and dispatch them to the GPU fleet.
fn start_coordinator(
    config: &HostConfig,
    endpoints: ApiEndpoints,
    server_id: [u8; 16],
    token: &str,
) -> Result<BackgroundLoop, String> {
    use makepad_asset_importer::gen_service::{FleetSource, GenServiceConfig};
    if JOBS_RUNNING.swap(true, Ordering::AcqRel) {
        return Err("a job coordinator is already running in this process".to_string());
    }
    JOBS_STOP.store(false, Ordering::Release);
    let service = GenServiceConfig {
        servers: vec![endpoints],
        server_id: Some(server_id),
        token: token.to_string(),
        cache_root: config.work_root.join("jobs-cache"),
        namespace: config.namespace.clone(),
        suffix: "asset-host".to_string(),
        rights: PublishRights::generated_cc0(),
        fleet: match &config.fleet_file {
            Some(path) => FleetSource::File(path.clone()),
            None => FleetSource::Lan,
        },
        announce: config.announce,
        log: config.log,
    };
    let log_enabled = config.log;
    let join = std::thread::Builder::new()
        .name("asset-host-jobs".to_string())
        .spawn(move || {
            log(
                log_enabled,
                &format!(
                    "job coordinator: {} -> the GPU fleet",
                    service.servers[0].control
                ),
            );
            makepad_asset_importer::gen_service::run(&service, &JOBS_STOP);
            log(log_enabled, "job coordinator: stopped");
            JOBS_RUNNING.store(false, Ordering::Release);
        });
    match join {
        Ok(join) => Ok(BackgroundLoop {
            stop: Stop::Static(&JOBS_STOP),
            join: Some(join),
        }),
        Err(error) => {
            JOBS_RUNNING.store(false, Ordering::Release);
            Err(format!("cannot spawn the coordinator thread: {error}"))
        }
    }
}

fn log(enabled: bool, message: &str) {
    if enabled {
        eprintln!("[asset-host] {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mp_asset_host_{}_{}_{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// An isolated host: loopback-only ephemeral planes, NO beacon (a test
    /// must never advertise itself to the operator's LAN), no coordinator
    /// (it would claim the real fleet's jobs).
    fn isolated(name: &str) -> HostConfig {
        let base = test_root(name);
        let mut config = HostConfig::new(base.join("server"));
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.beacon = false;
        config.jobs = false;
        config.log = false;
        config.work_root = base.join("work");
        config
    }

    fn cleanup(config: &HostConfig) {
        if let Some(parent) = config.root.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn the_host_serves_a_catalog_and_shuts_down_cleanly() {
        let config = isolated("serves");
        let mut host = Host::start(&config).expect("host starts");
        assert!(!host.token().is_empty(), "the host bootstraps an admin token");
        assert!(
            config.root.join("listen").is_file(),
            "the listen file is how every client attaches"
        );

        // A real client over real sockets, exactly as any other process
        // would reach it.
        let mut client_config = ClientConfig::new(config.work_root.join("probe-cache"));
        client_config.token = Some(host.token().to_string());
        let client =
            AssetClient::connect(client_config, host.endpoints(), Some(host.server_id()))
                .expect("client connects to the standalone host");
        assert_eq!(client.server_id(), host.server_id());

        drop(client);
        host.shutdown();
        host.shutdown();
        cleanup(&config);
    }

    #[test]
    fn a_second_host_on_one_root_is_refused_by_the_root_lock() {
        let config = isolated("locked");
        let host = Host::start(&config).expect("first host starts");
        let error = match Host::start(&config) {
            Ok(_) => panic!("the root lock is single-owner: a second host must be refused"),
            Err(error) => error,
        };
        assert!(
            error.contains("locked by another server process"),
            "the refusal must name the lock, got {error}"
        );
        drop(host);
        cleanup(&config);
    }

    /// The publisher is a CLIENT of the server beside it, and a library that
    /// is not there yet must not cost anyone their catalog: the loop starts,
    /// the server keeps serving, and shutdown still joins cleanly.
    #[test]
    fn a_missing_library_directory_never_costs_the_catalog_its_server() {
        let mut config = isolated("library");
        config.library = Some(config.work_root.join("nonexistent-library"));
        let mut host = Host::start(&config).expect("host starts");
        assert!(
            host.publisher_running(),
            "the publisher connected: {:?}",
            host.library_error()
        );
        assert_eq!(host.library_error(), None);

        let mut client_config = ClientConfig::new(config.work_root.join("probe-cache"));
        client_config.token = Some(host.token().to_string());
        AssetClient::connect(client_config, host.endpoints(), Some(host.server_id()))
            .expect("the server serves while the publisher waits for its library");

        host.shutdown();
        cleanup(&config);
    }

    #[test]
    fn the_work_root_defaults_beside_the_server_root() {
        let config = HostConfig::new(PathBuf::from("/store/local/asset-ui/asset-server"));
        assert_eq!(config.work_root, PathBuf::from("/store/local/asset-ui"));
        assert!(config.jobs, "a fleet host coordinates jobs by default");
        assert!(config.beacon, "a fleet host is discoverable by default");
        assert_eq!(config.namespace, DEFAULT_NAMESPACE);
        assert_eq!(
            config.control_addr.port(),
            0,
            "ephemeral by default: the listen file is the address of record"
        );
    }
}
