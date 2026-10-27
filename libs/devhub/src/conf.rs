//! A node's settings file (the WM's `~/.makepad/wm/devhub.conf`; any file through
//! `makepad devhub serve --node`). Platform-neutral: the WM reads it on every OS.

use crate::proto;

/// The env var the WM sets to the node's conf file (as for the AI Hub): its presence makes the
/// app a node.
pub const NODE_CONF_ENV: &str = "MAKEPAD_DEVHUB_NODE_CONF";

/// `key = value` lines, all optional:
/// ```text
/// enabled = on            # off: the WM leaves the hub stopped
/// name = 217              # default: the last octet of the LAN address
/// host = 10.0.0.217       # the LAN address to listen on (default: this machine's private IPv4)
/// port = 8797
/// hitch_ms = 500          # shorter hitches are not reported
/// launch = makepad run    # what `session launch <app>` runs
/// nice = 10               # the WM starts it under nice
/// embedded = off          # on: devhub runs inside the JIT host it observes (phones, headsets)
/// discovery = on          # off: no LAN beacon (macOS: sending or receiving broadcasts asks the
///                         # Local Network question, which nobody answers on a headless Mac);
///                         # the master still dials it from nodes.conf. Default off on macOS.
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct NodeConf {
    pub enabled: bool,
    pub name: String,
    pub host: Option<std::net::IpAddr>,
    pub port: u16,
    pub hitch_ms: f64,
    pub launch: Vec<String>,
    /// devhub runs inside the JIT host it observes (a phone or headset build): its own process's
    /// programs are observed too
    pub embedded: bool,
    /// the WM starts it under `nice -n <nice>` (0: not niced)
    pub nice: i32,
    /// announce itself and listen for the other nodes on the LAN beacon
    pub discovery: bool,
}

/// `makepad run` by this very binary when it is the `makepad` tool (`makepad devhub serve`), so a
/// hub started outside a shell never looks `makepad` up on PATH; else `makepad` next to it, then
/// the bare name.
fn default_launch() -> Vec<String> {
    let exe = std::env::current_exe().ok();
    let tool = exe.as_ref().and_then(|e| {
        if e.file_stem().is_some_and(|s| s == "makepad") {
            return Some(e.clone());
        }
        let sib = e.with_file_name(if cfg!(windows) { "makepad.exe" } else { "makepad" });
        sib.is_file().then_some(sib)
    });
    vec![tool.map_or_else(|| "makepad".to_string(), |t| t.display().to_string()), "run".into()]
}

impl Default for NodeConf {
    fn default() -> Self {
        NodeConf { enabled: true, name: String::new(), host: None, port: proto::NODE_PORT, hitch_ms: proto::HITCH_MS, launch: default_launch(), embedded: false, nice: 10, discovery: !cfg!(target_os = "macos") }
    }
}

impl NodeConf {
    pub fn parse(text: &str) -> NodeConf {
        let mut c = NodeConf::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "enabled" => c.enabled = !matches!(v, "off" | "false" | "0"),
                "name" if crate::trust::valid_name(v) => c.name = v.to_string(),
                "host" => c.host = v.parse().ok(),
                "port" => c.port = v.parse().unwrap_or(c.port),
                "hitch_ms" => c.hitch_ms = v.parse().unwrap_or(c.hitch_ms),
                "nice" => c.nice = v.parse().unwrap_or(c.nice),
                "embedded" => c.embedded = matches!(v, "on" | "true" | "1"),
                "discovery" => c.discovery = matches!(v, "on" | "true" | "1"),
                "launch" if !v.is_empty() => c.launch = v.split_whitespace().map(str::to_string).collect(),
                _ => eprintln!("devhub: conf: skipped `{line}`"),
            }
        }
        if c.name.is_empty() {
            c.name = proto::default_machine_name();
        }
        c
    }

    pub fn load(path: &std::path::Path) -> NodeConf {
        Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn conf_defaults_and_overrides() {
        let c = super::NodeConf::parse("name = 217\nport = 9001\nhitch_ms = 250\nlaunch = /home/arch/bin/makepad run\nbogus = 1\n");
        assert_eq!(c.name, "217");
        assert_eq!(c.port, 9001);
        assert_eq!(c.hitch_ms, 250.0);
        assert_eq!(c.launch, vec!["/home/arch/bin/makepad", "run"]);
        assert!(c.enabled);
        assert!(!super::NodeConf::parse("enabled = off\nname = x\n").enabled);
    }
}
