//! The JIT ring: a small ring in the running app's caption that says what the JIT is doing.
//! No text in it. While functions are compiled (built ahead, or the UI thread's lazy compiles)
//! an arc spins and grows as the compile queue drains; while only background tier-up (O2/O3) is
//! left it is a thin, dimmer ring; once settled it fades out in about half a second and is gone.
//! A program stopped on invalidated code (amber) or a panic (red) shows a solid dot that stays
//! until it is clicked. Hovering it shows a tooltip with the numbers; a click is the run's
//! (`Event::RingClick`). Placement follows the caption the app draws (`WindowCaption`): just left
//! of window buttons at the caption's right (Windows, Linux), else ~12 pt from the window's right
//! edge (macOS), vertically centred. It is a badge of the window (`window_kind::BADGE` with
//! `window_style::BADGE_INPUT`): the window system composites it over the app's frames, which
//! neither wait for it nor pay for it; the pointer over it is session 0's, everywhere else the
//! app's. The tooltip is a second badge (no input) while hovered. Every value eases on this
//! session's clock; the ring is the boot document's `jit_ring_px` (GPU), a frame one quad.
use makepad_hal::{AppCaption, Id};
use std::time::{Duration, Instant};

/// The ring's diameter, the ring badge's side (it takes the pointer), the ring's distance to the
/// window's right edge (no window buttons at the right) and to window buttons at its right (pt).
pub const RING: f32 = 14.0;
pub const SIDE: f32 = 24.0;
const EDGE: f32 = 12.0;
const BUTTON_GAP: f32 = 8.0;
/// the caption's height when the app reports none (a caption row of the stock height)
const CAPTION: f32 = 28.0;
/// the tooltip: its height, padding, distance below the ring badge, font size (pt)
pub const TIP_H: f32 = 22.0;
pub const TIP_PAD: f32 = 8.0;
const TIP_GAP: f32 = 2.0;
pub const TIP_FONT: f32 = 11.0;
/// the pointer rests this long on the ring before the tooltip shows
const TIP_AFTER: Duration = Duration::from_millis(250);
/// time constants (s): the fill, the thin ring's blend, the fades
const FILL: f32 = 0.25;
const THIN: f32 = 0.3;
const SHOW: f32 = 0.12;
const HIDE: f32 = 0.15;
/// the arc's turns a second (reduce motion: a third of it)
const SPIN: f32 = 0.8;
/// the ring draws at most this often
pub const FRAME: Duration = Duration::from_nanos(33_333_333);

/// What the program is stopped on (a solid dot that stays until it is clicked).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alert {
    /// a stale access: code the JIT invalidated was run (amber)
    Stale,
    /// a panic (red)
    Panic,
}

/// What the run sees of the JIT (hal_run).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Want {
    /// functions compiled since this compile began, and still queued or building
    pub done: u64,
    pub queued: u64,
    /// the UI thread waits on the JIT (lazy compiles) without a queue to show
    pub lazy: bool,
    /// O2/O3 builds queued or running
    pub tier_up: u64,
    /// the UI is blocked on the JIT right now
    pub block: bool,
    /// how long the JIT has been busy
    pub busy: Duration,
}

impl Want {
    fn compiling(&self) -> bool {
        self.queued > 0 || self.lazy || self.block
    }
}

/// One frame of the ring: its eased values (document `jit_ring_px`).
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// how much of the ring the arc covers (0..1)
    pub fill: f32,
    /// 0 compiling .. 1 the thin tier-up ring; 2 the amber dot, 3 the red one
    pub mode: f32,
    pub opacity: f32,
    /// the arc's turn so far (turns)
    pub spin: f32,
    pub hover: f32,
}

pub struct Ring {
    /// the ring's badge (its place for the pointer) while it shows, and the slot it reuses (a
    /// new generation each time); the ring and its tooltip draw in the window's badge layer
    pub window: Option<Id>,
    pub slot: Option<Id>,
    /// the tooltip shows (its glyphs are pinned)
    pub tip: bool,
    /// where they were placed last (window points)
    pub placed: [f32; 4],
    pub tip_placed: [f32; 4],
    want: Option<Want>,
    alert: Option<Alert>,
    fill: f32,
    thin: f32,
    opacity: f32,
    spin: f32,
    hover: f32,
    /// the pointer is on the ring since, a press on it is held
    pub hovered: Option<Instant>,
    pub pressed: bool,
    /// the last alert, and whether the dot is what shows (it fades out as the dot)
    last_alert: Alert,
    dot: bool,
    last: Instant,
    pub next_frame: Instant,
}

fn ease(x: f32, target: f32, dt: f32, rise: f32, fall: f32) -> f32 {
    let tau = if target > x { rise } else { fall };
    x + (target - x) * (1.0 - (-dt / tau).exp())
}

