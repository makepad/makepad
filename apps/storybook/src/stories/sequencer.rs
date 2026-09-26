//! The sequencer page: one GSAP-style timeline (a title, a stagger of eight
//! dots, a nested timeline of three cards, a call, a motion path and a
//! tint, with the labels `intro` and `travel`) shown and edited in a
//! `Sequencer`, with the stage it drives.
//!
//! The readout under the sequencer is the page's evidence where no
//! screenshot is available: every line is a label with an id, rewritten on
//! each frame that changed something, so `/snap?q=seq_readout` on the
//! `--remote` surface reads the playhead, the selected item as the widget
//! and the engine see it, the view, the last edit and the values the
//! timeline writes; `/snap?q=seq` reads the widget's own geometry.
use crate::makepad_widgets::tween::{
    parse_gsap_ease, prop, tag, ColorSpace, Easing, Emit, EventMask, MotionPath, PathId, PathOpts,
    Position, PropKey, PropTo, Rgba, Seek, Stagger, Tag, TargetId, Targets, TimelineOpts,
    TweenEngine, TweenEvent, TweenHost, TweenId, TweenOpts,
};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StorySeqCanvasBase = #(StorySeqCanvas::register_widget(vm))
    /** The stage the sequencer's timeline drives: a title bar, eight dots, three cards, a tint swatch and an arrow on a motion path, at the values the timeline last wrote. */
    mod.storybook.StorySeqCanvas = set_type_default() do mod.storybook.StorySeqCanvasBase{
        width: Fill
        height: 240
        title_color: theme.color_primary
        dot_color: theme.color_tertiary
        card_color: theme.color_secondary
        path_color: theme.color_outline
        lead_color: theme.color_error
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
        draw_bar +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 4.0)
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

    mod.storybook.StorySeqStageBase = #(StorySeqStage::register_widget(vm))
    /** A timeline shown and edited in a Sequencer, its transport, a text readout and the stage it drives. */
    mod.storybook.StorySeqStage = set_type_default() do mod.storybook.StorySeqStageBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        path_color: theme.color_error

        StoryRow{
            seq_play := Button{text: "Play"}
            seq_pause := Button{text: "Pause"}
            seq_reverse := Button{text: "Reverse"}
            seq_restart := Button{text: "Restart"}
        }
        seq := Sequencer{}
        seq_readout_time := Readout{}
        seq_readout_sel := Readout{}
        seq_readout_view := Readout{}
        seq_readout_edit := Readout{}
        seq_readout_values := Readout{}
        canvas := mod.storybook.StorySeqCanvas{height: 240}
    }

    mod.stories.FoundationsMotionSequencer = StoryPage{
        StoryNote{text: "One timeline, as GSAP builds one, in a Sequencer: a title fades in, eight dots pop in with a stagger from the label `intro`, a nested timeline slides three cards in (twice: it repeats after a 0.2 s gap), a call fires at 1.4 s, an arrow follows a motion path from the label `travel` and a swatch changes colour. Press Play, or press and drag on the ruler to scrub. Drag a bar to move it and an edge to resize it (snapped to labels, the playhead and other bars; Alt places freely, Escape puts it back), drag a label flag to move the label: the timeline is edited and the stage follows. Ctrl + wheel (Cmd on macOS) zooms, Shift + wheel pans, the arrows fold rows, Left / Right nudge the selected bar."}
        StoryHeading{text: "A timeline in a Sequencer"}
        stage := mod.storybook.StorySeqStage{}
    }
}

// ---------------------------------------------------------------------------
// Targets, properties and tags
// ---------------------------------------------------------------------------

const TITLE: TargetId = TargetId(0);
/// The first dot's target; dot `i` is `DOT0 + i`.
const DOT0: u32 = 10;
const DOT_COUNT: usize = 8;
const DOTS: Targets<'static> = Targets::Range {
    first: DOT0,
    count: DOT_COUNT as u32,
};
/// The first card's target.
const CARD0: u32 = 20;
const FOLLOW: TargetId = TargetId(30);
const TINT_T: TargetId = TargetId(31);

