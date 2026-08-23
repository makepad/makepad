//! The multi-stage post chain: an ordered list of render-to-texture passes
//! run after the scene pass, exactly the gauss-stack idiom (one `DrawPass` +
//! `DrawList2d` + `RenderBGRAu8` texture per sub-pass, begun/ended
//! sequentially inside one widget draw). In effect-channel mode the chain
//! runs directly on the channel's content texture (`engine: "screen"`).
//!
//! Stage kinds (documents declare them in `stages: [...]`, see `mod.rs`):
//!   * `feedback` — ping-pong pair; the previous FRAME's stage output is
//!     re-projected (zoom/rotate) under the fresh input. Classic VJ trails.
//!   * `bloom` — bright-pass + down/up pyramid + screen composite.
//!   * `blur` — the same pyramid without threshold or composite.
//!   * `tiltshift` — pyramid blur mixed back by a screen-Y focus ramp.
//!   * warp modes (`kaleido`, `chroma`, `glitch`, `radial_blur`, …) — one
//!     fullscreen warp pass each.
//!   * `hold` — frame latch: grab the incoming frame on a beat slot or a
//!     rising trigger and hold it over the live one (freeze, scanline
//!     delay, strip delay).
//!
//! Numeric stage parameters arrive RESOLVED per frame (the binding layer
//! evaluates them in `view.rs`), so a kaleidoscope can spin on the bar.
//! Deep mips are only ever tent-upsampled one doubling at a time on the way
//! back — never bilinear-stretched (the repo's blur law).
//!
//! # THE COLD-START LAW (the one cross-document leak this file can have)
//!
//! Most stages write every texture they read inside the same frame, so they
//! are self-contained. Two do not: `feedback` samples the PREVIOUS frame's
//! output and `hold` samples the frame it latched EARLIER. On the first
//! frame after [`PostChain::configure`] those textures have never been
//! written by this chain — and `configure` cannot clear them: it allocates
//! out of the platform texture pool, which hands back a slot the last owner
//! left dirty. Sampling them there composites the PREVIOUS DOCUMENT'S
//! picture into this one, which is exactly the "a document's frame depends
//! on the document shown before it" bug.
//!
//! The fix is structural, not a clear: a stage that samples its own texture
//! carries a [`ColdStart`], and while it is cold it does not read that
//! texture AT ALL — feedback binds the live input in its place and runs
//! with the trail amount forced to zero, hold takes a fresh grab first.
//! [`ColdStart::new`] is the only constructor and it starts cold, so a
//! stage added later cannot forget to opt in.

use super::doc::{StageCfg, WarpMode};
use super::shaders::*;
use makepad_widgets::*;

const MAX_LEVELS: usize = 4;

/// One stage's parameters for THIS frame (bindings already evaluated).
#[derive(Clone, Copy, Default)]
pub struct ResolvedStage {
    pub p: [f32; 4],
}

/// Cold-start guard for a stage that samples its OWN texture across frames
/// (see "THE COLD-START LAW" at the top of this file). Starts cold by
/// construction — there is no `Default`, and no way to build a warm one —
/// and [`ColdStart::warm`] may only be called once the stage has written
/// that texture itself, in this chain, this run.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ColdStart(bool);

impl ColdStart {
    pub(crate) fn new() -> Self {
        Self(true)
    }

    /// True while the stage's own texture still holds whatever the texture
    /// pool left there — a previous document's frames, or noise.
    pub(crate) fn is_cold(&self) -> bool {
        self.0
    }

    /// Call AFTER the stage has written its own texture this frame.
    pub(crate) fn warm(&mut self) {
        self.0 = false;
    }
}

struct SubPass {
    pass: DrawPass,
    draw_list: DrawList2d,
    texture: Texture,
}

impl SubPass {
    fn new(cx: &mut Cx, name: &str) -> Self {
        let pass = DrawPass::new_with_name(cx, name);
        let draw_list = DrawList2d::new(cx);
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        pass.set_color_texture(
            cx,
            &texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        Self { pass, draw_list, texture }
    }

    /// Run one full-screen-quad pass at `size` (logical units), letting
    /// `draw` bind its textures/uniforms and draw the quad.
    fn run(&mut self, cx: &mut Cx2d, size: Vec2d, draw: &mut dyn FnMut(&mut Cx2d, Rect)) {
        let dpi = cx.current_dpi_factor();
        self.pass.set_size(cx, size);
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(dpi));
        self.draw_list.begin_always(cx);
        let pass_size = cx.current_pass_size();
        cx.begin_root_turtle(pass_size, Layout::flow_overlay());
        draw(cx, Rect { pos: dvec2(0.0, 0.0), size: pass_size });
        cx.end_pass_sized_turtle();
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
    }
}

