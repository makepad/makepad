
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    use makepad_draw::shader::std::*;
    
    DrawCandleStick = {{DrawCandleStick}} {
        fn pixel(self) -> vec4 {
            //return mix(#f00,#0f0, left+0.5);
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            // first we darw a line from min to max
            // then we draw a box from open to close
            
            return sdf.result
        }
    }
    
    CandleStick = {{CandleStick}} {
        width: Fill,
        height: Fill
    }
}

#[derive(Live, LiveHook, LiveRegister)]#[repr(C)]
struct DrawCandleStick {
    #[deref] draw_super: DrawQuad,
    #[calc] open: f32,
    #[calc] close: f32
}

struct _CandleStickData{
    min: f64,
    max: f64,
    open: f64,
    close: f64,
}

#[derive(Live, LiveHook, Widget)]
pub struct CandleStick {
    #[walk] walk: Walk,
    #[redraw] #[live] draw_cs: DrawCandleStick,
    #[rust(Texture::new(cx))] _data_texture: Texture,
    #[rust] _screen_view: Rect,
    #[rust] _data_view: Rect
}

impl Widget for CandleStick {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_cs.draw_walk(cx, walk);
        DrawStep::done()
    }
}

#[derive(Clone, DefaultNone)]
pub enum CandleStickAction {
    None
}

impl CandleStick {
    pub fn process_buffer(&mut self, _cx: &mut Cx) {
 
    }
}

impl CandleStick {
    
    pub fn handle_event_with(&mut self, _cx: &mut Cx, _event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, CandleStickAction),) {
    }
}

impl CandleStickRef {
    pub fn process_buffer(&self, cx: &mut Cx ){
        if let Some(mut inner) = self.borrow_mut() {
            inner.process_buffer(cx);
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize) {
    }
}

impl CandleStickSet {
    pub fn process_buffer(&self, cx: &mut Cx) {
        for item in self.iter(){
            item.process_buffer(cx);
        }
    }
}
