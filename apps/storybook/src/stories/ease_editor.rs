//! The ease editor page: an `EaseEditor`, a preview that plays the edited
//! curve through a `TweenHost` on every commit, and the same four numbers
//! drawn twice, once with the runtime's parity solver (`Ease::Bezier`, what
//! the Animator and the theme tokens run) and once with the precise CSS
//! solver (`Easing::CubicBezier`), with their largest difference in text.
//!
//! Every value the page shows is also a label with an id, so `/snap?q=ease`
//! on the `--remote` surface reads the curve, the last action, the preview
//! and the difference where no screenshot is available.
use crate::makepad_widgets::tween::{
    prop, Easing, PropKey, PropTo, TargetId, TweenHost, TweenId, TweenOpts,
};
use crate::makepad_widgets::*;
use crate::registry::{Control, ControlKind, Story};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.StoryEaseScopeBase = #(StoryEaseScope::register_widget(vm))
    /** Two tracks the preview runs along (parity and precise), and the difference between the two solvers as bars scaled to fit. */
    mod.storybook.StoryEaseScope = set_type_default() do mod.storybook.StoryEaseScopeBase{
        width: Fill
        height: 170
        parity_color: theme.color_primary
        precise_color: theme.color_tertiary
        draw_panel +: {
            color: theme.color_surface_container_low
        }
        draw_line +: {
            color: theme.color_outline_variant
        }
        draw_bar +: {
            color: theme.color_tertiary
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

    let Column = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    mod.storybook.StoryEaseLabBase = #(StoryEaseLab::register_widget(vm))
    /** An EaseEditor with a preview that replays the curve on every commit, and the curve drawn by both bezier solvers. */
    mod.storybook.StoryEaseLab = set_type_default() do mod.storybook.StoryEaseLabBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
        /** preview travel time in seconds 0.2..4 step 0.1 */
        preview_secs: 1.2

        ease_readout := Readout{}
        ease_preview := Readout{}
        ease_diff := Readout{}
        StoryRow{
            align: Align{x: 0. y: 0.}
            editor := EaseEditor{}
            Column{
                StoryRow{
                    ease_replay := Button{text: "Replay"}
                    Caption{text: "Both dots run the same four numbers for the same time."}
                }
                scope := mod.storybook.StoryEaseScope{}
                StoryRow{
                    align: Align{x: 0. y: 0.}
                    Column{
                        Caption{text: "Parity solver (Ease::Bezier, the Animator)"}
                        ease_parity := EaseCurve{height: 220}
                    }
                    Column{
                        Caption{text: "Precise solver (CSS cubic-bezier)"}
                        ease_precise := EaseCurve{height: 220}
                    }
                }
            }
        }
    }

    mod.stories.FoundationsMotionEaseEditor = StoryPage{
        StoryNote{text: "Drag a handle, type a number or pick a preset. Every commit replays the preview: one dot eased with the runtime's parity solver, the one Ease::Bezier, the Animator and the theme tokens use, the other with the precise CSS solver. The bars under the tracks are their difference, scaled to fit, and the third line says how large it really is."}
        StoryHeading{text: "Edit a cubic-bezier and watch it play"}
        lab := mod.storybook.StoryEaseLab{}
    }
}

// ---------------------------------------------------------------------------
// The scope: two tracks and the difference bars
// ---------------------------------------------------------------------------

/// Bars in the difference strip.
const BARS: usize = 48;
/// Samples behind the reported largest difference (0..=1 inclusive).
const SAMPLES: usize = 1000;
/// The value range a track shows: back and elastic curves leave 0..1.
const TRACK_LO: f64 = -0.3;
const TRACK_HI: f64 = 1.3;

const PAD: f64 = 12.0;
const NAME_W: f64 = 64.0;
const ROW: f64 = 30.0;
const DOT: f64 = 14.0;

/// What the scope draws, written by the lab.
#[derive(Clone, Copy)]
struct ScopeFrame {
    /// The preview values: parity, precise.
    values: [f64; 2],
    /// Per bar: parity minus precise at the bar's centre, over `peak`.
    bars: [f32; BARS],
}

