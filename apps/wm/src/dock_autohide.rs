//! Dock auto-hide: the floating dock slides off the bottom edge when it is
//! not in use, comes back when the pointer reaches the bottom edge of its
//! display, and leaves again a moment after the pointer does.
//!
//! The state machine is pure (the caller passes the clock and the facts), so
//! its timings and hysteresis are unit tests. It asks for frames only while
//! the dock slides and for one timer wake-up while a delay runs: an idle
//! desktop draws nothing.
use crate::motion::Ease;

/// The pointer must rest at the bottom edge this long before the dock slides in (s).
pub const REVEAL_DELAY: f64 = 0.15;
/// The pointer must be away this long before the dock slides out (s).
pub const HIDE_DELAY: f64 = 0.5;
/// One slide in or out (s).
pub const SLIDE_SECS: f64 = 0.25;
/// The band along the display's bottom edge that calls the hidden dock (px).
pub const EDGE: f64 = 2.0;
/// How far around the shown dock the pointer still counts as using it (px).
pub const KEEP_MARGIN: f64 = 24.0;

/// Where the pointer counts as "at the dock": the display's bottom edge and,
/// while the dock is out, the dock's own footprint (resting place).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub dock_left: f64,
    pub dock_right: f64,
    pub dock_top: f64,
}

impl Zone {
    /// Whether a pointer at `(x, y)` keeps (or calls) the dock. A hidden
    /// dock answers to the thin edge band over the whole display width; a
    /// shown one to its footprint with a margin, so leaving it takes a
    /// deliberate move, not a graze.
    pub fn over(&self, shown: bool, x: f64, y: f64) -> bool {
        if x < self.left || x > self.right || y > self.bottom + 1.0 {
            return false;
        }
        if y >= self.bottom - EDGE {
            return true;
        }
        shown
            && x >= self.dock_left - KEEP_MARGIN
            && x <= self.dock_right + KEEP_MARGIN
            && y >= self.dock_top - KEEP_MARGIN
    }
}

#[derive(Clone, Debug)]
pub struct DockAutohide {
    pub enabled: bool,
    /// Where the slide is heading: true = out.
    out: bool,
    /// A change of heart waiting for its delay: where to, when (clock s).
    pending: Option<(bool, f64)>,
    slide: Ease,
}

impl Default for DockAutohide {
    fn default() -> Self {
        DockAutohide { enabled: false, out: true, pending: None, slide: Ease::at(1.0, SLIDE_SECS) }
    }
}

