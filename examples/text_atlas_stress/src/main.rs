//! The stage for the glyph-atlas regression test (`tests/ui.rs`).
//!
//! Phase 0 draws four rows of text at four sizes. Clicking `stress` opens
//! phase 1: four MORE rows in a second font (a fresh set of glyph outlines,
//! so the append-only slug atlas grows past its first rows) while `burst`
//! creates and binds K brand-new Vec textures inside the same draw, every
//! frame — the sandbox's Doom load in miniature (2026-08-24: on Metal the
//! atlas texture reallocated with only the appended rows dirty and every
//! glyph cached before the growth vanished for the rest of the process).
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
                return self.tex.sample_as_bgra(self.pos)
            }
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
                window.inner_size: vec2(900, 720)
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
                    burst := mod.widgets.TexBurst {}
                    stress := Button { text: "stress" }
                    status := Label { text: "phase 0 frame 0" draw_text +: { color: #fff } }
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
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
