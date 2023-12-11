use {
    crate::{
        makepad_widgets::makepad_derive_widget::*,
        makepad_widgets::makepad_draw::*,
        makepad_widgets::widget::*,
    }
};

// the "MyWidget" on the *left* hand side of the below is the name we will refer to the
// widget in the app's live_design block
live_design!{
    MyWidget = {{MyWidget}} {}
}

#[derive(Live, Widget)]
pub struct MyWidget {
    #[redraw] #[live] draw: DrawQuad,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live] time: f32,
    #[rust] next_frame: NextFrame,
}

impl LiveHook for MyWidget{
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
    }
}

#[derive(Clone, DefaultNone)]
pub enum MyWidgetAction {
    None
}

impl Widget for MyWidget{
    fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _scope: &mut Scope
    ){
        if let Some(ne) = self.next_frame.is_event(event) {
            // update time to use for animation
            self.time = (ne.time * 0.001).fract() as f32;
            // force updates, so that we can animate in the absence of user-generated events
            self.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw.begin(cx, walk, self.layout);
        self.draw.end(cx);
        DrawStep::done()
    }
}
