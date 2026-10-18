//! Procedural skyline strip generator: a tiling, alpha-channel silhouette band
//! (mountains, cities, treelines) for compositing in front of a sky in an HDRI.
pub use makepad_widgets;

use makepad_widgets::*;

mod skyline;
use skyline::SkylineViewWidgetRefExt;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // A labelled slider bound to a shader parameter by its widget name.
    let Param = Slider{
        width: Fill
        step: 0.01
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1500, 640)
                pass.clear_color: #x10141c
                body +: {
                    flow: Right
                    panel := ScrollYView{
                        width: 300 height: Fill
                        flow: Down spacing: 6
                        padding: Inset{left: 12, right: 12, top: 12, bottom: 12}
                        H4{text: "Skyline strip"}
                        reroll := Button{text: "New seed"}
                        export := Button{text: "Export PNG + EXR"}
                        status := Label{text: ""}
                        Hr{}
                        horizon := Param{text: "Horizon" min: 0.2 max: 0.95 default: 0.72}
                        Hr{}
                        mountain_amt := Param{text: "Mountains" min: 0.0 max: 1.0 default: 1.0}
                        mountain_height := Param{text: "Mountain height" min: 0.02 max: 0.3 default: 0.10}
                        Hr{}
                        city_amt := Param{text: "City" min: 0.0 max: 1.0 default: 1.0}
                        city_height := Param{text: "Skyscraper height" min: 0.2 max: 2.5 default: 1.0}
                        city_density := Param{text: "City density" min: 0.0 max: 1.0 default: 0.7}
                        Hr{}
                        tree_amt := Param{text: "Trees" min: 0.0 max: 1.0 default: 1.0}
                        tree_height := Param{text: "Tree height" min: 0.002 max: 0.04 default: 0.012}
                        tree_conifer := Param{text: "Conifer share" min: 0.0 max: 1.0 default: 0.7}
                        Hr{}
                        lights := Param{text: "Light intensity" min: 0.0 max: 10.0 default: 3.0}
                        window_density := Param{text: "Lit windows" min: 0.0 max: 1.0 default: 0.45}
                        glow := Param{text: "City glow" min: 0.0 max: 2.0 default: 0.5}
                        haze := Param{text: "Haze" min: 0.0 max: 1.0 default: 0.7}
                        Hr{}
                        H4{text: "Where (360 degrees)"}
                        land_cover := Param{text: "Land covers" min: 0.0 max: 1.0 default: 0.8}
                        land_center := Param{text: "Land at" min: 0.0 max: 1.0 default: 0.4}
                        mountain_cover := Param{text: "Mountains cover" min: 0.0 max: 1.0 default: 0.45}
                        mountain_center := Param{text: "Mountains at" min: 0.0 max: 1.0 default: 0.15}
                        city_cover := Param{text: "City covers" min: 0.0 max: 1.0 default: 0.3}
                        city_center := Param{text: "City at" min: 0.0 max: 1.0 default: 0.55}
                        tree_cover := Param{text: "Trees cover" min: 0.0 max: 1.0 default: 0.55}
                        tree_center := Param{text: "Trees at" min: 0.0 max: 1.0 default: 0.3}
                        treeline := Param{text: "Treeline height" min: 0.0 max: 0.3 default: 0.05}
                    }
                    // Checkerboard-free dark sky stand-in: the strip is transparent.
                    stage := SolidView{
                        width: Fill height: Fill
                        draw_bg +: {color: #x1a2644}
                        strip := SkylineView{width: Fill height: Fill}
                    }
                }
            }
        }
    }
}

const PARAMS: &[&str] = &[
    "horizon", "mountain_amt", "mountain_height", "city_amt", "city_height", "city_density",
    "tree_amt", "tree_height", "tree_conifer", "lights", "window_density", "glow", "haze",
    "mountain_cover", "mountain_center", "city_cover", "city_center", "tree_cover", "tree_center",
    "treeline", "land_cover", "land_center",
];

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(1.0)]
    seed: f32,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let strip = self.ui.widget(cx, ids!(strip));
        let strip = strip.as_skyline_view();
        for name in PARAMS {
            let id = LiveId::from_str(name);
            if let Some(v) = self.ui.slider(cx, &[id]).slided(actions) {
                strip.set_param(cx, name, v as f32);
            }
        }
        if self.ui.button(cx, ids!(reroll)).clicked(actions) {
            self.seed += 1.0;
            strip.set_param(cx, "seed", self.seed);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        self::skyline::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
