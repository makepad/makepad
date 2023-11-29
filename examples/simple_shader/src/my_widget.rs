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

#[derive(Live)]
pub struct MyWidget {
    #[live] draw: DrawQuad,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live] time: f32,
    #[rust] next_frame: NextFrame,
}

impl LiveHook for MyWidget{
    fn before_live_design(cx:&mut Cx) {
        register_widget!(cx, MyWidget)
    }

    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        // starts the animation cycle on startup
        self.next_frame = cx.new_next_frame();
    }
}

#[derive(Clone, WidgetAction)]
pub enum MyWidgetAction {
    None
}

impl Widget for MyWidget{
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(),uid));
        });
    }

    fn walk(&mut self, _cx:&mut Cx)->Walk{
        self.walk
    }

    fn redraw(&mut self, cx:&mut Cx){
        self.draw.redraw(cx)
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

impl MyWidget {

    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, MyWidgetAction)) {
        if let Some(ne) = self.next_frame.is_event(event) {
            // update time to use for animation
            self.time = (ne.time * 0.001).fract() as f32;
            // force updates, so that we can animate in the absence of user-generated events
            self.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw.begin(cx, walk, self.layout);
        self.draw.end(cx);
    }
}