impl DockAutohide {
    /// Switch the feature. Turning it on starts with the dock out and lets the
    /// usual delay take it away; turning it off brings it back out.
    pub fn set_enabled(&mut self, on: bool, now: f64) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        if on {
            self.out = true;
            self.slide.snap(1.0);
            self.pending = Some((false, now + HIDE_DELAY));
        } else {
            self.pending = None;
            self.out = true;
            self.slide.retarget(1.0);
        }
    }

    /// Call the hidden thing out for a moment (the dock's icon of an app
    /// that lives on a shelf was clicked): it slides out and, unless the
    /// pointer is using it by then, goes back after `hold` seconds.
    pub fn reveal(&mut self, now: f64, hold: f64) {
        if !self.enabled {
            return;
        }
        self.out = true;
        self.slide.retarget(1.0);
        self.pending = Some((false, now + hold));
    }

    /// The pointer moved, or a fact about "busy" changed. `over` is
    /// `Zone::over` for the current pointer; `busy` is a dock menu, a
    /// "+N" menu or any drag: nothing changes while it lasts.
    pub fn update(&mut self, now: f64, over: bool, busy: bool) {
        if !self.enabled || busy {
            self.pending = None;
            return;
        }
        if over == self.out {
            self.pending = None;
        } else if self.pending.map(|(to, _)| to) != Some(over) {
            let delay = if over { REVEAL_DELAY } else { HIDE_DELAY };
            self.pending = Some((over, now + delay));
        }
    }

    /// The clock moved: act on a delay that has run out.
    pub fn fire(&mut self, now: f64) {
        if let Some((to, at)) = self.pending {
            if now >= at {
                self.pending = None;
                self.out = to;
                self.slide.retarget(f64::from(to as u8));
            }
        }
    }

    /// When the next `fire` is due (clock s), if a delay runs.
    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, at)| at)
    }

    /// Advance a slide; true while it still moves.
    pub fn step(&mut self, dt: f64) -> bool {
        self.slide.step(dt)
    }

    pub fn sliding(&self) -> bool {
        !self.slide.settled()
    }

    /// How far out the dock is: 1 resting place, 0 fully away.
    pub fn out_fraction(&self) -> f64 {
        if self.enabled { self.slide.value().clamp(0.0, 1.0) } else { 1.0 }
    }

    /// Whether the dock is out, or sliding, so it is drawn and hit at all.
    pub fn drawn(&self) -> bool {
        !self.enabled || self.out_fraction() > 0.0
    }

    /// Whether the dock is at rest in its place (hits and layout agree).
    pub fn is_out(&self) -> bool {
        self.out
    }

    /// The share of the desk a maximised window leaves free: none while the
    /// feature is on (the dock overlays windows when it shows), else `reserve`.
    pub fn reserve(&self, reserve: f64) -> f64 {
        if self.enabled { 0.0 } else { reserve }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> Zone {
        Zone { left: 0.0, right: 1280.0, bottom: 800.0, dock_left: 400.0, dock_right: 880.0, dock_top: 712.0 }
    }

    /// Run the clock in 10 ms steps from `t0` to `t1`, firing delays and
    /// sliding, the way the app's timer and frames do.
    fn run(d: &mut DockAutohide, t0: f64, t1: f64) {
        let mut t = t0;
        while t < t1 {
            t += 0.01;
            d.fire(t);
            d.step(0.01);
        }
    }

    fn on() -> DockAutohide {
        let mut d = DockAutohide::default();
        d.set_enabled(true, 0.0);
        run(&mut d, 0.0, 1.5);
        d
    }

    #[test]
    fn enabling_hides_the_idle_dock_after_the_delay_and_the_slide() {
        let mut d = DockAutohide::default();
        d.set_enabled(true, 0.0);
        assert_eq!(d.out_fraction(), 1.0);
        run(&mut d, 0.0, 0.45);
        assert_eq!(d.out_fraction(), 1.0, "still waiting");
        run(&mut d, 0.45, 0.6);
        assert!(d.out_fraction() < 1.0 && d.out_fraction() > 0.0, "mid-slide");
        run(&mut d, 0.6, 1.0);
        assert_eq!(d.out_fraction(), 0.0);
        assert!(!d.drawn() && !d.sliding() && d.deadline().is_none(), "idle asks for nothing");
    }

    #[test]
    fn the_edge_reveals_after_a_short_delay_and_slides_in() {
        let mut d = on();
        d.update(2.0, true, false);
        assert_eq!(d.deadline(), Some(2.0 + REVEAL_DELAY));
        run(&mut d, 2.0, 2.1);
        assert_eq!(d.out_fraction(), 0.0, "not before the delay");
        run(&mut d, 2.1, 2.27);
        assert!(d.out_fraction() > 0.0 && d.out_fraction() < 1.0);
        run(&mut d, 2.27, 2.6);
        assert_eq!(d.out_fraction(), 1.0);
    }

    #[test]
    fn a_called_out_thing_slides_out_and_goes_back_after_the_hold_unless_used() {
        let mut d = on();
        assert_eq!(d.out_fraction(), 0.0);
        d.reveal(2.0, 1.5);
        // The pointer is elsewhere (it is on the dock): moving there keeps the hold.
        d.update(2.1, false, false);
        assert_eq!(d.deadline(), Some(3.5));
        run(&mut d, 2.0, 3.0);
        assert_eq!(d.out_fraction(), 1.0);
        run(&mut d, 3.0, 4.2);
        assert_eq!(d.out_fraction(), 0.0, "back after the hold");
        // Used meanwhile: stays.
        d.reveal(5.0, 1.5);
        d.update(5.5, true, false);
        run(&mut d, 5.0, 8.0);
        assert_eq!(d.out_fraction(), 1.0);
        // Off: nothing to call out.
        let mut off = DockAutohide::default();
        off.reveal(0.0, 1.0);
        assert!(off.deadline().is_none());
    }

    #[test]
    fn a_graze_of_the_edge_does_not_reveal() {
        let mut d = on();
        d.update(2.0, true, false);
        d.update(2.05, false, false);
        assert_eq!(d.deadline(), None);
        run(&mut d, 2.05, 3.0);
        assert_eq!(d.out_fraction(), 0.0);
    }

    #[test]
    fn leaving_hides_after_the_longer_delay_and_returning_cancels_it() {
        let mut d = on();
        d.update(2.0, true, false);
        run(&mut d, 2.0, 3.0);
        assert_eq!(d.out_fraction(), 1.0);
        d.update(3.0, false, false);
        run(&mut d, 3.0, 3.4);
        assert_eq!(d.out_fraction(), 1.0, "0.4 s away is not enough");
        d.update(3.4, true, false);
        run(&mut d, 3.4, 4.5);
        assert_eq!(d.out_fraction(), 1.0, "coming back kept it");
        d.update(4.5, false, false);
        run(&mut d, 4.5, 5.4);
        assert_eq!(d.out_fraction(), 0.0);
    }

    #[test]
    fn repeated_moves_outside_do_not_restart_the_hide_delay() {
        let mut d = on();
        d.update(2.0, true, false);
        run(&mut d, 2.0, 3.0);
        d.update(3.0, false, false);
        let due = d.deadline();
        d.update(3.2, false, false);
        assert_eq!(d.deadline(), due);
    }

    #[test]
    fn a_menu_or_drag_holds_the_dock_where_it_is() {
        let mut d = on();
        d.update(2.0, true, false);
        run(&mut d, 2.0, 3.0);
        d.update(3.0, false, true);
        run(&mut d, 3.0, 6.0);
        assert_eq!(d.out_fraction(), 1.0, "a menu is open: it stays");
        d.update(6.0, false, false);
        run(&mut d, 6.0, 6.9);
        assert_eq!(d.out_fraction(), 0.0);
        // A window drag over the hidden edge does not pop the dock up.
        d.update(7.0, true, true);
        run(&mut d, 7.0, 8.0);
        assert_eq!(d.out_fraction(), 0.0);
    }

    #[test]
    fn a_change_of_mind_mid_slide_bends_without_a_jump() {
        let mut d = on();
        d.update(2.0, true, false);
        run(&mut d, 2.0, 2.3);
        let mid = d.out_fraction();
        assert!(mid > 0.0 && mid < 1.0);
        d.update(2.3, false, false);
        run(&mut d, 2.3, 2.31);
        assert!((d.out_fraction() - mid).abs() < 0.2, "continuous");
    }

    #[test]
    fn turning_it_off_brings_the_dock_back_and_gives_the_space_back() {
        let mut d = on();
        assert_eq!(d.reserve(88.0), 0.0);
        d.set_enabled(false, 5.0);
        assert_eq!(d.reserve(88.0), 88.0);
        run(&mut d, 5.0, 6.0);
        assert_eq!(d.out_fraction(), 1.0);
        assert!(d.drawn());
    }

    #[test]
    fn the_zone_has_an_edge_band_when_hidden_and_a_footprint_when_shown() {
        let z = zone();
        assert!(z.over(false, 20.0, 799.0), "edge band, any x of the display");
        assert!(!z.over(false, 600.0, 760.0), "the hidden dock's place is not the edge");
        assert!(z.over(true, 600.0, 760.0), "the shown dock's footprint");
        assert!(z.over(true, 380.0, 720.0), "its margin");
        assert!(!z.over(true, 100.0, 760.0), "beside the dock");
        assert!(!z.over(true, 600.0, 600.0), "above it");
        assert!(!z.over(false, 1300.0, 799.0), "another display");
    }
}
