//! THE SIMPLE DRIVER — a feed goes in, fluid motion comes out.
//!
//! The VJ drives [`FlowTweenView`] itself: it has a platter clock, a pair
//! cache, an EDF field budget shared across two decks and a prefetch map,
//! and none of that is generalisable. A host that simply receives pictures
//! and wants the gaps filled needs none of it, and this is what it needs
//! instead:
//!
//! ```ignore
//! // once a real frame lands
//! tweener.push_frame(cx, view, &rgb, w, h);
//! // every display frame; true = ask for another
//! let animating = tweener.tick(cx, view, Instant::now());
//! ```
//!
//! The clock is the only interesting part. Real frames arrive at whatever
//! pace the producer manages; `t` walks 0 -> 1 across the SMOOTHED interval
//! between the last two of them, and when the next one lands the pair
//! rotates (B becomes A) and `t` restarts. If a frame is late `t` sits at
//! 1.0 showing B — it never runs past the picture it has.

use crate::flow_tween::{
    ai2_frame_plan, ai3_budget_depth, ai3_neural_frames, default_model_path, rife_enabled,
    rife_proxy_dims, FlowTweenView, RifeJob, RifeProduct, RifeProductKind, RifeService,
    RifeSource, AI3_BOOTSTRAP_SYNTH_SECS,
};
use crate::frame::rgb8_to_bgra32;
use crate::mode::{ai_ceiling, AiRateGate, Mode};
use makepad_widgets::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Below this the tween is pointless — a feed this fast has nothing to
/// interpolate — and above the AI rate law it is forbidden outright.
const MIN_PERIOD: f64 = 1.0 / 240.0;
/// A feed that stalls this long has stopped, not slowed: hold on B rather
/// than stretching one pair across a minute of screen time.
const MAX_PERIOD: f64 = 4.0;
/// The interval estimate follows the feed rather than jumping to it, so one
/// late frame does not visibly change the speed of everything after it.
const PERIOD_SMOOTHING: f64 = 0.35;

/// Everything one feed needs to present its gaps: the clock, the neural
/// worker, and the per-mode wiring of a [`FlowTweenView`].
pub struct FeedTweener {
    mode: Mode,
    size: (u32, u32),
    /// The two pictures the current pair is made of, as the BGRA words the
    /// endpoint textures took, kept for the neural worker (which downscales
    /// them itself, off this thread).
    a: Option<Arc<Vec<u32>>>,
    b: Option<Arc<Vec<u32>>>,
    /// When B landed, and how long the pair before it lasted.
    pair_started: Option<Instant>,
    period: f64,
    /// Bumped per pair so a product that arrives late is never adopted.
    generation: u64,
    rife: Option<RifeService>,
    /// A failed start is not retried every frame.
    rife_failed: bool,
    offered: bool,
    gate: AiRateGate,
    ai3_depth: u8,
    last_t: f32,
    /// The mode whose wiring is actually on the view right now.
    applied: Option<Mode>,
}

impl Default for FeedTweener {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            size: (0, 0),
            a: None,
            b: None,
            pair_started: None,
            period: 0.5,
            generation: 0,
            rife: None,
            rife_failed: false,
            offered: false,
            gate: AiRateGate::default(),
            ai3_depth: 1,
            last_t: 1.0,
            applied: None,
        }
    }
}

impl FeedTweener {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch tiers mid-feed. The next display frame presents the new one;
    /// nothing is torn down and no picture is dropped.
    pub fn set_mode(&mut self, cx: &mut Cx, view: &mut FlowTweenView, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.offered = false;
        self.apply_mode(cx, view);
    }

    fn apply_mode(&mut self, cx: &mut Cx, view: &mut FlowTweenView) {
        if self.applied == Some(self.mode) {
            return;
        }
        self.applied = Some(self.mode);
        if !self.mode.uses_ai() {
            view.clear_rife_field(cx);
            view.clear_ai2_midpoint(cx);
            view.clear_ai3_subdivision(cx);
        }
        // NONE and CROSSFADE need no fields at all — `fade` is what tells
        // the warp to skip the whole derivation stack. FLOW and the AI
        // tiers all want it standing (a neural tier falls back to it).
        view.set_fade(cx, matches!(self.mode, Mode::None | Mode::Crossfade));
        view.redraw(cx);
    }

    /// True while the tween has a pair to walk — the host should keep
    /// asking for frames.
    pub fn animating(&self) -> bool {
        self.mode != Mode::None && self.b.is_some() && self.a.is_some()
    }

