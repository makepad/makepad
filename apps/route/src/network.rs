//! Whether Route may use the network.
//!
//! Offline means cache-only: the map reads only the archives' disk cache
//! (tiles it does not hold show as no data), search and routing use the
//! local nav pairs, and nothing that needs a server runs (rain radar, wind,
//! the weather nowcast, image search, the cloud model, the test-map
//! download). No connection is attempted, so nothing waits or retries.
//!
//! Route is offline when the person says so (the "Offline" switch, kept as
//! the `route/offline` file in the makepad home so it holds from the first
//! frame) or by itself when this machine has no network: no up,
//! non-loopback IPv4 interface, or on Linux no default route. The check
//! reads interface and routing tables only; it sends nothing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const OFFLINE_PREF: &str = "route/offline";

/// Seconds between re-checks of the machine's network.
pub const RECHECK_SECONDS: f64 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkState {
    /// The person's switch: offline whatever the machine has.
    pub forced_offline: bool,
    /// The machine has a network to reach anything with.
    pub present: bool,
}

impl NetworkState {
    pub fn offline(&self) -> bool {
        self.forced_offline || !self.present
    }

    /// The status line for this state.
    pub fn describe(&self) -> &'static str {
        match (self.forced_offline, self.present) {
            (false, true) => "Online",
            (true, _) => "Offline — cached maps and local search only",
            (false, false) => "No network — cached maps and local search only",
        }
    }
}

fn pref_path(home: &Path) -> PathBuf {
    home.join(OFFLINE_PREF)
}

/// The person's offline switch.
pub fn forced_offline(home: &Path) -> bool {
    pref_path(home).is_file()
}

pub fn set_forced_offline(home: &Path, on: bool) -> io::Result<()> {
    let path = pref_path(home);
    if on {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, b"cache-only\n")
    } else {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// True when this machine has a network: an up, non-loopback IPv4
/// interface, and on Linux a default route (IPv4 or IPv6). Where the
/// interfaces cannot be listed (not unix) it assumes a network and the
/// switch decides.
pub fn network_present() -> bool {
    if !cfg!(unix) {
        return true;
    }
    if makepad_core_util::netif::interfaces().is_empty() {
        return false;
    }
    let v4 = fs::read_to_string("/proc/net/route");
    let v6 = fs::read_to_string("/proc/net/ipv6_route");
    match (v4, v6) {
        // No routing tables to read (macOS, a sandboxed Android app).
        (Err(_), Err(_)) => true,
        (v4, v6) => {
            v4.is_ok_and(|text| has_default_route_v4(&text))
                || v6.is_ok_and(|text| has_default_route_v6(&text))
        }
    }
}

fn has_default_route_v4(text: &str) -> bool {
    text.lines().skip(1).any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        fields.len() > 7 && fields[0] != "lo" && fields[1] == "00000000" && fields[7] == "00000000"
    })
}

fn has_default_route_v6(text: &str) -> bool {
    const ANY: &str = "00000000000000000000000000000000";
    text.lines().any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        fields.len() >= 10 && fields[0] == ANY && fields[1] == "00" && fields[9] != "lo"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_is_the_switch_or_no_network() {
        let online = NetworkState { forced_offline: false, present: true };
        assert!(!online.offline());
        assert!(NetworkState { forced_offline: true, present: true }.offline());
        assert!(NetworkState { forced_offline: false, present: false }.offline());
    }

    #[test]
    fn default_routes_are_found_and_loopback_is_not_one() {
        let v4 = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                  wlan0\t00000000\t014DA8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n";
        assert!(has_default_route_v4(v4));
        let lan_only = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                        wlan0\t004DA8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert!(!has_default_route_v4(lan_only));
        let v6 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003 wlan0\n\
                  00000000000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 ffffffff 00000001 00000000 00200200 lo\n";
        assert!(has_default_route_v6(v6));
        assert!(!has_default_route_v6(v6.lines().nth(1).unwrap()));
    }

    #[test]
    fn the_switch_is_a_file_in_the_home() {
        let home = std::env::temp_dir().join(format!("route-offline-pref-{}", std::process::id()));
        assert!(!forced_offline(&home));
        set_forced_offline(&home, true).unwrap();
        assert!(forced_offline(&home));
        set_forced_offline(&home, false).unwrap();
        assert!(!forced_offline(&home));
        set_forced_offline(&home, false).unwrap();
        let _ = fs::remove_dir(home.join("route"));
        let _ = fs::remove_dir(&home);
    }
}