impl Default for ScopeFrame {
    fn default() -> Self {
        Self {
            values: [0.0; 2],
            bars: [0.0; BARS],
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryEaseScope {
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
    draw_line: DrawColor,
    /// The difference bars, in `precise_color` (the rules and ticks are
    /// `draw_line`).
    #[live]
    draw_bar: DrawColor,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    parity_color: Vec4f,
    #[live]
    precise_color: Vec4f,
    #[rust]
    frame: ScopeFrame,
}

impl StoryEaseScope {
    fn line(&mut self, cx: &mut Cx2d, x: f64, y: f64, w: f64, h: f64) {
        self.draw_line.draw_abs(
            cx,
            Rect {
                pos: dvec2(x, y),
                size: dvec2(w, h),
            },
        );
    }
}

impl Widget for StoryEaseScope {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let f = self.frame;
        self.draw_panel.draw_abs(cx, r);
        let x0 = r.pos.x + PAD + NAME_W;
        let w = (r.size.x - 2.0 * PAD - NAME_W).max(1.0);
        let at = |v: f64| x0 + (v.clamp(TRACK_LO, TRACK_HI) - TRACK_LO) / (TRACK_HI - TRACK_LO) * w;

        // The tracks: a rule, ticks at 0 and 1, the dot at the value.
        for (k, name) in ["parity", "precise"].iter().enumerate() {
            let y = r.pos.y + PAD + k as f64 * ROW + ROW * 0.5;
            self.draw_text
                .draw_abs(cx, dvec2(r.pos.x + PAD, y - 8.0), name);
            self.line(cx, x0, y - 0.5, w, 1.0);
            self.line(cx, at(0.0) - 0.5, y - 6.0, 1.0, 12.0);
            self.line(cx, at(1.0) - 0.5, y - 6.0, 1.0, 12.0);
            self.draw_dot.color = if k == 0 {
                self.parity_color
            } else {
                self.precise_color
            };
            let x = at(f.values[k]);
            self.draw_dot.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x - DOT * 0.5, y - DOT * 0.5),
                    size: dvec2(DOT, DOT),
                },
            );
        }

        // The difference strip: parity minus precise, the peak at full height.
        let top = r.pos.y + PAD + 2.0 * ROW + PAD;
        let half = ((r.pos.y + r.size.y - PAD - top) * 0.5).max(1.0);
        let mid = top + half;
        self.draw_text
            .draw_abs(cx, dvec2(r.pos.x + PAD, mid - 8.0), "difference");
        let bw = w / BARS as f64;
        self.line(cx, x0, mid - 0.5, w, 1.0);
        self.draw_bar.color = self.precise_color;
        for (i, b) in f.bars.iter().enumerate() {
            let h = *b as f64 * half;
            let (y, h) = if h >= 0.0 { (mid - h, h) } else { (mid, -h) };
            if h < 0.5 {
                continue;
            }
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x0 + i as f64 * bw + 1.0, y),
                    size: dvec2((bw - 2.0).max(1.0), h),
                },
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// ---------------------------------------------------------------------------
// The lab
// ---------------------------------------------------------------------------

const ME: TargetId = TargetId(0);
const PARITY: PropKey = prop(live_id!(parity));
const PRECISE: PropKey = prop(live_id!(precise));

/// The runtime (parity) solver of four points: what `Ease::Bezier` runs.
fn parity(p: [f64; 4]) -> Easing {
    Easing::Bezier {
        x1: p[0],
        y1: p[1],
        x2: p[2],
        y2: p[3],
    }
}

