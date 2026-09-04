use makepad_flow::embed::default_root;
use makepad_flow::host::{FlowServer, FlowServerConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const USAGE: &str = "\
makepad-flow-server [options]

Options:
  --root <dir>          Server root (default: ~/.makepad/flow)
  --bind <ip>           Bind IP (default: 127.0.0.1)
  --control-port <port> Control port (default: 0, ephemeral)
  --data-port <port>    Data port (default: 0, ephemeral)
  --help                Show this help
";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_signal: i32) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn signal(signum: i32, handler: usize) -> usize;
        }
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        unsafe {
            signal(SIGINT, on_signal as *const () as usize);
            signal(SIGTERM, on_signal as *const () as usize);
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("makepad-flow-server: {message}");
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn value(name: &str, args: &mut impl Iterator<Item = String>) -> String {
    args.next().unwrap_or_else(|| fail(&format!("{name} needs a value")))
}

fn parse_config() -> FlowServerConfig {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mut control_port = 0u16;
    let mut data_port = 0u16;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(value("--root", &mut args))),
            "--bind" => {
                bind = value("--bind", &mut args)
                    .parse()
                    .unwrap_or_else(|_| fail("--bind must be an IP address"));
            }
            "--control-port" => {
                control_port = value("--control-port", &mut args)
                    .parse()
                    .unwrap_or_else(|_| fail("--control-port must be 0..65535"));
            }
            "--data-port" => {
                data_port = value("--data-port", &mut args)
                    .parse()
                    .unwrap_or_else(|_| fail("--data-port must be 0..65535"));
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => fail(&format!("unknown option {other}")),
        }
    }
    let mut config = FlowServerConfig::new(root.unwrap_or_else(default_root));
    config.asset.token = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/asset-ui/asset-server/admin-token"),
    )
    .ok()
    .map(|token| token.trim().to_string())
    .filter(|token| !token.is_empty());
    config.control_addr = SocketAddr::new(bind, control_port).to_string();
    config.data_addr = SocketAddr::new(bind, data_port).to_string();
    config
}

fn main() {
    let config = parse_config();
    let root = config.root.clone();
    install_signal_handlers();
    let server = match FlowServer::start(config) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("makepad-flow-server: failed to start: {error}");
            std::process::exit(1);
        }
    };
    let endpoints = server.endpoints();
    println!(
        "[flow-server] listening control={} data={} root={}",
        endpoints.control,
        endpoints.data,
        root.display()
    );
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    server.shutdown();
}
