//! The tween playground: one GSAP-style timeline (a title, a staggered grid
//! of dots and a colour tween) with the playback controls GSAP users expect,
//! and a text readout of everything it does.
//!
//! The readout, right under the transport so it is on screen without
//! scrolling, is the page's evidence where a screenshot is not available:
//! every line is a label with an id, rewritten on each frame that changed
//! something, so `/snap?q=readout` on the `--remote` surface reads the
//! playhead, the state, the current label, the values and the last events.
use crate::makepad_widgets::animator::Ease;
use crate::makepad_widgets::tween::{
    parse_gsap_ease, prop, set_tween_ticker, tag, tween_ticker_ref, ColorSpace, Easing, Emit,
    EventKind, EventMask, Position, PropKey, PropTo, Rgba, Seek, Stagger, StaggerAxis, StaggerFrom,
    Tag, TargetId, Targets, TimelineOpts, TweenEvent, TweenHost, TweenId, TweenOpts, TweenValue,
    BIG, CSS_PRESETS,
};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryTweenCanvasBase = #(StoryTweenCanvas::register_widget(vm))
    /** The stage the playground draws: a title bar, an 8 by 5 grid of dots and two colour swatches, at the values the timeline last wrote. */
    mod.storybook.StoryTweenCanvas = set_type_default() do mod.storybook.StoryTweenCanvasBase{
        width: Fill
        height: 300
        title_color: theme.color_primary
        dot_color: theme.color_tertiary
        draw_panel +: {
            color: theme.color_surface_container_low
        }
        draw_ghost +: {
            color: theme.color_outline_variant
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                sdf.circle(c.x, c.y, min(c.x, c.y))
                sdf.fill(self.color)
                return sdf.result
            }
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

    let Caption = Label{
        draw_text +: {color: theme.color_on_surface_variant}
    }

    mod.storybook.StoryTweenStageBase = #(StoryTweenStage::register_widget(vm))
    /** A timeline over a title, a grid of dots and two swatches, with its transport, its knobs and a text readout. */
    mod.storybook.StoryTweenStage = set_type_default() do mod.storybook.StoryTweenStageBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        /** the timeline's default tween duration in seconds 0.05..3 step 0.05 */
        duration: 0.6
        /** the timeline's own time scale (GSAP timeScale) 0.1..4 step 0.1 */
        time_scale: 1.0
        /** seconds between neighbouring dots (each), or a fifth of the whole span (amount) 0..0.5 step 0.01 */
        stagger_each: 0.05
        /** the timeline's default ease, used by the title tween */
        ease: theme.motion_ease_standard
        /** mirror every other iteration */
        yoyo: false
        /** extra iterations, -1 forever -1..5 step 1 */
        repeat: 0.0

        StoryRow{
            play := Button{text: "Play"}
            pause := Button{text: "Pause"}
            resume := Button{text: "Resume"}
            reverse := Button{text: "Reverse"}
            restart := Button{text: "Restart"}
            pause_at_grid := CheckBox{text: "addPause at grid"}
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
        readout_time := Readout{}
        readout_progress := Readout{}
        readout_state := Readout{}
        readout_label := Readout{}
        readout_values := Readout{}
        readout_setup := Readout{}
        readout_events := Readout{}
        canvas := mod.storybook.StoryTweenCanvas{}
        StoryRow{
            Caption{text: "Jump to label"}
            jump_intro := Button{text: "intro"}
            jump_grid := Button{text: "grid"}
            jump_outro := Button{text: "outro"}
            Caption{text: "Ease"}
            ease_pick := DropDown{
                width: 260.
                popup_menu +: {width: 260.}
                labels: ["theme ease (Controls tab)"]
            }
        }
        StoryRow{
            Caption{text: "Stagger from"}
            stagger_from := SegmentedControl{options: ["Index 0" "Start" "Center" "Edges" "End" "Random" "Index 13"] selected: 2}
        }
        StoryRow{
            Caption{text: "Axis"}
            stagger_axis := SegmentedControl{options: ["Both" "X" "Y"] selected: 0}
            Caption{text: "Spread"}
            stagger_spread := SegmentedControl{options: ["Each" "Amount"] selected: 0}
        }
        StoryRow{
            Caption{text: "Right swatch space"}
            color_space := SegmentedControl{options: ["sRGB" "Linear" "HSV" "OKLab" "OKLCH"] selected: 4}
        }
        StoryRow{
            Caption{text: "Global ticker"}
            ticker_pause := CheckBox{text: "Pause all"}
            ticker_speed := Slider{
                width: 260.
                text: "Global speed"
                min: 0.1
                max: 4.
                step: 0.05
                default: 1.
                precision: 2
            }
            reduced := CheckBox{text: "Reduced motion"}
        }
    }

    mod.stories.FoundationsMotionTween = StoryPage{
        StoryNote{text: "One timeline, built the way GSAP builds one: a title fades and lifts in, the label `grid` sits 0.1 s before it ends, forty dots pop in with a stagger from that label, a call fires when they are done, and after the label `outro` two swatches change colour, the left one in sRGB and the right one in the space picked below. Press Play. Drag the playhead to scrub, jump to a label, change a knob and the timeline is rebuilt where it was."}
        StoryHeading{text: "A timeline with labels, a stagger and a colour tween"}
        stage := mod.storybook.StoryTweenStage{}
    }
}

