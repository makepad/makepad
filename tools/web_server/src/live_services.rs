//! Reserved ownership/status registry for the later radar, weather, and wind
//! backend lane. No live-data endpoint or upstream poller is implemented here.

use std::sync::RwLock;

#[derive(Clone, Copy, Debug)]
pub struct LiveServiceState {
    pub radar: &'static str,
    pub wind: &'static str,
}

impl Default for LiveServiceState {
    fn default() -> Self {
        Self { radar: "unavailable", wind: "unavailable" }
    }
}

pub struct LiveServiceRegistry {
    state: RwLock<LiveServiceState>,
}

impl Default for LiveServiceRegistry {
    fn default() -> Self {
        Self { state: RwLock::new(LiveServiceState::default()) }
    }
}

impl LiveServiceRegistry {
    pub fn state(&self) -> LiveServiceState {
        *self.state.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
