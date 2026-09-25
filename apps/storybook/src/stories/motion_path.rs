//! The motion paths page: a lead and a five-dot stagger trail follow one
//! `MotionPath` (GSAP's MotionPathPlugin, rebuilt in the tween engine), on
//! three shapes: a curve through points, a closed loop through points and
//! SVG path data with arcs.
//!
//! The readout under the transport is the page's evidence where no
//! screenshot is available: every line is a label with an id, rewritten on
//! each frame that changed something, so `/snap?q=path_readout` on the
//! `--remote` surface reads the playhead, the lead's position and angle
//! (and whether it landed exactly on the path's end), the path and the
//! setup.
use crate::makepad_widgets::tween::{
    parse_gsap_ease, prop, Easing, Emit, MotionPath, PathId, PathOpts, PropKey, PropTo, Seek,
    Stagger, TargetId, Targets, TimelineOpts, TweenHost, TweenId, TweenOpts, TweenValue, BIG,
};
use crate::makepad_widgets::tween::{PathPoint, Position};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryPathCanvasBase = #(StoryPathCanvas::register_widget(vm))
    /** The motion paths stage: a grid, the path, 20 ticks at equal arc lengths, the span's start and end, the trail and the lead (a dot, or an arrow when it auto-rotates), at the values the timeline last wrote. */
    mod.storybook.StoryPathCanvas = set_type_default() do mod.storybook.StoryPathCanvasBase{
        width: Fill
        height: 280
        grid_color: theme.color_outline_variant
        path_color: theme.color_on_surface_variant
        tick_color: theme.color_outline
        lead_color: theme.color_primary
        trail_color: theme.color_tertiary
        start_color: theme.color_secondary
        end_color: theme.color_error
        draw_panel +: {
            color: theme.color_surface_container_low
        }
        draw_dot +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                sdf.circle(c.x, c.y, min(c.x, c.y))
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_text +: {
            text_style: theme.font_body_s
            color: theme.color_text_meta
        }
    }

    let Readout = Label{
        width: Fill
        text: "-"
        draw_text +: {
            text_style: theme.font_code{font_size: theme.font_size_p}
            color: theme.color_text
        }
    }

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    let SpanSlider = Slider{
        width: 260.
        min: -0.5
        max: 1.5
        step: 0.01
        precision: 2
    }

    mod.storybook.StoryPathStageBase = #(StoryPathStage::register_widget(vm))
    /** A timeline whose lead and stagger trail follow one motion path, with its transport, its knobs and a text readout. */
    mod.storybook.StoryPathStage = set_type_default() do mod.storybook.StoryPathStageBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        /** added to the auto-rotate angle, in degrees (GSAP autoRotate: 90) -180..180 step 5 */
        rotate_offset: 0.0
        /** mirror every other iteration */
        yoyo: false
        /** extra iterations, -1 forever -1..5 step 1 */
        repeat: 0.0

        StoryRow{
            path_play := Button{text: "Play"}
            path_pause := Button{text: "Pause"}
            path_reverse := Button{text: "Reverse"}
            path_restart := Button{text: "Restart"}
            auto_rotate := CheckBox{text: "Auto-rotate" active: true}
        }
        scrub := Slider{
            width: Fill
            text: "Playhead (total progress)"
            min: 0.
            max: 1.
            step: 0.001
            default: 0.
            precision: 3
        }
        path_readout_time := Readout{}
        path_readout_pos := Readout{}
        path_readout_path := Readout{}
        path_readout_setup := Readout{}
        canvas := mod.storybook.StoryPathCanvas{}
        StoryRow{
            Caption{text: "Shape"}
            shape := SegmentedControl{options: ["Wave (thru)" "Loop (closed thru)" "SVG d (arcs)"] selected: 0}
        }
        StoryRow{
            span_start := SpanSlider{text: "start" default: 0.}
            span_end := SpanSlider{text: "end" default: 1.}
        }
        StoryRow{
            curviness := Slider{
                width: 260.
                text: "curviness"
                min: 0.
                max: 2.
                step: 0.05
                default: 1.
                precision: 2
            }
            path_duration := Slider{
                width: 260.
                text: "duration (s)"
                min: 0.2
                max: 6.
                step: 0.1
                default: 2.
                precision: 1
            }
        }
        StoryRow{
            Caption{text: "Ease"}
            ease_pick := DropDown{
                width: 220.
                popup_menu +: {width: 220.}
                labels: ["none" "power1.inOut" "power2.out" "sine.inOut" "back.out(1.7)" "elastic.out(1, 0.3)" "bounce.out" "steps(12)"]
                selected_item: 1
            }
        }
    }

    mod.stories.FoundationsMotionPaths = StoryPage{
        StoryNote{text: "A lead and a trail of five dots follow one motion path, the way GSAP's MotionPathPlugin moves an element: the path is built once (a curve through points, a closed loop through points, or SVG path data with arcs), stored in the tween engine, and a tween with PropTo::path writes x, y and, with auto-rotate, the angle of the path at the eased ratio. Press Play. Drag start and end to play part of the path (end below start runs it backwards; the loop wraps past 0 and 1), change the curviness, the ease or the shape, and the timeline is rebuilt where it was. The faint ticks sit at equal arc lengths: a lead moving with the linear ease passes them at equal times."}
        StoryHeading{text: "A lead and a stagger trail on one path"}
        stage := mod.storybook.StoryPathStage{}
    }
}

