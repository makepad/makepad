use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.TurtleStep = set_type_default() do #(TurtleStep::register_widget(vm)){
        width: Fit
        height: Fit
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct TurtleStep {
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<()>,
}

impl Widget for TurtleStep {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            cx.begin_turtle(walk, self.layout);
            return DrawStep::make_step();
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
