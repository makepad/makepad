use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    fold_button::*
};

live_design!{
    FoldHeaderBase = {{FoldHeader}} {}
}

#[derive(Live)]
pub struct FoldHeader {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] rect_size: f64,
    #[rust] area: Area,
    #[live] header: WidgetRef,
    #[live] body: WidgetRef,
    #[animator] animator: Animator,

    #[live] opened: f64,
    #[layout] layout: Layout,
    #[walk] walk: Walk,
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
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut WidgetScope)->WidgetActions {
        let mut actions = WidgetActions::new();
        if self.animator_handle_event(cx, event).must_redraw() {
            if self.animator.is_track_animating(cx, id!(open)) {
                self.area.redraw(cx);
            }
        };
        
        for item in self.header.handle_event(cx,  event, scope).iter() {
            if item.widget_uid_eq(self.header.widget(id!(fold_button)).widget_uid()){
                match item.cast() {
                    FoldButtonAction::Opening => {
                        self.animator_play(cx, id!(open.on))
                    }
                    FoldButtonAction::Closing => {
                        self.animator_play(cx, id!(open.off))
                    }
                    _ => ()
                }
            }
            actions.push(item.clone());
        }
        
        actions.extend(self.body.handle_event(cx, event, scope));
        return actions
    }
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.header.redraw(cx);
        self.body.redraw(cx);
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}

    fn find_widgets(&mut self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.header.find_widgets(path, cached, results);
        self.body.find_widgets(path, cached, results);
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut WidgetScope, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, DrawState::DrawHeader) {
            cx.begin_turtle(walk, self.layout);
        }
        if let Some(DrawState::DrawHeader) = self.draw_state.get() {
            let walk = self.header.walk(cx);
            self.header.draw_walk(cx, scope, walk) ?;
            cx.begin_turtle(
                self.body_walk,
                Layout::flow_down()
                .with_scroll(dvec2(0.0, self.rect_size * (1.0 - self.opened)))
            );
            self.draw_state.set(DrawState::DrawBody);
        }
        if let Some(DrawState::DrawBody) = self.draw_state.get() {
            let walk = self.body.walk(cx);
            self.body.draw_walk(cx, scope, walk) ?;
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