use {
    crate::{
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    import makepad_draw::shader::std::*;
    
    HookWidgetBase = {{HookWidget}} {}
}

#[derive(Live)]
pub struct HookWidget {
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[rust] draw_state: DrawStateWrap<DrawState>,
}

impl LiveHook for HookWidget{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,HookWidget)
    }
}

#[derive(Clone)]
enum DrawState {
    Hook,
}

impl Widget for HookWidget{
   fn handle_widget_event_with(
        &mut self,
        _cx: &mut Cx,
        _event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
    }

    fn walk(&self)->Walk{
        self.walk
    }
    
    fn redraw(&mut self, _cx:&mut Cx){}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, _walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::Hook) {
            return WidgetDraw::hook_above();
        }
        WidgetDraw::done()
    }
}
