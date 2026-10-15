//! The stage for the glyph-atlas regression test (`tests/ui.rs`).
//!
//! Phase 0 draws four rows of text at four sizes. Clicking `stress` opens
//! phase 1: four MORE rows in a second font (a fresh set of glyph outlines,
//! so the append-only slug atlas grows past its first rows) while `burst`
//! creates and binds K brand-new Vec textures inside the same draw, every
//! frame — the sandbox's Doom load in miniature (2026-08-24: on Metal the
//! atlas texture reallocated with only the appended rows dirty and every
//! glyph cached before the growth vanished for the rest of the process).
//!
//! `kept` shapes a few HUD-like words ONCE, on its first draw, keeps the
//! prepared runs and draws them every frame through
//! `draw_rasterized_glyphs_abs`; under them the same words are laid out
//! afresh each draw. Both lines must carry the same ink (2026-10-13: under the
//! HAL font service a kept run kept the placeholders of glyphs that were still
//! on their way when it was shaped, so Sandbox's HUD drew 'STRIKE' as 'STR  E').
//!
//! `churn` (button) animates for `CHURN_FRAMES` frames in its own draw list
//! while the `still` row (the code face, which nothing else on the page uses)
//! sits in another list that is never redrawn: first
//! `QUIET_FRAMES` frames of glyphs it already has (no glyph requests, so nothing
//! redraws the still row: past the glyph client's 600-frame idle sweep), then
//! fresh glyph/size pairs every frame (with a small `MAKEPAD_TEXT_ATLAS_SIZE`
//! the host evicts and reuses slots). No frame may show a churn cell blank,
//! and the still row must not change (2026-10-13, Stage's code pane beside an
//! animated slide: the pane's glyphs were released as idle, their slots given
//! to new glyphs, and its words went blank for a frame or more).
//!
//! `kept_list` records its words into its own draw list ONCE and never again,
//! not even on a full redraw (as a render target painted again from its kept
//! instances does). Its glyphs must survive the churn's fresh glyphs in four
//! faces filling a small curve store (2026-10-13, 149: the Relax deck title
//! read "Re ax" after a few minutes, its "l" released as unused and its curves
//! given to another glyph).
use makepad_widgets::*;

app_main!(App);