// ---------------------------------------------------------------------------
// Targets, properties and tags
// ---------------------------------------------------------------------------

const COLS: u32 = 8;
const ROWS: u32 = 5;
const DOTS: usize = (COLS * ROWS) as usize;

const TITLE: TargetId = TargetId(0);
const TINT_SRGB: TargetId = TargetId(1);
const TINT_SPACE: TargetId = TargetId(2);
/// The first dot's target; dot `i` is `DOT0 + i`.
const DOT0: u32 = 16;

const ALPHA: PropKey = prop(live_id!(alpha));
const LIFT: PropKey = prop(live_id!(lift));
const SCALE: PropKey = prop(live_id!(scale));
const TINT: PropKey = prop(live_id!(tint));

const TL: Tag = tag(live_id!(tl));
const INTRO: Tag = tag(live_id!(intro));
const GRID: Tag = tag(live_id!(grid));
const OUTRO: Tag = tag(live_id!(outro));
const GRID_DONE: Tag = tag(live_id!(grid_done));
const GRID_PAUSE: Tag = tag(live_id!(grid_pause));

const TAG_NAMES: &[(Tag, &str)] = &[
    (TL, "tl"),
    (INTRO, "intro"),
    (GRID, "grid"),
    (OUTRO, "outro"),
    (GRID_DONE, "grid_done"),
    (GRID_PAUSE, "grid_pause"),
];

const TINT_FROM: u32 = 0xff5040ff;
const TINT_TO: u32 = 0x40a0ffff;

/// GSAP ease strings offered by the ease picker, parsed with
/// `parse_gsap_ease`. The CSS presets follow them.
const GSAP_EASES: &[&str] = &[
    "none",
    "power1.out",
    "power2.out",
    "power3.out",
    "power4.out",
    "power2.in",
    "power2.inOut",
    "sine.inOut",
    "expo.out",
    "circ.out",
    "back.out(1.7)",
    "back.inOut(1.7)",
    "elastic.out(1, 0.3)",
    "bounce.out",
    "steps(5)",
];

const FROM_NAMES: &[&str] = &[
    "Index 0", "Start", "Center", "Edges", "End", "Random", "Index 13",
];
const AXIS_NAMES: &[&str] = &["Both", "X", "Y"];
const SPREAD_NAMES_LC: &[&str] = &["each", "amount"];
const SPACE_NAMES: &[&str] = &["sRGB", "Linear", "HSV", "OKLab", "OKLCH"];
/// [`SPACE_NAMES`] as the readout prints them (no per-frame lowercasing).
const SPACE_NAMES_LC: &[&str] = &["srgb", "linear", "hsv", "oklab", "oklch"];
const SPACES: &[ColorSpace] = &[
    ColorSpace::Srgb,
    ColorSpace::Linear,
    ColorSpace::Hsv,
    ColorSpace::Oklab,
    ColorSpace::Oklch,
];

/// How many events the readout keeps.
const EVENT_RING: usize = 6;

fn tag_name(t: Tag) -> &'static str {
    TAG_NAMES
        .iter()
        .find(|(k, _)| *k == t)
        .map(|(_, n)| *n)
        .unwrap_or("-")
}

fn opt_tag_name(t: Option<Tag>) -> &'static str {
    t.map(tag_name).unwrap_or("-")
}

/// Writes the ease picker's row name `i` into `s`: the theme ease, the GSAP
/// strings, then the CSS presets.
fn write_ease_label(s: &mut String, i: usize) {
    if i == 0 {
        s.push_str("theme ease (Controls tab)");
    } else if i <= GSAP_EASES.len() {
        s.push_str(GSAP_EASES[i - 1]);
    } else {
        let css = i - 1 - GSAP_EASES.len();
        s.push_str("css ");
        s.push_str(CSS_PRESETS.get(css).map(|(n, _)| *n).unwrap_or("-"));
    }
}

/// The ease picker's row name `i`, as an owned string (the picker's labels,
/// written once).
fn ease_label(i: usize) -> String {
    let mut s = String::new();
    write_ease_label(&mut s, i);
    s
}

fn ease_count() -> usize {
    1 + GSAP_EASES.len() + CSS_PRESETS.len()
}

