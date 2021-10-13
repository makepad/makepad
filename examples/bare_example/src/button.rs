
use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone, LiveComponent)]
pub struct NormalButton {
    pub live_node: LiveNode,
    pub logic: ButtonLogic,
    pub bg: DrawQuad,
    pub text: DrawText,
}
// components also connect to the Rust component


register_live!{
    // includes all the basic types
    use makepad_render::prelude::*;
    
    // this is the serialized props of NormalButton
    NormalButton: Component {
        live_layout: AutoLayout {
            align: Align::center,
            walk: Walk {
                width: Width::Compute,
                height: Height::Compute,
                margin: Margin {l: 1.0, t: 1.0, r: 1.0, b: 1.0},
            },
            padding: {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
        }
        
        bg: makepad_render::drawquad::DrawQuad {
            instance hover: float;
            instance down: float;
            
            const shadow: float = 3.0;
            const border_radius: float = 2.5;
            
            fn pixel(self) -> vec4 {
                let cx = Df::viewport(pos * rect_size);
                cx.box(shadow, shadow, rect_size.x - shadow * (1. + down), rect_size.y - shadow * (1. + down), border_radius);
                cx.blur = 6.0;
                cx.fill(mix(#x0007, #x0, self.hover));
                cx.blur = 0.001;
                cx.box(shadow, shadow, rect_size.x - shadow * 2., rect_size.y - shadow * 2., border_radius);
                return cx.fill(mix(mix(#x3, #x4, hover), #x2a, self.down));
            }
        }
        
        text: makepad_render::drawtext::DrawText {
            text_style: crate::widgetstyle::text_style_normal,
            color: #xfff0
        };
        
        state_default: Self {
            bg.color: #xfff
            text.color: #x0ff
        }
        
        state_hover: Self {
            bg.down: 0.0;
            bg.hover: 1.0;
        }
        
        state_down: Self {
            bg.down: 1.0;
            bg.hover: 1.0;
        }
    }
}

impl NormalButton {
    
    pub fn handle_normal_button(&mut self, id:u64, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        // this contains all the info we need
        self.handle_live_node(cx, event);
        
        let (le, be) = self.logic.handle_button_logic(cx, event, self.bg.area);
        match le{
            ButtonLogicEvent::Down => self.to_state(cx, id!(state_down)),
            ButtonLogicEvent::Default => self.to_state(cx, id!(state_default)),
            ButtonLogicEvent::Over => self.to_state(cx, id!(state_over)),
            _=>()
        }
        be
    }

    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        self.begin_live_node(cx);
        
        self.bg.begin_quad(cx, self.live_node.layout);
        self.text.draw_text_walk(cx, label);
        self.bg.end_quad(cx);
        
        self.end_live_node(cx);
    }
}

