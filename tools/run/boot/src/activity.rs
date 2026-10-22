//! The JIT activity badge: a small, dim glittering galaxy in the bottom-right corner of the
//! running app's window while the JIT works (O3 builds, functions built ahead, the UI thread's
//! lazy compiles). Its sparkle, brightness and spin follow how much is in flight -- calm for a
//! trickle, livelier for a burst -- and it fades out when the JIT is idle; on a true UI block
//! (the UI thread stuck on the JIT, no frame) it grows into the full stall galaxy and shrinks back
//! once frames resume. No numbers. It is a badge of the window (`window_kind::BADGE`): the window
//! system composites it over the app, so the app's frames neither wait for it nor pay for it, and
//! it takes no input. The run says what is going on (`Want`); every value eases on this session's
//! clock, so nothing ever jumps. The galaxy is the boot document's `activity_galaxy_px` (GPU);
//! a frame is one quad.
use makepad_hal::Id;
use std::time::{Duration, Instant};

/// The badge's side and its distance to the window's bottom-right corner (points).
pub const SIDE: f32 = 64.0;
pub const MARGIN: f32 = 6.0;
/// the galaxy grows inside the badge from its bottom-right corner, this far in (points)
const INSET: f32 = 4.0;
/// the galaxy's diameter (points): calm (a trickle), lively (a burst), on a UI block (its outer
/// ring of particles only shows on a block)
const CALM: f32 = 18.0;
const LIVELY: f32 = 26.0;
const BLOCKED: f32 = SIDE - INSET;
/// time constants of the easing (s): the work in flight, the UI block, the fades
const LEVEL_RISE: f32 = 0.8;
const LEVEL_FALL: f32 = 1.6;
const BLOCK_RISE: f32 = 0.18;
const BLOCK_FALL: f32 = 0.45;
const SHOW: f32 = 0.5;
const HIDE: f32 = 0.9;
/// turns a second: idle drift, more with the work in flight, more on a block
const SPIN: f32 = 0.012;
const SPIN_LEVEL: f32 = 0.05;
const SPIN_BLOCK: f32 = 0.025;
/// the glitter's pace (radians a second): idle, and more with the work in flight
const BEAT: f32 = 0.9;
const BEAT_LEVEL: f32 = 2.6;
/// the badge draws at most this often
pub const FRAME: Duration = Duration::from_nanos(33_333_333);

/// What the run sees of the JIT (hal_run): `level` 0..1, how much is in flight (a look, not a
/// measurement), and whether the UI is blocked on the JIT right now.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Want {
    pub level: f32,
    pub block: bool,
}

/// One frame of the badge: the galaxy's square in the badge (points) and its eased values.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    pub rect: [f32; 4],
    pub level: f32,
    pub block: f32,
    pub opacity: f32,
    /// the galaxy's turn so far (turns) and its glitter's phase so far (radians): both only ever
    /// advance, at eased speeds
    pub spin: f32,
    pub beat: f32,
}

pub struct Activity {
    /// the badge window while it exists, and the slot it reuses (a new generation each time)
    pub window: Option<Id>,
    pub slot: Option<Id>,
    /// where it was placed last (window points)
    pub placed: [f32; 4],
    want: Option<Want>,
    level: f32,
    block: f32,
    opacity: f32,
    spin: f32,
    beat: f32,
    last: Instant,
    pub next_frame: Instant,
}

fn ease(x: f32, target: f32, dt: f32, rise: f32, fall: f32) -> f32 {
    let tau = if target > x { rise } else { fall };
    x + (target - x) * (1.0 - (-dt / tau).exp())
}

impl Activity {
    pub fn new() -> Self {
        Self {
            window: None,
            slot: None,
            placed: [0.0; 4],
            want: None,
            level: 0.0,
            block: 0.0,
            opacity: 0.0,
            spin: 0.0,
            beat: 0.0,
            last: Instant::now(),
            next_frame: Instant::now(),
        }
    }

    /// The run's wish this turn (`None`: the JIT is idle, the badge fades out).
    pub fn set(&mut self, want: Option<Want>) {
        if self.window.is_none() && want.is_some() {
            // a new badge starts from nothing
            self.level = 0.0;
            self.block = 0.0;
            self.opacity = 0.0;
            self.spin = 0.0;
            self.last = Instant::now();
            self.next_frame = Instant::now();
        }
        self.want = want;
    }

    pub fn wanted(&self) -> bool {
        self.want.is_some()
    }

    /// The badge has faded out and nothing is wanted: it can go.
    pub fn done(&self) -> bool {
        self.want.is_none() && self.opacity < 0.004
    }

    /// Where the badge goes on a window of `size` points.
    pub fn place(size: [f32; 2]) -> [f32; 4] {
        [size[0] - SIDE - MARGIN, size[1] - SIDE - MARGIN, SIDE, SIDE]
    }

    /// This frame's look (advances the easing).
    pub fn look(&mut self, reduce_motion: bool) -> Look {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let want = self.want.unwrap_or_default();
        let shown = if self.want.is_some() { 1.0 } else { 0.0 };
        self.level = ease(self.level, want.level.clamp(0.0, 1.0), dt, LEVEL_RISE, LEVEL_FALL);
        self.block = ease(self.block, if want.block { 1.0 } else { 0.0 }, dt, BLOCK_RISE, BLOCK_FALL);
        self.opacity = ease(self.opacity, shown, dt, SHOW, HIDE);
        let speed = SPIN + SPIN_LEVEL * self.level + SPIN_BLOCK * self.block;
        // (unwrapped: its arms turn at different rates, so no whole turn brings them back)
        self.spin += dt * speed * if reduce_motion { 0.3 } else { 1.0 };
        self.beat = (self.beat + dt * (BEAT + BEAT_LEVEL * self.level.max(self.block))) % (1000.0 * std::f32::consts::TAU);
        let calm = CALM + (LIVELY - CALM) * self.level;
        let side = calm + (BLOCKED - calm) * self.block;
        let at = SIDE - INSET - side;
        Look {
            rect: [at, at, side, side],
            level: self.level,
            block: self.block,
            opacity: self.opacity,
            spin: self.spin,
            beat: self.beat,
        }
    }
}
