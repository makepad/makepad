//! Desk Link's settings: `~/.makepad/wm/desk-link.conf` (`MAKEPAD_WM_DESK_LINK_CONF` names
//! another file), `key = value` lines, every key optional:
//!
//! ```text
//! enabled = on            # off (the default): the WM opens no socket at all
//! host = 0.0.0.0          # an address of this machine: loopback, a private LAN address or 0.0.0.0
//! port = 8790
//! screen = primary        # the screen a viewer that names none gets: primary or a connector name
//! kbps = 0                # a fixed stream rate; 0 = adaptive (climbs while the link stays clean,
//!                         # backs off when a viewer falls behind)
//! max_mbps = 0            # kbps = 0: the whole stream (all its tiles) at most this; 0 = no cap
//! fps = 120               # at most this many pictures a second; the stream runs at the lowest of
//!                         # this, the streamed screen's refresh and the watching viewers' refresh
//! chroma = 444           # 444 (every pixel's colour: sharp text; the default, for viewers whose
//!                         # decoder takes it) or 420
//! split = tiles           # a GPU with several encode engines: tiles (each picture in bands, one
//!                         # per engine, at once: the lowest latency), interleave (whole pictures,
//!                         # the engines in turn: the most pictures per second) or off
//! tiles = auto            # tiles: the cut of each picture, auto (every engine the same share,
//!                         # within the encoder's size and the decoder's rate) or CxR (e.g. 2x2)
//! dump =                  # <file>[,<pictures>]: a check of the encoder without the network: the
//!                         # first pictures (default 600) go to an Annex-B file (no socket opens)
//! ```
//!
//! Whatever the host, nothing listens without this machine's fleet identity (`id.key`,
//! `id.cred`, `root.pub` in `~/.makepad/fleet`; tools/fleet-identity.md): every session is TLS
//! with the desktop's fleet certificate, and only the admin (the Mac's device credential from
//! its bound address) may view, loopback included.

use std::net::IpAddr;
#[cfg(not(target_arch = "wasm32"))]
use std::net::SocketAddr;
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
    /// `codec = auto` (the default: H.265 where the GPU and every watching viewer take it) or
    /// `h264` (never H.265).
    pub hevc: bool,
    pub dump: Option<String>,
}

impl DeskLinkSettings {
    /// The adaptive rate's ceiling (kbit/s, all tiles): `max_mbps`, or none.
    pub fn ceiling_kbps(&self) -> u32 {
        if self.max_mbps == 0 { u32::MAX } else { self.max_mbps.saturating_mul(1000) }
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
            max_mbps: 0,
            fps: 120,
            split: Split::Tiles,
            tiles: None,
            full_chroma: true,
            hevc: true,
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
                "codec" => match value {
                    "auto" | "hevc" | "h265" => { s.hevc = true; true }
                    "h264" => { s.hevc = false; true }
                    _ => false,
                },
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
    #[cfg(not(target_arch = "wasm32"))]
    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.host.parse().map_err(|_| format!("host {:?} is not an IP address", self.host))?;
        if !makepad_desk_link::server::bindable(ip) {
            return Err(format!("host {ip} is not a LAN address (loopback, 10/8, 172.16/12, 192.168/16, link-local, fc00::/7)"));
        }
        Ok(SocketAddr::new(ip, self.port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_desk_link::server::lan_address;

    #[test]
    fn no_ceiling_unless_set() {
        assert_eq!(DeskLinkSettings::default().ceiling_kbps(), u32::MAX, "the adaptive rate alone decides");
        assert_eq!(DeskLinkSettings::parse("max_mbps = 300").ceiling_kbps(), 300_000);
    }

    #[test]
    fn off_unless_enabled() {
        assert!(!DeskLinkSettings::parse("").enabled);
        assert!(!DeskLinkSettings::parse("enabled = maybe").enabled);
        let s = DeskLinkSettings::parse("enabled = on\nhost = 10.0.0.166\nport = 9000\nkbps = 30000 # LAN\nfps = 999");
        assert!(s.enabled);
        assert_eq!((s.host.as_str(), s.port, s.kbps, s.fps), ("10.0.0.166", 9000, 30000, 120));
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
