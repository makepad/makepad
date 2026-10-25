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
//! kbps = 0                # stream rate, 0 = by picture size (about 0.45 bits a pixel a picture
//!                         # at 4:4:4, 0.3 at 4:2:0) up to max_mbps
//! max_mbps = 400          # kbps = 0: the whole stream (all its tiles) at most this; 0 = no cap
//!                         # (a 6800x2100 4:4:4 desktop at 120 would take ~770 Mbit/s)
//! fps = 120               # at most this many pictures a second; the stream runs at the lowest of
//!                         # this, the streamed screen's refresh and the watching viewers' refresh
//! chroma = 444           # 444 (every pixel's colour: sharp text; the default, for viewers whose
//!                         # decoder takes it) or 420
//! split = tiles           # a GPU with several encode engines: tiles (each picture in bands, one
//!                         # per engine, at once: the lowest latency), interleave (whole pictures,
//!                         # the engines in turn: the most pictures per second) or off
//! tiles = auto            # tiles: the cut of each picture, auto (every engine the same share,
//!                         # within the encoder's size and the decoder's rate) or CxR (e.g. 2x2)
//! fleet_dir =             # where fleet.key / revoked are; default: the AI hub's <cache_dir>/fleet
//! dump =                  # <file>[,<pictures>]: a check of the encoder without the network: the
//!                         # first pictures (default 600) go to an Annex-B file (no socket opens)
//! ```
//!
//! Whatever the host, nothing listens without a loadable fleet key: every session is TLS with
//! a fleet credential (role `lan`), loopback included.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// How the streams of a GPU with several encode engines share the pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// One stream (one engine).
    Off,
    /// Each picture in bands, one per engine, encoded at once.
    Tiles,
    /// Whole pictures, the engines in turn.
    Interleave,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeskLinkSettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub screen: String,
    pub kbps: u32,
    /// `kbps = 0`: the whole stream's cap (Mbit/s; 0 = none).
    pub max_mbps: u32,
    pub fps: u32,
    pub split: Split,
    /// `tiles = CxR`: columns x bands of each picture (tiles only); `None`: auto.
    pub tiles: Option<(u32, u32)>,
    pub full_chroma: bool,
    pub fleet_dir: Option<PathBuf>,
    /// The only peers served (`allow_peers`).
    pub allow_peers: Vec<IpAddr>,
    pub dump: Option<String>,
}

impl DeskLinkSettings {
    /// The whole stream's rate (kbit/s, shared by its tiles) for `size` pictures at `fps`:
    /// `kbps`, or by picture size up to `max_mbps`.
    pub fn stream_kbps(&self, size: [u32; 2], fps: u32, full_chroma: bool) -> u32 {
        if self.kbps != 0 {
            return self.kbps;
        }
        let per_mille: u64 = if full_chroma { 450 } else { 300 };
        let by_size = (size[0] as u64 * size[1] as u64 * fps.max(1) as u64 * per_mille / 1_000_000).max(2_000);
        let cap = if self.max_mbps == 0 { u64::MAX } else { self.max_mbps as u64 * 1000 };
        by_size.min(cap) as u32
    }
}

impl Default for DeskLinkSettings {
    fn default() -> Self {
        DeskLinkSettings {
            enabled: false,
            host: "0.0.0.0".into(),
            port: makepad_desk_link::DEFAULT_PORT,
            screen: "primary".into(),
            kbps: 0,
            max_mbps: 400,
            fps: 120,
            split: Split::Tiles,
            tiles: None,
            full_chroma: true,
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
                "max_mbps" => value.parse().map(|m| s.max_mbps = m).is_ok(),
                "fps" => value.parse::<u32>().ok().filter(|f| (1..=240).contains(f)).map(|f| s.fps = f).is_some(),
                "chroma" => match value {
                    "444" => { s.full_chroma = true; true }
                    "420" => { s.full_chroma = false; true }
                    _ => false,
                },
                "split" => match value {
                    "tiles" => { s.split = Split::Tiles; true }
                    "interleave" => { s.split = Split::Interleave; true }
                    "off" => { s.split = Split::Off; true }
                    _ => false,
                },
                "tiles" => match value {
                    "auto" => { s.tiles = None; true }
                    _ => match value.split_once('x').map(|(c, r)| (c.trim().parse::<u32>(), r.trim().parse::<u32>())) {
                        Some((Ok(c), Ok(r))) if (1..=8).contains(&c) && (1..=8).contains(&r) => { s.tiles = Some((c, r)); true }
                        _ => false,
                    },
                },
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
        let (_, hub) = crate::hub_app::load_conf();
        hub.cache_dir.map(PathBuf::from).unwrap_or_else(|| crate::theme::makepad_home().join("weights")).join("fleet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_desk_link::server::lan_address;

    #[test]
    fn a_big_desktop_streams_within_the_cap() {
        let s = DeskLinkSettings::default();
        assert_eq!(s.stream_kbps([6800, 2100], 120, true), 400_000, "~770 Mbit/s by size, capped");
        assert_eq!(s.stream_kbps([1920, 1080], 60, true), 55_987, "under the cap: by size");
        let none = DeskLinkSettings::parse("max_mbps = 0");
        assert_eq!(none.stream_kbps([6800, 2100], 120, true), 771_120);
    }

    #[test]
    fn off_unless_enabled() {
        assert!(!DeskLinkSettings::parse("").enabled);
        assert!(!DeskLinkSettings::parse("enabled = maybe").enabled);
        let s = DeskLinkSettings::parse("enabled = on\nhost = 10.0.0.166\nport = 9000\nkbps = 30000 # LAN\nfps = 999");
        assert_eq!(s.stream_kbps([7680, 2160], 120, true), 30000, "an explicit kbps wins");
        assert!(s.enabled);
        assert_eq!((s.host.as_str(), s.port, s.kbps, s.fps), ("10.0.0.166", 9000, 30000, 120));
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
