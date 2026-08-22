//! Live set of asset-ai boxes for one named fleet.
//!
//! Every Asset Server embed (sandbox, asset-ui, standalone) uses this so a
//! GPU node can serve many servers — but only those that asked for the same
//! fleet name. Sandbox uses `game` (the 4090 on .123). Asset-ui stays on
//! `default` (the unscoped .169 box).
//!
//! Sources, merged and de-duplicated:
//! - operator seeds (`ChatConfig.fleet_bases`, `MAKEPAD_CHAT_FLEET`)
//! - LAN UDP beacons on the asset-ai discovery port (41830) whose `fleet`
//!   matches the wanted name (missing fleet on old beacons = `default`)

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DISCOVERY_PORT: u16 = 41830;
const BEACON_EXPIRY: Duration = Duration::from_secs(20);
pub const DEFAULT_FLEET: &str = "default";

#[derive(Clone, Default)]
struct Roster {
    wanted: String,
    static_bases: Vec<String>,
    beacons: HashMap<u64, (String, Instant)>,
}

static ROSTER: OnceLock<Arc<Mutex<Roster>>> = OnceLock::new();

fn roster() -> Arc<Mutex<Roster>> {
    ROSTER
        .get_or_init(|| {
            let shared = Arc::new(Mutex::new(Roster {
                wanted: wanted_from_env(),
                ..Roster::default()
            }));
            start_listener(shared.clone());
            shared
        })
        .clone()
}

/// Lowercase trimmed fleet name. Empty becomes [`DEFAULT_FLEET`].
pub fn normalize_fleet(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_FLEET.to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

fn wanted_from_env() -> String {
    normalize_fleet(&std::env::var("MAKEPAD_AI_FLEET").unwrap_or_default())
}

/// Frontends call this before chatting so beacons from other fleets are ignored.
pub fn set_wanted_fleet(name: impl AsRef<str>) {
    let hold = roster();
    let mut slot = hold.lock().unwrap();
    slot.wanted = normalize_fleet(name.as_ref());
}

/// Remember operator-supplied bases (config + env). Safe to call many times.
pub fn seed_bases(bases: impl IntoIterator<Item = String>) {
    let hold = roster();
    let mut slot = hold.lock().unwrap();
    for base in bases {
        let base = normalize(&base);
        if base.is_empty() {
            continue;
        }
        if !slot.static_bases.iter().any(|b| b == &base) {
            slot.static_bases.push(base);
        }
    }
}

/// Current fleet URLs: operator seeds, env, and live beacons for this fleet.
pub fn live_bases() -> Vec<String> {
    seed_from_env();
    let mut out = Vec::new();
    let mut push = |url: String| {
        if !url.is_empty() && !out.iter().any(|b| b == &url) {
            out.push(url);
        }
    };
    {
        let hold = roster();
        let mut slot = hold.lock().unwrap();
        if slot.wanted.is_empty() {
            slot.wanted = wanted_from_env();
        }
        slot.beacons
            .retain(|_, (_, seen)| seen.elapsed() < BEACON_EXPIRY);
        for base in &slot.static_bases {
            push(base.clone());
        }
        for (_, (url, _)) in slot.beacons.iter() {
            push(url.clone());
        }
    }
    out
}

fn seed_from_env() {
    let Ok(raw) = std::env::var("MAKEPAD_CHAT_FLEET") else {
        return;
    };
    seed_bases(
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    );
}

fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn start_listener(shared: Arc<Mutex<Roster>>) {
    if std::env::var_os("MAKEPAD_AI_NO_BEACON").is_some_and(|v| v == "1") {
        return;
    }
    std::thread::Builder::new()
        .name("asset-ai-fleet-listen".into())
        .spawn(move || {
            // Reuse-group bind: the VJ, the Asset UI and a game all listen
            // for the same fleet beacons on one machine. An exclusive bind
            // left every app but the first without a fleet (the asset-ui
            // logged "listener bind :41830 failed: Address already in use").
            let socket = match makepad_asset_client::bind_reuse_udp(DISCOVERY_PORT) {
                Ok(s) => s,
                Err(_) => return,
            };
            let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = [0u8; 512];
            loop {
                let Ok((len, from)) = socket.recv_from(&mut buf) else {
                    continue;
                };
                let Ok(text) = std::str::from_utf8(&buf[..len]) else {
                    continue;
                };
                if !text.contains("makepad-asset-ai") {
                    continue;
                }
                let fleet = beacon_fleet(text);
                let want = shared
                    .lock()
                    .ok()
                    .map(|s| s.wanted.clone())
                    .unwrap_or_else(wanted_from_env);
                if fleet != want {
                    continue;
                }
                let port = beacon_port(text).unwrap_or(8765);
                let node_id = beacon_node_id(text).unwrap_or(from.port() as u64);
                let url = format!("http://{}:{port}", from.ip());
                if let Ok(mut slot) = shared.lock() {
                    slot.beacons.insert(node_id, (url, Instant::now()));
                }
            }
        })
        .ok();
}

fn beacon_port(text: &str) -> Option<u16> {
    json_number(text, "\"port\":")
}

fn beacon_node_id(text: &str) -> Option<u64> {
    json_number(text, "\"node_id\":")
}

fn beacon_fleet(text: &str) -> String {
    let key = "\"fleet\":";
    let Some(rest) = text.split(key).nth(1) else {
        return DEFAULT_FLEET.to_string();
    };
    let rest = rest.trim_start();
    if !rest.starts_with('"') {
        return DEFAULT_FLEET.to_string();
    }
    let name: String = rest.chars().skip(1).take_while(|c| *c != '"').collect();
    normalize_fleet(&name)
}

fn json_number<T: std::str::FromStr>(text: &str, key: &str) -> Option<T> {
    let rest = text.split(key).nth(1)?;
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_beacon_fields() {
        let text = r#"{"service":"makepad-asset-ai","node_id":42,"port":8767,"fleet":"game"}"#;
        assert_eq!(beacon_port(text), Some(8767));
        assert_eq!(beacon_node_id(text), Some(42));
        assert_eq!(beacon_fleet(text), "game");
    }

    #[test]
    fn missing_fleet_is_default() {
        let text = r#"{"service":"makepad-asset-ai","node_id":42,"port":8767}"#;
        assert_eq!(beacon_fleet(text), DEFAULT_FLEET);
    }

    #[test]
    fn seeds_dedup() {
        seed_bases(["http://10.0.0.123:8123/".into(), "http://10.0.0.123:8123".into()]);
        let bases = live_bases();
        assert_eq!(
            bases.iter().filter(|b| *b == "http://10.0.0.123:8123").count(),
            1
        );
    }
}