enum StageRt {
    Feedback { ping: SubPass, pong: SubPass, flip: bool, cold: ColdStart },
    Pyramid { downs: Vec<SubPass>, ups: Vec<SubPass>, mix: Option<SubPass>, kind: PyramidKind },
    Warp { mode: WarpMode, sub: SubPass },
    Hold {
        /// The latched frame.
        latch: SubPass,
        /// The composite of latch + live.
        out: SubPass,
        /// Beats per automatic re-grab (0 = trigger only).
        beats: f32,
        /// 0 = rows, 1 = columns (the shader's u_hold.w).
        axis: f32,
        /// Last beat slot grabbed, so a slot boundary grabs exactly once.
        slot: f64,
        /// Last trigger level, for rising-edge detection.
        trig: f32,
        cold: ColdStart,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum PyramidKind {
    Bloom,
    Blur,
    Tiltshift,
}

#[derive(Default)]
pub struct PostChain {
    stages: Vec<StageRt>,
}

impl PostChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// (Re)build the runtime for a document's stage list.
    ///
    /// Every self-sampling stage comes back COLD (see the cold-start law at
    /// the top of this file): the textures allocated here are pool slots
    /// that may still hold the OUTGOING document's pixels, and nothing in
    /// this call clears them, so the first run must not read them.
    pub fn configure(&mut self, cx: &mut Cx, stages: &[StageCfg]) {
        self.stages.clear();
        for (i, cfg) in stages.iter().enumerate() {
            match cfg {
                StageCfg::Feedback { .. } => self.stages.push(StageRt::Feedback {
                    ping: SubPass::new(cx, &format!("vjfx_fb{i}_a")),
                    pong: SubPass::new(cx, &format!("vjfx_fb{i}_b")),
                    flip: false,
                    cold: ColdStart::new(),
                }),
                StageCfg::Hold { beats, vertical, .. } => self.stages.push(StageRt::Hold {
                    latch: SubPass::new(cx, &format!("vjfx_hold{i}_l")),
                    out: SubPass::new(cx, &format!("vjfx_hold{i}_o")),
                    beats: if beats.is_finite() { beats.clamp(0.0, 64.0) } else { 4.0 },
                    axis: if *vertical { 1.0 } else { 0.0 },
                    slot: f64::NAN,
                    trig: 0.0,
                    cold: ColdStart::new(),
                }),
                StageCfg::Bloom { levels, .. } => {
                    self.push_pyramid(cx, i, *levels, PyramidKind::Bloom);
                }
                StageCfg::Blur { levels } => {
                    self.push_pyramid(cx, i, *levels, PyramidKind::Blur);
                }
                StageCfg::Tiltshift { levels, .. } => {
                    self.push_pyramid(cx, i, *levels, PyramidKind::Tiltshift);
                }
                StageCfg::Warp { mode, .. } => self.stages.push(StageRt::Warp {
                    mode: *mode,
                    sub: SubPass::new(cx, &format!("vjfx_warp{i}")),
                }),
            }
        }
    }

    fn push_pyramid(&mut self, cx: &mut Cx, i: usize, levels: usize, kind: PyramidKind) {
        let levels = levels.clamp(1, MAX_LEVELS);
        let mix = match kind {
            PyramidKind::Blur => None,
            _ => Some(SubPass::new(cx, &format!("vjfx_pyr{i}_mix"))),
        };
        self.stages.push(StageRt::Pyramid {
            downs: (0..levels)
                .map(|l| SubPass::new(cx, &format!("vjfx_pyr{i}_d{l}")))
                .collect(),
            ups: (0..levels)
                .map(|l| SubPass::new(cx, &format!("vjfx_pyr{i}_u{l}")))
                .collect(),
            mix,
            kind,
        });
    }

    fn level_size(base: Vec2d, level: usize) -> Vec2d {
        let s = (1usize << (level + 1)) as f64;
        dvec2((base.x / s).max(1.0), (base.y / s).max(1.0))
    }

    /// Run the chain on `input`, returning the final output texture.
    /// `size` is the scene pass size in logical units; `resolved` carries
    /// this frame's evaluated stage parameters (one entry per stage);
    /// `beat` = (beat, phase, pulse, time) for the warp shader.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        cx: &mut Cx2d,
        input: Texture,
        size: Vec2d,
        resolved: &[ResolvedStage],
        beat: [f32; 4],
        draws: &mut PostDraws,
    ) -> Texture {
        let mut current = input;
        for (i, stage) in self.stages.iter_mut().enumerate() {
            let p = resolved.get(i).copied().unwrap_or_default().p;
            match stage {
                StageRt::Feedback { ping, pong, flip, cold } => {
                    let (dst, src) = if *flip { (ping, pong) } else { (pong, ping) };
                    *flip = !*flip;
                    draws.feedback.draw_vars.set_texture(0, &current);
                    // COLD-START LAW: on the first frame after configure,
                    // `src` has never been written by this chain. Bind the
                    // live frame there instead and zero the trail amount —
                    // the stale slot is never sampled, and the output is
                    // exactly the fresh frame (which is what a warm chain
                    // with a black history produces anyway, so a correctly
                    // pooled texture renders identically either way).
                    let p = if cold.is_cold() {
                        draws.feedback.draw_vars.set_texture(1, &current);
                        [0.0, p[1], p[2], p[3]]
                    } else {
                        draws.feedback.draw_vars.set_texture(1, &src.texture);
                        p
                    };
                    draws.feedback.draw_vars.set_uniform(cx, live_id!(u_fb), &p);
                    let d = &mut *draws.feedback;
                    dst.run(cx, size, &mut |cx, rect| {
                        d.draw_abs(cx, rect);
                    });
                    // `dst` is written now; the next frame reads it as src.
                    cold.warm();
                    current = dst.texture.clone();
                }
                StageRt::Pyramid { downs, ups, mix, kind } => {
                    let base = current.clone();
                    let levels = downs.len();
                    // p for bloom: [threshold, strength, _, _];
                    // tiltshift: [focus 0..1, width, strength?, _]; blur: [].
                    let threshold = if *kind == PyramidKind::Bloom { p[0] } else { 0.0 };
                    let mut src = current.clone();
                    for (l, sub) in downs.iter_mut().enumerate() {
                        let thresh = if l == 0 { threshold } else { 0.0 };
                        draws.down.draw_vars.set_texture(0, &src);
                        draws.down.draw_vars.set_uniform(
                            cx,
                            live_id!(u_bright),
                            &[thresh, 0.35, 0.0, 0.0],
                        );
                        let d = &mut *draws.down;
                        sub.run(cx, Self::level_size(size, l), &mut |cx, rect| {
                            d.draw_abs(cx, rect);
                        });
                        src = sub.texture.clone();
                    }
                    // Back up one doubling at a time to full size.
                    for (k, sub) in ups.iter_mut().enumerate() {
                        let level_back = levels - 1 - k;
                        let up_size = if level_back == 0 {
                            size
                        } else {
                            Self::level_size(size, level_back - 1)
                        };
                        draws.up.draw_vars.set_texture(0, &src);
                        let d = &mut *draws.up;
                        sub.run(cx, up_size, &mut |cx, rect| {
                            d.draw_abs(cx, rect);
                        });
                        src = sub.texture.clone();
                    }
                    match kind {
                        PyramidKind::Blur => {
                            current = src;
                        }
                        PyramidKind::Bloom => {
                            let mix = mix.as_mut().unwrap();
                            draws.bloom_mix.draw_vars.set_texture(0, &base);
                            draws.bloom_mix.draw_vars.set_texture(1, &src);
                            draws.bloom_mix.draw_vars.set_uniform(
                                cx,
                                live_id!(u_mix),
                                &[p[1], 0.0, 0.0, 0.0],
                            );
                            let d = &mut *draws.bloom_mix;
                            mix.run(cx, size, &mut |cx, rect| {
                                d.draw_abs(cx, rect);
                            });
                            current = mix.texture.clone();
                        }
                        PyramidKind::Tiltshift => {
                            let mix = mix.as_mut().unwrap();
                            draws.tilt_mix.draw_vars.set_texture(0, &base);
                            draws.tilt_mix.draw_vars.set_texture(1, &src);
                            draws.tilt_mix.draw_vars.set_uniform(
                                cx,
                                live_id!(u_tilt),
                                &[p[0], p[1].max(0.01), 0.0, 0.0],
                            );
                            let d = &mut *draws.tilt_mix;
                            mix.run(cx, size, &mut |cx, rect| {
                                d.draw_abs(cx, rect);
                            });
                            current = mix.texture.clone();
                        }
                    }
                }
                StageRt::Warp { mode, sub } => {
                    draws.warp.draw_vars.set_texture(0, &current);
                    draws.warp.draw_vars.set_uniform(
                        cx,
                        live_id!(u_warp),
                        &[*mode as i32 as f32, p[0], p[1], beat[3]],
                    );
                    draws.warp.draw_vars.set_uniform(
                        cx,
                        live_id!(u_beat),
                        &[beat[0], beat[1], beat[2], size.x as f32 / size.y.max(1.0) as f32],
                    );
                    let d = &mut *draws.warp;
                    sub.run(cx, size, &mut |cx, rect| {
                        d.draw_abs(cx, rect);
                    });
                    current = sub.texture.clone();
                }
                StageRt::Hold { latch, out, beats, axis, slot, trig, cold } => {
                    // p = (mix, bands, stagger, trigger level).
                    let level = if p[3].is_finite() { p[3] } else { 0.0 };
                    let edge = level >= 0.5 && *trig < 0.5;
                    *trig = level;
                    // Beat-slot boundary: floor of the song position over
                    // the grab period. Compared, never accumulated, so a
                    // tempo change or a cue jump lands on the next slot
                    // instead of drifting.
                    let now = if *beats > 0.0 {
                        (beat[0] as f64 / *beats as f64).floor()
                    } else {
                        f64::NAN
                    };
                    let slot_moved = now.is_finite() && *slot != now;
                    *slot = now;
                    // COLD-START LAW: a cold latch holds the previous
                    // document's frame, so the first run ALWAYS grabs.
                    if cold.is_cold() || edge || slot_moved {
                        // The grab is this same shader with mix 0 — a copy.
                        // Both slots take the live frame so the pass can
                        // never sample the texture it renders into.
                        draws.hold.draw_vars.set_texture(0, &current);
                        draws.hold.draw_vars.set_texture(1, &current);
                        draws.hold.draw_vars.set_uniform(
                            cx,
                            live_id!(u_hold),
                            &[0.0, 1.0, 0.0, *axis],
                        );
                        draws.hold.draw_vars.set_uniform(cx, live_id!(u_hbeat), &beat);
                        let d = &mut *draws.hold;
                        latch.run(cx, size, &mut |cx, rect| {
                            d.draw_abs(cx, rect);
                        });
                        cold.warm();
                    }
                    draws.hold.draw_vars.set_texture(0, &current);
                    draws.hold.draw_vars.set_texture(1, &latch.texture);
                    draws.hold.draw_vars.set_uniform(
                        cx,
                        live_id!(u_hold),
                        &[p[0], p[1], p[2], *axis],
                    );
                    draws.hold.draw_vars.set_uniform(cx, live_id!(u_hbeat), &beat);
                    let d = &mut *draws.hold;
                    out.run(cx, size, &mut |cx, rect| {
                        d.draw_abs(cx, rect);
                    });
                    current = out.texture.clone();
                }
            }
        }
        current
    }
}

