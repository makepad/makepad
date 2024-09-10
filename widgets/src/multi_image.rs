use crate::{
    makepad_derive_widget::*,
    image_cache::*,
    makepad_draw::*,
    widget::*
};

live_design!{
    MultiImageBase = {{MultiImage}} {}
}
 
#[derive(Live, Widget)]
pub struct MultiImage {
    #[walk] walk: Walk,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] min_width: i64,
    #[live] min_height: i64,
    #[live(1.0)] width_scale: f64,
    #[live] fit: ImageFit,
    #[live] source1: LiveDependency,
    #[live] source2: LiveDependency,
    #[live] source3: LiveDependency,
    #[live] source4: LiveDependency,
    #[rust] textures: [Option<Texture>;4],
}

impl ImageCacheImpl for MultiImage {
    fn get_texture(&self, id:usize) -> &Option<Texture> {
        &self.textures[id]
    }
    
    fn set_texture(&mut self, texture: Option<Texture>, id:usize) {
        self.textures[id] = texture;
    }
}

impl LiveHook for MultiImage{
    fn after_apply(&mut self, cx: &mut Cx, _applyl: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        self.lazy_create_image_cache(cx);
        for (i,source) in [self.source1.clone(), self.source2.clone(), self.source2.clone(), self.source3.clone()].iter().enumerate(){
            if source.as_str().len()>0 {
                let _ = self.load_image_dep_by_path(cx, source.as_str(), i);
            }
        }
    }
}

impl Widget for MultiImage {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_walk(cx, walk)
    }
}

impl MultiImage {
    /// Returns the original size of the image in pixels (not its displayed size).
    ///
    /// Returns `None` if the image has not been loaded into a texture yet.
    pub fn size_in_pixels(&self, cx: &mut Cx) -> Option<(usize, usize)> {
        self.textures[0].as_ref()
            .and_then(|t| t.get_format(cx).vec_width_height())
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, mut walk: Walk) -> DrawStep {
        // alright we get a walk. depending on our aspect ratio
        // we change either nothing, or width or height
        let rect = cx.peek_walk_turtle(walk);
        let dpi = cx.current_dpi_factor();
        for i in 0..self.textures.len(){        
            if let Some(image_texture) = &self.textures[i]{
                self.draw_bg.draw_vars.set_texture(i, image_texture);
            }
        }
        
        let (width, height) = if let Some(image_texture) = &self.textures[0]{
            let (width,height) = image_texture.get_format(cx).vec_width_height().unwrap_or((self.min_width as usize, self.min_height as usize));
            (width as f64 * self.width_scale, height as f64)
        }
        else {
            self.draw_bg.draw_vars.empty_texture(0);
            (self.min_width as f64 / dpi, self.min_height as f64 / dpi)
        };
        
        let aspect = width / height;
        match self.fit {
            ImageFit::Size => {
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
}