/// The ease picked in row `i`, or `None` for the theme ease. Parses: call
/// it when the pick changes, never per frame.
fn picked_ease(i: usize) -> Option<Easing> {
    if i == 0 {
        None
    } else if i <= GSAP_EASES.len() {
        parse_gsap_ease(GSAP_EASES[i - 1])
    } else {
        CSS_PRESETS
            .get(i - 1 - GSAP_EASES.len())
            .and_then(|(n, _)| Easing::css_preset(n))
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

// ---------------------------------------------------------------------------
// The canvas
// ---------------------------------------------------------------------------

/// What the canvas draws: the values the timeline wrote, pulled once per
/// drawn frame by the stage.
#[derive(Clone, Copy)]
struct StageFrame {
    /// Title alpha and lift.
    title: [f32; 2],
    /// Per dot: scale and lift.
    dots: [[f32; 2]; DOTS],
    tint_srgb: [f32; 4],
    tint_space: [f32; 4],
    space: usize,
}

impl Default for StageFrame {
    fn default() -> Self {
        Self {
            title: [0.0, 12.0],
            dots: [[0.0, 8.0]; DOTS],
            tint_srgb: Rgba::from_u32(TINT_FROM).to_f32(),
            tint_space: Rgba::from_u32(TINT_FROM).to_f32(),
            space: 4,
        }
    }
}

const PAD: f64 = 16.0;
const PITCH: f64 = 30.0;
const DOT: f64 = 18.0;
const SWATCH: f64 = 96.0;

#[derive(Script, ScriptHook, Widget)]
pub struct StoryTweenCanvas {
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
    draw_ghost: DrawColor,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_bar: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    title_color: Vec4f,
    #[live]
    dot_color: Vec4f,
    #[rust]
    frame: StageFrame,
}

impl Widget for StoryTweenCanvas {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let f = self.frame;
        self.draw_panel.draw_abs(cx, r);

        // Title bar: alpha and lift.
        let title_w = (r.size.x - 2.0 * PAD).clamp(0.0, 8.0 * PITCH);
        let mut c = self.title_color;
        c.w *= f.title[0].clamp(0.0, 1.0);
        self.draw_bar.color = c;
        self.draw_bar.draw_abs(
            cx,
            Rect {
                pos: dvec2(r.pos.x + PAD, r.pos.y + PAD + f.title[1] as f64),
                size: dvec2(title_w, 24.0),
            },
        );

        // The grid: a faint slot per dot, then the dot at its scale and lift.
        let top = r.pos.y + PAD + 24.0 + PAD;
        for (i, d) in f.dots.iter().enumerate() {
            let col = (i as u32 % COLS) as f64;
            let row = (i as u32 / COLS) as f64;
            let cx0 = r.pos.x + PAD + col * PITCH + PITCH * 0.5;
            let cy0 = top + row * PITCH + PITCH * 0.5;
            self.draw_ghost.draw_abs(
                cx,
                Rect {
                    pos: dvec2(cx0 - 2.0, cy0 - 2.0),
                    size: dvec2(4.0, 4.0),
                },
            );
            let size = DOT * (d[0].max(0.0) as f64);
            if size > 0.01 {
                self.draw_dot.color = self.dot_color;
                self.draw_dot.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(cx0 - size * 0.5, cy0 - size * 0.5 + d[1] as f64),
                        size: dvec2(size, size),
                    },
                );
            }
        }

        // Two swatches right of the grid: sRGB and the picked space.
        let sx = r.pos.x + PAD + COLS as f64 * PITCH + 2.0 * PAD;
        for (k, tint) in [f.tint_srgb, f.tint_space].iter().enumerate() {
            let x = sx + k as f64 * (SWATCH + PAD);
            self.draw_bar.color = Vec4f {
                x: tint[0],
                y: tint[1],
                z: tint[2],
                w: tint[3],
            };
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, top),
                    size: dvec2(SWATCH, SWATCH),
                },
            );
            let name = if k == 0 {
                SPACE_NAMES[0]
            } else {
                SPACE_NAMES.get(f.space).copied().unwrap_or("-")
            };
            self.draw_text
                .draw_abs(cx, dvec2(x, top + SWATCH + 6.0), name);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// ---------------------------------------------------------------------------
// The stage
// ---------------------------------------------------------------------------

/// The on-page picks (segmented controls, the ease picker, the pause box).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Picks {
    from: usize,
    axis: usize,
    spread: usize,
    space: usize,
    ease: usize,
    pause_at_grid: bool,
}

impl Default for Picks {
    fn default() -> Self {
        Self {
            from: 2,
            axis: 0,
            spread: 0,
            space: 4,
            ease: 0,
            pause_at_grid: false,
        }
    }
}

