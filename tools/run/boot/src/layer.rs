//! The window layer: the Ignition Field over the running app, composed by this session (the
//! HAL's `window_overlay` with a layer texture). The app's frames go to its retained image, which
//! the HAL copies into `APP` at the start of each of our frames; we draw the window.
//!
//! The sequence: the boot field (the app's first frames already land under it, never on the
//! screen); at the first present a Gaussian-blurred app fades in behind the still-working galaxy
//! (its stars glinting, the warm-up arc advancing with the functions built ahead); when the app
//! is ready (the JIT's readiness signal) the arc closes, the sharp app fades in and the galaxy
//! goes. Later, a stall of the UI thread on JIT compiles (a tab switch) veils the app the same
//! way: its frozen frame fades into the blur under the glinting galaxy, and the sharp app fades
//! back in once it presents again. Every fade runs on this session's clock, never the app's.
use makepad_hal::Id;
use std::time::Instant;

/// The app's image (the window's pixel size), its half and two quarter-size blur stages.
pub const APP: u32 = 20;
pub const HALF: u32 = 21;
pub const QUARTER_X: u32 = 22;
pub const QUARTER: u32 = 23;

/// The blurred app's darkening under the galaxy.
pub const BLUR_DIM: f32 = 0.55;
/// The blur's sigma, in quarter-size texels (one pass per direction).
pub const SIGMA: f32 = 4.0;

const FADE_IN: f32 = 0.5;
const LIFT: f32 = 1.1;
/// a stall's veil: in over the frozen app, out as the app presents again
const STALL_IN: f32 = 0.15;
const STALL_OUT: f32 = 0.3;

/// What the run asks the layer to show (hal_run, from the JIT's signals).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Want {
    /// the boot field; the app's first frames go under it
    Boot,
    /// after the first present, not ready yet: functions built ahead so far and still queued
    WarmUp { done: u64, queued: u64 },
    /// ready: the arc closes, the sharp app comes in, the galaxy goes
    Ready,
    /// the UI thread stalled on JIT compiles after the warm-up: still stalled, or it presented
    Stall { stalled: bool },
}

/// What one frame of the layer draws.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// the blurred app under the field (drawn opaque when > 0)
    pub blur: bool,
    /// the field's backdrop opacity over the blurred app
    pub backdrop: f32,
    pub galaxy: f32,
    /// the sharp app's opacity, over everything
    pub sharp: f32,
    /// the warm-up arc (-1: none) and the seconds since ready (-1: not yet)
    pub progress: f32,
    pub resolve: f32,
    /// the HUD, the frame's brackets and the swap ring (the warm-up; a stall shows the galaxy only)
    pub hud: bool,
    /// the layer has faded out: it can go
    pub done: bool,
}

pub struct Layer {
    /// the HAL layer is on, with the APP texture of this size and generation
    pub on: bool,
    pub size: [u32; 2],
    pub gen: u32,
    want: Option<Want>,
    warm_at: Option<Instant>,
    ready_at: Option<Instant>,
    /// the arc, never going back
    progress: f32,
    /// a stall's veil, 0 (the sharp app) to 1 (the blurred app under the galaxy)
    veil: f32,
    last: Instant,
    /// the app frames blurred last (the blur is made again only for a new app frame)
    pub blurred: Option<u64>,
    pub app_frames: u64,
    /// functions built ahead after the first present (the HUD)
    pub ahead: u64,
    /// the last frame's look had faded out
    done: bool,
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The veil going (`u` from 0, the blurred app under the galaxy, to 1, the sharp app): the sharp
/// app comes in over it as the galaxy goes over `galaxy_out` of the way (the ready lift, and a
/// stall's veil both ways).
fn unveil(look: &mut Look, u: f32, galaxy_out: [f32; 2]) {
    look.sharp = smooth(0.0, 1.0, u);
    look.galaxy = 1.0 - smooth(galaxy_out[0], galaxy_out[1], u);
}

impl Layer {
    pub fn new() -> Self {
        Self {
            on: false,
            size: [0; 2],
            gen: 0,
            want: None,
            warm_at: None,
            ready_at: None,
            progress: 0.0,
            veil: 0.0,
            last: Instant::now(),
            blurred: None,
            app_frames: 0,
            ahead: 0,
            done: false,
        }
    }

    pub fn texture(&self, index: u32) -> Id {
        Id::new(index, self.gen.max(1))
    }

    pub fn want(&self) -> Option<Want> {
        self.want
    }

    /// The run's wish for this turn. A new boot starts the sequence again.
    pub fn set(&mut self, want: Option<Want>) {
        match want {
            Some(Want::Boot) | None => {
                self.warm_at = None;
                self.ready_at = None;
                self.progress = 0.0;
            }
            Some(Want::WarmUp { done, queued }) => {
                self.warm_at.get_or_insert_with(Instant::now);
                self.ahead = done;
                if done + queued > 0 {
                    self.progress = self.progress.max(done as f32 / (done + queued) as f32);
                }
            }
            Some(Want::Ready) => {
                self.ready_at.get_or_insert_with(Instant::now);
            }
            Some(Want::Stall { .. }) => {
                if !matches!(self.want, Some(Want::Stall { .. })) {
                    self.veil = 0.0;
                    self.last = Instant::now();
                }
            }
        }
        if self.want.as_ref().map(std::mem::discriminant) != want.as_ref().map(std::mem::discriminant) {
            self.done = false;
        }
        self.want = want;
    }

    /// Whether the last frame drawn had faded out.
    pub fn peek_done(&self) -> bool {
        self.done
    }

    /// This frame's look (advances the fades).
    pub fn look(&mut self, reduce_motion: bool) -> Look {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let since = |at: Option<Instant>| at.map(|t| now.duration_since(t).as_secs_f32());
        let mut look = Look {
            blur: false,
            backdrop: 1.0,
            galaxy: 1.0,
            sharp: 0.0,
            progress: -1.0,
            resolve: -1.0,
            hud: true,
            done: false,
        };
        match self.want {
            None | Some(Want::Boot) => {}
            Some(Want::WarmUp { .. }) | Some(Want::Ready) => {
                // the blurred app replaces the deep field behind the galaxy
                if let Some(t) = since(self.warm_at) {
                    look.blur = true;
                    look.backdrop = 1.0 - smooth(0.0, FADE_IN, t);
                }
                look.progress = self.progress;
                if let Some(t) = since(self.ready_at) {
                    // the arc closes and flares, then the sharp app comes in as the galaxy goes
                    let lift = if reduce_motion { LIFT * 0.5 } else { LIFT };
                    look.resolve = t;
                    look.progress = 1.0;
                    unveil(&mut look, (t / lift - 0.3) / 0.7, [-0.05 / 0.7, 0.6 / 0.7]);
                    look.done = t >= lift;
                }
            }
            Some(Want::Stall { stalled }) => {
                // the warm-up's veil without its HUD, coming and going with the stall
                let rate = if stalled { dt / STALL_IN } else { -dt / STALL_OUT };
                self.veil = (self.veil + rate).clamp(0.0, 1.0);
                look.blur = true;
                look.backdrop = 0.0;
                look.hud = false;
                unveil(&mut look, 1.0 - self.veil, [0.0, 0.8]);
                look.done = !stalled && self.veil <= 0.0;
            }
        }
        self.done = look.done;
        look
    }
}
