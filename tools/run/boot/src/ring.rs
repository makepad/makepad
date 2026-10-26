//! The JIT ring: a small ring in the running app's caption that answers "is the UI waiting on
//! the JIT?". No text in it. The run shows it only past a real wait -- the UI blocked on the JIT
//! for 100 ms (postframe::BLOCK_SHOW_NS), or makepad rebuilding itself -- never for short first
//! compiles or background work alone (hal_run `Watch`). While it shows and the JIT builds anything
//! (lazy compiles, builds ahead, O2/O3 tier-up, the settle rebuild, warm-cache links) a bright
//! head with a fading tail turns around it, faster the more builds finish a second; work the program waits for (lazy
//! compiles, builds ahead, a UI block) turns bright and fast, background tier-up slower and dimmer,
//! but it always moves: nothing static means busy. Under the head a progress arc from the top
//! covers the builds finished of those queued since the JIT got busy (when a queue says). Once
//! nothing has been built for half a second the ring fades out and is gone: nothing static means
//! idle either. A program stopped on invalidated code (amber) or a panic (red) shows a solid dot
//! that stays while the program is stopped on it. Hovering it shows a tooltip with the live numbers;
//! it is display only: a click, a scroll or a drag over it is the app's. Placement follows the
//! caption the app draws (`WindowCaption`):
//! just left of window buttons at the caption's right (Windows, Linux), else ~12 pt from the
//! window's right edge (macOS), vertically centred. It is a badge of the window
//! (`window_kind::BADGE` with `window_style::BADGE_HOVER`): the HAL composes it over the app's
//! frames, which neither wait for it nor pay for it; session 0 sees the pointer hover over it,
//! the input itself stays the app's. The tooltip draws in the same badge layer while hovered. Every value
//! eases on this session's clock; the ring is the boot document's `jit_ring_px` (GPU), one quad.
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
/// time constants (s): the progress arc, the foreground / background blend, the head's pace, the fades
const FILL: f32 = 0.25;
const BLEND: f32 = 0.3;
const PACE: f32 = 0.4;
const SHOW: f32 = 0.12;
const HIDE: f32 = 0.15;
/// the head's turns a second with work the program waits for and with background work, at a
/// trickle; up to twice that as the build rate rises to RATE_TOP builds a second (logarithmic);
/// reduce motion: a third of it, still moving
const SPIN_FG: f32 = 0.9;
const SPIN_BG: f32 = 0.4;
const RATE_TOP: f32 = 1000.0;
/// the ring draws at most this often
pub const FRAME: Duration = Duration::from_nanos(33_333_333);

/// What the program is stopped on (a solid dot that stays while it is).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alert {
    /// a stale access: code the JIT invalidated was run (amber)
    Stale,
    /// a panic (red)
    Panic,
    /// rebuilding makepad for a change it cannot patch failed: the app runs the previous code (amber)
    RebuildFailed,
}

/// What the run sees of the JIT (hal_run): it is busy (a ring without a `Want` fades out).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Want {
    /// builds finished out of the queues since the JIT got busy, and builds still queued or
    /// running (ahead, O2, O3)
    pub done: u64,
    pub pending: u64,
    /// of the pending: functions queued or building ahead, O2 / verified O1 builds, O3 builds
    pub ahead: u64,
    pub o2: u64,
    pub o3: u64,
    /// a program thread compiles lazily (the UI thread's JIT duty cycle is up, or first compiles
    /// finished) without a queue to show
    pub lazy: bool,
    /// the UI is blocked on the JIT right now
    pub block: bool,
    /// the settle rebuild runs
    pub settling: bool,
    /// of the builds a second, warm-cache links (no compile)
    pub linking: bool,
    /// builds finished a second (smoothed)
    pub rate: f32,
    /// how long the JIT has been busy
    pub busy: Duration,
    /// makepad rebuilds itself in the background (1) or waits for the app to restart into the
    /// rebuilt binary (2), for a change the running process cannot take
    pub rebuild: u8,
}

impl Want {
    /// Work the program waits for (bright and fast); else background tier-up (slower, dimmer).
    fn foreground(&self) -> bool {
        self.ahead > 0 && !self.linking || self.lazy || self.block
    }

    /// The builds finished of those queued since the JIT got busy (None: no queue says).
    fn progress(&self) -> Option<f32> {
        let total = self.done + self.pending;
        (self.pending > 0 && total > 0).then(|| self.done as f32 / total as f32)
    }
}