/// Everything the timeline was built from; a change rebuilds it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Knobs {
    duration: f64,
    stagger_each: f64,
    ease: Easing,
    yoyo: bool,
    repeat: i32,
    picks: Picks,
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryTweenStage {
    #[deref]
    view: View,
    #[live(0.6)]
    duration: f64,
    #[live(1.0)]
    time_scale: f64,
    #[live(0.05)]
    stagger_each: f64,
    #[live(Ease::Bezier { cp0: 0.2, cp1: 0.0, cp2: 0.0, cp3: 1.0 })]
    ease: Ease,
    #[live]
    yoyo: bool,
    #[live(0.0)]
    repeat: f64,
    #[rust]
    motion: TweenHost,
    #[rust]
    tl: TweenId,
    #[rust]
    built_from: Option<Knobs>,
    #[rust]
    picks: Picks,
    /// The ease of `picks.ease`, parsed when the pick changes.
    #[rust]
    picked: Option<Easing>,
    #[rust]
    ev_buf: Vec<TweenEvent>,
    #[rust]
    last_events: [Option<TweenEvent>; EVENT_RING],
    #[rust]
    ev_next: usize,
    #[rust]
    text: String,
    /// While the playhead slider is held: whether the timeline was playing.
    #[rust]
    scrubbing: Option<bool>,
    /// A value is being typed into the playhead's text field: the playhead
    /// write-back leaves the field alone until Enter, Escape or focus loss.
    #[rust]
    typing: bool,
    /// The setup line needs rewriting (a rebuild or a pick change).
    #[rust]
    setup_dirty: bool,
    /// A one-off frame that refreshes the readout (first draw, ticker change).
    #[rust]
    refresh: NextFrame,
    #[rust]
    seen_epoch: Option<u64>,
    #[rust]
    frames: u64,
    #[rust]
    labels_set: bool,
}

impl StoryTweenStage {
    fn knobs(&self) -> Knobs {
        let ease = self.picked.unwrap_or_else(|| Easing::from(&self.ease));
        Knobs {
            duration: self.duration.max(0.0),
            stagger_each: self.stagger_each.max(0.0),
            ease,
            yoyo: self.yoyo,
            repeat: self.repeat.round().clamp(-1.0, 1000.0) as i32,
            picks: self.picks,
        }
    }

    fn stagger(k: &Knobs) -> Stagger {
        let s = if k.picks.spread == 0 {
            Stagger::each(k.stagger_each)
        } else {
            Stagger::amount(k.stagger_each * 5.0)
        };
        let from = match k.picks.from {
            0 => StaggerFrom::Index(0),
            1 => StaggerFrom::Start,
            2 => StaggerFrom::Center,
            3 => StaggerFrom::Edges,
            4 => StaggerFrom::End,
            5 => StaggerFrom::Random(7),
            _ => StaggerFrom::Index(13),
        };
        let s = s.from(from).grid(ROWS, COLS);
        match k.picks.axis {
            1 => s.axis(StaggerAxis::X),
            2 => s.axis(StaggerAxis::Y),
            _ => s,
        }
    }

    /// Builds (or rebuilds) the timeline when a knob changed. A rebuild
    /// kills the old timeline and lands the new one where the old one was,
    /// keeping its direction and whether it was playing: at the same total
    /// progress, or, when either repeats forever (a total progress there is
    /// meaningless), in the same iteration at the same iteration progress.
    fn ensure_built(&mut self, cx: &mut Cx) {
        let k = self.knobs();
        let alive = self.motion.engine.anim_ref(self.tl).is_alive();
        if !alive || self.built_from != Some(k) {
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
        }
        // GSAP timeScale() is signed (negative while reversed): compare the
        // magnitude and keep the direction.
        let ts = self.time_scale.max(0.01);
        let a = self.motion.engine.anim_ref(self.tl);
        if (a.time_scale().abs() - ts).abs() > 1e-9 {
            let signed = if a.reversed() { -ts } else { ts };
            let tl = self.tl;
            self.motion.control(cx, tl).set_time_scale(signed);
        }
    }