/// The precise CSS solver of four points.
fn precise(p: [f64; 4]) -> Easing {
    Easing::css(p[0], p[1], p[2], p[3])
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryEaseLab {
    #[deref]
    view: View,
    #[live(1.2)]
    preview_secs: f64,
    #[rust]
    motion: TweenHost,
    /// The parity tween of the current run (its time is the readout's).
    #[rust]
    run: TweenId,
    #[rust]
    runs: u64,
    /// The duration of the current run.
    #[rust]
    run_secs: f64,
    /// The points the curves, the bars and the readouts show.
    #[rust]
    shown: Option<[f64; 4]>,
    /// The editor's last action and how many the page has seen.
    #[rust]
    last_action: &'static str,
    #[rust]
    actions: u64,
    /// The largest difference and where it is.
    #[rust]
    peak: (f64, f64),
    #[rust]
    bezier: String,
    #[rust]
    text: String,
}

impl StoryEaseLab {
    /// Shows `p`: both curves, the difference, the readout. No replay.
    fn show(&mut self, cx: &mut Cx, p: [f64; 4]) {
        self.shown = Some(p);
        self.view
            .ease_curve(cx, ids!(ease_parity))
            .set_points(cx, p);
        self.view
            .ease_curve(cx, ids!(ease_precise))
            .set_easing(cx, precise(p));

        let (a, b) = (parity(p), precise(p));
        let mut peak = (0.0f64, 0.0f64);
        for i in 0..=SAMPLES {
            let t = i as f64 / SAMPLES as f64;
            let d = (a.map(t) - b.map(t)).abs();
            if d > peak.0 {
                peak = (d, t);
            }
        }
        self.peak = peak;
        {
            let scope = self.view.story_ease_scope(cx, ids!(scope));
            if let Some(mut scope) = scope.borrow_mut() {
                for (i, bar) in scope.frame.bars.iter_mut().enumerate() {
                    let t = (i as f64 + 0.5) / BARS as f64;
                    let d = a.map(t) - b.map(t);
                    *bar = if peak.0 > 0.0 {
                        (d / peak.0) as f32
                    } else {
                        0.0
                    };
                }
            }
            scope.redraw(cx);
        }

        let mut s = std::mem::take(&mut self.text);
        s.clear();
        let _ = write!(
            s,
            "max |parity - precise| {:.5} at t {:.3} over {} samples",
            peak.0,
            peak.1,
            SAMPLES + 1
        );
        self.view.label(cx, ids!(ease_diff)).set_text(cx, &s);
        self.text = s;
        self.write_readout(cx);
    }

    fn write_readout(&mut self, cx: &mut Cx) {
        let Some(p) = self.shown else {
            return;
        };
        let editor = self.view.ease_editor(cx, ids!(editor));
        write_cubic_bezier(&mut self.bezier, p);
        let mut s = std::mem::take(&mut self.text);
        s.clear();
        let _ = write!(
            s,
            "{} | preset {} | {} ({} actions)",
            self.bezier,
            editor.preset().unwrap_or("custom"),
            self.last_action,
            self.actions
        );
        self.view.label(cx, ids!(ease_readout)).set_text(cx, &s);
        self.text = s;
    }

    /// Plays the shown points from 0 to 1 on both tracks.
    fn replay(&mut self, cx: &mut Cx) {
        let Some(p) = self.shown else {
            return;
        };
        let secs = self.preview_secs.max(0.05);
        self.run_secs = secs;
        self.motion.kill_all(cx);
        self.motion.seed(ME, PARITY, 0.0);
        self.motion.seed(ME, PRECISE, 0.0);
        self.run = self.motion.from_to(
            cx,
            ME.into(),
            &[PropTo::from_to(PARITY, 0.0.into(), 1.0.into())],
            TweenOpts::new().duration(secs).ease(parity(p)),
        );
        self.motion.from_to(
            cx,
            ME.into(),
            &[PropTo::from_to(PRECISE, 0.0.into(), 1.0.into())],
            TweenOpts::new().duration(secs).ease(precise(p)),
        );
        self.runs += 1;
        self.write_preview(cx);
    }

    /// The preview line and the scope's dots (per frame while a run plays).
    fn write_preview(&mut self, cx: &mut Cx) {
        let v = [
            self.motion.f64(ME, PARITY, 0.0),
            self.motion.f64(ME, PRECISE, 0.0),
        ];
        let a = self.motion.engine.anim_ref(self.run);
        // A finished tween is released: it ran its whole duration.
        let (time, playing) = if a.is_alive() {
            (a.time(), a.is_active())
        } else {
            (self.run_secs, false)
        };
        let scope = self.view.story_ease_scope(cx, ids!(scope));
        if let Some(mut scope) = scope.borrow_mut() {
            scope.frame.values = v;
        }
        scope.redraw(cx);
        let mut s = std::mem::take(&mut self.text);
        s.clear();
        let _ = write!(
            s,
            "preview run {} {} t {:.3} / {:.2} s | parity {:.4} | precise {:.4} | now {:+.5}",
            self.runs,
            if playing { "playing" } else { "done" },
            time,
            self.run_secs,
            v[0],
            v[1],
            v[0] - v[1]
        );
        self.view.label(cx, ids!(ease_preview)).set_text(cx, &s);
        self.text = s;
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let editor = self.view.ease_editor(cx, ids!(editor));
        // Changed moves the curves with the drag; Committed also replays.
        let changed = editor.changed(actions).is_some();
        let committed = editor.committed(actions).is_some();
        if changed || committed {
            self.actions += changed as u64 + committed as u64;
            self.last_action = if committed { "Committed" } else { "Changed" };
            self.show(cx, editor.points());
        }
        if committed || self.view.button(cx, ids!(ease_replay)).clicked(actions) {
            self.replay(cx);
        }
    }
}

impl Widget for StoryEaseLab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.motion.draw_check(cx.cx.cx);
        let step = self.view.draw_walk(cx, scope, walk);
        // The first draw (the editor exists once the view has been walked)
        // and any value set from outside the editor's own actions.
        let editor = self.view.ease_editor(cx.cx.cx, ids!(editor));
        if editor.borrow().is_some() && self.shown != Some(editor.points()) {
            if self.last_action.is_empty() {
                self.last_action = "none";
            }
            self.show(cx.cx.cx, editor.points());
            self.replay(cx.cx.cx);
        } else if self.shown.is_some() && self.preview_secs.max(0.05) != self.run_secs {
            // A new preview time from the Controls tab (an apply, then a
            // redraw): replay at once, so the knob shows and the readout's
            // "/ N s" says the time in use.
            self.replay(cx.cx.cx);
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
        if self.motion.handle_event(cx, event).changed() {
            self.motion.clear_changes();
            self.write_preview(cx);
        }
    }
}

