//! Browser-side fleet discovery surface.
//!
//! A web client cannot listen for LAN UDP beacons. Operator-provided HTTP
//! bases remain visible, while discovery itself resolves to an empty roster.

use makepad_platform::thread::ThreadSpawner;
use std::cell::RefCell;
use std::time::Duration;

pub const DEFAULT_FLEET: &str = "default";

thread_local! {
    static BASES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn start_listening(_spawner: ThreadSpawner) {}

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
    BASES.with(|bases| {
        let mut slot = bases.borrow_mut();
        for item in items {
            let item = item.trim().trim_end_matches('/').to_string();
            if !item.is_empty() && !slot.iter().any(|base| base == &item) {
                slot.push(item);
            }
        }
    });
}

pub fn live_bases_within(_grace: Duration) -> Vec<String> {
    live_bases()
}

pub fn live_bases() -> Vec<String> {
    BASES.with(|bases| bases.borrow().clone())
}
