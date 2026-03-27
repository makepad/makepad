use makepad_widgets::makepad_draw::text::geom::Point as TextPoint;
use makepad_widgets::makepad_draw::text::rasterizer::RasterizedGlyph;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let GlyphPlacementDemoBase = #(GlyphPlacementDemo::register_widget(vm))
    let GlyphPlacementDemo = set_type_default() do GlyphPlacementDemoBase{
        width: Fill
        height: Fill
        draw_bg: mod.draw.DrawColor{color: #x1f1f21}
        draw_guides: mod.draw.DrawColor{color: #fff}
        label_text.text_style: theme.font_regular{font_size: 18}
        label_text.color: #fff
        glyph_text.text_style: theme.font_regular{font_size: 32}
        glyph_text.color: #fff
        glyph_text.temp_y_shift: 0.25
    }

    let AppUi = SolidView{
        width: Fill
        height: Fill
        draw_bg.color: #x1f1f21
        flow: Down
        spacing: 10
        padding: 10

        controls := RoundedView{
            width: Fill
            height: Fit
            flow: Down
            spacing: 8
            padding: 12
            draw_bg.color: #x2a2a2e
            draw_bg.radius: 6.0

            Label{text: "Controls" draw_text.color: #fff draw_text.text_style.font_size: 13}
            shift_slider := Slider{width: Fill text: "temp_y_shift" min: -0.5 max: 0.5 default: 0.25}
            font_size_slider := Slider{width: Fill text: "glyph font size" min: 16.0 max: 72.0 default: 32.0}
            baseline_slider := Slider{width: Fill text: "baseline y" min: 100.0 max: 260.0 default: 180.0}
        }

        demo := GlyphPlacementDemo{}
    }

    mod.gc.set_static(AppUi)
    mod.gc.run()

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(0.15 0.15 0.15 1.0)
                window.inner_size: vec2(1200 640)
                window.title: "Glyph Placement Example"
                body +: {
                    app := AppUi{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(0.25)]
    shift: f32,
    #[rust(32.0)]
    glyph_font_size: f32,
    #[rust(180.0)]
    baseline_y: f64,
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

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(value) = self.ui.slider(cx, ids!(shift_slider)).slided(actions) {
            self.shift = value as f32;
            let demo_widget = self.ui.widget(cx, ids!(demo));
            if let Some(mut demo) = demo_widget.borrow_mut::<GlyphPlacementDemo>() {
                demo.glyph_text.temp_y_shift = self.shift;
                demo.redraw(cx);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(font_size_slider)).slided(actions) {
            self.glyph_font_size = value as f32;
            let demo_widget = self.ui.widget(cx, ids!(demo));
            if let Some(mut demo) = demo_widget.borrow_mut::<GlyphPlacementDemo>() {
                demo.glyph_text.text_style.font_size = self.glyph_font_size;
                demo.glyphs.clear();
                demo.redraw(cx);
            };
        }
        if let Some(value) = self.ui.slider(cx, ids!(baseline_slider)).slided(actions) {
            self.baseline_y = value;
            let demo_widget = self.ui.widget(cx, ids!(demo));
            if let Some(mut demo) = demo_widget.borrow_mut::<GlyphPlacementDemo>() {
                demo.baseline_y = self.baseline_y;
                demo.redraw(cx);
            };
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct GlyphPlacementDemo {
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
    draw_guides: DrawColor,
    #[live]
    label_text: DrawText,
    #[live]
    glyph_text: DrawText,
    #[rust]
    area: Area,
    #[rust]
    glyphs: Vec<(TextPoint<f32>, f32, RasterizedGlyph)>,
    #[live(180.0)]
    baseline_y: f64,
}

impl GlyphPlacementDemo {
    fn ensure_glyphs(&mut self, cx: &mut Cx2d) {
        self.glyphs.clear();
        let Some(run) = self.glyph_text.prepare_single_line_run(cx, "AgjpQy") else {
            return;
        };
        self.glyphs = run
            .glyphs
            .iter()
            .map(|glyph| {
                (
                    TextPoint::new(glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs, 0.0),
                    glyph.font_size_in_lpxs,
                    glyph.rasterized,
                )
            })
            .collect();
    }

    fn draw_panel(&mut self, cx: &mut Cx2d, rect: Rect, title: &str, exact: bool) {
        self.draw_bg.color = vec4(0.18, 0.18, 0.2, 1.0);
        self.draw_bg.draw_abs(cx, rect);

        self.label_text.color = vec4(1.0, 1.0, 1.0, 1.0);
        self.label_text.draw_abs(cx, rect.pos + dvec2(16.0, 34.0), title);

        let baseline_y = rect.pos.y + self.baseline_y;
        let content_x = rect.pos.x + 32.0;
        let content_w = (rect.size.x - 64.0).max(1.0);

        self.draw_guides.color = vec4(0.2, 0.65, 1.0, 1.0);
        self.draw_guides.draw_abs(
            cx,
            Rect {
                pos: dvec2(content_x, baseline_y),
                size: dvec2(content_w, 2.0),
            },
        );

        for (origin, _, _) in &self.glyphs {
            self.draw_guides.color = vec4(1.0, 0.35, 0.35, 1.0);
            self.draw_guides.draw_abs(
                cx,
                Rect {
                    pos: dvec2(content_x + origin.x as f64 - 2.0, baseline_y + origin.y as f64 - 2.0),
                    size: dvec2(5.0, 5.0),
                },
            );
        }

        let positioned: Vec<_> = self
            .glyphs
            .iter()
            .map(|(origin, font_size, rasterized)| {
                (
                    TextPoint::new(content_x as f32 + origin.x, baseline_y as f32 + origin.y),
                    *font_size,
                    *rasterized,
                )
            })
            .collect();

        if exact {
            self.glyph_text
                .draw_rasterized_glyphs_exact_abs(cx, &positioned, vec4(1.0, 1.0, 1.0, 1.0));
        } else {
            self.glyph_text
                .draw_rasterized_glyphs_abs(cx, &positioned, vec4(1.0, 1.0, 1.0, 1.0));
        }

        self.label_text.color = vec4(0.75, 0.75, 0.78, 1.0);
        self.label_text.draw_abs(
            cx,
            rect.pos + dvec2(16.0, rect.size.y - 22.0),
            if exact {
                "Exact API: supplied glyph origin is used directly."
            } else {
                "Existing API: temp_y_shift still moves the glyph quad."
            },
        );
    }
}

impl Widget for GlyphPlacementDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.color = vec4(0.12, 0.12, 0.13, 1.0);
        self.draw_bg.draw_abs(cx, rect);
        self.ensure_glyphs(cx);

        let title = "draw_rasterized_glyphs_abs vs draw_rasterized_glyphs_exact_abs";
        self.label_text.color = vec4(1.0, 1.0, 1.0, 1.0);
        self.label_text.draw_abs(cx, rect.pos + dvec2(20.0, 34.0), title);
        self.label_text.color = vec4(0.7, 0.7, 0.72, 1.0);
        let subtitle = format!(
            "Blue line = requested baseline. Red squares = supplied glyph origins. temp_y_shift = {:.2}, font size = {:.1}, baseline = {:.0}. String = AgjpQy.",
            self.glyph_text.temp_y_shift,
            self.glyph_text.text_style.font_size,
            self.baseline_y
        );
        self.label_text.draw_abs(cx, rect.pos + dvec2(20.0, 68.0), &subtitle);

        let gap = 20.0;
        let panel_top = rect.pos.y + 96.0;
        let panel_h = (rect.size.y - 116.0).max(220.0);
        let panel_w = ((rect.size.x - gap * 3.0) * 0.5).max(240.0);
        let left = Rect {
            pos: dvec2(rect.pos.x + gap, panel_top),
            size: dvec2(panel_w, panel_h),
        };
        let right = Rect {
            pos: dvec2(rect.pos.x + gap * 2.0 + panel_w, panel_top),
            size: dvec2(panel_w, panel_h),
        };

        self.draw_panel(cx, left, "Widget-adjusted", false);
        self.draw_panel(cx, right, "Exact", true);

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}
