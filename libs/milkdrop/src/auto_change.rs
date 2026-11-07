//! The automatic preset change, on the music's bars.
//!
//! A host that changes presets by itself every so many seconds asks
//! [`AutoChange::due`] once a frame with the view's beat
//! ([`crate::MilkdropViewRef::beat`]). Once the interval is up the change
//! waits for the next bar line, so a new preset starts on the one rather
//! than in the middle of a phrase. Music without a beat (the tracker does
//! not follow one) has no bar lines to wait for; there the change comes at
//! the interval.

use makepad_beat::BeatFrame;

#[derive(Clone, Copy, Debug, Default)]
pub struct AutoChange {
    /// When the preset showing was put up, host seconds.
    since: f64,
    /// The bar the last frame was in.
    bar: f64,
}

impl AutoChange {
    /// A preset was put up at `now`: the interval counts from here.
    pub fn restart(&mut self, now: f64) {
        self.since = now;
    }

    /// Whether to change now: `interval` seconds (0 = never) have passed
    /// since [`AutoChange::restart`], and a bar has just started, or the
    /// music has no beat. Call it every frame; it notes the bar each time.
    pub fn due(&mut self, now: f64, interval: f64, beat: &BeatFrame) -> bool {
        let new_bar = beat.bar_pos.floor() > self.bar.floor();
        self.bar = beat.bar_pos;
        interval > 0.0 && now - self.since >= interval && (!beat.locked || new_bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_change_waits_for_the_bar_and_comes_at_the_interval_without_a_beat() {
        let mut auto = AutoChange::default();
        auto.restart(0.0);
        let mut beat = BeatFrame { locked: true, bar_pos: 3.5, ..BeatFrame::default() };
        assert!(!auto.due(10.0, 15.0, &beat), "before the interval");
        // Past the interval, inside a bar: wait.
        beat.bar_pos = 7.6;
        assert!(auto.due(16.0, 15.0, &beat), "a bar line was crossed since the last frame");
        auto.restart(16.0);
        beat.bar_pos = 7.9;
        assert!(!auto.due(31.5, 15.0, &beat), "mid-bar: waits");
        beat.bar_pos = 8.02;
        assert!(auto.due(31.6, 15.0, &beat), "on the bar line");
        // No beat: at the interval.
        auto.restart(40.0);
        let free = BeatFrame { locked: false, bar_pos: 8.4, ..BeatFrame::default() };
        assert!(!auto.due(50.0, 15.0, &free));
        assert!(auto.due(55.0, 15.0, &free));
        // Never.
        assert!(!auto.due(1000.0, 0.0, &free));
    }
}