const BURST_TEXTURES: usize = 48;
const BURST_FRAMES: u64 = 8;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.TexBurstBase = #(TexBurst::register_widget(vm))
    mod.widgets.TexBurst = set_type_default() do mod.widgets.TexBurstBase {
        width: Fill
        height: 40
        draw_tex +: {
            tex: texture_2d(float)
            pixel: fn() {
                return self.tex.sample(self.pos)
            }
        }
    }

    mod.widgets.KeptRunBase = #(KeptRun::register_widget(vm))
    mod.widgets.KeptRun = set_type_default() do mod.widgets.KeptRunBase {
        width: Fill
        height: 120
        draw_text +: {
            color: #fff
            text_style: theme.font_bold{ font_size: 16 }
        }
    }

    mod.widgets.ChurnBase = #(Churn::register_widget(vm))
    mod.widgets.Churn = set_type_default() do mod.widgets.ChurnBase {
        width: Fill
        height: 70
        draw_text +: {
            color: #fff
            text_style: theme.font_regular{ font_size: 12 }
        }
        draw_text_b +: {
            color: #fff
            text_style: theme.font_bold{ font_size: 12 }
        }
        draw_text_c +: {
            color: #fff
            text_style: theme.font_code{ font_size: 12 }
        }
        draw_text_d +: {
            color: #fff
            text_style: theme.font_bold_italic{ font_size: 12 }
        }
    }

    mod.widgets.KeptListBase = #(KeptList::register_widget(vm))
    mod.widgets.KeptList = set_type_default() do mod.widgets.KeptListBase {
        width: Fill
        height: 60
        draw_text +: {
            color: #fff
            text_style: theme.font_italic{ font_size: 20 }
        }
    }

    let Row = View {
        width: Fill
        height: 60
        clip_y: true
        padding: Inset{left: 8, top: 4}
    }

    let Text = Label {
        width: Fill
        height: Fit
        margin: 0
        text: "AOER YUDISPWNG Bmfjkqxz 0123456789 ÀÉÎÕÜ ßçñ !?&%"
        draw_text +: {
            color: #fff
        }
    }

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(900, 1200)
                body +: {
                    flow: Down
                    padding: 0
                    spacing: 0
                    show_bg: true
                    draw_bg +: { color: #000 }
                    Row { row_0 := Text { draw_text +: { text_style: theme.font_regular{ font_size: 12 } } } }
                    Row { row_1 := Text { draw_text +: { text_style: theme.font_regular{ font_size: 16 } } } }
                    Row { row_2 := Text { draw_text +: { text_style: theme.font_regular{ font_size: 24 } } } }
                    Row { row_3 := Text { draw_text +: { text_style: theme.font_regular{ font_size: 36 } } } }
                    bold_rows := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        visible: false
                        Row { row_b0 := Text { draw_text +: { text_style: theme.font_bold{ font_size: 12 } } } }
                        Row { row_b1 := Text { draw_text +: { text_style: theme.font_bold{ font_size: 16 } } } }
                        Row { row_b2 := Text { draw_text +: { text_style: theme.font_bold{ font_size: 24 } } } }
                        Row { row_b3 := Text { draw_text +: { text_style: theme.font_bold{ font_size: 36 } } } }
                    }
                    kept := mod.widgets.KeptRun {}
                    View {
                        width: Fill
                        height: 60
                        new_batch: true
                        show_bg: true
                        draw_bg +: { color: #000 }
                        still := Text { draw_text +: { text_style: theme.font_code{ font_size: 20 } } text: "STILL 0123456789 abcdefghijklm" }
                    }
                    View {
                        width: Fill
                        height: 70
                        new_batch: true
                        churn_row := mod.widgets.Churn {}
                    }
                    kept_list := mod.widgets.KeptList {}
                    burst := mod.widgets.TexBurst {}
                    stress := Button { text: "stress" }
                    status := Label { text: "phase 0 frame 0" draw_text +: { color: #fff } }
                    churn := Button { text: "churn" }
                    churn_status := Label { text: "churn idle" draw_text +: { color: #fff } }
                }
            }
        }
    }
}

/// One line per draw of `TexBurst`, mirrored into the `status` label so the
/// test can wait on widget text.
#[derive(Clone, Debug, Default)]
pub enum BurstAction {
    Frame { phase: u32, frame: u64 },
    #[default]
    None,
}

/// K fresh 8x8 Vec textures created and bound inside one draw, `BURST_FRAMES`
/// frames in a row once `active`.
#[derive(Script, ScriptHook, Widget)]
pub struct TexBurst {
    #[source] source: ScriptObjectRef,
    #[uid] widget_uid: WidgetUid,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_tex: DrawQuad,
    #[rust] area: Area,
    #[rust] active: bool,
    #[rust] frame: u64,
    #[rust] next_frame: NextFrame,
    #[rust] textures: Vec<Texture>,
}

impl TexBurst {
    fn start(&mut self, cx: &mut Cx) {
        self.active = true;
        self.frame = 0;
        self.area.redraw(cx);
    }
}

impl Widget for TexBurst {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let uid = self.widget_uid();
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        if self.active {
            // New textures every frame: the previous frame's are dropped here,
            // exactly like a level load cutting sprite sheets during a draw.
            self.textures.clear();
            for k in 0..BURST_TEXTURES {
                let shade = 0xff00_0000 | ((k as u32 * 5) << 16) | (0x80 << 8) | (255 - k as u32 * 5);
                let texture = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: 8,
                        height: 8,
                        data: Some(vec![shade; 64]),
                        updated: TextureUpdated::Full,
                    },
                );
                self.draw_tex.draw_vars.set_texture(0, &texture);
                let x = rect.pos.x + 8.0 + k as f64 * 18.0;
                self.draw_tex.draw_abs(cx, Rect { pos: dvec2(x, rect.pos.y + 8.0), size: dvec2(16.0, 16.0) });
                self.textures.push(texture);
            }
            self.frame += 1;
            log!("atlas-stress: phase 1 frame {}", self.frame);
            cx.widget_action(uid, BurstAction::Frame { phase: 1, frame: self.frame });
            if self.frame < BURST_FRAMES {
                self.next_frame = cx.new_next_frame();
            }
        } else {
            self.frame += 1;
            log!("atlas-stress: phase 0 frame {}", self.frame);
            cx.widget_action(uid, BurstAction::Frame { phase: 0, frame: self.frame });
            if self.frame < 3 {
                self.next_frame = cx.new_next_frame();
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
    }
}

