//! Browser-side fleet discovery surface.
//!
//! A web client cannot listen for LAN UDP beacons. Operator-provided HTTP
//! bases remain visible, while discovery itself resolves to an empty roster.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const DEFAULT_FLEET: &str = "default";

static BASES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn bases() -> &'static Mutex<Vec<String>> {
    BASES.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn start_listening() {}

pub fn normalize_fleet(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_FLEET.to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

pub fn resolve_wanted_fleet(configured: &str) -> String {
    if configured.trim().is_empty() {
        DEFAULT_FLEET.to_string()
    } else {
        normalize_fleet(configured)
    }
}

pub fn set_wanted_fleet(_name: impl AsRef<str>) {}

pub fn seed_bases(items: impl IntoIterator<Item = String>) {
    let mut slot = bases().lock().unwrap();
    for item in items {
        let item = item.trim().trim_end_matches('/').to_string();
        if !item.is_empty() && !slot.iter().any(|base| base == &item) {
            slot.push(item);
        }
    }
}

pub fn live_bases_within(_grace: Duration) -> Vec<String> {
    live_bases()
}

pub fn live_bases() -> Vec<String> {
    bases().lock().unwrap().clone()
}
