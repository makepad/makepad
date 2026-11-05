//! A devhub node's settings file (the WM's `~/.makepad/wm/devhub.conf`; any file through
//! `makepad devhub serve --node`), its default name and port, and devhub's tray glyph: what the WM
//! needs of devhub, without linking devhub (a devhub landing relinked and so restarted every idle
//! node's WM), and the report of a hub the WM could not keep running ([`Trouble`]).
//! Platform-neutral: the WM reads it on every OS.

pub mod tray_glyph;

/// The node's fleet TLS listener (master and peer connections).
pub const NODE_PORT: u16 = 8797;
/// Hitches shorter than this are not worth a line (the JIT reports from 50 ms).
pub const HITCH_MS: f64 = 500.0;

/// A lane's one spelling, lower case: a Mac's file system folds case, so `SLICECPU` and
/// `slicecpu` are one native tree there (a startup scrub took a running lease's shipped files when
/// its hold said the one and the scrub the other, 11-04). Each place a lane enters devhub (a lease
/// ask, `lane new`, a job submit, a live session, `stop`) folds it here; every key after is that one string.
pub fn lane_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Whether `name` is a hub name: what a line and a hosts entry can carry.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// This machine's first private IPv4 address (the LAN address a node binds), if any.
pub fn lan_ipv4() -> Option<std::net::Ipv4Addr> {
    // a UDP connect sends nothing: it only asks the routing table for the source address of a
    // TEST-NET-1 destination (routed, so it never counts as touching the local network)
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if v4.is_private() => Some(v4),
        _ => None,
    }
}

/// The default machine name: a node is called by the last octet of its LAN address (`217`), the
/// way the fleet is spoken of; a machine without one by its host name.
pub fn default_machine_name() -> String {
    if let Some(ip) = lan_ipv4() {
        return ip.octets()[3].to_string();
    }
    host_name()
}

/// `/etc/hostname`, else the OS's, cleaned to `a-z 0-9 - _`.
pub fn host_name() -> String {
    let raw = std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_else(os_hostname);
    let clean: String = raw.split('.').next().unwrap_or("").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).take(48).collect();
    if clean.is_empty() { "machine".into() } else { clean }
}

#[cfg(unix)]
fn os_hostname() -> String {
    extern "C" {
        fn gethostname(name: *mut u8, len: usize) -> i32;
    }
    let mut buf = [0u8; 256];
    if unsafe { gethostname(buf.as_mut_ptr(), buf.len()) } != 0 {
        return String::new();
    }
    let n = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

#[cfg(not(unix))]
fn os_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}


/// The env var the WM sets to the node's conf file (as for the AI Hub): its presence makes the
/// app a node.
pub const NODE_CONF_ENV: &str = "MAKEPAD_DEVHUB_NODE_CONF";

/// `key = value` lines, all optional:
/// ```text
/// enabled = on            # off: the WM leaves the hub stopped
/// name = 217              # default: the last octet of the LAN address
/// host = 192.0.2.89       # the LAN address to listen on (default: this machine's private IPv4)
/// port = 8797
/// hitch_ms = 500          # shorter hitches are not reported
/// launch = makepad run    # what `session launch <app>` runs
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
        NodeConf { enabled: true, name: String::new(), host: None, port: NODE_PORT, hitch_ms: HITCH_MS, launch: default_launch(), embedded: false, discovery: !cfg!(target_os = "macos") }
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
                "name" if valid_name(v) => c.name = v.to_string(),
                "host" => c.host = v.parse().ok(),
                "port" => c.port = v.parse().unwrap_or(c.port),
                "hitch_ms" => c.hitch_ms = v.parse().unwrap_or(c.hitch_ms),
                "embedded" => c.embedded = matches!(v, "on" | "true" | "1"),
                "discovery" => c.discovery = matches!(v, "on" | "true" | "1"),
                "launch" if !v.is_empty() => c.launch = v.split_whitespace().map(str::to_string).collect(),
                _ => eprintln!("devhub: conf: skipped `{line}`"),
            }
        }
        if c.name.is_empty() {
            c.name = default_machine_name();
        }
        c
    }

    pub fn load(path: &std::path::Path) -> NodeConf {
        Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }
}

