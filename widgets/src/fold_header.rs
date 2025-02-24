use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    fold_button::*
};

live_design!{
    link widgets;
    use link::theme::*;
    
    pub FoldHeaderBase = {{FoldHeader}} {}
    pub FoldHeader = <FoldHeaderBase> {
        width: Fill, height: Fit,
        body_walk: { width: Fill, height: Fit}
        
        flow: Down,
        
        animator: {
            active = {
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

#[derive(Live, LiveHook, Widget)]
pub struct FoldHeader {
    #[rust] draw_state: DrawStateWrap<DrawState>,
    #[rust] rect_size: f64,
    #[rust] area: Area,
    #[find] #[redraw] #[live] header: WidgetRef,
    #[find] #[redraw] #[live] body: WidgetRef,
    #[animator] animator: Animator,

    #[live] opened: f64,
    #[layout] layout: Layout,
    #[walk] walk: Walk,
    #[live] body_walk: Walk,
}

#[derive(Clone)]
enum DrawState {
    DrawHeader,
    DrawBody
}

impl Widget for FoldHeader {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            if self.animator.is_track_animating(cx, id!(active)) {
                self.area.redraw(cx);
            }
        };
        
        self.header.handle_event(cx,  event, scope);
        
        if let Event::Actions(actions) = event{
            match actions.find_widget_action(self.header.widget(id!(fold_button)).widget_uid()).cast() {
                FoldButtonAction::Opening => {
                    self.animator_play(cx, id!(active.on))
                }
                FoldButtonAction::Closing => {
                    self.animator_play(cx, id!(active.off))
                }
                _ => ()
            }
        }
    }

    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
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
        DrawStep::done()
    }
}

#[derive(Clone, DefaultNone)]
pub enum FoldHeaderAction {
    Opening,
    Closing,
    None
}