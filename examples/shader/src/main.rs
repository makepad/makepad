use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    set_type_default() do #(DrawFullscreenShader::script_shader(vm)){
        ..mod.draw.DrawQuad
        time: 0.0

        pixel: fn() {
            let uv = self.pos * 2.0 - vec2(1.0, 1.0)
            let aspect = self.rect_size.x / max(self.rect_size.y, 0.0001)
            let p = vec2(uv.x * aspect, uv.y)
            let t = self.time * 1.35
            let angle = atan2(p.y, p.x)
            let radius = length(p)
            let ripple = sin(radius * 12.0 - t * 6.0 + angle * 5.0)
            let bands = 0.5 + 0.5 * sin(angle * 6.0 - t * 4.0 + radius * 10.0)
            let target = 0.42 + 0.07 * sin(t + angle * 3.0 + ripple * 0.4)
            let ring = clamp(1.0 - abs(radius - target) * 18.0, 0.0, 1.0)
            let glow = clamp(1.0 - radius * 0.85, 0.0, 1.0)
            let vignette = clamp(1.15 - radius * 0.75, 0.0, 1.0)
            let base = vec3(0.02, 0.04, 0.09)
                .mix(vec3(0.08, 0.34, 0.92), bands)
                .mix(vec3(1.05, 0.42, 0.16), ring * 0.9)
            let shimmer = vec3(0.10, 0.12, 0.18) * (0.5 + 0.5 * ripple)
            let color = (base + shimmer * glow + vec3(1.2, 0.5, 0.18) * ring) * vignette
            return vec4(color, 1.0)
        }
    }

    let FullscreenShaderBase = #(FullscreenShader::register_widget(vm))
    let FullscreenShader = set_type_default() do FullscreenShaderBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 720)
                pass.clear_color: #x03070d
                body +: {
                    shader := FullscreenShader{}
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
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFullscreenShader {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    time: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FullscreenShader {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawFullscreenShader,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    area: Area,
}

impl Widget for FullscreenShader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.draw_bg.time = ne.time as f32;
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }

        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