impl Ring {
    pub fn new() -> Self {
        Self {
            window: None,
            slot: None,
            tip: false,
            placed: [0.0; 4],
            tip_placed: [0.0; 4],
            want: None,
            alert: None,
            fill: 0.0,
            thin: 0.0,
            opacity: 0.0,
            spin: 0.0,
            hover: 0.0,
            hovered: None,
            pressed: false,
            last_alert: Alert::Panic,
            dot: false,
            last: Instant::now(),
            next_frame: Instant::now(),
        }
    }

    /// The run's wish this turn: the JIT's work (`None`: settled) and what the program is
    /// stopped on.
    pub fn set(&mut self, want: Option<Want>, alert: Option<Alert>) {
        if self.window.is_none() && (want.is_some() || alert.is_some()) {
            // a new ring starts from nothing
            self.fill = 0.0;
            self.thin = want.is_some_and(|w| !w.compiling()) as u8 as f32;
            self.opacity = 0.0;
            self.spin = 0.0;
            self.hover = 0.0;
            self.last = Instant::now();
            self.next_frame = Instant::now();
        }
        if let Some(alert) = alert {
            self.last_alert = alert;
            self.dot = true;
        } else if want.is_some() {
            self.dot = false;
        }
        self.want = want;
        self.alert = alert;
    }

    pub fn wanted(&self) -> bool {
        self.want.is_some() || self.alert.is_some()
    }

    /// The ring has faded out and nothing is wanted: it can go.
    pub fn done(&self) -> bool {
        !self.wanted() && self.opacity < 0.004
    }

    /// The tooltip shows: the pointer has rested on the ring.
    pub fn tip_shown(&self) -> bool {
        self.window.is_some() && self.hovered.is_some_and(|t| t.elapsed() >= TIP_AFTER)
    }

    /// How soon the tooltip is due (the pointer rests on the ring, it does not show yet).
    pub fn tip_due(&self) -> Option<Duration> {
        self.hovered.map(|t| TIP_AFTER.saturating_sub(t.elapsed())).filter(|d| !d.is_zero())
    }

    /// Where the ring's badge goes on a window `size` points wide with the app's `caption`.
    pub fn place(size: [f32; 2], caption: Option<AppCaption>) -> [f32; 4] {
        let c = caption.unwrap_or_default();
        let [bx, by, bw, bh] = c.buttons;
        let buttons_right = bw > 0.0 && bh > 0.0 && bx + bw * 0.5 > size[0] * 0.5;
        let (x, y) = if buttons_right {
            // just left of the window buttons (minimise first), on their middle
            (bx - BUTTON_GAP - RING * 0.5, by + bh * 0.5)
        } else {
            let y = if c.height > 0.0 { c.top + c.height * 0.5 } else { CAPTION * 0.5 };
            (size[0] - EDGE - RING * 0.5, y)
        };
        let x = x.clamp(SIDE * 0.5, (size[0] - SIDE * 0.5).max(SIDE * 0.5));
        let y = y.max(SIDE * 0.5);
        [(x - SIDE * 0.5).round(), (y - SIDE * 0.5).round(), SIDE, SIDE]
    }

    /// Where the tooltip `width` points wide goes under the ring's badge at `ring`.
    pub fn place_tip(ring: [f32; 4], width: f32, size: [f32; 2]) -> [f32; 4] {
        let w = width + 2.0 * TIP_PAD;
        let right = (ring[0] + ring[2] * 0.5 + RING * 0.5 + TIP_PAD).min(size[0] - 4.0);
        let x = (right - w).max(4.0);
        [x.round(), ring[1] + ring[2] + TIP_GAP, w.ceil(), TIP_H]
    }

    /// The tooltip's line: what the JIT does, what is queued, how long it has been at it.
    pub fn tip_text(&self) -> String {
        summary(self.want, self.alert)
    }

    /// This frame's look (advances the easing).
    pub fn look(&mut self, reduce_motion: bool) -> Look {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let shown = self.wanted() as u8 as f32;
        self.opacity = ease(self.opacity, shown, dt, SHOW, HIDE);
        let hover = (self.hovered.is_some() || self.pressed) as u8 as f32;
        self.hover = ease(self.hover, hover, dt, 0.06, 0.12);
        if let Some(w) = self.want {
            let thin = (!w.compiling()) as u8 as f32;
            self.thin = ease(self.thin, thin, dt, THIN, THIN);
            let total = w.done + w.queued;
            let fill = if w.compiling() {
                if total > 0 { w.done as f32 / total as f32 } else { self.fill }
            } else {
                1.0
            };
            self.fill = ease(self.fill, fill, dt, FILL, FILL);
        }
        // the arc turns while compiling; the thin ring and the dot stand still
        let pace = SPIN * (1.0 - self.thin) * if reduce_motion { 0.33 } else { 1.0 };
        self.spin = (self.spin + dt * pace) % 1000.0;
        // a dot that goes (clicked, nothing else to show) fades out as the dot
        let alert = self.alert.or(self.want.is_none().then_some(self.last_alert).filter(|_| self.dot));
        let mode = match alert {
            Some(Alert::Stale) => 2.0,
            Some(Alert::Panic) => 3.0,
            None => self.thin,
        };
        Look { fill: self.fill, mode, opacity: self.opacity, spin: self.spin, hover: self.hover }
    }

}

