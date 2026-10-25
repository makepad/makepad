//! `~/.makepad/deskhost.conf` (`MAKEPAD_DESKHOST_CONF` names another file), `key = value`
//! lines, every key optional:
//!
//! ```text
//! enabled = on            # off (the default): nothing listens, the host exits
//! allow_peers = 10.0.0.222  # the viewer machines' addresses, comma separated; required: empty
//!                         # or missing serves nobody (IP pinning: give them a DHCP reservation)
//! host = 0.0.0.0          # loopback, a private LAN address of this machine, or 0.0.0.0
//! port = 8790
//! kbps = 0                # 0: by picture size
//! fps = 120               # the cap: the stream runs at the lowest of it, the screen's refresh
//!                         # and the watching viewers' displays
//! fleet_dir =             # fleet.key + revoked; default: ~/.makepad/weights/fleet (the AI hub's)
//! ```

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub kbps: u32,
    pub fps: u32,
    pub fleet_dir: Option<PathBuf>,
    /// The only peers served (`allow_peers`).
    pub allow_peers: Vec<IpAddr>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, host: "0.0.0.0".into(), port: makepad_desk_link::DEFAULT_PORT, kbps: 0, fps: 120, fleet_dir: None, allow_peers: Vec::new() }
    }
}

/// `~/.makepad` (`MAKEPAD_HOME` names another).
pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("MAKEPAD_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(".makepad")
}

pub fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "desk host".into())
}

impl Settings {
    pub fn path() -> PathBuf {
        match std::env::var_os("MAKEPAD_DESKHOST_CONF").filter(|p| !p.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => home().join("deskhost.conf"),
        }
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
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
                _ => false,
            };
            if !ok {
                crate::log(&format!("settings: skipped {line:?}"));
            }
        }
        s
    }

        pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.host.parse().map_err(|_| format!("host {:?} is not an IP address", self.host))?;
        if !makepad_desk_link::server::bindable(ip) {
            return Err(format!("host {ip} is not a LAN address"));
        }
        Ok(SocketAddr::new(ip, self.port))
    }

        pub fn fleet_dir(&self) -> PathBuf {
        self.fleet_dir.clone().unwrap_or_else(|| home().join("weights").join("fleet"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_enabled_and_lan_only() {
        assert!(!Settings::parse("").enabled);
        let s = Settings::parse("enabled = on\nhost = 10.0.0.230\nfps = 0\nallow_peers = 10.0.0.222");
        assert!(s.enabled && s.fps == 120 && s.listen_addr().is_ok());
        assert_eq!(s.allow_peers, vec!["10.0.0.222".parse::<IpAddr>().unwrap()]);
        assert!(Settings::parse("host = 1.1.1.1").listen_addr().is_err());
    }
}