// ---------------------------------------------------------------------------
// The record
// ---------------------------------------------------------------------------

pub const STORIES: &[Story] = &[Story {
    key: "foundations/motion/ease-editor",
    category: "Foundations",
    component: "Motion",
    also: &["EaseEditor", "EaseCurve"],
    name: "Ease editor",
    dsl: "FoundationsMotionEaseEditor",
    added: "2026-09-25",
    tags: &["new", "ease", "easing", "cubic-bezier", "bezier", "curve", "presets"],
    doc: "# Ease editor\n\n`EaseEditor` edits a cubic-bezier ease: a preset picker over the 24 named CSS curves plus linear, a curve with two draggable control points, four fields (x1, y1, x2, y2) and a `cubic-bezier(..)` readout. Its value is always `Ease::Bezier { cp0: x1, cp1: y1, cp2: x2, cp3: y2 }`, the type the Animator, the theme's motion tokens and the tween engine take. x stays in 0..1, y may reach -1..2 for back and overshoot curves.\n\n- **Drag** a handle; the cursor says Grab over one. A double tap on the curve goes back to the last preset.\n- **Arrow keys** nudge the hovered or last used handle by 0.01, 0.1 with Shift.\n- **Actions**: `changed(&actions)` while a value moves, `committed(&actions)` when a gesture ends (finger up, a field commit, a preset pick, a key nudge).\n\n`EaseCurve` is the canvas on its own: read-only by default, with a preview dot that runs the curve, `editable: true` for the handles.\n\n## This page\n\nEvery commit replays the preview through a `TweenHost`: one dot with the parity solver (`Easing::Bezier`, identical to `Ease::Bezier`), one with the precise CSS solver (`Easing::css`). The two small curves draw the same points with each solver, and the bars under the tracks are their difference scaled to fit. The parity solver stops once x is within t/200, so it can sit a few thousandths off the CSS curve; the third readout line gives the largest difference over 1001 samples.\n\n## The readout\n\n`/snap?q=ease` reads `ease_readout` (the `cubic-bezier(..)` string, the preset, the last action and how many the page has seen), `ease_preview` (the run, its time and both values) and `ease_diff` (the largest difference and where it is).",
    subject: "editor",
    feature: None,
    controls: &[Control {
        label: "Preview time",
        target: "lab",
        kind: ControlKind::Number {
            prop: "preview_secs",
            min: 0.2,
            max: 4.,
            step: 0.1,
            default: 1.2,
        },
    }],
    on_actions: None,
}];
