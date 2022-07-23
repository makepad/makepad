use crate::{
    makepad_platform::*,
    frame_traits::*,
    fold_button::*
};

live_register!{
    FoldHeader: {{FoldHeader}} {
        walk: {
            width: Size::Fill,
            height: Size::Fit
        }
        body_walk: {
            width: Size::Fill,
            height: Size::Fit
        }
        layout: {
            flow: Flow::Down,
        }
        
        state: {
            open = {
                default: on
                off = {
                    from: {all: Play::Exp {speed1: 0.96, speed2: 0.97}}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                    }
                }
                on = {
                    from: {all: Play::Exp {speed1: 0.98, speed2: 0.95}}
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
#[live_register(frame_component!(FoldHeader))]
pub struct FoldHeader {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    header: FrameRef,
    body: FrameRef,
    
    state: State,
    opened: f32,
    
    view: View,
    layout: Layout,
    walk: Walk,
    body_walk: Walk,
}

#[derive(Clone)]
enum DrawState {
    DrawHeader,
    DrawBody
}

impl FrameComponent for FoldHeader {
    fn handle_component_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FramePath, Box<dyn FrameAction>)
    ) {
        if self.state_handle_event(cx, event).must_redraw() {
            if self.state.is_track_animating(cx, id!(open)) {
                let rect = self.view.get_rect(cx);
                self.view.set_scroll_pos(cx, vec2(0.0, rect.size.y * (1.0 - self.opened)));
                self.view.redraw(cx);
            }
        };
        
        for item in self.header.handle_event_iter(cx, event) {
            if item.id() == id!(fold_button) {
                match item.action.cast() {
                    FoldButtonAction::Opening => {
                        self.animate_state(cx, ids!(open.on))
                    }
                    FoldButtonAction::Closing => {
                        self.animate_state(cx, ids!(open.off))
                    }
                    _ => ()
                }
            }
            dispatch_action(cx, item.path, item.action)
        }
        
        self.body.handle_component_event(cx, event, dispatch_action);
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        self.header.redraw(cx);
        self.body.redraw(cx);
    }
    
    fn get_walk(&self) -> Walk {self.walk}
    
    fn query_child(&mut self, query: &QueryChild, callback: &mut Option<&mut dyn FnMut(QueryInner)>) -> QueryResult{
        self.header.query_child(query, callback)?;
        self.body.query_child(query, callback)
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk, self_uid: FrameUid) -> DrawResult {
        if self.draw_state.begin(cx, DrawState::DrawHeader) {
            cx.begin_turtle(walk, self.layout);
        }
        if let DrawState::DrawHeader = self.draw_state.get() {
            self.header.draw_walk_component(cx) ?;
            if self.view.begin(cx, self.body_walk, Layout::flow_down()).is_err() {
                self.reverse_walk_opened(cx);
                cx.end_turtle();
                self.draw_state.end();
                return DrawResult::Done
            };
            self.draw_state.set(DrawState::DrawBody);
        }
        if let DrawState::DrawBody = self.draw_state.get() {
            self.body.draw_walk_component(cx) ?;
            self.view.end(cx);
            // reverse walk
            self.reverse_walk_opened(cx);
            cx.end_turtle();
            self.draw_state.end();
        }
        DrawResult::Done
    }
}

impl FoldHeader {
    fn reverse_walk_opened(&mut self, cx: &mut Cx2d) {
        let rect = self.view.get_rect(cx);
        cx.walk_turtle(Walk::size(Size::Fill, Size::Negative(rect.size.y * (1.0 - self.opened))));
    }
}

#[derive(Clone, FrameAction)]
pub enum FoldHeaderAction {
    Opening,
    Closing,
    None
}