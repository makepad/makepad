//! `makepad hub`, `makepad remote-relay` and `makepad remote ...`: the command line of the remote hub
//! (routing and links live in libs/remote: hub.rs, relay.rs, pair.rs, hosts.rs).

use makepad_remote::hosts::{self, Host, Hosts, Via};

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn registry() -> Result<Hosts, String> {
    Hosts::load(&hosts::registry_path()).map_err(|e| e.to_string())
}

/// `makepad hub [--listen <addr>]`: the router `makepad inspect <host>:...` starts on demand. It exits
/// 60 s after its last client; with `--listen` (paired devices dial this address) it keeps running.
pub fn hub(args: &[String]) -> i32 {
    #[cfg(unix)]
    {
        let hosts = match registry() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("makepad hub: {e}");
                return 1;
            }
        };
        match makepad_remote::hub::serve(hosts, flag(args, "--listen")) {
            Ok(true) => 0,
            Ok(false) => {
                eprintln!("makepad hub: one is already running");
                0
            }
            Err(e) => {
                eprintln!("makepad hub: {e}");
                1
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        eprintln!("makepad hub: not available on Windows yet");
        2
    }
}

/// Serves this process's makepad-remote endpoints to a hub by dialling out (what `makepad remote-relay
/// --dial` does as its own process): for a host that cannot run one, the Android host APK. `spec` is
/// `<hub host:port>|<key hex>|<hub fingerprint hex>|<name>` (what `makepad remote pair --print` made);
/// the key goes into `dir` (private), the relay dials forever on its own thread.
pub fn dial_in_background(spec: &str, dir: &std::path::Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use makepad_remote::relay;
        let mut parts = spec.split(',');
        let (addr, key, fp, name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or("phone"));
        if addr.is_empty() || key.is_empty() {
            return Err("remote spec: <hub host:port>,<key hex>,<fingerprint hex>,<name>".into());
        }
        let key_file = dir.join("remote.key");
        makepad_network::tls::write_private(&key_file, format!("{key}\n").as_bytes()).map_err(|e| e.to_string())?;
        let creds = relay::credentials(&key_file, fp, dir.join("known_hosts")).map_err(|e| e.to_string())?;
        let cfg = relay::Config { name: name.to_string(), launch: Vec::new(), env: Vec::new(), ledger: dir.join("sessions") };
        let ep = relay::endpoint(cfg);
        let addr = addr.to_string();
        std::thread::Builder::new().name("remote-dial".into()).spawn(move || relay::dial_forever(&ep, &addr, &creds)).map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (spec, dir);
        Err("not available on Windows yet".into())
    }
}

/// `makepad remote-relay [--env NAME=value ...] [--dial <hub host:port> --key-file <path> [--name <n>] [--fingerprint <hex>]]`:
/// serves this machine's endpoints to a hub, over stdin/stdout (run through ssh) or by dialling.
/// `--env` adds to the environment of the sessions it launches (the hub passes a host's `env`).
pub fn relay(args: &[String]) -> i32 {
    #[cfg(unix)]
    {
        use makepad_remote::relay;
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("remote-relay: cannot find the makepad executable");
            return 1;
        };
        let name = flag(args, "--name").unwrap_or("relay").to_string();
        let mut env: Vec<(String, String)> = vec![("MAKEPAD_HIDE_WINDOWS".into(), "1".into()), ("SANDBOX_MUTE".into(), "1".into())];
        for (i, a) in args.iter().enumerate() {
            if a != "--env" {
                continue;
            }
            let Some((k, v)) = args.get(i + 1).and_then(|p| p.split_once('=')).filter(|(k, _)| hosts::valid_env_name(k)) else {
                eprintln!("remote-relay: --env needs NAME=value");
                return 2;
            };
            env.retain(|(have, _)| have != k);
            env.push((k.to_string(), v.to_string()));
        }
        let cfg = relay::Config { name, launch: vec![exe.display().to_string(), "run".into()], env, ledger: relay::default_ledger() };
        let ep = relay::endpoint(cfg);
        let Some(addr) = flag(args, "--dial") else {
            return match relay::serve_stdio(&ep) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("remote-relay: {e}");
                    1
                }
            };
        };
        let Some(key_file) = flag(args, "--key-file") else {
            eprintln!("remote-relay: --dial needs --key-file <path>");
            return 2;
        };
        let creds = match relay::credentials(std::path::Path::new(key_file), flag(args, "--fingerprint").unwrap_or(""), hosts::config_dir().join("known_hosts")) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("remote-relay: {key_file}: {e}");
                return 1;
            }
        };
        relay::dial_forever(&ep, addr, &creds)
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        eprintln!("remote-relay: not available on Windows yet");
        2
    }
}

fn remote_usage() -> i32 {
    eprintln!("usage: makepad remote add <name> --ssh <user@host> [--makepad <path on that host>] [--env NAME=value ...]");
    eprintln!("       makepad remote pair <name> [--hub <host:port>] [--print | --adb | --ssh <user@host>]");
    eprintln!("       makepad remote list");
    2
}

/// `makepad remote add|pair|list`: the hosts registry (`~/.config/makepad/remote/hosts.toml`).
pub fn remote(args: &[String]) -> i32 {
    let run = || -> Result<(), String> {
        match args.first().map(String::as_str) {
            Some("add") => {
                let name = args.get(1).ok_or("add: a host name")?;
                let ssh = flag(args, "--ssh").ok_or("add: --ssh <user@host>")?;
                if !hosts::valid_name(name) {
                    return Err(format!("host name `{name}`: use letters, digits, `-` and `_`"));
                }
                let mut reg = registry()?;
                let mut env = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    if a == "--env" {
                        let (k, v) = args.get(i + 1).and_then(|p| p.split_once('=')).filter(|(k, _)| hosts::valid_env_name(k)).ok_or("add: --env NAME=value")?;
                        env.push((k.to_string(), v.to_string()));
                    }
                }
                reg.set(Host { name: name.clone(), via: Via::Ssh { ssh: ssh.to_string(), relax: flag(args, "--makepad").unwrap_or("makepad").to_string(), env } });
                reg.save(&hosts::registry_path()).map_err(|e| e.to_string())
            }
            Some("pair") => {
                use makepad_remote::pair::{pair, Delivery};
                let name = args.get(1).ok_or("pair: a host name")?;
                let delivery = if args.iter().any(|a| a == "--adb") {
                    Delivery::Adb
                } else if let Some(t) = flag(&args[2..], "--ssh") {
                    Delivery::Ssh(t.to_string())
                } else {
                    Delivery::Print
                };
                let hub = flag(args, "--hub").unwrap_or(if matches!(delivery, Delivery::Adb) { "127.0.0.1:7907" } else { "" });
                if hub.is_empty() {
                    return Err("pair: --hub <host:port> (the address the device dials; the hub runs `makepad hub --listen` on it)".into());
                }
                let cmd = pair(name, hub, delivery).map_err(|e| e.to_string())?;
                println!("{cmd}");
                eprintln!("paired `{name}`; (re)start `makepad hub --listen {hub}` to accept it");
                Ok(())
            }
            Some("list") => {
                for h in registry()?.hosts {
                    match h.via {
                        Via::Ssh { ssh, relax, .. } => println!("{} ssh {ssh} {relax}", h.name),
                        Via::Dial { .. } => println!("{} dial", h.name),
                    }
                }
                Ok(())
            }
            _ => Err(String::new()),
        }
    };
    match run() {
        Ok(()) => 0,
        Err(e) if e.is_empty() => remote_usage(),
        Err(e) => {
            eprintln!("makepad remote: {e}");
            1
        }
    }
}