    fn build(&mut self, k: &Knobs) {
        let h = &mut self.motion;
        // The values `to()` starts from (GSAP reads them off the targets).
        h.seed(TITLE, ALPHA, 0.0);
        h.seed(TITLE, LIFT, 12.0);
        for i in 0..DOTS as u32 {
            h.seed(TargetId(DOT0 + i), SCALE, 0.0);
            h.seed(TargetId(DOT0 + i), LIFT, 8.0);
        }
        h.seed(TINT_SRGB, TINT, Rgba::from_u32(TINT_FROM));
        h.seed(TINT_SPACE, TINT, Rgba::from_u32(TINT_FROM));

        let space = SPACES.get(k.picks.space).copied().unwrap_or_default();
        let dots = Targets::Range {
            first: DOT0,
            count: DOTS as u32,
        };
        let tint_to = TweenValue::from(Rgba::from_u32(TINT_TO));
        let tint_opts = TweenOpts::new().duration(0.6).ease(Easing::InOutSine);
        // gsap.timeline({paused: true, repeat, yoyo, defaults: {duration, ease}})
        let tl = h.timeline(
            TimelineOpts::new()
                .defaults(TweenOpts::new().duration(k.duration).ease(k.ease))
                .repeat(k.repeat)
                .yoyo(k.yoyo)
                .events(EventMask::EDGES)
                .watch_labels()
                .tag(TL)
                .paused(true),
        );
        h.tl(tl)
            .add_label(INTRO, Position::at(0.0))
            .from_to(
                TITLE.into(),
                &[
                    PropTo::from_to(ALPHA, 0.0.into(), 1.0.into()),
                    PropTo::from_to(LIFT, 12.0.into(), 0.0.into()),
                ],
                TweenOpts::new(),
                Position::at(0.0),
            )
            .add_label(GRID, Position::prev_end(-0.1))
            .from_to(
                dots,
                &[
                    PropTo::from_to(SCALE, 0.0.into(), 1.0.into()),
                    PropTo::from_to(LIFT, 8.0.into(), 0.0.into()),
                ],
                TweenOpts::new()
                    .duration(0.35)
                    .ease(Easing::OutBack)
                    .stagger(Self::stagger(k)),
                Position::label(GRID),
            )
            .call(GRID_DONE, Position::END)
            .add_label(OUTRO, Position::rel(0.4))
            .to(
                TINT_SRGB.into(),
                &[PropTo::to(TINT, tint_to)],
                tint_opts.color_space(ColorSpace::Srgb),
                Position::label(OUTRO),
            )
            .to(
                TINT_SPACE.into(),
                &[PropTo::to(TINT, tint_to)],
                tint_opts.color_space(space),
                Position::label(OUTRO),
            );
        if k.picks.pause_at_grid {
            h.tl(tl).add_pause(Position::label(GRID), GRID_PAUSE);
        }
        self.tl = tl;
    }

    /// Pulls the current values into the canvas (draw time: the same clock
    /// the frame step used).
    fn fill_canvas(&mut self, cx: &mut Cx) {
        let canvas = self.view.story_tween_canvas(cx, ids!(canvas));
        let Some(mut canvas) = canvas.borrow_mut() else {
            return;
        };
        let h = &self.motion;
        let f = &mut canvas.frame;
        f.title = [
            h.f64(TITLE, ALPHA, 0.0) as f32,
            h.f64(TITLE, LIFT, 12.0) as f32,
        ];
        for (i, d) in f.dots.iter_mut().enumerate() {
            let t = TargetId(DOT0 + i as u32);
            *d = [h.f64(t, SCALE, 0.0) as f32, h.f64(t, LIFT, 8.0) as f32];
        }
        let from = Rgba::from_u32(TINT_FROM);
        f.tint_srgb = h.rgba(TINT_SRGB, TINT, from).to_f32();
        f.tint_space = h.rgba(TINT_SPACE, TINT, from).to_f32();
        f.space = self.picks.space;
    }

    fn record_events(&mut self) {
        self.motion.swap_events(&mut self.ev_buf);
        for ev in &self.ev_buf {
            // Updates would drown the rest; none are asked for anyway.
            if ev.kind == EventKind::Update {
                continue;
            }
            self.last_events[self.ev_next % EVENT_RING] = Some(*ev);
            self.ev_next = self.ev_next.wrapping_add(1);
        }
    }

    /// Rewrites every readout line (labels only redraw when their text
    /// changed) and puts the playhead back on the slider.
    fn update_readout(&mut self, cx: &mut Cx) {
        let tk = tween_ticker_ref(cx);
        let a = self.motion.engine.anim_ref(self.tl);
        let (time, dur, ttime, tdur) = (a.time(), a.duration(), a.total_time(), a.total_duration());
        let (prog, tprog, iter, rep) =
            (a.progress(), a.total_progress(), a.iteration(), a.repeat());
        let (paused, active, reversed, scale) =
            (a.paused(), a.is_active(), a.reversed(), a.time_scale());
        let (cur, next, prev) = (a.current_label(), a.next_label(), a.previous_label());
        // Repeating forever, a total progress is 0 for the whole run: the
        // playhead then shows (and scrubs) the iteration progress.
        let infinite = tdur >= BIG;
        let h = &self.motion;
        let mut s = std::mem::take(&mut self.text);

        s.clear();
        let _ = write!(s, "time {:.3} / {:.3} s  total {:.3} / ", time, dur, ttime);
        if infinite {
            s.push_str("inf");
        } else {
            let _ = write!(s, "{:.3}", tdur);
        }
        let _ = write!(s, " s  frame {}", self.frames);
        self.view.label(cx, ids!(readout_time)).set_text(cx, &s);

        s.clear();
        let _ = write!(
            s,
            "progress {:.3}  total {:.3}  iteration {} of ",
            prog, tprog, iter
        );
        if rep < 0 {
            s.push_str("inf");
        } else {
            let _ = write!(s, "{}", rep + 1);
        }
        s.push_str(if infinite {
            "  playhead: iteration progress"
        } else {
            "  playhead: total progress"
        });
        self.view.label(cx, ids!(readout_progress)).set_text(cx, &s);

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
            "{}, {}, scale {:.2}, ticker x{:.2} paused:{} reduced:{}",
            state,
            if reversed { "reversed" } else { "forward" },
            scale.abs(),
            tk.time_scale,
            yes_no(tk.paused),
            yes_no(tk.reduced_motion)
        );
        self.view.label(cx, ids!(readout_state)).set_text(cx, &s);

