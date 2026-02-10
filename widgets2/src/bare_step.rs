use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.BareStep = #(BareStep::register_widget(vm)){}
}

#[derive(Script, ScriptHook, Widget)]
pub struct BareStep {
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<()>,
}

impl Widget for BareStep {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            return DrawStep::make_step();
        }
        DrawStep::done()
    }
}
