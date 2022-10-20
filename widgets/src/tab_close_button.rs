use crate::makepad_draw_2d::*;

live_design!{
    import makepad_draw_2d::shader::std::*;
    import makepad_widgets::theme::*;
    
    TabCloseButton= {{TabCloseButton}} {
        button: {
            
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
                    COLOR_TEXT_DEFAULT,
                    COLOR_TEXT_HOVER,
                    self.hover
                ), 1.0);
            }
        }
        
        state:{
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        button: {hover: 0.0}
                    }
                }
                
                on =  {
                    cursor: Hand,
                    from: {all: Snap}
                    apply: {
                        button: {hover:1.0}
                    }
                }
            }
        }
        
        walk: {
            height: 10.0,
            width: 10.0,
            margin: {right: 5},
        },
    }
}

#[derive(Live, LiveHook)]
pub struct TabCloseButton {
    button: DrawQuad,
    state: State,

    walk: Walk
}

impl TabCloseButton {
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.button.draw_walk(
            cx,
            self.walk
        );
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
    ) -> TabCloseButtonAction {
        self.state_handle_event(cx, event);
        match event.hits(cx, self.button.area()) {
            Hit::FingerHoverIn(_) => {
                self.animate_state(cx, id!(hover.on));
                return TabCloseButtonAction::HoverIn;
            }
            Hit::FingerHoverOut(_)=>{
                self.animate_state(cx, id!(hover.off));
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