const ALPHA: PropKey = prop(live_id!(alpha));
const LIFT: PropKey = prop(live_id!(lift));
const SCALE: PropKey = prop(live_id!(scale));
const SLIDE: PropKey = prop(live_id!(slide));
const PX: PropKey = prop(live_id!(px));
const PY: PropKey = prop(live_id!(py));
const PR: PropKey = prop(live_id!(pr));
const TINT: PropKey = prop(live_id!(tint));

const SEQ: Tag = tag(live_id!(sequence));
const INTRO: Tag = tag(live_id!(intro));
const TRAVEL: Tag = tag(live_id!(travel));
const T_TITLE: Tag = tag(live_id!(title));
const T_DOTS: Tag = tag(live_id!(dots));
const T_CARDS: Tag = tag(live_id!(cards));
const T_CARD: [Tag; 3] = [
    tag(live_id!(card_1)),
    tag(live_id!(card_2)),
    tag(live_id!(card_3)),
];
const T_HALF: Tag = tag(live_id!(halfway));
const T_PATH: Tag = tag(live_id!(path));
const T_TINT: Tag = tag(live_id!(tint));

const TAG_NAMES: &[(Tag, &str)] = &[
    (SEQ, "sequence"),
    (INTRO, "intro"),
    (TRAVEL, "travel"),
    (T_TITLE, "title"),
    (T_DOTS, "dots"),
    (T_CARDS, "cards"),
    (T_CARD[0], "card 1"),
    (T_CARD[1], "card 2"),
    (T_CARD[2], "card 3"),
    (T_HALF, "halfway"),
    (T_PATH, "path"),
    (T_TINT, "tint"),
];

const TINT_FROM: u32 = 0xff5040ff;
const TINT_TO: u32 = 0x40a0ffff;

/// The motion path, a curve through five points in a 600 x 240 design box.
const PATH_POINTS: &[[f64; 2]] = &[
    [40.0, 180.0],
    [160.0, 60.0],
    [300.0, 200.0],
    [440.0, 80.0],
    [560.0, 160.0],
];
const BOX: [f64; 2] = [600.0, 240.0];
const PAD: f64 = 16.0;
/// The width of the stage's left column (title, dots, cards, swatch).
const LEFT_W: f64 = 250.0;

fn tag_name(t: Tag) -> Option<&'static str> {
    TAG_NAMES.iter().find(|(k, _)| *k == t).map(|(_, n)| *n)
}

fn opt_tag_name(t: Option<Tag>) -> &'static str {
    t.and_then(tag_name).unwrap_or("-")
}

