use makepad_render::*;
use crate::buttonlogic::*;

live_register!{
    use makepad_render::shader_std::*;
    use makepad_render::turtle::*;
    use makepad_render::animation::*;
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
            play: Play::Cut{duration:1.0}
            bg: {
                down: [[1.0,0.0]],
                hover: 0.0
            }
        }
        
        state_over: {
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

#[derive(LiveComponent)]
pub struct NormalButton {
    #[hidden()] pub button_logic: ButtonLogic,
    #[hidden()] pub animator:Animator,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout,
    #[live()] pub label: String
}

impl LiveComponentHooks for NormalButton{
    fn after_live_update(&mut self, _cx: &mut Cx, live_ptr: LivePtr) {
        self.animator.live_ptr = Some(live_ptr);
    }
}

impl CanvasComponent for NormalButton{
    fn handle(&mut self, cx: &mut Cx, event:&mut Event){
        self.handle_normal_button(cx, event);
    }
    
    fn draw(&mut self, cx: &mut Cx){
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, &self.label);
        self.bg.end_quad(cx);
    }
}

impl NormalButton {
    
    pub fn set_live_state(&mut self, cx:&mut Cx, state_id:Id){

        let sub_ptr = cx.find_class_prop_ptr(self.animator.live_ptr.unwrap(), state_id);

        let mut state=Vec::new();
        GenNode::new_from_live_ptr(cx, sub_ptr.unwrap(), &mut state);

        // we can just implement an animation system on top of an array here

        self.apply(cx, &state);
        
        cx.redraw_child_area(self.bg.area);
    }
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        let res = self.button_logic.handle_button_logic(cx, event, self.bg.area);
        match res.state{
            ButtonState::Down => self.set_live_state(cx, id!(state_down)),
            ButtonState::Default => self.set_live_state(cx, id!(state_default)),
            ButtonState::Over => self.set_live_state(cx, id!(state_over)),
            _=>()
        };
        res.action
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, label);
        self.bg.end_quad(cx);
    }
}