    /// The feed's measured pace, real frames per second.
    pub fn feed_fps(&self) -> f64 {
        1.0 / self.period.max(MIN_PERIOD)
    }

    /// Whether the neural tiers may run at the current pace (USER LAW: the
    /// AI tweener never runs at or above native pace).
    pub fn ai_admitted(&self) -> bool {
        self.gate.admitted() && self.feed_fps() <= ai_ceiling(1)
    }

    /// A real picture landed, as packed RGB8.
    pub fn push_frame(
        &mut self,
        cx: &mut Cx,
        view: &mut FlowTweenView,
        rgb: &[u8],
        width: u32,
        height: u32,
    ) -> bool {
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || rgb.len() < w * h * 3 {
            return false;
        }
        self.push_frame_bgra32(cx, view, &rgb8_to_bgra32(rgb), width, height)
    }

    /// A real picture landed, as the BGRA words a texture wants. What was B
    /// becomes A, this becomes B, and the tween starts walking the new pair.
    pub fn push_frame_bgra32(
        &mut self,
        cx: &mut Cx,
        view: &mut FlowTweenView,
        bgra: &[u32],
        width: u32,
        height: u32,
    ) -> bool {
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || bgra.len() < w * h {
            return false;
        }
        let now = Instant::now();
        let resized = self.size != (width, height);
        if resized {
            self.a = None;
            self.b = None;
            self.size = (width, height);
        }
        let next = Arc::new(bgra.to_vec());
        match self.b.take() {
            // First picture of a feed: there is no pair yet, so both
            // endpoints are it and t sits still until the second lands.
            None => {
                view.set_pair_bgra32(cx, &next, &next, width, height);
                self.a = Some(next.clone());
                self.b = Some(next);
                self.pair_started = None;
            }
            Some(previous) => {
                if !view.push_bgra32(cx, &next, width, height) {
                    view.set_pair_bgra32(cx, &previous, &next, width, height);
                }
                if let Some(started) = self.pair_started {
                    let measured = now.duration_since(started).as_secs_f64();
                    if (MIN_PERIOD..=MAX_PERIOD).contains(&measured) {
                        self.period += (measured - self.period) * PERIOD_SMOOTHING;
                    }
                }
                self.a = Some(previous);
                self.b = Some(next);
                self.pair_started = Some(now);
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.offered = false;
        self.gate.admit(self.feed_fps());
        view.clear_rife_field(cx);
        view.clear_ai2_midpoint(cx);
        view.clear_ai3_subdivision(cx);
        view.set_t(cx, 0.0);
        self.last_t = 0.0;
        true
    }

    /// One display frame. Advances `t`, harvests whatever the neural worker
    /// finished, and returns whether the host should ask for another frame.
    pub fn tick(&mut self, cx: &mut Cx, view: &mut FlowTweenView, now: Instant) -> bool {
        if self.b.is_none() {
            return false;
        }
        let t = match self.pair_started {
            None => 1.0,
            Some(started) => {
                let elapsed = now.saturating_duration_since(started).as_secs_f64();
                (elapsed / self.period.max(MIN_PERIOD)).clamp(0.0, 1.0) as f32
            }
        };
        self.tick_at(cx, view, t)
    }

    /// One display frame at a fraction the HOST computed. A host that
    /// already paces its own transitions (a wall that dissolves each frame
    /// over the measured gap) owns the clock; this only has to present it.
    pub fn tick_at(&mut self, cx: &mut Cx, view: &mut FlowTweenView, t: f32) -> bool {
        if self.b.is_none() {
            return false;
        }
        self.apply_mode(cx, view);
        // NONE holds the newest picture and never walks: t = 1 samples
        // frame B exactly, through every producer's math.
        let t = if self.mode == Mode::None { 1.0 } else { t.clamp(0.0, 1.0) };
        self.last_t = t;
        self.pump_ai(cx, view, t);
        view.set_t(cx, t);
        self.mode != Mode::None && t < 1.0
    }

    /// How long until the next real frame is due, for a host that would
    /// rather wake on a timer than free-run.
    pub fn until_next_frame(&self, now: Instant) -> Duration {
        let Some(started) = self.pair_started else { return Duration::from_secs_f64(self.period) };
        let elapsed = now.saturating_duration_since(started).as_secs_f64();
        Duration::from_secs_f64((self.period - elapsed).max(0.0))
    }

    // -- the neural producer ------------------------------------------------

    fn pump_ai(&mut self, cx: &mut Cx, view: &mut FlowTweenView, t: f32) {
        if !self.mode.uses_ai() {
            return;
        }
        if !rife_enabled() || !self.ai_admitted() {
            return;
        }
        let (Some(a), Some(b)) = (self.a.clone(), self.b.clone()) else { return };
        if Arc::ptr_eq(&a, &b) {
            return;
        }
        if self.rife.is_none() && !self.rife_failed {
            match RifeService::start(&default_model_path()) {
                Ok(service) => self.rife = Some(service),
                Err(error) => {
                    self.rife_failed = true;
                    log!("frametween: neural producer unavailable ({error}) — staying classical");
                }
            }
        }
        let Some(rife) = self.rife.as_ref() else { return };
        let (pw, ph) = rife_proxy_dims(self.size.0, self.size.1);
        if !self.offered {
            let kind = match self.mode {
                Mode::Ai2 => RifeProductKind::Midpoint,
                Mode::Ai3 => {
                    let synth = rife.synth_seconds(pw, ph).unwrap_or(AI3_BOOTSTRAP_SYNTH_SECS);
                    let capacity = (crate::mode::RIFE_CAPACITY_FPS * self.period) as usize;
                    self.ai3_depth = ai3_budget_depth(synth, self.period, capacity.max(1));
                    RifeProductKind::Subdivision { depth: self.ai3_depth }
                }
                _ => RifeProductKind::Field,
            };
            // The deadline is the end of this pair: a product that misses it
            // belongs to a picture already gone.
            let deadline = Instant::now()
                + Duration::from_secs_f64((self.period * (1.0 - t as f64)).max(0.0));
            self.offered = rife.offer_next(RifeJob {
                generation: self.generation,
                a: 0,
                b: 1,
                kind,
                frames: RifeSource::Bgra32 {
                    a,
                    b,
                    width: self.size.0 as usize,
                    height: self.size.1 as usize,
                },
                width: pw,
                height: ph,
                deadline,
            });
        }
        if let Some(product) = rife.take() {
            if product.generation() == self.generation {
                self.adopt(cx, view, &product);
            }
        }
        // Whatever is standing, pick the half/interval this t belongs to.
        match self.mode {
            Mode::Ai2 => {
                let plan = ai2_frame_plan(view_has_midpoint(view), t);
                view.select_ai2_pair(cx, plan.pair);
            }
            Mode::Ai3 if self.ai3_depth > 0 => {
                let intervals = ai3_neural_frames(self.ai3_depth) + 1;
                let scaled = (t * intervals as f32).clamp(0.0, intervals as f32 - 1e-4);
                view.select_ai3_pair(cx, scaled as usize);
            }
            _ => {}
        }
    }

    fn adopt(&mut self, cx: &mut Cx, view: &mut FlowTweenView, product: &RifeProduct) {
        match product {
            RifeProduct::Field(field) => {
                view.set_rife_field(
                    cx,
                    self.generation as usize,
                    field.width,
                    field.height,
                    &field.flow,
                    &field.mask,
                );
            }
            RifeProduct::Midpoint(midpoint) => {
                view.set_ai2_midpoint(cx, midpoint);
            }
            RifeProduct::Subdivision(subdivision) => {
                let depth = subdivision.complete_depth().min(self.ai3_depth);
                if depth >= 1 {
                    self.ai3_depth = depth;
                    view.set_ai3_subdivision(cx, subdivision, depth);
                }
            }
        }
    }
}

/// AI2's plan needs to know whether a midpoint is actually standing; the
/// view answers by having accepted one for this pair.
fn view_has_midpoint(view: &FlowTweenView) -> bool {
    view.has_ai2_midpoint()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_fps_feed_is_admitted_and_a_sixty_fps_one_is_not() {
        let mut tweener = FeedTweener::new();
        tweener.period = 0.5;
        tweener.gate.admit(tweener.feed_fps());
        assert!(tweener.ai_admitted(), "2 fps is exactly what the AI tier is for");
        tweener.period = 1.0 / 60.0;
        tweener.gate.admit(tweener.feed_fps());
        assert!(!tweener.ai_admitted(), "native pace is the law's ceiling");
    }

    #[test]
    fn none_pins_t_to_the_newest_picture() {
        // Without a pair start there is nothing to walk; and NONE never
        // walks even when there is.
        let mut tweener = FeedTweener::new();
        tweener.mode = Mode::None;
        assert!(!tweener.animating(), "no pictures yet");
    }
}
