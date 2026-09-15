//! The forecast's small drawn parts: the sunrise-to-sunset arc of the
//! sunrise card and the page dots under the forecast (one per city).
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    mod.widgets.DrawSolarArc = set_type_default() do #(DrawSolarArc::script_shader(vm)) {
        ..mod.draw.DrawQuad
        progress: -1.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let w = self.rect_size.x
            let h = self.rect_size.y
            let c = vec2(w * 0.5, h - 2.0)
            let r = w * 0.5 - 4.0
            let d = abs(length(p - c) - r)
            let above = step(p.y, c.y)
            let track = (1.0 - smoothstep(0.6, 1.6, d)) * above
            let mut col = vec4(1.0, 1.0, 1.0, 0.35) * track
            let horizon = (1.0 - smoothstep(0.4, 1.2, abs(p.y - c.y))) * 0.35
            col = mix(col, vec4(1.0, 1.0, 1.0, 0.45), horizon)
            if self.progress >= 0.0 {
                let a = 3.14159265 * (1.0 - self.progress)
                let s = c + vec2(cos(a), -sin(a)) * r
                let dot = 1.0 - smoothstep(3.0, 4.2, length(p - s))
                col = mix(col, vec4(1.0, 0.86, 0.35, 1.0), dot)
            }
            return col
        }
    }

    mod.widgets.SolarArcBase = #(SolarArc::register_widget(vm))
    mod.widgets.SolarArc = set_type_default() do mod.widgets.SolarArcBase{
        width: 80 height: 36
        draw_arc: mod.widgets.DrawSolarArc{}
    }

    mod.widgets.PageDotsBase = #(PageDots::register_widget(vm))
    mod.widgets.PageDots = set_type_default() do mod.widgets.PageDotsBase{
        width: 44 height: 16
        dot +: {
            color: #ffffff
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5)
                sdf.fill(self.color)
                return sdf.result
            }
        }
    }
}

/// The sunrise-to-sunset arc with the sun where the day stands.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSolarArc {
    #[deref]
    draw_super: DrawQuad,
    #[live(-1.0)]
    progress: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SolarArc {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_arc: DrawSolarArc,
}

impl SolarArc {
    pub fn set_progress(&mut self, cx: &mut Cx, progress: f32) {
        if self.draw_arc.progress != progress {
            self.draw_arc.progress = progress;
            self.draw_arc.redraw(cx);
        }
    }
}

impl Widget for SolarArc {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_arc.draw_walk(cx, walk);
        DrawStep::done()
    }
}

/// The page dots under the forecast: one per city, the chosen one solid.
#[derive(Script, ScriptHook, Widget)]
pub struct PageDots {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    dot: DrawColor,
    #[rust]
    count: usize,
    #[rust]
    active: usize,
}

impl PageDots {
    pub fn set(&mut self, cx: &mut Cx, count: usize, active: usize) {
        if self.count != count || self.active != active {
            self.count = count;
            self.active = active;
            self.redraw(cx);
        }
    }
}

impl Widget for PageDots {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let r = cx.turtle().rect();
        let n = self.count.max(1) as f64;
        let total = n * 6.0 + (n - 1.0) * 6.0;
        let x0 = r.pos.x + (r.size.x - total) * 0.5;
        let base = self.dot.color;
        for i in 0..self.count {
            let alpha = if i == self.active { 1.0 } else { 0.35 };
            self.dot.color = vec4(base.x, base.y, base.z, alpha);
            self.dot.draw_abs(cx, Rect { pos: dvec2(x0 + i as f64 * 12.0, r.pos.y + (r.size.y - 6.0) * 0.5), size: dvec2(6.0, 6.0) });
        }
        self.dot.color = base;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