        s.clear();
        let _ = write!(
            s,
            "label {} (next {}, previous {})",
            opt_tag_name(cur),
            opt_tag_name(next),
            opt_tag_name(prev)
        );
        self.view.label(cx, ids!(readout_label)).set_text(cx, &s);

        s.clear();
        let _ = write!(
            s,
            "title a={:.3} y={:.2}",
            h.f64(TITLE, ALPHA, 0.0),
            h.f64(TITLE, LIFT, 0.0)
        );
        for i in [0u32, 19, 39] {
            let t = TargetId(DOT0 + i);
            let _ = write!(
                s,
                " | dot{} s={:.2} y={:.2}",
                i,
                h.f64(t, SCALE, 0.0),
                h.f64(t, LIFT, 0.0)
            );
        }
        let from = Rgba::from_u32(TINT_FROM);
        let _ = write!(
            s,
            " | tint srgb #{:08x} {} #{:08x}",
            h.rgba(TINT_SRGB, TINT, from).to_u32(),
            SPACE_NAMES_LC.get(self.picks.space).copied().unwrap_or("-"),
            h.rgba(TINT_SPACE, TINT, from).to_u32()
        );
        self.view.label(cx, ids!(readout_values)).set_text(cx, &s);

        // The setup changes only with a rebuild (every pick and knob is part
        // of one), not per frame.
        if self.setup_dirty {
            self.setup_dirty = false;
            s.clear();
            let p = self.picks;
            s.push_str("ease ");
            write_ease_label(&mut s, p.ease);
            let _ = write!(
                s,
                " | stagger from {} axis {} {} {:.3} | right swatch {} | duration {:.2} repeat {} yoyo {} addPause {}",
                FROM_NAMES.get(p.from).copied().unwrap_or("-"),
                AXIS_NAMES.get(p.axis).copied().unwrap_or("-"),
                SPREAD_NAMES_LC.get(p.spread).copied().unwrap_or("-"),
                if p.spread == 0 {
                    self.stagger_each
                } else {
                    self.stagger_each * 5.0
                },
                SPACE_NAMES.get(p.space).copied().unwrap_or("-"),
                self.duration,
                self.repeat.round() as i32,
                yes_no(self.yoyo),
                yes_no(p.pause_at_grid)
            );
            self.view.label(cx, ids!(readout_setup)).set_text(cx, &s);
        }

        s.clear();
        s.push_str("events:");
        let count = self.ev_next.min(EVENT_RING);
        if count == 0 {
            s.push_str(" none yet");
        }
        for k in 0..count {
            let ix = (self.ev_next - count + k) % EVENT_RING;
            let Some(ev) = self.last_events[ix] else {
                continue;
            };
            let sep = if k == 0 { " " } else { "; " };
            let _ = match ev.kind {
                EventKind::Start => write!(s, "{sep}Start {}", tag_name(ev.tag)),
                EventKind::Update => write!(s, "{sep}Update {}", tag_name(ev.tag)),
                EventKind::Repeat => write!(s, "{sep}Repeat {}", tag_name(ev.tag)),
                EventKind::Complete => write!(s, "{sep}Complete {}", tag_name(ev.tag)),
                EventKind::ReverseComplete => {
                    write!(s, "{sep}ReverseComplete {}", tag_name(ev.tag))
                }
                EventKind::Interrupt => write!(s, "{sep}Interrupt {}", tag_name(ev.tag)),
                EventKind::Call { forward } => write!(
                    s,
                    "{sep}Call {} {}",
                    tag_name(ev.tag),
                    if forward { "fwd" } else { "back" }
                ),
                EventKind::Pause => write!(s, "{sep}Pause {}", tag_name(ev.tag)),
                EventKind::Label(l) => write!(s, "{sep}Label {}", tag_name(l)),
            };
        }
        self.view.label(cx, ids!(readout_events)).set_text(cx, &s);
        self.text = s;