/// The file beside a node's conf (`~/.makepad/wm/devhub.down`) in which the WM keeps why it could
/// not start or keep the node's hub: the WM writes it on each failed start or exit, and the hub that
/// starts next reports it to the master's inbox and removes it. While no hub runs on a box nothing
/// there reaches the master; this way the outage and its reason still arrive.
pub const TROUBLE_FILE: &str = "devhub.down";

/// What [`TROUBLE_FILE`] holds: since when the hub has not run, how many starts failed or ended,
/// and the latest reason.
#[derive(Clone, Debug, PartialEq)]
pub struct Trouble {
    pub since_ms: u64,
    pub tries: u32,
    pub reason: String,
}

impl Trouble {
    /// One more failed start or exit at `now_ms`, after `prev` (None: the first of this outage).
    pub fn next(prev: Option<Trouble>, now_ms: u64, reason: &str) -> Trouble {
        let reason = reason.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        match prev {
            Some(p) => Trouble { since_ms: p.since_ms, tries: p.tries + 1, reason },
            None => Trouble { since_ms: now_ms, tries: 1, reason },
        }
    }

    pub fn to_text(&self) -> String {
        format!("since_ms = {}\ntries = {}\nreason = {}\n", self.since_ms, self.tries, self.reason)
    }

    pub fn parse(text: &str) -> Option<Trouble> {
        let get = |key: &str| text.lines().find_map(|l| l.split_once(" = ").filter(|(k, _)| *k == key).map(|(_, v)| v.trim().to_string()));
        Some(Trouble { since_ms: get("since_ms")?.parse().ok()?, tries: get("tries")?.parse().ok()?, reason: get("reason")? })
    }

    /// The line the hub reports once it runs, at `now_ms`.
    pub fn report(&self, machine: &str, now_ms: u64) -> String {
        let s = now_ms.saturating_sub(self.since_ms) / 1000;
        let took = if s < 120 { format!("{s} s") } else { format!("{} min", s / 60) };
        let tries = if self.tries == 1 { "1 try".to_string() } else { format!("{} tries", self.tries) };
        format!("devhub on {machine} could not run for {took} ({tries}): {}", self.reason)
    }
}

/// The trouble file of the node whose conf is `conf_path`.
pub fn trouble_path(conf_path: &std::path::Path) -> std::path::PathBuf {
    conf_path.with_file_name(TROUBLE_FILE)
}

#[cfg(test)]
mod tests {
    #[test]
    fn trouble_keeps_its_start_counts_tries_and_says_the_latest_reason() {
        use super::Trouble;
        let first = Trouble::next(None, 1_000, "could not start: the user's service manager is down\n");
        assert_eq!(first, Trouble { since_ms: 1_000, tries: 1, reason: "could not start: the user's service manager is down".into() });
        let second = Trouble::next(Trouble::parse(&first.to_text()), 9_000, "exited (signal: 9)");
        assert_eq!((second.since_ms, second.tries, second.reason.as_str()), (1_000, 2, "exited (signal: 9)"));
        assert_eq!(second.report("node-a", 31_000), "devhub on node-a could not run for 30 s (2 tries): exited (signal: 9)");
        assert_eq!(first.report("node-a", 1_000 + 27 * 60_000), "devhub on node-a could not run for 27 min (1 try): could not start: the user's service manager is down");
        assert_eq!(Trouble::parse("tries = 1\n"), None);
        assert_eq!(super::trouble_path(std::path::Path::new("/h/.makepad/wm/devhub.conf")), std::path::Path::new("/h/.makepad/wm/devhub.down"));
    }

    #[test]
    fn conf_defaults_and_overrides() {
        let c = super::NodeConf::parse("name = 217\nport = 9001\nhitch_ms = 250\nlaunch = /home/me/bin/makepad run\nbogus = 1\n");
        assert_eq!(c.name, "217");
        assert_eq!(c.port, 9001);
        assert_eq!(c.hitch_ms, 250.0);
        assert_eq!(c.launch, vec!["/home/me/bin/makepad", "run"]);
        assert!(c.enabled);
        assert!(!super::NodeConf::parse("enabled = off\nname = x\n").enabled);
    }
}
