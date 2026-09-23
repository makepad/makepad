//! An animated emissive source for the relief buffer: a plasma rendered into
//! its own pass while it runs, shown on a small screen, and registered as a
//! texture light so the controls around the screen take its colours.
//!
//! Tapping the screen pauses it; paused, it renders nothing and the relief
//! buffer does no work.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawPlasma::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * vec2(3.2, 1.3)
            let t = self.phase
            let c1 = vec2(1.6 + sin(t * 0.53) * 1.1, 0.65 + cos(t * 0.41) * 0.45)
            let v = sin(p.x * 2.4 + t)
                + sin(p.y * 4.1 - t * 1.27)
                + sin((p.x + p.y) * 1.9 + t * 0.71)
                + sin(length(p - c1) * 4.6 - t * 1.6)
            let a = v * 0.9 + t * 0.25
            // A cool palette: deep blue through violet to cyan, hot where the
            // waves meet.
            let col = vec3(0.35 + 0.35 * sin(a + 2.6), 0.25 + 0.3 * sin(a + 0.4), 0.6 + 0.4 * sin(a))
            let hot = pow(max(v * 0.25, 0.0), 3.0)
            return vec4(col * col + vec3(hot, hot * 0.8, hot), 1.0)
        }
    }

    set_type_default() do #(DrawPlasmaScreen::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        radius: uniform(6.0)
        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
            let d = Material.sd_box(p, c, c, self.radius)
            var col = self.image.sample(self.pos).xyz
            // Glass: faint scanlines, a darker rim, a soft reflection.
            let line = 0.92 + 0.08 * sin(p.y * 3.14159 * self.draw_pass.dpi_factor)
            let vig = 1.0 - smoothstep(-24.0, 0.0, d) * 0.55
            col = col * line * vig + vec3(0.05, 0.06, 0.07) * smoothstep(0.62, 0.0, self.pos.y) * 0.6
            let cov = 1.0 - smoothstep(-px, px, d)
            return vec4(col * cov, cov)
        }
    }

    mod.widgets.PlasmaScreenBase = #(PlasmaScreen::register_widget(vm))
    mod.widgets.PlasmaScreen = set_type_default() do mod.widgets.PlasmaScreenBase{
        width: 300
        height: 120
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlasma {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    phase: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPlasmaScreen {
    #[deref]
    draw_super: DrawQuad,
}

struct Target {
    pass: DrawPass,
    list: DrawList2d,
    texture: Texture,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct PlasmaScreen {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_plasma: DrawPlasma,
    #[live]
    draw_screen: DrawPlasmaScreen,
    /// How strongly the screen lights its surroundings.
    #[live(3.0)]
    light: f64,
    #[rust]
    area: Area,
    #[rust]
    target: Option<Target>,
    #[rust(true)]
    running: bool,
    #[rust]
    phase: f64,
    #[rust]
    last_clock: Option<f64>,
    /// Size of the last rendered frame; a new size renders even when paused.
    #[rust]
    rendered: Vec2d,
    #[rust]
    next_frame: NextFrame,
}

impl WidgetNode for PlasmaScreen {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for PlasmaScreen {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.running {
            self.area.redraw(cx);
        }
        if let Hit::FingerUp(fe) = event.hits(cx, self.area) {
            if fe.is_over {
                self.set_running(cx, !self.running);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x < 1.0 || rect.size.y < 1.0 {
            return DrawStep::done();
        }
        let target = self.target.get_or_insert_with(|| {
            let pass = DrawPass::new_with_name(cx.cx, "plasma");
            let list = DrawList2d::new(cx.cx);
            let texture = Texture::new_with_format(
                cx.cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            pass.set_color_texture(cx.cx, &texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
            Target { pass, list, texture }
        });
        let clock = cx.seconds_since_app_start();
        let dt = self
            .last_clock
            .replace(clock)
            .map(|last| (clock - last).clamp(0.0, 0.1))
            .unwrap_or(0.0);
        // Half density is plenty for a light source and a soft screen.
        let size = (rect.size * 0.5).floor();
        if self.running || self.rendered != size {
            if self.running {
                self.phase += dt;
            }
            self.rendered = size;
            target.pass.set_size(cx, size);
            cx.begin_pass(&target.pass, Some(1.0));
            target.list.begin_always(cx);
            let pass_size = cx.current_pass_size();
            cx.begin_root_turtle(pass_size, Layout::flow_overlay());
            self.draw_plasma.phase = self.phase as f32;
            self.draw_plasma.draw_abs(
                cx,
                Rect {
                    pos: dvec2(0.0, 0.0),
                    size: pass_size,
                },
            );
            cx.end_pass_sized_turtle();
            target.list.end(cx);
            cx.end_pass(&target.pass);
        }
        cx.make_child_pass(&target.pass);
        self.draw_screen.draw_vars.set_texture(0, &target.texture);
        self.draw_screen.draw_abs(cx, rect);
        relief_texture_light(
            cx,
            self.draw_screen.draw_vars.area,
            &ReliefTextureLight {
                texture: target.texture.clone(),
                producer: target.pass.draw_pass_id(),
                intensity: self.light as f32,
                knee: 3.0,
                depth: 0.0,
                inset: 0.0,
                radius: 6.0,
            },
        );
        if self.running {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl PlasmaScreen {
    pub fn set_running(&mut self, cx: &mut Cx, running: bool) {
        if self.running != running {
            self.running = running;
            self.last_clock = None;
            self.area.redraw(cx);
        }
    }
}
