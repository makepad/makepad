use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
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
            from: {all: Play::Forward {duration: 0.1}}
            bg: {down: 0.0, hover: 0.0}
            text: {color: #9}
        }
        
        state_hover: {
            from: {
                all: Play::Forward {duration: 0.1}
                state_down: Play::Forward {duration: 0.01}
            }
            bg: { 
                down: 0.0,
                hover: [{time: 0.0, value: 1.0}],
            } 
            text: {color: [{time: 0.0, value: #f}]}
        }
         
        state_down: {
            from: {all: Play::Forward {duration: 0.2}}
            bg: {
                down: [{time: 0.0, value: 1.0}],
                hover: 1.0,
            }
            text: {color: [{time: 0.0, value: #c}]}
        }
    }
}

#[derive(LiveComponent, LiveComponentHooks, LiveAnimate)]
pub struct NormalButton {
    #[hidden()] pub button_logic: ButtonLogic,
    #[hidden()] pub animator: Animator,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout,
    #[live()] pub label: String
}

impl CanvasComponent for NormalButton {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event) {
        self.handle_normal_button(cx, event);
    }
    
    fn draw(&mut self, cx: &mut Cx) {
        self.draw_normal_button(cx, None);
    }
}

impl NormalButton {
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        self.handle_animation(cx, event);
        let res = self.button_logic.handle_button_logic(cx, event, self.bg.draw_vars.area);
        match res.state {
            ButtonState::Down => self.animate_to(cx, id!(state_down)),
            ButtonState::Default => self.animate_to(cx, id!(state_default)),
            ButtonState::Hover => self.animate_to(cx, id!(state_hover)),
            _ => ()
        };
        res.action
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: Option<&str>) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, label.unwrap_or(&self.label));
        self.bg.end_quad(cx);
        //self.bg.draw_vars.redraw(cx);
    }
}
