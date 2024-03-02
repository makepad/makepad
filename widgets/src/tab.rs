use {
    crate::{
        tab_close_button::{TabCloseButtonAction, TabCloseButton},
        makepad_draw::*,
    }
};

live_design!{
    TabBase = {{Tab}} {}
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct Tab {
    #[rust] is_selected: bool,
    #[rust] is_dragging: bool,
    
    #[live] draw_bg: DrawQuad,
    #[live] draw_name: DrawText,
    //#[live] draw_drag: DrawColor,
    
    #[animator] animator: Animator,
    
    #[live] close_button: TabCloseButton,
    
    // height: f32,
    
    #[live] hover: f32,
    #[live] selected: f32,
    
    #[live(10.0)] min_drag_dist: f64,
    
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    
}

pub enum TabAction {
    WasPressed,
    CloseWasPressed,
    ShouldTabStartDrag,
    ShouldTabStopDrag
    //DragHit(DragHit)
}

pub enum TabClosable{
    Yes,
    No
}

impl TabClosable{
    pub fn as_bool(&self)->bool{
        match self{
            Self::Yes=>true,
            Self::No=>false
        }
    }
}

impl Tab {
    
    pub fn is_selected(&self) -> bool {
        self.is_selected
    }
    
    pub fn set_is_selected(&mut self, cx: &mut Cx, is_selected: bool, animate: Animate) {
        self.is_selected = is_selected;
        self.animator_toggle(cx, is_selected, animate, id!(selected.on), id!(selected.off));
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, name: &str, closable:TabClosable) {
        //self.bg_quad.color = self.color(self.is_selected);
        self.draw_bg.begin(cx, self.walk, self.layout);
        //self.name_text.color = self.name_color(self.is_selected);
        if let TabClosable::Yes = closable{
            self.close_button.draw(cx);
        }
        //cx.turtle_align_y();
        self.draw_name.draw_walk(cx, Walk::fit(), Align::default(), name);
        //cx.turtle_align_y();
        self.draw_bg.end(cx);
        
        //if self.is_dragged {
        //    self.draw_drag.draw_abs(cx, self.draw_bg.area().get_clipped_rect(cx));
        //}
    }
    
    pub fn area(&self) -> Area {
        self.draw_bg.area()
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, TabAction),
    ) {
        self.animator_handle_event(cx, event);
        
        let mut block_hover_out = false;
        match self.close_button.handle_event(cx, event) {
            TabCloseButtonAction::WasPressed => dispatch_action(cx, TabAction::CloseWasPressed),
            TabCloseButtonAction::HoverIn => block_hover_out = true,
            TabCloseButtonAction::HoverOut => self.animator_play(cx, id!(hover.off)),
            _ => ()
        };
        
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => if !block_hover_out {
                self.animator_play(cx, id!(hover.off));
            }
            Hit::FingerMove(e) => {
                if !self.is_dragging && (e.abs - e.abs_start).length() > self.min_drag_dist {
                    self.is_dragging = true;
                    dispatch_action(cx, TabAction::ShouldTabStartDrag);
                }
            }
            Hit::FingerUp(_) => {
                if self.is_dragging {
                    dispatch_action(cx, TabAction::ShouldTabStopDrag);
                    self.is_dragging = false;
                }
            }
            Hit::FingerDown(_) => {
                dispatch_action(cx, TabAction::WasPressed);
            }
            _ => {}
        }
    }
}

