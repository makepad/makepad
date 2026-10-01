use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

mod client;
mod procs;
mod server;

use makepad_network::tunnel::*;

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  Server: makepad-remote --server [--port 8384] [--bind lan|<ip>] [--all]");
    eprintln!("                         [--keys <file>] [--identity <dir>] [--audit-log <file>] [--max-run <secs>]");
    eprintln!("          makepad-remote --init [--identity <dir>]     create the TLS identity, print its pin");
    eprintln!("  Client: makepad-remote <host[:port]> cargo [args...]");
    eprintln!("          makepad-remote <host[:port]> shell <command...>  (requires --all)");
    eprintln!("          makepad-remote <host[:port]> spawn <command...>  (hidden, survives disconnect)");
    eprintln!("          makepad-remote <host[:port]> ps [filter]");
    eprintln!("          makepad-remote <host[:port]> kill [--tree] <pid>");
    eprintln!("          makepad-remote <host[:port]> pull <remote-rel-path> <local-path>");
    eprintln!("          makepad-remote <host[:port]> push <local-path> <remote-rel-path>  (requires --all)");
    eprintln!("          makepad-remote <host[:port]> admin <{}>", server::ADMIN_ACTIONS.join("|"));
    eprintln!("  Keys:   makepad-remote keygen <host>            new key (client file + server file in outbox)");
    eprintln!("          makepad-remote pin <host> <sha256-hex>  record the server's certificate pin");
    eprintln!("          makepad-remote rotate <host[:port]>     replace the key over the tunnel");
    eprintln!("          makepad-remote --probe <host[:port]> --keys <file> --identity <dir>");
    eprintln!("  Files:  {} (MAKEPAD_TUNNEL_DIR): psk, pins; server: server-keys, identity/, audit.log", tunnel_dir().display());
}

fn exit_with(result: std::io::Result<i32>) -> ExitCode {
    match result {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1).max(1)),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return ExitCode::from(1);
    }

    // Server-side and key commands take --flag value options.
    let opt = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };

    match args[0].as_str() {
        "--server" => {
            let mut opts = server::ServerOptions::defaults();
            let mut i = 1;
            while i < args.len() {
                let value = args.get(i + 1).cloned();
                match (args[i].as_str(), value) {
                    ("--all", _) => {
                        opts.allow_all = true;
                        i += 1;
                        continue;
                    }
                    ("--port", Some(v)) => match v.parse() {
                        Ok(p) => opts.port = p,
                        Err(_) => {
                            eprintln!("invalid port: {v}");
                            return ExitCode::from(1);
                        }
                    },
                    ("--bind", Some(v)) => opts.bind = v,
                    ("--keys", Some(v)) => opts.keys = PathBuf::from(v),
                    ("--identity", Some(v)) => opts.identity = PathBuf::from(v),
                    ("--audit-log", Some(v)) => opts.audit_log = PathBuf::from(v),
                    ("--max-run", Some(v)) => match v.parse() {
                        Ok(s) => opts.max_run = Duration::from_secs(s),
                        Err(_) => {
                            eprintln!("invalid --max-run: {v}");
                            return ExitCode::from(1);
                        }
                    },
                    (other, _) => {
                        eprintln!("unknown or incomplete server option: {other}");
                        print_usage();
                        return ExitCode::from(1);
                    }
                }
                i += 2;
            }
            exit_with(server::run_server(opts).map(|_| 0))
        }
        "--init" => {
            let dir = opt("--identity").map(PathBuf::from).unwrap_or_else(|| tunnel_dir().join("identity"));
            exit_with(server::init_identity(&dir).map(|id| {
                println!("{}", id.fingerprint_hex());
                0
            }))
        }
        "--probe" => {
            let (Some(addr), Some(keys), Some(identity)) = (args.get(1), opt("--keys"), opt("--identity")) else {
                print_usage();
                return ExitCode::from(1);
            };
            exit_with(client::probe(addr, &PathBuf::from(keys), &PathBuf::from(identity)).map(|_| 0))
        }
        "keygen" if args.len() == 2 => exit_with(client::keygen(&args[1]).map(|_| 0)),
        "pin" if args.len() == 3 => exit_with(client::set_pin(&args[1], &args[2]).map(|_| 0)),
        "rotate" if args.len() == 2 => exit_with(client::rotate(&args[1]).map(|_| 0)),
        addr if args.len() >= 2 => {
            let rest = &args[2..];
            let result = match args[1].as_str() {
                "cargo" => client::run_client(addr, rest, false),
                "shell" => client::run_client(addr, rest, true),
                "spawn" if !rest.is_empty() => client::run_simple(addr, TAG_SPAWN, rest.join(" ").as_bytes()),
                "ps" => client::run_simple(addr, TAG_PS, rest.first().map(String::as_str).unwrap_or("").as_bytes()),
                "kill" => {
                    let tree = rest.iter().any(|a| a == "--tree");
                    let Some(pid) = rest.iter().find(|a| *a != "--tree") else {
                        print_usage();
                        return ExitCode::from(1);
                    };
                    let payload = if tree { format!("{pid}\ntree") } else { pid.clone() };
                    client::run_simple(addr, TAG_KILL, payload.as_bytes())
                }
                "pull" if rest.len() == 2 => client::pull(addr, &rest[0], &rest[1]),
                "push" if rest.len() == 2 => client::push(addr, &rest[0], &rest[1]),
                "admin" if rest.len() == 1 => client::run_simple(addr, TAG_ADMIN, rest[0].as_bytes()),
                _ => {
                    print_usage();
                    return ExitCode::from(1);
                }
            };
            exit_with(result)
        }
        _ => {
            print_usage();
            ExitCode::from(1)
        }
    }
}
