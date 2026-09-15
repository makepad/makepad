use makepad_score_view::{
    build_drum_score, makepad_widgets::*, BuildOptions, DrumHit, DrumVoice,
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.TextureSiblingBase = #(TextureSibling::register_widget(vm))
    mod.widgets.TextureSibling = set_type_default() do mod.widgets.TextureSiblingBase {
        width: 600
        height: 64
        draw_tex +: {
            tex: texture_2d(float)
            pixel: fn() {
                return self.tex.sample_as_bgra(self.pos)
            }
        }
    }

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(1400, 360)
                body +: {
                    flow: Down
                    spacing: 8
                    padding: 8
                    show_bg: true
                    draw_bg +: { color: #182029 }

                    score_host := View {
                        width: 1318
                        height: 181
                        score := ScoreView {
                            width: Fill
                            height: Fill
                            draw_bg +: { color: #f4f1ea }
                        }
                    }
                    controls := View {
                        width: Fit
                        height: 30
                        flow: Right
                        spacing: 8
                        resize := Button { text: "resize" }
                        status := Label { text: "1318x181" }
                    }
                    texture_host := View {
                        width: 600
                        height: 64
                        texture_sibling := mod.widgets.TextureSibling {
                            width: Fill
                            height: Fill
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TextureSibling {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_tex: DrawQuad,
    #[rust]
    area: Area,
    #[rust]
    texture: Option<Texture>,
}

impl Widget for TextureSibling {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        let texture = self
            .texture
            .get_or_insert_with(|| {
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: 4,
                        height: 4,
                        data: Some(vec![0xff00_ff00; 16]),
                        updated: TextureUpdated::Full,
                    },
                )
            })
            .clone();
        self.draw_tex.draw_vars.set_texture(0, &texture);
        self.draw_tex.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let voices = [
            DrumVoice::Kick,
            DrumVoice::HiHatClosed,
            DrumVoice::Snare,
            DrumVoice::HiHatOpen,
            DrumVoice::TomHigh,
            DrumVoice::Ride,
            DrumVoice::Crash,
        ];
        let hits: Vec<_> = (0..64)
            .map(|step| DrumHit {
                time_beats: step as f64 * 0.25,
                voice: voices[step % voices.len()],
                velocity: 0.8,
            })
            .collect();
        let score = build_drum_score(
            &hits,
            &BuildOptions {
                bars: 4,
                bpm: Some(124.0),
                title: Some("Embedded score".to_string()),
                ..BuildOptions::default()
            },
        );
        self.ui
            .widget(cx, ids!(score))
            .borrow_mut::<makepad_score_view::ScoreView>()
            .expect("score fixture lost its ScoreView")
            .set_score(cx, score);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(resize)).clicked(actions) {
            let mut host = self.ui.widget(cx, ids!(score_host));
            script_apply_eval!(cx, host, {
                width: 600
                height: 160
            });
            self.ui
                .label(cx, ids!(status))
                .set_text(cx, "600x160");
            self.ui.redraw(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_score_view::makepad_widgets::script_mod(vm);
        makepad_score_view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
