//! The other file of the shader kinds (live_kinds.py): a lookup the window's shaders spread, as
//! renderers spread `mod.draw.PaintLookup`, and a draw shader type of its own, drawn by a widget's
//! field (left) and by one made from the type's default and held outside the tree (right), as a
//! renderer's lanes are. An edit of either colour must show in the next frames.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.LiveKindsTint = {
        swatch_tint: fn() -> vec4 {
            return vec4(0.0, 0.0, 1.0, 1.0)
        }
    }

    // the left half is this type's own colour, the right half the tint
    mod.draw.DrawLiveKindsSwatch = set_type_default() do #(DrawLiveKindsSwatch::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.widgets.LiveKindsTint,
        pixel: fn() {
            return mix(vec4(1.0, 0.0, 1.0, 1.0), self.swatch_tint(), step(0.5, self.pos.x))
        }
    }

    mod.widgets.LiveKindsSwatchBase = #(LiveKindsSwatch::register_widget(vm))
    mod.widgets.LiveKindsSwatch = set_type_default() do mod.widgets.LiveKindsSwatchBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLiveKindsSwatch {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct LiveKindsSwatch {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_swatch: DrawLiveKindsSwatch,
    #[rust]
    held: Option<DrawLiveKindsSwatch>,
}

impl Widget for LiveKindsSwatch {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let half = dvec2(rect.size.x * 0.5, rect.size.y);
        self.draw_swatch.draw_abs(cx, Rect { pos: rect.pos, size: half });
        if self.held.is_none() {
            self.held = cx.try_with_vm(DrawLiveKindsSwatch::script_new_with_default);
        }
        if let Some(held) = &mut self.held {
            held.draw_abs(cx, Rect { pos: dvec2(rect.pos.x + half.x, rect.pos.y), size: half });
        }
        DrawStep::done()
    }
}
