
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    
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

#[derive(Live, LiveHook)]#[repr(C)]
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

#[derive(Live)]
pub struct CandleStick {
    #[walk] walk: Walk,
    #[live] draw_cs: DrawCandleStick,
    #[rust(Texture::new(cx))] _data_texture: Texture,
    #[rust] _screen_view: Rect,
    #[rust] _data_view: Rect
}

impl Widget for CandleStick {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)
    ) {
        let uid = self.widget_uid();
        self.handle_event_with(cx, event, &mut | cx, action | {
            dispatch_action(cx, WidgetActionItem::new(action.into(), uid));
        });
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_cs.redraw(cx)
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        let _ = self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, WidgetAction)]
pub enum CandleStickAction {
    None
}

impl LiveHook for CandleStick {
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, CandleStick)
    }
    
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {

    }
}

impl CandleStick {
    pub fn process_buffer(&mut self, _cx: &mut Cx) {
 
    }
}

impl CandleStick {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_cs.draw_walk(cx, walk);
    }
    
    pub fn handle_event_with(&mut self, _cx: &mut Cx, _event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, CandleStickAction),) {
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, PartialEq, WidgetRef)]
pub struct CandleStickRef(WidgetRef);

impl CandleStickRef {
    pub fn process_buffer(&self, cx: &mut Cx ){
        if let Some(mut inner) = self.borrow_mut() {
            inner.process_buffer(cx);
        }
    }
    
    pub fn voice_off(&self, _cx: &mut Cx, _voice: usize) {
    }
}

// ImGUI convenience API for Piano
#[derive(Clone, WidgetSet)]
pub struct CandleStickSet(WidgetSet);

impl CandleStickSet {
    pub fn process_buffer(&self, cx: &mut Cx) {
        for item in self.iter(){
            item.process_buffer(cx);
        }
    }
}