/// One frame of the ring: its eased values (document `jit_ring_px`).
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// the progress arc from the top (0..1; 0: no queue says)
    pub fill: f32,
    /// 0 work the program waits for .. 1 background tier-up (dimmer); 2 the amber dot, 3 the red one
    pub mode: f32,
    pub opacity: f32,
    /// the head's turn so far (turns)
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
    /// 0 foreground .. 1 background, eased
    back: f32,
    /// the head's turns a second, eased
    pace: f32,
    opacity: f32,
    spin: f32,
    hover: f32,
    /// the pointer is on the ring since
    pub hovered: Option<Instant>,
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
            back: 0.0,
            pace: 0.0,
            opacity: 0.0,
            spin: 0.0,
            hover: 0.0,
            hovered: None,
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
            self.fill = want.and_then(|w| w.progress()).unwrap_or(0.0);
            self.back = want.is_some_and(|w| !w.foreground()) as u8 as f32;
            self.pace = want.map_or(0.0, |w| pace(&w));
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
        let hover = self.hovered.is_some() as u8 as f32;
        self.hover = ease(self.hover, hover, dt, 0.06, 0.12);
        if let Some(w) = self.want {
            let back = (!w.foreground()) as u8 as f32;
            self.back = ease(self.back, back, dt, BLEND, BLEND);
            // no queue says: the progress arc goes (the head alone turns)
            self.fill = ease(self.fill, w.progress().unwrap_or(0.0), dt, FILL, FILL);
            self.pace = ease(self.pace, pace(&w), dt, PACE, PACE);
        }
        // the head always turns while the ring shows (fading out too); the dot stands still
        let pace = self.pace * if reduce_motion { 0.33 } else { 1.0 };
        self.spin = (self.spin + dt * pace) % 1000.0;
        // a dot that goes (the stop ended, nothing else to show) fades out as the dot
        let alert = self.alert.or(self.want.is_none().then_some(self.last_alert).filter(|_| self.dot));
        let mode = match alert {
            Some(Alert::Stale | Alert::RebuildFailed) => 2.0,
            Some(Alert::Panic) => 3.0,
            None => self.back,
        };
        Look { fill: self.fill, mode, opacity: self.opacity, spin: self.spin, hover: self.hover }
    }

}

/// The head's turns a second for this work: a base by kind, up to twice it with the build rate.
fn pace(w: &Want) -> f32 {
    let base = if w.foreground() { SPIN_FG } else { SPIN_BG };
    let boost = ((1.0 + w.rate.max(0.0)).log10() / (1.0 + RATE_TOP).log10()).clamp(0.0, 1.0);
    base * (1.0 + boost)
}

