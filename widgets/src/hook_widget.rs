use {
    crate::{
        makepad_draw::*,
        widget::*
    }
};
live_design!{
    import makepad_draw::shader::std::*;
    
    HookWidget= {{HookWidget}} {
        walk: {
            width: Fit,
            height: Fit,
            margin: {left: 1.0, right: 1.0, top: 1.0, bottom: 1.0},
        }
        layout: {
            align: {x: 0.5, y: 0.5},
            padding: {left: 14.0, top: 10.0, right: 14.0, bottom: 10.0}
        }
    }
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

    fn get_walk(&self)->Walk{
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
