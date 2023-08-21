use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    fold_button::*
};

live_design!{
    FoldHeader= {{FoldHeader}} {
        walk: {
            width: Fill,
            height: Fit
        }
        body_walk: {
            width: Fill,
            height: Fit
        }
        layout: {
            flow: Down,
        }
        state: {
            open = {
                default: on
                off = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                    }
                }
                on = {
                    from: {all: Forward {duration: 0.2}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]
                    }
                }
            }
        }
    }
}

#[derive(Live)]
pub struct FoldHeader {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] rect_size: f64,
    #[rust] area: Area,
    #[live] header: WidgetRef,
    #[live] body: WidgetRef,
    #[state] state: LiveState,
    #[live] opened: f64,
    #[live] layout: Layout,
    #[live] walk: Walk,
    #[live] body_walk: Walk,
}

impl LiveHook for FoldHeader{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx,FoldHeader)
    }
}

#[derive(Clone)]
enum DrawState {
    DrawHeader,
    DrawBody
}

impl Widget for FoldHeader {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            if self.state.is_track_animating(cx, id!(open)) {
                self.area.redraw(cx);
            }
        };
        
        for item in self.header.handle_widget_event(cx, event) {
            if item.widget_uid == self.header.get_widget(id!(fold_button)).widget_uid(){
                match item.action.cast() {
                    FoldButtonAction::Opening => {
                        self.animate_state(cx, id!(open.on))
                    }
                    FoldButtonAction::Closing => {
                        self.animate_state(cx, id!(open.off))
                    }
                    _ => ()
                }
            }
            dispatch_action(cx, item)
        }
        
        self.body.handle_widget_event_with(cx, event, dispatch_action);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.header.redraw(cx);
        self.body.redraw(cx);
    }
    
    fn get_walk(&self) -> Walk {self.walk}

    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.header.find_widgets(path, cached, results);
        self.body.find_widgets(path, cached, results);
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::DrawHeader) {
            cx.begin_box(walk, self.layout);
        }
        if let Some(DrawState::DrawHeader) = self.draw_state.get() {
            self.header.draw_widget(cx) ?;
            cx.begin_box(
                self.body_walk,
                Layout::flow_down()
                .with_scroll(dvec2(0.0, self.rect_size * (1.0 - self.opened)))
            );
            self.draw_state.set(DrawState::DrawBody);
        }
        if let Some(DrawState::DrawBody) = self.draw_state.get() {
            self.body.draw_widget(cx) ?;
            self.rect_size = cx.r#box().used().y;
            cx.end_box();
            cx.end_box_with_area(&mut self.area);
            self.draw_state.end();
        }
        WidgetDraw::done()
    }
}

#[derive(Clone, WidgetAction)]
pub enum FoldHeaderAction {
    Opening,
    Closing,
    None
}