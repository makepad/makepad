use makepad_draw2::svg;
use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let VectorDemoBase = #(VectorDemo::register_widget(vm))
    let VectorDemo = set_type_default() do VectorDemoBase{
        width: Fill
        height: Fill
        draw_vector +: {
            // use shape_id to selectively apply dash pattern
            get_stroke_mask: fn() {
                // shape_id 4.0 = bezier curve: dotted (8 on, 8 off)
                if abs(self.v_shape_id - 4.0) < 0.5 {
                    return self.dash(8.0, 8.0)
                }
                // shape_id 6.0 = ellipse: dashed (16 on, 8 off)
                if abs(self.v_shape_id - 6.0) < 0.5 {
                    return self.dash(16.0, 8.0)
                }
                // shape_id 8.0 = nested rects: short dash
                if abs(self.v_shape_id - 8.0) < 0.5 {
                    return self.dash(4.0, 4.0)
                }
                return 1.0
            }
        }
    }

    let SvgDemoBase = #(SvgDemo::register_widget(vm))
    let SvgDemo = set_type_default() do SvgDemoBase{
        width: Fill
        height: Fill
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 600)
                pass.clear_color: vec4(0.15, 0.15, 0.18, 1.0)
                body +: {
                    flow: Right
                    vector_demo := VectorDemo{ width: Fill, height: Fill }
                    svg_demo := SvgDemo{ width: Fill, height: Fill }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
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
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VectorDemo {
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_vector: DrawVector,
    #[rust]
    area: Area,
}

impl Widget for VectorDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let ox = rect.pos.x as f32;
        let oy = rect.pos.y as f32;

        self.draw_vector.begin();

        // Shadow behind rounded rect
        self.draw_vector.set_color(0.0, 0.0, 0.0, 0.7);
        self.draw_vector.set_shape_id(0.0);
        self.draw_vector
            .shadow(ox + 40.0, oy + 40.0, 200.0, 120.0, 16.0, 16.0, 6.0, 8.0);

        // 1. Linear gradient filled rounded rectangle
        self.draw_vector.set_paint(VectorPaint::linear_gradient(
            ox + 40.0,
            oy + 40.0,
            ox + 240.0,
            oy + 160.0,
            vec![
                GradientStop::from_hex(0.0, 0x3498db, 1.0),
                GradientStop::from_hex(1.0, 0xe74c3c, 1.0),
            ],
        ));
        self.draw_vector.set_shape_id(1.0);
        self.draw_vector
            .rounded_rect(ox + 40.0, oy + 40.0, 200.0, 120.0, 16.0);
        self.draw_vector.fill();

        // 2. Stroked circle
        self.draw_vector.set_color(0.9, 0.3, 0.2, 1.0);
        self.draw_vector.set_shape_id(2.0);
        self.draw_vector.circle(ox + 400.0, oy + 100.0, 60.0);
        self.draw_vector.stroke(3.0);

        // 3. Radial gradient filled circle
        self.draw_vector.set_paint(VectorPaint::radial_gradient(
            ox + 400.0,
            oy + 100.0,
            40.0,
            vec![
                GradientStop::from_hex(0.0, 0xf1c40f, 1.0),
                GradientStop::from_hex(1.0, 0x2ecc71, 1.0),
            ],
        ));
        self.draw_vector.set_shape_id(3.0);
        self.draw_vector.circle(ox + 400.0, oy + 100.0, 40.0);
        self.draw_vector.fill();

        // 4. Bezier curve stroke
        self.draw_vector.set_color(1.0, 0.8, 0.2, 1.0);
        self.draw_vector.set_shape_id(4.0);
        self.draw_vector.move_to(ox + 40.0, oy + 250.0);
        self.draw_vector.bezier_to(
            ox + 100.0,
            oy + 180.0,
            ox + 250.0,
            oy + 320.0,
            ox + 350.0,
            oy + 250.0,
        );
        self.draw_vector.stroke(4.0);

        // Shadow behind star (geometry-based, follows star shape)
        self.draw_vector.set_color(0.0, 0.0, 0.0, 0.6);
        self.draw_vector.set_shape_id(0.0);
        {
            let scx = ox + 600.0 + 5.0;
            let scy = oy + 100.0 + 7.0;
            let souter = 50.0;
            let sinner = 22.0;
            for i in 0..10 {
                let a = std::f32::consts::PI * 2.0 * i as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
                let r = if i % 2 == 0 { souter } else { sinner };
                let px = scx + a.cos() * r;
                let py = scy + a.sin() * r;
                if i == 0 {
                    self.draw_vector.move_to(px, py);
                } else {
                    self.draw_vector.line_to(px, py);
                }
            }
            self.draw_vector.close();
            self.draw_vector.shape_shadow(14.0);
        }

        // 5. Star shape with linear gradient
        self.draw_vector.set_paint(VectorPaint::linear_gradient(
            ox + 550.0,
            oy + 50.0,
            ox + 650.0,
            oy + 150.0,
            vec![
                GradientStop::from_hex(0.0, 0xe67e22, 1.0),
                GradientStop::from_hex(0.5, 0xf39c12, 1.0),
                GradientStop::from_hex(1.0, 0xe74c3c, 1.0),
            ],
        ));
        self.draw_vector.set_shape_id(5.0);
        let star_cx = ox + 600.0;
        let star_cy = oy + 100.0;
        let outer = 50.0;
        let inner = 22.0;
        for i in 0..10 {
            let a = std::f32::consts::PI * 2.0 * i as f32 / 10.0 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { outer } else { inner };
            let px = star_cx + a.cos() * r;
            let py = star_cy + a.sin() * r;
            if i == 0 {
                self.draw_vector.move_to(px, py);
            } else {
                self.draw_vector.line_to(px, py);
            }
        }
        self.draw_vector.close();
        self.draw_vector.fill();

        // 6. Ellipse stroke
        self.draw_vector.set_color(0.7, 0.4, 0.9, 1.0);
        self.draw_vector.set_shape_id(6.0);
        self.draw_vector.ellipse(ox + 600.0, oy + 260.0, 80.0, 40.0);
        self.draw_vector.stroke(2.5);

        // 7. Triangle with radial gradient
        self.draw_vector.set_paint(VectorPaint::radial_gradient(
            ox + 140.0,
            oy + 380.0,
            60.0,
            vec![
                GradientStop::from_hex(0.0, 0x1abc9c, 1.0),
                GradientStop::from_hex(1.0, 0x2c3e50, 1.0),
            ],
        ));
        self.draw_vector.set_shape_id(7.0);
        self.draw_vector.move_to(ox + 40.0, oy + 400.0);
        self.draw_vector.line_to(ox + 140.0, oy + 340.0);
        self.draw_vector.line_to(ox + 240.0, oy + 420.0);
        self.draw_vector.close();
        self.draw_vector.fill();

        // 8. Nested rounded rects (dashed strokes)
        self.draw_vector.set_color(0.8, 0.8, 0.8, 0.8);
        self.draw_vector.set_shape_id(8.0);
        for i in 0..4 {
            let inset = i as f32 * 18.0;
            self.draw_vector.rounded_rect(
                ox + 310.0 + inset,
                oy + 330.0 + inset,
                220.0 - inset * 2.0,
                140.0 - inset * 2.0,
                (14.0 - inset * 0.7).max(2.0),
            );
            self.draw_vector.stroke(2.0);
        }

        // Shadow behind gradient bar
        self.draw_vector.set_color(0.0, 0.0, 0.0, 0.7);
        self.draw_vector.set_shape_id(0.0);
        self.draw_vector
            .shadow(ox + 40.0, oy + 460.0, 300.0, 40.0, 8.0, 12.0, 5.0, 6.0);

        // 9. Multi-stop linear gradient bar
        self.draw_vector.set_paint(VectorPaint::linear_gradient(
            ox + 40.0,
            oy + 480.0,
            ox + 340.0,
            oy + 480.0,
            vec![
                GradientStop::from_hex(0.0, 0xe74c3c, 1.0),
                GradientStop::from_hex(0.25, 0xf39c12, 1.0),
                GradientStop::from_hex(0.5, 0x2ecc71, 1.0),
                GradientStop::from_hex(0.75, 0x3498db, 1.0),
                GradientStop::from_hex(1.0, 0x9b59b6, 1.0),
            ],
        ));
        self.draw_vector.set_shape_id(9.0);
        self.draw_vector
            .rounded_rect(ox + 40.0, oy + 460.0, 300.0, 40.0, 8.0);
        self.draw_vector.fill();

        self.draw_vector.end(cx);

        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// ---- SVG Demo Widget ----

const SVG_SOURCE: &str = include_str!("../resources/test.svg");

#[derive(Script, ScriptHook, Widget)]
pub struct SvgDemo {
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
        // Parse SVG on first draw
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
        // Drive animation by requesting redraws each frame
        if let Event::NextFrame(ne) = event {
            self.time = ne.time;
            self.area.redraw(cx);
        }
        if let Event::Startup = event {
            cx.new_next_frame();
        }
    }
}
