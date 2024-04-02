use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*
};

live_design!{
    ImageBlendBase = {{ImageBlend}} {}
} 
  
#[derive(Live, Widget)]
pub struct ImageBlend {
    #[walk] walk: Walk,
    #[animator] animator:Animator,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] min_width: i64,
    #[live] min_height: i64,
    #[live(1.0)] width_scale: f64,
    #[live] fit: ImageFit,
    #[live] breathe: bool,
    #[live] source: LiveDependency,
    #[rust] texture: [Option<Texture>;2]
}

impl ImageCacheImpl for ImageBlend {
    fn get_texture(&self, id:usize) -> &Option<Texture> {
        &self.texture[id]
    }
    
    fn set_texture(&mut self, texture: Option<Texture>, id:usize) {
        if let Some(texture) = &texture{
            self.draw_bg.draw_vars.set_texture(id, texture);
        }
        else{ 
            self.draw_bg.draw_vars.empty_texture(id);
        }
        self.texture[id] = texture;
    }
}

impl LiveHook for ImageBlend{
    fn after_apply(&mut self, cx: &mut Cx, _applyl: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        self.lazy_create_image_cache(cx);
        let source = self.source.clone();
        if source.as_str().len()>0 {
            let _ = self.load_image_dep_by_path(cx, source.as_str(), 0);
        }
    }
    fn after_new_from_doc(&mut self, cx: &mut Cx){
        if self.breathe{
            self.animator_play(cx, id!(breathe.on));
        }
    }
}

impl Widget for ImageBlend {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
    }
}

impl ImageBlend {
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        let (width, height) = if let Some(image_texture) = &self.texture[0] {
            let (width,height) = image_texture.get_format(cx).vec_width_height().unwrap_or((self.min_width as usize, self.min_height as usize));
            (width as f64 * self.width_scale, height as f64)
        }
        else {
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };
                
        let aspect = width / height;
        match self.fit {
            ImageFit::Size=>{
                walk.width = Size::Fixed(width);
                walk.height = Size::Fixed(height);
            }
            ImageFit::Stretch => {
            }
            ImageFit::Horizontal => {
                walk.height = Size::Fixed(rect.size.x / aspect);
            }
            ImageFit::Vertical => {
                walk.width = Size::Fixed(rect.size.y * aspect);
            }
            ImageFit::Smallest => {
                let walk_height = rect.size.x / aspect;
                if walk_height > rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
            ImageFit::Biggest => {
                let walk_height = rect.size.x / aspect;
                if walk_height < rect.size.y {
                    walk.width = Size::Fixed(rect.size.y * aspect);
                }
                else {
                    walk.height = Size::Fixed(walk_height);
                }
            }
        }
        self.draw_bg.draw_walk(cx, walk);
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
    /// Loads the image at the given `image_path` into this `ImageRef`.
    pub fn load_image_dep_by_path(&self, cx: &mut Cx, image_path: &str) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            let slot = inner.flip_animate(cx);
            inner.load_image_dep_by_path(cx, image_path, slot)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    /// Loads a JPEG into this `ImageRef` by decoding the given encoded JPEG `data`.
    pub fn load_jpg_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            let slot = inner.flip_animate(cx);
            inner.load_jpg_from_data(cx, data, slot)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    /// Loads a PNG into this `ImageRef` by decoding the given encoded PNG `data`.
    pub fn load_png_from_data(&self, cx: &mut Cx, data: &[u8]) -> Result<(), ImageError> {
        if let Some(mut inner) = self.borrow_mut() {
            let slot = inner.flip_animate(cx);
            inner.load_png_from_data(cx, data, slot)
        } else {
            Ok(()) // preserving existing behavior of silent failures.
        }
    }
    
    pub fn set_texture(&self, cx:&mut Cx, texture: Option<Texture>) {
        if let Some(mut inner) = self.borrow_mut() {
            
            let slot = inner.flip_animate(cx); 
            inner.set_texture(texture, slot);
        }
    }
    
    pub fn set_uniform(&self, cx: &Cx, uniform: &[LiveId], value: &[f32]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_bg.set_uniform(cx, uniform, value);
        }
    }    
}