// ---------------------------------------------------------------------------
// Targets, properties, shapes and eases
// ---------------------------------------------------------------------------

const LEAD: TargetId = TargetId(0);
const TRAIL: usize = 5;
const TRAIL_TARGETS: Targets<'static> = Targets::Range {
    first: 1,
    count: TRAIL as u32,
};
/// Seconds between the lead and the first trail dot, and between the dots.
const TRAIL_EACH: f64 = 0.08;

const PX: PropKey = prop(live_id!(px));
const PY: PropKey = prop(live_id!(py));
const PR: PropKey = prop(live_id!(pr));

/// The design box every shape is drawn in; the canvas centres it.
const BOX: [f64; 2] = [600.0, 200.0];
const PAD: f64 = 16.0;

const WAVE: &[[f64; 2]] = &[
    [0.0, 100.0],
    [100.0, 20.0],
    [200.0, 180.0],
    [300.0, 60.0],
    [400.0, 160.0],
    [500.0, 40.0],
    [600.0, 100.0],
];
const LOOP: &[[f64; 2]] = &[
    [300.0, 20.0],
    [520.0, 60.0],
    [560.0, 150.0],
    [300.0, 190.0],
    [60.0, 150.0],
    [80.0, 50.0],
    [300.0, 20.0],
];
const SVG_D: &str = "M20,160 C120,-20 220,220 320,100 S520,0 580,160 A60,60 0 0,1 460,160 L400,120 Q340,60 300,140 T160,160 Z";
/// The shapes as the readout names them.
const SHAPE_NAMES: &[&str] = &["wave thru", "loop thru", "svg d"];

/// GSAP ease strings offered by the ease picker (the DSL's labels), parsed
/// with `parse_gsap_ease` once per pick.
const EASES: &[&str] = &[
    "none",
    "power1.inOut",
    "power2.out",
    "sine.inOut",
    "back.out(1.7)",
    "elastic.out(1, 0.3)",
    "bounce.out",
    "steps(12)",
];
const DEFAULT_EASE: usize = 1;

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// The geometry of `shape` (curviness applies to the two `thru` shapes).
/// Parsing and building happen here, once per change, never per frame.
fn shape_path(shape: usize, curviness: f64) -> MotionPath {
    let built = match shape {
        0 => MotionPath::through(WAVE, curviness),
        1 => MotionPath::through(LOOP, curviness),
        _ => MotionPath::from_svg(SVG_D),
    };
    built.unwrap_or_else(|e| {
        log!("motion paths: {e}");
        MotionPath::default()
    })
}

// ---------------------------------------------------------------------------
// The canvas
// ---------------------------------------------------------------------------

