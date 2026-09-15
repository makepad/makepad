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
use makepad_platform::thread::{CancellationToken, ThreadOptions, ThreadSpawner};
use makepad_platform::Cx;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DISCOVERY_PORT: u16 = 41830;
const BEACON_EXPIRY: Duration = Duration::from_secs(20);
pub const DEFAULT_FLEET: &str = "default";

/// One heard beacon: where the node serves, WHICH fleet it belongs to, and
/// when it was last heard. The fleet rides on every entry so `wanted` is a
/// READ-time filter: changing it can never leak nodes accepted for another
/// fleet (they simply stop matching), and beacons from other fleets keep
/// being tracked, so switching back is instant.
#[derive(Clone)]
struct Beacon {
    url: String,
    fleet: String,
    seen: Instant,
}

#[derive(Clone)]
struct Roster {
    wanted: String,
    static_bases: Vec<String>,
    beacons: HashMap<u64, Beacon>,
    /// When the listener was spawned: a roster younger than
    /// [`LISTENER_WARMUP`] with no beacons has simply not heard one yet.
    started: Instant,
}

impl Roster {
    /// Current fleet URLs: operator seeds plus live beacons whose fleet is
    /// the wanted one. Expired beacons are dropped as a side effect.
    fn collect_bases(&mut self) -> Vec<String> {
        self.beacons.retain(|_, b| b.seen.elapsed() < BEACON_EXPIRY);
        let mut out = Vec::new();
        let mut push = |url: &str| {
            if !url.is_empty() && !out.iter().any(|b| b == url) {
                out.push(url.to_string());
            }
        };
        for base in &self.static_bases {
            push(base);
        }
        for beacon in self.beacons.values() {
            if beacon.fleet == self.wanted {
                push(&beacon.url);
            }
        }
        out
    }
}

/// Boxes beacon every 2 s. A listener this old with an empty set is an
/// empty fleet, not a cold start.
const LISTENER_WARMUP: Duration = Duration::from_secs(8);

static ROSTER: OnceLock<Arc<Mutex<Roster>>> = OnceLock::new();
static LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

fn roster() -> Arc<Mutex<Roster>> {
    ROSTER
        .get_or_init(|| {
            let shared = Arc::new(Mutex::new(Roster {
                wanted: wanted_from_env(),
                static_bases: Vec::new(),
                beacons: HashMap::new(),
                started: Instant::now(),
            }));
            shared
        })
        .clone()
}

/// Spawn the beacon listener now rather than on the first probe. A server
/// that only started listening when the first chat arrived answered that
/// chat from an empty set — nothing can have been heard yet — and cached
/// the "unavailable" for its probe TTL, so the first message after every
/// launch failed on a healthy fleet.
pub fn start_listening(spawner: ThreadSpawner) {
    let shared = roster();
    if std::env::var_os("MAKEPAD_AI_NO_BEACON").is_some_and(|v| v == "1") {
        return;
    }
    if LISTENER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if start_listener(shared, &spawner).is_err() {
        LISTENER_STARTED.store(false, Ordering::Release);
    }
}

