use crate::makepad_draw::*;

live_design!{
    link widgets;
    use link::theme::*;
    use makepad_draw::shader::std::*;
    
    pub TabCloseButtonBase = {{TabCloseButton}} {}
    
    pub TabCloseButton = <TabCloseButtonBase> {
        // TODO: NEEDS FOCUS STATE
        height: 10.0, width: 10.0,
        margin: { right: (THEME_SPACE_2), left: -3.5 },
        draw_button: {
            
            instance hover: float;
            instance selected: float;
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let mid = self.rect_size / 2.0;
                let size = (self.hover * 0.25 + 0.5) * 0.25 * length(self.rect_size);
                let min = mid - vec2(size);
                let max = mid + vec2(size);
                sdf.move_to(min.x, min.y);
                sdf.line_to(max.x, max.y);
                sdf.move_to(min.x, max.y);
                sdf.line_to(max.x, min.y);
                return sdf.stroke(mix(
                    THEME_COLOR_TEXT_INACTIVE,
                    THEME_COLOR_TEXT_HOVER,
                    self.hover
                ), 1.0);
            }
        }
        
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_button: {hover: 0.0}
                    }
                }
                
                on = {
                    cursor: Hand,
                    from: {all: Snap}
                    apply: {
                        draw_button: {hover: 1.0}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, LiveRegister)]
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