/// What the JIT does, what is queued, how fast it goes, how long it has been at it (the ring's
/// tooltip and the run's inspect view), or what the program is stopped on.
pub fn summary(want: Option<Want>, alert: Option<Alert>) -> String {
    if let Some(alert) = alert {
        return match alert {
            Alert::Stale => format!("paused on invalidated code · makepad inspect {} panics", std::process::id()),
            Alert::Panic => format!("paused on a panic · makepad inspect {} panics", std::process::id()),
            Alert::RebuildFailed => "rebuilding makepad failed · the app runs the previous code (see the terminal)".into(),
        };
    }
    let Some(w) = want else { return "settled".into() };
    let mut parts = Vec::new();
    match w.rebuild {
        1 => parts.push("rebuilding makepad".to_string()),
        2 => parts.push("restarting into the rebuilt makepad".to_string()),
        _ => {}
    }
    if w.block {
        parts.push("UI waiting on the JIT".to_string());
    }
    if w.ahead > 0 && w.linking {
        // the queue ahead is warm-cache links (a warm start's manifest): nothing is compiled
        parts.push(format!("installing {} cached fns", w.ahead));
    } else if w.ahead > 0 {
        parts.push(format!("compiling {} fns", w.ahead));
    } else if w.lazy || w.block {
        parts.push("compiling".to_string());
    }
    if w.o2 > 0 {
        parts.push(format!("O2 queue {}", w.o2));
    }
    if w.o3 > 0 {
        parts.push(format!("O3 queue {}", w.o3));
    }
    if w.settling {
        parts.push("settle rebuild".to_string());
    }
    if parts.is_empty() {
        parts.push(if w.linking { "warm install" } else { "finishing" }.to_string());
    } else if w.linking && w.ahead == 0 {
        parts.push("warm install".to_string());
    }
    if let Some(p) = w.progress() {
        // the last word gets the percentage ("warm install 30%", "O3 queue 40 · 62%")
        parts.push(format!("{:.0}%", p * 100.0));
    }
    if w.rate >= 1.0 {
        parts.push(format!("{:.0} fns/s", w.rate));
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
    fn ring_badge_stays_in_the_room_apps_keep_free() {
        // apps keep `CAPTION_BADGE_ROOM` free left of the window buttons, else at the right edge
        let room = makepad_hal::CAPTION_BADGE_ROOM;
        let r = Ring::place([800.0, 600.0], caption([0.0, 0.0, 800.0, 29.0], [662.0, 0.0, 138.0, 29.0]));
        assert!(r[0] >= 662.0 - room && r[0] + r[2] <= 662.0);
        let r = Ring::place([900.0, 600.0], caption([0.0, 0.0, 900.0, 28.0], [7.0, 6.0, 54.0, 16.0]));
        assert!(r[0] >= 900.0 - room && r[0] + r[2] <= 900.0);
    }

    #[test]
    fn tooltip_stays_in_the_window() {
        let ring = Ring::place([300.0, 200.0], None);
        let tip = Ring::place_tip(ring, 400.0, [300.0, 200.0]);
        assert_eq!(tip[0], 4.0);
        assert!(tip[1] >= ring[1] + ring[3]);
    }

    /// Steps the ring `n` times 50 ms apart with `want`; the last look.
    fn run(ring: &mut Ring, want: Want, n: usize) -> Look {
        for _ in 0..n {
            ring.set(Some(want), None);
            ring.last -= Duration::from_millis(50);
            ring.look(false);
        }
        ring.last -= Duration::from_millis(50);
        ring.look(false)
    }

    #[test]
    fn the_progress_fills_as_the_queue_drains_and_the_head_always_turns() {
        let mut ring = Ring::new();
        ring.window = Some(Id::new(1, 1));
        ring.set(Some(Want { pending: 100, ahead: 100, ..Default::default() }), None);
        let a = ring.look(false);
        assert!(a.mode < 0.5 && a.fill < 0.1, "{a:?}");
        let b = run(&mut ring, Want { done: 90, pending: 10, ahead: 10, rate: 200.0, ..Default::default() }, 40);
        assert!(b.fill > 0.8 && b.mode < 0.05, "{b:?}");
        // background tier-up: dimmer, still turning
        let bg = Want { done: 12, pending: 40, o3: 40, rate: 5.0, ..Default::default() };
        let c = run(&mut ring, bg, 40);
        assert!(c.mode > 0.95 && (c.fill - 12.0 / 52.0).abs() < 0.02, "{c:?}");
        let d = run(&mut ring, bg, 4);
        assert!(d.spin > c.spin + 0.05, "the head stood still: {c:?} {d:?}");
        assert_eq!(ring.tip_text(), "O3 queue 40 · 23% · 5 fns/s · 0.0 s");
        // a lazy compile without a queue: no progress arc, the head turns fast
        let e = run(&mut ring, Want { lazy: true, rate: 30.0, ..Default::default() }, 40);
        assert!(e.fill < 0.02 && e.mode < 0.05, "{e:?}");
        assert!(ring.pace > SPIN_FG, "{}", ring.pace);
        ring.set(None, Some(Alert::Stale));
        assert_eq!(ring.look(false).mode, 2.0);
    }

    #[test]
    fn faster_work_turns_faster_background_slower() {
        let fg_slow = pace(&Want { ahead: 5, rate: 1.0, ..Default::default() });
        let fg_fast = pace(&Want { ahead: 5, rate: 500.0, ..Default::default() });
        let bg = pace(&Want { o3: 5, rate: 500.0, ..Default::default() });
        assert!(fg_fast > fg_slow && bg < fg_fast && bg > 0.3, "{fg_slow} {fg_fast} {bg}");
    }

    #[test]
    fn the_tooltip_says_what_runs() {
        let w = Want { done: 30, pending: 70, ahead: 70, rate: 120.0, busy: Duration::from_millis(1800), ..Default::default() };
        assert_eq!(summary(Some(w), None), "compiling 70 fns · 30% · 120 fns/s · 1.8 s");
        let w = Want { done: 3, pending: 7, linking: true, ahead: 7, ..Default::default() };
        assert_eq!(summary(Some(w), None), "installing 7 cached fns · 30% · 0.0 s");
        let w = Want { settling: true, busy: Duration::from_secs(2), ..Default::default() };
        assert_eq!(summary(Some(w), None), "settle rebuild · 2.0 s");
        assert_eq!(summary(None, None), "settled");
    }
}