/// What the JIT does, what is queued, how long it has been at it (the ring's tooltip and the
/// run's inspect view), or what the program is stopped on.
pub fn summary(want: Option<Want>, alert: Option<Alert>) -> String {
    if let Some(alert) = alert {
        return match alert {
            Alert::Stale => "paused on invalidated code · click to inspect".into(),
            Alert::Panic => "paused on a panic · click to inspect".into(),
        };
    }
    let Some(w) = want else { return "settled".into() };
    let mut parts = Vec::new();
    if w.block {
        parts.push("UI waiting on the JIT".to_string());
    }
    if w.queued > 0 {
        parts.push(format!("compiling {}", w.queued));
    } else if w.compiling() {
        parts.push("compiling".to_string());
    }
    if w.tier_up > 0 {
        parts.push(format!("O3 queue {}", w.tier_up));
    }
    if parts.is_empty() {
        parts.push("settling".to_string());
    }
    parts.push(format!("{:.1} s", w.busy.as_secs_f32()));
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caption(caption: [f32; 4], buttons: [f32; 4]) -> Option<AppCaption> {
        Some(AppCaption { top: caption[1], height: caption[3], buttons, dark: false })
    }

    #[test]
    fn macos_ring_sits_at_the_captions_right_edge() {
        // traffic lights at the left of a 28 pt caption
        let r = Ring::place([900.0, 600.0], caption([0.0, 0.0, 900.0, 28.0], [7.0, 6.0, 54.0, 16.0]));
        // ring's right edge 12 pt from the window's, centred on the caption
        assert_eq!(r[0] + r[2] * 0.5 + RING * 0.5, 900.0 - EDGE);
        assert_eq!(r[1] + r[3] * 0.5, 14.0);
    }

    #[test]
    fn windows_ring_sits_left_of_the_minimise_button() {
        // minimise / maximise / close, 46 x 29 each, at the right of a 29 pt caption
        let r = Ring::place([800.0, 600.0], caption([0.0, 0.0, 800.0, 29.0], [662.0, 0.0, 138.0, 29.0]));
        assert_eq!(r[0] + r[2] * 0.5 + RING * 0.5, 662.0 - BUTTON_GAP);
        assert_eq!(r[1] + r[3] * 0.5, 14.5f32.round().max(14.0));
    }

    #[test]
    fn no_caption_reported_keeps_the_top_right() {
        let r = Ring::place([400.0, 300.0], None);
        assert_eq!(r[0] + r[2] * 0.5 + RING * 0.5, 400.0 - EDGE);
        assert_eq!(r[1], 2.0);
    }

    #[test]
    fn tooltip_stays_in_the_window() {
        let ring = Ring::place([300.0, 200.0], None);
        let tip = Ring::place_tip(ring, 400.0, [300.0, 200.0]);
        assert_eq!(tip[0], 4.0);
        assert!(tip[1] >= ring[1] + ring[3]);
    }

    #[test]
    fn the_arc_fills_as_the_queue_drains_and_settles_thin() {
        let mut ring = Ring::new();
        ring.window = Some(Id::new(1, 1));
        ring.set(Some(Want { done: 0, queued: 100, ..Default::default() }), None);
        let a = ring.look(false);
        assert!(a.mode < 0.5 && a.fill < 0.1);
        for _ in 0..40 {
            ring.set(Some(Want { done: 90, queued: 10, ..Default::default() }), None);
            ring.last -= Duration::from_millis(50);
            ring.look(false);
        }
        let b = ring.look(false);
        assert!(b.fill > 0.8, "{b:?}");
        for _ in 0..40 {
            ring.set(Some(Want { tier_up: 40, ..Default::default() }), None);
            ring.last -= Duration::from_millis(50);
            ring.look(false);
        }
        let c = ring.look(false);
        assert!(c.mode > 0.95 && c.fill > 0.99, "{c:?}");
        assert_eq!(ring.tip_text(), "O3 queue 40 · 0.0 s");
        ring.set(None, Some(Alert::Stale));
        assert_eq!(ring.look(false).mode, 2.0);
    }
}