/// Names a row (or, with `TweenId::NONE`, a label): its tag's name, or
/// "dot N" for a stagger member (its target), else "-".
fn name_for(e: &TweenEngine, id: TweenId, t: Tag) -> String {
    if let Some(n) = tag_name(t) {
        return n.to_string();
    }
    if let Some(TargetId(i)) = e.anim_ref(id).first_target() {
        if (DOT0..DOT0 + DOT_COUNT as u32).contains(&i) {
            return format!("dot {}", i - DOT0 + 1);
        }
    }
    "-".to_string()
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// No "-0.000" in the readout.
fn clean(t: f64) -> f64 {
    if t.abs() < 1e-9 {
        0.0
    } else {
        t + 0.0
    }
}

// ---------------------------------------------------------------------------
// The canvas
// ---------------------------------------------------------------------------

/// What the canvas draws: the values the timeline wrote, pulled once per
/// drawn frame by the stage, and a copy of the path (taken once).
#[derive(Clone, Default)]
struct SeqFrame {
    path: MotionPath,
    has_path: bool,
    /// Alpha and lift.
    title: [f64; 2],
    dots: [f64; DOT_COUNT],
    cards: [f64; 3],
    /// x, y, rotation (degrees) on the path.
    follow: [f64; 3],
    tint: [f32; 4],
}

#[derive(Script, ScriptHook, Widget)]
pub struct StorySeqCanvas {
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
    draw_bar: DrawColor,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_path: DrawEaseSegment,
    #[live]
    draw_text: DrawText,
    #[live]
    title_color: Vec4f,
    #[live]
    dot_color: Vec4f,
    #[live]
    card_color: Vec4f,
    #[live]
    path_color: Vec4f,
    #[live]
    lead_color: Vec4f,
    #[rust]
    frame: SeqFrame,
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

fn with_alpha(mut c: Vec4f, a: f64) -> Vec4f {
    c.w *= a.clamp(0.0, 1.0) as f32;
    c
}

impl Widget for StorySeqCanvas {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        self.draw_panel.draw_abs(cx, r);
        let (x0, y0) = (r.pos.x + PAD, r.pos.y + PAD);

        // The title bar: alpha and lift.
        let f = &self.frame;
        self.draw_bar.color = with_alpha(self.title_color, f.title[0]);
        self.draw_bar.draw_abs(
            cx,
            Rect {
                pos: dvec2(x0, y0 + f.title[1]),
                size: dvec2(LEFT_W - 20.0, 22.0),
            },
        );
        // Eight dots at their scale.
        let cy = y0 + 22.0 + 30.0;
        for (i, s) in f.dots.iter().enumerate() {
            let size = 16.0 * s.max(0.0);
            if size > 0.01 {
                let c = dvec2(x0 + 10.0 + i as f64 * 28.0, cy);
                self.draw_dot.color = self.dot_color;
                self.draw_dot.draw_abs(
                    cx,
                    Rect {
                        pos: c - dvec2(size * 0.5, size * 0.5),
                        size: dvec2(size, size),
                    },
                );
            }
        }
        // Three cards sliding in from 40 px to the right.
        let top = cy + 24.0;
        for (i, slide) in f.cards.iter().enumerate() {
            self.draw_bar.color = with_alpha(self.card_color, 1.0 - slide / 40.0);
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0 + i as f64 * 76.0 + slide, top),
                    size: dvec2(68.0, 44.0),
                },
            );
        }
        // The tint swatch.
        let t = f.tint;
        self.draw_bar.color = Vec4f {
            x: t[0],
            y: t[1],
            z: t[2],
            w: t[3],
        };
        let sw = top + 44.0 + 16.0;
        self.draw_bar.draw_abs(
            cx,
            Rect {
                pos: dvec2(x0, sw),
                size: dvec2(68.0, 40.0),
            },
        );
        self.draw_text
            .draw_abs(cx, dvec2(x0 + 76.0, sw + 12.0), "tint");

        // The path box right of the column, scaled down (never up), centred.
        let left = x0 + LEFT_W;
        let avail = dvec2(
            (r.pos.x + r.size.x - PAD - left).max(1.0),
            (r.size.y - 2.0 * PAD).max(1.0),
        );
        let sc = (avail.x / BOX[0]).min(avail.y / BOX[1]).clamp(0.1, 1.0);
        let o = dvec2(
            left + (avail.x - BOX[0] * sc) * 0.5,
            r.pos.y + (r.size.y - BOX[1] * sc) * 0.5,
        );
        let px = |x: f64, y: f64| dvec2(o.x + x * sc, o.y + y * sc);
        let Self {
            frame, draw_path, ..
        } = self;
        let mut prev = dvec2(0.0, 0.0);
        let path_color = self.path_color;
        frame.path.flatten(24, |p, first| {
            let q = px(p[0], p[1]);
            if !first {
                capsule(draw_path, cx, prev, q, 2.0, path_color);
            }
            prev = q;
        });
        // The follower: an arrow along the written angle, above the path.
        let [fx, fy, rot] = frame.follow;
        let c = px(fx, fy);
        let (s, co) = rot.to_radians().sin_cos();
        let (tv, nv) = (dvec2(co, s), dvec2(-s, co));
        draw_path.new_draw_call(cx);
        let lead = self.lead_color;
        capsule(draw_path, cx, c - tv * 14.0, c + tv * 10.0, 3.0, lead);
        capsule(
            draw_path,
            cx,
            c + tv * 10.0,
            c + tv * 3.0 + nv * 5.0,
            3.0,
            lead,
        );
        capsule(
            draw_path,
            cx,
            c + tv * 10.0,
            c + tv * 3.0 - nv * 5.0,
            3.0,
            lead,
        );
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook, Widget)]
pub struct StorySeqStage {
    #[deref]
    view: View,
    /// The motion path row's colour in the sequencer.
    #[live]
    path_color: Vec4f,
    #[rust]
    motion: TweenHost,
    #[rust]
    tl: TweenId,
    /// The stage's hold on the path, for the page's life.
    #[rust]
    path: PathId,
    /// A copy of the path for the canvas.
    #[rust]
    geom: MotionPath,
    #[rust]
    events: Vec<TweenEvent>,
    #[rust]
    text: String,
    /// "path moved to 1.800 s", or empty.
    #[rust]
    last_edit: String,
    /// While the ruler is held: whether the timeline was playing.
    #[rust]
    scrubbing: Option<bool>,
    /// A one-off frame that refreshes the readout (first draw, a rebuild).
    #[rust]
    refresh: NextFrame,
    #[rust]
    shown: bool,
    /// The visible row count the readout last wrote: a fold (which emits
    /// no action) changes it, and the draw that follows asks for a refresh.
    #[rust]
    rows_shown: usize,
    /// `Zoom` actions seen (the view line's proof that a clamped zoom
    /// reports nothing).
    #[rust]
    zoom_actions: u64,
}

