use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
live_design! {
    TurtleStep = {{TurtleStep}} {}
}

#[derive(Live, LiveHook, Widget)]
pub struct TurtleStep {
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] area: Area,
    #[rust] draw_state: DrawStateWrap<()>
}

impl Widget for TurtleStep {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            cx.begin_turtle(self.walk, self.layout);
            return DrawStep::make_step()
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}