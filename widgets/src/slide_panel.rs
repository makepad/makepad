use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*,
    WidgetMatchEvent,
    WindowAction,
};

live_design!{
    link widgets
    pub SlidePanelBase = {{SlidePanel}} {}
    pub SlidePanel = <SlidePanelBase>{
        animator: {
            closed = {
                default: off,
                on = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.5}
                    }
                    ease: InQuad
                    apply: {
                        closed: 1.0
                    }
                }
                                
                off = {
                    redraw: true,
                    from: {
                        all: Forward {duration: 0.5}
                    }
                    ease: OutQuad
                    apply: {
                        closed: 0.0
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct SlidePanel {
    #[deref] frame: View,
    #[animator] animator: Animator,
    #[live] closed: f64,
    #[live] side: SlideSide,
    #[rust] screen_width: f64,
    #[rust] next_frame: NextFrame
}

#[derive(Clone, DefaultNone)]
pub enum SlidePanelAction {
    None,
}

impl Widget for SlidePanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        //let uid = self.widget_uid();
        self.frame.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
        // lets handle mousedown, setfocus
        if self.animator_handle_event(cx, event).must_redraw() {
            self.frame.redraw(cx);
        }
        
        match event {
            Event::NextFrame(ne) if ne.set.contains(&self.next_frame) => {
                self.frame.redraw(cx);
            }
            _ => ()
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, mut walk: Walk) -> DrawStep {
        // we need to make this thing work with a 
        let rect = cx.peek_walk_turtle(walk);
        match self.side{
            SlideSide::Top=>{
                walk.abs_pos = Some(dvec2(0.0, -rect.size.y * self.closed));
            }
            SlideSide::Left=>{
                walk.abs_pos = Some(dvec2(-rect.size.x * self.closed, 0.0));
            }
            SlideSide::Right => {
                walk.abs_pos = Some(dvec2(self.screen_width - rect.size.x + rect.size.x * self.closed, 0.0));
            }
        }
        self.frame.draw_walk(cx, scope, walk)
    }
}

impl WidgetMatchEvent for SlidePanel {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            if let WindowAction::WindowGeomChange(ce) = action.as_widget_action().cast() {
                self.screen_width = ce.new_geom.inner_size.x;
                self.redraw(cx);
            }
        }
    }
}

#[derive(Live, LiveHook)]
#[live_ignore]
pub enum SlideSide{
    #[pick] Left,
    Right,
    Top
}

impl SlidePanel {

    pub fn open(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
    
    pub fn close(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
}

impl SlidePanelRef {
    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, id!(closed.on))
        }
    }
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.animator_play(cx, id!(closed.off))
        }
    }
    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            if inner.animator_in_state(cx, id!(closed.on)){
                inner.animator_play(cx, id!(closed.off))
            }
            else{
                inner.animator_play(cx, id!(closed.on))
            }
        }
    }
    pub fn is_animating(&self, cx: &mut Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator.is_track_animating(cx, id!(closed))
        } else {
            false
        }
    }
}

impl SlidePanelSet {
    pub fn close(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.close(cx);
        }
    }
    pub fn open(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.open(cx);
        }
    }
}

