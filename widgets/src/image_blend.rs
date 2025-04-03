use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*,
    image::Image,
};

live_design!{
    link widgets;
    use link::widgets::*;
    use link::shaders::*;
    
    pub ImageBlendBase = {{ImageBlend}} {}
        
    pub ImageBlend = <ImageBlendBase> {
        image_a: <Image>{
            fit: Smallest
            width: Fill,
            height: Fill
        },
        image_b: <Image>{
            fit: Smallest
            width: Fill,
            height: Fill
        },
        width: Fill,
        height: Fill,
        flow: Overlay
        align: {x: 0.5, y: 0.5}
        animator: {
            blend = {
                default: zero,
                zero = {
                    from: {all: Forward {duration: 0.525}}
                    apply: {
                        image_b:{draw_bg:{opacity: 0.0}}
                    }
                }
                one = {
                    from: {
                        all: Forward {duration: 0.525}
                    }
                    apply: {
                        image_b:{draw_bg:{opacity: 1.0}}
                    }
                }
            } 
        }
    }
          
} 
  
#[derive(Live, Widget, LiveHook)]
pub struct ImageBlend {
    #[animator] animator:Animator,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] image_a: Image,
    #[redraw] #[live] image_b: Image,
}

impl ImageCacheImpl for ImageBlend {
    fn get_texture(&self, id:usize) -> &Option<Texture> {
        if id == 0 {
            self.image_a.get_texture(0)
        }
        else{
            self.image_b.get_texture(0)
        }
    }
    
    fn set_texture(&mut self, texture: Option<Texture>, id:usize) {
        if id == 0{
            self.image_a.set_texture(texture, 0)
        }
        else{
            self.image_b.set_texture(texture, 0)
        }
    }
}

impl Widget for ImageBlend {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk)
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.image_a.redraw(cx);
            self.image_b.redraw(cx);
        }
        self.image_a.handle_event(cx, event, scope);
        self.image_b.handle_event(cx, event, scope);
    }
}

impl ImageBlend {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        self.image_a.draw_all_unscoped(cx);
        self.image_b.draw_all_unscoped(cx);
        cx.end_turtle();
        DrawStep::done()
    }
    
    fn flip_animate(&mut self, cx: &mut Cx)->usize{
        if self.animator_in_state(cx, id!(blend.one)) {
            self.animator_play(cx, id!(blend.zero));
            0
        }
        else{
            self.animator_play(cx, id!(blend.one));
            1
        }
    }
}

impl ImageBlendRef {
     pub fn switch_image(&self, cx:&mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.flip_animate(cx);
        }
    }
    pub fn set_texture(&self, cx:&mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            
            let slot = inner.flip_animate(cx); 
            inner.set_texture(texture, slot);
        }
    }
    
}

