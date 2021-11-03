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

pub struct LiveStates{
    state_id: Id,
    live_ptr: Option<LivePtr>,
}

impl LiveStates{
    pub fn new(state_id:Id)->Self{
        Self{
            state_id,
            live_ptr: None,
        }
    }
    pub fn init_live_ptr(&mut self, live_ptr: LivePtr)->bool{
        if self.live_ptr.is_none(){
            self.live_ptr = Some(live_ptr);
            true
        }
        else{
            false
        }
    }
    pub fn state_live_ptr(&self, cx:&Cx)->Option<LivePtr>{
        cx.scan_live_ptr(self.live_ptr.unwrap(), self.state_id)
    }
}

#[derive(Live)]
pub struct NormalButton {
    #[hidden()] pub button_logic: ButtonLogic,
    #[hidden(LiveStates::new(id!(state_default)))] pub live_states:LiveStates,
    #[live()] pub bg: DrawQuad,
    #[live()] pub text: DrawText,
    #[live()] pub layout: Layout
}

impl LiveUpdateHooks for NormalButton {
    fn before_live_update(&mut self, _cx: &mut Cx, _live_ptr: LivePtr){
        
    }
    fn after_live_update(&mut self, cx: &mut Cx, live_ptr: LivePtr){
        if self.live_states.init_live_ptr(live_ptr){
            self.set_live_state(cx, id!(state_default));
        }
    }
}

impl NormalButton {
    
    pub fn set_live_state(&mut self, cx:&mut Cx, state_id:Id){
        self.live_states.state_id = state_id;
        if let Some(ptr) = self.live_states.state_live_ptr(cx){
            self.live_update(cx, ptr);
            cx.redraw_child_area(self.bg.area);
        }
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