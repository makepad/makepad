//! A test pattern for a compositor's view of this window (lane WMSYNC). In device pixels of its
//! quad (the window body):
//! - a 1-px green frame on the quad's outermost pixels (a missing or doubled edge shows),
//! - a 1-px black/white checkerboard whose phase flips every frame (a filtered or offset
//!   sample turns grey; two frames mixed in one picture show a phase seam),
//! - the frame number, 16 bits of 8x8 px red(1)/blue(0) blocks, X//!   top, bit 0 leftmost) and bottom-right (row 4..12 from the bottom, bit 0 rightmost) on grey
//!   bands of 16 px: a picture whose two stamps differ is torn or incomplete.
//!
//! `local/agent_state/relax-campaign/wmsync-tools/framecheck.py` decodes a grab of it.
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    set_type_default() do #(DrawProbe::script_shader(vm)){
        ..mod.draw.DrawQuad
        frame: 0.0
        phase: 0.0

        pixel: fn() {
            let dpi = self.draw_pass.dpi_factor
            let size = floor(self.rect_size * dpi + vec2(0.5, 0.5))
            let q = floor(self.pos * self.rect_size * dpi)
            if q.x < 1.0 || q.y < 1.0 || q.x >= size.x - 1.0 || q.y >= size.y - 1.0 {
                return vec4(0.0, 1.0, 0.0, 1.0)
            }
            let top = q.y < 16.0
            let bottom = q.y >= size.y - 16.0
            if top || bottom {
                let row = if top { q.y } else { size.y - 1.0 - q.y }
                let col = if top { q.x - 48.0 } else { size.x - 1.0 - q.x - 48.0 }
                if row >= 4.0 && row < 12.0 && col >= 0.0 && col < 128.0 {
                    let bit = floor(col / 8.0)
                    let v = floor(self.frame / pow(2.0, bit))
                    if v - 2.0 * floor(v / 2.0) > 0.5 {
                        return vec4(1.0, 0.0, 0.0, 1.0)
                    }
                    return vec4(0.0, 0.0, 1.0, 1.0)
                }
                return vec4(0.5, 0.5, 0.5, 1.0)
            }
            let c = q.x + q.y + self.phase
            if c - 2.0 * floor(c / 2.0) > 0.5 {
                return vec4(1.0, 1.0, 1.0, 1.0)
            }
            return vec4(0.0, 0.0, 0.0, 1.0)
        }
    }

    let ProbeBase = #(Probe::register_widget(vm))
    let Probe = set_type_default() do ProbeBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1001, 667)
                pass.clear_color: #x000000
                body +: {
                    probe := Probe{}
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
pub struct DrawProbe {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    frame: f32,
    #[live]
    phase: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Probe {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawProbe,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    area: Area,
    #[rust]
    frame: u32,
}

impl Widget for Probe {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                self.frame = (self.frame + 1) & 0xffff;
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
        self.draw_bg.frame = self.frame as f32;
        // FRAME_PROBE_STATIC=1: the checker keeps one phase (tells a spatial resample from frames mixed in time)
        self.draw_bg.phase = if std::env::var_os("FRAME_PROBE_STATIC").is_some() { 0.0 } else { self.frame as f32 };
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        if self.frame % 240 == 0 {
            let dpi = cx.current_dpi_factor();
            log!("frame-probe: frame {} body {:.3}x{:.3} at ({:.3},{:.3}) dpi {dpi} = {:.3}x{:.3} px",
                self.frame, rect.size.x, rect.size.y, rect.pos.x, rect.pos.y, rect.size.x * dpi, rect.size.y * dpi);
        }
        DrawStep::done()
    }
}
