//! The backdrop widget: the wallpaper layer's procedural background.
use super::shader::DrawBackdrop;
use super::{Kind, WarpUniforms};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    mod.widgets.WmBackdropBase = #(WmBackdrop::register_widget(vm))
    mod.widgets.WmBackdrop = set_type_default() do mod.widgets.WmBackdropBase {
        width: Fill height: Fill
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct WmBackdrop {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawBackdrop,
    #[rust]
    area: Area,
    #[rust]
    kind: Option<Kind>,
    #[rust]
    pointer: Option<DVec2>,
}

impl WmBackdrop {
    /// Show a procedural background, or none (the image / gradient).
    pub fn set_kind(&mut self, cx: &mut Cx, kind: Option<Kind>) {
        self.kind = kind;
        self.draw_bg.redraw(cx);
    }
}

impl Widget for WmBackdrop {
    /// The background draws when something it shows changed (the desk redraws
    /// it then) and never on its own: no timer, no frame loop. Only the
    /// crystal floor, whose glints follow the pointer, asks for a redraw
    /// when the pointer moves.
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let (Some(kind), Event::MouseMove(e)) = (self.kind, event) {
            if self.pointer != Some(e.abs) {
                self.pointer = Some(e.abs);
                if kind.follows_pointer() {
                    self.draw_bg.redraw(cx);
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(kind) = self.kind else {
            return DrawStep::done();
        };
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        let env = scope.data.get_mut::<crate::desk::WmState>().map(|s| &s.backdrop);
        let warp = env
            .as_ref()
            .and_then(|e| e.warp_shown.or(e.warp).map(|w| w.hidden(e.hide)))
            .unwrap_or_else(|| WarpUniforms::default_for(rect.size.x, rect.size.y));
        let focus = env.as_ref().and_then(|e| e.focus);
        let d = &mut self.draw_bg;
        d.kind = kind as usize as f32;
        d.time = 0.0;
        d.aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
        d.warp_on = if warp.enabled { 1.0 } else { 0.0 };
        let wz = warp.in_rect(rect);
        d.warp = vec4(wz[0], wz[1], wz[2], wz[3]);
        let (sx, sy) = (rect.size.x.max(1.0), rect.size.y.max(1.0));
        if let Some(e) = env.as_ref() {
            let r = e.drag_rect;
            d.tgt = vec4(
                ((r[0] as f64 - rect.pos.x) / sx) as f32,
                ((r[1] as f64 - rect.pos.y) / sy) as f32,
                ((r[2] as f64 - rect.pos.x) / sx) as f32,
                ((r[3] as f64 - rect.pos.y) / sy) as f32,
            );
            d.drag = vec4(e.drag_level, e.drag_absorb, 0.0, 0.0);
        }
        d.focus_on = if focus.is_some() { 1.0 } else { 0.0 };
        if let Some(f) = focus {
            d.focus = vec4(
                ((f.pos.x - rect.pos.x) / sx) as f32,
                ((f.pos.y - rect.pos.y) / sy) as f32,
                ((f.pos.x + f.size.x - rect.pos.x) / sx) as f32,
                ((f.pos.y + f.size.y - rect.pos.y) / sy) as f32,
            );
        }
        d.pointer = match self.pointer {
            Some(p) if rect.contains(p) => vec2(((p.x - rect.pos.x) / sx) as f32, ((p.y - rect.pos.y) / sy) as f32),
            _ => vec2(-1.0, -1.0),
        };
        let l = kind.light_dir();
        d.light = vec4(l[0], l[1], l[2], 0.0);
        d.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