/// What the canvas draws: a copy of the path (taken when it changes) and
/// the values the timeline wrote, pulled once per drawn frame by the stage.
#[derive(Clone, Default)]
struct PathFrame {
    path: MotionPath,
    /// The generation of `path` (the stage's `path_gen` it was copied at).
    gen: u64,
    /// GSAP `start` / `end`.
    span: [f64; 2],
    /// The lead's x, y and rotation (degrees).
    lead: [f64; 3],
    trail: [[f64; 2]; TRAIL],
    /// The lead auto-rotates: draw it as an arrow.
    arrow: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryPathCanvas {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[live]
    draw_panel: DrawColor,
    #[live]
    draw_grid: DrawColor,
    #[live]
    draw_path: DrawEaseSegment,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    grid_color: Vec4f,
    #[live]
    path_color: Vec4f,
    #[live]
    tick_color: Vec4f,
    #[live]
    lead_color: Vec4f,
    #[live]
    trail_color: Vec4f,
    #[live]
    start_color: Vec4f,
    #[live]
    end_color: Vec4f,
    #[rust]
    frame: PathFrame,
}

/// One anti-aliased capsule from `a` to `b` on a quad just big enough.
fn capsule(d: &mut DrawEaseSegment, cx: &mut Cx2d, a: DVec2, b: DVec2, width: f64, color: Vec4f) {
    let m = width + 2.0;
    let lo = dvec2(a.x.min(b.x) - m, a.y.min(b.y) - m);
    let hi = dvec2(a.x.max(b.x) + m, a.y.max(b.y) + m);
    d.thickness = width as f32;
    d.color = color;
    d.seg_a = vec2((a.x - lo.x) as f32, (a.y - lo.y) as f32);
    d.seg_b = vec2((b.x - lo.x) as f32, (b.y - lo.y) as f32);
    d.draw_abs(
        cx,
        Rect {
            pos: lo,
            size: hi - lo,
        },
    );
}

fn with_alpha(mut c: Vec4f, a: f32) -> Vec4f {
    c.w *= a;
    c
}

impl Widget for StoryPathCanvas {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        self.draw_panel.draw_abs(cx, r);

        // The design box, scaled down (never up) to fit and centred.
        let sc = ((r.size.x - 2.0 * PAD) / BOX[0])
            .min((r.size.y - 2.0 * PAD) / BOX[1])
            .clamp(0.1, 1.0);
        let o = dvec2(
            r.pos.x + (r.size.x - BOX[0] * sc) * 0.5,
            r.pos.y + (r.size.y - BOX[1] * sc) * 0.5,
        );
        let px = |p: [f64; 3]| dvec2(o.x + p[0] * sc, o.y + p[1] * sc);

