use makepad_widgets::makepad_draw::svg;
use makepad_widgets::makepad_draw::DrawVector;
use makepad_widgets::*;

app_main!(App);

const SVG_SOURCE: &str = include_str!("../resources/tiger.svg");

script_mod! {
    use mod.prelude.widgets.*

    let SvgDemoBase = #(SvgDemo::register_widget(vm))
    let SvgDemo = set_type_default() do SvgDemoBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 800)
                pass.clear_color: vec4(0.12, 0.12, 0.15, 1.0)
                body +: {
                    jellyfish := Svg{
                        width: Fill
                        height: Fill
                        animating: true
                        draw_svg +: { svg: crate_resource("self://resources/jellyfish_instances.svg") }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
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
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        cx.with_widget_tree(|cx| {
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
        });
    }
}

// ---- Old inline SVG Demo Widget (known working) ----

#[derive(Script, ScriptHook, Widget)]
pub struct SvgDemo {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_vector: DrawVector,
    #[rust]
    area: Area,
    #[rust]
    svg_doc: Option<svg::SvgDocument>,
    #[rust]
    time: f64,
}

impl Widget for SvgDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.svg_doc.is_none() {
            self.svg_doc = Some(svg::parse_svg(SVG_SOURCE));
        }

        let rect = cx.walk_turtle(walk);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;
        let w = rect.size.x as f32;
        let h = rect.size.y as f32;

        if let Some(ref doc) = self.svg_doc {
            self.draw_vector.begin();
            svg::render_svg(&mut self.draw_vector, doc, ox, oy, w, h, self.time as f32);
            self.draw_vector.end(cx);
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            self.time = ne.time;
            self.area.redraw(cx);
        }
        if let Event::Startup = event {
            cx.new_next_frame();
        }
    }
}
