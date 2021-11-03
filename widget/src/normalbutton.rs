use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
    use makepad_render::turtle::*;
    use makepad_render::drawquad::DrawQuad;
    use makepad_render::drawtext::DrawText;
    
    NormalButton: Component {
        rust_type: {{NormalButton}}
        bg: DrawQuad {
            instance color: vec4 = #333
            instance hover: float
            instance down: float
            
            const shadow: float = 3.0
            const border_radius: float = 2.5
            
            fn pixel(self) -> vec4 {
                let cx = Sdf2d::viewport(self.pos * self.rect_size);
                cx.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * (1. + self.down),
                    self.rect_size.y - shadow * (1. + self.down),
                    border_radius
                );
                cx.blur = 6.0;
                cx.fill(mix(#0007, #0, self.hover));
                cx.blur = 0.001;
                cx.box(
                    shadow,
                    shadow,
                    self.rect_size.x - shadow * 2.,
                    self.rect_size.y - shadow * 2.,
                    border_radius
                );
                return cx.fill(mix(mix(#3, #4, self.hover), #2a, self.down));
            }
        }
        
        text: DrawText {}
        
        layout: Layout {
            align: Align {fx: 0.5, fy: 0.5},
            walk: Walk {
                width: Width::Compute,
                height: Height::Compute,
                margin: Margin {l: 100.0, r: 1.0, t: 100.0, b: 1.0},
            }
            padding: Padding {l: 16.0, t: 12.0, r: 16.0, b: 12.0}
        }
        
        state_default: {
            bg: {
                down: 0.0
                hover: 0.0
            }
        }
        
        state_hover: {
            bg: {
                down: 0.0
                hover: 1.0
            }
        }
        
        state_down: {
            bg: {
                down: 1.0
                hover: 1.0
            }
        }
    }
}

#[derive(Live)]
pub struct NormalButton {
    #[hidden()] pub button_logic: ButtonLogic,
    #[hidden(id!(state_default))] pub state_id:Id,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout
}

impl LiveUpdateHooks for NormalButton {
    fn before_live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr) -> LivePtr {
        // so we can deserialize a 'state' again, over the base.
        // just like we do with animations.
        
    }
    
    fn after_live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr){
        //println!("{:?}", node);
    }
}

impl NormalButton {
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        // OK SO.. how are we going to do this state tweening.
        // essentially we need to look up 'state_bla'
        // and then create a delta against the current state
        //
        
        self.button_logic.handle_button_logic(cx, event, self.bg.area, | _cx, logic_event, _ | match logic_event {
            ButtonLogicEvent::Down => (),
            ButtonLogicEvent::Default => (),
            ButtonLogicEvent::Over => ()
        })
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, label);
        self.bg.end_quad(cx);
    }
}