impl StorySeqStage {
    /// Builds the timeline when it does not exist yet (first draw).
    fn ensure_built(&mut self, cx: &mut Cx) -> bool {
        if self.motion.engine.anim_ref(self.tl).is_alive() {
            return false;
        }
        self.build();
        let m = self.engine_model();
        // The stagger group opens (its eight dots are rows too), BEFORE the
        // model arrives: a host restoring folds and then showing a model,
        // with no draw in between, must get rows for the new model.
        let seq = self.view.sequencer(cx, ids!(seq));
        if let Some(t) = m.tracks.iter().find(|t| t.kind == SequencerKind::Group) {
            seq.set_expanded(cx, t.id, true);
        }
        seq.set_model(cx, m);
        true
    }

    fn build(&mut self) {
        if self.geom.segment_count() == 0 {
            self.geom = MotionPath::through(PATH_POINTS, 1.0).unwrap_or_else(|e| {
                log!("sequencer page: {e}");
                MotionPath::default()
            });
        }
        let h = &mut self.motion;
        if h.engine.path(self.path).is_none() {
            self.path = h.engine.add_path(self.geom.clone());
        }
        h.seed(TINT_T, TINT, Rgba::from_u32(TINT_FROM));
        // The follower waits at the path's start until its tween begins.
        let a = self.geom.sample(0.0);
        h.seed(FOLLOW, PX, a.pos[0]);
        h.seed(FOLLOW, PY, a.pos[1]);
        h.seed(FOLLOW, PR, a.angle_deg);
        let power1 = parse_gsap_ease("power1.inOut").unwrap_or(Easing::InOutQuad);
        // gsap.timeline({paused: true, id: "sequence"}), kept for the page.
        let tl = h.timeline(
            TimelineOpts::new()
                .paused(true)
                .keep(true)
                .tag(SEQ)
                .watch_labels()
                .events(EventMask::EDGES),
        );
        h.tl(tl)
            .add_label(INTRO, 0.0)
            .from_to(
                TITLE.into(),
                &[
                    PropTo::from_to(ALPHA, 0.0.into(), 1.0.into()),
                    PropTo::from_to(LIFT, 12.0.into(), 0.0.into()),
                ],
                TweenOpts::new()
                    .duration(0.6)
                    .ease(Easing::OutCubic)
                    .tag(T_TITLE),
                Position::at(0.0),
            )
            .from_to(
                DOTS,
                &[PropTo::from_to(SCALE, 0.0.into(), 1.0.into())],
                TweenOpts::new()
                    .duration(0.4)
                    .ease(Easing::OutBack)
                    .stagger(Stagger::each(0.08))
                    .tag(T_DOTS),
                Position::label_rel(INTRO, 0.3),
            );
        // A nested timeline: three cards one after another, twice (it
        // repeats once after a 0.2 s gap).
        let cards = h.timeline(TimelineOpts::new().tag(T_CARDS).repeat(1).repeat_delay(0.2));
        for (i, t) in T_CARD.iter().enumerate() {
            h.tl(cards).from_to(
                TargetId(CARD0 + i as u32).into(),
                &[PropTo::from_to(SLIDE, 40.0.into(), 0.0.into())],
                TweenOpts::new().duration(0.3).tag(*t),
                Position::END,
            );
        }
        let path = self.path;
        h.tl(tl)
            .add(cards, Position::prev_start(0.4))
            .call(T_HALF, Position::at(1.4))
            .add_label(TRAVEL, Position::at(1.6))
            .to(
                FOLLOW.into(),
                &[PropTo::path(
                    PX,
                    PY,
                    path,
                    PathOpts::new().auto_rotate(PR, 0.0),
                )],
                TweenOpts::new().duration(1.6).ease(power1).tag(T_PATH),
                Position::label(TRAVEL),
            )
            .to(
                TINT_T.into(),
                &[PropTo::to(TINT, Rgba::from_u32(TINT_TO).into())],
                TweenOpts::new()
                    .duration(0.8)
                    .ease(Easing::InOutSine)
                    .color_space(ColorSpace::Oklch)
                    .tag(T_TINT),
                Position::label_rel(TRAVEL, 0.4),
            );
        self.tl = tl;
    }