/// The post-quad draw shaders, borrowed from the widget for one chain run.
pub struct PostDraws<'a> {
    pub feedback: &'a mut DrawVjFxFeedback,
    pub down: &'a mut DrawVjFxDown,
    pub up: &'a mut DrawVjFxUp,
    pub bloom_mix: &'a mut DrawVjFxBloomMix,
    pub tilt_mix: &'a mut DrawVjFxTiltMix,
    pub warp: &'a mut DrawVjFxWarp,
    pub hold: &'a mut DrawVjFxHold,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE COLD-START LAW, pinned where it can be checked without a GPU.
    ///
    /// `PostChain::configure` re-allocates its stage textures out of the
    /// platform pool and cannot clear them, so a self-sampling stage that
    /// read its own texture on the first frame would composite the
    /// PREVIOUS document's picture. Every such stage owns a `ColdStart`;
    /// it exists only in the cold state until the stage has written that
    /// texture itself. There is deliberately no `Default` and no
    /// `ColdStart(false)` constructor — a stage added later gets the safe
    /// state or does not compile.
    #[test]
    fn cold_start_begins_cold_and_warms_exactly_once() {
        let mut c = ColdStart::new();
        assert!(c.is_cold(), "a freshly configured stage must start COLD");
        c.warm();
        assert!(!c.is_cold());
        c.warm();
        assert!(!c.is_cold(), "warming twice must stay warm");
        assert_ne!(ColdStart::new(), ColdStart(false));
    }
}
