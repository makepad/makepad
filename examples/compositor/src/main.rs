use makepad_compositor::{MpCompositedQuad, MpCompositor, MpSurface, MpSurfaceColorFormat};
use makepad_widgets::*;

app_main!(App);

const SURFACE_WIDTH: f64 = 256.0;
const SURFACE_HEIGHT: f64 = 128.0;
const COLOR_BG: Vec4f = Vec4f {
    x: 0.08,
    y: 0.20,
    z: 0.10,
    w: 1.0,
};
const COLOR_LEFT: Vec4f = Vec4f {
    x: 1.0,
    y: 0.0,
    z: 0.0,
    w: 1.0,
};
const COLOR_RIGHT: Vec4f = Vec4f {
    x: 0.0,
    y: 0.0,
    z: 1.0,
    w: 1.0,
};

script_mod! {
    use mod.prelude.widgets.*

    let CompositorDemoBase = #(CompositorDemo::register_widget(vm))
    let CompositorDemo = set_type_default() do CompositorDemoBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #(COLOR_BG)}
        draw_left: mod.draw.DrawColor{color: #(COLOR_LEFT)}
        draw_right: mod.draw.DrawColor{color: #(COLOR_RIGHT)}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(960, 540)
                pass.clear_color: #(COLOR_BG)
                body +: {
                    demo := CompositorDemo{}
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
        makepad_compositor::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DemoMode {
    #[default]
    Normal,
    ClipLeftHalf,
}

impl DemoMode {
    fn from_env() -> Self {
        match std::env::var("MAKEPAD_COMPOSITOR_TEST_MODE") {
            Ok(value) if value.eq_ignore_ascii_case("clip") => Self::ClipLeftHalf,
            _ => Self::Normal,
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct CompositorDemo {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_left: DrawColor,
    #[live]
    draw_right: DrawColor,
    #[rust]
    area: Area,
    #[rust]
    mode: DemoMode,
    #[rust]
    surface: Option<MpSurface>,
    #[rust]
    surface_draw_list: Option<DrawList2d>,
    #[rust]
    compositor: Option<MpCompositor>,
}

impl Widget for CompositorDemo {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.surface.is_none() {
            self.surface = Some(MpSurface::new(
                cx.cx.cx,
                dvec2(SURFACE_WIDTH, SURFACE_HEIGHT),
                MpSurfaceColorFormat::BgraU8,
                false,
            ));
        }
        if self.surface_draw_list.is_none() {
            self.surface_draw_list = Some(DrawList2d::new(cx.cx.cx));
        }
        if self.compositor.is_none() {
            self.compositor = Some(MpCompositor::new(cx.cx.cx));
        }
        self.mode = DemoMode::from_env();

        cx.begin_turtle(walk, self.layout);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size: cx.current_pass_size(),
        };

        self.draw_bg.draw_abs(cx, rect);

        let surface = self.surface.as_mut().unwrap();
        surface.resize(cx.cx.cx, dvec2(SURFACE_WIDTH, SURFACE_HEIGHT));
        surface.begin(cx, None);
        self.surface_draw_list.as_mut().unwrap().begin_always(cx);
        self.draw_left.draw_abs(
            cx,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(SURFACE_WIDTH * 0.5, SURFACE_HEIGHT),
            },
        );
        self.draw_right.draw_abs(
            cx,
            Rect {
                pos: dvec2(SURFACE_WIDTH * 0.5, 0.0),
                size: dvec2(SURFACE_WIDTH * 0.5, SURFACE_HEIGHT),
            },
        );
        self.surface_draw_list.as_mut().unwrap().end(cx);
        surface.end(cx);

        let mut quad = MpCompositedQuad::new(surface.color_texture().clone(), rect);
        quad.premultiplied = true;
        quad.depth_write = false;
        if self.mode == DemoMode::ClipLeftHalf {
            // The current projected quad occupies the left 256 px of a 960 px window.
            // Clip-space midpoint for that span is approximately -0.7333.
            quad.clip_planes.push(vec4(-1.0, 0.0, 0.0, -0.7333));
        }
        self.compositor.as_mut().unwrap().draw_quad(cx, &quad);

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