    /// Rebuilds the sequencer's model from the engine (after the build and
    /// after every applied edit; never per frame).
    fn sync_model(&mut self, cx: &mut Cx) {
        let m = self.engine_model();
        self.view.sequencer(cx, ids!(seq)).set_model(cx, m);
    }

    /// The sequencer's model of the timeline, the path row coloured.
    fn engine_model(&self) -> SequencerModel {
        let e = &self.motion.engine;
        let mut m = SequencerModel::from_engine(e, self.tl, |id, t| name_for(e, id, t));
        for t in m.tracks.iter_mut() {
            if e.anim_ref(TweenId::from_bits(t.id)).has_path() {
                let c = self.path_color;
                t.color = Some([c.x, c.y, c.z, c.w]);
            }
        }
        m
    }

    /// Pulls the current values into the canvas (draw time).
    fn fill_canvas(&mut self, cx: &mut Cx) {
        let canvas = self.view.story_seq_canvas(cx, ids!(canvas));
        let Some(mut canvas) = canvas.borrow_mut() else {
            return;
        };
        let f = &mut canvas.frame;
        if !f.has_path && self.geom.segment_count() > 0 {
            f.path = self.geom.clone();
            f.has_path = true;
        }
        let h = &self.motion;
        f.title = [h.f64(TITLE, ALPHA, 0.0), h.f64(TITLE, LIFT, 12.0)];
        for (i, d) in f.dots.iter_mut().enumerate() {
            *d = h.f64(TargetId(DOT0 + i as u32), SCALE, 0.0);
        }
        for (i, c) in f.cards.iter_mut().enumerate() {
            *c = h.f64(TargetId(CARD0 + i as u32), SLIDE, 40.0);
        }
        let start = self.geom.sample(0.0);
        f.follow = [
            h.f64(FOLLOW, PX, start.pos[0]),
            h.f64(FOLLOW, PY, start.pos[1]),
            h.f64(FOLLOW, PR, start.angle_deg),
        ];
        f.tint = h.rgba(TINT_T, TINT, Rgba::from_u32(TINT_FROM)).to_f32();
    }

