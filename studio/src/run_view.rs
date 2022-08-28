use crate::makepad_draw_2d::*;
use crate::build::build_manager::BuildState;

live_register!{
    import makepad_draw_2d::shader::std::*;

    DrawApp: {{DrawApp}} {
        texture tex: texture2d
        fn pixel(self) -> vec4 {
            //return vec4(self.max_iter / 1000.0,0.0,0.0,1.0);
            let fractal = sample2d_rt(self.tex, self.pos)
            return fractal;
        }
    }
    RunView: {{RunView}} {
    }
}

#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawApp {
    draw_super: DrawQuad,
}

#[derive(Live)]
pub struct RunView {
    bg_quad: DrawQuad,
    state: State,
    texture: Texture
}

impl LiveHook for RunView{
    fn after_new(&mut self, _cx:&mut Cx){
        
    }
}

impl RunView {
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.state_handle_event(cx, event);
    }
    
    pub fn redraw(&mut self, cx: &mut Cx) {
        self.bg_quad.area().redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, _state:&BuildState) {
        self.bg_quad.set_texture(0, &self.texture);
        self.bg_quad.draw_walk(cx, Walk::fit());
    }
}