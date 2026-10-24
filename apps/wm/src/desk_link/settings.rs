//! Desk Link's settings: `~/.makepad/wm/desk-link.conf` (`MAKEPAD_WM_DESK_LINK_CONF` names
//! another file), `key = value` lines, every key optional:
//!
//! ```text
//! enabled = on            # off (the default): the WM opens no socket at all
//! allow_peers = 10.0.0.222  # the viewer machines' addresses, comma separated; required: empty
//!                         # or missing serves nobody (IP pinning: give them a DHCP reservation)
//! host = 0.0.0.0          # an address of this machine: loopback, a private LAN address or 0.0.0.0
//! port = 8790
//! screen = primary        # the screen a viewer that names none gets: primary or a connector name
//! kbps = 0                # stream rate, 0 = by picture size
//! fps = 60                # the rate the encoder plans for
//! fleet_dir =             # where fleet.key / revoked are; default: the AI hub's <cache_dir>/fleet
//! dump =                  # <file>[,<pictures>]: a check of the encoder without the network: the
//!                         # first pictures (default 600) go to an Annex-B file (no socket opens)
//! ```
//!
//! Whatever the host, nothing listens without a loadable fleet key: every session is TLS with
//! a fleet credential (role `lan`), loopback included.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct DeskLinkSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub screen: String,
    pub kbps: u32,
    pub fps: u32,
    pub fleet_dir: Option<PathBuf>,
    /// The only peers served (`allow_peers`).
    pub allow_peers: Vec<IpAddr>,
    pub dump: Option<String>,
}

impl Default for DeskLinkSettings {
    fn default() -> Self {
        DeskLinkSettings {
            enabled: false,
            host: "0.0.0.0".into(),
            port: makepad_desk_link::DEFAULT_PORT,
            screen: "primary".into(),
            kbps: 0,
            fps: 60,
            fleet_dir: None,
            allow_peers: Vec::new(),
            dump: None,
        }
    }
}

const CONF_ENV: &str = "MAKEPAD_WM_DESK_LINK_CONF";

impl DeskLinkSettings {
    pub fn path() -> PathBuf {
        match std::env::var_os(CONF_ENV).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => crate::theme::makepad_home().join("wm/desk-link.conf"),
        }
    }

    pub fn load() -> DeskLinkSettings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// Unknown keys and bad values are skipped and logged; a typo never enables anything.
    pub fn parse(text: &str) -> DeskLinkSettings {
        let mut s = DeskLinkSettings::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let ok = match key {
                "enabled" => match value {
                    "on" | "true" | "yes" | "1" => {
                        s.enabled = true;
                        true
                    }
                    "off" | "false" | "no" | "0" => {
                        s.enabled = false;
                        true
                    }
                    _ => false,
                },
                "host" => {
                    s.host = value.to_string();
                    !value.is_empty()
                }
                "port" => value.parse().map(|p| s.port = p).is_ok(),
                "screen" => {
                    s.screen = value.to_string();
                    !value.is_empty()
                }
                "kbps" => value.parse().map(|k| s.kbps = k).is_ok(),
                "fps" => value.parse::<u32>().ok().filter(|f| (1..=240).contains(f)).map(|f| s.fps = f).is_some(),
                "allow_peers" => {
                    let mut ok = true;
                    s.allow_peers = value
                        .split(',')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .filter_map(|v| v.parse::<IpAddr>().map_err(|_| ok = false).ok())
                        .collect();
                    ok
                }
                "fleet_dir" => {
                    s.fleet_dir = Some(PathBuf::from(value)).filter(|_| !value.is_empty());
                    true
                }
                "dump" => {
                    s.dump = Some(value.to_string()).filter(|_| !value.is_empty());
                    true
                }
                _ => false,
            };
            if !ok {
                super::note(&format!("desk link settings: skipped {line:?}"));
            }
        }
        s
    }

    /// The address to listen on, or why it is refused: only this machine's loopback, a private
    /// LAN address or the unspecified address (all interfaces; the peer filter keeps it LAN).
    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.host.parse().map_err(|_| format!("host {:?} is not an IP address", self.host))?;
        if !makepad_desk_link::server::bindable(ip) {
            return Err(format!("host {ip} is not a LAN address (loopback, 10/8, 172.16/12, 192.168/16, link-local, fc00::/7)"));
        }
        Ok(SocketAddr::new(ip, self.port))
    }

    /// The fleet key's directory: `fleet_dir`, else the AI hub's (its settings' cache_dir, else
    /// `~/.makepad/weights`) `fleet`.
    pub fn fleet_dir(&self) -> PathBuf {
        if let Some(dir) = &self.fleet_dir {
            return dir.clone();
        }
        let hub = crate::hub_child::HubSettings::load();
        let cache = hub.cache_dir.map(PathBuf::from).unwrap_or_else(|| crate::theme::makepad_home().join("weights"));
        cache.join("fleet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_desk_link::server::lan_address;

    #[test]
    fn off_unless_enabled() {
        assert!(!DeskLinkSettings::parse("").enabled);
        assert!(!DeskLinkSettings::parse("enabled = maybe").enabled);
        let s = DeskLinkSettings::parse("enabled = on\nhost = 10.0.0.166\nport = 9000\nkbps = 30000 # LAN\nfps = 999");
        assert!(s.enabled);
        assert_eq!((s.host.as_str(), s.port, s.kbps, s.fps), ("10.0.0.166", 9000, 30000, 60));
        assert!(s.allow_peers.is_empty());
        let s = DeskLinkSettings::parse("allow_peers = 10.0.0.222, 10.0.0.230");
        assert_eq!(s.allow_peers, vec!["10.0.0.222".parse::<IpAddr>().unwrap(), "10.0.0.230".parse().unwrap()]);
    }

    #[test]
    fn only_lan_binds() {
        let at = |host: &str| DeskLinkSettings { host: host.into(), ..Default::default() }.listen_addr();
        assert!(at("0.0.0.0").is_ok());
        assert!(at("127.0.0.1").is_ok());
        assert!(at("10.0.0.166").is_ok());
        assert!(at("192.168.1.2").is_ok());
        assert!(at("fd00::1").is_ok());
        assert!(at("8.8.8.8").is_err());
        assert!(at("2001:db8::1").is_err());
        assert!(at("lan").is_err());
        assert!(!lan_address("100.64.0.1".parse().unwrap()));
        assert!(lan_address("::ffff:10.1.2.3".parse().unwrap()));
    }
}