/// How long the listener has been up; `None` before it was started.
fn listener_age() -> Option<Duration> {
    ROSTER.get().map(|hold| hold.lock().unwrap().started.elapsed())
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

/// The fleet a server listens for. A configured name always wins; an empty
/// one follows the process-wide `MAKEPAD_AI_FLEET` — the same variable the
/// Asset UI's fleet panel and every asset-ai frontend filter on — and only
/// then falls back to [`DEFAULT_FLEET`].
///
/// This is what keeps an embedded server's chat on the SAME fleet as the
/// app hosting it. An empty config used to pin the broker to `default`,
/// which silently dropped every `gen` beacon the UI's own panel was
/// showing as 2/2 up: "the server's fleet LLM is unavailable: no fleet
/// nodes configured".
pub fn resolve_wanted_fleet(configured: &str) -> String {
    resolve_wanted_fleet_with(configured, std::env::var("MAKEPAD_AI_FLEET").ok().as_deref())
}

fn resolve_wanted_fleet_with(configured: &str, env: Option<&str>) -> String {
    if !configured.trim().is_empty() {
        return normalize_fleet(configured);
    }
    normalize_fleet(env.unwrap_or_default())
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

/// [`live_bases`], but a cold listener gets up to `grace` to hear its first
/// beacon. Only waits while the listener is younger than
/// [`LISTENER_WARMUP`] AND the set is empty; a warm listener or a seeded
/// fleet answers instantly, and a genuinely empty fleet never stalls a
/// probe past its first seconds of life.
pub fn live_bases_within(grace: Duration) -> Vec<String> {
    let warmup_left = listener_age()
        .map(|age| LISTENER_WARMUP.saturating_sub(age))
        .unwrap_or(LISTENER_WARMUP);
    wait_for_bases(grace.min(warmup_left), live_bases)
}

/// Poll `bases` until it is non-empty or `budget` runs out (100 ms steps).
fn wait_for_bases(budget: Duration, mut bases: impl FnMut() -> Vec<String>) -> Vec<String> {
    let deadline = Instant::now() + budget;
    loop {
        let found = bases();
        if !found.is_empty() || Instant::now() >= deadline {
            return found;
        }
        worker_wait(Duration::from_millis(100).min(deadline - Instant::now()));
    }
}

/// Current fleet URLs: operator seeds, env, and live beacons for this fleet.
pub fn live_bases() -> Vec<String> {
    seed_from_env();
    let hold = roster();
    let mut slot = hold.lock().unwrap();
    if slot.wanted.is_empty() {
        slot.wanted = wanted_from_env();
    }
    slot.collect_bases()
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

/// How long to wait before the first bind retry; doubles up to the cap.
const BIND_RETRY_START: Duration = Duration::from_secs(2);
const BIND_RETRY_CAP: Duration = Duration::from_secs(60);

fn start_listener(shared: Arc<Mutex<Roster>>, spawner: &ThreadSpawner) -> Result<(), ()> {
    spawner
        .spawn_worker(
            ThreadOptions { name: Some("asset-ai-fleet-listen".into()), ..Default::default() },
            move || {
            // Reuse-group bind: the VJ, the Asset UI and a game all listen
            // for the same fleet beacons on one machine. An exclusive bind
            // left every app but the first without a fleet (the asset-ui
            // logged "listener bind :41830 failed: Address already in use").
            //
            // A failed bind RETRIES with backoff instead of retiring the
            // thread: the OnceLock roster means this thread is the only
            // listener this process will ever have, so giving up on a
            // transient failure (a dying process still holding the port
            // exclusively, a firewall prompt) would disable discovery for
            // the process's whole life.
            let mut backoff = BIND_RETRY_START;
            let socket = loop {
                match makepad_asset_client::bind_reuse_udp(DISCOVERY_PORT) {
                    Ok(s) => break s,
                    Err(e) => {
                        // Loud: without this the only symptom is "no fleet
                        // nodes configured" with nothing in the log why.
                        eprintln!(
                            "chat fleet discovery: listener bind :{DISCOVERY_PORT} failed: {e} — \
                             retrying in {}s (seeded fleet nodes keep working meanwhile)",
                            backoff.as_secs()
                        );
                        worker_wait(backoff);
                        backoff = (backoff * 2).min(BIND_RETRY_CAP);
                    }
                }
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
                // Every asset-ai beacon is recorded WITH its fleet; wanted
                // filters at read time (`Roster::collect_bases`), so a
                // wanted change mid-life can neither leak another fleet's
                // nodes nor forget this one's.
                let fleet = beacon_fleet(text);
                let port = beacon_port(text).unwrap_or(8765);
                let node_id = beacon_node_id(text).unwrap_or(from.port() as u64);
                let url = format!("http://{}:{port}", from.ip());
                if let Ok(mut slot) = shared.lock() {
                    slot.beacons.insert(node_id, Beacon { url, fleet, seen: Instant::now() });
                }
            }
            },
        )
        .map(|handle| handle.detach())
        .map_err(|_| ())
}

fn worker_wait(duration: Duration) {
    let wait = CancellationToken::new();
    let _ = wait.wait_until(Cx::monotonic_now() + duration.as_secs_f64());
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
    fn configured_fleet_wins_else_env_else_default() {
        assert_eq!(resolve_wanted_fleet_with("", None), DEFAULT_FLEET);
        assert_eq!(resolve_wanted_fleet_with("", Some("")), DEFAULT_FLEET);
        assert_eq!(resolve_wanted_fleet_with("", Some("gen")), "gen");
        assert_eq!(resolve_wanted_fleet_with("  ", Some(" Gen ")), "gen");
        assert_eq!(resolve_wanted_fleet_with("game", Some("gen")), "game");
        assert_eq!(resolve_wanted_fleet_with(" Game ", None), "game");
    }

    #[test]
    fn wait_for_bases_returns_on_first_hit_or_budget() {
        // Two empty polls, then a node: returns the node, well inside budget.
        let mut polls = 0;
        let t0 = Instant::now();
        let found = wait_for_bases(Duration::from_secs(5), || {
            polls += 1;
            if polls < 3 { Vec::new() } else { vec!["http://n1:8123".to_string()] }
        });
        assert_eq!(found, vec!["http://n1:8123".to_string()]);
        assert_eq!(polls, 3);
        assert!(t0.elapsed() < Duration::from_secs(2));
        // Nothing ever arrives: gives up at the budget, empty.
        let t0 = Instant::now();
        let found = wait_for_bases(Duration::from_millis(250), Vec::new);
        assert!(found.is_empty());
        let waited = t0.elapsed();
        assert!(waited >= Duration::from_millis(250) && waited < Duration::from_secs(2), "{waited:?}");
        // Zero budget: exactly one poll, no sleep.
        let mut polls = 0;
        let found = wait_for_bases(Duration::ZERO, || { polls += 1; Vec::new() });
        assert!(found.is_empty());
        assert_eq!(polls, 1);
    }

    #[test]
    fn beacons_filter_by_fleet_at_read_and_never_leak_across_a_wanted_change() {
        let mut roster = Roster {
            wanted: "gen".into(),
            static_bases: vec!["http://seed:1".into()],
            beacons: HashMap::new(),
            started: Instant::now(),
        };
        roster.beacons.insert(1, Beacon { url: "http://gen-node:8123".into(), fleet: "gen".into(), seen: Instant::now() });
        roster.beacons.insert(2, Beacon { url: "http://game-node:8123".into(), fleet: "game".into(), seen: Instant::now() });
        let bases = roster.collect_bases();
        assert!(bases.contains(&"http://seed:1".to_string()));
        assert!(bases.contains(&"http://gen-node:8123".to_string()));
        assert!(!bases.contains(&"http://game-node:8123".to_string()), "{bases:?}");
        // The wanted fleet changes: the other fleet's node appears, the
        // old one disappears — no expiry wait, no leakage either way.
        roster.wanted = "game".into();
        let bases = roster.collect_bases();
        assert!(bases.contains(&"http://game-node:8123".to_string()));
        assert!(!bases.contains(&"http://gen-node:8123".to_string()), "{bases:?}");
        // An expired beacon of the RIGHT fleet is gone too.
        roster.beacons.get_mut(&2).unwrap().seen = Instant::now() - BEACON_EXPIRY * 2;
        assert_eq!(roster.collect_bases(), vec!["http://seed:1".to_string()]);
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
