use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.BareStep = #(BareStep::register_widget(vm)){}
}

#[derive(Script, ScriptHook, Widget)]
pub struct BareStep {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<()>,
}

impl Widget for BareStep {
    fn semantics(&self, cx: &Cx, node: &mut a11y_tree::Node) {
        let _ = cx;
        node.role = a11y_tree::Role::Generic;
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            return DrawStep::make_step();
        }
        DrawStep::done()
    }
}