    /// Rewrites the readout (labels redraw only when their text changed).
    fn update_readout(&mut self, cx: &mut Cx) {
        let seq = self.view.sequencer(cx, ids!(seq));
        let e = &self.motion.engine;
        let a = e.anim_ref(self.tl);
        let mut s = std::mem::take(&mut self.text);

        s.clear();
        let state = if a.paused() {
            "paused"
        } else if a.is_active() {
            "playing"
        } else {
            "idle"
        };
        let _ = write!(
            s,
            "playhead {:.3} / {:.3} s  {}  label {} (next {})",
            clean(a.total_time()),
            a.total_duration(),
            state,
            opt_tag_name(a.current_label()),
            opt_tag_name(a.next_label())
        );
        self.view.label(cx, ids!(seq_readout_time)).set_text(cx, &s);

        s.clear();
        match seq.selected() {
            Some(id) => {
                seq.with_model(|m| {
                    if let Some((ti, ii)) = m.find(id) {
                        let t = &m.tracks[ti];
                        let it = &t.items[ii];
                        let _ = write!(
                            s,
                            "selected {} ({}): start {:.3} s, dur {:.3} s, delay {:.3}, repeat {}, gap {:.3}, yoyo {}",
                            t.name,
                            t.kind.name(),
                            clean(it.start),
                            clean(it.duration),
                            clean(it.delay),
                            it.repeat,
                            clean(it.repeat_delay),
                            yes_no(it.yoyo)
                        );
                    }
                });
                let n = e.anim_ref(TweenId::from_bits(id));
                let _ = write!(
                    s,
                    " | engine start {:.3} dur {:.3} ts {:.3}",
                    clean(n.start_time()),
                    clean(n.duration()),
                    n.time_scale()
                );
            }
            None => s.push_str("selected -"),
        }
        self.view.label(cx, ids!(seq_readout_sel)).set_text(cx, &s);

        s.clear();
        let (zoom, offset) = seq.zoom();
        self.rows_shown = seq.visible_rows();
        let _ = write!(
            s,
            "zoom {:.1} px/s offset {:.3} s  rows {} visible  zoom actions {}",
            zoom,
            clean(offset),
            self.rows_shown,
            self.zoom_actions
        );
        self.view.label(cx, ids!(seq_readout_view)).set_text(cx, &s);

        s.clear();
        s.push_str("last edit: ");
        if self.last_edit.is_empty() {
            s.push_str("none");
        } else {
            s.push_str(&self.last_edit);
        }
        self.view.label(cx, ids!(seq_readout_edit)).set_text(cx, &s);

        s.clear();
        let h = &self.motion;
        let start = self.geom.sample(0.0);
        let _ = write!(
            s,
            "follow x {:.2} y {:.2} rot {:.1}  title alpha {:.2} lift {:.2}  tint #{:08x}",
            h.f64(FOLLOW, PX, start.pos[0]),
            h.f64(FOLLOW, PY, start.pos[1]),
            h.f64(FOLLOW, PR, start.angle_deg),
            h.f64(TITLE, ALPHA, 0.0),
            clean(h.f64(TITLE, LIFT, 12.0)),
            h.rgba(TINT_T, TINT, Rgba::from_u32(TINT_FROM)).to_u32()
        );
        self.view
            .label(cx, ids!(seq_readout_values))
            .set_text(cx, &s);
        self.text = s;
    }