        // A faint grid every 50 units.
        self.draw_grid.color = with_alpha(self.grid_color, 0.5);
        for i in 0..=12 {
            let x = o.x + i as f64 * 50.0 * sc;
            self.draw_grid.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x - 0.5, o.y),
                    size: dvec2(1.0, BOX[1] * sc),
                },
            );
        }
        for j in 0..=4 {
            let y = o.y + j as f64 * 50.0 * sc;
            self.draw_grid.draw_abs(
                cx,
                Rect {
                    pos: dvec2(o.x, y - 0.5),
                    size: dvec2(BOX[0] * sc, 1.0),
                },
            );
        }

        let Self {
            frame,
            draw_path,
            draw_dot,
            draw_text,
            ..
        } = self;
        let f = &*frame;

        // The path: capsules along its flattening, a new run per subpath.
        let mut prev = dvec2(0.0, 0.0);
        let path_color = self.path_color;
        f.path.flatten(24, |p, first| {
            let q = px(p);
            if !first {
                capsule(draw_path, cx, prev, q, 2.0, path_color);
            }
            prev = q;
        });

        // 20 ticks at equal arc lengths: the table's proof.
        let tick = with_alpha(self.tick_color, 0.6);
        for k in 0..20 {
            let s = f.path.sample(k as f64 / 20.0);
            let c = px(s.pos);
            let n = dvec2(-s.tangent[1], s.tangent[0]);
            capsule(draw_path, cx, c - n * 5.0, c + n * 5.0, 1.5, tick);
        }

        // The span's start and end.
        if f.path.segment_count() > 0 {
            for (e, color, name) in [
                (0.0, self.start_color, "start"),
                (1.0, self.end_color, "end"),
            ] {
                let p: PathPoint = f.path.sample_span(f.span[0], f.span[1], e);
                let c = px(p.pos);
                draw_dot.color = color;
                draw_dot.draw_abs(
                    cx,
                    Rect {
                        pos: c - dvec2(6.0, 6.0),
                        size: dvec2(12.0, 12.0),
                    },
                );
                draw_text.draw_abs(cx, c + dvec2(8.0, 4.0), name);
            }
        }

        // The trail, faint, then the lead.
        draw_dot.color = with_alpha(self.trail_color, 0.5);
        for t in f.trail.iter() {
            let c = px([t[0], t[1], 0.0]);
            draw_dot.draw_abs(
                cx,
                Rect {
                    pos: c - dvec2(4.0, 4.0),
                    size: dvec2(8.0, 8.0),
                },
            );
        }
        let c = px([f.lead[0], f.lead[1], 0.0]);
        if f.arrow {
            // Three capsules along the written angle, drawn above the dots.
            let (s, co) = f.lead[2].to_radians().sin_cos();
            let t = dvec2(co, s);
            let n = dvec2(-s, co);
            draw_path.new_draw_call(cx);
            let lead = self.lead_color;
            capsule(draw_path, cx, c - t * 14.0, c + t * 10.0, 3.0, lead);
            capsule(
                draw_path,
                cx,
                c + t * 10.0,
                c + t * 3.0 + n * 5.0,
                3.0,
                lead,
            );
            capsule(
                draw_path,
                cx,
                c + t * 10.0,
                c + t * 3.0 - n * 5.0,
                3.0,
                lead,
            );
        } else {
            draw_dot.color = self.lead_color;
            draw_dot.draw_abs(
                cx,
                Rect {
                    pos: c - dvec2(7.0, 7.0),
                    size: dvec2(14.0, 14.0),
                },
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// The on-page picks (shape, span, curviness, duration, auto-rotate, ease).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Picks {
    shape: usize,
    span_start: f64,
    span_end: f64,
    curviness: f64,
    duration: f64,
    auto_rotate: bool,
    ease: usize,
}

impl Default for Picks {
    fn default() -> Self {
        Self {
            shape: 0,
            span_start: 0.0,
            span_end: 1.0,
            curviness: 1.0,
            duration: 2.0,
            auto_rotate: true,
            ease: DEFAULT_EASE,
        }
    }
}

/// Everything the timeline was built from; a change rebuilds it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Knobs {
    picks: Picks,
    ease: Easing,
    rotate_offset: f64,
    yoyo: bool,
    repeat: i32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryPathStage {
    #[deref]
    view: View,
    #[live(0.0)]
    rotate_offset: f64,
    #[live]
    yoyo: bool,
    #[live(0.0)]
    repeat: f64,
    #[rust]
    motion: TweenHost,
    #[rust]
    tl: TweenId,
    /// The lead's tween inside the timeline (its progress drives the readout).
    #[rust]
    lead: TweenId,
    /// The stage's hold on the current path (released when it is replaced).
    #[rust]
    path: PathId,
    /// A copy of the current path for the readout and the canvas.
    #[rust]
    geom: MotionPath,
    /// What the current path was built from: (shape, curviness bits).
    #[rust]
    path_key: Option<(usize, u64)>,
    #[rust]
    path_gen: u64,
    #[rust]
    built_from: Option<Knobs>,
    #[rust]
    picks: Picks,
    /// The ease of `picks.ease`, parsed when the pick changes.
    #[rust]
    picked: Option<Easing>,
    /// Timelines built so far: the setup line's proof of a rebuild.
    #[rust]
    builds: u64,
    #[rust]
    text: String,
    /// While the playhead slider is held: whether the timeline was playing.
    #[rust]
    scrubbing: Option<bool>,
    /// The path and setup lines need rewriting (a rebuild).
    #[rust]
    setup_dirty: bool,
    /// A one-off frame that refreshes the readout (first draw, a rebuild
    /// from the Controls tab).
    #[rust]
    refresh: NextFrame,
    #[rust]
    shown: bool,
}

impl StoryPathStage {
    fn knobs(&self) -> Knobs {
        Knobs {
            picks: self.picks,
            ease: self.picked.unwrap_or_default(),
            rotate_offset: self.rotate_offset,
            yoyo: self.yoyo,
            repeat: self.repeat.round().clamp(-1.0, 1000.0) as i32,
        }
    }

    /// Adds the path of the picked shape to the engine when the shape or
    /// the curviness changed, and releases the previous one (it lives on
    /// while the old timeline's tweens follow it). Returns whether it did.
    fn ensure_path(&mut self) -> bool {
        let p = self.picks;
        let key = (
            p.shape,
            if p.shape < 2 {
                p.curviness.to_bits()
            } else {
                0
            },
        );
        if self.path_key == Some(key) {
            return false;
        }
        let geom = shape_path(p.shape, p.curviness);
        let old = self.path;
        self.path = self.motion.engine.add_path(geom.clone());
        self.motion.engine.release_path(old);
        self.geom = geom;
        self.path_key = Some(key);
        self.path_gen += 1;
        true
    }

    /// Builds (or rebuilds) the timeline when a knob changed, landing the
    /// new one where the old one was: same total progress (or iteration and
    /// progress when either repeats forever), same direction, still playing
    /// if it was. Returns whether it rebuilt.
    fn ensure_built(&mut self, cx: &mut Cx) -> bool {
        if self.picked.is_none() {
            self.picked = parse_gsap_ease(EASES[self.picks.ease]);
        }
        self.ensure_path();
        let k = self.knobs();
        let alive = self.motion.engine.anim_ref(self.tl).is_alive();
        if alive && self.built_from == Some(k) {
            return false;
        }
        let (tprog, prog, iter, was_inf, playing, reversed) = if alive {
            let a = self.motion.engine.anim_ref(self.tl);
            (
                a.total_progress(),
                a.progress(),
                a.iteration(),
                a.total_duration() >= BIG,
                !a.paused() && a.is_active(),
                a.reversed(),
            )
        } else {
            (0.0, 0.0, 1, false, false, false)
        };
        if alive {
            let old = self.tl;
            self.motion.kill(cx, old);
        }
        self.build(&k);
        self.built_from = Some(k);
        self.builds += 1;
        self.setup_dirty = true;
        let tl = self.tl;
        let mut c = self.motion.control(cx, tl);
        if was_inf || k.repeat < 0 {
            let last = if k.repeat < 0 {
                iter
            } else {
                iter.min(k.repeat as u32 + 1)
            };
            c.set_iteration(last.max(1), Emit::Suppress);
            c.seek(Seek::Progress(prog), Emit::Suppress);
        } else {
            c.seek(Seek::TotalProgress(tprog), Emit::Suppress);
        }
        if reversed {
            c.set_reversed(true);
        }
        if playing {
            c.resume();
        }
        true
    }

    fn build(&mut self, k: &Knobs) {
        let [s0, s1] = [k.picks.span_start, k.picks.span_end];
        let h = &mut self.motion;
        // Dots whose stagger has not started yet sit at the span's start.
        let a = self.geom.sample_span(s0, s1, 0.0);
        for i in 0..=TRAIL as u32 {
            h.seed(TargetId(i), PX, a.pos[0]);
            h.seed(TargetId(i), PY, a.pos[1]);
        }
        h.seed(LEAD, PR, TweenValue::F64(a.angle_deg + k.rotate_offset));
        let span = PathOpts::new().span(s0, s1);
        let lead = if k.picks.auto_rotate {
            span.auto_rotate(PR, k.rotate_offset)
        } else {
            span
        };
        let o = TweenOpts::new().duration(k.picks.duration).ease(k.ease);
        // gsap.timeline({paused: true, repeat, yoyo}), kept for the page.
        let tl = h.timeline(
            TimelineOpts::new()
                .repeat(k.repeat)
                .yoyo(k.yoyo)
                .paused(true),
        );
        let path = self.path;
        h.tl(tl).to(
            LEAD.into(),
            &[PropTo::path(PX, PY, path, lead)],
            o,
            Position::at(0.0),
        );
        self.lead = h.tl(tl).last();
        h.tl(tl).to(
            TRAIL_TARGETS,
            &[PropTo::path(PX, PY, path, span)],
            o.stagger(Stagger::each(TRAIL_EACH)),
            Position::at(TRAIL_EACH),
        );
        self.tl = tl;
    }

    /// Pulls the current values into the canvas (draw time: the same clock
    /// the frame step used), and a copy of the path when it changed.
    fn fill_canvas(&mut self, cx: &mut Cx) {
        let canvas = self.view.story_path_canvas(cx, ids!(canvas));
        let Some(mut canvas) = canvas.borrow_mut() else {
            return;
        };
        let f = &mut canvas.frame;
        if f.gen != self.path_gen {
            f.path = self.geom.clone();
            f.gen = self.path_gen;
        }
        let p = self.built_from.map_or(self.picks, |k| k.picks);
        f.span = [p.span_start, p.span_end];
        f.arrow = p.auto_rotate;
        let h = &self.motion;
        f.lead = [
            h.f64(LEAD, PX, 0.0),
            h.f64(LEAD, PY, 0.0),
            h.f64(LEAD, PR, 0.0),
        ];
        for (i, t) in f.trail.iter_mut().enumerate() {
            let id = TargetId(1 + i as u32);
            *t = [h.f64(id, PX, 0.0), h.f64(id, PY, 0.0)];
        }
    }

    /// Rewrites the readout (labels only redraw when their text changed)
    /// and puts the playhead back on the slider.
    fn update_readout(&mut self, cx: &mut Cx) {
        let Some(k) = self.built_from else {
            return;
        };
        let tla = self.motion.engine.anim_ref(self.tl);
        let (paused, active, reversed) = (tla.paused(), tla.is_active(), tla.reversed());
        let (ttime, tdur, tprog, tlp) = (
            tla.total_time(),
            tla.total_duration(),
            tla.total_progress(),
            tla.progress(),
        );
        let la = self.motion.engine.anim_ref(self.lead);
        let (t, dur, p) = (la.time(), la.duration(), la.progress());
        let infinite = tdur >= BIG;
        // The ratio on the path: the engine clamps the eased ratio to 0..1.
        let [s0, s1] = [k.picks.span_start, k.picks.span_end];
        let e = if p >= 1.0 {
            1.0
        } else if p <= 0.0 {
            0.0
        } else {
            k.ease.map(p).clamp(0.0, 1.0)
        };
        let u = if e >= 1.0 { s1 } else { s0 + (s1 - s0) * e };
        let h = &self.motion;
        let (x, y, rot) = (
            h.f64(LEAD, PX, 0.0),
            h.f64(LEAD, PY, 0.0),
            h.f64(LEAD, PR, 0.0),
        );
        let mut s = std::mem::take(&mut self.text);

        s.clear();
        let state = if paused {
            "paused"
        } else if active {
            "playing"
        } else {
            "idle"
        };
        let _ = write!(
            s,
            "t {:.3} / {:.3} s  progress {:.3}  u {:.3}  {} {}  timeline {:.3} / ",
            t,
            dur,
            p,
            u,
            state,
            if reversed { "reversed" } else { "forward" },
            ttime
        );
        if infinite {
            s.push_str("inf s");
        } else {
            let _ = write!(s, "{:.3} s", tdur);
        }
        self.view
            .label(cx, ids!(path_readout_time))
            .set_text(cx, &s);

        s.clear();
        let angle = if k.picks.auto_rotate {
            rot
        } else {
            self.geom.sample_span(s0, s1, e).angle_deg
        };
        let _ = write!(s, "x {:.2} y {:.2} angle {:.1} deg  at end: ", x, y, angle);
        if p >= 1.0 {
            // The engine lands the end exactly: the path's point at `end`
            // plus the (zero) offset, bit for bit.
            let z = self.geom.sample_span(s0, s1, 1.0);
            let (zx, zy) = (z.pos[0] + 0.0, z.pos[1] + 0.0);
            if x.to_bits() == zx.to_bits() && y.to_bits() == zy.to_bits() {
                s.push_str("exact");
            } else {
                let _ = write!(s, "MISMATCH ({:e}, {:e})", x - zx, y - zy);
            }
        } else {
            s.push('-');
        }
        self.view.label(cx, ids!(path_readout_pos)).set_text(cx, &s);

        // The path and the setup change only with a rebuild.
        if self.setup_dirty {
            self.setup_dirty = false;
            let g = &self.geom;
            let b = g.bounds();
            s.clear();
            s.push_str(SHAPE_NAMES.get(k.picks.shape).copied().unwrap_or("-"));
            if k.picks.shape < 2 {
                let _ = write!(s, " curviness {:.2}", k.picks.curviness);
            }
            let _ = write!(
                s,
                ": {} segments, {} subpath{}, {}, length {:.1}, {} spans, bounds [{:.0},{:.0}]-[{:.0},{:.0}]",
                g.segment_count(),
                g.subpath_count(),
                if g.subpath_count() == 1 { "" } else { "s" },
                if g.is_closed() { "closed" } else { "open" },
                g.length(),
                g.span_count(),
                b.min[0],
                b.min[1],
                b.max[0],
                b.max[1]
            );
            self.view
                .label(cx, ids!(path_readout_path))
                .set_text(cx, &s);

            s.clear();
            let _ = write!(s, "span {:.2} -> {:.2}, auto-rotate ", s0, s1);
            if k.picks.auto_rotate {
                let _ = write!(s, "on {:+} deg", k.rotate_offset);
            } else {
                s.push_str("off");
            }
            let _ = write!(
                s,
                ", ease {}, {:.2} s, repeat {} yoyo {}, built #{}",
                EASES.get(k.picks.ease).copied().unwrap_or("-"),
                k.picks.duration,
                k.repeat,
                yes_no(k.yoyo),
                self.builds
            );
            self.view
                .label(cx, ids!(path_readout_setup))
                .set_text(cx, &s);
        }
        self.text = s;

        let scrub = self.view.slider(cx, ids!(scrub));
        if self.scrubbing.is_none() && !scrub.has_text_focus(cx) {
            scrub.set_value(cx, if infinite { tlp } else { tprog });
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let tl = self.tl;
        let v = &self.view;
        let (play, pause, reverse, restart) = (
            v.button(cx, ids!(path_play)),
            v.button(cx, ids!(path_pause)),
            v.button(cx, ids!(path_reverse)),
            v.button(cx, ids!(path_restart)),
        );
        let scrub = v.slider(cx, ids!(scrub));
        let shape = v.segmented_control(cx, ids!(shape));
        let span_start = v.slider(cx, ids!(span_start));
        let span_end = v.slider(cx, ids!(span_end));
        let curviness = v.slider(cx, ids!(curviness));
        let duration = v.slider(cx, ids!(path_duration));
        let auto_rotate = v.check_box(cx, ids!(auto_rotate));
        let ease_pick = v.drop_down(cx, ids!(ease_pick));

        // Transport: GSAP's Animation methods on the timeline.
        let mut acted = false;
        if play.clicked(actions) {
            self.motion.control(cx, tl).play();
            acted = true;
        }
        if pause.clicked(actions) {
            self.motion.control(cx, tl).pause();
            acted = true;
        }
        if reverse.clicked(actions) {
            // GSAP reverse() only sets the direction; the button toggles it.
            let r = !self.motion.engine.anim_ref(tl).reversed();
            self.motion.control(cx, tl).set_reversed(r).resume();
            acted = true;
        }
        if restart.clicked(actions) {
            self.motion.control(cx, tl).restart(false, Emit::Suppress);
            acted = true;
        }

        // The playhead: hold (pause), scrub (renders now, fires nothing),
        // let go (resume if it was playing).
        if scrub.start_slide(actions) {
            let a = self.motion.engine.anim_ref(tl);
            self.scrubbing = Some(!a.paused() && a.is_active());
            self.motion.control(cx, tl).pause();
            acted = true;
        }
        if let Some(p) = scrub.slided(actions) {
            let seek = if self.motion.engine.anim_ref(tl).total_duration() >= BIG {
                Seek::Progress(p)
            } else {
                Seek::TotalProgress(p)
            };
            self.motion.control(cx, tl).seek(seek, Emit::Suppress);
            acted = true;
        }
        if scrub.end_slide(actions).is_some() {
            if self.scrubbing == Some(true) {
                self.motion.control(cx, tl).resume();
            }
            self.scrubbing = None;
            acted = true;
        }

        // Picks that rebuild the timeline.
        let mut picks = self.picks;
        if let Some(i) = shape.selected(actions) {
            picks.shape = i;
        }
        if let Some(x) = span_start.slided(actions) {
            picks.span_start = x;
        }
        if let Some(x) = span_end.slided(actions) {
            picks.span_end = x;
        }
        if let Some(x) = curviness.slided(actions) {
            picks.curviness = x.max(0.0);
        }
        if let Some(x) = duration.slided(actions) {
            picks.duration = x.max(0.01);
        }
        if let Some(b) = auto_rotate.changed(actions) {
            picks.auto_rotate = b;
        }
        if let Some(i) = ease_pick.selected(actions) {
            if i != picks.ease && i < EASES.len() {
                picks.ease = i;
                // Parsed here, once per pick: knobs() reads the result.
                self.picked = parse_gsap_ease(EASES[i]);
            }
        }
        if picks != self.picks {
            self.picks = picks;
            self.ensure_built(cx);
            self.view.widget(cx, ids!(canvas)).redraw(cx);
            acted = true;
        }
        if acted {
            self.update_readout(cx);
        }
    }
}

impl Widget for StoryPathStage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Knobs from the Controls tab arrive as applies followed by a
        // redraw: this is where a changed knob rebuilds the timeline.
        let rebuilt = self.ensure_built(cx.cx.cx);
        self.motion.draw_check(cx.cx.cx);
        if rebuilt || !self.shown {
            // The readout is written from the event side (labels set their
            // text there), on the next frame.
            self.shown = true;
            self.refresh = cx.cx.cx.new_next_frame();
        }
        self.fill_canvas(cx.cx.cx);
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
        let refresh = match event {
            Event::NextFrame(ne) => self.refresh.0 != 0 && ne.set.contains(&self.refresh),
            _ => false,
        };
        if refresh {
            self.refresh = NextFrame::default();
        }
        let act = self.motion.handle_event(cx, event);
        if act.changed() || refresh {
            // The canvas pulls in draw_walk; the host's change list is only
            // a cue here.
            self.motion.clear_changes();
            self.update_readout(cx);
            self.view.widget(cx, ids!(canvas)).redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/motion-paths",
    category: "Foundations",
    component: "Motion",
    also: &[],
    name: "Motion paths",
    dsl: "FoundationsMotionPaths",
    added: "2026-09-25",
    tags: &["new", "tween", "motion path", "motionpath", "gsap", "svg", "catmull-rom", "auto-rotate"],
    doc: "# Motion paths\n\nA tween can follow a path, as GSAP's `MotionPathPlugin` moves an element along one. `makepad_widgets::tween` (the `makepad_tween` engine) has it in three pieces:\n\n- **`MotionPath`** is the geometry: line, quadratic and cubic segments in f64, possibly in several subpaths. Build one from SVG path data (`MotionPath::from_svg(\"M0,0 C..\")`: `M L H V C S Q T A Z`, arcs become cubics), as a smooth curve through points (`MotionPath::through(&points, curviness)`, GSAP `type: \"thru\"`: curviness 0 draws straight lines, 1 a natural curve, 2 a loose one; a last point on the first closes it), from anchor and control points (`cubic_points`, GSAP `type: \"cubic\"`), as a polyline, or command by command with a `PathBuilder`. Finishing it builds an arc-length table once; sampling at a fraction `u` of the length is then allocation-free and exact at both ends. The coordinates are 3D: the 2D builders set z = 0 and the `*3` forms take z.\n- **`TweenEngine::add_path`** stores it and returns a `PathId`; `release_path` drops your hold, so the geometry lives exactly as long as the tweens that follow it.\n- **`PropTo::path(x, y, id, PathOpts)`** makes a tween follow it. `PathOpts` is GSAP's options: `span(start, end)` (fractions of the length; end below start runs backwards, a closed path wraps past 0 and 1, an open one clamps), `offset(dx, dy)` (`offsetX` / `offsetY`), `align(PathAlign::Start)` (`align: self`: the path moves so its start sits on the target's current x / y), `auto_rotate(key, degrees)` (`autoRotate`, or `auto_rotate_radians`) and `z(key)` for 3D paths.\n\nThe tween's eased ratio is clamped to the path, as in GSAP: an overshooting ease such as `back.out` rests at the end instead of leaving the path, and both ends land exactly.\n\n## This page\n\nOne paused, kept timeline: the lead follows the path with the picked ease (and auto-rotates into an arrow when **Auto-rotate** is on), and five trail dots follow it with a stagger of 0.08 s. **Shape** switches between a wave through seven points, a closed loop through six points and SVG path data with every command (C S A L Q T Z). **start / end** set the span, **curviness** the `thru` shapes' curviness, **duration** and **Ease** the tween. The Controls tab has the auto-rotate offset, yoyo and repeat. Any change rebuilds the timeline where it was: same total progress, same direction, still playing if it was. The 20 faint ticks sit at equal arc lengths along the path (`sample(k / 20)`), the proof of the arc-length table: with the ease `none` the lead passes them at equal times.\n\n- **Play, Pause, Restart** are GSAP's `play()`, `pause()` and `restart()`; **Reverse** toggles the direction and resumes.\n- **The playhead** seeks the timeline's total progress with events suppressed while it is held.\n\n## The readout\n\n`/snap?q=path_readout` reads it on the `--remote` surface: `path_readout_time` (the lead's time, progress and path fraction `u`, the timeline's state and time), `path_readout_pos` (the lead's x, y and angle; `at end: exact` when the lead has landed bit for bit on the path's point at `end`), `path_readout_path` (segments, subpaths, open or closed, length, table spans, bounds) and `path_readout_setup` (span, auto-rotate, ease, duration, repeat, yoyo and `built #N`, which goes up on every rebuild).",
    subject: "stage",
    feature: None,
    controls: &[
        Control { label: "Rotate offset", target: "stage", kind: ControlKind::Number { prop: "rotate_offset", min: -180., max: 180., step: 5., default: 0. } },
        Control { label: "Yoyo", target: "stage", kind: ControlKind::Bool { prop: "yoyo", default: false } },
        Control { label: "Repeat", target: "stage", kind: ControlKind::Number { prop: "repeat", min: -1., max: 5., step: 1., default: 0. } },
    ],
    on_actions: None,
}];
