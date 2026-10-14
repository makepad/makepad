//! The desktop's GPU memory comes first. When the HAL says the device is short of memory
//! for the UI (`Event::GpuPressure`), the WM, as the session owner, asks the local AI hub
//! for room through its existing hub link (`hub_control`), in steps:
//!
//! 1. a yield without unloading: the hub shrinks its chat model's context (KV cache) and
//!    drops idle models;
//! 2. still short a few seconds later: a yield that may unload the chat model;
//! 3. still short after that: nothing more to ask; the HAL's last resort (textures handed
//!    back to their apps to make again) carries on alone.
//!
//! The shortage ending (`wanted_mb == 0`) starts over, and tells the hub (an ask of 0 MB) so
//! its chat model grows back to its configured context when the budget allows. Plain state over event times, so it
//! is tested without a hub or a GPU.

/// Seconds a step is given to show in the device's free memory before the next one: the
/// hub rebuilding a 27B model at a smaller window takes a few seconds, and the HAL only
/// repeats a shortage that lasted 4 s.
pub const SETTLE_S: f64 = 12.0;
/// Asked for on top of the shortage, so the next allocation does not start it again.
pub const MARGIN_MB: u64 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Calm,
    Asked,
    AskedUnload,
    Exhausted,
}

/// A request to the hub: `mb` of GPU memory for the UI, the chat model unloaded only with
/// `unload`; the hub keeps `reserve_mb` free for the UI from now on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    pub mb: u64,
    pub unload: bool,
    pub reserve_mb: u64,
}

#[derive(Default)]
pub struct YieldPolicy {
    stage: Stage,
    since: f64,
}

impl YieldPolicy {
    /// One pressure event at `now` (seconds). Answers what to ask the hub, if anything.
    pub fn on_pressure(&mut self, wanted_mb: u32, reserve_mb: u32, now: f64) -> Option<Ask> {
        if wanted_mb == 0 {
            // The shortage is over: after the hub gave something, it may grow back
            // (an ask of 0 MB; the hub does it at its next idle moment).
            let asked = self.stage != Stage::Calm;
            self.stage = Stage::Calm;
            return asked.then_some(Ask { mb: 0, unload: false, reserve_mb: reserve_mb as u64 });
        }
        let ask = |unload| Ask { mb: wanted_mb as u64 + MARGIN_MB, unload, reserve_mb: reserve_mb as u64 };
        let settled = now - self.since >= SETTLE_S;
        let (next, out) = match self.stage {
            Stage::Calm => (Stage::Asked, Some(ask(false))),
            Stage::Asked if settled => (Stage::AskedUnload, Some(ask(true))),
            Stage::AskedUnload if settled => (Stage::Exhausted, None),
            _ => return None,
        };
        self.stage = next;
        self.since = now;
        out
    }

    /// Nothing more the hub can give for this shortage.
    pub fn exhausted(&self) -> bool {
        self.stage == Stage::Exhausted
    }
}

/// The notice for a hub report, when the user should know: the chat model went.
pub fn notice_for(report: &str) -> Option<&'static str> {
    if report.contains("unload chat model") {
        Some("AI model unloaded to free GPU memory")
    } else if report.contains(" context ") {
        Some("AI context shortened to free GPU memory")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_go_caches_then_unload_then_stop() {
        let mut p = YieldPolicy::default();
        assert_eq!(p.on_pressure(500, 1536, 0.0), Some(Ask { mb: 756, unload: false, reserve_mb: 1536 }));
        // repeats while the first step settles: nothing new
        assert_eq!(p.on_pressure(500, 1536, 2.0), None);
        // still short after it settled: the model may go
        assert_eq!(p.on_pressure(400, 1536, 12.5), Some(Ask { mb: 656, unload: true, reserve_mb: 1536 }));
        assert_eq!(p.on_pressure(400, 1536, 14.0), None);
        // still short after that too: nothing more to ask
        assert_eq!(p.on_pressure(400, 1536, 25.0), None);
        assert!(p.exhausted());
        assert_eq!(p.on_pressure(400, 1536, 60.0), None);
    }

    #[test]
    fn relief_starts_over() {
        let mut p = YieldPolicy::default();
        assert!(p.on_pressure(300, 1536, 0.0).is_some());
        assert_eq!(p.on_pressure(0, 1536, 1.0), Some(Ask { mb: 0, unload: false, reserve_mb: 1536 }), "the hub may grow back");
        assert_eq!(p.on_pressure(0, 1536, 1.5), None, "said once");
        // a new shortage begins with the gentle step again
        assert_eq!(p.on_pressure(200, 1536, 2.0).map(|a| a.unload), Some(false));
    }

    #[test]
    fn the_notice_names_what_the_user_loses() {
        assert_eq!(notice_for("yield 900 MB for the UI (unload allowed): unload idle kokoro (~400 MB), unload chat model qwen3.8-27b (~20878 MB); freed 21000 MB"),
            Some("AI model unloaded to free GPU memory"));
        assert_eq!(notice_for("yield 700 MB for the UI: qwen3.8-27b context 49152 -> 32768 (~578 MB); freed 590 MB"),
            Some("AI context shortened to free GPU memory"));
        assert_eq!(notice_for("yield 700 MB for the UI: unload idle kokoro (~400 MB); freed 380 MB"), None);
    }
}