/// The words a game HUD shows, at HUD sizes (font size multipliers).
const KEPT_WORDS: [(&str, f32); 3] = [("STRIKE", 1.0), ("POSITION", 1.5), ("MATCH 1ST", 0.75)];

/// Words shaped once and drawn from the kept runs every frame (top half),
/// and the same words laid out afresh every draw (bottom half).
#[derive(Script, ScriptHook, Widget)]
pub struct KeptRun {
    #[source] source: ScriptObjectRef,
    #[uid] widget_uid: WidgetUid,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_text: DrawText,
    #[rust] area: Area,
    #[rust] runs: Vec<PreparedTextRun<makepad_widgets::makepad_draw::text::rasterizer::RasterizedGlyph>>,
    #[rust] cells: Vec<(makepad_widgets::makepad_draw::text::geom::Point<f32>, f32, makepad_widgets::makepad_draw::text::rasterizer::RasterizedGlyph)>,
}

impl Widget for KeptRun {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        use makepad_widgets::makepad_draw::text::geom::Point;
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        let (x0, y0) = (rect.pos.x as f32 + 8.0, rect.pos.y as f32 + 4.0);
        let base_scale = self.draw_text.font_scale;
        if self.runs.is_empty() {
            for (word, scale) in KEPT_WORDS {
                self.draw_text.font_scale = scale;
                if let Some(run) = self.draw_text.prepare_single_line_raster_run(cx, word) {
                    self.runs.push(run);
                }
            }
            self.draw_text.font_scale = base_scale;
        }
        let mut x = x0;
        for run in &self.runs {
            self.cells.clear();
            let base = y0 + run.ascender_in_lpxs.abs();
            for g in &run.glyphs {
                self.cells.push((Point::new(x + g.pen_x_in_lpxs + g.offset_x_in_lpxs, base), g.font_size_in_lpxs, g.glyph));
            }
            // the cells carry their scale in their sizes
            self.draw_text.font_scale = 1.0;
            let color = self.draw_text.color;
            self.draw_text.draw_rasterized_glyphs_abs(cx, &self.cells, color);
            x += run.width_in_lpxs + 24.0;
        }
        let mut x = x0;
        for (word, scale) in KEPT_WORDS {
            self.draw_text.font_scale = scale;
            let width = self.draw_text.prepare_single_line_run(cx, word).map(|r| r.width_in_lpxs).unwrap_or(0.0);
            self.draw_text.draw_abs(cx, dvec2(x as f64, y0 as f64 + 60.0), word);
            x += width + 24.0;
        }
        self.draw_text.font_scale = base_scale;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// Frames the churn runs: past the glyph client's 600-frame idle sweep.
const CHURN_FRAMES: u64 = 1000;
/// Churn frames that draw only glyphs already held (no requests).
const QUIET_FRAMES: u64 = 760;
/// Cells per churn frame, `CHURN_CELL` points apart.
pub const CHURN_CELLS: usize = 8;
pub const CHURN_CELL: f64 = 64.0;

/// The characters the churn cycles through (all with ink).
fn churn_char(i: usize) -> char {
    const ASCII: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let latin: Vec<char> = ASCII.chars()
        .chain((0xC0u32..=0x17F).filter(|c| *c != 0xD7 && *c != 0xF7).filter_map(char::from_u32))
        .collect();
    latin[i % latin.len()]
}

#[derive(Clone, Debug, Default)]
pub enum ChurnAction {
    Frame(u64),
    #[default]
    None,
}

/// Fresh glyph/size pairs every frame: cell k of churn frame f draws one
/// character at a size that moves on as the characters wrap around.
#[derive(Script, ScriptHook, Widget)]
pub struct Churn {
    #[source] source: ScriptObjectRef,
    #[uid] widget_uid: WidgetUid,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_text: DrawText,
    #[live] draw_text_b: DrawText,
    #[live] draw_text_c: DrawText,
    #[live] draw_text_d: DrawText,
    #[rust] area: Area,
    #[rust] frame: u64,
    #[rust] active: bool,
    #[rust] next_frame: NextFrame,
}

impl Widget for Churn {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        // after the churn its last cells stay on screen (the test grabs until it reads "done")
        if self.active || self.frame > 0 {
            if self.active {
                self.frame += 1;
            }
            let base_size = self.draw_text.text_style.font_size;
            let mut buf = [0u8; 4];
            for k in 0..CHURN_CELLS {
                let i = if self.frame <= QUIET_FRAMES {
                    (self.frame % 2) as usize * CHURN_CELLS + k
                } else {
                    (self.frame - QUIET_FRAMES) as usize * CHURN_CELLS + k
                };
                let wraps = if self.frame <= QUIET_FRAMES { 0 } else { 1 + i / 230 };
                // after the quiet frames the faces take turns: three times the glyphs
                let draw = match if self.frame <= QUIET_FRAMES { 0 } else { (i / 230) % 4 } {
                    0 => &mut self.draw_text,
                    1 => &mut self.draw_text_b,
                    2 => &mut self.draw_text_c,
                    _ => &mut self.draw_text_d,
                };
                draw.text_style.font_size = 12.0 + (wraps % 24) as f32 * 1.5;
                let text = churn_char(i).encode_utf8(&mut buf);
                draw.draw_abs(cx, dvec2(rect.pos.x + 8.0 + k as f64 * CHURN_CELL, rect.pos.y + 4.0), text);
            }
            self.draw_text.text_style.font_size = base_size;
            self.draw_text_b.text_style.font_size = base_size;
            self.draw_text_c.text_style.font_size = base_size;
            self.draw_text_d.text_style.font_size = base_size;
            if self.active && self.frame == QUIET_FRAMES {
                // everything draws again (a window resize, a theme change): the still row's
                // glyphs, unused for QUIET_FRAMES, must still be there
                cx.redraw_all();
            }
            if self.active {
                cx.widget_action(self.widget_uid(), ChurnAction::Frame(self.frame));
                if self.frame < CHURN_FRAMES {
                    self.next_frame = cx.new_next_frame();
                } else {
                    self.active = false;
                }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.area.redraw(cx);
        }
    }
}

/// Words recorded into their own draw list once and never again, even on a full redraw: the
/// list is painted from its kept instances (as a render target's passes are).
#[derive(Script, ScriptHook, Widget)]
pub struct KeptList {
    #[source] source: ScriptObjectRef,
    #[uid] widget_uid: WidgetUid,
    #[walk] walk: Walk,
    #[redraw] #[live] draw_text: DrawText,
    #[rust] area: Area,
    #[rust] list: Option<DrawList2d>,
    #[rust] recorded: bool,
}

impl Widget for KeptList {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let list = self.list.get_or_insert_with(|| DrawList2d::new(cx));
        if list.begin_maybe(cx, !self.recorded).is_redrawing() {
            self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + 8.0), "KEPT LIST 0123456789 abcdefghijklmnop");
            list.end(cx);
            // kept once complete (as Motion keeps a frame once nothing it shows is pending)
            self.recorded = !cx.pending_async_loads().iter().any(|(kind, n)| *kind == "glyph" && *n > 0);
            if !self.recorded {
                cx.redraw_all();
            }
        }
        self.area = Area::Empty;
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let BurstAction::Frame { phase, frame } = action.as_widget_action().cast::<BurstAction>() {
                self.ui
                    .label(cx, ids!(status))
                    .set_text(cx, &format!("phase {phase} frame {frame}"));
            }
        }
        for action in actions {
            if let ChurnAction::Frame(frame) = action.as_widget_action().cast::<ChurnAction>() {
                // the status changes only at the switch to fresh glyphs and at the end: a
                // label change redraws the window's main list, the still row must stay idle
                let text = if frame >= CHURN_FRAMES {
                    format!("churn done blank_frames={}", cx.blank_glyph_frames())
                } else if frame == QUIET_FRAMES + 1 {
                    "churn fresh".to_string()
                } else {
                    continue;
                };
                self.ui.label(cx, ids!(churn_status)).set_text(cx, &text);
            }
        }
        if self.ui.button(cx, ids!(churn)).clicked(actions) {
            if let Some(mut churn) = self.ui.widget(cx, ids!(churn_row)).borrow_mut::<Churn>() {
                churn.active = true;
                churn.frame = 0;
                churn.area.redraw(cx);
            }
        }
        if self.ui.button(cx, ids!(stress)).clicked(actions) {
            self.ui.view(cx, ids!(bold_rows)).set_visible(cx, true);
            if let Some(mut burst) = self.ui.widget(cx, ids!(burst)).borrow_mut::<TexBurst>() {
                burst.start(cx);
            }
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
