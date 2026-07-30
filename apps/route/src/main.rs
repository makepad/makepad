//! AI route/trip planner — voice-driven navigation copilot.
//!
//! This is the M0 skeleton: full-screen MapView plus an assistant panel with
//! a typed prompt as the stand-in for the voice loop. The full scope —
//! tool broker, local/cloud LLM orchestration, weather-aware charge
//! planning, image search — is laid out in route.md at the repo root.
//!
//! Map data: shares the archives built for examples/map:
//!   local/maps/europe-shortbread.mbtiles (+ detail, bridge-dz overlays)

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let PanelText = Label{
        width: Fill
        draw_text +: {
            color: #x22303c
            text_style: theme.font_regular{font_size: 9}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 840)
                pass.clear_color: vec4(0.08, 0.10, 0.12, 1.0)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        map := MapView{
                            width: Fill
                            height: Fill
                            center_lon: 4.8952
                            center_lat: 52.3702
                            zoom: 13.0
                            min_zoom: 3.0
                            mbtiles_path: "local/maps/europe-shortbread.mbtiles"
                            detail_mbtiles_path: "local/maps/europe-osm-detail.mbtiles"
                            bridge_dz_mbtiles_path: "local/maps/ams-bridge-dz.mbtiles"
                            buildings_3d: true
                        }

                        // --- Assistant panel (right) ---
                        View{
                            width: Fill
                            height: Fill
                            align: Align{x: 1.0 y: 0.0}
                            assistant_panel := RoundedView{
                                flow: Down
                                width: 380
                                height: Fill
                                margin: 12
                                padding: 10
                                draw_bg +: {
                                    color: #xfffffff2
                                    border_radius: 9.0
                                    border_size: 1.0
                                    border_color: #x00000022
                                }
                                Label{
                                    draw_text +: {
                                        color: #x223038
                                        text_style: theme.font_bold{font_size: 11}
                                    }
                                    text: "Route Assistant"
                                }
                                transcript_scroll := ScrollYView{
                                    flow: Down
                                    width: Fill
                                    height: Fill
                                    margin: Inset{top: 8, bottom: 8}
                                    transcript_label := PanelText{
                                        height: Fit
                                        text: "Ask about a trip: destinations, charge stops, weather, sights.\n(Voice loop and tools land per route.md — typed prompt only for now.)"
                                    }
                                }
                                prompt_input := TextInput{
                                    width: Fill
                                    empty_text: "Plan a trip…"
                                }
                                status_label := PanelText{
                                    margin: Inset{top: 6, left: 2}
                                    text: "M0 skeleton — agent loop not wired"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    transcript: String,
}

impl App {
    fn push_line(&mut self, cx: &mut Cx, line: &str) {
        if !self.transcript.is_empty() {
            self.transcript.push('\n');
        }
        self.transcript.push_str(line);
        self.ui
            .label(cx, ids!(transcript_label))
            .set_text(cx, &self.transcript);
        self.ui.view(cx, ids!(transcript_scroll)).redraw(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some((text, _)) = self.ui.text_input(cx, ids!(prompt_input)).returned(actions) {
            let text = text.trim().to_string();
            if !text.is_empty() {
                self.push_line(cx, &format!("you: {text}"));
                self.push_line(cx, "route: agent loop not wired yet — see route.md M1");
                self.ui.text_input(cx, ids!(prompt_input)).set_text(cx, "");
            }
        }
        if let Some((lon, lat)) = self.ui.map_view(cx, ids!(map)).long_pressed(actions) {
            self.push_line(cx, &format!("map: long-press at {lon:.5}, {lat:.5}"));
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
