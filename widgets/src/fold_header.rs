use crate::{
    makepad_derive_widget::*,
    makepad_draw_2d::*,
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

#[derive(Live, LiveHook)]
#[live_design_fn(widget_factory!(FoldHeader))]
pub struct FoldHeader {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] rect_size: f64,
    #[rust] area: Area,
    header: WidgetRef,
    body: WidgetRef,
    state: State,
    opened: f64,
    layout: Layout,
    walk: Walk,
    body_walk: Walk,
}

#[derive(Clone)]
enum DrawState {
    DrawHeader,
    DrawBody
}

impl Widget for FoldHeader {
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}

    fn handle_widget_event_fn(
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
        
        self.body.handle_widget_event_fn(cx, event, dispatch_action);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.header.redraw(cx);
        self.body.redraw(cx);
    }
    
    fn get_walk(&self) -> Walk {self.walk}

    fn find_widget(&mut self, path: &[LiveId], cached: WidgetCache) -> WidgetResult {
        self.header.find_widget(path, cached) ?;
        self.body.find_widget(path, cached)
    }
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::DrawHeader) {
            cx.begin_turtle(walk, self.layout);
        }
        if let DrawState::DrawHeader = self.draw_state.get() {
            self.header.draw_walk_widget(cx) ?;
            cx.begin_turtle(
                self.body_walk,
                Layout::flow_down()
                .with_scroll(dvec2(0.0, self.rect_size * (1.0 - self.opened)))
            );
            self.draw_state.set(DrawState::DrawBody);
        }
        if let DrawState::DrawBody = self.draw_state.get() {
            self.body.draw_walk_widget(cx) ?;
            self.rect_size = cx.turtle().used().y;
            cx.end_turtle();
            cx.end_turtle_with_area(&mut self.area);
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