    /// The "last edit" line for an applied edit.
    fn note_edit(&mut self, seq: &SequencerRef, a: SequencerAction) {
        let e = &mut self.last_edit;
        e.clear();
        match a {
            SequencerAction::ItemMoved { id, start } => {
                seq.with_model(|m| {
                    if let Some((t, _)) = m.find(id) {
                        e.push_str(&m.tracks[t].name);
                    }
                });
                let _ = write!(e, " moved to {:.3} s", clean(start));
            }
            SequencerAction::ItemResized { id, duration } => {
                seq.with_model(|m| {
                    if let Some((t, _)) = m.find(id) {
                        e.push_str(&m.tracks[t].name);
                    }
                });
                let _ = write!(e, " resized to {:.3} s", clean(duration));
            }
            SequencerAction::LabelMoved { id, time } => {
                e.push_str("label ");
                e.push_str(tag_name(Tag(id)).unwrap_or("-"));
                let _ = write!(e, " moved to {:.3} s", clean(time));
            }
            _ => {}
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let tl = self.tl;
        let v = &self.view;
        let (play, pause, reverse, restart) = (
            v.button(cx, ids!(seq_play)),
            v.button(cx, ids!(seq_pause)),
            v.button(cx, ids!(seq_reverse)),
            v.button(cx, ids!(seq_restart)),
        );
        let seq = v.sequencer(cx, ids!(seq));

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

        // The sequencer: scrub = seek (paused while held), edits = the
        // engine's startTime / duration / addLabel, then a fresh model.
        let mut edited = false;
        for a in seq.actions(actions) {
            match a {
                SequencerAction::ScrubStart => {
                    let r = self.motion.engine.anim_ref(tl);
                    self.scrubbing = Some(!r.paused() && r.is_active());
                    self.motion.control(cx, tl).pause();
                    acted = true;
                }
                SequencerAction::Scrub(t) => {
                    self.motion
                        .control(cx, tl)
                        .seek(Seek::Time(t), Emit::Suppress);
                    acted = true;
                }
                SequencerAction::ScrubEnd => {
                    if self.scrubbing == Some(true) {
                        self.motion.control(cx, tl).resume();
                    }
                    self.scrubbing = None;
                    acted = true;
                }
                SequencerAction::ItemMoved { .. }
                | SequencerAction::ItemResized { .. }
                | SequencerAction::LabelMoved { .. } => {
                    if apply_sequencer_edit(&mut self.motion.engine, tl, a) {
                        self.note_edit(&seq, a);
                        edited = true;
                    }
                }
                SequencerAction::Zoom(..) => {
                    self.zoom_actions += 1;
                    acted = true;
                }
                SequencerAction::Selected(_) | SequencerAction::EditEnd => acted = true,
                _ => {}
            }
        }
        if edited {
            self.sync_model(cx);
            // The refreshed values are new: arm a frame so they are reported.
            self.motion.draw_check(cx);
            self.view.widget(cx, ids!(canvas)).redraw(cx);
        }
        if acted || edited {
            self.update_readout(cx);
        }
    }
}

impl Widget for StorySeqStage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let built = self.ensure_built(cx.cx.cx);
        self.motion.draw_check(cx.cx.cx);
        let rows = self.view.sequencer(cx, ids!(seq)).visible_rows();
        if built || !self.shown || rows != self.rows_shown {
            // The readout is written from the event side, on the next frame.
            self.shown = true;
            self.rows_shown = rows;
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
            // Callbacks are not used here: drained so the queue stays empty.
            self.motion.swap_events(&mut self.events);
            self.motion.clear_changes();
            let t = self.motion.engine.anim_ref(self.tl).total_time();
            self.view.sequencer(cx, ids!(seq)).set_playhead(cx, t);
            self.update_readout(cx);
            self.view.widget(cx, ids!(canvas)).redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/sequencer",
    category: "Foundations",
    component: "Motion",
    also: &["Sequencer"],
    name: "Sequencer",
    dsl: "FoundationsMotionSequencer",
    added: "2026-09-25",
    tags: &["new", "tween", "timeline", "sequencer", "timeline editor", "scrub", "gsap", "motion path"],
    doc: "# Sequencer\n\n`Sequencer` is a timeline editor (the idea of ImSequencer and GSAP's GSDevTools, rebuilt): rows of bars under a time ruler, with a name column, foldable rows, labels on the ruler and a draggable playhead. It shows a `SequencerModel`, plain data any timeline source can fill: tracks (a pre-order tree by `depth`) of items with a start, a delay, a duration, repeats, a repeat delay and yoyo, and named labels. `SequencerModel::from_engine(&engine, timeline, name_of)` fills it from a `makepad_tween` timeline: the timeline itself on the first row, then every child in start order, stagger and keyframes groups and nested timelines as foldable rows.\n\n- **Scrub**: press or drag on the ruler (`ScrubStart`, `Scrub(t)`, `ScrubEnd`); Home / End jump to either end.\n- **Edit**: drag a bar to move it, an edge to resize it, a label flag to move the label (`ItemMoved`, `ItemResized`, `LabelMoved`, then `EditEnd`). Edits snap within 6 px to 0, the end, the playhead, the labels and the start and end of every bar outside the dragged one's subtree, taken when the drag starts (a nested timeline or the last bar never snaps to itself); Alt places freely; Escape puts the bar back. Left / Right nudge the selected bar by one minor tick (Shift: a major one). A stagger group's members are locked rows.\n- **View**: Ctrl + wheel (Cmd on macOS) zooms about the pointer, Shift + wheel, a horizontal wheel or a drag on empty space pans, and the corner's -, + and Fit buttons zoom (`Zoom(px_per_s, offset)`).\n\nThe widget only emits actions; the host applies them. For a tween timeline `apply_sequencer_edit(&mut engine, timeline, action)` maps an edit to GSAP's `startTime()`, `duration()` and `addLabel()` and refreshes the timeline so the values at the playhead follow; the host then rebuilds the model. `set_playhead` moves the playhead quad in place (no redraw of its own) while the view holds still. On this page the readout lines change every playback frame and redraw the page, so the sequencer is drawn every frame here anyway; it allocates nothing when it is.\n\n## This page\n\nOne paused timeline: a title (from_to), a stagger of eight dots from the label `intro` + 0.3 (its group is open, the dots locked), a nested timeline of three cards that repeats once after a 0.2 s gap (resizing its bar rescales it, gap included), a call at 1.4 s, a tween along a motion path from the label `travel` (the coloured row) and a colour tween in OKLCH. Play, Pause, Reverse (toggles the direction) and Restart are the transport; scrubbing pauses it and seeks with events suppressed.\n\n## The readout\n\n`/snap?q=seq_readout` reads `seq_readout_time` (playhead, total, state, current and next label), `seq_readout_sel` (the selected item as the widget has it, and its start, duration and time scale read back from the engine), `seq_readout_view` (zoom, offset, visible rows, how many `Zoom` actions arrived), `seq_readout_edit` (the last applied edit) and `seq_readout_values` (the follower's x, y and angle, the title, the tint). `/snap?q=seq` gives the widget's geometry: `t= dur= zoom= off= name_w= ruler_h= row_h= sel= rows=[name@start+duration, ..] labels=[..]`.",
    subject: "stage",
    feature: None,
    controls: &[
        Control { label: "Row height", target: "seq", kind: ControlKind::Number { prop: "row_height", min: 14., max: 48., step: 1., default: 22. } },
        Control { label: "Snap distance", target: "seq", kind: ControlKind::Number { prop: "snap_px", min: 0., max: 24., step: 1., default: 6. } },
        Control { label: "Follow the playhead", target: "seq", kind: ControlKind::Bool { prop: "follow", default: true } },
    ],
    on_actions: None,
}];
