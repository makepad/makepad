use crate::makepad_draw::*;

live_design!{
    TabCloseButtonBase = {{TabCloseButton}} {}
}

#[derive(Live, LiveHook)]
pub struct TabCloseButton {
    #[live] draw_button: DrawQuad,
    #[animator] animator: Animator,

    #[walk] walk: Walk
}

impl TabCloseButton {
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_button.draw_walk(
            cx,
            self.walk
        );
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
    ) -> TabCloseButtonAction {
        self.animator_handle_event(cx, event);
        match event.hits(cx, self.draw_button.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
                return TabCloseButtonAction::HoverIn;
            }
            Hit::FingerHoverOut(_)=>{
                self.animator_play(cx, id!(hover.off));
                return TabCloseButtonAction::HoverOut;
            }
            Hit::FingerDown(_) => return TabCloseButtonAction::WasPressed,
            _ => {}
        }
        TabCloseButtonAction::None
    }
}

pub enum TabCloseButtonAction {
    None,
    WasPressed,
    HoverIn,
    HoverOut,
}
