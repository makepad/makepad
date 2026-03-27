pub use makepad_browser_scene;
pub use makepad_widgets;

use makepad_browser_scene::{
    MpBlendMode, MpBrowserRenderer, MpEffectNode, MpIsolation, MpPrimitive, MpScene, MpSceneId,
};
use makepad_widgets::*;

app_main!(App);

const COLOR_BG: Vec4f = Vec4f {
    x: 0.08,
    y: 0.09,
    z: 0.11,
    w: 1.0,
};

script_mod! {
    use mod.prelude.widgets.*

    let BrowserSceneDemoBase = #(BrowserSceneDemo::register_widget(vm))
    let BrowserSceneDemo = set_type_default() do BrowserSceneDemoBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(COLOR_BG)}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                pass.clear_color: #(COLOR_BG)
                body +: {
                    demo := BrowserSceneDemo{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_browser_scene::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct BrowserSceneDemo {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[rust]
    renderer: Option<MpBrowserRenderer>,
    #[rust]
    scene: Option<MpScene>,
    #[rust]
    logged: bool,
}

impl Widget for BrowserSceneDemo {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.renderer.is_none() {
            self.renderer = Some(MpBrowserRenderer::new(cx.cx));
        }

        cx.begin_turtle(walk, self.layout);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        };
        self.draw_bg.draw_abs(cx, rect);

        if self.scene.is_none() {
            self.scene = Some(build_demo_scene(rect));
        }

        let stats = self
            .renderer
            .as_mut()
            .unwrap()
            .draw_scene(cx, self.scene.as_ref().unwrap(), rect)
            .expect("browser scene render");

        if !self.logged {
            self.logged = true;
            log!(
                "[makepad-browser-scene] direct={} isolated_boundaries={} isolated_primitives={} compositor_surfaces={}",
                stats.direct_primitive_count,
                stats.isolated_boundary_count,
                stats.isolated_primitive_count,
                stats.compositor_surface_count
            );
        }

        cx.end_turtle();
        DrawStep::done()
    }
}

fn build_demo_scene(viewport: Rect) -> MpScene {
    let mut scene = MpScene::new(MpSceneId(1), viewport);
    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(0),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(40.0, 40.0),
            size: dvec2(220.0, 120.0),
        },
        vec4(0.92, 0.26, 0.30, 1.0),
    ));
    scene.push_primitive(MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(1),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(300.0, 70.0),
            size: dvec2(260.0, 90.0),
        },
        vec4(0.18, 0.72, 0.45, 1.0),
    ));

    let effect_id = scene.push_effect(MpEffectNode {
        spatial_id: scene.root_spatial_id,
        clip_chain_id: scene.root_clip_chain_id,
        opacity: 0.6,
        filters: Vec::new(),
        blend_mode: MpBlendMode::Normal,
        isolation: MpIsolation::Isolate,
        mask: None,
    });

    let mut isolated_a = MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(2),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(160.0, 210.0),
            size: dvec2(220.0, 150.0),
        },
        vec4(0.24, 0.52, 0.96, 1.0),
    );
    isolated_a.effect_id = Some(effect_id);
    scene.push_primitive(isolated_a);

    let mut isolated_b = MpPrimitive::solid_rect(
        makepad_browser_scene::MpPrimitiveId(3),
        scene.root_spatial_id,
        scene.root_clip_chain_id,
        Rect {
            pos: dvec2(260.0, 250.0),
            size: dvec2(220.0, 150.0),
        },
        vec4(0.98, 0.80, 0.22, 1.0),
    );
    isolated_b.effect_id = Some(effect_id);
    scene.push_primitive(isolated_b);

    scene
}