        let scrub = self.view.slider(cx, ids!(scrub));
        if self.typing && !scrub.has_text_focus(cx) {
            self.typing = false;
        }
        if self.scrubbing.is_none() && !self.typing {
            scrub.set_value(cx, if infinite { prog } else { tprog });
        }
    }

    /// Puts the ticker row back in step with the ticker (a remote op or
    /// another page may have changed it).
    fn sync_ticker_row(&mut self, cx: &mut Cx) {
        let tk = tween_ticker_ref(cx);
        self.view
            .check_box(cx, ids!(ticker_pause))
            .set_active(cx, tk.paused, Animate::No);
        self.view
            .check_box(cx, ids!(reduced))
            .set_active(cx, tk.reduced_motion, Animate::No);
        self.view
            .slider(cx, ids!(ticker_speed))
            .set_value(cx, tk.time_scale);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let tl = self.tl;
        let v = &self.view;
        let (play, pause, resume, reverse, restart) = (
            v.button(cx, ids!(play)),
            v.button(cx, ids!(pause)),
            v.button(cx, ids!(resume)),
            v.button(cx, ids!(reverse)),
            v.button(cx, ids!(restart)),
        );
        let scrub = v.slider(cx, ids!(scrub));
        let jumps = [
            (v.button(cx, ids!(jump_intro)), INTRO),
            (v.button(cx, ids!(jump_grid)), GRID),
            (v.button(cx, ids!(jump_outro)), OUTRO),
        ];
        let pause_at_grid = v.check_box(cx, ids!(pause_at_grid));
        let ease_pick = v.drop_down(cx, ids!(ease_pick));
        let from = v.segmented_control(cx, ids!(stagger_from));
        let axis = v.segmented_control(cx, ids!(stagger_axis));
        let spread = v.segmented_control(cx, ids!(stagger_spread));
        let space = v.segmented_control(cx, ids!(color_space));
        let ticker_pause = v.check_box(cx, ids!(ticker_pause));
        let ticker_speed = v.slider(cx, ids!(ticker_speed));
        let reduced = v.check_box(cx, ids!(reduced));

        // Transport: GSAP's Animation methods on the timeline. A control
        // that moves nothing (a pause, a reverse at the start) still changes
        // what the readout says, so any of them rewrites it.
        let mut acted = false;
        if play.clicked(actions) {
            self.motion.control(cx, tl).play();
            acted = true;
        }
        if pause.clicked(actions) {
            self.motion.control(cx, tl).pause();
            acted = true;
        }
        if resume.clicked(actions) {
            self.motion.control(cx, tl).resume();
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
        for (button, label) in &jumps {
            if button.clicked(actions) {
                self.motion
                    .control(cx, tl)
                    .seek(Seek::Label(*label, 0.0), Emit::Suppress);
                acted = true;
            }
        }

        // The playhead: hold, scrub (renders now, fires nothing), let go.
        // It is the total progress, or the iteration progress when the
        // timeline repeats forever (its total progress never leaves 0).
        if scrub.start_slide(actions) {
            let a = self.motion.engine.anim_ref(tl);
            self.scrubbing = Some(!a.paused() && a.is_active());
            self.typing = false;
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
        }
        if scrub.end_slide(actions).is_some() {
            if self.scrubbing == Some(true) {
                self.motion.control(cx, tl).resume();
            }
            self.scrubbing = None;
            self.typing = false;
            acted = true;
        }

        // Picks that rebuild the timeline.
        let mut picks = self.picks;
        if let Some(b) = pause_at_grid.changed(actions) {
            picks.pause_at_grid = b;
        }
        if let Some(i) = ease_pick.selected(actions) {
            if i != picks.ease {
                picks.ease = i;
                // Parsed here, once per pick: knobs() reads the result.
                self.picked = picked_ease(i);
            }
        }
        if let Some(i) = from.selected(actions) {
            picks.from = i;
        }
        if let Some(i) = axis.selected(actions) {
            picks.axis = i;
        }
        if let Some(i) = spread.selected(actions) {
            picks.spread = i;
        }
        if let Some(i) = space.selected(actions) {
            picks.space = i;
        }
        if picks != self.picks {
            self.picks = picks;
            self.ensure_built(cx);
        }

        // The app-wide ticker (GSAP globalTimeline / ticker).
        if let Some(b) = ticker_pause.changed(actions) {
            set_tween_ticker(cx, |t| t.paused = b);
        }
        if let Some(s) = ticker_speed.slided(actions) {
            set_tween_ticker(cx, |t| t.time_scale = s);
        }
        if let Some(b) = reduced.changed(actions) {
            set_tween_ticker(cx, |t| t.reduced_motion = b);
        }

        if acted {
            self.update_readout(cx);
        }
    }
}

