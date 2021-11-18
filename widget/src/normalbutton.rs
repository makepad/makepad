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
            from: {
                all: Play::Forward {duration: 0.1} // from everything to default
            }
            bg: {
                down: 0.0,
                hover: 0.0
            }
        }
        
        state_hover: {
            bg: {
                down: [{time:1.0, value: 0.0, ease: Ease::Linear}],
                hover: 1.0
            }
        }
        
        state_down: {
            bg: {
                down: [{time: 1.0, value: 1.0, ease: Ease::Linear}],
                hover: 1.0,
            }
        }
    }
}

#[derive(LiveComponent)]
pub struct NormalButton {
    #[hidden()] pub button_logic: ButtonLogic,
    #[hidden()] pub animator: Animator,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout,
    #[live()] pub label: String
}

impl LiveComponentHooks for NormalButton {
    fn after_new(&mut self, _cx: &mut Cx) {
    }
    
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(file_id) = apply_from.file_id() {
            self.animator.live_ptr = Some(LivePtr::from_index(file_id, index));
            // lets apply our initial state directly from the DSL structure without alloc
            if let Ok(index) = nodes.child_by_name(index, id!(state_default)){
                self.apply(cx, ApplyFrom::Animate, index, nodes);
            }
            
            if self.animator.state.is_none(){ // we have no current state, fetch default
                self.animator.cut_state_to(cx, id!(state_default));
            }
            //self.animator.create_timeline_to(cx, id!(state_hover));
            //self.animator.create_timeline_to(cx, id!(state_default));
        }
    }
}

impl CanvasComponent for NormalButton {
    fn handle(&mut self, cx: &mut Cx, event: &mut Event) {
        self.handle_normal_button(cx, event);
    }
    
    fn draw(&mut self, cx: &mut Cx) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, &self.label);
        self.bg.end_quad(cx);
    }
}

impl NormalButton {
    
    pub fn init_state(&mut self, _cx: &mut Cx, _state_id: Id) {
        // take the live DSL and turn it into a Gen
        /*
        let sub_ptr = cx.find_class_prop_ptr(self.animator.live_ptr.unwrap(), state_id);
        let mut state = Vec::new();
        GenNode::convert_live_to_gen(cx, sub_ptr.unwrap(), &mut state);

        // take the Gen and sample the last keyframe
        self.animator.init_from_last_keyframe(cx, &state);
        
        // apply the last keyframe to self
        let state = self.animator.swap_out_state();
        self.apply(cx, &state);
        self.animator.swap_in_state(state);*/
    } 
    
    pub fn set_state(&mut self, cx: &mut Cx, state_id: Id) {
        // ok so we set state. we have a default state AND the state we need to go to
        if self.animator.state.is_none(){ // we have no current state, fetch default
            self.animator.cut_state_to(cx, id!(state_default));
        }
        self.animator.create_timeline_to(cx, state_id);
        self.animator.cut_state_to(cx, state_id);
        let state = self.animator.swap_out_state();
        self.apply(cx, ApplyFrom::Animate, 0, &state);
        self.animator.swap_in_state(state);
        cx.redraw_child_area(self.bg.draw_call_vars.area);
    }
    
    pub fn handle_normal_button(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonAction {
        let res = self.button_logic.handle_button_logic(cx, event, self.bg.draw_call_vars.area);
        match res.state {
            ButtonState::Down => self.set_state(cx, id!(state_down)),
            ButtonState::Default => self.set_state(cx, id!(state_default)),
            ButtonState::Over => self.set_state(cx, id!(state_hover)),
            _ => ()
        };
        res.action
    }
    
    pub fn draw_normal_button(&mut self, cx: &mut Cx, label: &str) {
        self.bg.begin_quad(cx, self.layout);
        self.text.draw_text_walk(cx, label);
        self.bg.end_quad(cx);
    }
}
