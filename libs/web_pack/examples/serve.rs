//! Serves a packed web build's folder on localhost (cross-origin isolated,
//! so a threaded build runs): `serve <dir> [--port N] [--lan]`.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("usage: serve <dir> [--port N] [--lan]");
        std::process::exit(2);
    };
    let port = args.iter().position(|a| a == "--port").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()).unwrap_or(0);
    let lan = args.iter().any(|a| a == "--lan");
    match makepad_web_pack::serve::serve(std::path::Path::new(dir), lan, port) {
        Ok(server) => {
            println!("serving {dir} at {}{}", server.local_url, server.lan_url.map(|u| format!(" and {u}")).unwrap_or_default());
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