impl Widget for StoryTweenStage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Knobs from the Controls tab arrive as applies followed by a
        // redraw: this is where a changed knob rebuilds the timeline.
        self.ensure_built(cx.cx.cx);
        self.motion.draw_check(cx.cx.cx);
        let epoch = tween_ticker_ref(cx.cx.cx).epoch;
        if self.seen_epoch != Some(epoch) {
            self.seen_epoch = Some(epoch);
            self.refresh = cx.cx.cx.new_next_frame();
        }
        self.fill_canvas(cx.cx.cx);
        let step = self.view.draw_walk(cx, scope, walk);
        // The picker's rows are data, so they are written from here once
        // the view has been walked and the picker can be found.
        if !self.labels_set {
            let pick = self.view.drop_down(cx.cx.cx, ids!(ease_pick));
            if pick.borrow().is_some() {
                self.labels_set = true;
                pick.set_labels(cx.cx.cx, (0..ease_count()).map(ease_label).collect());
            }
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Keys going to the playhead's value field: the user is typing a
        // position, so the playing timeline stops rewriting the field until
        // Enter (an end_slide), Escape or a focus change.
        match event {
            Event::TextInput(_) => {
                if self.view.slider(cx, ids!(scrub)).has_text_focus(cx) {
                    self.typing = true;
                }
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => self.typing = false,
            Event::KeyDown(ke)
                if matches!(ke.key_code, KeyCode::Backspace | KeyCode::Delete)
                    && self.view.slider(cx, ids!(scrub)).has_text_focus(cx) =>
            {
                self.typing = true
            }
            _ => {}
        }
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
            self.sync_ticker_row(cx);
        }
        let act = self.motion.handle_event(cx, event);
        if act.changed() {
            self.frames += 1;
        }
        if act.changed() || refresh {
            self.record_events();
            // The canvas pulls in draw_walk; the host's change list is only
            // a cue here.
            self.motion.clear_changes();
            self.update_readout(cx);
            // Only the loop-drawn canvas: the readout labels and the playhead
            // redraw themselves when their text or value changes.
            self.view.widget(cx, ids!(canvas)).redraw(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

const EASES: &[&str] = &[
    "theme.motion_ease_standard",
    "theme.motion_ease_standard_decelerate",
    "theme.motion_ease_standard_accelerate",
    "theme.motion_ease_emphasized_decelerate",
    "theme.motion_ease_emphasized_accelerate",
    "theme.motion_ease_linear",
    "theme.motion_ease_spring",
    "theme.motion_ease_bounce",
];

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/tween-and-timeline",
    category: "Foundations",
    component: "Motion",
    also: &[],
    name: "Tween & timeline",
    dsl: "FoundationsMotionTween",
    added: "2026-09-24",
    tags: &["new", "tween", "timeline", "stagger", "gsap", "keyframes", "scrub", "yoyo"],
    doc: "# Tweens and timelines\n\n`makepad_widgets::tween` is GSAP 3's model in Rust: tweens, timelines, positions and labels, staggers, eases, repeat and yoyo, callbacks and playback control. A widget keeps a `TweenHost` in a `#[rust]` field, builds on it, forwards its events to `handle_event` and pushes or pulls the values it reports.\n\n## This page\n\nOne timeline: `from_to` on the title at 0, the label `grid` at `prev_end(-0.1)` (GSAP `\"<-0.1\"` of the end), a staggered `from_to` of forty dots from that label, a `call` at the end, the label `outro` at `rel(0.4)` (GSAP `\"+=0.4\"`) and two colour tweens from it, one in sRGB and one in the space picked on the page.\n\n- **Play, Pause, Resume, Restart** are GSAP's `play()`, `pause()`, `resume()` and `restart()`. **Reverse** toggles the direction and resumes (GSAP's `reverse()` only sets it).\n- **The playhead** seeks the total progress with events suppressed while it is held, then resumes if the timeline was playing. While the timeline plays the slider follows it.\n- **Jump to label** is `seek(\"label\")`.\n- **addPause at grid** stops the playhead at the label and reports a `Pause` event.\n- **Stagger from / axis / spread** are GSAP's `stagger: {from, axis, each | amount, grid: [5, 8]}`.\n- **The ease picker** overrides the timeline's default ease with a GSAP ease string or a CSS preset; its first row uses the ease from the Controls tab.\n- **Global ticker** is the app-wide `TweenTicker`: GSAP's `globalTimeline.pause()` and `timeScale()`, plus reduced motion, which finishes animations instead of playing them.\n\nChanging any knob rebuilds the timeline where it was: same total progress, same direction, still playing if it was.\n\n## The readout\n\nEvery line under the playhead is a label that is rewritten on each frame that changed something: time and total time, progress, iteration, state, the current label, a few current values and the last six events. On the `--remote` surface `/snap?q=readout` reads it, and `/tweak/op?op=tween&scale=0.25&paused=1&reduced=0` sets the ticker.",
    subject: "stage",
    feature: None,
    controls: &[
        Control { label: "Duration", target: "stage", kind: ControlKind::Number { prop: "duration", min: 0.05, max: 3., step: 0.05, default: 0.6 } },
        Control { label: "Time scale", target: "stage", kind: ControlKind::Number { prop: "time_scale", min: 0.1, max: 4., step: 0.1, default: 1. } },
        Control { label: "Stagger", target: "stage", kind: ControlKind::Number { prop: "stagger_each", min: 0., max: 0.5, step: 0.01, default: 0.05 } },
        Control { label: "Ease", target: "stage", kind: ControlKind::Choice { prop: "ease", options: EASES, default: 0 } },
        Control { label: "Yoyo", target: "stage", kind: ControlKind::Bool { prop: "yoyo", default: false } },
        Control { label: "Repeat", target: "stage", kind: ControlKind::Number { prop: "repeat", min: -1., max: 5., step: 1., default: 0. } },
    ],
    on_actions: None,
}];
